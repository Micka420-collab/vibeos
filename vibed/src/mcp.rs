//! Minimal MCP-style JSON-RPC 2.0 server over the unix socket.
//!
//! Transport: line-delimited JSON (one request or response per line), which
//! is directly bridgeable to an MCP stdio client via `socat` (shipped in the
//! image, see `agent/README.md`).
//!
//! Call pipeline — no exceptions:
//!   1. path/service arguments are extracted and lexically normalized;
//!   2. the BUILT-IN hard denylist is checked in code, regardless of policy;
//!   3. `policy::evaluate(tool, tier, ctx)` -> Allow | Deny | RequireApproval
//!      (first matching rule wins, absolute default-deny, T2/T3 floor);
//!   4. `audit::record(...)` with the caller identity (SO_PEERCRED) —
//!      fail-closed on the Allow path;
//!   5. execution (only if allowed), then a second audit record with the
//!      final outcome.
//!
//! v0.1 honesty note: tools run in-process under `spawn_blocking`. Per-tool
//! sandboxing (systemd-run, seccomp, landlock) is Phase 3 — see ROADMAP.md.

use std::sync::Arc;

use serde::Deserialize;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tracing::{debug, info, warn};

use crate::audit::{AuditLog, Caller};
use crate::glob::{glob_match, normalize_path};
use crate::policy::{CallContext, Decision, PolicyEngine, Tier};

/// Memory store created by vibeos-genesis.service (see memory/genesis.sh).
pub const MEMORY_DIR: &str = "/var/lib/vibeos/memory";
/// MCP protocol revision advertised in `initialize`.
const PROTOCOL_VERSION: &str = "2024-11-05";
/// Hard cap on file content returned by fs.read.
const MAX_READ_BYTES: usize = 256 * 1024;
/// Hard cap on a single JSON-RPC request line read from the socket. A client
/// that sends a line longer than this (e.g. no newline ever) is disconnected,
/// so an unbounded line can never exhaust the daemon's memory (DoS guard).
const MAX_LINE_BYTES: usize = 1024 * 1024;
/// `O_NOFOLLOW` (Linux, `bits/fcntl-linux.h`): open() fails with ELOOP if the
/// final path component is a symbolic link, instead of silently following it.
/// Defined here to keep the crate free of a libc dependency.
const O_NOFOLLOW: i32 = 0x20000;
/// Per-file cap when scanning memory content in memory.query.
const MAX_MEMORY_SCAN_BYTES: usize = 64 * 1024;
/// Upper bound on files walked by memory.query.
const MAX_MEMORY_FILES: usize = 200;
/// Hard cap on one serialized memory.append line, newline included (anti-DoS:
/// an agent cannot balloon the memory store with a single call).
const MAX_APPEND_BYTES: usize = 16 * 1024;
/// Journal event types an AGENT may append via memory.append. The remaining
/// types of docs/MEMORY.md §3.5 (`genesis`, `boot`, `tool_call`, `purge`) are
/// reserved for the system itself (genesis.sh, vibed, vibectl) and refused.
const JOURNAL_AGENT_TYPES: [&str; 5] = [
    "observation",
    "decision",
    "preference",
    "project_seen",
    "error",
];
const JOURNAL_RESERVED_TYPES: [&str; 4] = ["genesis", "boot", "tool_call", "purge"];
/// Memory sub-scopes addressable by memory.query's `scope` argument, mapped to
/// their location in the store (relative path, is_directory). Keep in sync
/// with the layout in docs/MEMORY.md §3.
const MEMORY_SCOPES: [(&str, &str, bool); 6] = [
    ("identity", "identity.toml", false),
    ("hardware", "hardware.json", false),
    ("user", "user", true),
    ("projects", "projects", true),
    ("journal", "journal", true),
    ("knowledge", "knowledge", true),
];
/// fs.write (T1 modify-user) is confined to user-scope prefixes.
/// On Fedora bootc systems /home is a symlink to /var/home, so both spellings
/// must be accepted. Keep in sync with `security/policy.d/default.toml`
/// (rule fs-write-user) and vibed.service (`ReadWritePaths=/var/home`).
const USER_WRITE_PREFIXES: [&str; 2] = ["/home/", "/var/home/"];

/// BUILT-IN hard denylist, enforced in code regardless of what the loaded
/// policy says (defense in depth: a mistaken or tampered policy drop-in can
/// never expose these). Applies to reads AND writes.
/// Keep in sync with `docs/THREAT-MODEL.md` (S2) and the `denied` mirror in
/// `security/policy.d/default.toml`.
///
/// Glob semantics (see glob.rs): `*` matches within one path segment, `**`
/// matches zero or more whole segments (so `/proc/**/environ` also catches the
/// `/proc/<pid>/task/<tid>/environ` sibling-thread bypass).
const BUILTIN_DENY_ALWAYS: &[&str] = &[
    "/var/lib/vibeos/audit/**", // audit trail: agents must never see or probe it
    "/etc/shadow*",             // /etc/shadow and /etc/shadow- (password hashes)
    "/etc/gshadow*",            // group password hashes
    "**/.ssh/**",               // SSH key material, wherever it lives
    "**/.gnupg/**",             // GnuPG key material, wherever it lives
    "/etc/ssh/*",               // sshd config AND host keys
    "/etc/ssh/ssh_host_*",      // host private keys (explicit, in case of subdirs)
    "**/.aws/credentials",      // AWS access keys
    "**/.aws/config",           // AWS profiles / sso config
    "**/.config/gcloud/**",     // Google Cloud credentials store
    "/etc/NetworkManager/system-connections/**", // Wi-Fi/VPN PSKs and certs
    "**/.docker/config.json",   // registry auth tokens
    "**/.kube/config",          // kubernetes cluster credentials
    "**/.netrc",                // machine login credentials
    "/root/**",                 // root's home directory
    "/proc/*/environ",          // process environments may leak secrets
    "/proc/**/environ",         // ...including per-thread /proc/<pid>/task/<tid>/environ
    "/proc/**/cmdline",         // command lines may carry secrets/tokens
    "/run/credentials/**",      // decrypted systemd credentials
    "/boot/**",                 // boot chain is none of the agent's business
    // Credentials of the AI agents themselves and of the developer tooling
    // shipped in the image. fs.read is NOT confined to the caller's home
    // (vibed runs as root), so without these entries any agent could read
    // every user's agent tokens — including other users'.
    "**/.claude/**",               // Claude Code state: OAuth token, transcripts
    "**/.claude.json",             // Claude Code top-level config may carry keys
    "**/.config/gh/**",            // GitHub CLI hosts.yml (oauth_token)
    "**/.gemini/**",               // gemini-cli oauth_creds.json
    "**/.codex/**",                // codex auth.json
    "**/.local/share/opencode/**", // opencode auth.json + agent-internal state
    "**/.ollama/**",               // ollama keypair (id_ed25519)
    "**/.npmrc",                   // npm registry authTokens
    "**/.git-credentials",         // plaintext git credentials store
    "**/.config/sops/**",          // SOPS/age private keys (keys.txt)
];

/// Additional BUILT-IN denylist for WRITES only: agents may query the memory
/// through memory.query but never write it via fs.write — memory.append is
/// the governed write path (scope-based, no path argument, so this path
/// denylist cannot and need not apply to it) — and the policy itself is not
/// agent-writable.
const BUILTIN_DENY_WRITE: &[&str] = &["/etc/vibeos/policy.d/**", "/var/lib/vibeos/memory/**"];

/// Returns the matched pattern when `path` (already normalized) hits the
/// built-in denylist. `write` selects the additional write-only entries.
fn builtin_denied(path: &str, write: bool) -> Option<&'static str> {
    if let Some(&pattern) = BUILTIN_DENY_ALWAYS.iter().find(|p| glob_match(p, path)) {
        return Some(pattern);
    }
    if write {
        if let Some(&pattern) = BUILTIN_DENY_WRITE.iter().find(|p| glob_match(p, path)) {
            return Some(pattern);
        }
    }
    None
}

#[derive(Debug, Deserialize)]
struct Request {
    #[allow(dead_code)]
    jsonrpc: Option<String>,
    /// Absent for JSON-RPC notifications (no response expected).
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

/// One task per client connection. Requests are handled sequentially per
/// connection; concurrency comes from multiple connections. `caller` is the
/// peer credential captured at accept time and stamped on every audit record.
pub async fn handle_connection(
    stream: UnixStream,
    policy: Arc<PolicyEngine>,
    audit: Arc<AuditLog>,
    caller: Caller,
) {
    info!(
        "MCP client connected (uid={:?} gid={:?} pid={:?})",
        caller.uid, caller.gid, caller.pid
    );
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);
    let mut buf: Vec<u8> = Vec::new();

    loop {
        match read_bounded_line(&mut reader, &mut buf).await {
            Ok(LineRead::Eof) => break,
            Ok(LineRead::TooLong) => {
                // Never buffer an unbounded line: refuse and drop the client.
                warn!("oversized JSON-RPC line (> {MAX_LINE_BYTES} bytes), closing connection");
                let resp = error_response(
                    Value::Null,
                    -32600,
                    "request line exceeds the 1 MiB limit; connection closed",
                );
                let mut out = resp.to_string();
                out.push('\n');
                let _ = write_half.write_all(out.as_bytes()).await;
                break;
            }
            Ok(LineRead::Line) => {}
            Err(e) => {
                warn!("socket read error: {e}");
                break;
            }
        }

        // The buffer holds one line (with any trailing '\n'); UTF-8 is required.
        let line = match std::str::from_utf8(&buf) {
            Ok(s) => s.trim(),
            Err(_) => {
                let resp =
                    error_response(Value::Null, -32700, "parse error: line is not valid UTF-8");
                let mut out = resp.to_string();
                out.push('\n');
                if let Err(e) = write_half.write_all(out.as_bytes()).await {
                    warn!("socket write error: {e}");
                    break;
                }
                continue;
            }
        };
        if line.is_empty() {
            continue;
        }

        let response = match serde_json::from_str::<Request>(line) {
            Ok(request) => {
                let is_notification = request.id.is_none();
                let response = dispatch(request, &policy, &audit, caller).await;
                if is_notification {
                    None
                } else {
                    Some(response)
                }
            }
            Err(e) => Some(error_response(
                Value::Null,
                -32700,
                &format!("parse error: {e}"),
            )),
        };

        if let Some(response) = response {
            let mut out = response.to_string();
            out.push('\n');
            if let Err(e) = write_half.write_all(out.as_bytes()).await {
                warn!("socket write error: {e}");
                break;
            }
        }
    }
    info!("MCP client disconnected");
}

