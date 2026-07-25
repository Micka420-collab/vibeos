//! Append-only, hash-chained JSON-lines audit log, rotated per UTC day.
//!
//! Every tool call handled by vibed produces at least one record in the audit
//! directory `/var/lib/vibeos/audit/`, whatever the policy decision was.
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
//! ROTATION. Records are written to one file per UTC day,
//! `vibed-YYYY-MM-DD.jsonl`, so no single file grows without bound (a full
//! disk would otherwise eventually block every tool call via the fail-closed
//! path). Old days can be archived/pruned by policy without touching today's.
//!
//! TAMPER EVIDENCE (hash chain). Each record carries three chain fields:
//! `seq` (a monotonic counter starting at 0, detecting truncation/reorder),
//! `prev` (the SHA-256 of the PREVIOUS record's line — genesis IV = 64 × 0),
//! and `hash` (the SHA-256 of THIS record serialized without `hash`). The
//! chain is CONTINUOUS across daily files: `seq`/`prev` never reset at a day
//! boundary, so altering, removing or reordering any record *in the interior*
//! of the chain breaks it, which `verify_chain` detects and localizes across
//! the whole directory (e.g. dropping a whole MIDDLE day leaves a `seq`/`prev`
//! discontinuity).
//!
//! SCOPE, honestly stated. The chain is KEYLESS and has no external anchor, so
//! it detects tampering by a party that CANNOT recompute it — not by one that
//! can. It gives two guarantees against such a party, and no more: (1) integrity
//! of every record except the tail — an append-only log cannot self-seal its
//! last record(s), so **tail truncation** (dropping the most-recent records, or
//! the most-recent daily file) is NOT detectable here; (2) it does NOT resist an
//! attacker who can *write* the audit directory and recompute every subsequent
//! `seq`/`prev`/`hash`. Both gaps close the same way: external anchoring of the
//! head hash (TPM / Rekor transparency log), the remaining **Phase 4** step (see
//! `docs/SECURITY-ARCHITECTURE.md` §8). Until then, the operative protection is
//! access control — the store is root-only and on vibed's built-in hard denylist
//! (`/var/lib/vibeos/audit/**`), so a confined agent cannot write it at all.
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

/// Default audit DIRECTORY under the vibed state directory (one JSONL file per
/// UTC day is created inside it).
pub const DEFAULT_AUDIT_DIR: &str = "/var/lib/vibeos/audit";

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
    /// UTC date ("YYYY-MM-DD") of the daily file the last record was written to.
    /// The next write never targets a file sorting BEFORE this — see the
    /// monotonic-date clamp in `record_with_digest_at`. Empty for a fresh chain.
    last_file_date: String,
}

pub struct AuditLog {
    dir: PathBuf,
    /// Serializes writers AND carries the chain state. O_APPEND keeps
    /// concurrent small writes atomic at the kernel level; the mutex keeps
    /// line boundaries and the hash chain deterministic within the process.
    state: Mutex<ChainState>,
}

impl AuditLog {
    /// Open (or prepare) the audit directory `dir`, recovering the chain head
    /// from the existing daily files so a restart continues the same chain. An
    /// empty/absent directory starts a fresh chain (seq 0, prev = IV).
    pub fn new(dir: PathBuf) -> Self {
        // Roll back a torn trailing record BEFORE recovering the chain head, so
        // the next append never merges un-terminated bytes with a fresh record.
        truncate_trailing_partial(&dir);
        let state = recover_chain_state(&dir);
        Self {
            dir,
            state: Mutex::new(state),
        }
    }

    pub fn open_default() -> Self {
        Self::new(PathBuf::from(DEFAULT_AUDIT_DIR))
    }

    /// Directory the audit trail is written to. Used by the `agents.list` tool
    /// to derive a live roster from the recent tail (honoring any dev override).
    pub fn dir(&self) -> &Path {
        &self.dir
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
        self.record_with_digest(
            tool,
            &fnv1a_64_hex(args.to_string().as_bytes()),
            target,
            decision,
            outcome,
            caller,
        )
    }

