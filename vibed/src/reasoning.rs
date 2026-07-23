//! Agent reasoning store — the durable capture of what an agent *thought*.
//!
//! Delivers the storage + read side of ADR-012 (docs/DECISIONS.md). The agent
//! supervisor (Phase 2.5, `vibectl agent run`) TAPS the CLI's structured stream
//! (`stream-json`) as it streams and appends each `thinking` block here, one
//! JSONL line per block, at `<memory>/reasoning/<session-id>.jsonl`. This is
//! independent of whatever transcript the CLI keeps on its own — current models
//! blank the thinking field of their saved JSONL, so there is nothing to re-read
//! after the fact if VibeOS did not capture it live.
//!
//! Trust model: the store lives under `/var/lib/vibeos/memory/reasoning/`, which
//! is on the built-in WRITE denylist (`/var/lib/vibeos/memory/**`) — agents can
//! never write it via `fs.write`. Only the supervisor writes it. Agents READ it
//! through the T0 `agent.thinking` MCP tool (bounded), never by path. A
//! `session_id` becomes a filename, so it is charset-validated (no traversal).

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

/// Subdirectory of the memory store holding one JSONL file per session.
pub const REASONING_SUBDIR: &str = "reasoning";
/// Hard cap on one serialized reasoning line (a thinking block can be several
/// KiB per turn — larger than a plain journal entry, hence its own, larger cap).
pub const REASONING_MAX_LINE_BYTES: usize = 64 * 1024;
/// Hard cap on how many lines `read_thinking` will ever return (anti-DoS: a long
/// autonomous run can accumulate thousands of blocks).
pub const REASONING_MAX_TAIL: usize = 500;
/// Default tail when the caller does not specify one.
pub const REASONING_DEFAULT_TAIL: usize = 100;
/// Hard cap on how many sessions `list_sessions` ever returns. The store grows
/// one file per autonomous run and is never purged automatically (retention is
/// still open — ADR-012), so an uncapped listing would make a repeatable T0 call
/// return an ever-growing response. Newest activity wins; the caller is told the
/// true total so it can say "showing N of M" rather than pretend it saw all.
pub const REASONING_MAX_SESSIONS: usize = 200;

/// `O_NOFOLLOW` (Linux): refuse to open a final component that is a symlink.
const O_NOFOLLOW: i32 = 0x20000;

/// Validate a `session_id` used as a filename component. Charset is
/// `[A-Za-z0-9._-]`, non-empty, ≤ 128 chars, and never contains `..` — so it
/// can never traverse out of the reasoning directory (there is no `/` in the
/// charset either). Returns the id if safe.
pub fn safe_session_id(id: &str) -> Option<&str> {
    let ok = !id.is_empty()
        && id.len() <= 128
        && !id.contains("..")
        && id != "."
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    ok.then_some(id)
}

/// Path of a session's reasoning file under the memory `root`.
fn session_file(root: &Path, session_id: &str) -> PathBuf {
    root.join(REASONING_SUBDIR)
        .join(format!("{session_id}.jsonl"))
}

/// Append one thinking block to `<root>/reasoning/<session_id>.jsonl`. Called by
/// the supervisor as it taps the CLI stream. `block` is the captured content
/// (opaque object); `ts` is stamped by the writer. Strictly additive
/// (`O_APPEND` + `O_NOFOLLOW`, private mode, per-line byte cap) — same discipline
/// as the memory journal. Never creates the store root (fail-closed if Genesis
/// has not run).
pub fn append_thinking(
    root: &Path,
    session_id: &str,
    block: &Value,
    ts_unix: u64,
) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};

    let id = safe_session_id(session_id).ok_or_else(|| {
        "reasoning: invalid session_id (charset [A-Za-z0-9._-], no '..')".to_string()
    })?;
    if !root.is_dir() {
        return Err("reasoning: memory store absent (Genesis has not run)".to_string());
    }

    let record = json!({ "ts_unix": ts_unix, "session_id": id, "block": block });
    let mut line = serde_json::to_string(&record)
        .map_err(|e| format!("reasoning: serialization failed: {e}"))?;
    line.push('\n');
    if line.len() > REASONING_MAX_LINE_BYTES {
        return Err(format!(
            "reasoning: block exceeds the {} KiB line cap",
            REASONING_MAX_LINE_BYTES / 1024
        ));
    }

    let dir = root.join(REASONING_SUBDIR);
    if !dir.is_dir() {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)
            .map_err(|e| format!("reasoning: cannot create store dir: {e}"))?;
    }

    static APPEND_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = APPEND_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let target = session_file(root, id);
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .custom_flags(O_NOFOLLOW)
        .open(&target)
        .map_err(|e| format!("reasoning append {id}: {e}"))?;
    file.write_all(line.as_bytes())
        .map_err(|e| format!("reasoning append {id}: {e}"))
}