/// Outcome of one bounded line read.
enum LineRead {
    /// A line was read into the buffer (trailing '\n' included if present).
    Line,
    /// The peer closed the connection with no more data.
    Eof,
    /// The line grew past `MAX_LINE_BYTES` before a newline was seen.
    TooLong,
}

/// Read a single '\n'-terminated line into `buf`, refusing to buffer more than
/// `MAX_LINE_BYTES`. Uses `fill_buf`/`consume` so the cap is enforced *before*
/// the bytes are accumulated — an attacker cannot force an unbounded allocation
/// by withholding the newline.
async fn read_bounded_line<R>(reader: &mut R, buf: &mut Vec<u8>) -> std::io::Result<LineRead>
where
    R: tokio::io::AsyncBufRead + Unpin,
{
    buf.clear();
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return Ok(if buf.is_empty() {
                LineRead::Eof
            } else {
                LineRead::Line
            });
        }
        if let Some(idx) = available.iter().position(|&b| b == b'\n') {
            buf.extend_from_slice(&available[..=idx]);
            reader.consume(idx + 1);
            return Ok(LineRead::Line);
        }
        let take = available.len();
        buf.extend_from_slice(available);
        reader.consume(take);
        if buf.len() > MAX_LINE_BYTES {
            return Ok(LineRead::TooLong);
        }
    }
}

async fn dispatch(
    request: Request,
    policy: &Arc<PolicyEngine>,
    audit: &AuditLog,
    caller: Caller,
) -> Value {
    debug!("dispatch method={}", request.method);
    let id = request.id.unwrap_or(Value::Null);
    match request.method.as_str() {
        "initialize" => result_response(
            id,
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "serverInfo": {
                    "name": "vibed",
                    "version": env!("CARGO_PKG_VERSION"),
                },
                "capabilities": { "tools": {} }
            }),
        ),
        // Sent as a notification by well-behaved clients; acknowledged
        // anyway if a client attaches an id.
        "notifications/initialized" => result_response(id, json!({})),
        "ping" => result_response(id, json!({})),
        "tools/list" => result_response(id, json!({ "tools": list_tools() })),
        "tools/call" => handle_tools_call(id, request.params, policy, audit, caller).await,
        other => error_response(id, -32601, &format!("method not found: {other}")),
    }
}

async fn handle_tools_call(
    id: Value,
    params: Value,
    policy: &Arc<PolicyEngine>,
    audit: &AuditLog,
    caller: Caller,
) -> Value {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if name.is_empty() {
        return error_response(id, -32602, "invalid params: missing tool name");
    }
    let args = params.get("arguments").cloned().unwrap_or(Value::Null);

    // Extract and normalize the call context before any decision is made.
    let raw_path = args.get("path").and_then(Value::as_str);
    let normalized_path = match raw_path {
        Some(raw) => match normalize_path(raw) {
            Some(normalized) => Some(normalized),
            None => {
                // Relative path or attempt to climb above `/`: fail-closed.
                // The raw (rejected) path is the non-secret audit target.
                try_audit(
                    audit,
                    &name,
                    &args,
                    Some(raw),
                    Decision::Deny,
                    "blocked_invalid_path",
                    caller,
                );
                return tool_result(
                    id,
                    format!("policy: path '{raw}' is not an absolute, normalizable path"),
                    true,
                );
            }
        },
        None => None,
    };
    let service = args.get("unit").and_then(Value::as_str);

    // Human-readable, non-secret target recorded in the audit trail so an
    // action's subject (which file / unit / package) is recoverable in
    // forensics — never any file content or secret argument.
    let target = audit_target(&name, normalized_path.as_deref(), service, &args);

    // BUILT-IN hard denylist: checked in code, before and regardless of the
    // loaded policy. A policy file can never open these paths back up.
    if let Some(path) = normalized_path.as_deref() {
        let is_write = name == "fs.write";
        if let Some(pattern) = builtin_denied(path, is_write) {
            try_audit(
                audit,
                &name,
                &args,
                target.as_deref(),
                Decision::Deny,
                "blocked_builtin_denylist",
                caller,
            );
            return tool_result(
                id,
                format!("policy: path '{path}' is denied by the built-in denylist ({pattern})"),
                true,
            );
        }
    }

    let tier = tool_tier(&name);
    let ctx = CallContext {
        path: normalized_path.as_deref(),
        service,
    };
    let decision = policy.evaluate(&name, tier, ctx);

    match decision {
        Decision::Deny => {
            try_audit(
                audit,
                &name,
                &args,
                target.as_deref(),
                decision,
                "blocked",
                caller,
            );
            tool_result(id, format!("policy: tool '{name}' is denied"), true)
        }
        Decision::RequireApproval => {
            try_audit(
                audit,
                &name,
                &args,
                target.as_deref(),
                decision,
                "pending_approval",
                caller,
            );
            let tier_str = tier.map(Tier::as_str).unwrap_or("?");
            tool_result(
                id,
                format!(
                    "policy: tool '{name}' (tier {tier_str}) requires human approval; \
                     the request was recorded in the audit log (approval workflow: see ROADMAP.md)"
                ),
                true,
            )
        }
        Decision::Allow => {
            // Fail-closed: if the audit trail cannot be written, nothing runs.
            if !try_audit(
                audit,
                &name,
                &args,
                target.as_deref(),
                decision,
                "started",
                caller,
            ) {
                return tool_result(
                    id,
                    "audit log unavailable: refusing execution (fail-closed)".to_string(),
                    true,
                );
            }
            let tool_name = name.clone();
            let tool_args = args.clone();
            let policy_exec = Arc::clone(policy);
            let caller_exec = caller;
            // Tool bodies use blocking std::fs; keep the reactor responsive.
            let executed = tokio::task::spawn_blocking(move || {
                execute_tool(&tool_name, &tool_args, &policy_exec, caller_exec)
            })
            .await;
            match executed {
                Ok(Ok(text)) => {
                    try_audit(
                        audit,
                        &name,
                        &args,
                        target.as_deref(),
                        decision,
                        "ok",
                        caller,
                    );
                    tool_result(id, text, false)
                }
                Ok(Err(message)) => {
                    try_audit(
                        audit,
                        &name,
                        &args,
                        target.as_deref(),
                        decision,
                        &format!("error: {message}"),
                        caller,
                    );
                    tool_result(id, message, true)
                }
                Err(join_error) => {
                    try_audit(
                        audit,
                        &name,
                        &args,
                        target.as_deref(),
                        decision,
                        "panic",
                        caller,
                    );
                    tool_result(id, format!("internal error: {join_error}"), true)
                }
            }
        }
    }
}

/// Derive the non-secret audit target for a call: the normalized path for
/// filesystem tools, the unit for service tools, the package name for
/// pkg.install. Returns `None` when the tool carries no such subject.
fn audit_target(
    name: &str,
    normalized_path: Option<&str>,
    service: Option<&str>,
    args: &Value,
) -> Option<String> {
    if let Some(path) = normalized_path {
        return Some(path.to_string());
    }
    if let Some(unit) = service {
        return Some(unit.to_string());
    }
    if name == "pkg.install" {
        if let Some(pkg) = args.get("name").and_then(Value::as_str) {
            return Some(pkg.to_string());
        }
    }
    None
}

fn try_audit(
    audit: &AuditLog,
    tool: &str,
    args: &Value,
    target: Option<&str>,
    decision: Decision,
    outcome: &str,
    caller: Caller,
) -> bool {
    match audit.record(tool, args, target, decision.as_str(), outcome, caller) {
        Ok(()) => true,
        Err(e) => {
            warn!("audit write failed for tool '{tool}': {e}");
            false
        }
    }
}

// ---------------------------------------------------------------------------
// Tool registry
// ---------------------------------------------------------------------------

/// (name, tier, description, input JSON Schema)
fn tool_catalog() -> Vec<(&'static str, Tier, &'static str, Value)> {
    vec![
        (
            "os.status",
            Tier::T0,
            "Read-only system status: uptime, load average, memory, mounts (from /proc)",
            json!({"type": "object", "properties": {}, "additionalProperties": false}),
        ),
        (
            "fs.read",
            Tier::T0,
            "Read a file as UTF-8 (lossy), truncated at 256 KiB; audit trail, \
             secret stores and key material are denied by the built-in denylist",
            json!({"type": "object", "required": ["path"],
                   "properties": {"path": {"type": "string"}}}),
        ),
        (
            "fs.write",
            Tier::T1,
            "Write a user-scope file (restricted to /home and /var/home)",
            json!({"type": "object", "required": ["path", "content"],
                   "properties": {"path": {"type": "string"}, "content": {"type": "string"}}}),
        ),
        (
            "pkg.install",
            Tier::T2,
            "Install a package via rpm-ostree/bootc (v0.1 stub: always requires approval)",
            json!({"type": "object", "required": ["name"],
                   "properties": {"name": {"type": "string"}}}),
        ),
        (
            "svc.restart",
            Tier::T2,
            "Restart a systemd unit (v0.1 stub: always requires approval)",
            json!({"type": "object", "required": ["unit"],
                   "properties": {"unit": {"type": "string"}}}),
        ),
        (
            "memory.query",
            Tier::T0,
            "Query the VibeOS memory store (/var/lib/vibeos/memory): list or substring-match \
             files, optionally restricted to one scope and capped by limit (docs/MEMORY.md §9)",
            json!({"type": "object",
            "properties": {
                "query": {"type": "string"},
                "scope": {"type": "string",
                          "enum": ["identity", "hardware", "user",
                                   "projects", "journal", "knowledge"]},
                "limit": {"type": "integer", "minimum": 1}
            }}),
        ),
        (
            "memory.append",
            Tier::T1,
            "Append ONE entry to the VibeOS memory store — strictly additive, no delete or \
             rewrite. Writable scopes in v0.2: 'journal' (entry: type/source/data) and \
             'knowledge' (entry: subject/fact/source[/confidence]); vibed stamps ts (and the \
             fact id). 'user' and 'projects' are a Phase 2/3 remainder (docs/MEMORY.md §9)",
            json!({"type": "object", "required": ["scope", "entry"],
            "properties": {
                "scope": {"type": "string",
                          "enum": ["journal", "knowledge", "user", "projects"]},
                "entry": {"type": "object"}
            }}),
        ),
    ]
}