    /// Same as [`AuditLog::record`], with the arguments digest already
    /// computed. The two records of one tool call (`started` + final outcome)
    /// always carried the same digest — same arguments — yet each `record`
    /// re-serialized the full argument tree (up to ~1 MiB for `fs.write`) to
    /// recompute it. The dispatch path computes the digest once per call and
    /// shares it between both records through this entry point.
    pub fn record_with_digest(
        &self,
        tool: &str,
        args_digest: &str,
        target: Option<&str>,
        decision: &str,
        outcome: &str,
        caller: Caller,
    ) -> io::Result<()> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        self.record_with_digest_at(
            now.as_millis() as u64,
            now.as_secs(),
            tool,
            args_digest,
            target,
            decision,
            outcome,
            caller,
        )
    }

    /// Body of [`AuditLog::record_with_digest`] with the clock injected, so tests
    /// can exercise a moving (including BACKWARD) wall clock deterministically.
    #[allow(clippy::too_many_arguments)]
    fn record_with_digest_at(
        &self,
        ts_unix_ms: u64,
        epoch_secs: u64,
        tool: &str,
        args_digest: &str,
        target: Option<&str>,
        decision: &str,
        outcome: &str,
        caller: Caller,
    ) -> io::Result<()> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        // Build the record WITHOUT `hash`, using the current chain head as
        // `prev`. The integrity guarantee does NOT depend on WHICH key order
        // serde_json uses: `record` (here) and `verify_chain` serialize the same
        // Value through the identical code path, so the exact byte stream hashed
        // here is reproduced byte-for-byte at verification. (With the default
        // BTreeMap backing that order is sorted; it stays consistent either way.)
        let mut entry = json!({
            "seq": state.next_seq,
            "prev": state.last_hash,
            "ts_unix_ms": ts_unix_ms,
            "tool": tool,
            "target": target,
            "args_fnv1a64": args_digest,
            "decision": decision,
            "outcome": outcome,
            "caller_uid": caller.uid,
            "caller_gid": caller.gid,
            "caller_pid": caller.pid,
        });
        let hash = sha256_hex(entry.to_string().as_bytes());
        entry["hash"] = Value::String(hash.clone());
        let line = entry.to_string();

        // Monotonic day-file selection. The daily file is normally chosen from
        // the wall clock, but `daily_files`, `verify_chain` and
        // `recover_chain_state` all assume filename order (UTC date) == chain
        // (seq) order. A BACKWARD wall-clock step across a UTC-day boundary — an
        // NTP correction, a VM-snapshot restore, or an unset RTC reading ~1970 at
        // boot — would otherwise drop this higher-seq record into an
        // EARLIER-sorting file, which (1) makes verify_chain read records out of
        // seq order and falsely report a break on an untampered log, and (2) makes
        // the next restart's recover_chain_state pick the wrong head and stamp a
        // DUPLICATE seq, permanently corrupting the chain — from benign clock skew,
        // no attacker. Clamping the file date forward keeps on-disk file order
        // monotone with seq, so that invariant holds and verify/recover stay
        // correct unchanged. (The record's `ts_unix_ms` still reflects the raw
        // clock: seq is the ordering authority, the timestamp is informational.)
        let file_date = monotonic_date(&state.last_file_date, &utc_date(epoch_secs));

        fs::create_dir_all(&self.dir)?;
        let target_file = self.dir.join(format!("vibed-{file_date}.jsonl"));
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&target_file)?;
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
        state.last_file_date = file_date;
        Ok(())
    }
}

/// The daily-file date to write to: never earlier than the last one written,
/// so a backward wall clock cannot drop a higher-seq record into an
/// earlier-sorting file (ISO dates sort lexicographically = chronologically).
fn monotonic_date(last_file_date: &str, now_date: &str) -> String {
    if now_date > last_file_date {
        now_date.to_string()
    } else {
        last_file_date.to_string()
    }
}

