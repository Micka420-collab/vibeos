//! Human-in-the-loop approval for T2/T3 tool calls — the minimal grant model.
//!
//! Today a `RequireApproval` decision is a polite refusal. This module turns it
//! into a real out-of-band flow WITHOUT ever letting an agent approve its own
//! request:
//!
//!   1. An agent calls a T2/T3 tool. `vibed` records a PENDING REQUEST in the
//!      approval store and answers "pending, id=X — run `vibectl approve X`".
//!   2. The human operator runs `vibectl approve X` (root/wheel), which turns
//!      the request into a one-shot, short-lived GRANT bound to the exact
//!      (tool, target, caller uid).
//!   3. The agent re-issues the SAME call. `vibed` finds the fresh grant,
//!      CONSUMES it (one-shot), and the call proceeds as if allowed — audited
//!      as `approved`.
//!
//! Security: the store lives under `/var/lib/vibeos/approvals`, root-only and
//! on the built-in denylist (agents can neither read nor forge grants). A grant
//! matches only the precise (tool, target, uid) of the request, is single-use
//! (deleted on consumption), and expires quickly. The pending side is **bounded**
//! (dedup of identical requests, age-based pruning, hard cap) so a compromised
//! agent cannot fill the memory volume by spamming un-approvable T2/T3 calls.
//! This is the plumbing; the Plasma approval dialog / HUD live wiring is the
//! presentation layer (Phase 4).

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::{json, Value};

/// Serializes the read-check-write of `request_approval` so the `MAX_PENDING`
/// cap and the dedup are strict even when several connections request approval
/// concurrently (handle_tools_call is async and runs across tasks). The
/// critical section is short (a directory scan + one small write) and approval
/// requests are rare and rate-limited upstream, so contention is negligible.
static REQUEST_LOCK: Mutex<()> = Mutex::new(());

/// Approval store root (contains `pending/` and `granted/`).
pub const APPROVAL_DIR: &str = "/var/lib/vibeos/approvals";
/// A grant is valid for this long after the operator approves it.
pub const GRANT_TTL_SECS: u64 = 300;
/// A pending request the operator never acts on is stale after this long and is
/// pruned. Bounds the store against an agent that spams T2/T3 calls.
pub const PENDING_TTL_SECS: u64 = 3600;
/// Hard cap on simultaneously-pending requests. Fail-closed: once reached, new
/// requests are refused (bounded disk) until the operator triages or they age
/// out. A flood is already audited per call, so it cannot pass unnoticed.
pub const MAX_PENDING: usize = 64;
/// Per-uid cap on simultaneously-pending requests. A single agent (one uid) must
/// not be able to fill the whole queue and STARVE other users' approval
/// requests — so each uid's footprint is bounded well below `MAX_PENDING`, which
/// stays as the global backstop. Fail-closed like the global cap.
pub const MAX_PENDING_PER_UID: usize = 16;

fn pending_dir(root: &Path) -> PathBuf {
    root.join("pending")
}
fn granted_dir(root: &Path) -> PathBuf {
    root.join("granted")
}

/// Sanitize a request id to a safe filename component (defense in depth: ids we
/// mint are already safe, but a `vibectl approve <id>` argument is user input).
fn safe_id(id: &str) -> Option<String> {
    if !id.is_empty()
        && id.len() <= 128
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
    {
        Some(id.to_string())
    } else {
        None
    }
}