fn tool_tier(name: &str) -> Option<Tier> {
    tool_catalog().iter().find(|t| t.0 == name).map(|t| t.1)
}

fn list_tools() -> Vec<Value> {
    tool_catalog()
        .into_iter()
        .map(|(name, tier, description, input_schema)| {
            json!({
                "name": name,
                "description": description,
                "inputSchema": input_schema,
                "annotations": { "vibeosTier": tier.as_str() }
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Tool implementations (synchronous, run under spawn_blocking)
// ---------------------------------------------------------------------------

fn execute_tool(
    name: &str,
    args: &Value,
    policy: &PolicyEngine,
    caller: Caller,
) -> Result<String, String> {
    match name {
        "os.status" => os_status(),
        "fs.read" => fs_read(args, policy),
        "fs.write" => fs_write(args, policy, caller),
        "pkg.install" => Ok(json!({
            "status": "requires_approval",
            "detail": "pkg.install is a v0.1 stub: no package was installed. \
                       The rpm-ostree/bootc backend and the vibectl approval \
                       workflow land in a later milestone (see ROADMAP.md)."
        })
        .to_string()),
        "svc.restart" => Ok(json!({
            "status": "requires_approval",
            "detail": "svc.restart is a v0.1 stub: no unit was restarted. \
                       The systemd D-Bus backend lands with vibectl (see ROADMAP.md)."
        })
        .to_string()),
        "memory.query" => memory_query(args),
        "memory.append" => memory_append(args),
        _ => Err(format!("unknown tool: {name}")),
    }
}

fn os_status() -> Result<String, String> {
    let uptime_seconds = std::fs::read_to_string("/proc/uptime")
        .ok()
        .and_then(|s| s.split_whitespace().next().map(str::to_string))
        .and_then(|first| first.parse::<f64>().ok());
    let loadavg = std::fs::read_to_string("/proc/loadavg").ok().map(|s| {
        s.split_whitespace()
            .take(3)
            .map(str::to_string)
            .collect::<Vec<_>>()
    });
    let (mem_total_kb, mem_available_kb) = read_meminfo();
    let mounts = read_mounts();
    Ok(json!({
        "uptime_seconds": uptime_seconds,
        "loadavg_1_5_15": loadavg,
        "mem_total_kb": mem_total_kb,
        "mem_available_kb": mem_available_kb,
        "mounts": mounts,
        "note": "std-only approximation: free disk space needs statvfs (libc), \
                 so v0.1 reports mounted block devices without usage figures"
    })
    .to_string())
}

fn read_meminfo() -> (Option<u64>, Option<u64>) {
    let Ok(content) = std::fs::read_to_string("/proc/meminfo") else {
        return (None, None);
    };
    let mut total: Option<u64> = None;
    let mut available: Option<u64> = None;
    for line in content.lines() {
        let mut parts = line.split_whitespace();
        match (parts.next(), parts.next()) {
            (Some("MemTotal:"), Some(value)) => total = value.parse().ok(),
            (Some("MemAvailable:"), Some(value)) => available = value.parse().ok(),
            _ => {}
        }
    }
    (total, available)
}

fn read_mounts() -> Vec<Value> {
    let Ok(content) = std::fs::read_to_string("/proc/mounts") else {
        return Vec::new();
    };
    content
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let device = fields.next()?;
            let mountpoint = fields.next()?;
            let fstype = fields.next()?;
            if !device.starts_with("/dev/") {
                return None;
            }
            Some(json!({"device": device, "mountpoint": mountpoint, "fstype": fstype}))
        })
        .take(16)
        .collect()
}

fn fs_read(args: &Value, policy: &PolicyEngine) -> Result<String, String> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;

    let raw = args
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| "missing 'path' argument".to_string())?;
    let path = normalize_path(raw).ok_or_else(|| format!("fs.read: invalid path '{raw}'"))?;
    // Canonicalize (resolves every symlink AND requires existence): a symlink
    // must not smuggle a read into a denied location. handle_tools_call already
    // checked the lexical form before the policy decision.
    let canonical = std::fs::canonicalize(&path).map_err(|e| format!("fs.read {path}: {e}"))?;
    let canonical_str = canonical.to_string_lossy();
    // Bind the validated file's identity (device, inode) as close to
    // canonicalization as possible: the open() below re-verifies against it,
    // so a path component swapped for a symlink between the checks and the
    // open (TOCTOU, vibed runs as root) is detected and refused. Residual
    // window: canonicalize -> this lstat (a few µs); full closure comes with
    // openat2(RESOLVE_NO_SYMLINKS)/per-tool sandboxing in Phase 3.
    let validated =
        std::fs::symlink_metadata(&canonical).map_err(|e| format!("fs.read {path}: {e}"))?;
    // Built-in denylist AND the operator's policy (paths.denied / allowed) are
    // re-checked against BOTH the lexical and the canonical path, so neither a
    // symlink nor a policy blind spot can be used to escape into denied ground.
    for candidate in [path.as_str(), canonical_str.as_ref()] {
        if let Some(pattern) = builtin_denied(candidate, false) {
            return Err(format!(
                "fs.read: '{candidate}' is denied by the built-in denylist ({pattern})"
            ));
        }
    }
    recheck_policy_canonical(policy, "fs.read", &canonical_str)?;

    // Reject anything that is not a regular file: character/block devices
    // (e.g. /dev/urandom) would exhaust memory, and FIFOs would block the
    // worker thread forever. O_NOFOLLOW: canonicalize already resolved every
    // symlink, so the final component of `canonical` must not be one anymore —
    // if it is (post-canonicalization swap), open() fails with ELOOP.
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(O_NOFOLLOW)
        .open(&canonical)
        .map_err(|e| format!("fs.read {path}: {e}"))?;
    let opened = file
        .metadata()
        .map_err(|e| format!("fs.read {path}: {e}"))?;
    if !opened.is_file() {
        return Err(format!("fs.read: '{canonical_str}' is not a regular file"));
    }
    // The file actually opened must be the very inode that passed the
    // denylist/policy checks above (fstat on the open fd is authoritative).
    {
        use std::os::unix::fs::MetadataExt;
        if opened.dev() != validated.dev() || opened.ino() != validated.ino() {
            return Err(format!(
                "fs.read: '{canonical_str}' changed between validation and open \
                 (possible symlink race); refusing (fail-closed)"
            ));
        }
    }
    // Bounded read: never pull more than MAX_READ_BYTES + 1 into memory (one
    // extra byte only to detect truncation). This caps the allocation well
    // under 1 MiB regardless of the on-disk size, so a huge or unbounded file
    // cannot OOM the daemon.
    let mut bytes = Vec::new();
    file.by_ref()
        .take((MAX_READ_BYTES as u64) + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("fs.read {path}: {e}"))?;
    let truncated = bytes.len() > MAX_READ_BYTES;
    let end = bytes.len().min(MAX_READ_BYTES);
    let mut text = String::from_utf8_lossy(&bytes[..end]).into_owned();
    if truncated {
        text.push_str("\n[vibed: truncated, file exceeds 256 KiB]");
    }
    Ok(text)
}

fn fs_write(args: &Value, policy: &PolicyEngine, caller: Caller) -> Result<String, String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let raw = args
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| "missing 'path' argument".to_string())?;
    let content = args
        .get("content")
        .and_then(Value::as_str)
        .ok_or_else(|| "missing 'content' argument".to_string())?;
    // Normalization resolves `..` lexically, so the prefix check below cannot
    // be escaped with traversal sequences.
    let path = normalize_path(raw).ok_or_else(|| format!("fs.write: invalid path '{raw}'"))?;
    if !USER_WRITE_PREFIXES
        .iter()
        .any(|prefix| path.starts_with(prefix))
    {
        return Err(format!(
            "fs.write is T1 (modify-user): path must start with one of {USER_WRITE_PREFIXES:?}"
        ));
    }

    // Symlink-safety, mirroring fs.read: resolve the PARENT directory through
    // symlinks with canonicalize (it MUST already exist — fs.write does not
    // create intermediate directories) and rebuild the canonical target as
    // "canonical parent + final component". Every confinement check below runs
    // against this CANONICAL path, not the lexical string, so a symlinked
    // parent cannot land the write outside the confinement.
    let path_ref = std::path::Path::new(&path);
    let file_name = path_ref
        .file_name()
        .ok_or_else(|| format!("fs.write: '{path}' has no final path component"))?;
    let parent = path_ref
        .parent()
        .ok_or_else(|| format!("fs.write: '{path}' has no parent directory"))?;
    let canonical_parent = std::fs::canonicalize(parent)
        .map_err(|e| format!("fs.write: parent directory of '{path}' must exist: {e}"))?;
    let canonical_target = canonical_parent.join(file_name);
    let canonical_str = canonical_target.to_string_lossy().into_owned();

    // (a) The canonical target must still sit under a user-write prefix; a
    //     symlinked parent that escaped to /etc, /var/lib, ... is rejected here.
    if !USER_WRITE_PREFIXES
        .iter()
        .any(|prefix| canonical_str.starts_with(prefix))
    {
        return Err(format!(
            "fs.write: canonical path '{canonical_str}' escapes the user-write scope \
             (symlinked parent?)"
        ));
    }
    // (b) Built-in write denylist on BOTH the lexical and canonical path, so a
    //     symlink cannot smuggle a write into the audit trail, the memory
    //     volume or the policy directory.
    for candidate in [path.as_str(), canonical_str.as_str()] {
        if let Some(pattern) = builtin_denied(candidate, true) {
            return Err(format!(
                "fs.write: '{candidate}' is denied by the built-in denylist ({pattern})"
            ));
        }
    }
    // (c) The operator's policy (paths.denied / allowed) re-checked on the
    //     canonical target: an operator's custom deny must hold after symlink
    //     resolution too.
    recheck_policy_canonical(policy, "fs.write", &canonical_str)?;
    // (d) Cross-user confinement: the canonical target MUST be inside the
    //     calling uid's OWN home directory (resolved from /etc/passwd). This is
    //     the check that stops one agent from writing into another user's — or
    //     root's — home, even though vibed itself runs as root.
    confine_to_caller_home(caller, &canonical_str)?;

    // Write the final component with O_NOFOLLOW so that, if the final name is
    // itself a symlink, open() fails with ELOOP instead of following it out of
    // the confinement (canonicalize above deliberately did NOT resolve the
    // final component, precisely so this guard can fire).
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .custom_flags(O_NOFOLLOW)
        .open(&canonical_target)
        .map_err(|e| format!("fs.write {canonical_str}: {e}"))?;
    file.write_all(content.as_bytes())
        .map_err(|e| format!("fs.write {canonical_str}: {e}"))?;
    Ok(format!("wrote {} bytes to {canonical_str}", content.len()))
}

