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
/// Hard cap on a single JSON-RPC request line read from the socket. A client
/// that sends a line longer than this (e.g. no newline ever) is disconnected,
/// so an unbounded line can never exhaust the daemon's memory (DoS guard).
const MAX_LINE_BYTES: usize = 1024 * 1024;
/// Hard cap on the `target` — the call's subject, which is both the approval
/// GRANT KEY and the only description the operator ever reads. Set to Linux's
/// PATH_MAX: no legitimate subject comes near it (a longer path cannot be opened
/// by the kernel at all, a unit name is capped at 255 by `validate_unit_name`,
/// and a package name is a few dozen characters). Anything above is refused
/// outright rather than truncated — see the check in `handle_tools_call`.
const MAX_TARGET_BYTES: usize = 4096;
/// `O_NOFOLLOW` (Linux, `bits/fcntl-linux.h`): open() fails with ELOOP if the
/// final path component is a symbolic link, instead of silently following it.
/// Defined here to keep the crate free of a libc dependency.
pub(crate) const O_NOFOLLOW: i32 = 0x20000;
/// Per-file cap when scanning memory content in memory.query.
pub(crate) const MAX_MEMORY_SCAN_BYTES: usize = 64 * 1024;
/// Chars of content returned per match by memory.query (so an agent can read
/// the memory in ONE call, not query + fs.read). Bounded to keep responses small.
pub(crate) const MEMORY_SNIPPET_CHARS: usize = 1024;
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
pub(crate) const MEMORY_SCOPES: [(&str, &str, bool); 7] = [
    ("identity", "identity.toml", false),
    ("hardware", "hardware.json", false),
    ("personality", "personality.toml", false),
    ("user", "user", true),
    ("projects", "projects", true),
    ("journal", "journal", true),
    ("knowledge", "knowledge", true),
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
    "/etc/krb5.keytab",         // Kerberos service keys
    "/etc/sssd/**",             // SSSD: LDAP/AD bind credentials
    "/etc/ipsec.secrets",       // IPsec PSK/keys
    "/etc/ipsec.d/private/**",  // IPsec private keys
    "/etc/pki/**/private/**",   // TLS/PKI private keys (public certs stay readable)
    "**/.docker/config.json",   // registry auth tokens (Docker)
    "**/.config/containers/auth.json", // Podman/skopeo/buildah registry authfile (image is Podman-first)
    "**/.kube/config",                 // kubernetes cluster credentials
    "**/.netrc",                       // machine login credentials
    "/root/**",                        // root's home directory
    // OSTree/bootc symlinks /root -> /var/roothome, and the fs tools canonicalize
    // before re-checking, so the raw glob above must be mirrored on the canonical
    // spelling too (builtin_denied uses glob_match, which is alias-blind). Reads
    // are already blocked by confine_read for non-root callers; this keeps the
    // denylist itself alias-consistent with the policy matcher. It is the only
    // alias-sensitive entry here — the others are **/-anchored (spelling-agnostic)
    // or under non-symlinked roots (/etc, /proc, /run, /boot, /var/lib/vibeos).
    "/var/roothome/**", // canonical form of /root on OSTree (see above)
    "/proc/*/environ",  // process environments may leak secrets
    "/proc/**/environ", // ...including per-thread /proc/<pid>/task/<tid>/environ
    "/proc/**/cmdline", // command lines may carry secrets/tokens
    // Kernel-memory pseudo-files: vibed reads as ROOT, so these expose kernel
    // memory and symbol addresses (KASLR defeat) to ANY caller — never a
    // legitimate fs.read target (adversarial review 2026-07-14).
    "/proc/kcore",     // kernel core image (physical RAM)
    "/proc/kallsyms",  // kernel symbol addresses (defeats KASLR)
    "/proc/kmsg",      // kernel ring buffer (also a blocking read)
    "/proc/*/mem",     // a process's address space
    "/proc/**/mem",    // ...including per-thread task/<tid>/mem
    "/proc/*/pagemap", // virtual->physical page mapping (exploit primitive)
    "/proc/**/pagemap",
    // Same class, missed by the 2026-07-14 pass and caught by the 2026-07-25
    // adversarial sweep: these are GLOBAL, root-only (mode 0400) pseudo-files, so
    // the per-pid confinement never applies to them and the blanket "/proc/"
    // system-read prefix let vibed-as-root hand them to ANY caller — including one
    // with no SO_PEERCRED identity at all. Each leaks kernel addresses (KASLR) or
    // allocator/hardware layout; none is a legitimate agent read.
    "/proc/vmallocinfo", // kernel vmalloc addresses
    "/proc/slabinfo",    // slab allocator internals
    "/proc/timer_list",  // kernel addresses + timer internals
    "/proc/iomem",       // physical memory map
    "/proc/ioports",     // I/O port map
    "/proc/keys",        // kernel keyring
    "/proc/key-users",   // kernel keyring, per-user
    // Per-process ASLR / kernel-state files. environ+cmdline+mem+pagemap were
    // already covered; these were not, and they defeat ASLR (auxv carries
    // AT_RANDOM/AT_BASE) or expose kernel stack/registers. Denied for EVERY pid,
    // not just other users': the per-pid owner check alone was not enough (a
    // setuid process keeps the caller's real uid — see `parse_proc_uids`).
    "/proc/*/auxv",
    "/proc/**/auxv",
    "/proc/*/stack",
    "/proc/**/stack",
    "/proc/*/syscall",
    "/proc/**/syscall",
    "/run/credentials/**", // decrypted systemd credentials
    // Per-user runtime dirs are mode 0700; vibed-as-root would otherwise let
    // caller A read caller B's session state/sockets/tokens (cross-user).
    "/run/user/**",    // XDG_RUNTIME_DIR of every user
    "/run/secrets/**", // systemd/container secrets convention
    "/boot/**",        // boot chain is none of the agent's business
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
    "**/.git-credentials",         // plaintext git credentials store (home-root)
    "**/.config/git/credentials",  // git-credential-store XDG fallback (git ships w/ gh, lazygit)
    "**/.config/sops/**",          // SOPS/age private keys (keys.txt)
];

/// Additional BUILT-IN denylist for WRITES only: agents may query the memory
/// through memory.query but never write it via fs.write — memory.append is
/// the governed write path (scope-based, no path argument, so this path
/// denylist cannot and need not apply to it) — and the policy itself is not
/// agent-writable.
const BUILTIN_DENY_WRITE: &[&str] = &[
    "/etc/vibeos/policy.d/**",
    "/var/lib/vibeos/memory/**",
    // The operating-mode record (ADR-027). An agent must NEVER be able to flip
    // itself into open mode via fs.write — the unlock is a human, out-of-band
    // `vibectl mode open` only. This is belt-and-suspenders (fs.write is already
    // confined to the caller's home, and the file is root-only), kept explicit
    // because it is the no-self-escalation invariant the whole model rests on.
    "/var/lib/vibeos/mode.json",
];

/// Returns the matched pattern when `path` (already normalized) hits the
/// built-in denylist. `write` selects the additional write-only entries.
pub(crate) fn builtin_denied(path: &str, write: bool) -> Option<&'static str> {
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
    // Operating-mode record (ADR-027). Production passes `crate::mode::MODE_PATH`;
    // tests inject a scratch path so the open-mode auto-grant is exercisable over
    // the real socket. Read-only here — the daemon never WRITES it (only the
    // out-of-band `vibectl mode` operator action does).
    mode_path: std::path::PathBuf,
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
                let response = dispatch(
                    request,
                    &policy,
                    &audit,
                    &limiter,
                    caller,
                    &approval_dir,
                    &mode_path,
                )
                .await;
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
    audit: &Arc<AuditLog>,
    limiter: &crate::ratelimit::RateLimiter,
    caller: Caller,
    approval_dir: &std::path::Path,
    mode_path: &std::path::Path,
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
                mode_path,
            )
            .await
        }
        other => error_response(id, -32601, &format!("method not found: {other}")),
    }
}

