//! `memory.*` tools — the VibeOS memory store (query + governed append).
//!
//! Extracted from `mcp.rs` (F6, mechanical split — zero behaviour change).
//! memory.query (T0, bounded content snippets) and memory.append (T1, strictly
//! additive). Shared constants and the `utc_*`/`MEMORY_DIR` helpers stay in
//! `mcp.rs` (also used by the wiring / agent tools / vibectl) and are imported.

use serde_json::{json, Value};

use crate::audit::Caller;
use crate::mcp::{
    utc_date_string, utc_iso8601, JOURNAL_AGENT_TYPES, JOURNAL_RESERVED_TYPES, MAX_APPEND_BYTES,
    MAX_MEMORY_FILES, MAX_MEMORY_SCAN_BYTES, MEMORY_DIR, MEMORY_SCOPES, MEMORY_SNIPPET_CHARS,
    O_NOFOLLOW,
};

pub(crate) fn memory_query(args: &Value) -> Result<String, String> {
    memory_query_at(std::path::Path::new(MEMORY_DIR), args)
}

/// Read up to MAX_MEMORY_SCAN_BYTES of `path` (bounded, O_NOFOLLOW) and return
/// the UTF-8-lossy text plus whether the file exceeded the scan window. The
/// bound caps the allocation; O_NOFOLLOW refuses a final component swapped for
/// a symlink (TOCTOU) instead of following it out of the store.
fn read_memory_scan(path: &std::path::Path) -> Option<(String, bool)> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(O_NOFOLLOW)
        .open(path)
        .ok()?;
    let mut bytes = Vec::new();
    file.take((MAX_MEMORY_SCAN_BYTES as u64) + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    let longer = bytes.len() > MAX_MEMORY_SCAN_BYTES;
    let end = bytes.len().min(MAX_MEMORY_SCAN_BYTES);
    Some((String::from_utf8_lossy(&bytes[..end]).into_owned(), longer))
}

/// Body of memory.query with the store root as a parameter, so the tests can
/// run it against a scratch layout without touching /var/lib/vibeos/memory.
fn memory_query_at(root: &std::path::Path, args: &Value) -> Result<String, String> {
    let query = args
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_lowercase();

    // `scope` restricts the walk to one entry of the docs/MEMORY.md §3 layout.
    let scope = match args.get("scope") {
        None | Some(Value::Null) => None,
        Some(Value::String(name)) => match MEMORY_SCOPES.iter().find(|(n, _, _)| n == name) {
            Some(entry) => Some(*entry),
            None => {
                let valid: Vec<&str> = MEMORY_SCOPES.iter().map(|(n, _, _)| *n).collect();
                return Err(format!(
                    "memory.query: unknown scope '{name}' (valid: {})",
                    valid.join(", ")
                ));
            }
        },
        Some(_) => return Err("memory.query: 'scope' must be a string".to_string()),
    };

    // `limit` caps the number of matches returned; the walk itself stays
    // bounded by MAX_MEMORY_FILES regardless.
    let limit = match args.get("limit") {
        None | Some(Value::Null) => MAX_MEMORY_FILES,
        Some(value) => match value.as_u64() {
            Some(n) if n >= 1 => (n as usize).min(MAX_MEMORY_FILES),
            _ => return Err("memory.query: 'limit' must be an integer >= 1".to_string()),
        },
    };

    if !root.is_dir() {
        return Ok(json!({
            "initialized": false,
            "note": "memory store absent: vibeos-genesis.service has not run yet, \
                     or amnesic mode (Phase 3 target) discarded it at shutdown"
        })
        .to_string());
    }

    // Bounded iterative walk: no recursion, hard cap on visited files, and
    // NEVER follows symlinks (entry.file_type() / symlink_metadata do not
    // traverse) — a link planted inside the store cannot route the walk (or
    // the content scan) outside /var/lib/vibeos/memory, e.g. into the audit
    // trail.
    let mut files = Vec::new();
    let mut stack: Vec<std::path::PathBuf> = Vec::new();
    // A walk root is only pushed if it is a REAL directory (symlink_metadata
    // does not follow): if `journal` (or the store root itself) is a symlink,
    // the walk must not descend through it out of the store.
    let push_if_real_dir = |stack: &mut Vec<std::path::PathBuf>, p: std::path::PathBuf| {
        if p.symlink_metadata().is_ok_and(|m| m.file_type().is_dir()) {
            stack.push(p);
        }
    };
    match scope {
        None => push_if_real_dir(&mut stack, root.to_path_buf()),
        Some((_, relative, is_dir)) => {
            let start = root.join(relative);
            if is_dir {
                push_if_real_dir(&mut stack, start);
            } else if start
                .symlink_metadata()
                .is_ok_and(|m| m.file_type().is_file())
            {
                files.push(start);
            }
        }
    }
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if files.len() >= MAX_MEMORY_FILES {
                break;
            }
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                stack.push(entry.path());
            } else if file_type.is_file() {
                files.push(entry.path());
            }
            // Symlinks (and any other special type) are deliberately skipped.
        }
        if files.len() >= MAX_MEMORY_FILES {
            break;
        }
    }

    let mut matches = Vec::new();
    let mut truncated = false;
    for path in &files {
        if matches.len() >= limit {
            truncated = true;
            break;
        }
        let relative = path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string();
        // Read a bounded window ONCE and reuse it for both the content match
        // test and the returned snippet.
        let scanned = read_memory_scan(path);
        let name_hit = !query.is_empty() && relative.to_lowercase().contains(&query);
        let content_hit = !query.is_empty()
            && scanned
                .as_ref()
                .is_some_and(|(text, _)| text.to_lowercase().contains(&query));
        if !(query.is_empty() || name_hit || content_hit) {
            continue;
        }
        // Bounded snippet so the agent can read the memory in this single call.
        let (snippet, snippet_truncated) = match &scanned {
            Some((text, longer)) => {
                let snippet: String = text.chars().take(MEMORY_SNIPPET_CHARS).collect();
                let trunc = *longer || snippet.chars().count() < text.chars().count();
                (snippet, trunc)
            }
            None => (String::new(), false),
        };
        matches.push(json!({
            "file": relative,
            "snippet": snippet,
            "snippet_truncated": snippet_truncated
        }));
    }

    Ok(json!({
        "initialized": true,
        "query": query,
        "scope": scope.map(|(name, _, _)| name),
        "scanned_files": files.len(),
        "matches": matches,
        "truncated": truncated
    })
    .to_string())
}

