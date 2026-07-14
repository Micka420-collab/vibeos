//! Minimal MCP-style JSON-RPC 2.0 server over the unix socket.
//!
//! Transport: line-delimited JSON (one request or response per line), which
//! is directly bridgeable to an MCP stdio client via `socat` (shipped in the
//! image, see `agent/README.md`).
//!
//! Call pipeline — no exceptions:
//!   1. path/service arguments are extracted and lexically normalized;
//!   2. per-uid rate limiting (token bucket, SO_PEERCRED): over-limit calls are
//!      refused fail-closed and audited (`rate_limited`), never executed;
//!   3. the BUILT-IN hard denylist is checked in code, regardless of policy;
//!   4. `policy::evaluate(tool, tier, ctx)` -> Allow | Deny | RequireApproval
//!      (first matching rule wins, absolute default-deny, T2/T3 floor);
//!   5. `audit::record(...)` with the caller identity (SO_PEERCRED) —
//!      fail-closed on the Allow path;
//!   6. execution (only if allowed), then a second audit record with the
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
pub(crate) const O_NOFOLLOW: i32 = 0x20000;
/// `O_NONBLOCK` (Linux): open() does not block. Belt-and-suspenders for fs.read
/// against a FIFO — even if a regular file is swapped for a FIFO in the TOCTOU
/// window after the file-type check, the open returns instead of hanging the
/// worker thread forever (a FIFO with no writer would otherwise block).
const O_NONBLOCK: i32 = 0x800;
/// Per-file cap when scanning memory content in memory.query.
pub(crate) const MAX_MEMORY_SCAN_BYTES: usize = 64 * 1024;
/// Chars of content returned per match by memory.query (so an agent can read
/// the memory in ONE call, not query + fs.read). Bounded to keep responses small.
pub(crate) const MEMORY_SNIPPET_CHARS: usize = 1024;
/// Hard cap on entries returned by one fs.list call.
const MAX_LIST_ENTRIES: usize = 500;
/// Upper bound on files walked by memory.query.
pub(crate) const MAX_MEMORY_FILES: usize = 200;
/// Hard cap on one serialized memory.append line, newline included (anti-DoS:
/// an agent cannot balloon the memory store with a single call).
pub(crate) const MAX_APPEND_BYTES: usize = 16 * 1024;
/// Journal event types an AGENT may append via memory.append. The remaining
/// types of docs/MEMORY.md §3.5 (`genesis`, `boot`, `tool_call`, `purge`,
/// `autonomous_session`) are reserved for the system itself (genesis.sh, vibed,
/// the agent supervisor) and refused to agents.
pub(crate) const JOURNAL_AGENT_TYPES: [&str; 5] = [
    "observation",
    "decision",
    "preference",
    "project_seen",
    "error",
];
pub(crate) const JOURNAL_RESERVED_TYPES: [&str; 5] = [
    "genesis",
    "boot",
    "tool_call",
    "purge",
    "autonomous_session",
];
/// Memory sub-scopes addressable by memory.query's `scope` argument, mapped to
/// their location in the store (relative path, is_directory). Keep in sync
/// with the layout in docs/MEMORY.md §3.
pub(crate) const MEMORY_SCOPES: [(&str, &str, bool); 6] = [
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

/// System locations fs.read / fs.list may reach OUTSIDE the caller's own home.
/// vibed runs as root, so without confinement a T0 read would expose every
/// user's personal files (docs, photos…) to any agent. Reads are therefore
/// restricted to the caller's own home (resolved from SO_PEERCRED) OR these
/// non-personal system trees. The built-in denylist still applies on top (it
/// carves the secrets — /etc/shadow, ~/.ssh, /proc/**/environ… — back out).
/// Deliberately EXCLUDED: /home and /var/home at large (other users), /root,
/// /tmp and /var (other users' state), /boot (denylisted). /var/lib/vibeos is
/// the machine's own state (memory readable here too; audit stays denylisted).
const SYSTEM_READ_PREFIXES: [&str; 6] = [
    "/etc/",
    "/usr/",
    "/proc/",
    "/sys/",
    "/run/",
    "/var/lib/vibeos/",
];

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
    "/var/lib/vibeos/approvals/**", // approval store: agents must not read or forge grants
    "/etc/shadow*",             // /etc/shadow and /etc/shadow- (password hashes)
    "/etc/gshadow*",            // group password hashes
    "**/.ssh/**",               // SSH key material, wherever it lives
    "**/.gnupg/**",             // GnuPG key material, wherever it lives
    "/etc/ssh/*",               // sshd config AND host keys
    "/etc/ssh/ssh_host_*",      // host private keys (explicit, in case of subdirs)
    "**/.aws/**",               // AWS credentials, config, SSO cache — whole dir
    "**/.config/gcloud/**",     // Google Cloud credentials store
    "/etc/NetworkManager/system-connections/**", // Wi-Fi/VPN PSKs and certs
    "**/.docker/config.json",   // registry auth tokens
    "**/.kube/config",          // kubernetes cluster credentials
    "**/.netrc",                // machine login credentials
    "/root/**",                 // root's home directory
    // OSTree/bootc symlinks /root -> /var/roothome, and the fs tools canonicalize
    // before re-checking, so the raw glob above must be mirrored on the canonical
    // spelling too (builtin_denied uses glob_match, which is alias-blind). Reads
    // are already blocked by confine_read for non-root callers; this keeps the
    // denylist itself alias-consistent with the policy matcher. It is the only
    // alias-sensitive entry here — the others are **/-anchored (spelling-agnostic)
    // or under non-symlinked roots (/etc, /proc, /run, /boot, /var/lib/vibeos).
    "/var/roothome/**",    // canonical form of /root on OSTree (see above)
    "/proc/*/environ",     // process environments may leak secrets
    "/proc/**/environ",    // ...including per-thread /proc/<pid>/task/<tid>/environ
    "/proc/**/cmdline",    // command lines may carry secrets/tokens
    "/run/credentials/**", // decrypted systemd credentials
    "/boot/**",            // boot chain is none of the agent's business
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
    limiter: Arc<crate::ratelimit::RateLimiter>,
    caller: Caller,
    // Root of the human-approval store. Production passes
    // `crate::approval::APPROVAL_DIR`; tests inject a scratch dir so the whole
    // require_approval -> approve -> grant-consumed -> Allow chain is exercisable
    // over the real socket without touching `/var/lib/vibeos`.
    approval_dir: std::path::PathBuf,
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
                let response =
                    dispatch(request, &policy, &audit, &limiter, caller, &approval_dir).await;
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
    limiter: &crate::ratelimit::RateLimiter,
    caller: Caller,
    approval_dir: &std::path::Path,
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
        "tools/call" => {
            handle_tools_call(
                id,
                request.params,
                policy,
                audit,
                limiter,
                caller,
                approval_dir,
            )
            .await
        }
        other => error_response(id, -32601, &format!("method not found: {other}")),
    }
}