// The call pipeline threads the per-connection environment (policy, audit,
// limiter, approval store, mode store) alongside the per-call (id, params,
// caller). Grouping the five environment refs into a struct would trade one
// honest signature for an indirection the whole file would then read through;
// the governed surface is small and stable, so the flat signature stays.
#[allow(clippy::too_many_arguments)]
async fn handle_tools_call(
    id: Value,
    params: Value,
    policy: &Arc<PolicyEngine>,
    audit: &Arc<AuditLog>,
    limiter: &crate::ratelimit::RateLimiter,
    caller: Caller,
    approval_dir: &std::path::Path,
    mode_path: &std::path::Path,
) -> Value {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if name.is_empty() {
        return error_response(id, -32602, "invalid params: missing tool name");
    }
    // Shared, never deep-cloned again: the audit records (blocking pool) and
    // the execution closure each take an `Arc` on the same parsed arguments —
    // a `fs.write` payload close to MAX_LINE_BYTES used to be deep-cloned for
    // the execution task on every allowed call.
    let args = Arc::new(params.get("arguments").cloned().unwrap_or(Value::Null));
    // Everything constant across THIS call's audit records, bundled once: the
    // records differ only by (target, decision, outcome).
    let actx = AuditCtx {
        audit: Arc::clone(audit),
        tool: name.clone(),
        args: Arc::clone(&args),
        digest: Arc::new(ArgsDigest::new()),
        caller,
    };

    // Extract and normalize the call context before any decision is made.
    let raw_path = args.get("path").and_then(Value::as_str);
    let normalized_path = match raw_path {
        Some(raw) => match normalize_path(raw) {
            Some(normalized) => Some(normalized),
            None => {
                // Relative path or attempt to climb above `/`: fail-closed.
                // The raw (rejected) path is the non-secret audit target.
                try_audit(&actx, Some(raw), Decision::Deny, "blocked_invalid_path").await;
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
    let service_owned = derive_service(&name, raw_service);
    let service = service_owned.as_deref();
    let raw_url = args.get("url").and_then(Value::as_str);
    let domain_owned = derive_domain(&name, raw_url);
    let domain = domain_owned.as_deref();
    let raw_provider = args.get("provider").and_then(Value::as_str);
    let raw_target = args.get("target").and_then(Value::as_str);
    let deploy_owned = derive_deploy(&name, raw_provider, raw_target);
    let deploy = deploy_owned
        .as_ref()
        .map(|(p, t)| crate::policy::DeployTarget {
            provider: p,
            target: t,
        });

    // ADR-022 : `browser.run` porte un BATCH (`steps`), pas un `url`/`path` unique. On le parse
    // UNE fois ici — même source de vérité (`parse_batch`) que le helper — pour en dériver le tier
    // effectif (max des verbes) et les domaines `navigate` (lint `[rule.domains]` + clé de grant).
    // Fail-closed : un batch invalide est refusé AVANT toute décision, jamais exécuté.
    let browser_batch = if name == "browser.run" {
        match crate::tools::browser::parse_batch(&args) {
            Ok(b) => Some(b),
            Err(e) => {
                try_audit(&actx, None, Decision::Deny, "browser_batch_invalid").await;
                return tool_result(id, format!("browser.run: {e}"), true);
            }
        }
    } else {
        None
    };
    let browser_domains = browser_batch
        .as_ref()
        .map(crate::tools::browser::batch_domains)
        .unwrap_or_default();

    // Human-readable, non-secret target recorded in the audit trail so an
    // action's subject (which file / unit / package) is recoverable in
    // forensics — never any file content or secret argument.
    //
    // M4 + F2 (Fable 5) : pour `browser.run`, la clé de grant lie (a) l'ENSEMBLE trié des hosts
    // `navigate` ET (b) le BATCH exact — un digest du payload canonique (steps ordonnés). (a) seul
    // (M4) empêche le blanc-seing tous-domaines ; (b) empêche la SUBSTITUTION de contenu à tier et
    // hosts égaux (submit d'un AUTRE formulaire sur les mêmes hosts recyclant l'approbation).
    // L'opérateur voit les hosts + le nombre d'étapes ; le digest scelle le contenu. Le tier fait
    // partie de la clé côté approbation (F1). Toujours présent pour browser.run (même sans navigate
    // — le digest lie tout de même le batch).
    let target = if let Some(batch) = &browser_batch {
        let digest = crate::sha256::sha256_hex(args.to_string().as_bytes());
        Some(format!(
            "{} | {} steps #{}",
            browser_domains.join(","),
            batch.steps.len(),
            &digest[..16]
        ))
    } else {
        audit_target(&name, normalized_path.as_deref(), service, &args)
    };

    // A target longer than any legitimate subject is refused OUTRIGHT, before it
    // can be recorded anywhere.
    //
    // Why reject rather than truncate: `target` is the approval GRANT KEY
    // (compared verbatim) and the only description the operator is shown. Cutting
    // it for display would hand an agent a deception primitive it does not have
    // today — the operator would read a prefix and approve the whole string. So
    // the bound has to live HERE, where the call can still be refused, and the
    // stored value stays exactly what executes.
    //
    // Why it matters: the only other ceiling is MAX_LINE_BYTES (1 MiB), and an
    // agent may park MAX_PENDING_PER_UID (16) requests. That is ~16 MiB of
    // arbitrary text sitting in the approval queue and dumped verbatim to the
    // operator's terminal — and `target` renders BEFORE `tier`/`tool` (the JSON
    // object is a BTreeMap, so keys print alphabetically), so a wall of garbage
    // pushes the actual action off screen. It grants nothing, but a review
    // surface that cannot be read is a review that does not happen.
    //
    // The bound is PATH_MAX: no legitimate subject comes near it — a path longer
    // than this cannot be opened by the kernel anyway, a unit name is capped at
    // 255 by validate_unit_name, and a package name is a few dozen characters.
    if let Some(t) = target.as_deref() {
        if t.len() > MAX_TARGET_BYTES {
            // Audit the LENGTH, not the value: echoing 1 MiB of hostile text
            // into the audit trail is the same flood, one file over.
            try_audit(
                &actx,
                Some(&format!("<target too long: {} bytes>", t.len())),
                Decision::Deny,
                "target_too_long",
            )
            .await;
            return tool_result(
                id,
                format!(
                    "policy: target is {} bytes; the maximum is {MAX_TARGET_BYTES} \
                     (no legitimate path, unit or package name is that long)",
                    t.len()
                ),
                true,
            );
        }
    }

    // Per-uid rate limiting: bound a runaway or compromised agent BEFORE any
    // execution, memory write or approval-store growth. Over-limit calls are
    // refused fail-closed and audited (the rejection itself is a security
    // signal), never executed. Keyed by the unforgeable SO_PEERCRED uid.
    if !limiter.check(caller.uid, now_epoch_secs()) {
        try_audit(&actx, target.as_deref(), Decision::Deny, "rate_limited").await;
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
                &actx,
                target.as_deref(),
                Decision::Deny,
                "blocked_builtin_denylist",
            )
            .await;
            return tool_result(
                id,
                format!("policy: path '{path}' is denied by the built-in denylist ({pattern})"),
                true,
            );
        }
    }

    let ctx = CallContext {
        path: normalized_path.as_deref(),
        service,
        domain,
        deploy,
    };
    // `browser.run` : tier effectif = MAX des verbes du batch (submit → T2), et lint
    // `[rule.domains]` défense-en-profondeur — la décision la plus STRICTE sur les hosts `navigate`
    // (le proxy CONNECT reste le vrai gate par-requête une fois le relais construit, ADR-022).
    // Tout le reste : le chemin générique (tier statique du catalogue, contexte unique).
    let (tier, decision) = if let Some(batch) = &browser_batch {
        let t = crate::tools::browser::batch_tier(batch);
        (Some(t), govern_browser_batch(policy, t, &browser_domains))
    } else {
        let t = tool_tier(&name);
        (t, policy.evaluate(&name, t, ctx))
    };

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
    // Both blocking store reads happen together, off the reactor, ONLY for a
    // T2/T3 RequireApproval: (1) is there a fresh one-shot human grant, and — if
    // not — (2) is OPEN MODE active (ADR-027)? A human grant is more specific
    // (it names the approver), so it wins; open mode is the blanket, bounded,
    // human-unlocked fallback. Neither ever runs for a Deny: an explicit deny
    // rule, the default-deny, and the built-in denylist were all decided ABOVE
    // and open mode cannot revive them — it only lifts the approval FLOOR.
    let (consumed, open_mode) = if matches!(decision, Decision::RequireApproval) {
        let name_g = name.clone();
        let target_g = target.clone();
        let tier_g = tier.map(Tier::as_str);
        let uid_g = caller.uid;
        let now_g = now_epoch_secs();
        let dir_g = approval_dir.to_path_buf();
        let mode_g = mode_path.to_path_buf();
        tokio::task::spawn_blocking(move || {
            let grant = crate::approval::check_and_consume_grant(
                &dir_g,
                &name_g,
                target_g.as_deref(),
                tier_g,
                uid_g,
                now_g,
            );
            // Consult open mode only when no human grant applied.
            let open = grant.is_none() && crate::mode::is_open(&mode_g, now_g);
            (grant, open)
        })
        .await
        .unwrap_or((None, false)) // a panic -> no grant, not open (fail-closed)
    } else {
        (None, false)
    };
    let human_approved = consumed.is_some();
    let approved = human_approved || open_mode;
    let decision = if approved { Decision::Allow } else { decision };
    let suffix = consumed
        .as_ref()
        .map(|c| approver_suffix(c.approver_uid))
        .unwrap_or_default();
    // Distinct audit outcomes so the trail never conflates the three paths:
    // a human one-shot approval, an autonomous open-mode auto-grant, or a plain
    // allowed call. Open-mode calls are always attributable AS open-mode.
    let (started_outcome, ok_outcome) = if human_approved {
        (
            format!("started_approved{suffix}"),
            format!("ok_approved{suffix}"),
        )
    } else if open_mode {
        ("started_open_mode".to_string(), "ok_open_mode".to_string())
    } else {
        ("started".to_string(), "ok".to_string())
    };

    match decision {
        Decision::Deny => {
            try_audit(&actx, target.as_deref(), decision, "blocked").await;
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
            try_audit(&actx, target.as_deref(), decision, "pending_approval").await;
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
            if !try_audit(&actx, target.as_deref(), decision, &started_outcome).await {
                return tool_result(
                    id,
                    "audit log unavailable: refusing execution (fail-closed)".to_string(),
                    true,
                );
            }
            let tool_name = name.clone();
            let tool_args = Arc::clone(&args);
            let policy_exec = Arc::clone(policy);
            let caller_exec = caller;
            let audit_dir_exec = audit.dir().to_path_buf();
            let mode_path_exec = mode_path.to_path_buf();
            // Tool bodies use blocking std::fs; keep the reactor responsive.
            let executed = tokio::task::spawn_blocking(move || {
                execute_tool(
                    &tool_name,
                    &tool_args,
                    &policy_exec,
                    caller_exec,
                    &audit_dir_exec,
                    &mode_path_exec,
                )
            })
            .await;
            match executed {
                Ok(Ok(text)) => {
                    try_audit(&actx, target.as_deref(), decision, &ok_outcome).await;
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
                        &actx,
                        target.as_deref(),
                        decision,
                        &format!("error: {message}"),
                    )
                    .await;
                    tool_result(id, message, true)
                }
                Err(join_error) => {
                    try_audit(&actx, target.as_deref(), decision, "panic").await;
                    tool_result(id, format!("internal error: {join_error}"), true)
                }
            }
        }
    }
}

