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
];

/// Additional BUILT-IN denylist for WRITES only: agents may query the memory
/// through memory.query but never write it directly (memory.append is the
/// Phase 2 write path), and the policy itself is not agent-writable.
const BUILTIN_DENY_WRITE: &[&str] = &[
    "/etc/vibeos/policy.d/**",
    "/var/lib/vibeos/memory/**",
];

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
                let resp = error_response(Value::Null, -32700, "parse error: line is not valid UTF-8");
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
            Err(e) => Some(error_response(Value::Null, -32700, &format!("parse error: {e}"))),
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
            return Ok(if buf.is_empty() { LineRead::Eof } else { LineRead::Line });
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

async fn dispatch(request: Request, policy: &Arc<PolicyEngine>, audit: &AuditLog, caller: Caller) -> Value {
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
                try_audit(audit, &name, &args, Some(raw), Decision::Deny, "blocked_invalid_path", caller);
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
            try_audit(audit, &name, &args, target.as_deref(), Decision::Deny, "blocked_builtin_denylist", caller);
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
            try_audit(audit, &name, &args, target.as_deref(), decision, "blocked", caller);
            tool_result(id, format!("policy: tool '{name}' is denied"), true)
        }
        Decision::RequireApproval => {
            try_audit(audit, &name, &args, target.as_deref(), decision, "pending_approval", caller);
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
            if !try_audit(audit, &name, &args, target.as_deref(), decision, "started", caller) {
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
                    try_audit(audit, &name, &args, target.as_deref(), decision, "ok", caller);
                    tool_result(id, text, false)
                }
                Ok(Err(message)) => {
                    try_audit(audit, &name, &args, target.as_deref(), decision, &format!("error: {message}"), caller);
                    tool_result(id, message, true)
                }
                Err(join_error) => {
                    try_audit(audit, &name, &args, target.as_deref(), decision, "panic", caller);
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
            "Query the VibeOS memory store (/var/lib/vibeos/memory): list or substring-match files \
             (scope/limit arguments and memory.append are Phase 2/3 targets, see docs/MEMORY.md)",
            json!({"type": "object",
                   "properties": {"query": {"type": "string"}}}),
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
        _ => Err(format!("unknown tool: {name}")),
    }
}

