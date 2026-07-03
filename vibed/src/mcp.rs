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
/// Keep in sync with `docs/THREAT-MODEL.md` (S2).
const BUILTIN_DENY_ALWAYS: [&str; 7] = [
    "/var/lib/vibeos/audit/**", // audit trail: agents must never see or probe it
    "/etc/shadow*",             // /etc/shadow and /etc/shadow-
    "**/.ssh/**",               // key material, wherever it lives
    "**/.gnupg/**",
    "/proc/*/environ",   // process environments may leak secrets
    "/run/credentials/**", // decrypted systemd credentials
    "/boot/**",          // boot chain is none of the agent's business
];

/// Additional BUILT-IN denylist for WRITES only: agents may query the memory
/// through memory.query but never write it directly (memory.append is the
/// Phase 2 write path), and the policy itself is not agent-writable.
const BUILTIN_DENY_WRITE: [&str; 2] = [
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
    let mut lines = BufReader::new(read_half).lines();

    loop {
        let line = match lines.next_line().await {
            Ok(Some(line)) => line,
            Ok(None) => break,
            Err(e) => {
                warn!("socket read error: {e}");
                break;
            }
        };
        let line = line.trim();
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

async fn dispatch(request: Request, policy: &PolicyEngine, audit: &AuditLog, caller: Caller) -> Value {
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
    policy: &PolicyEngine,
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
                try_audit(audit, &name, &args, Decision::Deny, "blocked_invalid_path", caller);
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

    // BUILT-IN hard denylist: checked in code, before and regardless of the
    // loaded policy. A policy file can never open these paths back up.
    if let Some(path) = normalized_path.as_deref() {
        let is_write = name == "fs.write";
        if let Some(pattern) = builtin_denied(path, is_write) {
            try_audit(audit, &name, &args, Decision::Deny, "blocked_builtin_denylist", caller);
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
            try_audit(audit, &name, &args, decision, "blocked", caller);
            tool_result(id, format!("policy: tool '{name}' is denied"), true)
        }
        Decision::RequireApproval => {
            try_audit(audit, &name, &args, decision, "pending_approval", caller);
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
            if !try_audit(audit, &name, &args, decision, "started", caller) {
                return tool_result(
                    id,
                    "audit log unavailable: refusing execution (fail-closed)".to_string(),
                    true,
                );
            }
            let tool_name = name.clone();
            let tool_args = args.clone();
            // Tool bodies use blocking std::fs; keep the reactor responsive.
            let executed = tokio::task::spawn_blocking(move || execute_tool(&tool_name, &tool_args)).await;
            match executed {
                Ok(Ok(text)) => {
                    try_audit(audit, &name, &args, decision, "ok", caller);
                    tool_result(id, text, false)
                }
                Ok(Err(message)) => {
                    try_audit(audit, &name, &args, decision, &format!("error: {message}"), caller);
                    tool_result(id, message, true)
                }
                Err(join_error) => {
                    try_audit(audit, &name, &args, decision, "panic", caller);
                    tool_result(id, format!("internal error: {join_error}"), true)
                }
            }
        }
    }
}

fn try_audit(
    audit: &AuditLog,
    tool: &str,
    args: &Value,
    decision: Decision,
    outcome: &str,
    caller: Caller,
) -> bool {
    match audit.record(tool, args, decision.as_str(), outcome, caller) {
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

fn execute_tool(name: &str, args: &Value) -> Result<String, String> {
    match name {
        "os.status" => os_status(),
        "fs.read" => fs_read(args),
        "fs.write" => fs_write(args),
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

fn fs_read(args: &Value) -> Result<String, String> {
    let raw = args
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| "missing 'path' argument".to_string())?;
    let path = normalize_path(raw).ok_or_else(|| format!("fs.read: invalid path '{raw}'"))?;
    // Re-check the built-in denylist on the canonicalized path too: a symlink
    // must not smuggle a read into a denied location. (handle_tools_call
    // already checked the lexical form before the policy decision.)
    let canonical = std::fs::canonicalize(&path).map_err(|e| format!("fs.read {path}: {e}"))?;
    let canonical_str = canonical.to_string_lossy();
    for candidate in [path.as_str(), canonical_str.as_ref()] {
        if let Some(pattern) = builtin_denied(candidate, false) {
            return Err(format!(
                "fs.read: '{candidate}' is denied by the built-in denylist ({pattern})"
            ));
        }
    }
    let bytes = std::fs::read(&canonical).map_err(|e| format!("fs.read {path}: {e}"))?;
    let truncated = bytes.len() > MAX_READ_BYTES;
    let end = bytes.len().min(MAX_READ_BYTES);
    let mut text = String::from_utf8_lossy(&bytes[..end]).into_owned();
    if truncated {
        text.push_str("\n[vibed: truncated, file exceeds 256 KiB]");
    }
    Ok(text)
}

fn fs_write(args: &Value) -> Result<String, String> {
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
    // Defense in depth: handle_tools_call already checked this before the
    // policy decision; the execution path re-checks it independently.
    if let Some(pattern) = builtin_denied(&path, true) {
        return Err(format!(
            "fs.write: '{path}' is denied by the built-in denylist ({pattern})"
        ));
    }
    if let Some(parent) = std::path::Path::new(&path).parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("fs.write mkdir: {e}"))?;
    }
    std::fs::write(&path, content).map_err(|e| format!("fs.write {path}: {e}"))?;
    Ok(format!("wrote {} bytes to {path}", content.len()))
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
        let err = fs_write(&json!({"path": "/etc/passwd", "content": "x"})).unwrap_err();
        assert!(err.contains("modify-user"), "unexpected error: {err}");
        // /tmp was removed from the v0.1 write scope (D7).
        let err = fs_write(&json!({"path": "/tmp/x", "content": "x"})).unwrap_err();
        assert!(err.contains("modify-user"), "unexpected error: {err}");
    }

    #[test]
    fn fs_write_rejects_traversal_and_memory_volume() {
        let err = fs_write(&json!({"path": "/home/dev/../../etc/cron.d/evil", "content": "x"}))
            .unwrap_err();
        assert!(err.contains("modify-user"), "unexpected error: {err}");
        // The memory volume is under /var/lib, so the prefix check rejects it;
        // the built-in denylist covers it too (checked directly here).
        assert!(builtin_denied("/var/lib/vibeos/memory/user/profile.toml", true).is_some());
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