/// Read a bounded tail of a session's reasoning (the read side of the T0
/// `agent.thinking` tool). Returns `{ session_id, lines: [...], count, truncated }`.
/// `tail` is clamped to `REASONING_MAX_TAIL`; `since_unix`, if given, drops lines
/// with `ts_unix < since_unix`. An absent session file is not an error — it
/// yields an empty result (the session may not have started, or capture is off).
pub fn read_thinking(
    root: &Path,
    session_id: &str,
    tail: Option<usize>,
    since_unix: Option<u64>,
) -> Result<Value, String> {
    let id = safe_session_id(session_id).ok_or_else(|| {
        "reasoning: invalid session_id (charset [A-Za-z0-9._-], no '..')".to_string()
    })?;
    let want = tail
        .unwrap_or(REASONING_DEFAULT_TAIL)
        .min(REASONING_MAX_TAIL);

    let path = session_file(root, id);
    // Bounded read: for a tail we only need the END of the file. Reading the
    // whole file (a multi-hour run can reach hundreds of MB) into memory on a
    // repeatable T0 call is a memory-amplification vector — cap it.
    let Some((content, window_bounded)) = read_tail_string(&path, READ_TAIL_CAP) else {
        return Ok(json!({ "session_id": id, "lines": [], "count": 0, "truncated": false }));
    };

    // If we started mid-file, the first line is likely partial — drop it.
    let mut lines = content.lines();
    if window_bounded {
        lines.next();
    }
    let mut parsed: Vec<Value> = lines
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|v| match since_unix {
            Some(s) => v.get("ts_unix").and_then(Value::as_u64).unwrap_or(0) >= s,
            None => true,
        })
        .collect();

    let total = parsed.len();
    let truncated = total > want;
    if truncated {
        parsed.drain(0..total - want); // keep the newest `want` lines
    }
    Ok(json!({
        "session_id": id,
        "lines": parsed,
        "count": total,
        "truncated": truncated,
        // True when the file was larger than the read window: `count` reflects
        // the window (the newest ~READ_TAIL_CAP bytes), not the whole file.
        "window_bounded": window_bounded,
    }))
}

/// Cap on how many bytes `read_thinking` reads from the tail of a session file.
const READ_TAIL_CAP: u64 = 4 * 1024 * 1024;

/// Read up to `cap` bytes from the END of `path`. Returns `(content, bounded)`;
/// `bounded` is true when the file exceeded `cap` (so the content starts mid-file
/// and its first line may be partial). `None` if the file is absent.
fn read_tail_string(path: &Path, cap: u64) -> Option<(String, bool)> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    if len <= cap {
        let mut s = String::new();
        file.read_to_string(&mut s).ok()?;
        return Some((s, false));
    }
    file.seek(SeekFrom::Start(len - cap)).ok()?;
    let mut bytes = Vec::new();
    file.take(cap).read_to_end(&mut bytes).ok()?;
    Some((String::from_utf8_lossy(&bytes).into_owned(), true))
}

/// What is known about one captured session WITHOUT reading its whole file.
/// `bytes`/`last_unix` come from a stat; `started_unix` from a bounded read of
/// the first line only. Deliberately carries no `provider`/`model`: the store
/// does not hold them (the supervisor records those in the memory journal, as an
/// `autonomous_session` record) and this listing never guesses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionMeta {
    pub id: String,
    /// `ts_unix` of the first captured block, or `None` when the first line is
    /// absent, truncated past `READ_HEAD_CAP`, or unparsable — never guessed.
    pub started_unix: Option<u64>,
    /// Last append, taken from the file mtime rather than a block timestamp: the
    /// store is strictly append-only, so mtime IS when the newest block landed,
    /// and it costs a stat instead of a tail read.
    pub last_unix: u64,
    pub bytes: u64,
}

