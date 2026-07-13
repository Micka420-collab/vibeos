//! vibed — VibeOS system AI daemon.
//!
//! Exposes OS control to AI agents through an MCP-style JSON-RPC 2.0 server
//! bound to a unix socket (`/run/vibed/mcp.sock`). Every tool call is gated
//! by the policy engine (`/etc/vibeos/policy.d/*.toml`, fail-closed on any
//! invalid file) and recorded in an append-only audit log
//! (`/var/lib/vibeos/audit/vibed.jsonl`) together with the caller identity
//! (SO_PEERCRED: uid/gid/pid).
//!
//! Privileges, honestly stated: in v0.1 vibed runs as root under
//! `vibed.service`. Socket access is restricted to root + the
//! `vibeos-agents` group (0660). Dropping to `User=vibed` with an empty
//! capability allow-list is the Phase 4 hardening target — see
//! `docs/SECURITY-ARCHITECTURE.md` and `ROADMAP.md`.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;

use tokio::net::UnixListener;
use tokio::signal::unix::{signal, SignalKind};
use tracing::{error, info, warn};

use vibed::audit::{self, Caller};
use vibed::mcp;
use vibed::policy;
use vibed::ratelimit;

/// MCP unix socket. Parent directory is normally provided by systemd
/// (`RuntimeDirectory=vibed`); we create it as a fallback for dev runs.
const SOCKET_PATH: &str = "/run/vibed/mcp.sock";
/// Drop-in policy directory, loaded in lexicographic order.
const POLICY_DIR: &str = "/etc/vibeos/policy.d";
/// Group allowed to talk to the socket (0660). Created by
/// `os/rootfs/usr/lib/sysusers.d/vibeos.conf` at image build/boot time.
const SOCKET_GROUP: &str = "vibeos-agents";

#[tokio::main]
async fn main() -> std::io::Result<()> {
    // Offline maintenance subcommand, handled before any daemon setup:
    //   vibed --verify-audit [dir]
    // Verifies the tamper-evident hash chain of the audit log (across all daily
    // files in the directory) and prints a JSON report, exiting 0 if intact and
    // 1 if broken. Usable by an operator (or `vibectl audit verify`) without
    // stopping the daemon.
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--verify-audit") {
        let dir = args
            .iter()
            .position(|a| a == "--verify-audit")
            .and_then(|i| args.get(i + 1))
            .map(String::as_str)
            .unwrap_or(audit::DEFAULT_AUDIT_DIR);
        let report = audit::verify_chain(Path::new(dir))?;
        println!(
            "{}",
            serde_json::json!({
                "dir": dir,
                "records": report.records,
                "ok": report.ok,
                "broken_at": report.broken_at,
                "reason": report.reason,
            })
        );
        std::process::exit(if report.ok { 0 } else { 1 });
    }

    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    info!("vibed {} starting", env!("CARGO_PKG_VERSION"));

    // Policy loading is FAIL-CLOSED: any unreadable or invalid *.toml in
    // policy.d aborts startup with a non-zero exit. A broken policy must
    // never degrade into a more permissive daemon.
    let policy = match policy::PolicyEngine::load_dir(Path::new(POLICY_DIR)) {
        Ok(engine) => Arc::new(engine),
        Err(e) => {
            error!("{e}");
            error!("refusing to serve with an invalid policy (fail-closed)");
            std::process::exit(1);
        }
    };
    let audit = Arc::new(audit::AuditLog::open_default());
    // Process-wide per-uid rate limiter, shared across all connections so an
    // agent cannot multiply its budget by opening many sockets.
    let limiter = Arc::new(ratelimit::RateLimiter::default());

    // Socket setup: create runtime dir if needed, remove a stale socket left
    // by a previous unclean shutdown, then bind and restrict permissions.
    let sock = Path::new(SOCKET_PATH);
    if let Some(parent) = sock.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if sock.exists() {
        warn!("removing stale socket {}", SOCKET_PATH);
        std::fs::remove_file(sock)?;
    }
    let listener = UnixListener::bind(sock)?;
    // root:vibeos-agents 0660 — group membership is the first authorization
    // gate; the policy engine is the second. Missing group (e.g. dev machine
    // without the sysusers.d file) is a warning, not a fatal error: the
    // socket then stays root-only, which is the more restrictive outcome.
    match find_group_gid(SOCKET_GROUP) {
        Some(gid) => {
            if let Err(e) = std::os::unix::fs::chown(sock, None, Some(gid)) {
                warn!("cannot chgrp socket to {SOCKET_GROUP} (gid {gid}): {e}");
            } else {
                info!("socket group set to {SOCKET_GROUP} (gid {gid})");
            }
        }
        None => warn!(
            "group '{SOCKET_GROUP}' not found in /etc/group; socket stays root-only \
             (expected on dev machines without usr/lib/sysusers.d/vibeos.conf)"
        ),
    }
    if let Err(e) = std::fs::set_permissions(sock, std::fs::Permissions::from_mode(0o660)) {
        warn!("cannot set socket permissions: {e}");
    }
    info!("MCP server listening on {}", SOCKET_PATH);

    let mut sigterm = signal(SignalKind::terminate())?;
    let mut sigint = signal(SignalKind::interrupt())?;

    loop {
        tokio::select! {
            _ = sigterm.recv() => {
                info!("SIGTERM received, shutting down");
                break;
            }
            _ = sigint.recv() => {
                info!("SIGINT received, shutting down");
                break;
            }
            accepted = listener.accept() => {
                match accepted {
                    Ok((stream, _addr)) => {
                        // Capture SO_PEERCRED at accept time; the identity is
                        // stamped on every audit record of this connection.
                        let caller = match stream.peer_cred() {
                            Ok(cred) => Caller {
                                uid: Some(cred.uid()),
                                gid: Some(cred.gid()),
                                pid: cred.pid(),
                            },
                            Err(e) => {
                                warn!("cannot read peer credentials: {e}");
                                Caller::default()
                            }
                        };
                        let policy = Arc::clone(&policy);
                        let audit = Arc::clone(&audit);
                        let limiter = Arc::clone(&limiter);
                        tokio::spawn(mcp::handle_connection(
                            stream,
                            policy,
                            audit,
                            limiter,
                            caller,
                            std::path::PathBuf::from(vibed::approval::APPROVAL_DIR),
                        ));
                    }
                    Err(e) => {
                        warn!("accept error: {e}");
                    }
                }
            }
        }
    }

    // Graceful shutdown: in-flight connections are dropped with the runtime;
    // v0.1 tools are short-lived reads/writes so this is acceptable.
    let _ = std::fs::remove_file(sock);
    info!("vibed stopped");
    Ok(())
}

/// Resolve a group name to its gid by parsing `/etc/group`
/// (format: `name:password:gid:members`). No libc/nss dependency.
fn find_group_gid(name: &str) -> Option<u32> {
    let content = std::fs::read_to_string("/etc/group").ok()?;
    for line in content.lines() {
        let mut fields = line.split(':');
        if fields.next() == Some(name) {
            let _password = fields.next()?;
            return fields.next()?.parse().ok();
        }
    }
    None
}