/// Derive the non-secret audit target for a call: the normalized path for
/// filesystem tools, the unit for service tools, the package name for
/// pkg.install. Returns `None` when the tool carries no such subject.
/// SECURITY-CRITICAL — this is not merely a log field.
///
/// `target` is (1) the approval GRANT KEY (`approval::check_and_consume_grant`
/// matches `(tool, target, uid)` by exact string compare) and (2) the ONLY
/// description of the action the operator ever sees: the pending record carries
/// `{id, tool, target, tier, caller_uid}` and NOT the arguments. Meanwhile
/// `execute_tool` re-reads the RAW arguments. So the target must name the thing
/// the tool will actually act on — otherwise one approval authorises a different
/// action, and the audit trail describes something that did not happen.
///
/// It is therefore derived PER TOOL, from the argument that tool actually uses.
/// It used to be "the first non-None of (path, unit, package name)" for every
/// tool, which broke that binding in both directions, because `path` and `unit`
/// are read off the arguments of ANY call:
///   * `svc.restart {"unit":"x","path":"/etc/nginx/nginx.conf"}` → the path won,
///     so the operator was asked to approve `svc.restart` on a plausible-looking
///     CONFIG FILE, the grant was keyed on that path, and the same grant then
///     authorised restarting ANY other non-denied unit (the unit is never
///     compared to the grant). The audit recorded the path, not the unit.
///   * `pkg.install {"name":"evil","unit":"vim"}` → the unit won, so a grant
///     approved for `vim` matched a call installing `evil`.
///
/// Binding the target to the tool's real subject closes both.
fn audit_target(
    name: &str,
    normalized_path: Option<&str>,
    service: Option<&str>,
    args: &Value,
) -> Option<String> {
    if name.starts_with("fs.") {
        return normalized_path.map(str::to_string);
    }
    if unit_bearing(name) {
        return service.map(str::to_string);
    }
    if deploy_bearing(name) {
        // Bind the grant/audit to the DERIVED (provider, target) pair the verdict
        // checked — never raw args — so a one-shot approval for one listed pair
        // can NEVER be spent on another, and the operator (and the audit trail)
        // see exactly which target is being deployed (ADR-021 lock 1; Fable 5).
        let raw_provider = args.get("provider").and_then(Value::as_str);
        let raw_target = args.get("target").and_then(Value::as_str);
        return derive_deploy(name, raw_provider, raw_target).map(|(p, t)| format!("{p}:{t}"));
    }
    if name == "pkg.install" {
        return args.get("name").and_then(Value::as_str).map(str::to_string);
    }
    // Every other tool acts on no nameable subject (os.status, memory.*,
    // agent.*, sectools.list, policy.check). A `path`/`unit` smuggled into their
    // arguments is read by nothing — recording it would put caller-controlled
    // fiction in the audit trail.
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
    let name = name.to_string();
    let target = target.map(str::to_string);
    // Fire-and-forget on the blocking pool, not awaited (the dropped
    // JoinHandle detaches the task; it still runs): this append is best-effort
    // by contract (see the call site — a failure never fails the
    // already-succeeded call), and its open/write contends on APPEND_LOCK with
    // memory.append writers running on blocking threads. Awaiting it inline
    // would park a reactor worker on a mutex whose hold time is disk-bound.
    drop(tokio::task::spawn_blocking(move || {
        if let Err(e) = crate::tools::memory::journal_tool_call_at(
            std::path::Path::new(MEMORY_DIR),
            now,
            &name,
            target.as_deref(),
            tier_str,
            caller,
        ) {
            warn!("memory journal (tool_call) write failed for '{name}': {e}");
        }
    }));
}

/// Current unix time in whole seconds (0 on a clock error). Honest note: with
/// `now == 0`, the TTL comparisons in `approval.rs` (`now >= expires`,
/// `now - ts >= TTL`) never fire, so grants are NOT force-expired and stale
/// pendings are NOT pruned. That is not a bypass: grant consumption stays
/// one-shot (atomic unlink) and the per-uid/global pending caps still bound the
/// store — a stuck clock can neither grant nor replay anything. A system clock
/// at the unix epoch is itself a catastrophic failure, well outside this model.
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

/// Lazily-computed, per-call FNV digest of the tool arguments, shared between
/// the `started` and final audit records of one call (both always carried the
/// same digest — same arguments). `OnceLock` so the serialization + hash run at
/// most once per call, and only on the blocking pool, never on the reactor.
type ArgsDigest = std::sync::OnceLock<String>;

/// Everything constant across ONE `tools/call`'s audit records — the log
/// handle, the tool name, the parsed arguments with their lazily-computed
/// digest, and the caller identity. Built once per call in
/// `handle_tools_call`; the individual records then differ only by
/// `(target, decision, outcome)`.
struct AuditCtx {
    audit: Arc<AuditLog>,
    tool: String,
    args: Arc<Value>,
    digest: Arc<ArgsDigest>,
    caller: Caller,
}

/// Append one audit record from the async path WITHOUT parking the reactor on
/// disk I/O. `AuditLog::record` holds the chain mutex across an open, a write
/// and a deliberate fsync — milliseconds on NVMe, tens on slower media — and it
/// used to run directly on a tokio worker, serializing EVERY connection (the
/// accept loop included) behind whichever call was fsync-ing. The write now
/// runs on the blocking pool and is awaited to completion, so the fail-closed
/// contract is untouched: callers still observe the record durably on disk (or
/// a failure) before proceeding — only WHERE the wait happens moves. A panic in
/// the audit task counts as an audit failure (fail-closed), like an I/O error.
async fn try_audit(
    ctx: &AuditCtx,
    target: Option<&str>,
    decision: Decision,
    outcome: &str,
) -> bool {
    let audit = Arc::clone(&ctx.audit);
    let tool = ctx.tool.clone();
    let args = Arc::clone(&ctx.args);
    let digest = Arc::clone(&ctx.digest);
    let caller = ctx.caller;
    let target = target.map(str::to_string);
    let outcome = outcome.to_string();
    tokio::task::spawn_blocking(move || {
        let d = digest.get_or_init(|| crate::audit::fnv1a_64_hex(args.to_string().as_bytes()));
        match audit.record_with_digest(
            &tool,
            d,
            target.as_deref(),
            decision.as_str(),
            &outcome,
            caller,
        ) {
            Ok(()) => true,
            Err(e) => {
                warn!("audit write failed for tool '{tool}': {e}");
                false
            }
        }
    })
    .await
    .unwrap_or(false) // a panic in the audit task -> no audit (fail-closed)
}

// ---------------------------------------------------------------------------
// Tool registry
// ---------------------------------------------------------------------------

/// (name, tier, description, input JSON Schema)
///
/// Built once and cached for the daemon's lifetime (the registry is immutable
/// by construction). It used to be rebuilt — 18 tuples, each with a `json!`
/// schema tree — on every `tool_tier` lookup, i.e. on every `tools/call` AND
/// once per audit record walked by the `agents.list` roster loop, which the
/// HUD polls continuously.
fn tool_catalog() -> &'static [(&'static str, Tier, &'static str, Value)] {
    static CATALOG: std::sync::OnceLock<Vec<(&'static str, Tier, &'static str, Value)>> =
        std::sync::OnceLock::new();
    CATALOG.get_or_init(build_tool_catalog)
}