/// Extract "YYYY-MM-DD" from a daily audit file path (`vibed-YYYY-MM-DD.jsonl`).
fn file_date(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    name.strip_prefix("vibed-")
        .and_then(|s| s.strip_suffix(".jsonl"))
        .map(str::to_string)
}

/// The daily audit files under `dir`, sorted chronologically (the `vibed-` +
/// ISO-date name sorts lexicographically = by date). Empty if `dir` is absent.
fn daily_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(read_dir) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = read_dir
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("vibed-") && n.ends_with(".jsonl"))
        })
        .collect();
    files.sort();
    files
}

/// UN APPEL, DEUX ENREGISTREMENTS — le prédicat qui remet les compteurs d'aplomb.
///
/// Le répartiteur écrit `started*` AVANT d'exécuter puis `ok*`/`error`/`panic`
/// après : c'est l'invariant « auditer avant d'agir », et il ne vaut que pour le
/// chemin `Allow`. Un refus (`blocked*`), une approbation en attente
/// (`pending_approval`) ou un débit dépassé (`rate_limited`) n'écrivent qu'UNE
/// ligne.
///
/// Conséquence pour QUICONQUE agrège ce journal : compter les lignes fait peser
/// un appel autorisé DEUX fois et un appel refusé une seule. Tout ratio
/// autorisé/refusé est alors faux d'un facteur deux, dans le sens le plus
/// flatteur — la friction disparaît, le volume d'activité double. Ce prédicat
/// vit ici, avec le format d'enregistrement qu'il décrit, pour qu'il n'y ait
/// qu'UN endroit à corriger si le vocabulaire des issues change.
///
/// Conséquence assumée : un appel dont le `started` existe mais dont l'issue n'a
/// jamais été écrite (arrêt brutal entre les deux) n'est compté par personne. On
/// préfère ne compter que ce qu'on a vu conclure — l'autre sens inventerait une
/// issue qui n'a pas été enregistrée.
pub fn is_terminal_record(record: &Value) -> bool {
    !record
        .get("outcome")
        .and_then(Value::as_str)
        .is_some_and(|o| o.starts_with("started"))
}

/// Résultat d'un balayage en lecture du journal : ce qui a été lu, et ce qui
/// n'a PAS pu l'être. Les lignes illisibles sont comptées, jamais avalées — un
/// lecteur qui n'annoncerait que ce qu'il a compris donnerait une image
/// faussement complète du journal.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RecordScan {
    /// Enregistrements JSON valides passés à la fermeture.
    pub records: u64,
    /// Lignes non vides qui n'ont pas pu être analysées.
    pub skipped: u64,
}

/// Parcourt les enregistrements du journal dans l'ordre chronologique et passe
/// chacun à `f`, EN FLUX.
///
/// Pourquoi un flux et pas un `Vec` : le journal grandit sans borne (une ligne
/// par appel d'outil, pour toujours). Le charger en mémoire pour l'agréger
/// ferait grossir le coût du lecteur avec l'âge de la machine — un DoS lent,
/// offert par la simple durée de vie. Ici la mémoire est bornée par la ligne la
/// plus longue.
///
/// LECTURE SEULE, et volontairement distinct de `verify_chain` : celui-ci
/// VÉRIFIE l'intégrité et s'arrête à la première rupture ; celui-là ne fait que
/// lire pour agréger, et ne prétend RIEN sur l'intégrité. Un appelant qui a
/// besoin des deux doit appeler les deux — confondre « j'ai lu le journal » et
/// « le journal est intègre » est exactement le raccourci qu'on refuse.
pub fn for_each_record(dir: &Path, mut f: impl FnMut(&Value)) -> io::Result<RecordScan> {
    use std::io::BufRead;
    let mut scan = RecordScan::default();
    for file in daily_files(dir) {
        let handle = match fs::File::open(&file) {
            Ok(h) => h,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        };
        for line in io::BufReader::new(handle).lines() {
            let line = line?;
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            match serde_json::from_str::<Value>(line) {
                Ok(v) => {
                    scan.records += 1;
                    f(&v);
                }
                Err(_) => scan.skipped += 1,
            }
        }
    }
    Ok(scan)
}