/// Record a pending approval request; returns its id. `now` is epoch seconds.
///
/// The pending store is bounded so an agent cannot fill the disk by spamming
/// T2/T3 calls that never get approved:
///  - expired requests (older than `PENDING_TTL_SECS`) are pruned first;
///  - an identical still-fresh request `(tool, target, caller_uid)` is
///    deduplicated — its existing id is returned, no new file is written (this
///    is also the common case of an agent re-issuing the same call while it
///    waits for the operator);
///  - once `MAX_PENDING` distinct requests are outstanding, new ones are
///    refused (fail-closed) rather than growing the store without limit.
pub fn request_approval(
    root: &Path,
    tool: &str,
    target: Option<&str>,
    tier: &str,
    caller_uid: Option<u32>,
    now: u64,
) -> std::io::Result<String> {
    // Hold the lock across the whole read-check-write so the cap and dedup are
    // race-free: two concurrent callers can no longer both pass the cap gate,
    // nor both mint a file for the same (tool, target, uid).
    let _guard = REQUEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    let dir = pending_dir(root);
    std::fs::create_dir_all(&dir)?;

    // Prune stale requests, then scan what remains for a duplicate and a count
    // (both global and for the requesting uid).
    let mut live = 0usize;
    let mut same_uid = 0usize;
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(req) = serde_json::from_str::<Value>(&text) else {
                continue;
            };
            let ts = req.get("ts_unix").and_then(Value::as_u64).unwrap_or(0);
            if now.saturating_sub(ts) >= PENDING_TTL_SECS {
                let _ = std::fs::remove_file(&path); // prune stale pending
                continue;
            }
            live += 1;
            // Deduplicate an identical, still-fresh request.
            let r_tool = req.get("tool").and_then(Value::as_str);
            let r_target = req.get("target").and_then(Value::as_str);
            let r_uid = req
                .get("caller_uid")
                .and_then(Value::as_u64)
                .map(|u| u as u32);
            if r_uid == caller_uid {
                same_uid += 1;
            }
            if r_tool == Some(tool) && r_target == target && r_uid == caller_uid {
                if let Some(existing) = req.get("id").and_then(Value::as_str) {
                    return Ok(existing.to_string());
                }
            }
        }
    }
    // Per-uid cap first: one agent must not starve other users' approvals.
    if same_uid >= MAX_PENDING_PER_UID {
        return Err(std::io::Error::other(
            "approval store full for this uid: too many pending requests",
        ));
    }
    if live >= MAX_PENDING {
        return Err(std::io::Error::other(
            "approval store full: too many pending requests",
        ));
    }

    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let id = format!("{now}-{}-{n}", std::process::id());

    let record = json!({
        "id": id,
        "ts_unix": now,
        "tool": tool,
        "target": target,
        "tier": tier,
        "caller_uid": caller_uid,
    });
    write_private(&dir.join(format!("{id}.json")), &record.to_string())?;
    Ok(id)
}

/// Details of a grant that was found and consumed. Carries the operator
/// identity so the caller can record WHO approved the action in the audit log
/// — the grant file is deleted on consumption, so this is the only place that
/// identity survives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrantConsumption {
    /// Operator uid that approved the request (`None` if it was unreadable at
    /// approve time).
    pub approver_uid: Option<u32>,
    /// The grant/request id, for cross-referencing the audit trail.
    pub id: Option<String>,
}

/// Look for a FRESH, matching grant for `(tool, target, caller_uid)`. If one is
/// found it is CONSUMED (deleted) and `Some(GrantConsumption)` is returned with
/// the approver's identity; `None` otherwise. Expired grants are pruned as they
/// are encountered. One-shot by construction.
pub fn check_and_consume_grant(
    root: &Path,
    tool: &str,
    target: Option<&str>,
    caller_uid: Option<u32>,
    now: u64,
) -> Option<GrantConsumption> {
    let dir = granted_dir(root);
    let entries = std::fs::read_dir(&dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(grant) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        let expires = grant
            .get("expires_unix")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        if now >= expires {
            let _ = std::fs::remove_file(&path); // prune stale grant
            continue;
        }
        let g_tool = grant.get("tool").and_then(Value::as_str);
        let g_target = grant.get("target").and_then(Value::as_str);
        let g_uid = grant
            .get("caller_uid")
            .and_then(Value::as_u64)
            .map(|u| u as u32);
        if g_tool == Some(tool) && g_target == target && g_uid == caller_uid {
            // Consume before returning so a grant can never be replayed, even
            // if two calls race (the loser simply sees the file gone).
            if std::fs::remove_file(&path).is_ok() {
                return Some(GrantConsumption {
                    approver_uid: grant
                        .get("granted_by_uid")
                        .and_then(Value::as_u64)
                        .map(|u| u as u32),
                    id: grant.get("id").and_then(Value::as_str).map(str::to_string),
                });
            }
        }
    }
    None
}

/// Operator action: turn pending request `id` into a one-shot grant. Returns
/// the grant record. `operator_uid` is stamped for the audit trail.
pub fn approve(
    root: &Path,
    id: &str,
    operator_uid: Option<u32>,
    now: u64,
) -> std::io::Result<Value> {
    let id = safe_id(id).ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid approval id")
    })?;
    let pending = pending_dir(root).join(format!("{id}.json"));
    let text = std::fs::read_to_string(&pending)?;
    let req: Value = serde_json::from_str(&text)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    let granted = granted_dir(root);
    std::fs::create_dir_all(&granted)?;
    let grant = json!({
        "id": id,
        "tool": req.get("tool"),
        "target": req.get("target"),
        "tier": req.get("tier"),
        "caller_uid": req.get("caller_uid"),
        "granted_by_uid": operator_uid,
        "granted_ts_unix": now,
        "expires_unix": now + GRANT_TTL_SECS,
    });
    write_private(&granted.join(format!("{id}.json")), &grant.to_string())?;
    let _ = std::fs::remove_file(&pending);
    Ok(grant)
}

/// Operator action: reject and remove pending request `id`.
pub fn deny(root: &Path, id: &str) -> std::io::Result<()> {
    let id = safe_id(id).ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid approval id")
    })?;
    std::fs::remove_file(pending_dir(root).join(format!("{id}.json")))
}