/// Re-run the policy decision against a POST-canonicalization path. A symlink
/// (or a policy blind spot) must not be usable to reach a location the loaded
/// policy forbids via `paths.denied`/`paths.allowed`. Anything other than a
/// clean `Allow` is refused. The tool already passed the lexical policy check
/// in `handle_tools_call`, so this can only tighten, never loosen.
fn recheck_policy_canonical(
    policy: &PolicyEngine,
    tool: &str,
    canonical: &str,
) -> Result<(), String> {
    let tier = tool_tier(tool);
    let ctx = CallContext {
        path: Some(canonical),
        service: None,
    };
    match policy.evaluate(tool, tier, ctx) {
        Decision::Allow => Ok(()),
        _ => Err(format!(
            "{tool}: canonical path '{canonical}' is denied by policy after symlink resolution"
        )),
    }
}

/// Require that `canonical_target` lives inside the calling uid's own home
/// directory. Fail-closed: an unknown caller uid, a uid with no `/etc/passwd`
/// entry, or a home that cannot be resolved all refuse the write.
fn confine_to_caller_home(caller: Caller, canonical_target: &str) -> Result<(), String> {
    let uid = caller.uid.ok_or_else(|| {
        "fs.write: caller uid unavailable (SO_PEERCRED); refusing (fail-closed)".to_string()
    })?;
    let home = home_dir_for_uid(uid).ok_or_else(|| {
        format!("fs.write: no home directory for uid {uid} in /etc/passwd; refusing")
    })?;
    // Canonicalize the home so the Fedora `/home -> /var/home` symlink (and any
    // other) is resolved to the same space as the (already canonical) target.
    let canonical_home = std::fs::canonicalize(&home)
        .map_err(|e| format!("fs.write: cannot resolve home '{home}' for uid {uid}: {e}"))?;
    let canonical_home_str = canonical_home.to_string_lossy();
    if !is_within(canonical_target, &canonical_home_str) {
        return Err(format!(
            "fs.write: cross-user write refused: '{canonical_target}' is not within uid {uid}'s \
             home '{canonical_home_str}'"
        ));
    }
    Ok(())
}

/// True when `path` is `base` itself or a descendant of it, comparing whole
/// path segments so `/home/dev2` is NOT treated as inside `/home/dev`.
fn is_within(path: &str, base: &str) -> bool {
    if path == base {
        return true;
    }
    let prefix = if base.ends_with('/') {
        base.to_string()
    } else {
        format!("{base}/")
    };
    path.starts_with(&prefix)
}

/// Resolve the home directory (field 6) of `uid` (field 3) by parsing
/// `/etc/passwd` by hand — no libc / nss dependency, matching the style of
/// `find_group_gid` in main.rs.
fn home_dir_for_uid(uid: u32) -> Option<String> {
    let content = std::fs::read_to_string("/etc/passwd").ok()?;
    home_dir_for_uid_in(&content, uid)
}

/// Pure passwd parser, split out so the uid->home mapping is unit-testable
/// without touching the real `/etc/passwd`.
/// Line format: `name:passwd:uid:gid:gecos:home:shell`.
fn home_dir_for_uid_in(passwd: &str, uid: u32) -> Option<String> {
    for line in passwd.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut fields = line.split(':');
        let _name = fields.next()?;
        let _passwd = fields.next()?;
        let uid_field = fields.next()?;
        let _gid = fields.next()?;
        let _gecos = fields.next()?;
        let home = fields.next()?;
        if uid_field.parse::<u32>().ok() == Some(uid) && !home.is_empty() {
            return Some(home.to_string());
        }
    }
    None
}

fn memory_query(args: &Value) -> Result<String, String> {
    memory_query_at(std::path::Path::new(MEMORY_DIR), args)
}

/// Body of memory.query with the store root as a parameter, so the tests can
/// run it against a scratch layout without touching /var/lib/vibeos/memory.
fn memory_query_at(root: &std::path::Path, args: &Value) -> Result<String, String> {
    let query = args
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_lowercase();

    // `scope` restricts the walk to one entry of the docs/MEMORY.md §3 layout.
    let scope = match args.get("scope") {
        None | Some(Value::Null) => None,
        Some(Value::String(name)) => match MEMORY_SCOPES.iter().find(|(n, _, _)| n == name) {
            Some(entry) => Some(*entry),
            None => {
                let valid: Vec<&str> = MEMORY_SCOPES.iter().map(|(n, _, _)| *n).collect();
                return Err(format!(
                    "memory.query: unknown scope '{name}' (valid: {})",
                    valid.join(", ")
                ));
            }
        },
        Some(_) => return Err("memory.query: 'scope' must be a string".to_string()),
    };

    // `limit` caps the number of matches returned; the walk itself stays
    // bounded by MAX_MEMORY_FILES regardless.
    let limit = match args.get("limit") {
        None | Some(Value::Null) => MAX_MEMORY_FILES,
        Some(value) => match value.as_u64() {
            Some(n) if n >= 1 => (n as usize).min(MAX_MEMORY_FILES),
            _ => return Err("memory.query: 'limit' must be an integer >= 1".to_string()),
        },
    };

    if !root.is_dir() {
        return Ok(json!({
            "initialized": false,
            "note": "memory store absent: vibeos-genesis.service has not run yet, \
                     or amnesic mode (Phase 3 target) discarded it at shutdown"
        })
        .to_string());
    }

    // Bounded iterative walk: no recursion, hard cap on visited files, and
    // NEVER follows symlinks (entry.file_type() / symlink_metadata do not
    // traverse) — a link planted inside the store cannot route the walk (or
    // the content scan) outside /var/lib/vibeos/memory, e.g. into the audit
    // trail.
    let mut files = Vec::new();
    let mut stack: Vec<std::path::PathBuf> = Vec::new();
    match scope {
        None => stack.push(root.to_path_buf()),
        Some((_, relative, is_dir)) => {
            let start = root.join(relative);
            if is_dir {
                stack.push(start);
            } else if start
                .symlink_metadata()
                .is_ok_and(|m| m.file_type().is_file())
            {
                files.push(start);
            }
        }
    }
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if files.len() >= MAX_MEMORY_FILES {
                break;
            }
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                stack.push(entry.path());
            } else if file_type.is_file() {
                files.push(entry.path());
            }
            // Symlinks (and any other special type) are deliberately skipped.
        }
        if files.len() >= MAX_MEMORY_FILES {
            break;
        }
    }

    let mut matches = Vec::new();
    let mut truncated = false;
    for path in &files {
        if matches.len() >= limit {
            truncated = true;
            break;
        }
        let relative = path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string();
        if query.is_empty() {
            matches.push(json!({ "file": relative }));
            continue;
        }
        let name_hit = relative.to_lowercase().contains(&query);
        // Bounded scan: never pull more than MAX_MEMORY_SCAN_BYTES of one file
        // into memory (same take() discipline as fs.read) — a huge file in the
        // store must not translate into an unbounded allocation.
        let content_hit = std::fs::File::open(path).ok().is_some_and(|file| {
            use std::io::Read;
            let mut bytes = Vec::new();
            let ok = file
                .take(MAX_MEMORY_SCAN_BYTES as u64)
                .read_to_end(&mut bytes)
                .is_ok();
            ok && String::from_utf8_lossy(&bytes)
                .to_lowercase()
                .contains(&query)
        });
        if name_hit || content_hit {
            matches.push(json!({ "file": relative }));
        }
    }

    Ok(json!({
        "initialized": true,
        "query": query,
        "scope": scope.map(|(name, _, _)| name),
        "scanned_files": files.len(),
        "matches": matches,
        "truncated": truncated
    })
    .to_string())
}

fn memory_append(args: &Value) -> Result<String, String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| format!("memory.append: system clock error: {e}"))?
        .as_secs();
    memory_append_at(std::path::Path::new(MEMORY_DIR), args, now)
}