/// UTC calendar date "YYYY-MM-DD" from unix epoch seconds — Howard Hinnant's
/// civil_from_days algorithm, std-only (the crate deliberately has no date
/// dependency).
fn utc_date(epoch_secs: u64) -> String {
    let days = (epoch_secs / 86_400) as i64;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    format!("{year:04}-{month:02}-{day:02}")
}

/// Roll back a torn trailing record in the most-recent daily file: if the file
/// does not end on a record boundary (final byte != '\n'), truncate it to just
/// after its last newline, dropping the un-terminated tail.
///
/// Why: `record()` writes the JSON line, then '\n', then fsyncs and only THEN
/// returns Ok (and the Allow path executes only after that Ok). A power loss
/// mid-`record()` can leave a record's bytes on disk without their terminating
/// newline. Left as-is, the next `O_APPEND` write lands immediately after those
/// bytes, merging `{torn}{next}\n` into ONE physical line that `verify_chain`
/// can never parse — a permanent, false "broken chain". A record whose newline
/// never reached disk was never committed (the daemon crashed before `record()`
/// returned, so its action never ran), so rolling it back is correct, not a
/// loss of durable audit. Runs once at daemon start.
fn truncate_trailing_partial(dir: &Path) {
    let Some(file) = daily_files(dir).into_iter().next_back() else {
        return; // no files yet
    };
    let Ok(bytes) = fs::read(&file) else {
        return;
    };
    if bytes.is_empty() || bytes.last() == Some(&b'\n') {
        return; // already on a clean record boundary
    }
    let keep = bytes.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
    if let Ok(f) = fs::OpenOptions::new().write(true).open(&file) {
        let _ = f.set_len(keep as u64);
    }
}

