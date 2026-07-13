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
//! (deleted on consumption), and expires quickly. This is the plumbing; the
//! Plasma approval dialog / HUD live wiring is the presentation layer (Phase 4).

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

/// Approval store root (contains `pending/` and `granted/`).
pub const APPROVAL_DIR: &str = "/var/lib/vibeos/approvals";
/// A grant is valid for this long after the operator approves it.
pub const GRANT_TTL_SECS: u64 = 300;

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
pub fn request_approval(
    root: &Path,
    tool: &str,
    target: Option<&str>,
    tier: &str,
    caller_uid: Option<u32>,
    now: u64,
) -> std::io::Result<String> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let id = format!("{now}-{}-{n}", std::process::id());

    let dir = pending_dir(root);
    std::fs::create_dir_all(&dir)?;
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

/// Look for a FRESH, matching grant for `(tool, target, caller_uid)`. If one is
/// found it is CONSUMED (deleted) and `true` is returned. Expired grants are
/// pruned as they are encountered. One-shot by construction.
pub fn check_and_consume_grant(
    root: &Path,
    tool: &str,
    target: Option<&str>,
    caller_uid: Option<u32>,
    now: u64,
) -> bool {
    let dir = granted_dir(root);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return false;
    };
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
                return true;
            }
        }
    }
    false
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
        assert!(!check_and_consume_grant(
            &root,
            "svc.restart",
            Some("sshd.service"),
            Some(1000),
            now
        ));

        // Operator approves.
        approve(&root, &id, Some(0), now).expect("approve");
        assert_eq!(list_pending(&root).len(), 0, "request moves out of pending");

        // A DIFFERENT call is not covered by this grant.
        assert!(!check_and_consume_grant(
            &root,
            "svc.restart",
            Some("nginx.service"),
            Some(1000),
            now
        ));
        assert!(!check_and_consume_grant(
            &root,
            "pkg.install",
            Some("sshd.service"),
            Some(1000),
            now
        ));
        assert!(!check_and_consume_grant(
            &root,
            "svc.restart",
            Some("sshd.service"),
            Some(1001),
            now
        ));

        // The exact call is authorized — ONCE.
        assert!(check_and_consume_grant(
            &root,
            "svc.restart",
            Some("sshd.service"),
            Some(1000),
            now
        ));
        assert!(
            !check_and_consume_grant(&root, "svc.restart", Some("sshd.service"), Some(1000), now),
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
        assert!(!check_and_consume_grant(
            &root,
            "pkg.install",
            Some("htop"),
            Some(1000),
            later
        ));
        // Even at `now` it would be gone now (pruned by the expired check above).
        assert!(!check_and_consume_grant(
            &root,
            "pkg.install",
            Some("htop"),
            Some(1000),
            now
        ));
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
        assert!(!check_and_consume_grant(
            &root,
            "svc.restart",
            Some("sshd.service"),
            Some(1000),
            now
        ));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn approve_rejects_bad_ids_and_missing_requests() {
        let root = store("badid");
        assert!(approve(&root, "../etc/passwd", Some(0), 1).is_err());
        assert!(approve(&root, "no-such-id", Some(0), 1).is_err());
        assert!(deny(&root, "bad/id").is_err());
    }
}