/// List pending requests (most recent first) as JSON objects.
pub fn list_pending(root: &Path) -> Vec<Value> {
    let Ok(entries) = std::fs::read_dir(pending_dir(root)) else {
        return Vec::new();
    };
    let mut reqs: Vec<Value> = entries
        .flatten()
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("json"))
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .filter_map(|t| serde_json::from_str::<Value>(&t).ok())
        .collect();
    reqs.sort_by(|a, b| {
        b.get("ts_unix")
            .and_then(Value::as_u64)
            .cmp(&a.get("ts_unix").and_then(Value::as_u64))
    });
    reqs
}

/// Write a file with owner-only permissions (the store is root-only).
fn write_private(path: &Path, content: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(content.as_bytes())?;
    file.sync_data()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("vibed-appr-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn request_then_approve_then_consume_once() {
        let root = store("flow");
        let now = 1_000_000;
        let id = request_approval(
            &root,
            "svc.restart",
            Some("sshd.service"),
            "T2",
            Some(1000),
            now,
        )
        .expect("request");
        assert_eq!(list_pending(&root).len(), 1);

        // No grant yet: a matching call is not authorized.
        assert!(check_and_consume_grant(
            &root,
            "svc.restart",
            Some("sshd.service"),
            Some(1000),
            now
        )
        .is_none());

        // Operator (uid 0) approves.
        approve(&root, &id, Some(0), now).expect("approve");
        assert_eq!(list_pending(&root).len(), 0, "request moves out of pending");

        // A DIFFERENT call is not covered by this grant.
        assert!(check_and_consume_grant(
            &root,
            "svc.restart",
            Some("nginx.service"),
            Some(1000),
            now
        )
        .is_none());
        assert!(check_and_consume_grant(
            &root,
            "pkg.install",
            Some("sshd.service"),
            Some(1000),
            now
        )
        .is_none());
        assert!(check_and_consume_grant(
            &root,
            "svc.restart",
            Some("sshd.service"),
            Some(1001),
            now
        )
        .is_none());

        // The exact call is authorized — ONCE — and carries the approver identity.
        let consumed =
            check_and_consume_grant(&root, "svc.restart", Some("sshd.service"), Some(1000), now)
                .expect("exact call authorized");
        assert_eq!(
            consumed.approver_uid,
            Some(0),
            "audit can attribute the approval to operator uid 0"
        );
        assert_eq!(consumed.id.as_deref(), Some(id.as_str()));
        assert!(
            check_and_consume_grant(&root, "svc.restart", Some("sshd.service"), Some(1000), now)
                .is_none(),
            "a grant is single-use (consumed)"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn expired_grant_is_not_honored() {
        let root = store("expiry");
        let now = 2_000_000;
        let id = request_approval(&root, "pkg.install", Some("htop"), "T2", Some(1000), now)
            .expect("request");
        approve(&root, &id, Some(0), now).expect("approve");
        // Well after expiry: refused and pruned.
        let later = now + GRANT_TTL_SECS + 1;
        assert!(
            check_and_consume_grant(&root, "pkg.install", Some("htop"), Some(1000), later)
                .is_none()
        );
        // Even at `now` it would be gone now (pruned by the expired check above).
        assert!(
            check_and_consume_grant(&root, "pkg.install", Some("htop"), Some(1000), now).is_none()
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn deny_removes_the_request_and_grants_nothing() {
        let root = store("deny");
        let now = 3_000_000;
        let id = request_approval(
            &root,
            "svc.restart",
            Some("sshd.service"),
            "T2",
            Some(1000),
            now,
        )
        .expect("request");
        deny(&root, &id).expect("deny");
        assert_eq!(list_pending(&root).len(), 0);
        assert!(check_and_consume_grant(
            &root,
            "svc.restart",
            Some("sshd.service"),
            Some(1000),
            now
        )
        .is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn approve_rejects_bad_ids_and_missing_requests() {
        let root = store("badid");
        assert!(approve(&root, "../etc/passwd", Some(0), 1).is_err());
        assert!(approve(&root, "no-such-id", Some(0), 1).is_err());
        assert!(deny(&root, "bad/id").is_err());
    }

    #[test]
    fn identical_requests_are_deduplicated() {
        let root = store("dedup");
        let now = 5_000_000;
        let id1 = request_approval(
            &root,
            "svc.restart",
            Some("sshd.service"),
            "T2",
            Some(1000),
            now,
        )
        .expect("first");
        // Same (tool, target, uid) while still fresh: no new file, same id.
        let id2 = request_approval(
            &root,
            "svc.restart",
            Some("sshd.service"),
            "T2",
            Some(1000),
            now + 5,
        )
        .expect("second");
        assert_eq!(id1, id2, "identical fresh request reuses the same id");
        assert_eq!(list_pending(&root).len(), 1, "no duplicate pending file");

        // A different uid is a distinct request (grants are uid-scoped).
        request_approval(
            &root,
            "svc.restart",
            Some("sshd.service"),
            "T2",
            Some(1001),
            now + 6,
        )
        .expect("other uid");
        assert_eq!(list_pending(&root).len(), 2);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn stale_pending_is_pruned_when_a_new_request_arrives() {
        let root = store("prune");
        let now = 6_000_000;
        request_approval(&root, "pkg.install", Some("htop"), "T2", Some(1000), now).expect("old");
        assert_eq!(list_pending(&root).len(), 1);
        // A new, unrelated request well after the stale window prunes the old one.
        let later = now + PENDING_TTL_SECS + 1;
        request_approval(&root, "pkg.install", Some("btop"), "T2", Some(1000), later).expect("new");
        let pending = list_pending(&root);
        assert_eq!(
            pending.len(),
            1,
            "stale request pruned, only the fresh one remains"
        );
        assert_eq!(
            pending[0].get("target").and_then(Value::as_str),
            Some("btop")
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn pending_store_is_capped() {
        let root = store("cap");
        let now = 7_000_000;
        // Fill the store with MAX_PENDING distinct requests, SPREAD across uids so
        // the per-uid cap never binds first — this asserts the GLOBAL cap. (Each
        // uid gets exactly MAX_PENDING_PER_UID; MAX_PENDING is a multiple of it.)
        for i in 0..MAX_PENDING {
            let svc = format!("svc{i}.service");
            let uid = 1000 + (i as u32 / MAX_PENDING_PER_UID as u32);
            request_approval(&root, "svc.restart", Some(&svc), "T2", Some(uid), now)
                .unwrap_or_else(|_| panic!("request {i} within cap"));
        }
        assert_eq!(list_pending(&root).len(), MAX_PENDING);
        // One more distinct request from a FRESH uid is refused by the GLOBAL cap
        // (that uid has 0 pending, so only the global limit can stop it).
        assert!(
            request_approval(
                &root,
                "svc.restart",
                Some("overflow.service"),
                "T2",
                Some(9999),
                now
            )
            .is_err(),
            "request beyond MAX_PENDING is refused (fail-closed)"
        );
        assert_eq!(list_pending(&root).len(), MAX_PENDING, "store did not grow");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn per_uid_cap_prevents_one_uid_from_starving_others() {
        let root = store("cap-uid");
        let now = 9_000_000;
        // One uid fills its per-uid quota with distinct targets.
        for i in 0..MAX_PENDING_PER_UID {
            let svc = format!("svc{i}.service");
            request_approval(&root, "svc.restart", Some(&svc), "T2", Some(1000), now)
                .unwrap_or_else(|_| panic!("request {i} within per-uid cap"));
        }
        // The SAME uid's next distinct request is refused by the per-uid cap,
        // even though the GLOBAL store is far from full.
        assert!(
            request_approval(
                &root,
                "svc.restart",
                Some("more.service"),
                "T2",
                Some(1000),
                now
            )
            .is_err(),
            "uid 1000 beyond its per-uid cap is refused"
        );
        // ...but a DIFFERENT uid is not starved — it can still queue an approval.
        assert!(
            request_approval(
                &root,
                "svc.restart",
                Some("other.service"),
                "T2",
                Some(2000),
                now
            )
            .is_ok(),
            "a different uid must not be starved by uid 1000's flood"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn cap_is_strict_under_concurrent_requests() {
        // Without the serializing lock this would race past MAX_PENDING. Many
        // threads request DISTINCT targets at the same instant; exactly
        // MAX_PENDING must succeed, the rest must be refused, and the store must
        // hold exactly MAX_PENDING files — never more.
        let root = store("cap-race");
        std::fs::create_dir_all(&root).unwrap();
        let now = 8_000_000;
        let attempts = MAX_PENDING * 2;
        let ok = std::sync::atomic::AtomicUsize::new(0);
        std::thread::scope(|s| {
            for i in 0..attempts {
                let root = &root;
                let ok = &ok;
                s.spawn(move || {
                    let svc = format!("svc{i}.service");
                    // Spread uids so no single uid attempts more than its per-uid
                    // cap — the GLOBAL cap is what must bind here.
                    let uid = 1000 + (i as u32 / MAX_PENDING_PER_UID as u32);
                    if request_approval(root, "svc.restart", Some(&svc), "T2", Some(uid), now)
                        .is_ok()
                    {
                        ok.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                });
            }
        });
        assert_eq!(
            ok.load(std::sync::atomic::Ordering::Relaxed),
            MAX_PENDING,
            "exactly MAX_PENDING requests succeed under concurrency"
        );
        assert_eq!(
            list_pending(&root).len(),
            MAX_PENDING,
            "store never grows past the hard cap"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