pub(crate) fn memory_append(args: &Value) -> Result<String, String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| format!("memory.append: system clock error: {e}"))?
        .as_secs();
    memory_append_at(std::path::Path::new(MEMORY_DIR), args, now)
}

/// Body of memory.append with the store root and the clock as parameters
/// (deterministic, filesystem-scratch-friendly tests).
///
/// Strictly ADDITIVE by construction: the only operation is an O_APPEND write
/// of one serialized line to a scope-derived file — there is no delete, no
/// rewrite, and no caller-controlled path (docs/MEMORY.md §9). The caller's
/// authoritative identity (uid/gid/pid) is recorded by the audit pipeline;
/// the `source` field below is self-declared memory metadata.
fn memory_append_at(
    root: &std::path::Path,
    args: &Value,
    epoch_secs: u64,
) -> Result<String, String> {
    let scope = args
        .get("scope")
        .and_then(Value::as_str)
        .ok_or_else(|| "memory.append: missing 'scope' argument".to_string())?;
    let entry = args
        .get("entry")
        .ok_or_else(|| "memory.append: missing 'entry' argument".to_string())?;
    if !entry.is_object() {
        return Err("memory.append: 'entry' must be an object".to_string());
    }

    if !root.is_dir() {
        return Err(
            "memory.append: memory store absent: vibeos-genesis.service has not run yet \
             (nothing to append to; fail-closed)"
                .to_string(),
        );
    }

    let ts = utc_iso8601(epoch_secs);
    let (relative_file, line_value) = match scope {
        "journal" => {
            let event_type = entry.get("type").and_then(Value::as_str).ok_or_else(|| {
                "memory.append: missing 'type' field in journal entry".to_string()
            })?;
            if JOURNAL_RESERVED_TYPES.contains(&event_type) {
                return Err(format!(
                    "memory.append: journal type '{event_type}' is reserved for the system \
                     (genesis.sh, vibed, vibectl) and cannot be appended by an agent"
                ));
            }
            if !JOURNAL_AGENT_TYPES.contains(&event_type) {
                return Err(format!(
                    "memory.append: unknown journal type '{event_type}' (valid: {})",
                    JOURNAL_AGENT_TYPES.join(", ")
                ));
            }
            let source = validated_source(entry)?;
            let data = entry.get("data").cloned().unwrap_or_else(|| json!({}));
            if !data.is_object() {
                return Err("memory.append: 'data' must be an object".to_string());
            }
            (
                format!("journal/{}.jsonl", utc_date_string(epoch_secs)),
                json!({ "ts": ts, "type": event_type, "source": source, "data": data }),
            )
        }
        "knowledge" => {
            let subject = required_entry_str(entry, "subject", 256)?;
            let fact = required_entry_str(entry, "fact", 4096)?;
            let source = validated_source(entry)?;
            let confidence = match entry.get("confidence") {
                None | Some(Value::Null) => None,
                Some(value) => match value.as_f64() {
                    Some(c) if (0.0..=1.0).contains(&c) => Some(c),
                    _ => {
                        return Err(
                            "memory.append: 'confidence' must be a number between 0 and 1"
                                .to_string(),
                        )
                    }
                },
            };
            let mut fact_value = json!({
                "id": next_fact_id(epoch_secs),
                "ts": ts,
                "subject": subject,
                "fact": fact,
                "source": source
            });
            if let Some(c) = confidence {
                fact_value["confidence"] = json!(c);
            }
            ("knowledge/facts.jsonl".to_string(), fact_value)
        }
        "user" => {
            // Invariant §4 forbids read-modify-write / merge in memory.append.
            // So `user` is an APPEND-ONLY log of key/value updates
            // (user/updates.jsonl); the "current profile" is the fold of these
            // entries, last-write-wins per key (materialized by memory.query /
            // a future vibectl, never by rewriting a file here). See MEMORY §3.3.
            let key = required_entry_str(entry, "key", 256)?;
            if !key
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
            {
                return Err(
                    "memory.append: user 'key' must be a dotted config key [A-Za-z0-9._-] \
                     (e.g. 'preferences.editor')"
                        .to_string(),
                );
            }
            let value = entry
                .get("value")
                .cloned()
                .ok_or_else(|| "memory.append: missing 'value' field in user entry".to_string())?;
            let source = validated_source(entry)?;
            (
                "user/updates.jsonl".to_string(),
                json!({ "ts": ts, "key": key, "value": value, "source": source }),
            )
        }
        "projects" => {
            // Append-only project updates (projects/updates.jsonl); the current
            // project index is the fold, last-write-wins per `path`. A whitelist
            // of structured fields keeps the schema stable (unknown fields are
            // dropped, not stored). See MEMORY §3.4.
            let path = required_entry_str(entry, "path", 4096)?;
            if !path.starts_with('/') {
                return Err(
                    "memory.append: projects 'path' must be an absolute path (fold key)"
                        .to_string(),
                );
            }
            let source = validated_source(entry)?;
            let mut record = serde_json::Map::new();
            record.insert("ts".to_string(), json!(ts));
            record.insert("path".to_string(), json!(path));
            record.insert("source".to_string(), json!(source));
            for field in ["name", "languages", "vcs", "summary", "last_opened"] {
                if let Some(v) = entry.get(field) {
                    record.insert(field.to_string(), v.clone());
                }
            }
            ("projects/updates.jsonl".to_string(), Value::Object(record))
        }
        other => {
            return Err(format!(
                "memory.append: unknown scope '{other}' (writable: journal, knowledge)"
            ));
        }
    };

    let bytes = append_memory_jsonl(root, &relative_file, &line_value)?;
    Ok(json!({
        "appended": true,
        "file": relative_file,
        "bytes": bytes,
        "ts": ts
    })
    .to_string())
}

