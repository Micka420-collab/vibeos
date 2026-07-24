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

/// Case-insensitive (ASCII) substring test WITHOUT allocating a lowercased
/// copy of the haystack. `needle` is already lowercased by the caller; matching
/// folds ASCII case on the fly, so it differs from a full Unicode
/// `to_lowercase().contains()` ONLY on non-ASCII case pairs (Turkish İ, German
/// ß, …) — acceptable for a memory search, and it removes a per-file duplicate
/// of up to MAX_MEMORY_SCAN_BYTES (64 KiB) that memory.query used to allocate
/// for EVERY scanned file, on a path the HUD polls.
fn contains_ascii_ci(haystack: &str, needle: &str) -> bool {
    let (h, n) = (haystack.as_bytes(), needle.as_bytes());
    if n.is_empty() {
        return true;
    }
    if n.len() > h.len() {
        return false;
    }
    h.windows(n.len()).any(|w| w.eq_ignore_ascii_case(n))
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
    // `query` is optional (absent/null lists the whole scope), but a PRESENT
    // value must be a string — mirror the `scope` validation below. Silently
    // coercing a non-string to "" turned a mistyped `{query: 123}` into a
    // "return everything" instead of a search or an error (broader disclosure
    // than intended, and a wrong result the caller could not detect).
    let query = match args.get("query") {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.to_lowercase(),
        Some(_) => return Err("memory.query: 'query' must be a string".to_string()),
    };

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

    // `fold`: for the `user`/`projects` scopes, return the CONSOLIDATED current
    // view — the last-write-wins fold of the append-only `updates.jsonl` — rather
    // than the raw file walk. This is the SAME fold `vibectl memory profile /
    // projects` exposes to the operator (docs/MEMORY.md §3.3/§3.4), surfaced here
    // so an AGENT can read the current profile/index in ONE T0 call instead of
    // re-folding the raw log itself. Additive: default false; only user/projects.
    let fold = match args.get("fold") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err("memory.query: 'fold' must be a boolean".to_string()),
    };
    if fold {
        match scope.map(|(name, _, _)| name) {
            Some("user") => {
                let mut out = crate::vibectl::memory_profile_at(root);
                out["scope"] = json!("user");
                out["folded"] = json!(true);
                return Ok(out.to_string());
            }
            Some("projects") => {
                let mut out = crate::vibectl::memory_projects_at(root);
                out["scope"] = json!("projects");
                out["folded"] = json!(true);
                return Ok(out.to_string());
            }
            // Consolidated knowledge: facts.jsonl deduped by (subject, fact),
            // last-write-wins (ADR-030). Confidence is NEVER raised by the fold
            // (THREAT-MODEL §9: source/fact/confidence are agent-declared).
            Some("knowledge") => {
                let mut out = crate::vibectl::memory_knowledge_at(root);
                out["scope"] = json!("knowledge");
                out["folded"] = json!(true);
                return Ok(out.to_string());
            }
            _ => {
                return Err("memory.query: 'fold' applies only to scope 'user', \
                            'projects' or 'knowledge'"
                    .to_string())
            }
        }
    }

    // `rank`: additive, opt-in (same precedent as `fold`). For `journal` /
    // `knowledge`, return EVENTS ranked by the recall score (recency +
    // importance + relevance — recall.rs / ADR-030) instead of files in
    // filesystem-walk order, so `limit` keeps the MOST relevant rather than the
    // first encountered (the default order is filesystem-dependent). Default
    // false: the classic per-file path below is byte-for-byte unchanged.
    let rank = match args.get("rank") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err("memory.query: 'rank' must be a boolean".to_string()),
    };

    // Bounded, symlink-safe file collection — shared verbatim by the default
    // per-file match below and the ranked-event path.
    let files = collect_scope_files(root, scope);

    if rank {
        return match scope.map(|(name, _, _)| name) {
            Some("journal") | Some("knowledge") => {
                // `now` is the only impurity; the ranking itself is pure and
                // injectable (see memory_query_ranked's tests).
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                memory_query_ranked(root, scope.map(|(n, _, _)| n), &files, &query, limit, now)
            }
            _ => Err(
                "memory.query: 'rank' applies only to scope 'journal' or 'knowledge'".to_string(),
            ),
        };
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
        let name_hit = !query.is_empty() && contains_ascii_ci(&relative, &query);
        let content_hit = !query.is_empty()
            && scanned
                .as_ref()
                .is_some_and(|(text, _)| contains_ascii_ci(text, &query));
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

/// Bounded, symlink-safe collection of the files under a scope (or the whole
/// store). Iterative (no recursion), hard-capped at `MAX_MEMORY_FILES`, and
/// NEVER follows symlinks (`symlink_metadata` / `entry.file_type()` do not
/// traverse) — a link planted inside the store cannot route the walk outside
/// `/var/lib/vibeos/memory`, e.g. into the audit trail. Extracted verbatim from
/// `memory.query` so the default and ranked paths share ONE audited walk.
fn collect_scope_files(
    root: &std::path::Path,
    scope: Option<(&str, &str, bool)>,
) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    let mut stack: Vec<std::path::PathBuf> = Vec::new();
    // A walk root is only pushed if it is a REAL directory (symlink_metadata
    // does not follow): if a scope dir (or the store root) is a symlink, the
    // walk must not descend through it out of the store.
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
    files
}

