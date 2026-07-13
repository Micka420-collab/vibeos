//! Append-only, hash-chained JSON-lines audit log.
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
//! TAMPER EVIDENCE (hash chain). Each record carries three chain fields:
//! `seq` (a monotonic counter starting at 0, detecting truncation/reorder),
//! `prev` (the SHA-256 of the PREVIOUS record's line — genesis IV = 64 × 0),
//! and `hash` (the SHA-256 of THIS record serialized without `hash`).
//! Altering, removing or reordering any record breaks the chain, which
//! `verify_chain` detects and localizes. The chain proves integrity of every
//! record except possibly the very last (an append-only log cannot self-seal
//! its tail); external anchoring of the head hash (TPM / Rekor transparency
//! log) is the remaining Phase 4 step (see `docs/SECURITY-ARCHITECTURE.md` §8).
//! The SHA-256 is vibed's own dependency-free implementation (`crate::sha256`).
//!
//! Each record is fsync'd before the call proceeds, so the "audit before
//! execution" invariant survives a power cut, not just a process crash.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::sha256::sha256_hex;

/// Default location under the vibed state directory.
pub const DEFAULT_AUDIT_PATH: &str = "/var/lib/vibeos/audit/vibed.jsonl";

/// Genesis "previous hash" for the first record: 64 hex zeros (no predecessor).
pub const CHAIN_IV: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// Identity of the connected agent, from `SO_PEERCRED` on the unix socket.
/// Fields are `None` when the credentials could not be read (logged, but the
/// call is still audited rather than dropped on the floor).
#[derive(Debug, Clone, Copy, Default)]
pub struct Caller {
    pub uid: Option<u32>,
    pub gid: Option<u32>,
    pub pid: Option<i32>,
}

/// Running chain state, guarded by the writer mutex.
struct ChainState {
    /// Sequence number to stamp on the NEXT record.
    next_seq: u64,
    /// SHA-256 of the last written record's line (the next record's `prev`).
    last_hash: String,
}

pub struct AuditLog {
    path: PathBuf,
    /// Serializes writers AND carries the chain state. O_APPEND keeps
    /// concurrent small writes atomic at the kernel level; the mutex keeps
    /// line boundaries and the hash chain deterministic within the process.
    state: Mutex<ChainState>,
}