/// The one-time construction behind `tool_catalog` — never call this directly.
fn build_tool_catalog() -> Vec<(&'static str, Tier, &'static str, Value)> {
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
             restarted and read back to confirm; strict unit-name validation). The \
             result carries `readback`: \"confirmed\" means the fresh ActiveState/\
             SubState/ActiveEnterTimestamp in the payload PROVE the restart; \
             \"unavailable\" means the restart succeeded but the proof could not be \
             obtained (see `readback_note`) — do not report it as verified",
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
            "sandbox.probe",
            Tier::T1,
            "Prove the ADR-019 sandbox confines: spawns a transient hardened systemd \
             service running the low-privilege vibed-tool helper, captures its \
             /proc/self confinement report, and GRADES it against the chosen profile \
             (returns confined:true/false + the failing checks). Benign self-test — no \
             credential, no network beyond the deny-floor, no target, ephemeral. \
             'class': \"deploy\" (strict, default) or \"browser\" (relaxed for Chromium)",
            json!({"type": "object",
                   "properties": {"class": {"type": "string", "enum": ["deploy", "browser"]}}}),
        ),
        (
            "browser.run",
            // Tier de PLANCHER (affichage/tools.list) : un batch purement lecture est T1. Le tier
            // EFFECTIF est calculé par batch (max des verbes ; `submit` → T2/approbation) dans
            // handle_tools_call — voir `batch_tier`. Ne jamais sous-gouverner via ce plancher seul.
            Tier::T1,
            "Run a governed browser batch (ADR-022): a `steps` array — navigate/read/screenshot/\
             click/fill/submit — driven by ONE ephemeral, credential-free headless Chromium. The \
             tier is the MAX over the batch's verbs (a `submit` escalates the whole batch to T2, \
             human approval). Egress is confined to the approved `navigate` domains. `submit` MUST \
             be the last step (anti double-POST).",
            json!({"type": "object", "required": ["steps"],
                   "properties": {"steps": {"type": "array", "items": {"type": "object"}}}}),
        ),
        (
            "deploy.plan",
            Tier::T2,
            "Read a deployment's current state (READ-ONLY), governed. Runs the \
             provider CLI's status/inspect command inside the ADR-019 sandbox with a \
             sealed READ-ONLY token, and returns its output. Gated by a [rule.deploy] \
             verdict (which (provider, target) is allowed) + human approval (T2). \
             Denied until the operator adds a [rule.deploy] rule and provisions the \
             sealed token + egress CIDRs. 'provider': fly|vercel|railway ; 'target': \
             the IMMUTABLE id (Fly app-name, Vercel prj_…, Railway project-id)",
            json!({"type": "object", "required": ["provider", "target"],
            "properties": {
                "provider": {"type": "string", "enum": ["fly", "vercel", "railway"]},
                "target": {"type": "string"}
            }}),
        ),
        (
            "log.read",
            Tier::T0,
            "Read the last N lines (default 50, hard cap 200) of ONE systemd unit's \
             journal — EXFILTRATION-SENSITIVE (ADR-011). NOT a generic journalctl: the \
             unit must be on the policy allowlist ([rule.services].allowed) or the call \
             is denied; output is byte-capped and passes a best-effort secret-redaction \
             pass (defense in depth, not a guarantee). No free grep/regex filter",
            json!({"type": "object", "required": ["unit"],
                   "properties": {"unit": {"type": "string"},
                                  "lines": {"type": "integer", "minimum": 1, "maximum": 200}}}),
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
             (identity/hardware/personality/user/projects/journal/knowledge) and 'limit'. With \
             'fold': true on scope 'user', 'projects' or 'knowledge', returns the CONSOLIDATED \
             current view (last-write-wins dedup of the append-only log) instead of raw matches. \
             With 'rank': true on scope 'journal' or 'knowledge', returns EVENTS ranked by a \
             recall score (recency + importance + relevance), so 'limit' keeps the most relevant \
             rather than the first found. Scope 'personality' is the AI citizen's own character \
             chosen at birth (docs/MEMORY.md §9)",
            json!({"type": "object",
            "properties": {
                "query": {"type": "string"},
                "scope": {"type": "string",
                          "enum": ["identity", "hardware", "personality", "user",
                                   "projects", "journal", "knowledge"]},
                "limit": {"type": "integer", "minimum": 1},
                "fold": {"type": "boolean"},
                "rank": {"type": "boolean"}
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
            "List the autonomous sessions that have captured reasoning \
             (/var/lib/vibeos/memory/reasoning/*.jsonl). Returns { sessions: [{ id, \
             started_unix (null if unknown), last_unix, bytes }...], count, total, \
             truncated, latest }, newest activity first; 'latest' is the most \
             recently appended session. Read-only discovery so an observer (the HUD) \
             can find a session to feed to agent.thinking and render a history. \
             Output is capped at 200 sessions ('total' reports how many exist). \
             Carries no provider/model: the reasoning store does not hold them. \
             No arguments.",
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
            "agent.activity",
            Tier::T0,
            "YOUR OWN recent governed tool calls, newest first, from the audit \
             trail — CONFINED to your uid (never another user's). The 'deeds' \
             counterpart to agent.thinking ('thoughts') and policy.capabilities \
             (the static map): it INCLUDES your refusals (decision deny / \
             require_approval), so you learn the policy boundaries you actually \
             hit, not only the ones you can read. Optional 'window_seconds' \
             (default 3600, max 3600) and 'limit' (default/max 200). Returns \
             { activity: [{ ts_unix, when, tool, target, decision, outcome, pid \
             }], count, total_in_window, truncated, window_seconds, uid }. \
             Read-only; exposes nothing agents.list would not for your own uid.",
            json!({"type": "object", "properties": {
                "window_seconds": {"type": "integer", "minimum": 1},
                "limit": {"type": "integer", "minimum": 1}
            }}),
        ),
        (
            "agent.identity",
            Tier::T0,
            "WHO YOU ARE on this machine: your BIRTH character, chosen at first boot \
             and derived deterministically from the machine id (ADR-029). Returns \
             { born, name, archetype, tone, birth, seed, traits{curiosity, caution, \
             initiative, warmth, concision, playfulness}, style{...} } — or \
             { born:false, note } before Genesis has run. `style` translates those \
             traits into ACTIONABLE conduct (verbosity, confirmation, next_steps, \
             explanation, exploration, register, plus a one-line `directive` you can \
             follow): it is CONDUCT ONLY and can never lower the governance floor — \
             a low-caution style never skips a tier, an approval or the audit. The \
             fourth face of self-knowledge, beside \
             agent.thinking (your thoughts), agent.activity (your deeds) and \
             user.model (your model of the human). Read-only; reads only your PUBLIC \
             character (personality.toml) — never hostname or machine_id. This is \
             constant across boots (amnesic included); what you LEARN lives in \
             memory.query. No arguments.",
            json!({"type": "object", "properties": {}}),
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
        (
            "policy.capabilities",
            Tier::T0,
            "Read the governed capability surface as a JSON manifest DERIVED from the \
             loaded policy: each rule's tools, tier, action, approval mode, and target \
             constraints (allowed paths/services/domains, deploy targets). Lets the \
             agent PLAN in reality instead of discovering limits by refusal. INDICATIVE \
             only — the authoritative decision is always the per-call evaluation \
             (first-match, tier floor, [rule.domains] predicate, [rule.deploy] verdict, \
             context constraints); anything no allow rule covers is default-denied. \
             Read-only, takes no arguments.",
            json!({"type": "object", "properties": {}}),
        ),
        (
            "user.model",
            Tier::T0,
            "The machine's DERIVED, TRANSPARENT understanding of YOU — how you \
             work — and a deterministic anticipation of your next moves (ADR-028, \
             symbiose). Folded from data VibeOS already holds under governance, \
             CONFINED to your uid (never another user): your explicit \
             preferences (user-memory fold), your patterns and rhythm and the \
             friction you hit (from the audit trail), what the agent has observed \
             (recent journal notes), and 'anticipations' — your most likely next \
             governed actions ranked by a frequency×recency heuristic, each with \
             its reason. NOT a trained predictor, NOT hidden profiling, NOT raw \
             keystrokes: every field is derived from records you can inspect, and \
             it exposes nothing memory.query + agent.activity would not for your \
             own uid. Optional 'window_seconds' (default 7d, max 30d). Read-only.",
            json!({"type": "object", "properties": {
                "window_seconds": {"type": "integer", "minimum": 1}
            }}),
        ),
    ]
}

pub(crate) fn tool_tier(name: &str) -> Option<Tier> {
    tool_catalog().iter().find(|t| t.0 == name).map(|t| t.1)
}

/// Does this tool name a systemd unit? These are the ONLY tools for which
/// `CallContext.service` is meaningful, and therefore the only ones a
/// `[rule.services]` constraint can ever govern.
///
/// Single-sourced deliberately. The real pipeline (`handle_tools_call`) and the
/// dry-run hint (`policy.check`) each derived this notion independently and had
/// drifted: the real path fed the raw `unit` argument of ANY tool into the policy
/// context, while the hint only ever set a unit for `svc.*`/`log.read`. Two
/// spellings of one rule is how the hint ends up predicting a decision the daemon
/// does not make. Adding a unit-bearing tool means editing this — once.
pub(crate) fn unit_bearing(tool: &str) -> bool {
    tool.starts_with("svc.") || tool == "log.read"
}

/// The systemd unit a call targets, AS THE POLICY SEES IT. The one helper both
/// `handle_tools_call` (the real decision) and `policy_check` (the dry-run hint)
/// call, so the two cannot drift apart again — parity is structural here, not
/// merely asserted by a test.
///
/// Canonicalizes BEFORE the decision so a bare `sshd` matches a deny rule that
/// spells `sshd.service` (without this an agent drops the suffix and walks past
/// the deny-list). An invalid name falls through to the raw string: it will not
/// match any allow rule, and execution re-validates and refuses — fail-closed.
/// A tool that carries no unit gets `None`: its `unit` argument, if any, is
/// caller-supplied data that nothing reads, and must never reach the policy.
fn derive_service(tool: &str, raw_unit: Option<&str>) -> Option<String> {
    if !unit_bearing(tool) {
        return None;
    }
    let raw = raw_unit?;
    Some(crate::tools::svc::validate_unit_name(raw).unwrap_or_else(|_| raw.to_string()))
}

/// Does this tool carry a `url` argument the policy is entitled to read?
///
/// Same reasoning as `unit_bearing`: an allow-list is only as good as its
/// certainty about WHAT it is matching. A `url` on a tool that has no business
/// with URLs is caller-supplied noise, and must never reach a `[rule.domains]`
/// scope — otherwise an agent appends `"url": "docs.rs"` to an unrelated call
/// and borrows a trusted rule's tier.
fn url_bearing(tool: &str) -> bool {
    tool.starts_with("browser.")
}

/// The host a call targets, AS THE POLICY SEES IT. Twin of `derive_service`:
/// the ONE helper both `handle_tools_call` (the real decision) and
/// `policy_check` (the dry-run hint) call, so the two cannot drift apart —
/// parity is structural here, not merely asserted by a test. That drift is not
/// hypothetical: it already shipped once for units.
///
/// `None` means "no host could be established", never "any host". A rule scoped
/// with `[rule.domains]` does not apply to a `None` host (see
/// `policy::rule_domain_applies`), so an unparseable or hostile URL falls
/// through to the untrusted rule and meets a human instead of inheriting T1.
fn derive_domain(tool: &str, raw_url: Option<&str>) -> Option<String> {
    if !url_bearing(tool) {
        return None;
    }
    crate::domain::host_of(raw_url?)
}

/// Gouverne un batch `browser.run` : rend la décision la plus **STRICTE**
/// (`Deny` > `RequireApproval` > `Allow`) obtenue en évaluant la politique à `tier` pour CHAQUE
/// host `navigate` du batch. `[rule.domains]` est un **prédicat** (un host hors-liste rend la règle
/// non-applicable → retombe sur le défaut deny), donc un seul host non couvert suffit à refuser ou
/// escalader tout le batch. Un batch **sans** `navigate` (rien à cadrer) est évalué une fois avec
/// `domain = None`. Ceci est le **lint défense-en-profondeur** — le proxy CONNECT reste le vrai
/// gate par-requête (ADR-022) une fois le relais construit.
fn govern_browser_batch(policy: &PolicyEngine, tier: Tier, domains: &[String]) -> Decision {
    let eval = |domain: Option<&str>| {
        policy.evaluate(
            "browser.run",
            Some(tier),
            CallContext {
                path: None,
                service: None,
                domain,
                deploy: None,
            },
        )
    };
    if domains.is_empty() {
        return eval(None);
    }
    domains
        .iter()
        .map(|d| eval(Some(d.as_str())))
        .fold(Decision::Allow, strictest)
}

/// La plus stricte de deux décisions (`Deny` > `RequireApproval` > `Allow`) — fail-closed.
fn strictest(a: Decision, b: Decision) -> Decision {
    fn rank(d: &Decision) -> u8 {
        match d {
            Decision::Deny => 2,
            Decision::RequireApproval => 1,
            Decision::Allow => 0,
        }
    }
    if rank(&b) > rank(&a) {
        b
    } else {
        a
    }
}

/// Does this tool carry a deploy `(provider, target)` the policy governs?
/// Same reasoning as `unit_bearing`/`url_bearing`: a `provider`/`target` on a
/// tool that has no business deploying is caller-supplied noise and must never
/// reach a `[rule.deploy]` verdict — otherwise an agent appends
/// `"provider": "fly", "target": "…"` to an unrelated call to borrow a deploy
/// rule's tier. Only `deploy.*` tools bear a deploy target.
pub(crate) fn deploy_bearing(tool: &str) -> bool {
    tool.starts_with("deploy.")
}

/// The `(provider, target)` a `deploy.*` call targets, AS THE POLICY SEES IT.
/// Twin of `derive_service`/`derive_domain`: the ONE derivation both the real
/// decision (`handle_tools_call`) and the dry-run hint call, so they cannot
/// drift.
///
/// Returns `Some` for EVERY deploy-bearing tool — even one whose `provider`/
/// `target` argument is missing, in which case the field is the empty string.
/// That is deliberate: an empty target matches no `[rule.deploy]` allow-list
/// entry, so the verdict DENIES a deploy with no establishable target rather
/// than letting it slip past (fail-closed). `None` only for non-deploy tools.
fn derive_deploy(
    tool: &str,
    raw_provider: Option<&str>,
    raw_target: Option<&str>,
) -> Option<(String, String)> {
    if !deploy_bearing(tool) {
        return None;
    }
    Some((
        raw_provider.unwrap_or_default().to_string(),
        raw_target.unwrap_or_default().to_string(),
    ))
}

fn list_tools() -> Vec<Value> {
    tool_catalog()
        .iter()
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
    mode_path: &std::path::Path,
) -> Result<String, String> {
    match name {
        "os.status" => os_status(mode_path),
        "fs.read" => crate::tools::fs::fs_read(args, policy, caller),
        "fs.write" => crate::tools::fs::fs_write(args, policy, caller),
        "pkg.install" => Ok(json!({
            "status": "requires_approval",
            "detail": "pkg.install is a v0.1 stub: no package was installed. \
                       The rpm-ostree/bootc backend and the vibectl approval \
                       workflow land in a later milestone (see ROADMAP.md)."
        })
        .to_string()),
        "svc.restart" => crate::tools::svc::svc_restart(args),
        "svc.status" => crate::tools::svc::svc_status(args),
        "sandbox.probe" => crate::tools::sandbox_tool::sandbox_probe(args),
        "browser.run" => crate::tools::browser::run_governed(args),
        "deploy.plan" => crate::tools::deploy::deploy_plan(args),
        "log.read" => crate::tools::log::log_read(args),
        "sectools.list" => crate::tools::sectools::sectools_list(args),
        "fs.list" => crate::tools::fs::fs_list(args, policy, caller),
        "memory.query" => crate::tools::memory::memory_query(args),
        "memory.append" => crate::tools::memory::memory_append(args),
        "agent.thinking" => agent_thinking(args),
        "agent.sessions" => agent_sessions(),
        "agents.list" => agents_list(args, caller, audit_dir),
        "agent.activity" => agent_activity(args, caller, audit_dir),
        "agent.identity" => crate::tools::identity::agent_identity(),
        "user.model" => crate::tools::user_model::user_model(args, caller, audit_dir),
        "policy.check" => policy_check(args, policy),
        "policy.capabilities" => crate::tools::policy_tool::capabilities(policy),
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
    // A path the real pipeline REFUSES to normalize (relative, or climbing above
    // `/`) is rejected there before any policy evaluation. The hint must say so:
    // silently dropping it left the path constraints unevaluated and answered
    // "allow" for a call the daemon denies outright.
    if tool.starts_with("fs.") && target.is_some() && normalized.is_none() {
        return Ok(json!({
            "tool": tool, "target": target, "decision": "deny",
            "by": "invalid_path", "note": note
        })
        .to_string());
    }
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
    // Exactly what the real pipeline derives — same helper, so the hint cannot
    // drift laxer than the decision it predicts (it had: it compared a bare
    // "sshd" against a deny rule listing "sshd.service", answered
    // "require_approval", and the daemon then denied that very call).
    let service_owned = derive_service(tool, target);
    let domain_owned = derive_domain(tool, target);
    // The dry-run hint takes ONE `target`, not the (provider, target) a deploy
    // needs, so it does not model the `[rule.deploy]` verdict: `deploy: None`.
    // Harmless — deploy tools are T2, so the hint stays `require_approval`; the
    // real path derives the pair and enforces the verdict + approval floor. Per
    // this function's contract a hint may only ever be laxer, never let a T2
    // call through unapproved.
    let ctx = CallContext {
        path: normalized.as_deref(),
        service: service_owned.as_deref(),
        domain: domain_owned.as_deref(),
        deploy: None,
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

/// agent.sessions (T0): list the captured reasoning sessions so an observer (the
/// HUD) can both discover a session to pass to `agent.thinking` AND render a
/// history without one `agent.thinking` call per session. Read-only; no
/// arguments; output bounded by `REASONING_MAX_SESSIONS`, with `total` telling
/// the caller how many exist so a truncated view can say so instead of implying
/// it saw everything. Per-session cost is a stat plus a bounded first-line read
/// (see `reasoning::list_sessions`) — never a full-file scan.
fn agent_sessions() -> Result<String, String> {
    let (sessions, total) = crate::reasoning::list_sessions(std::path::Path::new(MEMORY_DIR));
    // Newest activity first, so the freshest session is the head of the list.
    let latest = sessions.first().map(|s| s.id.clone());
    let entries: Vec<Value> = sessions
        .iter()
        .map(|s| {
            json!({
                "id": s.id,
                // null when the start could not be read — an unknown start is
                // reported as unknown, never back-filled from the mtime.
                "started_unix": s.started_unix,
                "last_unix": s.last_unix,
                "bytes": s.bytes,
            })
        })
        .collect();
    serde_json::to_string(&json!({
        "sessions": entries,
        "count": entries.len(),
        "total": total,
        "truncated": total > entries.len(),
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

/// Longest roster window `agents.list` accepts (its `window_seconds` clamp
/// ceiling) — the tail cache below never needs records older than this.
pub(crate) const MAX_ROSTER_WINDOW_SECS: u64 = 3600;

/// Per-file incremental state for `read_recent_audit`. The audit files are
/// append-only (single writer behind the chain mutex), so bytes once parsed
/// never change: each probe reads and parses only the DELTA appended since the
/// previous one, instead of re-reading the whole 2 × 512 KiB window on every
/// HUD poll.
struct AuditTail {
    /// Byte offset up to which the file has been parsed — always the offset
    /// just after a `'\n'` (only complete lines are ever consumed).
    parsed_to: u64,
    /// Parsed records `(ts_secs, record)` in file order, purged below the
    /// maximum roster window and hard-capped.
    records: std::collections::VecDeque<(u64, Value)>,
}

/// Hard cap on cached records per file. A fresh 512 KiB window holds at most
/// ~3500 records (a record line is ≥ ~150 bytes), so this cap can never make
/// the cache return LESS than the plain windowed scan it replaces.
const TAIL_CACHE_MAX_RECORDS: usize = 8192;

/// Read a bounded tail of the audit trail as parsed records with `ts >= cutoff`.
/// Best-effort (skips unreadable/corrupt lines) and read-only — used ONLY to
/// derive the `agents.list` roster, never for chain verification. Bounds the
/// read to the last `MAX_TAIL_BYTES` of the two most recent daily files, and
/// keeps an incremental per-file cache (see [`AuditTail`]) so a steady-state
/// probe costs one `stat` plus the few lines appended since the previous one.
///
/// Invalidation is fail-safe in every uncertain case: a file whose size SHRANK
/// (the startup torn-tail rollback) is rescanned from scratch with the plain
/// windowed scan; entries for files that left the two-most-recent set are
/// dropped (daily rotation); a delta that does not yet end in `'\n'` leaves the
/// incomplete line unconsumed for the next probe (the writer emits the line and
/// its newline as separate writes, so a reader can observe the gap).
///
/// `pub(crate)` so the `user.model` derivation (ADR-028) can reuse the same
/// bounded, cached per-uid audit view instead of re-reading the trail.
pub(crate) fn read_recent_audit(dir: &std::path::Path, cutoff_secs: u64) -> Vec<Value> {
    const MAX_TAIL_BYTES: u64 = 512 * 1024;
    use std::collections::HashMap;
    use std::io::{Read, Seek, SeekFrom};
    use std::sync::{Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<HashMap<std::path::PathBuf, AuditTail>>> = OnceLock::new();

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

    let mut cache = CACHE
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    // Rotation: drop cached state for THIS dir's files that left the recent
    // set. Other directories' entries are none of this call's business (tests
    // and dev overrides run several audit dirs in one process).
    cache.retain(|p, _| p.parent() != Some(dir) || recent.contains(p));

    let purge_before = now_epoch_secs().saturating_sub(MAX_ROSTER_WINDOW_SECS);
    let mut out = Vec::new();
    for path in recent {
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        let len = meta.len();

        let needs_fresh_scan = match cache.get(&path) {
            Some(tail) => len < tail.parsed_to, // shrank: torn-tail rollback
            None => true,
        };
        if needs_fresh_scan {
            // Plain windowed scan (the pre-cache behavior), seeding the cache.
            let start = len.saturating_sub(MAX_TAIL_BYTES);
            let Ok(mut f) = std::fs::File::open(&path) else {
                cache.remove(&path);
                continue;
            };
            if start > 0 && f.seek(SeekFrom::Start(start)).is_err() {
                cache.remove(&path);
                continue;
            }
            let mut bytes = Vec::new();
            if f.read_to_end(&mut bytes).is_err() {
                cache.remove(&path);
                continue;
            }
            let mut records = std::collections::VecDeque::new();
            // Only complete lines are consumed; a torn trailing line stays
            // unparsed and unconsumed until its newline lands.
            let consumed = bytes.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
            let buf = String::from_utf8_lossy(&bytes[..consumed]);
            let mut lines = buf.lines();
            // A mid-file seek likely lands inside a line; drop that first partial.
            if start > 0 {
                lines.next();
            }
            for line in lines {
                push_audit_line(&mut records, line);
            }
            cache.insert(
                path.clone(),
                AuditTail {
                    parsed_to: start + consumed as u64,
                    records,
                },
            );
        } else if let Some(tail) = cache.get_mut(&path) {
            if len > tail.parsed_to {
                // Append-only delta: parsed_to sits just after a '\n', so the
                // delta starts on a line boundary.
                if let Ok(mut f) = std::fs::File::open(&path) {
                    if f.seek(SeekFrom::Start(tail.parsed_to)).is_ok() {
                        let mut bytes = Vec::new();
                        if f.read_to_end(&mut bytes).is_ok() {
                            let consumed =
                                bytes.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
                            let buf = String::from_utf8_lossy(&bytes[..consumed]);
                            for line in buf.lines() {
                                push_audit_line(&mut tail.records, line);
                            }
                            tail.parsed_to += consumed as u64;
                        }
                    }
                }
            }
        }

        if let Some(tail) = cache.get_mut(&path) {
            // Bound the cache: drop records past the maximum roster window,
            // then enforce the hard cap (oldest first).
            while tail
                .records
                .front()
                .is_some_and(|(ts, _)| *ts < purge_before)
            {
                tail.records.pop_front();
            }
            while tail.records.len() > TAIL_CACHE_MAX_RECORDS {
                tail.records.pop_front();
            }
            out.extend(
                tail.records
                    .iter()
                    .filter(|(ts, _)| *ts >= cutoff_secs)
                    .map(|(_, v)| v.clone()),
            );
        }
    }
    out
}

/// Parse one audit line into `(ts_secs, record)` and push it (best-effort:
/// blank or corrupt lines are skipped, exactly like the pre-cache scan did).
fn push_audit_line(records: &mut std::collections::VecDeque<(u64, Value)>, line: &str) {
    if line.trim().is_empty() {
        return;
    }
    if let Ok(v) = serde_json::from_str::<Value>(line) {
        let ts = v
            .get("ts_unix_ms")
            .and_then(Value::as_u64)
            .map(|ms| ms / 1000)
            .unwrap_or(0);
        records.push_back((ts, v));
    }
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
        // ONE CALL, ONE RECORD. The dispatcher writes `started*` before running
        // and `ok*`/`error` after, while a refusal writes a single line — so
        // counting every record made `calls` roughly DOUBLE for an agent whose
        // calls are allowed, and exact for one that keeps getting refused. The
        // number shown to the human was therefore not a call count at all, and
        // it flattered the busiest agents most. `agent.activity` deliberately
        // keeps BOTH records: it is a timeline, where "began" and "concluded"
        // are two real events and `outcome` is shown. Here we are counting, so
        // we count terminal records only.
        if !crate::audit::is_terminal_record(&r) {
            continue;
        }
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

/// agent.activity (T0): the caller's OWN recent governed tool calls, in
/// chronological order, derived from the audit trail — the "deeds" half of the
/// AI-citizen self-knowledge, complementing `agent.thinking` (thoughts) and
/// `policy.capabilities` (the static map of rights). Where `agents.list` gives a
/// per-pid ROSTER and deliberately EXCLUDES the caller's own process, this is the
/// mirror: the caller's OWN footprint, including its REFUSALS — so a citizen
/// learns the policy boundaries empirically (a `deny`/`require_approval` it hit)
/// on top of the manifest it can read statically.
///
/// Confinement is identical to `agents.list` (the same underlying data, self-
/// scoped): CONFINED to the requesting uid (SO_PEERCRED); an unidentified caller
/// sees nothing. It exposes no field an agent could not already obtain from
/// `agents.list` for its own uid — the audit trail itself stays root-only and on
/// the built-in denylist, unreadable via `fs.read` (no new leak; same reasoning
/// as ADR-023).
///
/// Anti-DoS: reached through `handle_tools_call` (per-uid rate limiter first,
/// call audited); the window is a bounded audit tail and the row count is capped.
fn agent_activity(
    args: &Value,
    caller: Caller,
    audit_dir: &std::path::Path,
) -> Result<String, String> {
    const DEFAULT_WINDOW_SECS: u64 = 3600;
    const MAX_ROWS: usize = 200;
    let window = args
        .get("window_seconds")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_WINDOW_SECS)
        .clamp(1, MAX_ROSTER_WINDOW_SECS);
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .map(|n| (n as usize).clamp(1, MAX_ROWS))
        .unwrap_or(MAX_ROWS);
    let now = now_epoch_secs();
    let cutoff = now.saturating_sub(window);

    // Fail-closed confinement: no SO_PEERCRED uid -> nothing (per-uid view only).
    let Some(uid) = caller.uid else {
        return Ok(json!({
            "activity": [],
            "count": 0,
            "window_seconds": window,
            "note": "no caller uid (SO_PEERCRED); activity is confined per-uid"
        })
        .to_string());
    };

    // The audit tail is chronological; keep only THIS uid's records, newest last.
    let mut rows: Vec<Value> = read_recent_audit(audit_dir, cutoff)
        .into_iter()
        .filter(|r| r.get("caller_uid").and_then(Value::as_u64) == Some(u64::from(uid)))
        .map(|r| {
            let ts = r
                .get("ts_unix_ms")
                .and_then(Value::as_u64)
                .map(|ms| ms / 1000)
                .unwrap_or(0);
            json!({
                "ts_unix": ts,
                "when": utc_iso8601(ts),
                "tool": r.get("tool").and_then(Value::as_str).unwrap_or(""),
                "target": r.get("target").cloned().unwrap_or(Value::Null),
                "decision": r.get("decision").cloned().unwrap_or(Value::Null),
                "outcome": r.get("outcome").cloned().unwrap_or(Value::Null),
                "pid": r.get("caller_pid").cloned().unwrap_or(Value::Null),
            })
        })
        .collect();

    // Newest first, then cap: an agent resuming a session wants its LAST actions.
    rows.reverse();
    let total = rows.len();
    rows.truncate(limit);

    Ok(json!({
        "activity": rows,
        "count": rows.len(),
        "total_in_window": total,
        "truncated": total > rows.len(),
        "window_seconds": window,
        "uid": uid,
    })
    .to_string())
}

fn os_status(mode_path: &std::path::Path) -> Result<String, String> {
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
    // Operating mode (ADR-027) — the DANGER PANEL feed. The HUD polls os.status,
    // so shipping the mode here means a live "AUTONOMOUS / OPEN MODE" banner with
    // zero new tool. Read-only, fail-safe (governed on any uncertainty).
    let mode = crate::mode::status(mode_path, now_epoch_secs());
    Ok(json!({
        "uptime_seconds": uptime_seconds,
        "loadavg_1_5_15": loadavg,
        "mem_total_kb": mem_total_kb,
        "mem_available_kb": mem_available_kb,
        "mounts": mounts,
        "mode": mode,
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
    use crate::test_support::policy_from_toml;

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
    fn builtin_denylist_covers_kernel_memory_and_runtime_dirs() {
        // Adversarial review 2026-07-14: vibed reads as ROOT, so /proc /run /etc
        // being "system read prefixes" exposed kernel memory (KASLR defeat) and
        // other users' 0700 runtime dirs. These must be read-denied.
        for path in [
            "/proc/kcore",
            "/proc/kallsyms",
            "/proc/kmsg",
            "/proc/1234/mem",
            "/proc/1234/task/5678/mem",
            "/proc/1234/pagemap",
            "/run/user/1000",
            "/run/user/1000/bus",
            "/run/secrets/db-password",
            "/etc/krb5.keytab",
            "/etc/sssd/sssd.conf",
            "/etc/ipsec.secrets",
            "/etc/ipsec.d/private/host.key",
            "/etc/pki/tls/private/server.key",
            // Adversarial sweep 2026-07-25: GLOBAL root-only (0400) /proc files
            // the blanket "/proc/" read prefix served to any caller — verified
            // leaking real bytes before this entry existed.
            "/proc/vmallocinfo",
            "/proc/slabinfo",
            "/proc/timer_list",
            "/proc/iomem",
            "/proc/ioports",
            "/proc/keys",
            "/proc/key-users",
            // Per-process ASLR / kernel-state files, denied for EVERY pid (the
            // per-pid owner check alone did not stop a setuid process).
            "/proc/1234/auxv",
            "/proc/1234/task/5678/auxv",
            "/proc/1234/stack",
            "/proc/1234/syscall",
        ] {
            assert!(
                builtin_denied(path, false).is_some(),
                "{path} must be read-denied by the built-in denylist"
            );
        }
        // ...without over-blocking legitimate system reads (public certs, the
        // agent's own os-release, a non-mem /proc file it may legitimately read).
        for path in [
            "/etc/os-release",
            "/etc/pki/tls/certs/ca-bundle.crt",
            "/usr/lib/os-release",
        ] {
            assert!(
                builtin_denied(path, false).is_none(),
                "{path} must stay readable (not a secret)"
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
            // The ADR-029 birth character: readable via memory.query, never
            // writable (an agent must not overwrite its own soul).
            "/var/lib/vibeos/memory/personality.toml",
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
        // The operating-mode record (ADR-027) is the no-self-escalation
        // invariant: an agent must never fs.write it (to flip open mode). It
        // stays READABLE — the mode is public anyway via os.status — so the
        // denial is write-only, and that asymmetry is the point of this check.
        assert!(
            builtin_denied("/var/lib/vibeos/mode.json", true).is_some(),
            "mode.json must be write-denied (no self-escalation)"
        );
        assert!(
            builtin_denied("/var/lib/vibeos/mode.json", false).is_none(),
            "mode.json stays readable (mode is public via os.status)"
        );
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
            "/home/dev/.config/git/credentials", // git-credential-store XDG fallback
            "/home/dev/.config/containers/auth.json", // Podman/skopeo/buildah authfile
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
        assert_eq!(tool_tier("agent.activity"), Some(Tier::T0));
        assert_eq!(tool_tier("agent.identity"), Some(Tier::T0));
        assert_eq!(tool_tier("user.model"), Some(Tier::T0));
        // `browser.run` : tier de PLANCHER T1 au catalogue ; le tier EFFECTIF est dynamique
        // (max des verbes du batch, via batch_tier) et calculé dans handle_tools_call.
        assert_eq!(tool_tier("browser.run"), Some(Tier::T1));
        assert_eq!(
            tool_tier("disk.wipe"),
            None,
            "unknown tool has no tier => default-deny"
        );
    }

    #[test]
    fn strictest_prefers_deny_then_approval_then_allow() {
        use Decision::*;
        // Deny domine tout.
        assert_eq!(strictest(Allow, Deny), Deny);
        assert_eq!(strictest(Deny, Allow), Deny);
        assert_eq!(strictest(RequireApproval, Deny), Deny);
        // RequireApproval domine Allow, pas Deny.
        assert_eq!(strictest(Allow, RequireApproval), RequireApproval);
        assert_eq!(strictest(RequireApproval, Allow), RequireApproval);
        // Allow ne domine rien.
        assert_eq!(strictest(Allow, Allow), Allow);
        // Le pli d'un batch : un seul host refusé refuse tout ; un seul en approbation escalade.
        let deny_run = [Allow, Deny, RequireApproval]
            .into_iter()
            .fold(Allow, strictest);
        assert_eq!(deny_run, Deny);
        let appr_run = [Allow, RequireApproval, Allow]
            .into_iter()
            .fold(Allow, strictest);
        assert_eq!(appr_run, RequireApproval);
    }

    #[test]
    fn tool_catalog_is_built_once_and_cached() {
        // Same allocation on every call: the registry (each tuple with a json!
        // schema tree) must never regress to being rebuilt per lookup —
        // tool_tier runs on every tools/call and once per audit record walked
        // by the agents.list roster loop.
        assert!(std::ptr::eq(tool_catalog(), tool_catalog()));
    }

    #[test]
    fn memory_scopes_match_the_query_catalog_enum() {
        // ADR-029 had to add "personality" in TWO hand-maintained places:
        // MEMORY_SCOPES (drives the tool logic) and the JSON-schema `scope`
        // enum advertised in the memory.query catalog entry. If they drift,
        // memory.query either advertises a scope it rejects or hides one it
        // accepts — a silent observability bug on a governed read surface.
        let entry = tool_catalog()
            .iter()
            .find(|(name, ..)| *name == "memory.query")
            .expect("memory.query is in the catalog");
        let enum_scopes: std::collections::BTreeSet<&str> = entry
            .3
            .pointer("/properties/scope/enum")
            .and_then(|v| v.as_array())
            .expect("memory.query schema has properties.scope.enum")
            .iter()
            .map(|v| v.as_str().expect("scope enum entries are strings"))
            .collect();
        let table_scopes: std::collections::BTreeSet<&str> =
            MEMORY_SCOPES.iter().map(|(name, ..)| *name).collect();
        assert_eq!(
            enum_scopes, table_scopes,
            "memory.query scope enum and MEMORY_SCOPES have drifted"
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
        assert!(out["total"].is_u64(), "total must be a number: {out}");
        assert!(
            out["truncated"].is_boolean(),
            "truncated must be a bool: {out}"
        );
        // 'latest' is null when empty, or the newest id otherwise — both valid.
        assert!(
            out.get("latest").is_some(),
            "latest key must be present: {out}"
        );
        // The listing never claims to have shown more than exists, and never
        // hides that it truncated.
        let (count, total) = (
            out["count"].as_u64().unwrap(),
            out["total"].as_u64().unwrap(),
        );
        assert!(count <= total, "count must never exceed total: {out}");
        assert_eq!(
            out["truncated"].as_bool().unwrap(),
            count < total,
            "truncated must agree with count vs total: {out}"
        );
        // Every entry carries the metadata the HUD history renders, so it never
        // needs one agent.thinking call per session just to date it.
        for entry in out["sessions"].as_array().unwrap() {
            assert!(entry["id"].is_string(), "id must be a string: {entry}");
            assert!(entry["last_unix"].is_u64(), "last_unix required: {entry}");
            assert!(entry["bytes"].is_u64(), "bytes required: {entry}");
            assert!(
                entry["started_unix"].is_u64() || entry["started_unix"].is_null(),
                "started_unix is a number or an honest null: {entry}"
            );
        }
        assert_eq!(tool_tier("agent.sessions"), Some(Tier::T0));
    }

    /// One hand-written audit line for the tail-cache tests (only the fields
    /// `read_recent_audit` actually reads).
    fn tail_rec(tool: &str, ts_ms: u64) -> String {
        json!({"ts_unix_ms": ts_ms, "tool": tool, "caller_uid": 1000, "caller_pid": 42}).to_string()
    }

    /// Fresh scratch audit dir for a tail-cache test. Each test uses its OWN
    /// dir: the incremental cache is keyed by file path, so distinct dirs mean
    /// independent cache state.
    fn tail_test_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("vibed-tail-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn recent_audit_tail_is_incremental_and_sees_appends() {
        let dir = tail_test_dir("incr");
        let file = dir.join("vibed-2099-01-01.jsonl");
        let now_ms = now_epoch_secs() * 1000;
        let cutoff = now_epoch_secs().saturating_sub(120);

        std::fs::write(
            &file,
            format!("{}\n{}\n", tail_rec("t1", now_ms), tail_rec("t2", now_ms)),
        )
        .unwrap();
        let first = read_recent_audit(&dir, cutoff);
        assert_eq!(first.len(), 2, "seed scan reads both records: {first:?}");

        // Append-only growth: the next probe must return the delta too.
        {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open(&file)
                .unwrap();
            writeln!(f, "{}", tail_rec("t3", now_ms)).unwrap();
        }
        let second = read_recent_audit(&dir, cutoff);
        assert_eq!(second.len(), 3, "delta record must appear: {second:?}");
        assert!(
            second.iter().any(|r| r["tool"] == "t3"),
            "the appended record is present: {second:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn recent_audit_tail_rescans_after_truncation() {
        let dir = tail_test_dir("trunc");
        let file = dir.join("vibed-2099-01-01.jsonl");
        let now_ms = now_epoch_secs() * 1000;
        let cutoff = now_epoch_secs().saturating_sub(120);

        std::fs::write(
            &file,
            format!("{}\n{}\n", tail_rec("t1", now_ms), tail_rec("t2", now_ms)),
        )
        .unwrap();
        assert_eq!(read_recent_audit(&dir, cutoff).len(), 2);

        // Startup torn-tail rollback shrinks the file: the cache must notice
        // (size < parsed_to) and rescan from scratch, not serve stale records.
        std::fs::write(&file, format!("{}\n", tail_rec("t1", now_ms))).unwrap();
        let after = read_recent_audit(&dir, cutoff);
        assert_eq!(after.len(), 1, "shrunk file must be rescanned: {after:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn recent_audit_tail_leaves_a_torn_line_until_its_newline_lands() {
        let dir = tail_test_dir("torn");
        let file = dir.join("vibed-2099-01-01.jsonl");
        let now_ms = now_epoch_secs() * 1000;
        let cutoff = now_epoch_secs().saturating_sub(120);

        // One complete record, then a record whose newline has not landed yet
        // (the writer emits the line and its '\n' as separate writes).
        std::fs::write(
            &file,
            format!("{}\n{}", tail_rec("t1", now_ms), tail_rec("t2", now_ms)),
        )
        .unwrap();
        let first = read_recent_audit(&dir, cutoff);
        assert_eq!(
            first.len(),
            1,
            "the un-terminated line is not consumed: {first:?}"
        );

        // The newline lands: the next probe parses the now-complete line.
        {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open(&file)
                .unwrap();
            f.write_all(b"\n").unwrap();
        }
        let second = read_recent_audit(&dir, cutoff);
        assert_eq!(second.len(), 2, "completed line is now parsed: {second:?}");
        assert!(second.iter().any(|r| r["tool"] == "t2"));
        let _ = std::fs::remove_dir_all(&dir);
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

    /// `calls` doit compter des APPELS, pas des lignes de journal.
    ///
    /// Le répartiteur écrit `started` avant d'exécuter puis `ok` après, alors
    /// qu'un refus n'écrit qu'une ligne. Compter toutes les lignes rendait donc
    /// un `calls` à peu près DOUBLE pour un agent dont les appels passent, et
    /// exact pour un agent qui se fait refuser — le nombre affiché à l'humain
    /// n'était pas un compte d'appels, et il flattait le plus les agents les
    /// plus actifs.
    #[test]
    fn agents_list_compte_des_appels_pas_des_lignes_de_journal() {
        let dir = std::env::temp_dir().join(format!("vibed-agents-calls-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let log = AuditLog::new(dir.clone());

        let me = Caller {
            uid: Some(1000),
            gid: Some(1000),
            pid: Some(111),
        };
        let peer = Caller {
            uid: Some(1000),
            gid: Some(1000),
            pid: Some(222),
        };

        // Deux appels AUTORISÉS, tels que le répartiteur les écrit réellement :
        // ouverture puis issue. Plus un refus, qui n'écrit qu'une ligne.
        for _ in 0..2 {
            log.record(
                "fs.read",
                &json!({}),
                Some("/home/a/x"),
                "allow",
                "started",
                peer,
            )
            .unwrap();
            log.record(
                "fs.read",
                &json!({}),
                Some("/home/a/x"),
                "allow",
                "ok",
                peer,
            )
            .unwrap();
        }
        log.record(
            "fs.write",
            &json!({}),
            Some("/etc/passwd"),
            "deny",
            "blocked",
            peer,
        )
        .unwrap();

        let out: Value = serde_json::from_str(&agents_list(&json!({}), me, &dir).unwrap()).unwrap();
        let agents = out["agents"].as_array().unwrap();
        assert_eq!(agents.len(), 1, "{out}");
        assert_eq!(
            agents[0]["calls"], 3,
            "5 lignes de journal, mais 3 appels : {out}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn agent_activity_is_own_uid_chronological_and_includes_refusals() {
        let dir = std::env::temp_dir().join(format!("vibed-agent-activity-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let log = AuditLog::new(dir.clone());

        let me = Caller {
            uid: Some(1000),
            gid: Some(1000),
            pid: Some(111),
        };
        let stranger = Caller {
            uid: Some(2000),
            gid: Some(2000),
            pid: Some(333),
        };

        // My own footprint: an allow, then a refusal, in this order.
        log.record("os.status", &json!({}), None, "allow", "ok", me)
            .unwrap();
        // A stranger's record between mine must never appear in my activity.
        log.record(
            "fs.read",
            &json!({}),
            Some("/home/b/y"),
            "allow",
            "ok",
            stranger,
        )
        .unwrap();
        log.record(
            "fs.read",
            &json!({}),
            Some("/etc/shadow"),
            "deny",
            "blocked_builtin_denylist",
            me,
        )
        .unwrap();

        let out: Value =
            serde_json::from_str(&agent_activity(&json!({}), me, &dir).unwrap()).unwrap();
        let act = out["activity"].as_array().unwrap();

        // Exactly my two records — the stranger's is confined out.
        assert_eq!(act.len(), 2, "only my own uid's records: {out}");
        // Newest first: the refusal is the most recent action.
        assert_eq!(act[0]["tool"], "fs.read");
        assert_eq!(
            act[0]["decision"], "deny",
            "refusals are surfaced, not hidden"
        );
        assert_eq!(act[0]["outcome"], "blocked_builtin_denylist");
        assert_eq!(act[0]["target"], "/etc/shadow");
        assert_eq!(act[1]["tool"], "os.status");
        assert!(act
            .iter()
            .all(|r| r["tool"] != "fs.read" || r["target"] != "/home/b/y"));

        // limit caps the rows but reports the true total in the window.
        let capped: Value =
            serde_json::from_str(&agent_activity(&json!({"limit": 1}), me, &dir).unwrap()).unwrap();
        assert_eq!(capped["count"], 1);
        assert_eq!(capped["total_in_window"], 2);
        assert_eq!(capped["truncated"], true);

        // No SO_PEERCRED uid -> nothing (fail-closed, per-uid view).
        let anon = Caller {
            uid: None,
            gid: None,
            pid: Some(9),
        };
        let out2: Value =
            serde_json::from_str(&agent_activity(&json!({}), anon, &dir).unwrap()).unwrap();
        assert_eq!(out2["count"], 0, "an unidentified caller sees nothing");

        assert_eq!(tool_tier("agent.activity"), Some(Tier::T0));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn policy_check_canonicalizes_the_unit_like_the_real_pipeline() {
        // A deny rule spells units out in full, the way an operator writes them.
        let policy = policy_from_toml(
            "[[rule]]\nid=\"svc\"\ntools=[\"svc.restart\"]\ntier=\"T2\"\naction=\"allow\"\napproval=\"human\"\n\
             [rule.services]\ndenied=[\"sshd.service\"]\n",
        );
        let check = |args: Value| -> Value {
            serde_json::from_str(&policy_check(&args, &policy).unwrap()).unwrap()
        };
        // The hint used to evaluate the bare name verbatim, miss "sshd.service",
        // and answer "require_approval" for a call the daemon denies outright.
        assert_eq!(
            check(json!({"tool": "svc.restart", "target": "sshd"}))["decision"],
            "deny",
            "a bare unit name must be canonicalized before the hint decides"
        );
        assert_eq!(
            check(json!({"tool": "svc.restart", "target": "sshd.service"}))["decision"],
            "deny"
        );
        // An off-list unit still reaches the human floor — the fix must not turn
        // the hint into a blanket deny.
        assert_eq!(
            check(json!({"tool": "svc.restart", "target": "nginx"}))["decision"],
            "require_approval"
        );
    }

    #[test]
    fn unit_bearing_is_the_single_source_of_truth_for_service_context() {
        // Only these tools name a unit; a [rule.services] constraint can govern
        // nothing else. Both the real pipeline and policy.check derive their
        // CallContext.service from this one predicate.
        assert!(unit_bearing("svc.restart"));
        assert!(unit_bearing("svc.status"));
        assert!(unit_bearing("log.read"));
        assert!(!unit_bearing("fs.read"));
        assert!(!unit_bearing("fs.write"));
        assert!(!unit_bearing("os.status"));
        assert!(!unit_bearing("pkg.install"));
        assert!(!unit_bearing("memory.append"));
    }

    #[test]
    fn derive_service_is_the_one_derivation_both_call_sites_use() {
        // Canonicalized before any decision: a bare name must match a deny rule
        // that spells the unit out, or the suffix trick walks past the deny-list.
        assert_eq!(
            derive_service("svc.restart", Some("sshd")).as_deref(),
            Some("sshd.service")
        );
        assert_eq!(
            derive_service("log.read", Some("vibed")).as_deref(),
            Some("vibed.service")
        );
        // A tool with no unit never carries one into the policy, whatever the
        // caller claims.
        assert_eq!(derive_service("fs.read", Some("sshd.service")), None);
        assert_eq!(derive_service("pkg.install", Some("vim.service")), None);
        assert_eq!(derive_service("os.status", Some("anything")), None);
        // No unit argument at all -> nothing to constrain on.
        assert_eq!(derive_service("svc.restart", None), None);
        // An unusable name is NOT dropped: dropping it would let a rule match on
        // "no unit at all". It falls through raw; execution re-validates.
        assert_eq!(
            derive_service("svc.restart", Some("../etc/passwd")).as_deref(),
            Some("../etc/passwd")
        );
    }

    #[test]
    fn derive_deploy_marks_every_deploy_call_and_no_other() {
        // A deploy tool ALWAYS carries a target — even with missing args, so the
        // [rule.deploy] verdict can refuse an empty (unestablished) target rather
        // than let it slip past the allow-list.
        assert_eq!(
            derive_deploy("deploy.apply", Some("fly"), Some("app-A")),
            Some(("fly".to_string(), "app-A".to_string()))
        );
        assert_eq!(
            derive_deploy("deploy.plan", None, None),
            Some((String::new(), String::new()))
        );
        // A smuggled provider/target on a NON-deploy tool is caller noise: it must
        // never reach a [rule.deploy] verdict (cannot borrow a deploy rule's tier).
        assert_eq!(
            derive_deploy("svc.restart", Some("fly"), Some("app-A")),
            None
        );
        assert_eq!(derive_deploy("fs.read", Some("fly"), Some("app-A")), None);
        assert_eq!(
            derive_deploy("browser.navigate", Some("fly"), Some("x")),
            None
        );
        // deploy_bearing agrees.
        assert!(deploy_bearing("deploy.apply") && deploy_bearing("deploy.plan"));
        assert!(!deploy_bearing("svc.restart") && !deploy_bearing("browser.navigate"));
    }

    #[test]
    fn audit_target_binds_a_deploy_to_its_derived_pair() {
        // The grant/audit subject is the DERIVED (provider, target), so a one-shot
        // approval for one listed pair can never be spent on another (Fable 5).
        let full = json!({"provider": "fly", "target": "app-A"});
        assert_eq!(
            audit_target("deploy.apply", None, None, &full),
            Some("fly:app-A".to_string())
        );
        // A missing target still names exactly what was asked (verdict denies it).
        let partial = json!({"provider": "fly"});
        assert_eq!(
            audit_target("deploy.plan", None, None, &partial),
            Some("fly:".to_string())
        );
        // A non-deploy tool never gets a deploy subject from smuggled args.
        let smuggle = json!({"provider": "fly", "target": "app-A"});
        assert_eq!(
            audit_target("svc.restart", None, Some("sshd.service"), &smuggle),
            Some("sshd.service".to_string())
        );
    }

    #[test]
    fn the_approval_grant_key_names_what_the_tool_actually_acts_on() {
        // `target` is the approval GRANT KEY (approval::check_and_consume_grant
        // compares it verbatim) AND the only thing the operator is shown. It must
        // name the tool's real subject: execute_tool re-reads the raw args, so a
        // target taken from an argument the tool ignores lets one approval
        // authorise a different action.

        // svc.restart acts on `unit`. A `path` smuggled alongside used to WIN and
        // become the target: the operator was asked to approve svc.restart on a
        // plausible-looking config file, and the resulting grant then authorised
        // restarting ANY other non-denied unit (the unit is never compared to the
        // grant).
        assert_eq!(
            audit_target(
                "svc.restart",
                Some("/etc/nginx/nginx.conf"),
                Some("nginx.service"),
                &json!({"unit": "nginx.service", "path": "/etc/nginx/nginx.conf"}),
            )
            .as_deref(),
            Some("nginx.service"),
            "a stray path must never become the approval subject of a unit restart"
        );

        // pkg.install acts on `name`. A stray `unit` used to win, so a grant
        // approved for "vim" matched a call installing something else.
        assert_eq!(
            audit_target(
                "pkg.install",
                None,
                None,
                &json!({"name": "evil-pkg", "unit": "vim"}),
            )
            .as_deref(),
            Some("evil-pkg"),
            "the package actually being installed is the approval subject"
        );

        // fs.* act on `path` — unchanged, and a stray unit cannot displace it.
        assert_eq!(
            audit_target(
                "fs.write",
                Some("/var/home/u/notes.txt"),
                None,
                &json!({"path": "/var/home/u/notes.txt", "unit": "sshd.service"}),
            )
            .as_deref(),
            Some("/var/home/u/notes.txt")
        );

        // A tool that acts on no nameable subject records none — a path or unit
        // smuggled into its args is fiction that nothing reads.
        assert_eq!(
            audit_target(
                "memory.append",
                Some("/etc/passwd"),
                None,
                &json!({"path": "/etc/passwd"}),
            ),
            None,
            "caller-supplied fiction must not enter the audit trail"
        );
        assert_eq!(audit_target("os.status", None, None, &json!({})), None);
    }

    #[test]
    fn policy_check_refuses_a_path_the_real_pipeline_would_refuse() {
        let policy = policy_from_toml(
            "[[rule]]\nid=\"fsw\"\ntools=[\"fs.write\"]\ntier=\"T1\"\naction=\"allow\"\n",
        );
        let check = |args: Value| -> Value {
            serde_json::from_str(&policy_check(&args, &policy).unwrap()).unwrap()
        };
        // The daemon rejects a non-absolute path before evaluating any rule. The
        // hint used to skip normalization silently and answer "allow".
        let rel = check(json!({"tool": "fs.write", "target": "etc/passwd"}));
        assert_eq!(rel["decision"], "deny", "relative path: {rel}");
        assert_eq!(rel["by"], "invalid_path");
        // An absolute path still evaluates normally.
        assert_eq!(
            check(json!({"tool": "fs.write", "target": "/var/home/u/a.txt"}))["decision"],
            "allow"
        );
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