/// Demi-vie de récence du classement (une semaine) : un événement d'il y a 7
/// jours pèse moitié moins qu'aujourd'hui sur l'axe récence.
const RANK_HALF_LIFE_SECS: u64 = 7 * 86_400;
/// Borne dure du nombre d'événements classés en un appel (anti-DoS mémoire, en
/// plus de la borne d'octets par fichier de `read_memory_scan`).
const MAX_RANK_EVENTS: usize = 5_000;

/// Métadonnées d'un événement classé, pour reconstruire la réponse après le tri
/// (le classeur pur `recall` ne connaît que `id`/`ts`/`importance`/`text`).
struct RankMeta {
    file: String,
    line: usize,
    ts: String,
    kind: String,
    snippet: String,
    snippet_truncated: bool,
}

/// Mode `rank` de `memory.query` (opt-in, journal/knowledge) : classe les
/// ÉVÉNEMENTS par le score de rappel (`recall::top_k_lexical`) au lieu de rendre
/// des fichiers dans l'ordre du walk. `now_epoch` est injecté (déterminisme des
/// tests). Fail-closed : une ligne malformée ou non-objet est ignorée, jamais un
/// panic ; bornes `MAX_MEMORY_FILES` (walk), `MAX_MEMORY_SCAN_BYTES` (octets/
/// fichier) et `MAX_RANK_EVENTS` (nombre d'événements) respectées.
fn memory_query_ranked(
    root: &std::path::Path,
    scope_name: Option<&str>,
    files: &[std::path::PathBuf],
    query: &str,
    limit: usize,
    now_epoch: u64,
) -> Result<String, String> {
    use crate::tools::recall;

    let mut items: Vec<recall::MemoryItem> = Vec::new();
    let mut metas: std::collections::BTreeMap<String, RankMeta> = std::collections::BTreeMap::new();
    let mut capped = false;
    // Did any file exceed MAX_MEMORY_SCAN_BYTES, so its tail was NOT ingested?
    // On an append-only store (knowledge/facts.jsonl, busy journal day-files)
    // that tail holds the NEWEST events — exactly what a recency-weighted rank
    // should surface. Dropping them silently while reporting `truncated:false`
    // told the caller it had the complete, correctly-ranked view when it did
    // not. Track it and fold it into `truncated` (and surface it explicitly).
    let mut byte_capped = false;

    'outer: for path in files {
        let relative = path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string();
        let Some((text, longer)) = read_memory_scan(path) else {
            continue;
        };
        if longer {
            byte_capped = true;
        }
        for (line_no, raw) in text.lines().enumerate() {
            if items.len() >= MAX_RANK_EVENTS {
                capped = true;
                break 'outer;
            }
            let line = raw.trim();
            if line.is_empty() {
                continue;
            }
            // Fail-closed: only well-formed JSON OBJECTS are events; anything
            // else (a corrupt line, a bare scalar) is skipped, never fatal.
            let Ok(ev) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            if !ev.is_object() {
                continue;
            }
            let ts = ev.get("ts").and_then(Value::as_str).unwrap_or("");
            // Day-granular recency from the event's own ts; a missing/invalid ts
            // ⇒ epoch 0 (treated as very old), never a crash.
            let ts_unix = recall::epoch_day_from_iso(ts).unwrap_or(0);
            // Importance from the SYSTEM-derived type (journal) or the stored
            // confidence (knowledge) — NEVER from an agent-declared field beyond
            // those (THREAT-MODEL §1: source/data are untrusted).
            let (importance, kind) = match scope_name {
                Some("knowledge") => {
                    let c = ev
                        .get("confidence")
                        .and_then(Value::as_f64)
                        .map(|c| c as f32)
                        .unwrap_or(0.5)
                        .clamp(0.0, 1.0);
                    (c, "fact".to_string())
                }
                _ => {
                    let t = ev.get("type").and_then(Value::as_str).unwrap_or("");
                    (recall::importance_of_type(t), t.to_string())
                }
            };
            let snippet: String = line.chars().take(MEMORY_SNIPPET_CHARS).collect();
            let snippet_truncated = snippet.chars().count() < line.chars().count();
            let id = format!("{relative}#{line_no}");
            metas.insert(
                id.clone(),
                RankMeta {
                    file: relative.clone(),
                    line: line_no,
                    ts: ts.to_string(),
                    kind,
                    snippet,
                    snippet_truncated,
                },
            );
            items.push(recall::MemoryItem {
                id,
                ts_unix,
                importance,
                // Raw line is the relevance text: the tokenizer strips JSON
                // punctuation, and field names are constant across events so
                // they never skew the ranking.
                text: line.to_string(),
            });
        }
    }

    let weights = recall::RecallWeights::default();
    let hits = recall::top_k_lexical(
        query,
        &items,
        now_epoch,
        RANK_HALF_LIFE_SECS,
        &weights,
        limit,
    );
    let matches: Vec<Value> = hits
        .iter()
        .filter_map(|(item, score)| {
            metas.get(&item.id).map(|m| {
                json!({
                    "file": m.file,
                    "line": m.line,
                    "ts": m.ts,
                    "type": m.kind,
                    // 3 decimals: enough to see the ranking, not float noise.
                    "score": (f64::from(*score) * 1000.0).round() / 1000.0,
                    "snippet": m.snippet,
                    "snippet_truncated": m.snippet_truncated,
                })
            })
        })
        .collect();
    let truncated = capped || byte_capped || items.len() > matches.len();

    Ok(json!({
        "initialized": true,
        "query": query,
        "scope": scope_name,
        "ranked": true,
        "scanned_files": files.len(),
        "scanned_events": items.len(),
        // Honest signal that at least one file was cut at MAX_MEMORY_SCAN_BYTES,
        // so its newest events were not ranked — distinct from the event-count
        // cap (`capped`) folded into `truncated`.
        "byte_capped": byte_capped,
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
        std::fs::write(
            dir.join("personality.toml"),
            "schema = 1\nname = \"Lumen\"\narchetype = \"sentinelle\"\n\n[traits]\ncaution = 71\n",
        )
        .expect("write personality");
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

    // -- memory.query rank mode (ADR-030, recall.rs) --------------------------

    /// A fixed "now" for deterministic recency: midday 2026-07-22 UTC.
    fn fixed_now() -> u64 {
        crate::tools::recall::epoch_day_from_iso("2026-07-22T12:00:00Z").unwrap()
    }

    /// Run the ranked path deterministically (injected `now`), via the SAME
    /// bounded walk the live tool uses.
    fn ranked(root: &std::path::Path, scope_name: &str, query: &str, limit: usize) -> Value {
        let scope = MEMORY_SCOPES
            .iter()
            .find(|(n, _, _)| *n == scope_name)
            .copied();
        let files = collect_scope_files(root, scope);
        parse_result(memory_query_ranked(
            root,
            Some(scope_name),
            &files,
            query,
            limit,
            fixed_now(),
        ))
    }

    fn write_journal_day(root: &std::path::Path, date: &str, lines: &[&str]) {
        let mut body = lines.join("\n");
        body.push('\n');
        std::fs::write(root.join("journal").join(format!("{date}.jsonl")), body)
            .expect("write journal day");
    }

    #[test]
    fn rank_orders_by_relevance_within_same_recency_and_type() {
        let root = memory_scratch("rankrel");
        // Drop the scratch's genesis line so exactly the two crafted events rank.
        let _ = std::fs::remove_file(root.join("journal").join("2026-01-01.jsonl"));
        // Two observations, SAME day and SAME type -> only relevance separates them.
        write_journal_day(
            &root,
            "2026-07-20",
            &[
                r#"{"ts":"2026-07-20T10:00:00Z","type":"observation","source":"a","data":{"note":"le projet utilise pnpm"}}"#,
                r#"{"ts":"2026-07-20T11:00:00Z","type":"observation","source":"a","data":{"note":"une recette de cuisine"}}"#,
            ],
        );
        let payload = ranked(&root, "journal", "pnpm", 10);
        let matches = payload["matches"].as_array().unwrap();
        assert!(payload["ranked"].as_bool().unwrap());
        assert!(
            matches[0]["snippet"].as_str().unwrap().contains("pnpm"),
            "the query-relevant event ranks first: {payload}"
        );
        // rank is a RANKING, not a filter: the non-matching event is still
        // returned (just lower) — a silent regression to substring-filtering
        // would drop it and this would catch it.
        assert_eq!(
            matches.len(),
            2,
            "all events are ranked and returned, not filtered to query matches: {payload}"
        );
        assert!(
            matches[1]["snippet"].as_str().unwrap().contains("cuisine"),
            "the non-matching event survives, ranked last: {payload}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn rank_flags_byte_capped_files_as_truncated() {
        let root = memory_scratch("rankbytecap");
        let _ = std::fs::remove_file(root.join("journal").join("2026-01-01.jsonl"));
        // Build one journal day-file LARGER than MAX_MEMORY_SCAN_BYTES: its tail
        // (the NEWEST events) will not be ingested by the bounded read. Rank must
        // then report that it did NOT see everything, not claim completeness.
        let mut body = String::new();
        let mut i = 0;
        while body.len() <= MAX_MEMORY_SCAN_BYTES + 4096 {
            body.push_str(&format!(
                "{{\"ts\":\"2026-07-20T10:00:00Z\",\"type\":\"observation\",\"source\":\"a\",\
                 \"data\":{{\"note\":\"event {i} pnpm padding padding padding padding\"}}}}\n"
            ));
            i += 1;
        }
        assert!(
            body.len() > MAX_MEMORY_SCAN_BYTES,
            "fixture must exceed the per-file byte cap"
        );
        std::fs::write(root.join("journal").join("2026-07-20.jsonl"), &body).unwrap();

        let payload = ranked(&root, "journal", "pnpm", 5000);
        assert_eq!(
            payload["byte_capped"], true,
            "a file cut at MAX_MEMORY_SCAN_BYTES is flagged: {payload}"
        );
        assert_eq!(
            payload["truncated"], true,
            "byte-capped rank must NOT report truncated:false: {payload}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn query_must_be_a_string_when_present() {
        let root = memory_scratch("qtype");
        // A present, non-string query is rejected — not silently coerced to ""
        // (which would return the whole store instead of searching or erroring).
        let err = memory_query_at(&root, &json!({"query": 123})).unwrap_err();
        assert!(err.contains("'query' must be a string"), "{err}");
        let err2 = memory_query_at(&root, &json!({"query": {"x": 1}})).unwrap_err();
        assert!(err2.contains("must be a string"), "{err2}");
        // Absent or null query is unchanged (lists the scope, no filter).
        assert!(memory_query_at(&root, &json!({"scope": "journal"})).is_ok());
        assert!(memory_query_at(&root, &json!({"scope": "journal", "query": null})).is_ok());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn rank_respects_limit_and_flags_truncation() {
        let root = memory_scratch("ranklimit");
        let _ = std::fs::remove_file(root.join("journal").join("2026-01-01.jsonl"));
        // Three events, all scanned; ask for the top 2.
        write_journal_day(
            &root,
            "2026-07-20",
            &[
                r#"{"ts":"2026-07-20T10:00:00Z","type":"observation","source":"a","data":{"note":"un"}}"#,
                r#"{"ts":"2026-07-20T11:00:00Z","type":"decision","source":"a","data":{"note":"deux"}}"#,
                r#"{"ts":"2026-07-20T12:00:00Z","type":"preference","source":"a","data":{"note":"trois"}}"#,
            ],
        );
        let payload = ranked(&root, "journal", "", 2);
        assert_eq!(
            payload["scanned_events"].as_u64().unwrap(),
            3,
            "all 3 scanned"
        );
        assert_eq!(
            payload["matches"].as_array().unwrap().len(),
            2,
            "limit keeps the top 2: {payload}"
        );
        assert!(
            payload["truncated"].as_bool().unwrap(),
            "more events than returned -> truncated: {payload}"
        );
        // limit >= events -> not truncated.
        let full = ranked(&root, "journal", "", 10);
        assert!(
            !full["truncated"].as_bool().unwrap(),
            "nothing dropped: {full}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn rank_keeps_event_with_missing_or_invalid_ts() {
        let root = memory_scratch("rankts");
        let _ = std::fs::remove_file(root.join("journal").join("2026-01-01.jsonl"));
        // One event with NO ts, one recent. The ts-less event maps to epoch 0
        // (very old) but must STILL appear in the ranking, just ranked lower —
        // never silently dropped.
        write_journal_day(
            &root,
            "2026-07-21",
            &[
                r#"{"type":"observation","source":"a","data":{"note":"sans horodatage"}}"#,
                r#"{"ts":"2026-07-21T10:00:00Z","type":"observation","source":"a","data":{"note":"recent"}}"#,
            ],
        );
        let payload = ranked(&root, "journal", "", 10);
        assert_eq!(
            payload["scanned_events"].as_u64().unwrap(),
            2,
            "the ts-less event is kept, not dropped: {payload}"
        );
        let matches = payload["matches"].as_array().unwrap();
        // The recent event ranks above the epoch-0 one.
        assert!(matches[0]["snippet"].as_str().unwrap().contains("recent"));
        assert!(
            matches
                .iter()
                .any(|m| m["snippet"].as_str().unwrap().contains("sans horodatage")),
            "the ts-less event is present in the ranking: {payload}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn rank_orders_by_recency_for_empty_query() {
        let root = memory_scratch("rankrec");
        write_journal_day(
            &root,
            "2026-07-21",
            &[
                r#"{"ts":"2026-07-21T10:00:00Z","type":"observation","source":"a","data":{"note":"tout frais"}}"#,
            ],
        );
        write_journal_day(
            &root,
            "2026-07-02",
            &[
                r#"{"ts":"2026-07-02T10:00:00Z","type":"observation","source":"a","data":{"note":"deja vieux"}}"#,
            ],
        );
        // Empty query -> relevance 0 for all -> recency (same type) decides.
        let payload = ranked(&root, "journal", "", 10);
        let matches = payload["matches"].as_array().unwrap();
        assert_eq!(
            matches[0]["ts"].as_str().unwrap(),
            "2026-07-21T10:00:00Z",
            "the most recent event ranks first: {payload}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn rank_importance_prefers_decision_over_observation_same_day() {
        let root = memory_scratch("rankimp");
        write_journal_day(
            &root,
            "2026-07-20",
            &[
                r#"{"ts":"2026-07-20T10:00:00Z","type":"observation","source":"a","data":{"note":"x"}}"#,
                r#"{"ts":"2026-07-20T10:00:00Z","type":"decision","source":"a","data":{"note":"y"}}"#,
            ],
        );
        // Empty query, same day -> importance (decision > observation) decides.
        let payload = ranked(&root, "journal", "", 10);
        assert_eq!(
            payload["matches"][0]["type"].as_str().unwrap(),
            "decision",
            "the more salient type ranks first: {payload}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn rank_knowledge_uses_confidence_for_importance() {
        let root = memory_scratch("rankknow");
        std::fs::write(
            root.join("knowledge").join("facts.jsonl"),
            format!(
                "{}\n{}\n",
                r#"{"id":"f1","ts":"2026-07-20T10:00:00Z","subject":"lowsubj","fact":"peu sûr","confidence":0.1,"source":"a"}"#,
                r#"{"id":"f2","ts":"2026-07-20T10:00:00Z","subject":"highsubj","fact":"très sûr","confidence":0.95,"source":"a"}"#,
            ),
        )
        .expect("write facts");
        // Empty query, same ts -> confidence decides (0.95 over 0.1).
        let payload = ranked(&root, "knowledge", "", 10);
        assert_eq!(payload["matches"][0]["type"].as_str().unwrap(), "fact");
        assert!(
            payload["matches"][0]["snippet"]
                .as_str()
                .unwrap()
                .contains("highsubj"),
            "the higher-confidence fact ranks first: {payload}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn rank_skips_malformed_and_nonobject_lines() {
        let root = memory_scratch("rankskip");
        // Remove the scratch's genesis line so the count is exact.
        let _ = std::fs::remove_file(root.join("journal").join("2026-01-01.jsonl"));
        write_journal_day(
            &root,
            "2026-07-20",
            &[
                r#"{"ts":"2026-07-20T10:00:00Z","type":"observation","source":"a","data":{"note":"real"}}"#,
                "not json at all",
                "123",
                "",
            ],
        );
        let payload = ranked(&root, "journal", "", 10);
        assert_eq!(
            payload["scanned_events"].as_u64().unwrap(),
            1,
            "only the well-formed object is an event: {payload}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn fold_knowledge_returns_consolidated_view() {
        let root = memory_scratch("foldknow");
        std::fs::write(
            root.join("knowledge").join("facts.jsonl"),
            format!(
                "{}\n{}\n",
                r#"{"id":"1","ts":"2026-07-01T00:00:00Z","subject":"s","fact":"f","confidence":0.4,"source":"a"}"#,
                r#"{"id":"2","ts":"2026-07-09T00:00:00Z","subject":"s","fact":"f","confidence":0.7,"source":"b"}"#,
            ),
        )
        .expect("write facts");
        let payload = parse_result(memory_query_at(
            &root,
            &json!({"scope": "knowledge", "fold": true}),
        ));
        assert_eq!(payload["folded"], json!(true));
        assert_eq!(payload["scope"], "knowledge");
        assert_eq!(payload["count"], 1, "the two re-assertions fold into one");
        assert_eq!(
            payload["knowledge"][0]["confidence"], 0.7,
            "last write wins"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn rank_arg_plumbing_and_scope_guard() {
        let root = memory_scratch("rankarg");
        // rank:true on journal returns the ranked shape.
        let payload = parse_result(memory_query_at(
            &root,
            &json!({"scope": "journal", "rank": true}),
        ));
        assert!(payload["ranked"].as_bool().unwrap_or(false));
        // rank only applies to journal/knowledge.
        assert!(
            memory_query_at(&root, &json!({"scope": "identity", "rank": true})).is_err(),
            "rank rejects non journal/knowledge scopes"
        );
        assert!(
            memory_query_at(&root, &json!({"rank": true})).is_err(),
            "rank with no scope is rejected"
        );
        // rank must be a boolean.
        assert!(
            memory_query_at(&root, &json!({"scope": "journal", "rank": "yes"})).is_err(),
            "non-boolean rank is rejected"
        );
        // Default path is untouched: no `ranked` flag without rank:true.
        let classic = parse_result(memory_query_at(&root, &json!({"scope": "journal"})));
        assert!(classic.get("ranked").is_none(), "default path is unchanged");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn contains_ascii_ci_matches_case_insensitively_without_allocating() {
        assert!(contains_ascii_ci("Hello WORLD", "world"));
        assert!(contains_ascii_ci("preferences.EDITOR = neovim", "editor"));
        assert!(contains_ascii_ci("abc", "abc"));
        assert!(contains_ascii_ci("anything", ""), "empty needle matches");
        assert!(!contains_ascii_ci("short", "longer-than-haystack"));
        assert!(!contains_ascii_ci("hello", "xyz"));
        // Fold is ASCII-only by design: the needle arrives already lowercased,
        // and matching is symmetric via eq_ignore_ascii_case.
        assert!(contains_ascii_ci("MixedCASE", "mixedcase"));
    }

    #[test]
    fn memory_query_content_match_is_case_insensitive() {
        let root = memory_scratch("qcasefold");
        // The journal genesis line contains lowercase text; query in UPPERCASE
        // must still hit it (ASCII case fold, no allocation).
        let payload = parse_result(memory_query_at(&root, &json!({"query": "GENESIS"})));
        assert!(
            payload["matches"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m["file"]
                    .as_str()
                    .is_some_and(|f| f.starts_with("journal/"))),
            "an uppercase query matches lowercase content: {payload}"
        );
        let _ = std::fs::remove_dir_all(&root);
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
    fn memory_query_personality_scope_resolves_the_character() {
        // The AI citizen's birth character (personality.toml, written by
        // Genesis §3.8) is a first-class, addressable, read-only scope — so the
        // HUD/agent can ask "who are you?" without walking the whole store.
        let root = memory_scratch("qpersona");
        let payload = parse_result(memory_query_at(&root, &json!({"scope": "personality"})));
        assert_eq!(payload["scope"], "personality");
        assert_eq!(payload["scanned_files"], 1);
        assert_eq!(payload["matches"][0]["file"], "personality.toml");
        assert!(
            payload["matches"][0]["snippet"]
                .as_str()
                .is_some_and(|s| s.contains("archetype")),
            "personality snippet carries the character: {payload}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn memory_query_fold_returns_consolidated_user_and_projects_views() {
        let root = memory_scratch("qfold");
        // Append-only user updates: the SAME key written twice — last wins.
        std::fs::write(
            root.join("user").join("updates.jsonl"),
            "{\"ts\":\"2026-01-01T00:00:00Z\",\"key\":\"preferences.editor\",\"value\":\"vim\",\"source\":\"agent\"}\n\
             {\"ts\":\"2026-01-02T00:00:00Z\",\"key\":\"preferences.editor\",\"value\":\"zed\",\"source\":\"agent\"}\n\
             {\"ts\":\"2026-01-02T00:00:00Z\",\"key\":\"profile.lang\",\"value\":\"fr\",\"source\":\"agent\"}\n",
        )
        .unwrap();
        let payload = parse_result(memory_query_at(
            &root,
            &json!({"scope": "user", "fold": true}),
        ));
        assert_eq!(payload["folded"], true);
        assert_eq!(payload["scope"], "user");
        assert_eq!(
            payload["profile"]["preferences.editor"], "zed",
            "last write wins per key"
        );
        assert_eq!(payload["profile"]["profile.lang"], "fr");

        // projects fold: last-write-wins per path.
        std::fs::write(
            root.join("projects").join("updates.jsonl"),
            "{\"ts\":\"2026-01-01T00:00:00Z\",\"path\":\"/home/dev/proj\",\"name\":\"old\",\"source\":\"agent\"}\n\
             {\"ts\":\"2026-01-02T00:00:00Z\",\"path\":\"/home/dev/proj\",\"name\":\"new\",\"source\":\"agent\"}\n",
        )
        .unwrap();
        let payload = parse_result(memory_query_at(
            &root,
            &json!({"scope": "projects", "fold": true}),
        ));
        assert_eq!(payload["folded"], true);
        let projects = payload["projects"].as_array().unwrap();
        assert_eq!(projects.len(), 1, "folded by path");
        assert_eq!(projects[0]["name"], "new", "last write wins per path");

        // fold on a non-user/projects scope is an error.
        assert!(memory_query_at(&root, &json!({"scope": "journal", "fold": true})).is_err());
        assert!(memory_query_at(&root, &json!({"fold": true})).is_err());
        // fold=false (the default) keeps the raw walk behavior unchanged.
        let payload = parse_result(memory_query_at(&root, &json!({"scope": "user"})));
        assert!(
            payload.get("folded").is_none(),
            "an un-folded query keeps the raw file-walk shape"
        );
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
        // identity/hardware/personality are written by Genesis only — never via memory.append.
        for scope in ["identity", "hardware", "personality"] {
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
