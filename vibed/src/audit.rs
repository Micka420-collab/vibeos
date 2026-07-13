//! Append-only JSON-lines audit log.
//!
//! Every tool call handled by vibed produces at least one record in
//! `/var/lib/vibeos/audit/vibed.jsonl`, whatever the policy decision was.
//! Records store a digest of the arguments (FNV-1a 64), not the arguments
//! themselves, so secrets passed to tools never land in the log — plus the
//! caller identity (uid/gid/pid) captured from the unix socket peer
//! credentials (`SO_PEERCRED`).
//!
//! Alongside the digest, each record carries a `target`: a human-readable,
//! NON-secret subject of the action (the path for fs.read/fs.write, the unit
//! for svc.restart, the package for pkg.install), so forensics can tell WHICH
//! object an action touched — the digest alone is not reversible. File content
//! and secret arguments are never written here.
//!
//! The file is opened in append mode for every record: simple, crash-safe
//! (no buffered state to lose), and adequate for the v0.1 call rate.
//!
//! v0.1 scope: plain JSONL with a NON-cryptographic correlation digest.
//! Hash-chained records, journald replication and TPM sealing are Phase 4
//! (see `docs/SECURITY-ARCHITECTURE.md` §8 and `ROADMAP.md`).

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

/// Default location under the vibed state directory.
pub const DEFAULT_AUDIT_PATH: &str = "/var/lib/vibeos/audit/vibed.jsonl";

/// Identity of the connected agent, from `SO_PEERCRED` on the unix socket.
/// Fields are `None` when the credentials could not be read (logged, but the
/// call is still audited rather than dropped on the floor).
#[derive(Debug, Clone, Copy, Default)]
pub struct Caller {
    pub uid: Option<u32>,
    pub gid: Option<u32>,
    pub pid: Option<i32>,
}

pub struct AuditLog {
    path: PathBuf,
    /// Serializes writers within this process. O_APPEND keeps concurrent
    /// small writes atomic at the kernel level, the mutex keeps line
    /// boundaries deterministic.
    lock: Mutex<()>,
}

impl AuditLog {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            lock: Mutex::new(()),
        }
    }

    pub fn open_default() -> Self {
        Self::new(PathBuf::from(DEFAULT_AUDIT_PATH))
    }

    /// Append one record. Callers on the `Allow` path treat an error here as
    /// fatal for the call (fail-closed): no audit, no execution.
    pub fn record(
        &self,
        tool: &str,
        args: &Value,
        target: Option<&str>,
        decision: &str,
        outcome: &str,
        caller: Caller,
    ) -> io::Result<()> {
        let ts_unix_ms: u64 = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let entry = json!({
            "ts_unix_ms": ts_unix_ms,
            "tool": tool,
            "target": target,
            "args_fnv1a64": fnv1a_64_hex(args.to_string().as_bytes()),
            "decision": decision,
            "outcome": outcome,
            "caller_uid": caller.uid,
            "caller_gid": caller.gid,
            "caller_pid": caller.pid,
        });
        let line = entry.to_string();

        let _guard = self
            .lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        file.write_all(line.as_bytes())?;
        file.write_all(b"\n")?;
        // Durability, not just crash-safety: the "audit before execution"
        // invariant must hold across a power cut too — without this fsync, a
        // 'started' record could vanish while its tool call DID run. sync_data
        // costs ~ms per call, negligible at the v0.x tool-call rate.
        file.sync_data()?;
        Ok(())
    }
}

/// FNV-1a 64-bit, hex-encoded. Not cryptographic: this is a correlation
/// digest for audit purposes, not an integrity guarantee (hash-chained,
/// sealed audit records are Phase 4, see ROADMAP.md).
fn fnv1a_64_hex(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unique per-process directory under the system temp dir (`/tmp` on
    /// Linux), removed at the end of the test.
    fn temp_test_dir(tag: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!("vibed-audit-test-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn record_appends_parseable_json_lines_with_caller_identity() {
        let dir = temp_test_dir("append");
        let path = dir.join("vibed.jsonl");
        let log = AuditLog::new(path.clone());

        let caller = Caller {
            uid: Some(1000),
            gid: Some(1001),
            pid: Some(4242),
        };
        log.record("os.status", &json!({}), None, "allow", "ok", caller)
            .expect("first record");
        log.record(
            "pkg.install",
            &json!({"name": "htop"}),
            Some("htop"),
            "require_approval",
            "pending_approval",
            Caller::default(),
        )
        .expect("second record");

        let content = fs::read_to_string(&path).expect("audit file readable");
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 2, "one JSON line per record");

        let first: Value = serde_json::from_str(lines[0]).expect("line 1 is JSON");
        assert_eq!(first["tool"], "os.status");
        assert_eq!(first["decision"], "allow");
        assert_eq!(first["outcome"], "ok");
        assert_eq!(first["caller_uid"], 1000);
        assert_eq!(first["caller_gid"], 1001);
        assert_eq!(first["caller_pid"], 4242);
        assert!(first["ts_unix_ms"].as_u64().unwrap_or(0) > 0);
        assert_eq!(first["args_fnv1a64"].as_str().map(str::len), Some(16));
        assert!(first["target"].is_null(), "os.status has no target subject");

        let second: Value = serde_json::from_str(lines[1]).expect("line 2 is JSON");
        assert_eq!(second["tool"], "pkg.install");
        assert_eq!(second["outcome"], "pending_approval");
        assert_eq!(
            second["target"], "htop",
            "the package name is the audit target"
        );
        assert!(
            second["caller_uid"].is_null(),
            "unknown caller is recorded as null"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn digest_is_stable_and_input_sensitive() {
        // FNV-1a offset basis for empty input.
        assert_eq!(
            fnv1a_64_hex(b""),
            format!("{:016x}", 0xcbf2_9ce4_8422_2325_u64)
        );
        assert_eq!(fnv1a_64_hex(b"abc"), fnv1a_64_hex(b"abc"));
        assert_ne!(fnv1a_64_hex(b"abc"), fnv1a_64_hex(b"abd"));
    }
}
