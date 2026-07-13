//! `vibectl` core — the operator/admin CLI logic, kept in the library so it is
//! unit-testable without spawning the binary. The thin front-end lives in
//! `src/bin/vibectl.rs`.
//!
//! v0.1 perimeter (ROADMAP Phase 3 "Ébauche de vibectl"): READ-ONLY memory
//! status and audit-chain verification. Destructive actions (factory reset =
//! T3) are deliberately NOT here yet — they require the human-approval flow
//! (Phase 4) and must never be a bare CLI switch.

use std::path::Path;

use serde_json::{json, Value};

use crate::{approval, audit};

/// Current effective uid, parsed from `/proc/self/status` (no libc), for the
/// `granted_by` field of an approval. `None` if it cannot be read.
fn current_euid() -> Option<u32> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("Uid:") {
            // Uid: <real> <effective> <saved> <fs>
            let mut fields = rest.split_whitespace();
            let _real = fields.next();
            return fields.next().or(_real).and_then(|s| s.parse().ok());
        }
    }
    None
}

fn now_epoch_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// `vibectl approvals list` — pending human-approval requests.
pub fn approvals_list() -> Value {
    json!({ "pending": approval::list_pending(Path::new(approval::APPROVAL_DIR)) })
}

/// `vibectl approve <id>` — grant a pending request (operator action; the store
/// is root-only, so the OS permissions restrict this to root). Returns
/// `(report, ok)`.
pub fn approve(id: &str) -> (Value, bool) {
    match approval::approve(
        Path::new(approval::APPROVAL_DIR),
        id,
        current_euid(),
        now_epoch_secs(),
    ) {
        Ok(grant) => (json!({"approved": id, "grant": grant}), true),
        Err(e) => (json!({"error": e.to_string(), "id": id}), false),
    }
}

/// `vibectl deny <id>` — reject and remove a pending request.
pub fn deny(id: &str) -> (Value, bool) {
    match approval::deny(Path::new(approval::APPROVAL_DIR), id) {
        Ok(()) => (json!({"denied": id}), true),
        Err(e) => (json!({"error": e.to_string(), "id": id}), false),
    }
}

/// Default runtime marker written by the amnesic generator
/// (`/run/vibeos/memory-mode`): `amnesic` | `persistent`.
pub const MEMORY_MODE_MARKER: &str = "/run/vibeos/memory-mode";

/// Count the non-empty lines of a file, or 0 if it is absent/unreadable.
fn count_lines(path: &Path) -> u64 {
    std::fs::read_to_string(path)
        .map(|c| c.lines().filter(|l| !l.trim().is_empty()).count() as u64)
        .unwrap_or(0)
}

/// Sum the entries across every `journal/*.jsonl` file.
fn journal_count(root: &Path) -> u64 {
    let dir = root.join("journal");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return 0;
    };
    entries
        .flatten()
        .filter(|e| {
            e.path()
                .extension()
                .is_some_and(|x| x.eq_ignore_ascii_case("jsonl"))
        })
        .map(|e| count_lines(&e.path()))
        .sum()
}

/// Read the current memory mode: prefer the runtime marker (set by the amnesic
/// generator), else fall back to the `mode` recorded in identity.toml, else
/// "unknown". `marker_path` is a parameter so tests need not touch `/run`.
fn read_mode(identity: Option<&Value>, marker_path: &Path) -> String {
    if let Ok(marker) = std::fs::read_to_string(marker_path) {
        let m = marker.trim();
        if !m.is_empty() {
            return m.to_string();
        }
    }
    identity
        .and_then(|v| v.get("mode"))
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string()
}

/// Parse `identity.toml` into a JSON object (best-effort; None if absent/invalid).
fn read_identity(root: &Path) -> Option<Value> {
    let content = std::fs::read_to_string(root.join("identity.toml")).ok()?;
    let parsed: toml::Value = content.parse().ok()?;
    serde_json::to_value(parsed).ok()
}