/// List captured reasoning sessions, newest activity first, capped at
/// `REASONING_MAX_SESSIONS`. Returns `(sessions, total_present)`.
///
/// Cost is bounded BY CONSTRUCTION, because this backs a repeatable T0 tool that
/// the HUD polls: every candidate is only stat'ed (cheap), the window is sorted
/// and truncated FIRST, and only the surviving `REASONING_MAX_SESSIONS` files
/// have their first line read. Counting lines exactly would mean reading every
/// file end to end on every poll — a multi-MB-per-call amplification vector — so
/// this exposes `bytes` (free, from the stat) and never a turn count.
pub fn list_sessions(root: &Path) -> (Vec<SessionMeta>, usize) {
    let dir = root.join(REASONING_SUBDIR);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return (Vec::new(), 0);
    };
    // Stat-only pass: (id, last_unix, bytes). No file content is read here.
    let mut rows: Vec<(String, u64, u64)> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let id = name.strip_suffix(".jsonl")?.to_string();
            // The store is ours, but never hand back a filename that could not
            // round-trip through `agent.thinking` — an id outside the safe
            // charset is unusable to the caller, so it is not listed.
            safe_session_id(&id)?;
            let md = e.metadata().ok()?;
            if !md.is_file() {
                return None;
            }
            let mtime = md
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs());
            Some((id, mtime, md.len()))
        })
        .collect();

    let total = rows.len();
    // Newest activity first. The id breaks ties so the order stays deterministic
    // when two files share an mtime second. This replaces an earlier lexical
    // sort, which only looked chronological because ids happen to embed a
    // fixed-width timestamp — any other id shape silently broke `latest`.
    rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| b.0.cmp(&a.0)));
    rows.truncate(REASONING_MAX_SESSIONS);

    let sessions = rows
        .into_iter()
        .map(|(id, last_unix, bytes)| {
            let started_unix = first_block_ts_cached(&session_file(root, &id));
            SessionMeta {
                id,
                started_unix,
                last_unix,
                bytes,
            }
        })
        .collect();
    (sessions, total)
}

/// Memoized [`first_block_ts`]. The store is strictly append-only (`O_APPEND`,
/// one line at a time), so a session file's FIRST line — and therefore its
/// `ts_unix` — is immutable once written: reading it again on every HUD poll
/// (one open + head read + parse per listed session, per probe) only repeats
/// work. A `None` head (empty or torn file) is NOT cached, so a session whose
/// first line lands later is retried. Hard-bounded: the map is cleared when it
/// outgrows several full rosters (an out-of-band store purge then repopulates
/// it on demand — at worst one uncached probe).
fn first_block_ts_cached(path: &Path) -> Option<u64> {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<HashMap<std::path::PathBuf, u64>>> = OnceLock::new();
    const CACHE_MAX: usize = 4 * REASONING_MAX_SESSIONS;

    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    {
        let map = cache.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(ts) = map.get(path) {
            return Some(*ts);
        }
    }
    let ts = first_block_ts(path)?;
    let mut map = cache.lock().unwrap_or_else(|p| p.into_inner());
    if map.len() >= CACHE_MAX {
        map.clear();
    }
    map.insert(path.to_path_buf(), ts);
    Some(ts)
}

/// Cap on how many bytes are read to recover a session's first line. A line is
/// capped at `REASONING_MAX_LINE_BYTES` on write, so this window always holds a
/// complete first line for a file we wrote — and bounds the work for one we did
/// not.
const READ_HEAD_CAP: u64 = REASONING_MAX_LINE_BYTES as u64;