/// Append one JSON value as a single line to a memory-store-relative file,
/// with the store's append-only discipline: parent dir created 0700 if missing
/// (never the store root), file 0600, `O_APPEND` + `O_NOFOLLOW`, 16 KiB line
/// cap, and process-serialized so a line is never interleaved. Shared by
/// `memory.append` (agent scopes) and the system journal writer
/// (`journal_tool_call_at`). Returns the byte length written.
fn append_memory_jsonl(
    root: &std::path::Path,
    relative_file: &str,
    value: &Value,
) -> Result<usize, String> {
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};

    let mut line = serde_json::to_string(value)
        .map_err(|e| format!("memory append: serialization failed: {e}"))?;
    line.push('\n');
    if line.len() > MAX_APPEND_BYTES {
        return Err(format!(
            "memory append: entry exceeds the {} KiB line cap",
            MAX_APPEND_BYTES / 1024
        ));
    }

    let target = root.join(relative_file);
    let parent = target
        .parent()
        .ok_or_else(|| "memory append: internal error: target has no parent".to_string())?;
    // Genesis creates the subdirectories; recreate defensively with the same
    // private permissions if one is missing (never the store root itself).
    if !parent.is_dir() {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)
            .map_err(|e| format!("memory append: cannot create '{relative_file}' parent: {e}"))?;
    }

    // Serialize appends within the process; combined with O_APPEND, each entry
    // lands as one contiguous line even under concurrent connections.
    // O_NOFOLLOW: if the target name was swapped for a symlink, refuse rather
    // than follow it out of the store.
    static APPEND_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = APPEND_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .custom_flags(O_NOFOLLOW)
        .open(&target)
        .map_err(|e| format!("memory append {relative_file}: {e}"))?;
    file.write_all(line.as_bytes())
        .map_err(|e| format!("memory append {relative_file}: {e}"))?;
    Ok(line.len())
}