/// Build the `vibectl memory status` report for a store rooted at `root`, using
/// `marker_path` for the amnesic-mode marker (injectable for tests).
pub fn memory_status_at(root: &Path, marker_path: &Path) -> Value {
    if !root.is_dir() {
        return json!({
            "initialized": false,
            "note": "no memory store: Genesis has not run (or amnesic tmpfs is unmounted)"
        });
    }
    let initialized = root.join(".initialized").exists();
    let identity = read_identity(root);
    let mode = read_mode(identity.as_ref(), marker_path);

    // Hardware summary (schema 2): surface the structured fields, not the raw blobs.
    let hardware = std::fs::read_to_string(root.join("hardware.json"))
        .ok()
        .and_then(|c| serde_json::from_str::<Value>(&c).ok())
        .map(|hw| {
            json!({
                "schema": hw.get("schema"),
                "cpu": hw.get("cpu"),
                "memory": hw.get("memory"),
                "gpu_count": hw.get("gpu").and_then(Value::as_array).map(|a| a.len()),
            })
        });

    json!({
        "initialized": initialized,
        "mode": mode,
        "identity": identity.as_ref().map(|v| json!({
            "hostname": v.get("hostname"),
            "birth": v.get("birth"),
            "schema": v.get("schema"),
        })),
        "hardware": hardware,
        "counts": {
            "journal": journal_count(root),
            "knowledge": count_lines(&root.join("knowledge").join("facts.jsonl")),
            "user": count_lines(&root.join("user").join("updates.jsonl")),
            "projects": count_lines(&root.join("projects").join("updates.jsonl")),
        }
    })
}

/// Read a memory-store JSONL file into parsed records (empty if absent).
fn read_jsonl(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .map(|c| {
            c.lines()
                .filter(|l| !l.trim().is_empty())
                .filter_map(|l| serde_json::from_str::<Value>(l).ok())
                .collect()
        })
        .unwrap_or_default()
}

/// `vibectl memory profile` — the CURRENT user profile, materialized as the
/// fold of the append-only `user/updates.jsonl` (last-write-wins per `key`).
/// This is the read side of the P1 append-only design (docs/MEMORY.md §3.3).
pub fn memory_profile_at(root: &Path) -> Value {
    let mut profile = serde_json::Map::new();
    for rec in read_jsonl(&root.join("user").join("updates.jsonl")) {
        if let Some(key) = rec.get("key").and_then(Value::as_str) {
            // Later lines overwrite earlier ones — append-only, fold on read.
            profile.insert(
                key.to_string(),
                rec.get("value").cloned().unwrap_or(Value::Null),
            );
        }
    }
    json!({ "profile": Value::Object(profile) })
}

/// `vibectl memory projects` — the CURRENT project index, materialized as the
/// fold of `projects/updates.jsonl` (last-write-wins per `path`), sorted by
/// path (docs/MEMORY.md §3.4).
pub fn memory_projects_at(root: &Path) -> Value {
    let mut by_path: std::collections::BTreeMap<String, Value> = std::collections::BTreeMap::new();
    for rec in read_jsonl(&root.join("projects").join("updates.jsonl")) {
        if let Some(path) = rec.get("path").and_then(Value::as_str) {
            by_path.insert(path.to_string(), rec);
        }
    }
    json!({ "projects": by_path.into_values().collect::<Vec<_>>() })
}