/// `ts_unix` of the first line of a session file, read within `READ_HEAD_CAP`.
/// `None` when the file is empty, its first line overruns the cap, or it carries
/// no `ts_unix` — the caller then reports an unknown start rather than a guess.
fn first_block_ts(path: &Path) -> Option<u64> {
    use std::io::Read;
    let file = std::fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(READ_HEAD_CAP).read_to_end(&mut bytes).ok()?;
    let head = String::from_utf8_lossy(&bytes);
    // A complete line always ends in '\n' (`append_thinking` appends one). If the
    // cap cut the line short there is no terminator, so refuse to parse it.
    let line = head.split_once('\n')?.0;
    serde_json::from_str::<Value>(line)
        .ok()?
        .get("ts_unix")
        .and_then(Value::as_u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("vibed-reason-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn safe_session_id_rejects_traversal_and_bad_charset() {
        assert!(safe_session_id("auto-abc123").is_some());
        assert!(safe_session_id("2026-07-13_run.1").is_some());
        assert!(safe_session_id("").is_none());
        assert!(safe_session_id(".").is_none());
        assert!(safe_session_id("..").is_none());
        assert!(safe_session_id("../etc/passwd").is_none()); // '/' + '..'
        assert!(safe_session_id("a..b").is_none()); // contains ".."
        assert!(safe_session_id("a/b").is_none());
        assert!(safe_session_id("a b").is_none());
        assert!(safe_session_id(&"x".repeat(129)).is_none());
    }

    #[test]
    fn append_then_read_tail_and_since() {
        let root = scratch("rw");
        for i in 0..5u64 {
            append_thinking(
                &root,
                "sess1",
                &json!({ "delta": format!("thought {i}") }),
                1_000 + i,
            )
            .expect("append");
        }
        // Full read.
        let all = read_thinking(&root, "sess1", None, None).unwrap();
        assert_eq!(all["count"], 5);
        assert_eq!(all["truncated"], false);
        assert_eq!(all["lines"].as_array().unwrap().len(), 5);
        assert_eq!(all["lines"][0]["block"]["delta"], "thought 0");

        // Tail keeps the NEWEST lines.
        let tail = read_thinking(&root, "sess1", Some(2), None).unwrap();
        assert_eq!(tail["truncated"], true);
        let lines = tail["lines"].as_array().unwrap();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["block"]["delta"], "thought 3");
        assert_eq!(lines[1]["block"]["delta"], "thought 4");

        // `since` filter.
        let since = read_thinking(&root, "sess1", None, Some(1_003)).unwrap();
        assert_eq!(since["lines"].as_array().unwrap().len(), 2); // ts 1003, 1004
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn tail_is_capped_at_max() {
        let root = scratch("cap");
        append_thinking(&root, "s", &json!({"x": 1}), 1).unwrap();
        // Asking for more than the cap is clamped, never rejected.
        let out = read_thinking(&root, "s", Some(100_000), None).unwrap();
        assert_eq!(out["count"], 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn absent_session_is_empty_not_error() {
        let root = scratch("absent");
        let out = read_thinking(&root, "nope", None, None).unwrap();
        assert_eq!(out["count"], 0);
        assert_eq!(out["lines"].as_array().unwrap().len(), 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn append_rejects_bad_id_and_missing_store() {
        let root = scratch("guard");
        assert!(append_thinking(&root, "../evil", &json!({}), 1).is_err());
        assert!(read_thinking(&root, "../evil", None, None).is_err());
        // Missing store root -> fail-closed (never created).
        let missing =
            std::env::temp_dir().join(format!("vibed-reason-none-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&missing);
        assert!(append_thinking(&missing, "s", &json!({}), 1).is_err());
        assert!(!missing.exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn read_tail_string_bounds_large_files() {
        let root = scratch("tail");
        let path = root.join("big.txt");
        // 10 lines; read with a tiny cap so only the last few bytes are read.
        let content = (0..10).map(|i| format!("line{i}\n")).collect::<String>();
        std::fs::write(&path, &content).unwrap();
        // Whole-file read (cap larger than file).
        let (all, bounded) = read_tail_string(&path, 10_000).unwrap();
        assert!(!bounded);
        assert_eq!(all, content);
        // Bounded read: last 12 bytes -> starts mid-file, flagged bounded.
        let (tail, bounded) = read_tail_string(&path, 12).unwrap();
        assert!(bounded);
        assert!(tail.len() <= 12);
        assert!(tail.ends_with("line9\n"));
        assert!(read_tail_string(&root.join("absent"), 100).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn list_sessions_returns_metadata_newest_first() {
        let root = scratch("list");
        append_thinking(&root, "alpha", &json!({}), 111).unwrap();
        append_thinking(&root, "alpha", &json!({}), 222).unwrap();
        append_thinking(&root, "beta", &json!({}), 333).unwrap();

        let (sessions, total) = list_sessions(&root);
        assert_eq!(total, 2);
        assert_eq!(sessions.len(), 2);

        let ids: Vec<&str> = sessions.iter().map(|s| s.id.as_str()).collect();
        assert!(
            ids.contains(&"alpha") && ids.contains(&"beta"),
            "both sessions listed: {ids:?}"
        );

        let alpha = sessions.iter().find(|s| s.id == "alpha").unwrap();
        // `started_unix` is the FIRST block's ts, not the last — the whole point
        // of reading the head rather than the tail.
        assert_eq!(alpha.started_unix, Some(111));
        assert!(alpha.bytes > 0, "bytes comes from the stat: {alpha:?}");
        // mtime is wall-clock, unrelated to the `ts_unix` we stamped.
        assert!(alpha.last_unix > 0, "last_unix from mtime: {alpha:?}");

        let beta = sessions.iter().find(|s| s.id == "beta").unwrap();
        assert_eq!(beta.started_unix, Some(333));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn list_sessions_is_capped_but_reports_the_true_total() {
        // NB: the scratch tag must be unique across this module — `scratch`
        // wipes the directory, so a shared tag makes parallel tests race.
        let root = scratch("listcap");
        let over = REASONING_MAX_SESSIONS + 5;
        for i in 0..over {
            append_thinking(&root, &format!("s{i:04}"), &json!({}), i as u64).unwrap();
        }
        let (sessions, total) = list_sessions(&root);
        // The listing is bounded, but never lies about how much exists: a caller
        // can tell it is seeing a window.
        assert_eq!(sessions.len(), REASONING_MAX_SESSIONS);
        assert_eq!(total, over);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn list_sessions_started_ts_is_stable_across_appends_and_probes() {
        let root = scratch("startcache");
        append_thinking(&root, "s", &json!({"a": 1}), 100).unwrap();
        let (first, _) = list_sessions(&root);
        assert_eq!(first[0].started_unix, Some(100));

        // Append-only growth: the first line is immutable, so the (now cached)
        // start timestamp must stay exactly the first block's, probe after
        // probe — never drift to a later block's ts.
        append_thinking(&root, "s", &json!({"a": 2}), 200).unwrap();
        let (second, _) = list_sessions(&root);
        assert_eq!(
            second[0].started_unix,
            Some(100),
            "start ts is the FIRST block's"
        );
        assert!(second[0].last_unix >= first[0].last_unix);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn list_sessions_reports_unknown_start_rather_than_guessing() {
        let root = scratch("badhead");
        append_thinking(&root, "good", &json!({}), 42).unwrap();
        // A file in the store that we did not write cleanly: empty, and one whose
        // first line is not JSON. Neither may fabricate a start timestamp.
        let dir = root.join(REASONING_SUBDIR);
        std::fs::write(dir.join("empty.jsonl"), b"").unwrap();
        std::fs::write(dir.join("garbage.jsonl"), b"not json at all\n").unwrap();

        let (sessions, total) = list_sessions(&root);
        assert_eq!(total, 3);
        let started = |id: &str| sessions.iter().find(|s| s.id == id).unwrap().started_unix;
        assert_eq!(started("good"), Some(42));
        assert_eq!(started("empty"), None, "empty file -> unknown start");
        assert_eq!(started("garbage"), None, "unparsable head -> unknown start");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn list_sessions_skips_unusable_names() {
        let root = scratch("names");
        append_thinking(&root, "real", &json!({}), 1).unwrap();
        let dir = root.join(REASONING_SUBDIR);
        // Not a .jsonl -> not a session file at all.
        std::fs::write(dir.join("notes.txt"), b"x").unwrap();
        // A .jsonl whose stem could never round-trip through `agent.thinking`
        // (safe_session_id rejects "..") must not be handed to the caller.
        std::fs::write(dir.join("a..b.jsonl"), b"{}\n").unwrap();

        let (sessions, total) = list_sessions(&root);
        assert_eq!(total, 1, "only the usable session counts: {sessions:?}");
        assert_eq!(sessions[0].id, "real");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn list_sessions_on_missing_store_is_empty_not_error() {
        let root = scratch("nostore");
        let (sessions, total) = list_sessions(&root.join("never-created"));
        assert!(sessions.is_empty());
        assert_eq!(total, 0);
        let _ = std::fs::remove_dir_all(&root);
    }
}