impl AuditLog {
    /// Open (or prepare) the log at `path`, recovering the chain head from an
    /// existing file so a restart continues the same chain. A missing or empty
    /// file starts a fresh chain (seq 0, prev = IV).
    pub fn new(path: PathBuf) -> Self {
        let state = recover_chain_state(&path);
        Self {
            path,
            state: Mutex::new(state),
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

        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        // Build the record WITHOUT `hash`, using the current chain head as
        // `prev`. serde_json::Value serializes object keys in sorted order, so
        // the byte stream hashed here is reproduced exactly by `verify_chain`.
        let mut entry = json!({
            "seq": state.next_seq,
            "prev": state.last_hash,
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
        let hash = sha256_hex(entry.to_string().as_bytes());
        entry["hash"] = Value::String(hash.clone());
        let line = entry.to_string();

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
        // invariant must hold across a power cut too. sync_data costs ~ms per
        // call, negligible at the v0.x tool-call rate.
        file.sync_data()?;

        // Advance the chain ONLY after the record is durably on disk, so a
        // failed write never desyncs the in-memory head from the file.
        state.next_seq += 1;
        state.last_hash = hash;
        Ok(())
    }
}

/// Recover `(next_seq, last_hash)` from the last valid record in an existing
/// log. Tolerant of a trailing partial line (crash mid-write): scans for the
/// last line that parses with both `seq` and `hash`.
fn recover_chain_state(path: &Path) -> ChainState {
    let fresh = ChainState {
        next_seq: 0,
        last_hash: CHAIN_IV.to_string(),
    };
    let Ok(content) = fs::read_to_string(path) else {
        return fresh;
    };
    let mut recovered: Option<(u64, String)> = None;
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<Value>(line) {
            if let (Some(seq), Some(hash)) = (
                value.get("seq").and_then(Value::as_u64),
                value.get("hash").and_then(Value::as_str),
            ) {
                recovered = Some((seq, hash.to_string()));
            }
        }
    }
    match recovered {
        Some((seq, hash)) => ChainState {
            next_seq: seq + 1,
            last_hash: hash,
        },
        None => fresh,
    }
}

/// Outcome of verifying an audit log's hash chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainVerification {
    /// Number of records walked before the first break (or the total if OK).
    pub records: u64,
    /// True when the whole chain is intact.
    pub ok: bool,
    /// Sequence number where the chain first broke, if any.
    pub broken_at: Option<u64>,
    /// Human-readable reason for the break.
    pub reason: Option<String>,
}

/// Walk the log and verify the hash chain: every record's own `hash` matches
/// its content, `seq` increments by one from 0, and each `prev` equals the
/// previous record's `hash`. Returns the first break (fail-closed for
/// forensics: any anomaly is reported, never silently skipped).
pub fn verify_chain(path: &Path) -> io::Result<ChainVerification> {
    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Ok(ChainVerification {
                records: 0,
                ok: true,
                broken_at: None,
                reason: None,
            })
        }
        Err(e) => return Err(e),
    };

    let mut expected_seq: u64 = 0;
    let mut expected_prev = CHAIN_IV.to_string();

    let broken = |seq: u64, why: &str| ChainVerification {
        records: seq,
        ok: false,
        broken_at: Some(seq),
        reason: Some(why.to_string()),
    };

    for raw in content.lines() {
        let raw = raw.trim();
        if raw.is_empty() {
            continue;
        }
        let Ok(mut value) = serde_json::from_str::<Value>(raw) else {
            return Ok(broken(expected_seq, "record is not valid JSON"));
        };
        let Some(stored_hash) = value
            .get("hash")
            .and_then(Value::as_str)
            .map(str::to_string)
        else {
            return Ok(broken(expected_seq, "record has no 'hash' field"));
        };
        // Recompute the hash over the record MINUS its `hash` field.
        if let Some(obj) = value.as_object_mut() {
            obj.remove("hash");
        }
        if sha256_hex(value.to_string().as_bytes()) != stored_hash {
            return Ok(broken(
                expected_seq,
                "record hash mismatch (content altered)",
            ));
        }
        let seq = value.get("seq").and_then(Value::as_u64);
        if seq != Some(expected_seq) {
            return Ok(broken(
                expected_seq,
                "sequence number gap (record inserted, removed or reordered)",
            ));
        }
        let prev = value.get("prev").and_then(Value::as_str).unwrap_or("");
        if prev != expected_prev {
            return Ok(broken(
                expected_seq,
                "prev hash does not match the chain head",
            ));
        }
        expected_seq += 1;
        expected_prev = stored_hash;
    }

    Ok(ChainVerification {
        records: expected_seq,
        ok: true,
        broken_at: None,
        reason: None,
    })
}