fn os_status() -> Result<String, String> {
    let uptime_seconds = std::fs::read_to_string("/proc/uptime")
        .ok()
        .and_then(|s| s.split_whitespace().next().map(str::to_string))
        .and_then(|first| first.parse::<f64>().ok());
    let loadavg = std::fs::read_to_string("/proc/loadavg")
        .ok()
        .map(|s| s.split_whitespace().take(3).map(str::to_string).collect::<Vec<_>>());
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
    // worker thread forever.
    let mut file = std::fs::File::open(&canonical).map_err(|e| format!("fs.read {path}: {e}"))?;
    let is_file = file
        .metadata()
        .map_err(|e| format!("fs.read {path}: {e}"))?
        .is_file();
    if !is_file {
        return Err(format!("fs.read: '{canonical_str}' is not a regular file"));
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
    if !USER_WRITE_PREFIXES.iter().any(|prefix| path.starts_with(prefix)) {
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
    if !USER_WRITE_PREFIXES.iter().any(|prefix| canonical_str.starts_with(prefix)) {
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
fn recheck_policy_canonical(policy: &PolicyEngine, tool: &str, canonical: &str) -> Result<(), String> {
    let tier = tool_tier(tool);
    let ctx = CallContext { path: Some(canonical), service: None };
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
    let home = home_dir_for_uid(uid)
        .ok_or_else(|| format!("fs.write: no home directory for uid {uid} in /etc/passwd; refusing"))?;
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
    let query = args
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_lowercase();
    let root = std::path::Path::new(MEMORY_DIR);
    if !root.is_dir() {
        return Ok(json!({
            "initialized": false,
            "note": "memory store absent: vibeos-genesis.service has not run yet, \
                     or amnesic mode (Phase 3 target) discarded it at shutdown"
        })
        .to_string());
    }

    // Bounded iterative walk: no recursion, hard cap on visited files.
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if files.len() >= MAX_MEMORY_FILES {
                break;
            }
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.is_file() {
                files.push(path);
            }
        }
        if files.len() >= MAX_MEMORY_FILES {
            break;
        }
    }

    let mut matches = Vec::new();
    for path in &files {
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
        let content_hit = std::fs::read(path).ok().is_some_and(|bytes| {
            let end = bytes.len().min(MAX_MEMORY_SCAN_BYTES);
            String::from_utf8_lossy(&bytes[..end]).to_lowercase().contains(&query)
        });
        if name_hit || content_hit {
            matches.push(json!({ "file": relative }));
        }
    }

    Ok(json!({
        "initialized": true,
        "query": query,
        "scanned_files": files.len(),
        "matches": matches
    })
    .to_string())
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
            assert!(builtin_denied(path, true).is_some(), "{path} must be write-denied");
            assert!(builtin_denied(path, false).is_none(), "{path} must stay readable");
        }
    }

    #[test]
    fn builtin_denylist_leaves_normal_paths_alone() {
        for path in ["/etc/os-release", "/home/dev/project/main.rs", "/var/home/dev/notes.md"] {
            assert!(builtin_denied(path, false).is_none(), "{path} must be readable");
            assert!(builtin_denied(path, true).is_none(), "{path} must be writable");
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
        let caller = Caller { uid: Some(1000), gid: None, pid: None };
        let err = fs_write(&json!({"path": "/etc/passwd", "content": "x"}), &policy, caller)
            .unwrap_err();
        assert!(err.contains("modify-user"), "unexpected error: {err}");
        // /tmp was removed from the v0.1 write scope (D7).
        let err = fs_write(&json!({"path": "/tmp/x", "content": "x"}), &policy, caller).unwrap_err();
        assert!(err.contains("modify-user"), "unexpected error: {err}");
    }

    #[test]
    fn fs_write_rejects_traversal_and_memory_volume() {
        let policy = empty_policy();
        let caller = Caller { uid: Some(1000), gid: None, pid: None };
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
        Caller { uid: Some(uid), gid: None, pid: None }
    }

    /// A fresh, empty scratch directory inside `uid`'s real home (removed by the
    /// caller). Used by the filesystem-touching tests.
    fn home_scratch(uid: u32, tag: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static SEQ: AtomicU32 = AtomicU32::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let home = home_dir_for_uid(uid).expect("caller uid has a home in /etc/passwd");
        let dir = std::path::Path::new(&home).join(format!(".vibed-test-{tag}-{}-{n}", std::process::id()));
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
        assert!(err.contains("cross-user"), "expected cross-user refusal, got: {err}");
        assert!(!target.exists(), "no file may be created on a cross-user refusal");
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
        assert!(res.unwrap_err().contains("uid unavailable"), "unknown uid must be refused");
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
        let victim = std::env::temp_dir().join(format!("vibed-nofollow-victim-{}", std::process::id()));
        let _ = std::fs::remove_file(&victim);
        let link = base.join("link");
        std::os::unix::fs::symlink(&victim, &link).expect("create final-component symlink");
        let res = fs_write(
            &json!({"path": link.to_string_lossy(), "content": "pwned"}),
            &permissive_policy(),
            caller_uid(uid),
        );
        assert!(res.is_err(), "writing through a symlinked final name must fail (O_NOFOLLOW)");
        assert!(!victim.exists(), "the symlink target must never be created/written");
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
        let res = fs_write(&json!({"path": target, "content": "x"}), &policy, caller_uid(uid));
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

    // -- Fix 4: fs.read special files and bounded reads ----------------------

    #[test]
    fn fs_read_rejects_non_regular_files() {
        let policy = permissive_policy();
        // Character device: reading to the end would exhaust memory.
        if std::path::Path::new("/dev/zero").exists() {
            let err = fs_read(&json!({"path": "/dev/zero"}), &policy).unwrap_err();
            assert!(err.contains("not a regular file"), "char device must be refused: {err}");
        }
        // A directory is not a regular file either.
        let dir = std::env::temp_dir().join(format!("vibed-read-dir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        let err = fs_read(&json!({"path": dir.to_string_lossy()}), &policy).unwrap_err();
        assert!(err.contains("not a regular file"), "directory must be refused: {err}");
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
        assert!(out.contains("truncated"), "oversized read must be truncated");
        assert!(
            out.len() <= MAX_READ_BYTES + 64,
            "returned content must stay within the cap (+ notice), got {}",
            out.len()
        );
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
        assert_eq!(home_dir_for_uid_in(passwd, 1000).as_deref(), Some("/home/micki"));
        assert_eq!(home_dir_for_uid_in(passwd, 1001).as_deref(), Some("/var/home/svc"));
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
        assert_eq!(tool_tier("disk.wipe"), None, "unknown tool has no tier => default-deny");
    }
}
