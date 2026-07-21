//! Operating mode — the governed default vs the human-unlocked **open mode**
//! (ADR-027).
//!
//! VibeOS is governed by default: a T2/T3 tool call is a `RequireApproval` that
//! only proceeds behind a one-shot human grant (see `crate::approval`). Open
//! mode is the deliberate, bounded relaxation of that floor: while it is active,
//! a T2/T3 `RequireApproval` is auto-satisfied, so an agent acts autonomously —
//! install packages, restart services, write files, deploy — WITHOUT a
//! per-action approval. It is what turns "an assistant that must ask each time"
//! into "an autonomous operator", on purpose, for a bounded window.
//!
//! THE IRREDUCIBLE CORE — what open mode does NOT do, by construction, so that
//! "open" can never mean "one-way trip":
//!   1. **The unlock is a HUMAN, out-of-band action.** Only `vibectl mode open`
//!      (root-gated, physically present) writes the open record. An agent can
//!      REQUEST it but can never grant it to itself — the mode file is root-only
//!      and on `vibed`'s built-in WRITE denylist (`fs.write` can never forge it).
//!      This is the one invariant the whole OS rests on: no self-escalation.
//!   2. **The audit trail stays on.** Every call is still audited (open-mode
//!      calls carry the distinct `*_open_mode` outcome), and every mode
//!      transition is audited. Autonomy is not untraceability.
//!   3. **The kill-switch stays with the human.** `vibectl mode governed`
//!      reverts instantly, and open mode AUTO-EXPIRES after its bounded window
//!      (checked on every evaluation — no background task, no way to "stick").
//!   4. **Open mode only lifts the tier FLOOR.** An explicit `deny` rule, the
//!      absolute default-deny for unknown tools, and the built-in hard denylist
//!      (audit store, approval store, THIS mode file, policy dir, credentials)
//!      are all decided BEFORE the tier and are untouched. Open mode auto-grants
//!      the human-approval floor; it does not dissolve the guards that keep the
//!      trace, the kill-switch and secrets intact — otherwise the agent could
//!      erase its tracks or lock the operator out, which is exactly the
//!      irreversibility this design forbids.
//!
//! FAIL-SAFE. The powerful state requires a valid, unexpired `open` record.
//! Anything else — file absent, unreadable, malformed, a different mode string,
//! or past its expiry — reads as `Governed`. The safe state is the default in
//! every uncertain case.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

/// Production location of the mode record. Under the vibed state directory,
/// root-only, and on the built-in WRITE denylist so no agent can forge it.
pub const MODE_PATH: &str = "/var/lib/vibeos/mode.json";

/// Hard ceiling on an open-mode window. An operator cannot leave the machine
/// autonomous "forever" with a fat-fingered duration: a request beyond this is
/// clamped. Open mode is meant to be a bounded, attended window, re-armed
/// deliberately, not a permanent posture.
pub const OPEN_MAX_SECS: u64 = 24 * 3600;

/// Default open-mode window when the operator does not specify one.
pub const OPEN_DEFAULT_SECS: u64 = 30 * 60;

/// The effective operating mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The governed default: T2/T3 need a one-shot human grant.
    Governed,
    /// Human-unlocked autonomy: T2/T3 `RequireApproval` is auto-satisfied,
    /// still audited, still bounded by the expiry.
    Open,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Governed => "governed",
            Mode::Open => "open",
        }
    }
}

/// The effective mode at instant `now`, read fail-safe from `path`. Only a
/// valid, unexpired `"open"` record yields `Open`; everything else is
/// `Governed`.
pub fn effective(path: &Path, now: u64) -> Mode {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Mode::Governed; // absent/unreadable -> safe default
    };
    let Ok(v) = serde_json::from_str::<Value>(&text) else {
        return Mode::Governed; // malformed -> safe default
    };
    if v.get("mode").and_then(Value::as_str) != Some("open") {
        return Mode::Governed;
    }
    let expires = v.get("expires_unix").and_then(Value::as_u64).unwrap_or(0);
    if now >= expires {
        return Mode::Governed; // expired -> safe default (auto-revert)
    }
    Mode::Open
}

/// Convenience: is open mode active at `now`?
pub fn is_open(path: &Path, now: u64) -> bool {
    matches!(effective(path, now), Mode::Open)
}

/// Operator action (root-gated by the caller): unlock open mode for
/// `window_secs` (clamped to `[1, OPEN_MAX_SECS]`). Records who unlocked it and
/// an optional human-readable reason, for the audit trail and the danger panel.
/// Returns the written record.
pub fn set_open(
    path: &Path,
    window_secs: u64,
    operator_uid: Option<u32>,
    now: u64,
    reason: Option<&str>,
) -> std::io::Result<Value> {
    let window = window_secs.clamp(1, OPEN_MAX_SECS);
    let record = json!({
        "mode": "open",
        "since_unix": now,
        "expires_unix": now + window,
        "window_secs": window,
        "set_by_uid": operator_uid,
        "reason": reason.unwrap_or(""),
    });
    write_private(path, &record.to_string())?;
    Ok(record)
}