/// FNV-1a 64-bit, hex-encoded. Not cryptographic: this is a correlation
/// digest of the (secret-bearing) arguments — its job is to let two records be
/// correlated without ever storing the arguments, NOT to secure the log. The
/// log's integrity comes from the SHA-256 hash chain above.
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
        // Chain fields on the genesis record.
        assert_eq!(first["seq"], 0);
        assert_eq!(first["prev"], CHAIN_IV);
        assert_eq!(first["hash"].as_str().map(str::len), Some(64));

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
        // The chain links: record 1's prev is record 0's hash.
        assert_eq!(second["seq"], 1);
        assert_eq!(second["prev"], first["hash"]);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn intact_chain_verifies() {
        let dir = temp_test_dir("verify-ok");
        let path = dir.join("vibed.jsonl");
        let log = AuditLog::new(path.clone());
        for i in 0..5 {
            log.record(
                "os.status",
                &json!({ "i": i }),
                None,
                "allow",
                "ok",
                Caller::default(),
            )
            .expect("record");
        }
        let v = verify_chain(&path).expect("verify");
        assert!(v.ok, "unbroken chain must verify: {v:?}");
        assert_eq!(v.records, 5);
        assert!(v.broken_at.is_none());

        // An absent log is trivially intact (0 records).
        let v = verify_chain(&dir.join("nope.jsonl")).expect("verify absent");
        assert!(v.ok && v.records == 0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn restart_continues_the_chain() {
        let dir = temp_test_dir("restart");
        let path = dir.join("vibed.jsonl");
        {
            let log = AuditLog::new(path.clone());
            log.record(
                "os.status",
                &json!({}),
                None,
                "allow",
                "ok",
                Caller::default(),
            )
            .expect("r0");
            log.record(
                "os.status",
                &json!({}),
                None,
                "allow",
                "ok",
                Caller::default(),
            )
            .expect("r1");
        }
        // A fresh AuditLog on the same path must recover seq=2, prev=last hash.
        let log2 = AuditLog::new(path.clone());
        log2.record(
            "os.status",
            &json!({}),
            None,
            "allow",
            "ok",
            Caller::default(),
        )
        .expect("r2");
        let v = verify_chain(&path).expect("verify");
        assert!(v.ok, "chain must stay intact across a restart: {v:?}");
        assert_eq!(v.records, 3);
        let last: Value =
            serde_json::from_str(fs::read_to_string(&path).unwrap().lines().last().unwrap())
                .unwrap();
        assert_eq!(last["seq"], 2);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn tampering_with_a_record_is_detected() {
        let dir = temp_test_dir("tamper-content");
        let path = dir.join("vibed.jsonl");
        let log = AuditLog::new(path.clone());
        for _ in 0..4 {
            log.record(
                "fs.read",
                &json!({"path": "/etc/os-release"}),
                Some("/etc/os-release"),
                "allow",
                "ok",
                Caller::default(),
            )
            .expect("record");
        }
        // Flip the `outcome` of record 1 from "ok" to "denied" — its stored
        // hash no longer matches its content.
        let content = fs::read_to_string(&path).unwrap();
        let mut lines: Vec<String> = content.lines().map(String::from).collect();
        lines[1] = lines[1].replace("\"ok\"", "\"denied\"");
        fs::write(&path, lines.join("\n") + "\n").unwrap();

        let v = verify_chain(&path).expect("verify");
        assert!(!v.ok, "content tampering must break verification");
        assert_eq!(v.broken_at, Some(1));
        assert!(v.reason.unwrap().contains("hash mismatch"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn deleting_a_record_is_detected() {
        let dir = temp_test_dir("tamper-delete");
        let path = dir.join("vibed.jsonl");
        let log = AuditLog::new(path.clone());
        for _ in 0..4 {
            log.record(
                "os.status",
                &json!({}),
                None,
                "allow",
                "ok",
                Caller::default(),
            )
            .expect("record");
        }
        // Remove record 2 entirely: record 3's `prev` no longer matches, and
        // its seq gaps.
        let content = fs::read_to_string(&path).unwrap();
        let mut lines: Vec<String> = content.lines().map(String::from).collect();
        lines.remove(2);
        fs::write(&path, lines.join("\n") + "\n").unwrap();

        let v = verify_chain(&path).expect("verify");
        assert!(!v.ok, "deleting a record must break the chain");
        assert_eq!(v.broken_at, Some(2));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn truncating_the_tail_leaves_a_shorter_valid_prefix_but_loses_records() {
        // Truncation of the TAIL cannot be detected by the chain alone (that is
        // what external anchoring solves) — but the surviving prefix stays
        // internally consistent, and the lost records are simply gone. This
        // test documents that boundary explicitly.
        let dir = temp_test_dir("truncate");
        let path = dir.join("vibed.jsonl");
        let log = AuditLog::new(path.clone());
        for _ in 0..5 {
            log.record(
                "os.status",
                &json!({}),
                None,
                "allow",
                "ok",
                Caller::default(),
            )
            .expect("record");
        }
        let content = fs::read_to_string(&path).unwrap();
        let lines: Vec<String> = content.lines().map(String::from).collect();
        fs::write(&path, lines[..3].join("\n") + "\n").unwrap();
        let v = verify_chain(&path).expect("verify");
        assert!(v.ok, "a truncated prefix stays internally consistent");
        assert_eq!(v.records, 3);
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