/// `vibectl audit verify [dir]` — verify the tamper-evident audit chain across
/// all daily files in `dir`. Returns `(json_report, ok)`; the caller maps `ok`
/// to the process exit code.
pub fn audit_verify(dir: &Path) -> (Value, bool) {
    match audit::verify_chain(dir) {
        Ok(report) => (
            json!({
                "dir": dir.to_string_lossy(),
                "records": report.records,
                "ok": report.ok,
                "broken_at": report.broken_at,
                "reason": report.reason,
            }),
            report.ok,
        ),
        Err(e) => (
            json!({"dir": dir.to_string_lossy(), "ok": false, "error": e.to_string()}),
            false,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("vibed-vibectl-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn memory_status_reports_absent_store() {
        let root = scratch("absent");
        let v = memory_status_at(&root, Path::new("/nonexistent-marker"));
        assert_eq!(v["initialized"], false);
    }

    #[test]
    fn memory_status_summarizes_a_populated_store() {
        let root = scratch("full");
        for sub in ["journal", "knowledge", "user", "projects"] {
            std::fs::create_dir_all(root.join(sub)).unwrap();
        }
        std::fs::write(
            root.join("identity.toml"),
            "schema = 1\nhostname = \"forge\"\nbirth = \"2026-07-13T00:00:00+00:00\"\nmode = \"persistent\"\n",
        )
        .unwrap();
        std::fs::write(
            root.join("hardware.json"),
            r#"{"schema":2,"cpu":{"model":"X","cores":16},"memory":{"total_bytes":1000},"gpu":[{"vendor":"NVIDIA","model":"Y","vram_bytes":8}],"raw":{}}"#,
        )
        .unwrap();
        std::fs::write(root.join("journal").join("2026-07-13.jsonl"), "{}\n{}\n").unwrap();
        std::fs::write(root.join("knowledge").join("facts.jsonl"), "{}\n").unwrap();
        std::fs::write(root.join("user").join("updates.jsonl"), "{}\n{}\n{}\n").unwrap();
        std::fs::write(root.join(".initialized"), "").unwrap();

        // Marker file overrides identity mode.
        let marker = root.join("mode-marker");
        std::fs::write(&marker, "amnesic\n").unwrap();

        let v = memory_status_at(&root, &marker);
        assert_eq!(v["initialized"], true);
        assert_eq!(v["mode"], "amnesic", "marker wins over identity mode");
        assert_eq!(v["identity"]["hostname"], "forge");
        assert_eq!(v["hardware"]["schema"], 2);
        assert_eq!(v["hardware"]["cpu"]["cores"], 16);
        assert_eq!(v["hardware"]["gpu_count"], 1);
        assert_eq!(v["counts"]["journal"], 2);
        assert_eq!(v["counts"]["knowledge"], 1);
        assert_eq!(v["counts"]["user"], 3);
        assert_eq!(v["counts"]["projects"], 0);

        // Without the marker, mode falls back to identity.toml.
        let v = memory_status_at(&root, Path::new("/nonexistent-marker"));
        assert_eq!(v["mode"], "persistent");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn memory_profile_folds_user_updates_last_write_wins() {
        let root = scratch("profile");
        std::fs::create_dir_all(root.join("user")).unwrap();
        // Append-only updates; the later editor value must win.
        let lines = [
            r#"{"ts":"t1","key":"preferences.editor","value":"neovim","source":"x"}"#,
            r#"{"ts":"t2","key":"profile.lang","value":"fr","source":"x"}"#,
            r#"{"ts":"t3","key":"preferences.editor","value":"helix","source":"x"}"#,
        ];
        std::fs::write(
            root.join("user").join("updates.jsonl"),
            lines.join("\n") + "\n",
        )
        .unwrap();
        let v = memory_profile_at(&root);
        assert_eq!(
            v["profile"]["preferences.editor"], "helix",
            "last write wins"
        );
        assert_eq!(v["profile"]["profile.lang"], "fr");
        // Absent store → empty profile, no panic.
        let empty = memory_profile_at(&scratch("profile-empty"));
        assert!(empty["profile"].as_object().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn memory_projects_folds_by_path_sorted() {
        let root = scratch("projects");
        std::fs::create_dir_all(root.join("projects")).unwrap();
        let lines = [
            r#"{"ts":"t1","path":"/home/dev/b","name":"b-old","source":"x"}"#,
            r#"{"ts":"t2","path":"/home/dev/a","name":"a","source":"x"}"#,
            r#"{"ts":"t3","path":"/home/dev/b","name":"b-new","source":"x"}"#,
        ];
        std::fs::write(
            root.join("projects").join("updates.jsonl"),
            lines.join("\n") + "\n",
        )
        .unwrap();
        let v = memory_projects_at(&root);
        let projs = v["projects"].as_array().unwrap();
        assert_eq!(projs.len(), 2, "folded by path");
        assert_eq!(projs[0]["path"], "/home/dev/a", "sorted by path");
        assert_eq!(projs[1]["path"], "/home/dev/b");
        assert_eq!(projs[1]["name"], "b-new", "last write wins per path");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn audit_verify_reports_ok_for_absent_log() {
        let (report, ok) = audit_verify(Path::new("/nonexistent-audit.jsonl"));
        assert!(ok);
        assert_eq!(report["records"], 0);
    }
}