/// Body of memory.append with the store root and the clock as parameters
/// (deterministic, filesystem-scratch-friendly tests).
///
/// Strictly ADDITIVE by construction: the only operation is an O_APPEND write
/// of one serialized line to a scope-derived file — there is no delete, no
/// rewrite, and no caller-controlled path (docs/MEMORY.md §9). The caller's
/// authoritative identity (uid/gid/pid) is recorded by the audit pipeline;
/// the `source` field below is self-declared memory metadata.
fn memory_append_at(
    root: &std::path::Path,
    args: &Value,
    epoch_secs: u64,
) -> Result<String, String> {
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};

    let scope = args
        .get("scope")
        .and_then(Value::as_str)
        .ok_or_else(|| "memory.append: missing 'scope' argument".to_string())?;
    let entry = args
        .get("entry")
        .ok_or_else(|| "memory.append: missing 'entry' argument".to_string())?;
    if !entry.is_object() {
        return Err("memory.append: 'entry' must be an object".to_string());
    }

    if !root.is_dir() {
        return Err(
            "memory.append: memory store absent: vibeos-genesis.service has not run yet \
             (nothing to append to; fail-closed)"
                .to_string(),
        );
    }

    let ts = utc_iso8601(epoch_secs);
    let (relative_file, line_value) = match scope {
        "journal" => {
            let event_type = entry.get("type").and_then(Value::as_str).ok_or_else(|| {
                "memory.append: missing 'type' field in journal entry".to_string()
            })?;
            if JOURNAL_RESERVED_TYPES.contains(&event_type) {
                return Err(format!(
                    "memory.append: journal type '{event_type}' is reserved for the system \
                     (genesis.sh, vibed, vibectl) and cannot be appended by an agent"
                ));
            }
            if !JOURNAL_AGENT_TYPES.contains(&event_type) {
                return Err(format!(
                    "memory.append: unknown journal type '{event_type}' (valid: {})",
                    JOURNAL_AGENT_TYPES.join(", ")
                ));
            }
            let source = validated_source(entry)?;
            let data = entry.get("data").cloned().unwrap_or_else(|| json!({}));
            if !data.is_object() {
                return Err("memory.append: 'data' must be an object".to_string());
            }
            (
                format!("journal/{}.jsonl", utc_date_string(epoch_secs)),
                json!({ "ts": ts, "type": event_type, "source": source, "data": data }),
            )
        }
        "knowledge" => {
            let subject = required_entry_str(entry, "subject", 256)?;
            let fact = required_entry_str(entry, "fact", 4096)?;
            let source = validated_source(entry)?;
            let confidence = match entry.get("confidence") {
                None | Some(Value::Null) => None,
                Some(value) => match value.as_f64() {
                    Some(c) if (0.0..=1.0).contains(&c) => Some(c),
                    _ => {
                        return Err(
                            "memory.append: 'confidence' must be a number between 0 and 1"
                                .to_string(),
                        )
                    }
                },
            };
            let mut fact_value = json!({
                "id": next_fact_id(epoch_secs),
                "ts": ts,
                "subject": subject,
                "fact": fact,
                "source": source
            });
            if let Some(c) = confidence {
                fact_value["confidence"] = json!(c);
            }
            ("knowledge/facts.jsonl".to_string(), fact_value)
        }
        "user" | "projects" => {
            return Err(format!(
                "memory.append: scope '{scope}' is not implemented yet (structured TOML/JSON \
                 merge — Phase 2/3 remainder, docs/MEMORY.md §3); the append-only scopes \
                 'journal' and 'knowledge' are the writable surface of v0.2"
            ));
        }
        other => {
            return Err(format!(
                "memory.append: unknown scope '{other}' (writable: journal, knowledge)"
            ));
        }
    };

    let mut line = serde_json::to_string(&line_value)
        .map_err(|e| format!("memory.append: serialization failed: {e}"))?;
    line.push('\n');
    if line.len() > MAX_APPEND_BYTES {
        return Err(format!(
            "memory.append: entry exceeds the {} KiB line cap",
            MAX_APPEND_BYTES / 1024
        ));
    }

    let target = root.join(&relative_file);
    let parent = target
        .parent()
        .ok_or_else(|| "memory.append: internal error: target has no parent".to_string())?;
    // Genesis creates the subdirectories; recreate defensively with the same
    // private permissions if one is missing (never the store root itself).
    if !parent.is_dir() {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)
            .map_err(|e| format!("memory.append: cannot create '{relative_file}' parent: {e}"))?;
    }

    // Serialize appends within the process; combined with O_APPEND, each entry
    // lands as one contiguous line even under concurrent connections.
    // O_NOFOLLOW: if the target name was swapped for a symlink, refuse rather
    // than follow it out of the store.
    static APPEND_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = APPEND_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .custom_flags(O_NOFOLLOW)
        .open(&target)
        .map_err(|e| format!("memory.append {relative_file}: {e}"))?;
    file.write_all(line.as_bytes())
        .map_err(|e| format!("memory.append {relative_file}: {e}"))?;

    Ok(json!({
        "appended": true,
        "file": relative_file,
        "bytes": line.len(),
        "ts": ts
    })
    .to_string())
}

/// `source` field of a memory entry: the self-declared emitter label (e.g.
/// "claude-code"), constrained to a safe charset. The authoritative caller
/// identity lives in the audit trail, not here.
fn validated_source(entry: &Value) -> Result<String, String> {
    let source = entry
        .get("source")
        .and_then(Value::as_str)
        .ok_or_else(|| "memory.append: missing 'source' field in entry".to_string())?;
    let valid = !source.is_empty()
        && source.len() <= 64
        && source
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if !valid {
        return Err(
            "memory.append: 'source' must be 1-64 characters of [A-Za-z0-9._-]".to_string(),
        );
    }
    Ok(source.to_string())
}

/// Required bounded string field of a memory entry.
fn required_entry_str(entry: &Value, field: &str, max_len: usize) -> Result<String, String> {
    let value = entry
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("memory.append: missing '{field}' field in entry"))?;
    if value.is_empty() || value.len() > max_len {
        return Err(format!(
            "memory.append: '{field}' must be 1-{max_len} bytes"
        ));
    }
    Ok(value.to_string())
}

/// Unique-enough fact id: epoch seconds + pid + per-process sequence number.
/// No dedup semantics — facts are append-only; curation is a vibectl matter.
fn next_fact_id(epoch_secs: u64) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    format!("{epoch_secs}-{}-{n}", std::process::id())
}

/// Civil UTC date-time from unix epoch seconds — Howard Hinnant's
/// `civil_from_days` algorithm, std-only (the crate deliberately has no
/// date/time dependency). Returns (year, month, day, hour, minute, second).
fn utc_civil(epoch_secs: u64) -> (i64, u32, u32, u32, u32, u32) {
    let days = (epoch_secs / 86_400) as i64;
    let seconds_of_day = epoch_secs % 86_400;
    let hour = (seconds_of_day / 3_600) as u32;
    let minute = ((seconds_of_day % 3_600) / 60) as u32;
    let second = (seconds_of_day % 60) as u32;

    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097); // [0, 146096]
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let year_of_era = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let month = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32; // [1, 12]
    let year = if month <= 2 {
        year_of_era + 1
    } else {
        year_of_era
    };
    (year, month, day, hour, minute, second)
}

/// `AAAA-MM-JJ` (UTC) — the journal's one-file-per-day naming (MEMORY.md §3.5).
fn utc_date_string(epoch_secs: u64) -> String {
    let (year, month, day, ..) = utc_civil(epoch_secs);
    format!("{year:04}-{month:02}-{day:02}")
}