/// Recover `(next_seq, last_hash)` from the last valid record across ALL daily
/// files (chronological). Tolerant of a trailing partial line (crash
/// mid-write): scans for the last line that parses with both `seq` and `hash`.
/// (Belt-and-suspenders: `truncate_trailing_partial` already removed a torn
/// tail at open, but recovery stays tolerant of one anyway.)
fn recover_chain_state(dir: &Path) -> ChainState {
    let files = daily_files(dir);
    // The newest daily file by name is, under the monotonic-date invariant the
    // writer maintains, where the chain head lives — seed `last_file_date` from
    // it so the first write after a restart also refuses an earlier-dated file.
    let last_file_date = files.last().and_then(|p| file_date(p)).unwrap_or_default();
    let mut recovered: Option<(u64, String)> = None;
    for file in &files {
        let Ok(content) = fs::read_to_string(file) else {
            continue;
        };
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
    }
    match recovered {
        Some((seq, hash)) => ChainState {
            next_seq: seq + 1,
            last_hash: hash,
            last_file_date,
        },
        None => ChainState {
            next_seq: 0,
            last_hash: CHAIN_IV.to_string(),
            last_file_date,
        },
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

/// Walk EVERY daily file in `dir` (chronological order) and verify the single
/// continuous hash chain: every record's own `hash` matches its content, `seq`
/// increments by one from 0, and each `prev` equals the previous record's
/// `hash` — across day boundaries too. Returns the first break (fail-closed for
/// forensics: any anomaly is reported, never silently skipped).
pub fn verify_chain(dir: &Path) -> io::Result<ChainVerification> {
    let mut expected_seq: u64 = 0;
    let mut expected_prev = CHAIN_IV.to_string();

    let broken = |seq: u64, why: &str| ChainVerification {
        records: seq,
        ok: false,
        broken_at: Some(seq),
        reason: Some(why.to_string()),
    };

    for file in daily_files(dir) {
        let content = match fs::read_to_string(&file) {
            Ok(c) => c,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
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
/// `pub(crate)` so the dispatch path (`mcp::try_audit`) can compute it once
/// per call and feed [`AuditLog::record_with_digest`].
pub(crate) fn fnv1a_64_hex(bytes: &[u8]) -> String {
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

    /// Unique per-process directory under the system temp dir, removed at the
    /// end of the test.
    fn temp_test_dir(tag: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!("vibed-audit-test-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    /// The single daily file in an audit dir (tests write within one UTC day).
    fn sole_daily_file(dir: &Path) -> PathBuf {
        let files = daily_files(dir);
        assert_eq!(files.len(), 1, "expected exactly one daily file: {files:?}");
        files.into_iter().next().unwrap()
    }

    #[test]
    fn torn_trailing_write_is_rolled_back_and_chain_still_verifies() {
        let dir = temp_test_dir("torn");
        let caller = Caller {
            uid: Some(1000),
            gid: Some(1000),
            pid: Some(1),
        };

        // One cleanly-committed record (seq 0, terminated by '\n').
        let log = AuditLog::new(dir.clone());
        log.record("os.status", &json!({}), None, "allow", "ok", caller)
            .expect("first record");

        // Simulate a power loss mid-`record()`: a full, VALID record whose bytes
        // reached disk but whose terminating newline did not. Left as-is,
        // `recover_chain_state` would even parse it (seq 1) and the next append
        // would merge `{torn}{next}\n` into one unparseable physical line.
        let day = sole_daily_file(&dir);
        {
            use std::io::Write;
            let mut f = fs::OpenOptions::new().append(true).open(&day).unwrap();
            f.write_all(b"{\"seq\":1,\"tool\":\"torn\",\"hash\":\"deadbeef\"}")
                .unwrap();
        }

        // Daemon restart: the torn tail is rolled back, and a fresh record lands
        // on a clean boundary.
        let log2 = AuditLog::new(dir.clone());
        log2.record(
            "fs.read",
            &json!({}),
            Some("/etc/os-release"),
            "allow",
            "ok",
            caller,
        )
        .expect("second record after recovery");

        // No false break: the chain verifies, and the torn record was rolled
        // back (seq 1 re-used by the real second record), not merged/counted.
        let report = verify_chain(&dir).expect("verify runs");
        assert!(
            report.ok,
            "a torn trailing write must not break the chain: {report:?}"
        );
        assert_eq!(
            report.records, 2,
            "torn record rolled back, exactly two committed records: {report:?}"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn monotonic_date_never_goes_backward() {
        assert_eq!(
            monotonic_date("", "2026-07-23"),
            "2026-07-23",
            "empty -> now"
        );
        assert_eq!(
            monotonic_date("2026-07-23", "2026-07-24"),
            "2026-07-24",
            "a later date advances"
        );
        assert_eq!(
            monotonic_date("2026-07-23", "2026-07-23"),
            "2026-07-23",
            "the same date stays"
        );
        assert_eq!(
            monotonic_date("2026-07-23", "2026-07-22"),
            "2026-07-23",
            "an earlier date is clamped forward (backward clock)"
        );
    }

    #[test]
    fn backward_clock_across_day_boundary_keeps_the_chain_ordered() {
        let dir = temp_test_dir("clockback");
        let caller = Caller {
            uid: Some(0),
            gid: Some(0),
            pid: Some(1),
        };
        let log = AuditLog::new(dir.clone());

        // Two epochs on DIFFERENT UTC days, written in BACKWARD order — a wall
        // clock that steps back across midnight between two audit writes (NTP
        // correction / VM snapshot / unset RTC).
        let later = 1_784_000_000u64; // day B
        let earlier = later - 2 * 86_400; // day A, two days earlier
        assert_ne!(
            utc_date(later),
            utc_date(earlier),
            "the two epochs must straddle a UTC-day boundary"
        );

        log.record_with_digest_at(
            later * 1000,
            later,
            "os.status",
            "0",
            None,
            "allow",
            "ok",
            caller,
        )
        .expect("seq 0 on day B");
        // Clock jumps BACK to day A for the next record.
        log.record_with_digest_at(
            earlier * 1000,
            earlier,
            "os.status",
            "0",
            None,
            "allow",
            "ok",
            caller,
        )
        .expect("seq 1 with a backward clock");

        // The backward record is clamped into day B's file, so on-disk file order
        // stays monotone with seq: a SINGLE daily file, not an earlier-dated one.
        let files = daily_files(&dir);
        assert_eq!(
            files.len(),
            1,
            "a backward-clock write must not open an earlier-dated file: {files:?}"
        );

        // verify_chain must NOT falsely flag this untampered log as broken.
        let report = verify_chain(&dir).expect("verify runs");
        assert!(report.ok, "clock skew must not break the chain: {report:?}");
        assert_eq!(report.records, 2);

        // A restart recovers seq 2 as the next — no duplicate-seq corruption.
        let log2 = AuditLog::new(dir.clone());
        log2.record("fs.read", &json!({}), None, "allow", "ok", caller)
            .expect("seq 2 after restart");
        let report2 = verify_chain(&dir).expect("verify runs");
        assert!(
            report2.ok && report2.records == 3,
            "restart continues the chain cleanly after a clock skew: {report2:?}"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn utc_date_matches_known_epochs() {
        assert_eq!(utc_date(0), "1970-01-01");
        // 2026-07-08T00:00:00Z = 1783468800 (same anchor as mcp.rs tests).
        assert_eq!(utc_date(1_783_468_800), "2026-07-08");
        assert_eq!(utc_date(1_783_468_800 + 86_399), "2026-07-08");
        assert_eq!(utc_date(1_783_468_800 + 86_400), "2026-07-09");
        // A day near a month boundary.
        assert_eq!(utc_date(1_783_468_800 + 86_400 * 24), "2026-08-01");
    }

    #[test]
    fn record_writes_a_dated_file_with_chain_fields() {
        let dir = temp_test_dir("append");
        let log = AuditLog::new(dir.clone());

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

        let file = sole_daily_file(&dir);
        let name = file.file_name().unwrap().to_string_lossy();
        assert!(
            name.starts_with("vibed-") && name.ends_with(".jsonl"),
            "dated file: {name}"
        );
        let content = fs::read_to_string(&file).expect("audit file readable");
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 2, "one JSON line per record");

        let first: Value = serde_json::from_str(lines[0]).expect("line 1 is JSON");
        assert_eq!(first["tool"], "os.status");
        assert_eq!(first["decision"], "allow");
        assert_eq!(first["caller_uid"], 1000);
        assert_eq!(first["seq"], 0);
        assert_eq!(first["prev"], CHAIN_IV);
        assert_eq!(first["hash"].as_str().map(str::len), Some(64));

        let second: Value = serde_json::from_str(lines[1]).expect("line 2 is JSON");
        assert_eq!(second["target"], "htop");
        assert!(second["caller_uid"].is_null());
        assert_eq!(second["seq"], 1);
        assert_eq!(second["prev"], first["hash"], "the chain links");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn intact_chain_verifies() {
        let dir = temp_test_dir("verify-ok");
        let log = AuditLog::new(dir.clone());
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
        let v = verify_chain(&dir).expect("verify");
        assert!(v.ok, "unbroken chain must verify: {v:?}");
        assert_eq!(v.records, 5);

        // An absent directory is trivially intact (0 records).
        let v = verify_chain(&dir.join("nope")).expect("verify absent");
        assert!(v.ok && v.records == 0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn restart_continues_the_chain() {
        let dir = temp_test_dir("restart");
        {
            let log = AuditLog::new(dir.clone());
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
        // A fresh AuditLog on the same dir must recover seq=2, prev=last hash.
        let log2 = AuditLog::new(dir.clone());
        log2.record(
            "os.status",
            &json!({}),
            None,
            "allow",
            "ok",
            Caller::default(),
        )
        .expect("r2");
        let v = verify_chain(&dir).expect("verify");
        assert!(v.ok, "chain must stay intact across a restart: {v:?}");
        assert_eq!(v.records, 3);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn chain_is_continuous_across_daily_files() {
        // Simulate two days by hand-writing dated files whose chain continues.
        let dir = temp_test_dir("multiday");
        fs::create_dir_all(&dir).unwrap();
        // Reuse AuditLog to generate a valid 3-record chain, then split it into
        // two dated files to prove verify_chain spans them.
        let log = AuditLog::new(dir.clone());
        for _ in 0..3 {
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
        let file = sole_daily_file(&dir);
        let lines: Vec<String> = fs::read_to_string(&file)
            .unwrap()
            .lines()
            .map(String::from)
            .collect();
        fs::remove_file(&file).unwrap();
        // Day 1: records 0-1 ; Day 2: record 2. Names sort chronologically.
        fs::write(
            dir.join("vibed-2026-07-13.jsonl"),
            lines[..2].join("\n") + "\n",
        )
        .unwrap();
        fs::write(dir.join("vibed-2026-07-14.jsonl"), lines[2].clone() + "\n").unwrap();
        let v = verify_chain(&dir).expect("verify");
        assert!(v.ok, "chain must be continuous across daily files: {v:?}");
        assert_eq!(v.records, 3);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn tampering_with_a_record_is_detected() {
        let dir = temp_test_dir("tamper-content");
        let log = AuditLog::new(dir.clone());
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
        let file = sole_daily_file(&dir);
        let content = fs::read_to_string(&file).unwrap();
        let mut lines: Vec<String> = content.lines().map(String::from).collect();
        lines[1] = lines[1].replace("\"ok\"", "\"denied\"");
        fs::write(&file, lines.join("\n") + "\n").unwrap();

        let v = verify_chain(&dir).expect("verify");
        assert!(!v.ok, "content tampering must break verification");
        assert_eq!(v.broken_at, Some(1));
        assert!(v.reason.unwrap().contains("hash mismatch"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn deleting_a_record_is_detected() {
        let dir = temp_test_dir("tamper-delete");
        let log = AuditLog::new(dir.clone());
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
        let file = sole_daily_file(&dir);
        let content = fs::read_to_string(&file).unwrap();
        let mut lines: Vec<String> = content.lines().map(String::from).collect();
        lines.remove(2);
        fs::write(&file, lines.join("\n") + "\n").unwrap();

        let v = verify_chain(&dir).expect("verify");
        assert!(!v.ok, "deleting a record must break the chain");
        assert_eq!(v.broken_at, Some(2));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn record_and_record_with_digest_write_the_same_digest_field() {
        let dir = temp_test_dir("digest-parity");
        let log = AuditLog::new(dir.clone());
        let args = json!({"name": "htop"});
        let digest = fnv1a_64_hex(args.to_string().as_bytes());

        // One record through each entry point, same arguments.
        log.record(
            "pkg.install",
            &args,
            Some("htop"),
            "allow",
            "ok",
            Caller::default(),
        )
        .expect("record");
        log.record_with_digest(
            "pkg.install",
            &digest,
            Some("htop"),
            "allow",
            "ok",
            Caller::default(),
        )
        .expect("record_with_digest");

        let content = fs::read_to_string(sole_daily_file(&dir)).unwrap();
        let lines: Vec<Value> = content
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(
            lines[0]["args_fnv1a64"], lines[1]["args_fnv1a64"],
            "both entry points must produce the identical correlation digest"
        );
        assert_eq!(lines[0]["args_fnv1a64"], Value::String(digest));
        // The chain still verifies across both entry points.
        assert!(verify_chain(&dir).unwrap().ok);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn digest_is_stable_and_input_sensitive() {
        assert_eq!(
            fnv1a_64_hex(b""),
            format!("{:016x}", 0xcbf2_9ce4_8422_2325_u64)
        );
        assert_eq!(fnv1a_64_hex(b"abc"), fnv1a_64_hex(b"abc"));
        assert_ne!(fnv1a_64_hex(b"abc"), fnv1a_64_hex(b"abd"));
    }
}