/// System-written journal event recording that an agent's tool call executed.
/// This is the RESERVED `tool_call` type (agents cannot append it via
/// memory.append); `source` is `vibed`. Distinct from the forensic audit log:
/// this feeds the machine's own memory ("what did I do?"), so only executed
/// state-changing actions are recorded, never secrets (the non-secret `target`
/// mirrors the audit target). See docs/MEMORY.md §3.5.
pub(crate) fn journal_tool_call_at(
    root: &std::path::Path,
    epoch_secs: u64,
    tool: &str,
    target: Option<&str>,
    tier: &str,
    caller: Caller,
) -> Result<(), String> {
    // Never create the store root: if Genesis has not run, there is nothing to
    // journal into (the caller treats this as a no-op).
    if !root.is_dir() {
        return Ok(());
    }
    let event = json!({
        "ts": utc_iso8601(epoch_secs),
        "type": "tool_call",
        "source": "vibed",
        "data": {
            "tool": tool,
            "target": target,
            "tier": tier,
            "caller_uid": caller.uid,
        }
    });
    let relative = format!("journal/{}.jsonl", utc_date_string(epoch_secs));
    append_memory_jsonl(root, &relative, &event).map(|_| ())
}

/// `source` field of a memory entry: the self-declared emitter label (e.g.
/// "claude-code"), constrained to a safe charset. The authoritative caller
/// identity lives in the audit trail, not here.
fn validated_source(entry: &Value) -> Result<String, String> {
    let source = entry
        .get("source")
        .and_then(Value::as_str)
        .ok_or_else(|| "memory.append: missing 'source' field in entry".to_string())?;
    let valid = !source.is_empty()
        && source.len() <= 64
        && source
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if !valid {
        return Err(
            "memory.append: 'source' must be 1-64 characters of [A-Za-z0-9._-]".to_string(),
        );
    }
    Ok(source.to_string())
}

/// Required bounded string field of a memory entry.
fn required_entry_str(entry: &Value, field: &str, max_len: usize) -> Result<String, String> {
    let value = entry
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("memory.append: missing '{field}' field in entry"))?;
    if value.is_empty() || value.len() > max_len {
        return Err(format!(
            "memory.append: '{field}' must be 1-{max_len} bytes"
        ));
    }
    Ok(value.to_string())
}