/// ISO 8601 UTC timestamp with a `Z` suffix.
fn utc_iso8601(epoch_secs: u64) -> String {
    let (year, month, day, hour, minute, second) = utc_civil(epoch_secs);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

// ---------------------------------------------------------------------------
// JSON-RPC helpers
// ---------------------------------------------------------------------------

fn result_response(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// MCP tool result envelope: content blocks + isError flag.
fn tool_result(id: Value, text: String, is_error: bool) -> Value {
    result_response(
        id,
        json!({
            "content": [ { "type": "text", "text": text } ],
            "isError": is_error
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_denylist_blocks_reads_of_sensitive_paths() {
        for path in [
            "/var/lib/vibeos/audit/vibed.jsonl",
            "/etc/shadow",
            "/etc/shadow-",
            "/home/dev/.ssh/id_ed25519",
            "/root/.gnupg/private-keys-v1.d/key",
            "/proc/1234/environ",
            "/run/credentials/vibed.service/api_key",
            "/boot/efi/EFI/fedora/grubx64.efi",
        ] {
            assert!(
                builtin_denied(path, false).is_some(),
                "{path} must be read-denied by the built-in denylist"
            );
        }
    }

    #[test]
    fn builtin_denylist_write_only_entries() {
        // Policy and memory are readable (policy via policy.d README workflow,
        // memory via memory.query) but never agent-writable.
        for path in [
            "/etc/vibeos/policy.d/default.toml",
            "/var/lib/vibeos/memory/identity.toml",
        ] {
            assert!(
                builtin_denied(path, true).is_some(),
                "{path} must be write-denied"
            );
            assert!(
                builtin_denied(path, false).is_none(),
                "{path} must stay readable"
            );
        }
    }

    #[test]
    fn builtin_denylist_leaves_normal_paths_alone() {
        for path in [
            "/etc/os-release",
            "/home/dev/project/main.rs",
            "/var/home/dev/notes.md",
        ] {
            assert!(
                builtin_denied(path, false).is_none(),
                "{path} must be readable"
            );
            assert!(
                builtin_denied(path, true).is_none(),
                "{path} must be writable"
            );
        }
    }

    #[test]
    fn traversal_normalizes_into_the_denylist() {
        // A `..` escape that lexically lands on a denied path is caught after
        // normalization.
        let normalized = normalize_path("/home/dev/../../etc/shadow").expect("normalizes");
        assert_eq!(normalized, "/etc/shadow");
        assert!(builtin_denied(&normalized, false).is_some());
    }

    #[test]
    fn fs_write_rejects_paths_outside_user_prefixes() {
        let policy = empty_policy();
        let caller = Caller {
            uid: Some(1000),
            gid: None,
            pid: None,
        };
        let err = fs_write(
            &json!({"path": "/etc/passwd", "content": "x"}),
            &policy,
            caller,
        )
        .unwrap_err();
        assert!(err.contains("modify-user"), "unexpected error: {err}");
        // /tmp was removed from the v0.1 write scope (D7).
        let err =
            fs_write(&json!({"path": "/tmp/x", "content": "x"}), &policy, caller).unwrap_err();
        assert!(err.contains("modify-user"), "unexpected error: {err}");
    }

    #[test]
    fn fs_write_rejects_traversal_and_memory_volume() {
        let policy = empty_policy();
        let caller = Caller {
            uid: Some(1000),
            gid: None,
            pid: None,
        };
        let err = fs_write(
            &json!({"path": "/home/dev/../../etc/cron.d/evil", "content": "x"}),
            &policy,
            caller,
        )
        .unwrap_err();
        assert!(err.contains("modify-user"), "unexpected error: {err}");
        // The memory volume is under /var/lib, so the prefix check rejects it;
        // the built-in denylist covers it too (checked directly here). This is
        // the same canonical-write denylist that stops a symlink from smuggling
        // a write into the memory/audit/policy areas (see the symlink tests).
        assert!(builtin_denied("/var/lib/vibeos/memory/user/profile.toml", true).is_some());
        assert!(builtin_denied("/var/lib/vibeos/audit/vibed.jsonl", true).is_some());
        assert!(builtin_denied("/etc/vibeos/policy.d/default.toml", true).is_some());
    }

    // -- Test helpers --------------------------------------------------------

    fn empty_policy() -> PolicyEngine {
        PolicyEngine::from_rules(Vec::new())
    }

    /// Build a `PolicyEngine` from inline TOML by loading it through the real
    /// fail-closed loader (writes a temp drop-in, loads it, cleans up).
    fn policy_from_toml(toml_src: &str) -> PolicyEngine {
        use std::sync::atomic::{AtomicU32, Ordering};
        static SEQ: AtomicU32 = AtomicU32::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("vibed-mcp-pol-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create policy test dir");
        std::fs::write(dir.join("00-test.toml"), toml_src).expect("write policy");
        let engine = PolicyEngine::load_dir(&dir).expect("policy loads");
        let _ = std::fs::remove_dir_all(&dir);
        engine
    }

    /// A policy that mirrors the shipped fs.read/fs.write allow rules so the
    /// canonical re-check passes on legitimate in-home paths.
    fn permissive_policy() -> PolicyEngine {
        policy_from_toml(
            r#"
            [[rule]]
            id = "fs-read"
            tools = ["fs.read"]
            tier = "T0"
            action = "allow"

            [[rule]]
            id = "fs-write"
            tools = ["fs.write"]
            tier = "T1"
            action = "allow"
            [rule.paths]
            allowed = ["/home/**", "/var/home/**"]
            "#,
        )
    }

    /// Real uid of the test process, read from `/proc/self/status` (no libc).
    fn current_uid() -> u32 {
        let status = std::fs::read_to_string("/proc/self/status").expect("read /proc/self/status");
        for line in status.lines() {
            if let Some(rest) = line.strip_prefix("Uid:") {
                let first = rest.split_whitespace().next().expect("uid field present");
                return first.parse().expect("uid parses");
            }
        }
        panic!("no Uid line in /proc/self/status");
    }

    fn caller_uid(uid: u32) -> Caller {
        Caller {
            uid: Some(uid),
            gid: None,
            pid: None,
        }
    }

    /// A fresh, empty scratch directory inside `uid`'s real home (removed by the
    /// caller). Used by the filesystem-touching tests.
    fn home_scratch(uid: u32, tag: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static SEQ: AtomicU32 = AtomicU32::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let home = home_dir_for_uid(uid).expect("caller uid has a home in /etc/passwd");
        let dir = std::path::Path::new(&home)
            .join(format!(".vibed-test-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create home scratch dir");
        dir
    }

    // -- Fix 1/2: fs.write symlink-safety and cross-user confinement ----------

    #[test]
    fn fs_write_allows_write_within_callers_own_home() {
        let uid = current_uid();
        if uid == 0 {
            return; // root's home is /root, outside the user-write scope
        }
        let base = home_scratch(uid, "ok");
        let target = base.join("notes.md");
        let res = fs_write(
            &json!({"path": target.to_string_lossy(), "content": "hello vibed"}),
            &permissive_policy(),
            caller_uid(uid),
        );
        assert!(res.is_ok(), "self-home write must succeed: {res:?}");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "hello vibed");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn fs_write_rejects_cross_user_write() {
        let uid = current_uid();
        if uid == 0 {
            return;
        }
        let base = home_scratch(uid, "xuser");
        let target = base.join("evil.txt");
        // Caller claims to be root (uid 0, home /root): writing into THIS user's
        // home is cross-user and must be refused, even though the path is a
        // perfectly valid /home path that passes the lexical checks.
        let res = fs_write(
            &json!({"path": target.to_string_lossy(), "content": "x"}),
            &permissive_policy(),
            caller_uid(0),
        );
        let err = res.unwrap_err();
        assert!(
            err.contains("cross-user"),
            "expected cross-user refusal, got: {err}"
        );
        assert!(
            !target.exists(),
            "no file may be created on a cross-user refusal"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn fs_write_rejects_missing_caller_uid() {
        // SO_PEERCRED failed => uid unknown => fail-closed refusal.
        let uid = current_uid();
        if uid == 0 {
            return;
        }
        let base = home_scratch(uid, "nouid");
        let target = base.join("x.txt");
        let res = fs_write(
            &json!({"path": target.to_string_lossy(), "content": "x"}),
            &permissive_policy(),
            Caller::default(),
        );
        assert!(
            res.unwrap_err().contains("uid unavailable"),
            "unknown uid must be refused"
        );
        assert!(!target.exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn fs_write_rejects_symlinked_final_component() {
        let uid = current_uid();
        if uid == 0 {
            return;
        }
        let base = home_scratch(uid, "nofollow");
        let victim =
            std::env::temp_dir().join(format!("vibed-nofollow-victim-{}", std::process::id()));
        let _ = std::fs::remove_file(&victim);
        let link = base.join("link");
        std::os::unix::fs::symlink(&victim, &link).expect("create final-component symlink");
        let res = fs_write(
            &json!({"path": link.to_string_lossy(), "content": "pwned"}),
            &permissive_policy(),
            caller_uid(uid),
        );
        assert!(
            res.is_err(),
            "writing through a symlinked final name must fail (O_NOFOLLOW)"
        );
        assert!(
            !victim.exists(),
            "the symlink target must never be created/written"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn fs_write_rejects_symlinked_parent_escaping_home() {
        let uid = current_uid();
        if uid == 0 {
            return;
        }
        let base = home_scratch(uid, "escape");
        // A directory symlink that escapes the user-write scope to /etc.
        let escape = base.join("escape");
        std::os::unix::fs::symlink("/etc", &escape).expect("create parent symlink to /etc");
        let target = format!("{}/escape/vibed-implant", base.display());
        let res = fs_write(
            &json!({"path": target, "content": "x"}),
            &permissive_policy(),
            caller_uid(uid),
        );
        let err = res.unwrap_err();
        assert!(
            err.contains("escapes the user-write scope"),
            "a symlinked parent escaping /home must be refused: {err}"
        );
        assert!(!std::path::Path::new("/etc/vibed-implant").exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn fs_write_rejects_symlink_into_a_denied_area() {
        // Proxy for "write into audit/memory/policy via symlink": a symlinked
        // parent resolving into an area the policy denies is caught by the
        // canonical policy re-check (the very same mechanism that guards the
        // built-in audit/memory/policy write denylist on canonical paths).
        let uid = current_uid();
        if uid == 0 {
            return;
        }
        let base = home_scratch(uid, "denied");
        let secret = base.join("secret_real");
        std::fs::create_dir_all(&secret).expect("mkdir secret_real");
        let via = base.join("via");
        std::os::unix::fs::symlink(&secret, &via).expect("create symlink into denied area");
        let policy = policy_from_toml(&format!(
            r#"
            [[rule]]
            id = "fs-write"
            tools = ["fs.write"]
            tier = "T1"
            action = "allow"
            [rule.paths]
            allowed = ["/home/**", "/var/home/**"]
            denied = ["{}/**"]
            "#,
            secret.display()
        ));
        let target = format!("{}/via/implant", base.display());
        let res = fs_write(
            &json!({"path": target, "content": "x"}),
            &policy,
            caller_uid(uid),
        );
        let err = res.unwrap_err();
        assert!(
            err.contains("denied by policy"),
            "a symlink into a policy-denied area must be refused: {err}"
        );
        assert!(!secret.join("implant").exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    // -- Fix 3: expanded read denylist ---------------------------------------

    #[test]
    fn builtin_denylist_covers_added_secret_stores() {
        for path in [
            "/etc/ssh/ssh_host_ed25519_key",
            "/etc/ssh/sshd_config",
            "/etc/gshadow",
            "/etc/gshadow-",
            "/home/dev/.aws/credentials",
            "/home/dev/.aws/config",
            "/home/dev/.config/gcloud/credentials.db",
            "/etc/NetworkManager/system-connections/home.nmconnection",
            "/home/dev/.docker/config.json",
            "/home/dev/.kube/config",
            "/home/dev/.netrc",
            "/root/.bashrc",
            "/root",
            "/proc/1234/task/5678/environ", // per-thread environ bypass
            "/proc/1234/cmdline",
            "/proc/1234/task/5678/cmdline",
        ] {
            assert!(
                builtin_denied(path, false).is_some(),
                "{path} must be read-denied by the built-in denylist"
            );
        }
    }

    #[test]
    fn builtin_denylist_covers_ai_agent_credentials() {
        // fs.read is not confined to the caller's home (vibed runs as root):
        // the tokens of the AI agents and dev tooling shipped in the image
        // must be unreadable for EVERY user's home.
        for path in [
            "/home/dev/.claude/.credentials.json",
            "/home/dev/.claude/history.jsonl",
            "/var/home/dev/.claude.json",
            "/home/dev/.config/gh/hosts.yml",
            "/home/dev/.gemini/oauth_creds.json",
            "/home/dev/.codex/auth.json",
            "/home/dev/.local/share/opencode/auth.json",
            "/home/dev/.ollama/id_ed25519",
            "/home/dev/.npmrc",
            "/home/dev/.git-credentials",
            "/home/dev/.config/sops/age/keys.txt",
        ] {
            assert!(
                builtin_denied(path, false).is_some(),
                "{path} must be read-denied by the built-in denylist"
            );
        }
        // But ordinary dotfiles stay readable: the denylist targets secrets,
        // not the whole home.
        for path in ["/home/dev/.bashrc", "/home/dev/.config/fish/config.fish"] {
            assert!(
                builtin_denied(path, false).is_none(),
                "{path} must stay readable"
            );
        }
    }

    // -- Fix 4: fs.read special files and bounded reads ----------------------

    #[test]
    fn fs_read_rejects_non_regular_files() {
        let policy = permissive_policy();
        // Character device: reading to the end would exhaust memory.
        if std::path::Path::new("/dev/zero").exists() {
            let err = fs_read(&json!({"path": "/dev/zero"}), &policy).unwrap_err();
            assert!(
                err.contains("not a regular file"),
                "char device must be refused: {err}"
            );
        }
        // A directory is not a regular file either.
        let dir = std::env::temp_dir().join(format!("vibed-read-dir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        let err = fs_read(&json!({"path": dir.to_string_lossy()}), &policy).unwrap_err();
        assert!(
            err.contains("not a regular file"),
            "directory must be refused: {err}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fs_read_reads_regular_file_and_bounds_large_one() {
        let policy = permissive_policy();
        let dir = std::env::temp_dir().join(format!("vibed-read-ok-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");

        let small = dir.join("small.txt");
        std::fs::write(&small, "hello vibed").unwrap();
        let out = fs_read(&json!({"path": small.to_string_lossy()}), &policy).expect("read small");
        assert_eq!(out, "hello vibed");

        let big = dir.join("big.bin");
        std::fs::write(&big, vec![b'a'; MAX_READ_BYTES + 4096]).unwrap();
        let out = fs_read(&json!({"path": big.to_string_lossy()}), &policy).expect("read big");
        assert!(
            out.contains("truncated"),
            "oversized read must be truncated"
        );
        assert!(
            out.len() <= MAX_READ_BYTES + 64,
            "returned content must stay within the cap (+ notice), got {}",
            out.len()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fs_read_still_reads_through_symlinks_via_canonicalization() {
        // O_NOFOLLOW + the dev/ino recheck guard against TOCTOU swaps, but a
        // legitimate read THROUGH a symlink keeps working: canonicalize
        // resolves the link before open, so the final component of the opened
        // path is the real file.
        let policy = permissive_policy();
        let dir = std::env::temp_dir().join(format!("vibed-read-link-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        let real = dir.join("real.txt");
        std::fs::write(&real, "via symlink").unwrap();
        let link = dir.join("link.txt");
        std::os::unix::fs::symlink(&real, &link).expect("symlink");
        let out = fs_read(&json!({"path": link.to_string_lossy()}), &policy)
            .expect("reading through a symlink must still work");
        assert_eq!(out, "via symlink");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // -- Fix 4: bounded JSON-RPC line reader ---------------------------------

    #[tokio::test]
    async fn bounded_line_reader_reads_a_normal_line() {
        let data = b"hello world\n";
        let mut reader = BufReader::new(&data[..]);
        let mut buf = Vec::new();
        assert!(matches!(
            read_bounded_line(&mut reader, &mut buf).await.unwrap(),
            LineRead::Line
        ));
        assert_eq!(&buf, b"hello world\n");
        assert!(matches!(
            read_bounded_line(&mut reader, &mut buf).await.unwrap(),
            LineRead::Eof
        ));
    }

    #[tokio::test]
    async fn bounded_line_reader_rejects_oversized_line() {
        let big = vec![b'a'; MAX_LINE_BYTES + 16]; // never a newline
        let mut reader = BufReader::new(&big[..]);
        let mut buf = Vec::new();
        assert!(matches!(
            read_bounded_line(&mut reader, &mut buf).await.unwrap(),
            LineRead::TooLong
        ));
    }

    // -- passwd / confinement helpers ----------------------------------------

    #[test]
    fn home_dir_for_uid_parses_passwd() {
        let passwd = "root:x:0:0:root:/root:/bin/bash\n\
                      # a comment line\n\
                      \n\
                      micki:x:1000:1000:,,,:/home/micki:/bin/bash\n\
                      svc:x:1001:1001::/var/home/svc:/usr/sbin/nologin\n";
        assert_eq!(home_dir_for_uid_in(passwd, 0).as_deref(), Some("/root"));
        assert_eq!(
            home_dir_for_uid_in(passwd, 1000).as_deref(),
            Some("/home/micki")
        );
        assert_eq!(
            home_dir_for_uid_in(passwd, 1001).as_deref(),
            Some("/var/home/svc")
        );
        assert_eq!(home_dir_for_uid_in(passwd, 4242), None);
    }

    #[test]
    fn is_within_respects_segment_boundaries() {
        assert!(is_within("/home/dev/x", "/home/dev"));
        assert!(is_within("/home/dev", "/home/dev"));
        assert!(!is_within("/home/dev2/x", "/home/dev"));
        assert!(!is_within("/home", "/home/dev"));
    }

    #[test]
    fn registry_tiers_are_stable() {
        assert_eq!(tool_tier("os.status"), Some(Tier::T0));
        assert_eq!(tool_tier("fs.read"), Some(Tier::T0));
        assert_eq!(tool_tier("fs.write"), Some(Tier::T1));
        assert_eq!(tool_tier("pkg.install"), Some(Tier::T2));
        assert_eq!(tool_tier("svc.restart"), Some(Tier::T2));
        assert_eq!(tool_tier("memory.query"), Some(Tier::T0));
        assert_eq!(tool_tier("memory.append"), Some(Tier::T1));
        assert_eq!(
            tool_tier("disk.wipe"),
            None,
            "unknown tool has no tier => default-deny"
        );
    }

    // -- memory.query (scope/limit) and memory.append -------------------------

    /// A scratch memory store mimicking the docs/MEMORY.md §3 layout.
    fn memory_scratch(tag: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static SEQ: AtomicU32 = AtomicU32::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("vibed-mem-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for sub in ["user", "projects", "journal", "knowledge"] {
            std::fs::create_dir_all(dir.join(sub)).expect("create memory scratch");
        }
        std::fs::write(
            dir.join("identity.toml"),
            "schema = 1\nhostname = \"testhost\"\n",
        )
        .expect("write identity");
        std::fs::write(dir.join("user").join("profile.toml"), "lang = \"fr\"\n")
            .expect("write profile");
        std::fs::write(
            dir.join("journal").join("2026-01-01.jsonl"),
            "{\"ts\":\"2026-01-01T00:00:00Z\",\"type\":\"genesis\",\"source\":\"genesis.sh\",\"data\":{}}\n",
        )
        .expect("write journal");
        dir
    }

    fn parse_result(result: Result<String, String>) -> Value {
        serde_json::from_str(&result.expect("tool call succeeds")).expect("valid JSON payload")
    }

    #[test]
    fn memory_query_scope_restricts_the_walk() {
        let root = memory_scratch("qscope");
        // journal scope: only the journal file is visible.
        let payload = parse_result(memory_query_at(&root, &json!({"scope": "journal"})));
        assert_eq!(payload["scope"], "journal");
        assert_eq!(payload["scanned_files"], 1);
        assert_eq!(payload["matches"][0]["file"], "journal/2026-01-01.jsonl");
        // identity scope resolves the single file.
        let payload = parse_result(memory_query_at(&root, &json!({"scope": "identity"})));
        assert_eq!(payload["matches"][0]["file"], "identity.toml");
        // a scope never leaks files from another scope.
        let payload = parse_result(memory_query_at(&root, &json!({"scope": "knowledge"})));
        assert_eq!(payload["matches"].as_array().unwrap().len(), 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn memory_query_rejects_invalid_scope_and_limit() {
        let root = memory_scratch("qbad");
        let err = memory_query_at(&root, &json!({"scope": "audit"})).unwrap_err();
        assert!(err.contains("unknown scope"), "unexpected error: {err}");
        let err = memory_query_at(&root, &json!({"scope": 3})).unwrap_err();
        assert!(err.contains("must be a string"), "unexpected error: {err}");
        let err = memory_query_at(&root, &json!({"limit": 0})).unwrap_err();
        assert!(err.contains("integer >= 1"), "unexpected error: {err}");
        let err = memory_query_at(&root, &json!({"limit": -4})).unwrap_err();
        assert!(err.contains("integer >= 1"), "unexpected error: {err}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn memory_query_limit_caps_and_flags_truncation() {
        let root = memory_scratch("qlimit");
        let payload = parse_result(memory_query_at(&root, &json!({"limit": 1})));
        assert_eq!(payload["matches"].as_array().unwrap().len(), 1);
        assert_eq!(payload["truncated"], true);
        // without a limit, the scratch store fits well under the cap.
        let payload = parse_result(memory_query_at(&root, &json!({})));
        assert!(payload["matches"].as_array().unwrap().len() >= 3);
        assert_eq!(payload["truncated"], false);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn memory_query_reports_uninitialized_store() {
        let root = std::env::temp_dir().join(format!("vibed-mem-absent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let payload = parse_result(memory_query_at(&root, &json!({})));
        assert_eq!(payload["initialized"], false);
    }

    #[test]
    fn memory_query_content_scan_is_bounded_per_file() {
        let root = memory_scratch("qbound");
        // The needle sits AFTER the 64 KiB scan cap: a bounded scan must not
        // see it (and must not have loaded the whole file to find out).
        let mut big = vec![b'a'; MAX_MEMORY_SCAN_BYTES + 1024];
        big.extend_from_slice(b"needle-beyond-cap");
        std::fs::write(root.join("knowledge").join("big.md"), &big).expect("write big");
        let payload = parse_result(memory_query_at(
            &root,
            &json!({"query": "needle-beyond-cap"}),
        ));
        assert_eq!(
            payload["matches"].as_array().unwrap().len(),
            0,
            "content beyond MAX_MEMORY_SCAN_BYTES must not be scanned"
        );
        // Same needle before the cap: found.
        std::fs::write(
            root.join("knowledge").join("small.md"),
            b"needle-before-cap",
        )
        .expect("write small");
        let payload = parse_result(memory_query_at(
            &root,
            &json!({"query": "needle-before-cap"}),
        ));
        assert_eq!(payload["matches"].as_array().unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn memory_query_walk_never_follows_symlinks() {
        let root = memory_scratch("qsymlink");
        // An out-of-store area holding a "secret" the walk must never reach.
        let outside =
            std::env::temp_dir().join(format!("vibed-mem-outside-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&outside);
        std::fs::create_dir_all(&outside).expect("mkdir outside");
        std::fs::write(outside.join("secret.txt"), "outside-secret-content").expect("write secret");
        // A symlinked DIRECTORY and a symlinked FILE planted inside the store.
        std::os::unix::fs::symlink(&outside, root.join("knowledge").join("linkdir"))
            .expect("dir symlink");
        std::os::unix::fs::symlink(
            outside.join("secret.txt"),
            root.join("knowledge").join("linkfile.txt"),
        )
        .expect("file symlink");
        // Neither shows up in a listing...
        let payload = parse_result(memory_query_at(&root, &json!({"scope": "knowledge"})));
        assert_eq!(
            payload["matches"].as_array().unwrap().len(),
            0,
            "symlinks inside the store must be invisible to the walk"
        );
        // ...nor can their content be matched.
        let payload = parse_result(memory_query_at(&root, &json!({"query": "outside-secret"})));
        assert_eq!(payload["matches"].as_array().unwrap().len(), 0);
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&outside);
    }

    // 2026-07-08T01:01:01Z (see the utc_helpers test for the constant's proof).
    const T_2026_07_08: u64 = 1_783_468_800 + 3_661;

    #[test]
    fn memory_append_journal_writes_one_dated_line() {
        let root = memory_scratch("ajournal");
        let args = json!({
            "scope": "journal",
            "entry": {
                "type": "observation",
                "source": "claude-code",
                "data": { "note": "le projet vibeos-ui utilise pnpm, pas npm" }
            }
        });
        let payload = parse_result(memory_append_at(&root, &args, T_2026_07_08));
        assert_eq!(payload["appended"], true);
        assert_eq!(payload["file"], "journal/2026-07-08.jsonl");
        let written = std::fs::read_to_string(root.join("journal").join("2026-07-08.jsonl"))
            .expect("journal file written");
        let event: Value = serde_json::from_str(written.trim()).expect("one valid JSON line");
        assert_eq!(event["ts"], "2026-07-08T01:01:01Z");
        assert_eq!(event["type"], "observation");
        assert_eq!(event["source"], "claude-code");
        assert_eq!(
            event["data"]["note"],
            "le projet vibeos-ui utilise pnpm, pas npm"
        );
        // A second append lands on a NEW line of the same file (append-only).
        let _ = memory_append_at(&root, &args, T_2026_07_08 + 60).expect("second append");
        let written =
            std::fs::read_to_string(root.join("journal").join("2026-07-08.jsonl")).unwrap();
        assert_eq!(written.lines().count(), 2, "appends must never overwrite");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn memory_append_rejects_reserved_and_unknown_journal_types() {
        let root = memory_scratch("atypes");
        for reserved in JOURNAL_RESERVED_TYPES {
            let err = memory_append_at(
                &root,
                &json!({"scope": "journal",
                        "entry": {"type": reserved, "source": "claude-code", "data": {}}}),
                T_2026_07_08,
            )
            .unwrap_err();
            assert!(
                err.contains("reserved"),
                "type '{reserved}': unexpected error: {err}"
            );
        }
        let err = memory_append_at(
            &root,
            &json!({"scope": "journal",
                    "entry": {"type": "opinion", "source": "claude-code", "data": {}}}),
            T_2026_07_08,
        )
        .unwrap_err();
        assert!(
            err.contains("unknown journal type"),
            "unexpected error: {err}"
        );
        assert!(!root.join("journal").join("2026-07-08.jsonl").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn memory_append_validates_source_data_and_size() {
        let root = memory_scratch("aguard");
        // bad source charset
        let err = memory_append_at(
            &root,
            &json!({"scope": "journal",
                    "entry": {"type": "observation", "source": "a b/c", "data": {}}}),
            T_2026_07_08,
        )
        .unwrap_err();
        assert!(err.contains("'source'"), "unexpected error: {err}");
        // data must be an object
        let err = memory_append_at(
            &root,
            &json!({"scope": "journal",
                    "entry": {"type": "observation", "source": "x", "data": "free text"}}),
            T_2026_07_08,
        )
        .unwrap_err();
        assert!(err.contains("'data'"), "unexpected error: {err}");
        // line cap (anti-DoS)
        let err = memory_append_at(
            &root,
            &json!({"scope": "journal",
                    "entry": {"type": "observation", "source": "x",
                              "data": {"blob": "A".repeat(MAX_APPEND_BYTES)}}}),
            T_2026_07_08,
        )
        .unwrap_err();
        assert!(err.contains("line cap"), "unexpected error: {err}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn memory_append_knowledge_stamps_id_and_ts() {
        let root = memory_scratch("afact");
        let payload = parse_result(memory_append_at(
            &root,
            &json!({"scope": "knowledge",
                    "entry": {"subject": "vibeos-ui", "fact": "utilise pnpm",
                              "source": "claude-code", "confidence": 0.9}}),
            T_2026_07_08,
        ));
        assert_eq!(payload["file"], "knowledge/facts.jsonl");
        let written = std::fs::read_to_string(root.join("knowledge").join("facts.jsonl")).unwrap();
        let fact: Value = serde_json::from_str(written.trim()).unwrap();
        assert_eq!(fact["subject"], "vibeos-ui");
        assert_eq!(fact["fact"], "utilise pnpm");
        assert_eq!(fact["confidence"], 0.9);
        assert_eq!(fact["ts"], "2026-07-08T01:01:01Z");
        assert!(fact["id"]
            .as_str()
            .unwrap()
            .starts_with(&T_2026_07_08.to_string()));
        // out-of-range confidence is refused
        let err = memory_append_at(
            &root,
            &json!({"scope": "knowledge",
                    "entry": {"subject": "s", "fact": "f", "source": "x", "confidence": 2.0}}),
            T_2026_07_08,
        )
        .unwrap_err();
        assert!(err.contains("between 0 and 1"), "unexpected error: {err}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn memory_append_refuses_unimplemented_and_unknown_scopes() {
        let root = memory_scratch("ascope");
        for scope in ["user", "projects"] {
            let err = memory_append_at(
                &root,
                &json!({"scope": scope, "entry": {"source": "x"}}),
                T_2026_07_08,
            )
            .unwrap_err();
            assert!(
                err.contains("not implemented"),
                "scope '{scope}': unexpected error: {err}"
            );
        }
        let err = memory_append_at(
            &root,
            &json!({"scope": "identity", "entry": {"source": "x"}}),
            T_2026_07_08,
        )
        .unwrap_err();
        assert!(err.contains("unknown scope"), "unexpected error: {err}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn memory_append_fails_closed_on_uninitialized_store() {
        let root = std::env::temp_dir().join(format!("vibed-mem-noinit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let err = memory_append_at(
            &root,
            &json!({"scope": "journal",
                    "entry": {"type": "observation", "source": "x", "data": {}}}),
            T_2026_07_08,
        )
        .unwrap_err();
        assert!(
            err.contains("memory store absent"),
            "unexpected error: {err}"
        );
        assert!(
            !root.exists(),
            "the store root must never be created by memory.append"
        );
    }

    #[test]
    fn utc_helpers_match_known_dates() {
        assert_eq!(utc_iso8601(0), "1970-01-01T00:00:00Z");
        assert_eq!(utc_date_string(0), "1970-01-01");
        // 2024-02-29 (leap day): 1_704_067_200 (2024-01-01) + 59 days.
        assert_eq!(utc_date_string(1_704_067_200 + 59 * 86_400), "2024-02-29");
        // 2026-07-08: 1_767_225_600 (2026-01-01) + 188 days = 1_783_468_800.
        assert_eq!(utc_date_string(1_783_468_800), "2026-07-08");
        assert_eq!(utc_iso8601(1_783_468_800 + 3_661), "2026-07-08T01:01:01Z");
        // end-of-year boundary
        assert_eq!(utc_date_string(1_767_225_600 - 1), "2025-12-31");
        assert_eq!(utc_iso8601(1_767_225_600 - 1), "2025-12-31T23:59:59Z");
    }
}