async fn handle_tools_call(
    id: Value,
    params: Value,
    policy: &Arc<PolicyEngine>,
    audit: &AuditLog,
    limiter: &crate::ratelimit::RateLimiter,
    caller: Caller,
    approval_dir: &std::path::Path,
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
    let raw_service = args.get("unit").and_then(Value::as_str);
    // Canonicalize the unit name for svc.* tools BEFORE the policy decision, so
    // the deny-list (which lists fully-qualified units like "sshd.service")
    // matches a bare "sshd" too. Without this an agent bypasses the deny-list by
    // dropping the ".service" suffix: the tool canonicalizes the name only
    // internally, AFTER the decision, so "vibed"/"sshd" would slip past the deny
    // rule and reach the T2 approval queue instead of an outright Deny. An
    // invalid unit falls through to the raw name (execution rejects it anyway).
    let canonical_unit: Option<String> = if name.starts_with("svc.") {
        raw_service.and_then(|u| crate::tools::svc::validate_unit_name(u).ok())
    } else {
        None
    };
    let service = canonical_unit.as_deref().or(raw_service);

    // Human-readable, non-secret target recorded in the audit trail so an
    // action's subject (which file / unit / package) is recoverable in
    // forensics — never any file content or secret argument.
    let target = audit_target(&name, normalized_path.as_deref(), service, &args);

    // Per-uid rate limiting: bound a runaway or compromised agent BEFORE any
    // execution, memory write or approval-store growth. Over-limit calls are
    // refused fail-closed and audited (the rejection itself is a security
    // signal), never executed. Keyed by the unforgeable SO_PEERCRED uid.
    if !limiter.check(caller.uid, now_epoch_secs()) {
        try_audit(
            audit,
            &name,
            &args,
            target.as_deref(),
            Decision::Deny,
            "rate_limited",
            caller,
        );
        return tool_result(
            id,
            format!(
                "policy: rate limit exceeded for uid {:?}; slow down and retry shortly",
                caller.uid
            ),
            true,
        );
    }

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

    // Human-in-the-loop: a T2/T3 RequireApproval becomes Allow ONLY if the
    // operator has already granted this exact (tool, target, uid) call. The
    // grant is one-shot and short-lived — consumed here so it can never be
    // replayed. An agent cannot reach the approval store (root-only + denylist),
    // so it can never approve its own request. The consumed grant carries the
    // approver's uid, which we fold into the audit outcome: the grant file is
    // deleted on consumption, so the tamper-evident log is the ONLY durable
    // record of who authorized the action.
    //
    // ORDERING (deliberate, see below): the grant is CONSUMED here, before the
    // `started_approved` audit write on the Allow path. That means a rare audit
    // failure (disk-full / I/O error — itself a fail-closed catastrophe that
    // refuses execution anyway) burns the operator's single approval, forcing a
    // re-approval. We accept that over the alternative: consuming only AFTER the
    // audit would require a peek-then-delete split, which reopens a double-
    // execution window (two concurrent identical approved calls could both pass
    // the peek before either deletes). The one-shot guarantee — the atomic unlink
    // is the execution gate — is worth more than saving one approval in an
    // already-failing-disk state. Not silent debt: this is the documented choice.
    //
    // The blocking std::fs of the approval store runs on a blocking thread
    // (spawn_blocking), exactly like `execute_tool` below — never on the reactor.
    let consumed = if matches!(decision, Decision::RequireApproval) {
        let name_g = name.clone();
        let target_g = target.clone();
        let uid_g = caller.uid;
        let now_g = now_epoch_secs();
        let dir_g = approval_dir.to_path_buf();
        tokio::task::spawn_blocking(move || {
            crate::approval::check_and_consume_grant(
                &dir_g,
                &name_g,
                target_g.as_deref(),
                uid_g,
                now_g,
            )
        })
        .await
        .unwrap_or(None) // a panic in the blocking task -> no grant (fail-closed)
    } else {
        None
    };
    let approved = consumed.is_some();
    let decision = if approved { Decision::Allow } else { decision };
    let suffix = consumed
        .as_ref()
        .map(|c| approver_suffix(c.approver_uid))
        .unwrap_or_default();
    let started_outcome = if approved {
        format!("started_approved{suffix}")
    } else {
        "started".to_string()
    };
    let ok_outcome = if approved {
        format!("ok_approved{suffix}")
    } else {
        "ok".to_string()
    };

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
            // No fresh grant: record a pending request the operator can act on
            // with `vibectl approve <id>`, and tell the agent to wait. The
            // store's blocking std::fs runs on a blocking thread, not the reactor.
            let name_r = name.clone();
            let target_r = target.clone();
            let tier_r = tier.map(Tier::as_str).unwrap_or("?").to_string();
            let uid_r = caller.uid;
            let now_r = now_epoch_secs();
            let dir_r = approval_dir.to_path_buf();
            let request_id = tokio::task::spawn_blocking(move || {
                crate::approval::request_approval(
                    &dir_r,
                    &name_r,
                    target_r.as_deref(),
                    &tier_r,
                    uid_r,
                    now_r,
                )
            })
            .await
            .ok()
            .and_then(Result::ok);
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
            let how = match &request_id {
                Some(rid) => format!(
                    "a human approval was requested (id {rid}); the operator runs \
                     `vibectl approve {rid}`, then re-issue this call"
                ),
                None => "the request was recorded in the audit log".to_string(),
            };
            tool_result(
                id,
                format!("policy: tool '{name}' (tier {tier_str}) requires human approval; {how}"),
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
                &started_outcome,
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
            let audit_dir_exec = audit.dir().to_path_buf();
            // Tool bodies use blocking std::fs; keep the reactor responsive.
            let executed = tokio::task::spawn_blocking(move || {
                execute_tool(
                    &tool_name,
                    &tool_args,
                    &policy_exec,
                    caller_exec,
                    &audit_dir_exec,
                )
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
                        &ok_outcome,
                        caller,
                    );
                    // Feed the machine's own memory: record executed,
                    // state-changing actions as a reserved `tool_call` journal
                    // event (distinct from the forensic audit log). T0 reads and
                    // the memory.* tools themselves are excluded to keep the
                    // biography meaningful and avoid meta-noise. Best-effort:
                    // a failure here never fails the (already-succeeded) call.
                    try_journal_tool_call(&name, tier, target.as_deref(), caller);
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

/// Best-effort: record an executed, state-changing tool call in the machine's
/// memory journal (reserved `tool_call` event). Only T1+ tools qualify (T0 is
/// read-only, not biography) and the `memory.*` tools are excluded (they are
/// themselves memory writes/reads — journaling them would be meta-noise and, in
/// the append case, double every entry). Never fails the caller.
fn try_journal_tool_call(name: &str, tier: Option<Tier>, target: Option<&str>, caller: Caller) {
    let is_state_changing = matches!(tier, Some(Tier::T1) | Some(Tier::T2) | Some(Tier::T3));
    if !is_state_changing || name.starts_with("memory.") {
        return;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let tier_str = tier.map(Tier::as_str).unwrap_or("?");
    if let Err(e) = crate::tools::memory::journal_tool_call_at(
        std::path::Path::new(MEMORY_DIR),
        now,
        name,
        target,
        tier_str,
        caller,
    ) {
        warn!("memory journal (tool_call) write failed for '{name}': {e}");
    }
}

/// Current unix time in whole seconds (0 on a clock error — the approval store
/// then treats any grant as effectively immediate/expired, fail-safe).
fn now_epoch_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Audit-outcome suffix identifying WHO approved a consumed grant, appended to
/// the `*_approved` outcomes. The grant file is deleted on consumption, so this
/// is what preserves the operator identity in the tamper-evident log. `?` when
/// the approver uid was not recorded at approve time.
fn approver_suffix(approver_uid: Option<u32>) -> String {
    match approver_uid {
        Some(uid) => format!("(by_uid={uid})"),
        None => "(by_uid=?)".to_string(),
    }
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
            "Read a file as UTF-8 (lossy), truncated at 256 KiB. Confined to the CALLER's own \
             home (SO_PEERCRED) plus non-personal system trees (/etc /usr /proc /sys /run \
             /var/lib/vibeos); cross-user personal files are refused. Secret stores and key \
             material are denied by the built-in denylist on top",
            json!({"type": "object", "required": ["path"],
                   "properties": {"path": {"type": "string"}}}),
        ),
        (
            "fs.list",
            Tier::T0,
            "List one directory (non-recursive, capped at 500 entries): name, type \
             (file/dir/symlink/other) and size for regular files. Symlinks are reported, \
             never followed. Same confinement as fs.read (caller's own home + system trees) \
             and the same built-in denylist",
            json!({"type": "object", "required": ["path"],
                   "properties": {"path": {"type": "string"},
                                  "limit": {"type": "integer", "minimum": 1}}}),
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
            "Restart a systemd unit via systemctl (T2: human approval required — the \
             operator grants once with `vibectl approve`, then the unit is actually \
             restarted and read back to confirm; strict unit-name validation)",
            json!({"type": "object", "required": ["unit"],
                   "properties": {"unit": {"type": "string"}}}),
        ),
        (
            "svc.status",
            Tier::T0,
            "Read-only state of one systemd unit (systemctl show: load/active/sub state, \
             unit file state, description). Strict unit-name validation; no state change",
            json!({"type": "object", "required": ["unit"],
                   "properties": {"unit": {"type": "string"}}}),
        ),
        (
            "sectools.list",
            Tier::T0,
            "Discover the VibeOS security toolkit (read-only, executes nothing): lists the \
             curated tools, their category, the capability tier that gates AGENT invocation \
             (T2/T3 = human approval), and whether each is installed. Optional filters: \
             'category' and 'installed_only'. See docs/SECURITY-TOOLKIT.md",
            json!({"type": "object",
            "properties": {
                "category": {"type": "string"},
                "installed_only": {"type": "boolean"}
            }}),
        ),
        (
            "memory.query",
            Tier::T0,
            "Query the VibeOS memory store (/var/lib/vibeos/memory): substring-match files by \
             name and content, returning each match WITH a bounded content snippet (read the \
             memory in one call, no follow-up fs.read). Optional 'scope' \
             (identity/hardware/user/projects/journal/knowledge) and 'limit' (docs/MEMORY.md §9)",
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
             rewrite, no path argument (the file is scope-derived). Scopes: 'journal' \
             (entry: type/source/data), 'knowledge' (entry: subject/fact/source[/confidence]), \
             'user' (entry: key/value/source — append-only update, current profile = fold, \
             last-write-wins per key), 'projects' (entry: path/source[/name/languages/vcs/\
             summary/last_opened] — fold per path). vibed stamps ts (and the fact id). \
             NOTE: 'source' is a self-declared label, NOT trusted provenance — the \
             authoritative caller identity is the audit log's SO_PEERCRED uid. Never write \
             secrets here. See docs/MEMORY.md §9",
            json!({"type": "object", "required": ["scope", "entry"],
            "properties": {
                "scope": {"type": "string",
                          "enum": ["journal", "knowledge", "user", "projects"]},
                "entry": {"type": "object"}
            }}),
        ),
        (
            "agent.thinking",
            Tier::T0,
            "Read a bounded tail of an autonomous session's captured reasoning \
             (/var/lib/vibeos/memory/reasoning/<session-id>.jsonl), written by the agent \
             supervisor by tapping the CLI stream (ADR-012) — NOT the CLI's own transcript. \
             Required 'session_id' (charset [A-Za-z0-9._-], no path); optional 'tail' \
             (default 100, max 500 lines) and 'since' (unix seconds). Observability, not a \
             learned fact — distinct from memory.query. Absent session -> empty result.",
            json!({"type": "object", "required": ["session_id"],
            "properties": {
                "session_id": {"type": "string"},
                "tail": {"type": "integer", "minimum": 1},
                "since": {"type": "integer", "minimum": 0}
            }}),
        ),
        (
            "agent.sessions",
            Tier::T0,
            "List the autonomous-session ids that have captured reasoning \
             (/var/lib/vibeos/memory/reasoning/*.jsonl). Returns { sessions: [id...], \
             count, latest } (lexical order; 'latest' is the last id). Read-only \
             discovery so an observer (the HUD) can find a session to feed to \
             agent.thinking. No arguments.",
            json!({"type": "object", "properties": {}}),
        ),
        (
            "agents.list",
            Tier::T0,
            "Roster of YOUR OWN recently active agent processes, from the audit \
             trail — CONFINED to your uid (never another user's activity), your own \
             process excluded. Optional 'window_seconds' (default 120, max 3600). \
             Returns { agents: [{ uid, pid, name, tier, activity, awaiting_approval, \
             last_seen_unix, idle_seconds, calls }], count, window_seconds }. \
             Read-only observability for the HUD.",
            json!({"type": "object", "properties": {
                "window_seconds": {"type": "integer", "minimum": 1}
            }}),
        ),
        (
            "policy.check",
            Tier::T0,
            "Classify a HYPOTHETICAL tool call WITHOUT executing it: returns the \
             policy decision (allow/deny/require_approval) and tier for a (tool, \
             target) pair. Read-only — never executes, never approves, never \
             touches the approval store. A HINT for a governed editor auto-mode \
             (ADR-014): real enforcement (denylist, home confinement, T2/T3 floor) \
             always happens at execution, so a wrong hint can never bypass approval.",
            json!({"type": "object", "required": ["tool"],
            "properties": {
                "tool": {"type": "string"},
                "target": {"type": "string"}
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
    audit_dir: &std::path::Path,
) -> Result<String, String> {
    match name {
        "os.status" => os_status(),
        "fs.read" => fs_read(args, policy, caller),
        "fs.write" => fs_write(args, policy, caller),
        "pkg.install" => Ok(json!({
            "status": "requires_approval",
            "detail": "pkg.install is a v0.1 stub: no package was installed. \
                       The rpm-ostree/bootc backend and the vibectl approval \
                       workflow land in a later milestone (see ROADMAP.md)."
        })
        .to_string()),
        "svc.restart" => crate::tools::svc::svc_restart(args),
        "svc.status" => crate::tools::svc::svc_status(args),
        "sectools.list" => crate::tools::sectools::sectools_list(args),
        "fs.list" => fs_list(args, policy, caller),
        "memory.query" => crate::tools::memory::memory_query(args),
        "memory.append" => crate::tools::memory::memory_append(args),
        "agent.thinking" => agent_thinking(args),
        "agent.sessions" => agent_sessions(),
        "agents.list" => agents_list(args, caller, audit_dir),
        "policy.check" => policy_check(args, policy),
        _ => Err(format!("unknown tool: {name}")),
    }
}

/// policy.check (T0): classify a HYPOTHETICAL `(tool, target)` call — return the
/// policy `Decision` (allow / deny / require_approval) and tier WITHOUT executing
/// it, consuming a grant, or touching the approval store. This is what an
/// editor's governed auto-mode (ADR-014, couche 2) queries to decide "prompt or
/// not". It is a HINT: the real enforcement (built-in denylist, per-caller home
/// confinement, T2/T3 approval floor) always happens at execution in vibed, so a
/// wrong hint can never let a T2/T3 call through without approval — at worst the
/// editor shows or omits a prompt in error, and the real call is still gated.
///
/// Anti-DoS disciplines (same as every other tool, not weaker for being a
/// dry-run): it is reached through `handle_tools_call`, so the **per-uid rate
/// limiter runs first** (that check is tool-agnostic, before dispatch — see the
/// `limiter.check` gate) and the call is audited; its output is a **small, fixed
/// JSON** (echoes the bounded `target`, no content amplification) and it does no
/// unbounded work (a lexical path normalize + the fixed denylist + one policy
/// evaluation).
fn policy_check(args: &Value, policy: &PolicyEngine) -> Result<String, String> {
    let tool = args
        .get("tool")
        .and_then(Value::as_str)
        .ok_or_else(|| "policy.check: missing 'tool' argument".to_string())?;
    let target = args.get("target").and_then(Value::as_str);

    let note = "hint; real enforcement (denylist, home confinement, T2/T3 floor) \
                happens at execution in vibed";

    // Path tools: normalize the target and apply the built-in denylist FIRST,
    // mirroring the real pipeline's ordering (a denied path is denied whatever
    // the policy says).
    let normalized = if tool.starts_with("fs.") {
        target.and_then(normalize_path)
    } else {
        None
    };
    if let Some(path) = normalized.as_deref() {
        if let Some(pattern) = builtin_denied(path, tool == "fs.write") {
            return Ok(json!({
                "tool": tool, "target": target, "decision": "deny",
                "by": "builtin_denylist", "pattern": pattern, "note": note
            })
            .to_string());
        }
    }

    let tier = tool_tier(tool);
    let service = if tool.starts_with("svc.") {
        target
    } else {
        None
    };
    let ctx = CallContext {
        path: normalized.as_deref(),
        service,
    };
    let decision = match policy.evaluate(tool, tier, ctx) {
        Decision::Allow => "allow",
        Decision::Deny => "deny",
        Decision::RequireApproval => "require_approval",
    };
    Ok(json!({
        "tool": tool,
        "target": target,
        "tier": tier.map(Tier::as_str),
        "decision": decision,
        "note": note,
    })
    .to_string())
}

/// agent.thinking (T0): read a bounded tail of a session's captured reasoning
/// from the store written by the supervisor (ADR-012). Read-only; the session_id
/// is charset-validated by `reasoning::read_thinking` so it can never traverse
/// out of `/var/lib/vibeos/memory/reasoning/`.
fn agent_thinking(args: &Value) -> Result<String, String> {
    let session_id = args
        .get("session_id")
        .and_then(Value::as_str)
        .ok_or_else(|| "agent.thinking: missing 'session_id' argument".to_string())?;
    let tail = args.get("tail").and_then(Value::as_u64).map(|n| n as usize);
    let since = args.get("since").and_then(Value::as_u64);
    let out =
        crate::reasoning::read_thinking(std::path::Path::new(MEMORY_DIR), session_id, tail, since)?;
    serde_json::to_string(&out).map_err(|e| format!("agent.thinking: serialization failed: {e}"))
}

/// agent.sessions (T0): list the reasoning-session ids so an observer (the HUD)
/// can discover a session to pass to `agent.thinking`. Read-only directory
/// listing; no arguments, bounded output (one short id per captured session).
fn agent_sessions() -> Result<String, String> {
    let sessions = crate::reasoning::list_sessions(std::path::Path::new(MEMORY_DIR));
    let latest = sessions.last().cloned();
    serde_json::to_string(&json!({
        "sessions": sessions,
        "count": sessions.len(),
        "latest": latest,
    }))
    .map_err(|e| format!("agent.sessions: serialization failed: {e}"))
}

/// Map a capability tier to its numeric level (0..3) for the HUD roster.
fn tier_number(t: Tier) -> u8 {
    match t {
        Tier::T0 => 0,
        Tier::T1 => 1,
        Tier::T2 => 2,
        Tier::T3 => 3,
    }
}

/// Best-effort process name for a pid (`/proc/<pid>/comm`). `None` if the pid is
/// 0/unknown or the process has already exited — the roster then falls back to a
/// generic label rather than inventing a name.
fn proc_comm(pid: u64) -> Option<String> {
    if pid == 0 {
        return None;
    }
    let raw = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    let name = raw.trim();
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

/// Read a bounded tail of the audit trail as parsed records with `ts >= cutoff`.
/// Best-effort (skips unreadable/corrupt lines) and read-only — used ONLY to
/// derive the `agents.list` roster, never for chain verification. Bounds the
/// read to the last `MAX_TAIL_BYTES` of the two most recent daily files, so it
/// stays cheap regardless of how large the audit log has grown.
fn read_recent_audit(dir: &std::path::Path, cutoff_secs: u64) -> Vec<Value> {
    const MAX_TAIL_BYTES: u64 = 512 * 1024;
    use std::io::{Read, Seek, SeekFrom};

    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<std::path::PathBuf> = read_dir
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("vibed-") && n.ends_with(".jsonl"))
        })
        .collect();
    files.sort();
    // Two most recent daily files cover any short window across a midnight roll.
    let recent: Vec<std::path::PathBuf> = files.iter().rev().take(2).rev().cloned().collect();

    let mut out = Vec::new();
    for path in recent {
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        let start = meta.len().saturating_sub(MAX_TAIL_BYTES);
        let Ok(mut f) = std::fs::File::open(&path) else {
            continue;
        };
        if start > 0 && f.seek(SeekFrom::Start(start)).is_err() {
            continue;
        }
        let mut bytes = Vec::new();
        if f.read_to_end(&mut bytes).is_err() {
            continue;
        }
        let buf = String::from_utf8_lossy(&bytes);
        let mut lines = buf.lines();
        // A mid-file seek likely lands inside a line; drop that first partial.
        if start > 0 {
            lines.next();
        }
        for line in lines {
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(v) = serde_json::from_str::<Value>(line) {
                let ts = v
                    .get("ts_unix_ms")
                    .and_then(Value::as_u64)
                    .map(|ms| ms / 1000)
                    .unwrap_or(0);
                if ts >= cutoff_secs {
                    out.push(v);
                }
            }
        }
    }
    out
}

/// agents.list (T0): a live roster of the CALLER'S OWN recently active agent
/// processes, derived from the audit trail. **Confined to the requesting uid**
/// (SO_PEERCRED): an agent — or the HUD, which runs as the session user — sees
/// only processes of its OWN uid, never another user's activity (no cross-user
/// leak; same confinement discipline as fs.read). The caller's own process is
/// excluded (the HUD does not list itself). Grouped by pid so distinct agent
/// processes of the same user appear separately.
///
/// Anti-DoS: reached through `handle_tools_call` (per-uid rate limiter runs
/// first, call audited); output is bounded (a short entry per pid seen in a
/// bounded time window read from a bounded audit tail); no unbounded work.
///
/// v0.2.5 note: `name` is best-effort from `/proc/<pid>/comm` (a Node-based CLI
/// shows as "node"); a finished process yields "agent". Per-connection identity
/// and a richer roster are future work — the reliable key is the SO_PEERCRED uid.
fn agents_list(
    args: &Value,
    caller: Caller,
    audit_dir: &std::path::Path,
) -> Result<String, String> {
    const DEFAULT_WINDOW_SECS: u64 = 120;
    let window = args
        .get("window_seconds")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_WINDOW_SECS)
        .clamp(1, 3600);
    let now = now_epoch_secs();
    let cutoff = now.saturating_sub(window);

    // Fail-closed confinement: an unidentified caller (no SO_PEERCRED uid) sees
    // nothing — the roster is a per-uid view, never a global one.
    let Some(uid) = caller.uid else {
        return Ok(json!({
            "agents": [],
            "count": 0,
            "window_seconds": window,
            "note": "no caller uid (SO_PEERCRED); the roster is confined per-uid"
        })
        .to_string());
    };
    // SO_PEERCRED pids are positive; a non-positive pid never matches a real one.
    let self_pid = caller.pid.filter(|p| *p > 0).map(|p| p as u64);

    struct Agg {
        tier: u8,
        last_ts: u64,
        last_tool: String,
        last_target: Option<String>,
        calls: u64,
    }
    let mut by_pid: std::collections::BTreeMap<u64, Agg> = std::collections::BTreeMap::new();

    for r in read_recent_audit(audit_dir, cutoff) {
        // Confine to the caller's own uid; skip the caller's own process.
        if r.get("caller_uid").and_then(Value::as_u64) != Some(u64::from(uid)) {
            continue;
        }
        let pid = r.get("caller_pid").and_then(Value::as_u64).unwrap_or(0);
        if Some(pid) == self_pid {
            continue;
        }
        let ts = r
            .get("ts_unix_ms")
            .and_then(Value::as_u64)
            .map(|ms| ms / 1000)
            .unwrap_or(0);
        let tool = r
            .get("tool")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let target = r.get("target").and_then(Value::as_str).map(str::to_string);
        let tier = tool_tier(&tool).map(tier_number).unwrap_or(0);

        let e = by_pid.entry(pid).or_insert(Agg {
            tier: 0,
            last_ts: 0,
            last_tool: String::new(),
            last_target: None,
            calls: 0,
        });
        e.calls += 1;
        e.tier = e.tier.max(tier);
        if ts >= e.last_ts {
            e.last_ts = ts;
            e.last_tool = tool;
            e.last_target = target;
        }
    }

    // A pending T2/T3 approval is keyed by uid, so it flags all of that uid's
    // processes (best-effort — the store may distinguish finer later).
    let awaiting =
        crate::approval::list_pending(std::path::Path::new(crate::approval::APPROVAL_DIR))
            .iter()
            .any(|p| p.get("caller_uid").and_then(Value::as_u64) == Some(u64::from(uid)));

    let agents: Vec<Value> = by_pid
        .into_iter()
        .map(|(pid, a)| {
            let name = proc_comm(pid).unwrap_or_else(|| "agent".to_string());
            let activity = match &a.last_target {
                Some(t) if !t.is_empty() => format!("{} {}", a.last_tool, t),
                _ => a.last_tool.clone(),
            };
            json!({
                "uid": uid,
                "pid": pid,
                "name": name,
                "tier": a.tier,
                "activity": activity,
                "awaiting_approval": awaiting,
                "last_seen_unix": a.last_ts,
                "idle_seconds": now.saturating_sub(a.last_ts),
                "calls": a.calls,
            })
        })
        .collect();

    let count = agents.len();
    Ok(json!({
        "agents": agents,
        "count": count,
        "window_seconds": window,
    })
    .to_string())
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

fn fs_read(args: &Value, policy: &PolicyEngine, caller: Caller) -> Result<String, String> {
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
    // Cross-user confinement: a system tree or the caller's OWN home only.
    let scope = confine_read("fs.read", caller, &canonical_str)?;

    // Reject anything that is not a regular file BEFORE opening: a FIFO opened
    // O_RDONLY with no writer BLOCKS the spawn_blocking worker forever (a DoS —
    // enough of them exhaust the blocking pool and stall every user's calls),
    // and a character/block device would exhaust memory. `validated` is the
    // lstat of the already-canonical path (no symlink left), so its type is
    // authoritative — check it here, not after the (potentially blocking) open.
    if !validated.file_type().is_file() {
        return Err(format!("fs.read: '{canonical_str}' is not a regular file"));
    }
    // O_NOFOLLOW: canonicalize already resolved every symlink, so the final
    // component must not be one anymore — a post-canonicalization swap fails
    // with ELOOP. O_NONBLOCK: belt-and-suspenders so a swap to a FIFO in the
    // TOCTOU window returns instead of blocking (regular-file reads ignore it).
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(O_NOFOLLOW | O_NONBLOCK)
        .open(&canonical)
        .map_err(|e| format!("fs.read {path}: {e}"))?;
    let opened = file
        .metadata()
        .map_err(|e| format!("fs.read {path}: {e}"))?;
    if !opened.is_file() {
        return Err(format!("fs.read: '{canonical_str}' is not a regular file"));
    }
    // Hardlink defense (cross-owner): the path-based denylist + home confinement
    // are blind to hardlinks (canonicalize resolves symlinks, not hardlinks), so
    // an agent could hardlink another user's / a root-owned inode into its own
    // home and have root-`vibed` read it. For a read resolved INTO the caller's
    // home, require the opened inode to be owned by the caller — a system-tree
    // read is legitimately root-owned and exempt. (A hardlink to the caller's
    // OWN file stays readable: the agent already owns it via its uid.)
    if scope == ReadScope::Home {
        use std::os::unix::fs::MetadataExt;
        if Some(opened.uid()) != caller.uid {
            return Err(format!(
                "fs.read: '{canonical_str}' is owned by uid {} but the caller is uid {:?}; \
                 refusing (possible cross-owner hardlink)",
                opened.uid(),
                caller.uid
            ));
        }
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

/// fs.list (T0): bounded, non-recursive listing of one directory.
///
/// Same confinement discipline as fs.read: lexical normalization (done by the
/// caller pipeline too), canonicalization, built-in denylist + operator
/// policy re-checked on BOTH the lexical and canonical form. Symlinks inside
/// the directory are REPORTED (type "symlink") but never followed — neither
/// their target type nor size is disclosed.
fn fs_list(args: &Value, policy: &PolicyEngine, caller: Caller) -> Result<String, String> {
    let raw = args
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| "missing 'path' argument".to_string())?;
    let limit = match args.get("limit") {
        None | Some(Value::Null) => MAX_LIST_ENTRIES,
        Some(value) => match value.as_u64() {
            Some(n) if n >= 1 => (n as usize).min(MAX_LIST_ENTRIES),
            _ => return Err("fs.list: 'limit' must be an integer >= 1".to_string()),
        },
    };
    let path = normalize_path(raw).ok_or_else(|| format!("fs.list: invalid path '{raw}'"))?;
    let canonical = std::fs::canonicalize(&path).map_err(|e| format!("fs.list {path}: {e}"))?;
    let canonical_str = canonical.to_string_lossy();
    for candidate in [path.as_str(), canonical_str.as_ref()] {
        if let Some(pattern) = builtin_denied(candidate, false) {
            return Err(format!(
                "fs.list: '{candidate}' is denied by the built-in denylist ({pattern})"
            ));
        }
    }
    recheck_policy_canonical(policy, "fs.list", &canonical_str)?;
    // Cross-user confinement: a system tree or the caller's OWN home only.
    confine_read("fs.list", caller, &canonical_str)?;

    // Anti-TOCTOU (same discipline as fs.read; vibed runs as root): capture the
    // directory's identity right before the read and re-check it right after.
    // canonicalize resolved every symlink, so the target must be a real
    // directory now; if a component is swapped for a symlink in the window
    // around read_dir (e.g. `proj` -> /root), dev/ino changes and we refuse.
    // Full closure (parent-component swaps mid-read) awaits openat2
    // (RESOLVE_NO_SYMLINKS) in Phase 3.
    let before =
        std::fs::symlink_metadata(&canonical).map_err(|e| format!("fs.list {path}: {e}"))?;
    if !before.file_type().is_dir() {
        return Err(format!("fs.list: '{canonical_str}' is not a directory"));
    }
    let read_dir = std::fs::read_dir(&canonical).map_err(|e| format!("fs.list {path}: {e}"))?;

    // Keep the `limit` lexicographically-smallest names with O(limit) memory,
    // so a truncated result is a STABLE prefix regardless of readdir order (a
    // sort AFTER truncation would return an arbitrary readdir-order subset).
    struct ByName(String, Value);
    let mut kept: std::collections::BinaryHeap<ByName> = std::collections::BinaryHeap::new();
    impl PartialEq for ByName {
        fn eq(&self, other: &Self) -> bool {
            self.0 == other.0
        }
    }
    impl Eq for ByName {}
    impl PartialOrd for ByName {
        fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
            Some(self.cmp(other))
        }
    }
    impl Ord for ByName {
        fn cmp(&self, other: &Self) -> std::cmp::Ordering {
            self.0.cmp(&other.0)
        }
    }
    let mut total = 0usize;
    for entry in read_dir.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        // file_type() does not follow symlinks; size only for regular files.
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let kind = if file_type.is_dir() {
            "dir"
        } else if file_type.is_file() {
            "file"
        } else if file_type.is_symlink() {
            "symlink"
        } else {
            "other"
        };
        let size = if file_type.is_file() {
            entry.metadata().ok().map(|m| m.len())
        } else {
            None
        };
        total += 1;
        let item = ByName(
            name.clone(),
            json!({"name": name, "type": kind, "size": size}),
        );
        // Max-heap of size `limit`: its root is the largest kept name; a
        // smaller incoming name evicts it, so we retain the smallest `limit`.
        if kept.len() < limit {
            kept.push(item);
        } else if let Some(top) = kept.peek() {
            if item.0 < top.0 {
                kept.pop();
                kept.push(item);
            }
        }
    }

    let after =
        std::fs::symlink_metadata(&canonical).map_err(|e| format!("fs.list {path}: {e}"))?;
    {
        use std::os::unix::fs::MetadataExt;
        if before.dev() != after.dev() || before.ino() != after.ino() {
            return Err(format!(
                "fs.list: '{canonical_str}' changed during the read (possible symlink race); \
                 refusing (fail-closed)"
            ));
        }
    }

    let truncated = total > kept.len();
    let mut sorted = kept.into_vec();
    sorted.sort();
    let listed: Vec<Value> = sorted.into_iter().map(|b| b.1).collect();
    Ok(json!({
        "path": canonical_str,
        "entries": listed,
        "truncated": truncated
    })
    .to_string())
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
    //
    // RESIDUAL TOCTOU (Phase 3): O_NOFOLLOW guards only the FINAL component. The
    // open() below re-walks the whole path, so an agent that swaps an
    // INTERMEDIATE parent directory for a symlink in the tiny window between
    // canonicalize(parent) and this open (vibed runs as root) could still route
    // the create+truncate elsewhere. Unlike fs.read, an O_CREAT|O_TRUNC write
    // cannot be undone by a post-open dev/ino recheck. Full closure needs a
    // single atomic resolve+open — `openat2(RESOLVE_NO_SYMLINKS)` — which lands
    // with per-tool sandboxing in Phase 3. Winning the race is hard (a tight,
    // repeated swap) and the common cases (symlinked final component, symlinked
    // parent already in place at check time) are already refused above.
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
    // A home of '/' (or a broad ancestor) would make is_within() trivially true
    // and disable write confinement — refuse fail-closed.
    if canonical_home_str == "/" {
        return Err(format!(
            "fs.write: uid {uid}'s home resolves to '/' — refusing (would disable confinement)"
        ));
    }
    if !is_within(canonical_target, &canonical_home_str) {
        return Err(format!(
            "fs.write: cross-user write refused: '{canonical_target}' is not within uid {uid}'s \
             home '{canonical_home_str}'"
        ));
    }
    Ok(())
}

/// Which allowed area a read resolved into. The caller applies extra checks per
/// scope (an in-`Home` read must be owned by the caller — hardlink defense —
/// whereas a `System` tree read is legitimately root-owned).
#[derive(PartialEq, Eq, Clone, Copy, Debug)]
enum ReadScope {
    System,
    Home,
}

/// Read confinement for fs.read / fs.list. `canonical` is allowed iff it is a
/// system-read tree (SYSTEM_READ_PREFIXES) OR inside the calling uid's own home
/// (resolved from SO_PEERCRED). Fail-closed: an unknown/unresolvable caller may
/// still read the system trees, but NEVER a home — so a missing peer cred
/// cannot be leveraged to read personal data. The built-in denylist is applied
/// separately (before this) and always wins.
fn confine_read(tool: &str, caller: Caller, canonical: &str) -> Result<ReadScope, String> {
    if SYSTEM_READ_PREFIXES
        .iter()
        .any(|p| canonical == p.trim_end_matches('/') || canonical.starts_with(p))
    {
        return Ok(ReadScope::System);
    }
    // Otherwise it must be within the caller's OWN home.
    let uid = caller.uid.ok_or_else(|| {
        format!(
            "{tool}: '{canonical}' is outside the readable system trees and the caller uid is \
             unavailable (SO_PEERCRED); refusing (fail-closed)"
        )
    })?;
    let home = home_dir_for_uid(uid)
        .ok_or_else(|| format!("{tool}: no home for uid {uid} in /etc/passwd; refusing"))?;
    let canonical_home = std::fs::canonicalize(&home)
        .map_err(|e| format!("{tool}: cannot resolve home '{home}' for uid {uid}: {e}"))?;
    let canonical_home_str = canonical_home.to_string_lossy();
    // A home that resolves to "/" (some system accounts) or any broad ancestor
    // would make is_within() trivially true — home confinement would silently
    // become a no-op, opening every path. Refuse fail-closed instead.
    if canonical_home_str == "/" {
        return Err(format!(
            "{tool}: uid {uid}'s home resolves to '/' — refusing (would disable confinement)"
        ));
    }
    if is_within(canonical, &canonical_home_str) {
        return Ok(ReadScope::Home);
    }
    Err(format!(
        "{tool}: '{canonical}' is neither a readable system path nor inside uid {uid}'s own home; \
         cross-user reads are refused"
    ))
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
pub fn utc_date_string(epoch_secs: u64) -> String {
    let (year, month, day, ..) = utc_civil(epoch_secs);
    format!("{year:04}-{month:02}-{day:02}")
}

/// ISO 8601 UTC timestamp with a `Z` suffix.
pub fn utc_iso8601(epoch_secs: u64) -> String {
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
            "/var/lib/vibeos/approvals/granted/x.json",
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
    fn builtin_denylist_covers_root_via_ostree_alias() {
        // On OSTree/bootc, /root is a symlink to /var/roothome; the fs tools
        // canonicalize before re-checking, so root's home must be denied under
        // BOTH spellings (builtin_denied is alias-blind by itself).
        for path in [
            "/root",
            "/root/.bashrc",
            "/root/.ssh/id_ed25519",
            "/var/roothome",
            "/var/roothome/.bashrc",
            "/var/roothome/notes.txt",
            "/var/roothome/.ssh/id_ed25519",
        ] {
            assert!(
                builtin_denied(path, false).is_some(),
                "{path} must be read-denied (root home, either OSTree spelling)"
            );
        }
        // A sibling under /var that is NOT root's home stays readable — the alias
        // entry must not over-match.
        assert!(builtin_denied("/var/lib/vibeos/x", false).is_none());
        assert!(builtin_denied("/var/roothomeXYZ", false).is_none());
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
        let uid = current_uid();
        if uid == 0 {
            return; // root's home is /root, denylisted
        }
        let policy = permissive_policy();
        let caller = caller_uid(uid);
        // A directory is not a regular file — created in the caller's OWN home
        // so confinement lets the attempt reach the regular-file check.
        let base = home_scratch(uid, "read-nonreg");
        let subdir = base.join("adir");
        std::fs::create_dir_all(&subdir).unwrap();
        let err = fs_read(&json!({"path": subdir.to_string_lossy()}), &policy, caller).unwrap_err();
        assert!(
            err.contains("not a regular file"),
            "directory must be refused: {err}"
        );
        // A FIFO in the caller's own home: open(O_RDONLY) with no writer would
        // BLOCK the worker forever without the pre-open type check (the DoS this
        // guards). It must return a refusal instead of hanging the test.
        let fifo = base.join("pipe");
        let made = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if made {
            let err =
                fs_read(&json!({"path": fifo.to_string_lossy()}), &policy, caller).unwrap_err();
            assert!(
                err.contains("not a regular file"),
                "a FIFO must be refused BEFORE the blocking open: {err}"
            );
        }
        // A char device outside the readable trees is refused by confinement.
        if std::path::Path::new("/dev/zero").exists() {
            let err = fs_read(&json!({"path": "/dev/zero"}), &policy, caller).unwrap_err();
            assert!(
                !err.is_empty(),
                "char device outside the trees must be refused"
            );
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn fs_read_reads_regular_file_and_bounds_large_one() {
        let uid = current_uid();
        if uid == 0 {
            return;
        }
        let policy = permissive_policy();
        let caller = caller_uid(uid);
        let dir = home_scratch(uid, "read-ok");

        let small = dir.join("small.txt");
        std::fs::write(&small, "hello vibed").unwrap();
        let out = fs_read(&json!({"path": small.to_string_lossy()}), &policy, caller)
            .expect("read small");
        assert_eq!(out, "hello vibed");

        let big = dir.join("big.bin");
        std::fs::write(&big, vec![b'a'; MAX_READ_BYTES + 4096]).unwrap();
        let out =
            fs_read(&json!({"path": big.to_string_lossy()}), &policy, caller).expect("read big");
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
    fn fs_read_reads_a_system_tree_regardless_of_caller() {
        // /etc is a readable system tree — even an unknown caller (no peer cred)
        // may read it, but NEVER personal data (see confine_read test).
        let policy = permissive_policy();
        if std::path::Path::new("/etc/os-release").is_file() {
            let out = fs_read(
                &json!({"path": "/etc/os-release"}),
                &policy,
                Caller::default(),
            )
            .expect("system read of /etc/os-release");
            assert!(!out.is_empty());
        }
    }

    #[test]
    fn fs_read_still_reads_through_symlinks_via_canonicalization() {
        // O_NOFOLLOW + the dev/ino recheck guard against TOCTOU swaps, but a
        // legitimate read THROUGH a symlink keeps working: canonicalize
        // resolves the link before open, so the final component of the opened
        // path is the real file.
        let uid = current_uid();
        if uid == 0 {
            return;
        }
        let policy = permissive_policy();
        let caller = caller_uid(uid);
        let dir = home_scratch(uid, "read-link");
        let real = dir.join("real.txt");
        std::fs::write(&real, "via symlink").unwrap();
        let link = dir.join("link.txt");
        std::os::unix::fs::symlink(&real, &link).expect("symlink");
        let out = fs_read(&json!({"path": link.to_string_lossy()}), &policy, caller)
            .expect("reading through a symlink must still work");
        assert_eq!(out, "via symlink");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn confine_read_allows_system_and_own_home_refuses_others() {
        let uid = current_uid();
        let caller = caller_uid(uid);
        // System trees are always readable.
        for p in [
            "/etc/os-release",
            "/usr/bin",
            "/proc/uptime",
            "/var/lib/vibeos/x",
            "/etc",
        ] {
            assert!(
                confine_read("fs.read", caller, p).is_ok(),
                "{p} must be readable"
            );
        }
        // A non-system path outside the caller's home is refused (cross-user).
        assert!(confine_read("fs.read", caller, "/tmp/elsewhere").is_err());
        assert!(confine_read("fs.read", caller, "/var/home/someoneelse/secret").is_err());
        // Unknown caller: system OK, home refused (fail-closed — a missing peer
        // cred can never be used to read personal data).
        assert!(confine_read("fs.read", Caller::default(), "/etc/x").is_ok());
        assert!(confine_read("fs.read", Caller::default(), "/tmp/x").is_err());
        // The caller's OWN home is readable.
        if uid != 0 {
            let home = home_dir_for_uid(uid).expect("caller has a home");
            let canon = std::fs::canonicalize(&home).expect("home resolves");
            let inhome = format!("{}/somefile", canon.to_string_lossy());
            assert!(
                confine_read("fs.read", caller, &inhome).is_ok(),
                "own home must be readable"
            );
        }
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
        assert_eq!(tool_tier("fs.list"), Some(Tier::T0));
        assert_eq!(tool_tier("fs.write"), Some(Tier::T1));
        assert_eq!(tool_tier("pkg.install"), Some(Tier::T2));
        assert_eq!(tool_tier("svc.restart"), Some(Tier::T2));
        assert_eq!(tool_tier("svc.status"), Some(Tier::T0));
        assert_eq!(tool_tier("sectools.list"), Some(Tier::T0));
        assert_eq!(tool_tier("memory.query"), Some(Tier::T0));
        assert_eq!(tool_tier("memory.append"), Some(Tier::T1));
        assert_eq!(
            tool_tier("disk.wipe"),
            None,
            "unknown tool has no tier => default-deny"
        );
    }

    // -- fs.list (T0) ----------------------------------------------------------

    /// A policy allowing fs.list everywhere except an explicit denied subtree,
    /// mirroring the shipped fs-read rule shape.
    fn list_policy() -> PolicyEngine {
        policy_from_toml(
            r#"
            [[rule]]
            id = "fs-read"
            tools = ["fs.read", "fs.list"]
            tier = "T0"
            action = "allow"
            "#,
        )
    }

    #[test]
    fn fs_list_lists_names_types_sizes_sorted() {
        let uid = current_uid();
        if uid == 0 {
            return;
        }
        let policy = list_policy();
        let caller = caller_uid(uid);
        let dir = home_scratch(uid, "list-ok");
        std::fs::create_dir_all(dir.join("sub")).expect("mkdir");
        std::fs::write(dir.join("b.txt"), "12345").unwrap();
        std::fs::write(dir.join("a.txt"), "1").unwrap();
        std::os::unix::fs::symlink("/etc/os-release", dir.join("link")).expect("symlink");
        let payload: Value = serde_json::from_str(
            &fs_list(&json!({"path": dir.to_string_lossy()}), &policy, caller).unwrap(),
        )
        .unwrap();
        let entries = payload["entries"].as_array().unwrap();
        let names: Vec<&str> = entries
            .iter()
            .map(|e| e["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["a.txt", "b.txt", "link", "sub"], "sorted by name");
        assert_eq!(entries[0]["type"], "file");
        assert_eq!(entries[0]["size"], 1);
        assert_eq!(entries[1]["size"], 5);
        assert_eq!(entries[2]["type"], "symlink");
        assert!(
            entries[2]["size"].is_null(),
            "a symlink discloses no target size"
        );
        assert_eq!(entries[3]["type"], "dir");
        assert_eq!(payload["truncated"], false);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fs_list_respects_limit_and_returns_stable_lexicographic_prefix() {
        let uid = current_uid();
        if uid == 0 {
            return;
        }
        let policy = list_policy();
        let caller = caller_uid(uid);
        let dir = home_scratch(uid, "list-lim");
        // Create in a shuffled order so a readdir-order bug would surface.
        for name in ["e", "b", "d", "a", "c"] {
            std::fs::write(dir.join(name), "x").unwrap();
        }
        let payload: Value = serde_json::from_str(
            &fs_list(
                &json!({"path": dir.to_string_lossy(), "limit": 3}),
                &policy,
                caller,
            )
            .unwrap(),
        )
        .unwrap();
        let entries = payload["entries"].as_array().unwrap();
        let names: Vec<&str> = entries
            .iter()
            .map(|e| e["name"].as_str().unwrap())
            .collect();
        // The truncated set must be the SMALLEST `limit` names, sorted — a
        // stable prefix, not an arbitrary readdir-order subset.
        assert_eq!(
            names,
            ["a", "b", "c"],
            "truncation must keep the lexicographic prefix"
        );
        assert_eq!(payload["truncated"], true);
        let err = fs_list(
            &json!({"path": dir.to_string_lossy(), "limit": 0}),
            &policy,
            caller,
        )
        .unwrap_err();
        assert!(err.contains("integer >= 1"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fs_list_refuses_when_target_is_not_a_directory() {
        // canonicalize follows a symlink to its real target; that target must
        // itself be a real directory. A symlink to a regular file is refused.
        let uid = current_uid();
        if uid == 0 {
            return;
        }
        let policy = list_policy();
        let caller = caller_uid(uid);
        let base = home_scratch(uid, "list-sym");
        let file = base.join("regular.txt");
        std::fs::write(&file, "x").unwrap();
        let link = base.join("link-to-file");
        std::os::unix::fs::symlink(&file, &link).expect("symlink");
        let err = fs_list(&json!({"path": link.to_string_lossy()}), &policy, caller).unwrap_err();
        assert!(err.contains("not a directory"), "unexpected error: {err}");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn fs_list_refuses_denylisted_directories() {
        let policy = list_policy();
        // The denylist applies to the DIRECTORY itself: /root and a user's
        // .ssh directory are unlistable, whoever asks.
        for path in ["/root", "/home/dev/.ssh", "/home/dev/.claude"] {
            let err = builtin_denied(path, false);
            assert!(err.is_some(), "{path} must be builtin-denied");
        }
        // End-to-end through fs_list for a path that exists on every test box.
        if std::path::Path::new("/root").exists() {
            let err = fs_list(&json!({"path": "/root"}), &policy, Caller::default()).unwrap_err();
            assert!(err.contains("denylist"), "unexpected error: {err}");
        }
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

    #[test]
    fn approver_suffix_records_operator_identity() {
        // A known approver lands in the audit outcome; an unknown one is `?`.
        assert_eq!(approver_suffix(Some(0)), "(by_uid=0)");
        assert_eq!(approver_suffix(Some(1000)), "(by_uid=1000)");
        assert_eq!(approver_suffix(None), "(by_uid=?)");
    }

    #[test]
    fn agent_thinking_validates_args() {
        // Missing session_id -> error.
        assert!(agent_thinking(&json!({})).is_err());
        // Traversal session_id -> rejected by reasoning::read_thinking before any I/O.
        let err = agent_thinking(&json!({"session_id": "../etc/passwd"})).unwrap_err();
        assert!(err.contains("session_id"), "unexpected error: {err}");
        // agent.thinking is T0 in the catalog.
        assert_eq!(tool_tier("agent.thinking"), Some(Tier::T0));
    }

    #[test]
    fn agent_sessions_returns_a_well_formed_listing() {
        // No arguments; on a machine without the store it yields an empty, valid
        // listing (never an error) — the discovery tool degrades gracefully.
        let out: Value = serde_json::from_str(&agent_sessions().unwrap()).unwrap();
        assert!(
            out["sessions"].is_array(),
            "sessions must be an array: {out}"
        );
        assert!(out["count"].is_u64(), "count must be a number: {out}");
        // 'latest' is null when empty, or the last id otherwise — both are valid.
        assert!(
            out.get("latest").is_some(),
            "latest key must be present: {out}"
        );
        assert_eq!(tool_tier("agent.sessions"), Some(Tier::T0));
    }

    #[test]
    fn agents_list_confines_to_caller_uid_and_excludes_self() {
        let dir = std::env::temp_dir().join(format!("vibed-agents-list-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let log = AuditLog::new(dir.clone());

        let me = Caller {
            uid: Some(1000),
            gid: Some(1000),
            pid: Some(111),
        }; // the HUD / requesting process
        let peer = Caller {
            uid: Some(1000),
            gid: Some(1000),
            pid: Some(222),
        }; // another agent, SAME uid
        let stranger = Caller {
            uid: Some(2000),
            gid: Some(2000),
            pid: Some(333),
        }; // a different user's agent

        log.record(
            "fs.write",
            &json!({}),
            Some("/home/a/x"),
            "allow",
            "ok",
            peer,
        )
        .unwrap();
        log.record("os.status", &json!({}), None, "allow", "ok", me)
            .unwrap();
        log.record(
            "fs.read",
            &json!({}),
            Some("/home/b/y"),
            "allow",
            "ok",
            stranger,
        )
        .unwrap();

        let out: Value = serde_json::from_str(&agents_list(&json!({}), me, &dir).unwrap()).unwrap();
        let agents = out["agents"].as_array().unwrap();

        // Only the PEER process of MY uid (pid 222); never me (111) or the
        // stranger (uid 2000) — cross-user confinement + self-exclusion.
        assert_eq!(agents.len(), 1, "roster must hold exactly the peer: {out}");
        assert_eq!(agents[0]["uid"], 1000);
        assert_eq!(agents[0]["pid"], 222);
        assert_eq!(agents[0]["tier"], 1, "fs.write is T1");
        assert!(
            agents[0]["activity"].as_str().unwrap().contains("fs.write"),
            "activity must carry the last tool: {out}"
        );

        // A caller with no SO_PEERCRED uid sees an empty roster (fail-closed).
        let anon = Caller {
            uid: None,
            gid: None,
            pid: Some(9),
        };
        let out2: Value =
            serde_json::from_str(&agents_list(&json!({}), anon, &dir).unwrap()).unwrap();
        assert_eq!(out2["count"], 0, "an unidentified caller sees nothing");

        assert_eq!(tool_tier("agents.list"), Some(Tier::T0));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn policy_check_classifies_without_executing() {
        let policy = policy_from_toml(
            "[[rule]]\nid=\"os\"\ntools=[\"os.status\"]\ntier=\"T0\"\naction=\"allow\"\n\
             [[rule]]\nid=\"pkg\"\ntools=[\"pkg.install\"]\ntier=\"T2\"\naction=\"allow\"\napproval=\"human\"\n\
             [[rule]]\nid=\"fsread\"\ntools=[\"fs.read\"]\ntier=\"T1\"\naction=\"allow\"\n\
             [[rule]]\nid=\"deny-all\"\ntools=[\"*\"]\ntier=\"T0\"\naction=\"deny\"\n",
        );
        let check = |args: Value| -> Value {
            serde_json::from_str(&policy_check(&args, &policy).unwrap()).unwrap()
        };
        // T0 allowed.
        assert_eq!(check(json!({"tool": "os.status"}))["decision"], "allow");
        // T2 floor: an `allow` at T2 is classified as require_approval.
        assert_eq!(
            check(json!({"tool": "pkg.install", "target": "htop"}))["decision"],
            "require_approval"
        );
        // Built-in denylist wins over policy, whatever the tier.
        let shadow = check(json!({"tool": "fs.read", "target": "/etc/shadow"}));
        assert_eq!(shadow["decision"], "deny");
        assert_eq!(shadow["by"], "builtin_denylist");
        // Unknown tool -> catch-all deny.
        assert_eq!(check(json!({"tool": "disk.wipe"}))["decision"], "deny");
        // Missing tool -> error; policy.check is T0.
        assert!(policy_check(&json!({}), &policy).is_err());
        assert_eq!(tool_tier("policy.check"), Some(Tier::T0));
    }
}