/// Unique-enough fact id: epoch seconds + pid + per-process sequence number.
/// No dedup semantics — facts are append-only; curation is a vibectl matter.
fn next_fact_id(epoch_secs: u64) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    format!("{epoch_secs}-{}-{n}", std::process::id())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // -- memory.query (scope/limit) and memory.append -------------------------

    /// A scratch memory store mimicking the docs/MEMORY.md §3 layout.
    fn memory_scratch(tag: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static SEQ: AtomicU32 = AtomicU32::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("vibed-mem-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for sub in ["user", "projects", "journal", "knowledge"] {
            std::fs::create_dir_all(dir.join(sub)).expect("create memory scratch");
        }
        std::fs::write(
            dir.join("identity.toml"),
            "schema = 1\nhostname = \"testhost\"\n",
        )
        .expect("write identity");
        std::fs::write(dir.join("user").join("profile.toml"), "lang = \"fr\"\n")
            .expect("write profile");
        std::fs::write(
            dir.join("journal").join("2026-01-01.jsonl"),
            "{\"ts\":\"2026-01-01T00:00:00Z\",\"type\":\"genesis\",\"source\":\"genesis.sh\",\"data\":{}}\n",
        )
        .expect("write journal");
        dir
    }

    fn parse_result(result: Result<String, String>) -> Value {
        serde_json::from_str(&result.expect("tool call succeeds")).expect("valid JSON payload")
    }

    #[test]
    fn memory_query_scope_restricts_the_walk() {
        let root = memory_scratch("qscope");
        // journal scope: only the journal file is visible.
        let payload = parse_result(memory_query_at(&root, &json!({"scope": "journal"})));
        assert_eq!(payload["scope"], "journal");
        assert_eq!(payload["scanned_files"], 1);
        assert_eq!(payload["matches"][0]["file"], "journal/2026-01-01.jsonl");
        // identity scope resolves the single file.
        let payload = parse_result(memory_query_at(&root, &json!({"scope": "identity"})));
        assert_eq!(payload["matches"][0]["file"], "identity.toml");
        // a scope never leaks files from another scope.
        let payload = parse_result(memory_query_at(&root, &json!({"scope": "knowledge"})));
        assert_eq!(payload["matches"].as_array().unwrap().len(), 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn memory_query_returns_bounded_content_snippets() {
        // The agent must be able to READ the memory in one call, not just learn
        // filenames (F2). Each match carries a bounded snippet.
        let root = memory_scratch("qsnippet");
        let payload = parse_result(memory_query_at(&root, &json!({"scope": "identity"})));
        let m = &payload["matches"][0];
        assert_eq!(m["file"], "identity.toml");
        assert!(
            m["snippet"].as_str().unwrap().contains("testhost"),
            "snippet must carry the file content, got: {}",
            m["snippet"]
        );
        assert_eq!(m["snippet_truncated"], false);

        // A file larger than the snippet window is flagged truncated and capped.
        std::fs::write(
            root.join("knowledge").join("big.md"),
            "x".repeat(MEMORY_SNIPPET_CHARS * 4),
        )
        .unwrap();
        let payload = parse_result(memory_query_at(&root, &json!({"scope": "knowledge"})));
        let big = payload["matches"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["file"] == "knowledge/big.md")
            .expect("big.md listed");
        assert!(big["snippet"].as_str().unwrap().chars().count() <= MEMORY_SNIPPET_CHARS);
        assert_eq!(big["snippet_truncated"], true);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn memory_query_rejects_invalid_scope_and_limit() {
        let root = memory_scratch("qbad");
        let err = memory_query_at(&root, &json!({"scope": "audit"})).unwrap_err();
        assert!(err.contains("unknown scope"), "unexpected error: {err}");
        let err = memory_query_at(&root, &json!({"scope": 3})).unwrap_err();
        assert!(err.contains("must be a string"), "unexpected error: {err}");
        let err = memory_query_at(&root, &json!({"limit": 0})).unwrap_err();
        assert!(err.contains("integer >= 1"), "unexpected error: {err}");
        let err = memory_query_at(&root, &json!({"limit": -4})).unwrap_err();
        assert!(err.contains("integer >= 1"), "unexpected error: {err}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn memory_query_limit_caps_and_flags_truncation() {
        let root = memory_scratch("qlimit");
        let payload = parse_result(memory_query_at(&root, &json!({"limit": 1})));
        assert_eq!(payload["matches"].as_array().unwrap().len(), 1);
        assert_eq!(payload["truncated"], true);
        // without a limit, the scratch store fits well under the cap.
        let payload = parse_result(memory_query_at(&root, &json!({})));
        assert!(payload["matches"].as_array().unwrap().len() >= 3);
        assert_eq!(payload["truncated"], false);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn memory_query_reports_uninitialized_store() {
        let root = std::env::temp_dir().join(format!("vibed-mem-absent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let payload = parse_result(memory_query_at(&root, &json!({})));
        assert_eq!(payload["initialized"], false);
    }

    #[test]
    fn memory_query_content_scan_is_bounded_per_file() {
        let root = memory_scratch("qbound");
        // The needle sits AFTER the 64 KiB scan cap: a bounded scan must not
        // see it (and must not have loaded the whole file to find out).
        let mut big = vec![b'a'; MAX_MEMORY_SCAN_BYTES + 1024];
        big.extend_from_slice(b"needle-beyond-cap");
        std::fs::write(root.join("knowledge").join("big.md"), &big).expect("write big");
        let payload = parse_result(memory_query_at(
            &root,
            &json!({"query": "needle-beyond-cap"}),
        ));
        assert_eq!(
            payload["matches"].as_array().unwrap().len(),
            0,
            "content beyond MAX_MEMORY_SCAN_BYTES must not be scanned"
        );
        // Same needle before the cap: found.
        std::fs::write(
            root.join("knowledge").join("small.md"),
            b"needle-before-cap",
        )
        .expect("write small");
        let payload = parse_result(memory_query_at(
            &root,
            &json!({"query": "needle-before-cap"}),
        ));
        assert_eq!(payload["matches"].as_array().unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn memory_query_walk_never_follows_symlinks() {
        let root = memory_scratch("qsymlink");
        // An out-of-store area holding a "secret" the walk must never reach.
        let outside =
            std::env::temp_dir().join(format!("vibed-mem-outside-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&outside);
        std::fs::create_dir_all(&outside).expect("mkdir outside");
        std::fs::write(outside.join("secret.txt"), "outside-secret-content").expect("write secret");
        // A symlinked DIRECTORY and a symlinked FILE planted inside the store.
        std::os::unix::fs::symlink(&outside, root.join("knowledge").join("linkdir"))
            .expect("dir symlink");
        std::os::unix::fs::symlink(
            outside.join("secret.txt"),
            root.join("knowledge").join("linkfile.txt"),
        )
        .expect("file symlink");
        // Neither shows up in a listing...
        let payload = parse_result(memory_query_at(&root, &json!({"scope": "knowledge"})));
        assert_eq!(
            payload["matches"].as_array().unwrap().len(),
            0,
            "symlinks inside the store must be invisible to the walk"
        );
        // ...nor can their content be matched.
        let payload = parse_result(memory_query_at(&root, &json!({"query": "outside-secret"})));
        assert_eq!(payload["matches"].as_array().unwrap().len(), 0);
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&outside);
    }

    // 2026-07-08T01:01:01Z (see the utc_helpers test for the constant's proof).
    const T_2026_07_08: u64 = 1_783_468_800 + 3_661;

    #[test]
    fn memory_append_journal_writes_one_dated_line() {
        let root = memory_scratch("ajournal");
        let args = json!({
            "scope": "journal",
            "entry": {
                "type": "observation",
                "source": "claude-code",
                "data": { "note": "le projet vibeos-ui utilise pnpm, pas npm" }
            }
        });
        let payload = parse_result(memory_append_at(&root, &args, T_2026_07_08));
        assert_eq!(payload["appended"], true);
        assert_eq!(payload["file"], "journal/2026-07-08.jsonl");
        let written = std::fs::read_to_string(root.join("journal").join("2026-07-08.jsonl"))
            .expect("journal file written");
        let event: Value = serde_json::from_str(written.trim()).expect("one valid JSON line");
        assert_eq!(event["ts"], "2026-07-08T01:01:01Z");
        assert_eq!(event["type"], "observation");
        assert_eq!(event["source"], "claude-code");
        assert_eq!(
            event["data"]["note"],
            "le projet vibeos-ui utilise pnpm, pas npm"
        );
        // A second append lands on a NEW line of the same file (append-only).
        let _ = memory_append_at(&root, &args, T_2026_07_08 + 60).expect("second append");
        let written =
            std::fs::read_to_string(root.join("journal").join("2026-07-08.jsonl")).unwrap();
        assert_eq!(written.lines().count(), 2, "appends must never overwrite");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn memory_append_rejects_reserved_and_unknown_journal_types() {
        let root = memory_scratch("atypes");
        for reserved in JOURNAL_RESERVED_TYPES {
            let err = memory_append_at(
                &root,
                &json!({"scope": "journal",
                        "entry": {"type": reserved, "source": "claude-code", "data": {}}}),
                T_2026_07_08,
            )
            .unwrap_err();
            assert!(
                err.contains("reserved"),
                "type '{reserved}': unexpected error: {err}"
            );
        }
        let err = memory_append_at(
            &root,
            &json!({"scope": "journal",
                    "entry": {"type": "opinion", "source": "claude-code", "data": {}}}),
            T_2026_07_08,
        )
        .unwrap_err();
        assert!(
            err.contains("unknown journal type"),
            "unexpected error: {err}"
        );
        assert!(!root.join("journal").join("2026-07-08.jsonl").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn memory_append_validates_source_data_and_size() {
        let root = memory_scratch("aguard");
        // bad source charset
        let err = memory_append_at(
            &root,
            &json!({"scope": "journal",
                    "entry": {"type": "observation", "source": "a b/c", "data": {}}}),
            T_2026_07_08,
        )
        .unwrap_err();
        assert!(err.contains("'source'"), "unexpected error: {err}");
        // data must be an object
        let err = memory_append_at(
            &root,
            &json!({"scope": "journal",
                    "entry": {"type": "observation", "source": "x", "data": "free text"}}),
            T_2026_07_08,
        )
        .unwrap_err();
        assert!(err.contains("'data'"), "unexpected error: {err}");
        // line cap (anti-DoS)
        let err = memory_append_at(
            &root,
            &json!({"scope": "journal",
                    "entry": {"type": "observation", "source": "x",
                              "data": {"blob": "A".repeat(MAX_APPEND_BYTES)}}}),
            T_2026_07_08,
        )
        .unwrap_err();
        assert!(err.contains("line cap"), "unexpected error: {err}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn memory_append_knowledge_stamps_id_and_ts() {
        let root = memory_scratch("afact");
        let payload = parse_result(memory_append_at(
            &root,
            &json!({"scope": "knowledge",
                    "entry": {"subject": "vibeos-ui", "fact": "utilise pnpm",
                              "source": "claude-code", "confidence": 0.9}}),
            T_2026_07_08,
        ));
        assert_eq!(payload["file"], "knowledge/facts.jsonl");
        let written = std::fs::read_to_string(root.join("knowledge").join("facts.jsonl")).unwrap();
        let fact: Value = serde_json::from_str(written.trim()).unwrap();
        assert_eq!(fact["subject"], "vibeos-ui");
        assert_eq!(fact["fact"], "utilise pnpm");
        assert_eq!(fact["confidence"], 0.9);
        assert_eq!(fact["ts"], "2026-07-08T01:01:01Z");
        assert!(fact["id"]
            .as_str()
            .unwrap()
            .starts_with(&T_2026_07_08.to_string()));
        // out-of-range confidence is refused
        let err = memory_append_at(
            &root,
            &json!({"scope": "knowledge",
                    "entry": {"subject": "s", "fact": "f", "source": "x", "confidence": 2.0}}),
            T_2026_07_08,
        )
        .unwrap_err();
        assert!(err.contains("between 0 and 1"), "unexpected error: {err}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn memory_append_rejects_readonly_and_unknown_scopes() {
        let root = memory_scratch("ascope");
        // identity/hardware are written by Genesis only — never via memory.append.
        for scope in ["identity", "hardware"] {
            let err = memory_append_at(
                &root,
                &json!({"scope": scope, "entry": {"source": "x"}}),
                T_2026_07_08,
            )
            .unwrap_err();
            assert!(
                err.contains("unknown scope"),
                "scope '{scope}' must be rejected (Genesis-only): {err}"
            );
        }
        let err = memory_append_at(
            &root,
            &json!({"scope": "bogus", "entry": {"source": "x"}}),
            T_2026_07_08,
        )
        .unwrap_err();
        assert!(err.contains("unknown scope"), "unexpected error: {err}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn memory_append_user_writes_append_only_kv_update() {
        let root = memory_scratch("auser");
        let payload = parse_result(memory_append_at(
            &root,
            &json!({"scope": "user",
                    "entry": {"key": "preferences.editor", "value": "neovim",
                              "source": "claude-code"}}),
            T_2026_07_08,
        ));
        assert_eq!(payload["appended"], true);
        assert_eq!(payload["file"], "user/updates.jsonl");
        let written = std::fs::read_to_string(root.join("user").join("updates.jsonl")).unwrap();
        let rec: Value = serde_json::from_str(written.trim()).unwrap();
        assert_eq!(rec["key"], "preferences.editor");
        assert_eq!(rec["value"], "neovim");
        assert_eq!(rec["source"], "claude-code");
        assert_eq!(rec["ts"], "2026-07-08T01:01:01Z");

        // A second append is additive (two lines), never a rewrite (invariant §4).
        memory_append_at(
            &root,
            &json!({"scope": "user",
                    "entry": {"key": "preferences.editor", "value": "helix", "source": "x"}}),
            T_2026_07_08,
        )
        .expect("second user append");
        let lines = std::fs::read_to_string(root.join("user").join("updates.jsonl")).unwrap();
        assert_eq!(lines.lines().count(), 2, "append-only: both updates kept");

        // Invalid key charset and missing value are rejected.
        assert!(memory_append_at(
            &root,
            &json!({"scope": "user", "entry": {"key": "bad key!", "value": 1, "source": "x"}}),
            T_2026_07_08,
        )
        .unwrap_err()
        .contains("dotted config key"));
        assert!(memory_append_at(
            &root,
            &json!({"scope": "user", "entry": {"key": "a.b", "source": "x"}}),
            T_2026_07_08,
        )
        .unwrap_err()
        .contains("value"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn memory_append_projects_writes_whitelisted_record() {
        let root = memory_scratch("aproj");
        let payload = parse_result(memory_append_at(
            &root,
            &json!({"scope": "projects",
                    "entry": {"path": "/var/home/dev/vibeos-ui", "name": "vibeos-ui",
                              "languages": ["ts", "rust"], "vcs": "git",
                              "summary": "front du HUD", "source": "claude-code",
                              "ignored_field": "dropped"}}),
            T_2026_07_08,
        ));
        assert_eq!(payload["file"], "projects/updates.jsonl");
        let written = std::fs::read_to_string(root.join("projects").join("updates.jsonl")).unwrap();
        let rec: Value = serde_json::from_str(written.trim()).unwrap();
        assert_eq!(rec["path"], "/var/home/dev/vibeos-ui");
        assert_eq!(rec["name"], "vibeos-ui");
        assert_eq!(rec["vcs"], "git");
        assert_eq!(rec["ts"], "2026-07-08T01:01:01Z");
        assert!(
            rec.get("ignored_field").is_none(),
            "unknown fields are dropped"
        );

        // A relative path (bad fold key) is refused.
        assert!(memory_append_at(
            &root,
            &json!({"scope": "projects", "entry": {"path": "rel/path", "source": "x"}}),
            T_2026_07_08,
        )
        .unwrap_err()
        .contains("absolute path"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn journal_tool_call_writes_reserved_system_event() {
        let root = memory_scratch("jtc");
        journal_tool_call_at(
            &root,
            T_2026_07_08,
            "fs.write",
            Some("/var/home/dev/notes.md"),
            "T1",
            Caller {
                uid: Some(1000),
                gid: None,
                pid: None,
            },
        )
        .expect("tool_call journal write");
        let written =
            std::fs::read_to_string(root.join("journal").join("2026-07-08.jsonl")).unwrap();
        let ev: Value = serde_json::from_str(written.trim()).unwrap();
        assert_eq!(ev["type"], "tool_call", "reserved system type");
        assert_eq!(ev["source"], "vibed");
        assert_eq!(ev["data"]["tool"], "fs.write");
        assert_eq!(ev["data"]["target"], "/var/home/dev/notes.md");
        assert_eq!(ev["data"]["tier"], "T1");
        assert_eq!(ev["data"]["caller_uid"], 1000);
        assert_eq!(ev["ts"], "2026-07-08T01:01:01Z");

        // An agent can NEVER forge this reserved type via memory.append.
        let err = memory_append_at(
            &root,
            &json!({"scope": "journal",
                    "entry": {"type": "tool_call", "source": "x", "data": {}}}),
            T_2026_07_08,
        )
        .unwrap_err();
        assert!(err.contains("reserved"), "unexpected error: {err}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn journal_tool_call_is_a_noop_without_a_store() {
        // Best-effort: no store (Genesis not run) => Ok, nothing created.
        let root = std::env::temp_dir().join(format!("vibed-jtc-none-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        journal_tool_call_at(
            &root,
            T_2026_07_08,
            "fs.write",
            None,
            "T1",
            Caller::default(),
        )
        .expect("no-op without store");
        assert!(!root.exists(), "must never create the store root");
    }

    #[test]
    fn memory_append_fails_closed_on_uninitialized_store() {
        let root = std::env::temp_dir().join(format!("vibed-mem-noinit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let err = memory_append_at(
            &root,
            &json!({"scope": "journal",
                    "entry": {"type": "observation", "source": "x", "data": {}}}),
            T_2026_07_08,
        )
        .unwrap_err();
        assert!(
            err.contains("memory store absent"),
            "unexpected error: {err}"
        );
        assert!(
            !root.exists(),
            "the store root must never be created by memory.append"
        );
    }
}