/// Operator kill-switch (root-gated by the caller): revert to the governed
/// default immediately. Idempotent — writing a governed record is enough, and
/// an absent file already reads as governed, so a best-effort remove is fine.
pub fn set_governed(path: &Path, operator_uid: Option<u32>, now: u64) -> std::io::Result<Value> {
    let record = json!({
        "mode": "governed",
        "since_unix": now,
        "set_by_uid": operator_uid,
    });
    write_private(path, &record.to_string())?;
    Ok(record)
}

/// Human-readable status of the mode at `now` (for `vibectl mode status`, the
/// selfcheck, and the HUD danger panel). Always safe to call.
pub fn status(path: &Path, now: u64) -> Value {
    let mode = effective(path, now);
    let mut out = json!({
        "mode": mode.as_str(),
        "danger": mode == Mode::Open,
    });
    if mode == Mode::Open {
        if let Ok(text) = std::fs::read_to_string(path) {
            if let Ok(v) = serde_json::from_str::<Value>(&text) {
                let expires = v.get("expires_unix").and_then(Value::as_u64).unwrap_or(0);
                out["expires_unix"] = json!(expires);
                out["remaining_secs"] = json!(expires.saturating_sub(now));
                out["set_by_uid"] = v.get("set_by_uid").cloned().unwrap_or(Value::Null);
                out["reason"] = v.get("reason").cloned().unwrap_or(Value::Null);
            }
        }
    }
    out
}

/// Write a file with owner-only permissions (0600). The mode store is root-only;
/// mirror the approval store's discipline (private mode, fsync for durability).
fn write_private(path: &Path, content: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(content.as_bytes())?;
    file.sync_data()
}

/// Default production path as a `PathBuf` (for wiring; tests inject a scratch
/// path directly).
pub fn default_path() -> PathBuf {
    PathBuf::from(MODE_PATH)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("vibed-mode-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("mode.json")
    }

    #[test]
    fn absent_or_broken_reads_governed() {
        let p = scratch("absent");
        assert_eq!(
            effective(&p, 1000),
            Mode::Governed,
            "absent file -> governed"
        );
        std::fs::write(&p, b"not json at all").unwrap();
        assert_eq!(effective(&p, 1000), Mode::Governed, "malformed -> governed");
        std::fs::write(&p, br#"{"mode":"banana","expires_unix":99999999999}"#).unwrap();
        assert_eq!(
            effective(&p, 1000),
            Mode::Governed,
            "unknown mode string -> governed"
        );
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn open_is_active_only_within_its_window() {
        let p = scratch("window");
        let now = 1_000_000;
        set_open(&p, 600, Some(0), now, Some("nightly self-improve")).expect("unlock");
        assert!(is_open(&p, now), "active right after unlock");
        assert!(is_open(&p, now + 599), "active within the window");
        assert!(
            !is_open(&p, now + 600),
            "auto-expires at the boundary (now >= expires)"
        );
        assert!(!is_open(&p, now + 10_000), "long past -> governed");
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn window_is_clamped_to_the_ceiling() {
        let p = scratch("clamp");
        let now = 2_000_000;
        let rec = set_open(&p, OPEN_MAX_SECS * 10, Some(0), now, None).expect("unlock");
        assert_eq!(
            rec["window_secs"].as_u64(),
            Some(OPEN_MAX_SECS),
            "an over-long window is clamped to the ceiling"
        );
        assert!(is_open(&p, now));
        assert!(
            !is_open(&p, now + OPEN_MAX_SECS),
            "even clamped, it expires"
        );
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn kill_switch_reverts_immediately() {
        let p = scratch("kill");
        let now = 3_000_000;
        set_open(&p, OPEN_MAX_SECS, Some(0), now, None).expect("unlock");
        assert!(is_open(&p, now));
        set_governed(&p, Some(0), now + 1).expect("kill-switch");
        assert!(
            !is_open(&p, now + 1),
            "governed takes effect at once, well before the window would have ended"
        );
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn status_reports_danger_and_details_when_open() {
        let p = scratch("status");
        let now = 4_000_000;
        let governed = status(&p, now);
        assert_eq!(governed["mode"], "governed");
        assert_eq!(governed["danger"], false);

        set_open(&p, 1200, Some(0), now, Some("audit sweep")).expect("unlock");
        let open = status(&p, now + 200);
        assert_eq!(open["mode"], "open");
        assert_eq!(open["danger"], true);
        assert_eq!(open["remaining_secs"], 1000);
        assert_eq!(open["set_by_uid"], 0);
        assert_eq!(open["reason"], "audit sweep");
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn mode_file_is_written_private() {
        use std::os::unix::fs::PermissionsExt;
        let p = scratch("perms");
        set_open(&p, 60, Some(0), 5_000_000, None).expect("unlock");
        let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "mode record is owner-only");
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }
}
