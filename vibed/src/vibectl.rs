//! `vibectl` core — the operator/admin CLI logic, kept in the library so it is
//! unit-testable without spawning the binary. The thin front-end lives in
//! `src/bin/vibectl.rs`.
//!
//! v0.1 perimeter: read-only memory status and audit-chain verification, the
//! operator side of the approval flow (`approve`/`deny`, root-gated), the
//! **agent supervisor** (`agent run`/`stop`/`thinking`) that runs a CLI in
//! structured mode and taps its reasoning stream (ADR-012/013), and the
//! **memory factory reset** (`memory reset`, root + `--yes`). The reset is not
//! a bare CLI switch: like `mode open`, it is an out-of-band HUMAN action —
//! root-gated, confirmation-guarded, never exposed as an MCP tool — so the
//! destructive decision stays with the operator, on the operator's channel.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use crate::{approval, audit, mcp, mode, reasoning, supervisor};

/// Current effective uid, parsed from `/proc/self/status` (no libc), for the
/// `granted_by` field of an approval and the `require_root` check. `None` if it
/// cannot be read.
fn current_euid() -> Option<u32> {
    parse_effective_uid(&std::fs::read_to_string("/proc/self/status").ok()?)
}

/// Extract the EFFECTIVE uid (2nd field of the `Uid:` line) from the contents of
/// a `/proc/<pid>/status`. Fail-closed: returns `None` if the line or the field
/// is absent — it never falls back to the real uid, so a privilege-dropped
/// process (real=0, euid≠0) can never be read as root.
///
/// Requires EXACTLY ONE `Uid:` line, and fails closed on more. A real
/// `/proc/<pid>/status` has one — but this parser reads the file as TEXT, and
/// the first field of that file is `Name:`, taken from the process's `comm`,
/// which is the basename of the executed file. Linux filenames may contain
/// newlines (only `/` and NUL are forbidden), so `exec`ing a symlink named
/// "\nUid:\t0\t0\t0" is a natural attempt at forging a *second* `Uid:` line that
/// an earlier-wins parser would read first — and this parser gates
/// `vibectl approve`, i.e. the human approval floor.
///
/// It does not work today: the kernel escapes `comm` when rendering status
/// (`fs/proc/array.c` `proc_task_name(..., escape=true)` → `seq_escape_str(...,
/// ESCAPE_SPACE|ESCAPE_SPECIAL, "\n\\")`), so a newline arrives as the two
/// characters `\` `n` and no second line can exist. But that is an INCIDENTAL
/// kernel guarantee this code has no business depending on silently. Demanding a
/// single match removes the dependency: any forgery attempt yields two `Uid:`
/// lines and we refuse.
///
/// Second, independent layer (unchanged): even a forged euid of 0 only passes
/// `require_root`; `approval::approve` then reads a root-owned 0600 file and
/// fails EACCES, minting nothing. `require_root` makes the boundary explicit —
/// it is not what enforces it.
fn parse_effective_uid(status: &str) -> Option<u32> {
    let mut hits = status.lines().filter_map(|l| l.strip_prefix("Uid:"));
    let rest = hits.next()?;
    if hits.next().is_some() {
        // Two `Uid:` lines: a real status has one. Something is forging.
        return None;
    }
    // Uid: <real> <effective> <saved> <fs>
    rest.split_whitespace().nth(1).and_then(|s| s.parse().ok())
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

/// Render an operator-facing value for a TERMINAL. SECURITY-RELEVANT: this is
/// the single chokepoint through which attacker-controlled text reaches the human
/// who decides an approval.
///
/// The pending record shows `{id, tool, target, tier, caller_uid}` and NOT the
/// arguments, so this output is the ONLY basis for the decision — and `target`
/// comes straight from the agent's own tool arguments. An agent that could smuggle
/// a terminal escape in there would rewrite what the operator reads: `\r`
/// overwrites the line, CSI erases it, `\n` forges a plausible extra row.
/// Approving a request you did not actually read is the whole ballgame.
///
/// JSON encoding is what closes that: serde_json escapes all of 0x00–0x1F, so
/// every control byte arrives as inert text. That was TRUE BEFORE THIS FUNCTION
/// EXISTED, but only as a side effect of `to_string_pretty` being what the CLI
/// happened to call — nothing named it, and nothing tested it. A later switch to
/// a "nicer" table renderer would have reopened the whole class in silence.
/// It now has a name, a reason, and a regression test
/// (`attacker_controlled_fields_cannot_rewrite_the_operator_s_terminal`).
///
/// If you change how this renders: the invariant is NO RAW BYTE BELOW 0x20 may
/// reach the terminal.
pub fn render_for_operator(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_default()
}

/// Root gate for operator actions (`approve`/`deny`, `mode open`/`governed`,
/// `memory reset`). The underlying stores are already root-only at the
/// filesystem level; this makes the trust boundary explicit and turns a
/// would-be opaque "permission denied" into a clear message. Fail-closed: if
/// the euid cannot be determined, refuse.
fn require_root(euid: Option<u32>) -> Result<(), Value> {
    match euid {
        Some(0) => Ok(()),
        Some(uid) => Err(json!({
            "error": format!("must be root for this operator action (euid={uid}); use sudo")
        })),
        None => Err(json!({
            "error": "cannot determine caller euid (/proc unavailable); refusing operator action"
        })),
    }
}

/// `vibectl approve <id>` — grant a pending request (operator action, root only).
/// Returns `(report, ok)`.
pub fn approve(id: &str) -> (Value, bool) {
    let euid = current_euid();
    if let Err(e) = require_root(euid) {
        return (e, false);
    }
    match approval::approve(
        Path::new(approval::APPROVAL_DIR),
        id,
        euid,
        now_epoch_secs(),
    ) {
        Ok(grant) => (json!({"approved": id, "grant": grant}), true),
        Err(e) => (json!({"error": e.to_string(), "id": id}), false),
    }
}

/// `vibectl deny <id>` — reject and remove a pending request (root only).
pub fn deny(id: &str) -> (Value, bool) {
    if let Err(e) = require_root(current_euid()) {
        return (e, false);
    }
    match approval::deny(Path::new(approval::APPROVAL_DIR), id) {
        Ok(()) => (json!({"denied": id}), true),
        Err(e) => (json!({"error": e.to_string(), "id": id}), false),
    }
}

/// `vibectl mode status` — the current operating mode (ADR-027). Read-only, for
/// anyone: the mode is not a secret (it is literally what the danger panel
/// shows). Never mutates.
pub fn mode_status() -> Value {
    mode::status(Path::new(mode::MODE_PATH), now_epoch_secs())
}

/// `vibectl mode open [--minutes N] [--reason R]` — the OUT-OF-BAND HUMAN unlock
/// of autonomous/open mode (ADR-027). Root only — the same `require_root` gate
/// as `approve`, because this IS a blanket approval of the T2/T3 floor for a
/// bounded window. An agent can never reach this: it is a `vibectl` command run
/// by the operator, and the mode file is root-only + on vibed's write denylist.
/// Returns `(report, ok)`; the report carries a loud warning by design.
pub fn mode_open(minutes: Option<u64>, reason: Option<&str>) -> (Value, bool) {
    let euid = current_euid();
    if let Err(e) = require_root(euid) {
        return (e, false);
    }
    let secs = minutes
        .map(|m| m.saturating_mul(60))
        .unwrap_or(mode::OPEN_DEFAULT_SECS);
    match mode::set_open(
        Path::new(mode::MODE_PATH),
        secs,
        euid,
        now_epoch_secs(),
        reason,
    ) {
        Ok(record) => (
            json!({
                "mode": "open",
                "record": record,
                "warning": "AUTONOMOUS / OPEN MODE ACTIVE — the AI can act on the system \
                            WITHOUT per-action approval (T2/T3 auto-granted) until this window \
                            expires or you run `vibectl mode governed`. Every call is still \
                            audited; the mode file, the audit trail and the kill-switch stay \
                            out of the agent's reach.",
            }),
            true,
        ),
        Err(e) => (json!({"error": e.to_string()}), false),
    }
}

/// `vibectl mode governed` — the KILL-SWITCH (ADR-027): revert to the governed
/// default immediately, ending autonomous mode. Root only. Idempotent.
pub fn mode_governed() -> (Value, bool) {
    let euid = current_euid();
    if let Err(e) = require_root(euid) {
        return (e, false);
    }
    match mode::set_governed(Path::new(mode::MODE_PATH), euid, now_epoch_secs()) {
        Ok(record) => (json!({"mode": "governed", "record": record}), true),
        Err(e) => (json!({"error": e.to_string()}), false),
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

/// Memoized fold over an append-only updates file. The updates stores grow
/// forever without compaction, and this fold used to re-read and re-parse the
/// WHOLE history on every call — the daemon's only per-call cost that grows
/// without bound with the machine's age, on a path (`memory.query fold:true`)
/// the HUD polls. An unchanged `(len, mtime)` pair on an append-only file
/// means an unchanged fold, so the cached result (keyed by path) is returned
/// as-is; any change re-folds from scratch (simple and always correct, cost
/// only WHEN the file changed). For the one-shot `vibectl` CLI the cache is
/// simply never warm — same behavior as before.
fn fold_updates_cached(path: &Path, fold: fn(Vec<Value>) -> Value) -> Value {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    type Key = (u64, std::time::SystemTime);
    static CACHE: OnceLock<Mutex<HashMap<std::path::PathBuf, (Key, Value)>>> = OnceLock::new();

    let meta = std::fs::metadata(path).ok();
    let key: Option<Key> = meta.and_then(|m| m.modified().ok().map(|t| (m.len(), t)));
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(k) = key {
        let map = cache.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((ck, v)) = map.get(path) {
            if *ck == k {
                return v.clone();
            }
        }
    }
    let v = fold(read_jsonl(path));
    if let Some(k) = key {
        let mut map = cache.lock().unwrap_or_else(|p| p.into_inner());
        map.insert(path.to_path_buf(), (k, v.clone()));
    }
    v
}

/// `vibectl memory profile` — the CURRENT user profile, materialized as the
/// fold of the append-only `user/updates.jsonl` (last-write-wins per `key`).
/// This is the read side of the P1 append-only design (docs/MEMORY.md §3.3).
pub fn memory_profile_at(root: &Path) -> Value {
    fold_updates_cached(&root.join("user").join("updates.jsonl"), |records| {
        let mut profile = serde_json::Map::new();
        for rec in records {
            if let Some(key) = rec.get("key").and_then(Value::as_str) {
                // Later lines overwrite earlier ones — append-only, fold on read.
                let value = rec.get("value").cloned().unwrap_or(Value::Null);
                profile.insert(key.to_string(), value);
            }
        }
        json!({ "profile": Value::Object(profile) })
    })
}

/// `vibectl memory projects` — the CURRENT project index, materialized as the
/// fold of `projects/updates.jsonl` (last-write-wins per `path`), sorted by
/// path (docs/MEMORY.md §3.4).
pub fn memory_projects_at(root: &Path) -> Value {
    fold_updates_cached(&root.join("projects").join("updates.jsonl"), |records| {
        let mut by_path: std::collections::BTreeMap<String, Value> =
            std::collections::BTreeMap::new();
        for rec in records {
            if let Some(path) = rec.get("path").and_then(Value::as_str) {
                by_path.insert(path.to_string(), rec);
            }
        }
        json!({ "projects": by_path.into_values().collect::<Vec<_>>() })
    })
}

// ---------------------------------------------------------------------------
// `vibectl memory reset` — factory reset of the memory store.
//
// Like `mode open`, this is an OUT-OF-BAND HUMAN action: a root-only vibectl
// command on the operator's own channel, never an MCP tool an agent could
// reach. It deliberately does NOT write the vibed audit trail — that trail
// records the agent's mediated actions, and this is the operator acting
// directly, exactly like `mode open`/`mode governed`.
// ---------------------------------------------------------------------------

/// Genesis re-run marker at the store root: while it exists, the Genesis unit
/// stays disarmed (`ConditionPathExists=!`); removing it re-arms Genesis for
/// the next boot.
const MEMORY_INIT_MARKER: &str = ".initialized";

/// Birth files written once by Genesis (identity, hardware survey, drawn
/// personality — ADR-029). Removed on reset so the Genesis replay re-creates
/// them from scratch.
const MEMORY_BIRTH_FILES: [&str; 3] = ["identity.toml", "hardware.json", "personality.toml"];

/// Store subdirectories whose CONTENT is purged on reset. The directories
/// themselves — and the store root, which may be a mount point — are kept, so
/// the on-disk layout stays exactly what the Genesis replay expects.
const MEMORY_SUBDIRS: [&str; 5] = ["user", "projects", "journal", "knowledge", "reasoning"];

/// Remove one known file, best-effort: already absent is fine (the reset is
/// idempotent); any other failure is collected and the purge continues.
fn reset_remove_file(path: &Path, removed: &mut u64, errors: &mut Vec<String>) {
    match std::fs::remove_file(path) {
        Ok(()) => *removed += 1,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => errors.push(format!("{}: {e}", path.display())),
    }
}

/// Purge every entry INSIDE `dir` without removing `dir` itself. Best-effort:
/// an absent subdirectory is fine (Genesis recreates the layout), and per-entry
/// failures (e.g. EACCES) are collected while the purge continues. Each direct
/// entry counts once, whether it is a file or a whole subtree.
fn reset_purge_dir(dir: &Path, removed: &mut u64, errors: &mut Vec<String>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(e) => {
            errors.push(format!("{}: {e}", dir.display()));
            return;
        }
    };
    for entry in entries {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        // `file_type()` does not follow symlinks, so a symlinked entry is
        // removed as a link — the purge never reaches outside the store.
        let outcome = if entry.file_type().is_ok_and(|t| t.is_dir()) {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        match outcome {
            Ok(()) => *removed += 1,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => errors.push(format!("{}: {e}", path.display())),
        }
    }
}

/// `vibectl memory reset --yes` core: purge the memory store rooted at `root`
/// and re-arm Genesis. Without `confirmed`, refuses with an explicit listing
/// of what WOULD be destroyed (the CLI maps this to the `--yes` flag).
///
/// What it destroys is a CLOSED LIST — the init marker, the Genesis birth
/// files, and the content of the known subdirectories. Anything ELSE at the
/// store root (an operator's stray backup, `lost+found` on a dedicated
/// filesystem, a file a newer schema added before this list learned about it)
/// is deliberately left alone: a factory reset must never eat data this code
/// did not write — least surprise beats completeness when mistakes are
/// unrecoverable. The root and the subdirectories themselves are also kept
/// (the root may be a mount point), so the layout is ready for the Genesis
/// replay at next boot.
///
/// Best-effort: already-absent paths are fine (idempotent) and per-path
/// failures land in `errors` while the purge continues. HONESTY: this is a
/// file-level purge — the bytes remain recoverable on the underlying device
/// until cryptographic erasure ships with LUKS (Phase 3).
pub fn memory_reset_at(root: &Path, confirmed: bool) -> Result<Value, Value> {
    if !confirmed {
        return Err(json!({
            "error": "memory reset is destructive and needs explicit confirmation: \
                      re-run with --yes",
            "would_remove": {
                "root": root.to_string_lossy(),
                "files": std::iter::once(MEMORY_INIT_MARKER)
                    .chain(MEMORY_BIRTH_FILES)
                    .collect::<Vec<_>>(),
                "content_of": MEMORY_SUBDIRS,
            },
            "kept": "the store root and the subdirectories themselves \
                     (mount point / layout preserved), plus anything not listed above",
            "then": "Genesis re-runs at next boot and re-creates a fresh identity",
        }));
    }

    let mut removed_files: u64 = 0;
    let mut removed_entries: u64 = 0;
    let mut errors: Vec<String> = Vec::new();

    // The marker first: even if a later step fails, Genesis is already re-armed.
    reset_remove_file(
        &root.join(MEMORY_INIT_MARKER),
        &mut removed_files,
        &mut errors,
    );
    for name in MEMORY_BIRTH_FILES {
        reset_remove_file(&root.join(name), &mut removed_files, &mut errors);
    }
    for sub in MEMORY_SUBDIRS {
        reset_purge_dir(&root.join(sub), &mut removed_entries, &mut errors);
    }

    // "Re-armed" is measured, not assumed: Genesis runs at next boot iff the
    // marker is actually gone (also true for a store that never existed).
    let rearmed = !root.join(MEMORY_INIT_MARKER).exists();
    Ok(json!({
        "removed_files": removed_files,
        "removed_entries": removed_entries,
        "errors": errors,
        "rearmed": rearmed,
        "note": "file-level purge only: bytes remain on the device until cryptographic \
                 erasure ships with LUKS (Phase 3)",
    }))
}

/// `vibectl memory reset [--yes]` — production wrapper: root only (the same
/// gate as `mode open`), default store root. `ok` is true only for a clean
/// purge — any collected error flips the exit code so a partial reset is
/// impossible to miss.
pub fn memory_reset(confirmed: bool) -> (Value, bool) {
    if let Err(e) = require_root(current_euid()) {
        return (e, false);
    }
    match memory_reset_at(Path::new(mcp::MEMORY_DIR), confirmed) {
        Ok(report) => {
            let clean = report["errors"].as_array().map_or(true, |a| a.is_empty());
            (report, clean)
        }
        Err(refusal) => (refusal, false),
    }
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

// ---------------------------------------------------------------------------
// Agent supervisor (ADR-012/013) — `vibectl agent run/stop/thinking`.
//
// Runs a CLI in structured mode and TAPS its `stream-json` output into the
// reasoning store, under a wall-clock + tool-call budget. The kill-switch is
// operator-only (`agent stop` drops a `.stop` marker the run loop polls) — never
// an MCP tool the agent could reach. A T2/T3 that is not yet approved does NOT
// block: vibed already answers `pending_approval`, the agent moves on to other
// T0/T1 work, and the operator approves out of band (ADR-013). Paths are
// parameters so the whole flow is unit-testable against scratch dirs.
// ---------------------------------------------------------------------------

/// Options for `agent run`.
pub struct AgentRunOpts {
    /// CLI executable + args (everything after `--`).
    pub command: Vec<String>,
    pub budget_secs: Option<u64>,
    pub max_calls: Option<u64>,
    /// Total-token allowance (see [`supervisor::Budget::max_tokens`]). `None` =
    /// unbounded on tokens.
    pub max_tokens: Option<u64>,
    pub session_id: Option<String>,
    pub provider: String,
}

/// Append a reserved `autonomous_session` event to the memory journal
/// (journal/<utc-date>.jsonl). Best-effort: a supervised run is worth doing even
/// if the machine journal is momentarily unwritable. Never creates the store
/// root (fail-closed if Genesis has not run).
fn write_session_journal(mem: &Path, event: &Value, epoch: u64) {
    if !mem.is_dir() {
        return;
    }
    let dir = mem.join("journal");
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let path = dir.join(format!("{}.jsonl", mcp::utc_date_string(epoch)));
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(&path)
    {
        if let Ok(mut line) = serde_json::to_string(event) {
            line.push('\n');
            let _ = f.write_all(line.as_bytes());
        }
    }
}

/// Terminate the CLI's whole process group (it is the group leader via
/// `process_group(0)`, so its pgid equals its pid). SIGTERM for a graceful stop
/// — letting the CLI flush a final reasoning block — then SIGKILL after a short
/// grace so a grandchild that ignores SIGTERM (or holds the stdout pipe) cannot
/// keep the reader thread blocked. Shells out to `kill` (util-linux, always
/// present) so vibed's TCB stays dependency-free; a negative target is the
/// process group.
///
/// LIMIT (best-effort): a grandchild that detaches its own process group
/// (`setsid`/`setpgid`) escapes the group signal and can leak as an orphan.
/// This is inherent to signal-based group kill. It is a resource-leak concern,
/// NOT a liveness one: the bounded drain below means such a grandchild can never
/// hang `agent_run` even while holding the stdout pipe (see the C2 regression
/// test). True containment of detached descendants is a Phase 4 sandbox concern
/// (a per-run cgroup/scope kills the whole subtree regardless of pgid).
fn terminate_group(pid: u32) {
    let group = format!("-{pid}");
    let _ = Command::new("kill").arg("-TERM").arg(&group).status();
    std::thread::sleep(Duration::from_millis(300));
    let _ = Command::new("kill").arg("-KILL").arg(&group).status();
}

/// Max bytes buffered for one `stream-json` line before it is dropped. Aligned
/// with the reasoning store's per-line cap (a bigger line would be refused there
/// anyway). Bounds the reader's memory against a giant single-line event.
const STREAM_LINE_CAP: usize = reasoning::REASONING_MAX_LINE_BYTES;

/// Read one `\n`-terminated line into `buf`, capping at `max` bytes. Returns
/// `Some(false)` for a normal line (in `buf`, newline stripped), `Some(true)`
/// when a line exceeded `max` and was DROPPED (buf cleared, bytes discarded to
/// the next newline — memory bounded), and `None` at EOF. Unlike
/// `BufRead::lines()`/`read_until`, the buffer never grows past `max`.
fn read_capped_line<R: BufRead>(reader: &mut R, buf: &mut Vec<u8>, max: usize) -> Option<bool> {
    buf.clear();
    let mut over = false;
    loop {
        let available = reader.fill_buf().ok()?;
        if available.is_empty() {
            return if buf.is_empty() && !over {
                None
            } else {
                Some(over)
            };
        }
        match available.iter().position(|&b| b == b'\n') {
            Some(i) => {
                if !over && buf.len() + i <= max {
                    buf.extend_from_slice(&available[..i]);
                } else {
                    over = true; // the line (with this chunk) exceeds `max`
                    buf.clear();
                }
                reader.consume(i + 1);
                return Some(over);
            }
            None => {
                let n = available.len();
                if !over && buf.len() + n <= max {
                    buf.extend_from_slice(available);
                } else {
                    over = true;
                    buf.clear(); // release the partial over-long line
                }
                reader.consume(n);
            }
        }
    }
}

/// `vibectl agent thinking` — bounded tail of a session's captured reasoning
/// (the same store as the T0 `agent.thinking` MCP tool).
pub fn agent_thinking(
    mem: &Path,
    session_id: &str,
    tail: Option<usize>,
    since: Option<u64>,
) -> (Value, bool) {
    match reasoning::read_thinking(mem, session_id, tail, since) {
        Ok(v) => (v, true),
        Err(e) => (json!({"error": e}), false),
    }
}

/// `vibectl agent stop <session>` — operator kill-switch: drop a `.stop` marker
/// the running supervisor polls, so it terminates its child and writes the END
/// event. Never an MCP tool.
pub fn agent_stop(run_dir: &Path, session_id: &str) -> (Value, bool) {
    let Some(id) = reasoning::safe_session_id(session_id) else {
        return (json!({"error": "invalid session id"}), false);
    };
    if std::fs::create_dir_all(run_dir).is_err() {
        return (json!({"error": "cannot access agent run dir"}), false);
    }
    match std::fs::write(run_dir.join(format!("{id}.stop")), b"stop\n") {
        Ok(()) => (json!({"stopping": id}), true),
        Err(e) => (json!({"error": e.to_string(), "session": id}), false),
    }
}

/// Live token tallies shared between the stdout reader thread and the budget
/// poll loop — grouped so the counters thread as one `Arc`. Accumulated from
/// per-turn `assistant` usage; the authoritative run cost is stored (last
/// `result` wins) separately, gated by `has_cost` (0.0 is a real cost, not
/// "unset").
#[derive(Default)]
struct TokenAtomics {
    input: AtomicU64,
    output: AtomicU64,
    cache_creation: AtomicU64,
    cache_read: AtomicU64,
    turns: AtomicU64,
    cost_micro_usd: AtomicU64,
    has_cost: AtomicBool,
}

impl TokenAtomics {
    /// Fold one `stream-json` event: sum per-turn usage, capture the terminal
    /// cost. Mirrors [`supervisor::TokenLedger::add_event`] over shared atomics.
    fn observe(&self, ev: &Value) {
        if let Some(u) = supervisor::extract_usage(ev) {
            self.input.fetch_add(u.input, Ordering::Relaxed);
            self.output.fetch_add(u.output, Ordering::Relaxed);
            self.cache_creation
                .fetch_add(u.cache_creation, Ordering::Relaxed);
            self.cache_read.fetch_add(u.cache_read, Ordering::Relaxed);
            self.turns.fetch_add(1, Ordering::Relaxed);
        }
        if let Some(cost) = supervisor::extract_final_cost(ev) {
            // Store, not add: `result` is cumulative, so the last one wins.
            self.cost_micro_usd
                .store((cost * 1_000_000.0) as u64, Ordering::Relaxed);
            self.has_cost.store(true, Ordering::Relaxed);
        }
    }
    /// Snapshot the running total the token budget caps.
    fn total_tokens(&self) -> u64 {
        self.input
            .load(Ordering::Relaxed)
            .saturating_add(self.output.load(Ordering::Relaxed))
            .saturating_add(self.cache_creation.load(Ordering::Relaxed))
            .saturating_add(self.cache_read.load(Ordering::Relaxed))
    }
    /// Rebuild a [`supervisor::TokenLedger`] to reuse its derived metrics
    /// (cache ratio, input-equivalent, savings) — one source of truth for the
    /// formulas.
    fn ledger(&self) -> supervisor::TokenLedger {
        supervisor::TokenLedger {
            totals: supervisor::TokenUsage {
                input: self.input.load(Ordering::Relaxed),
                output: self.output.load(Ordering::Relaxed),
                cache_creation: self.cache_creation.load(Ordering::Relaxed),
                cache_read: self.cache_read.load(Ordering::Relaxed),
            },
            turns: self.turns.load(Ordering::Relaxed),
        }
    }
    /// The authoritative run cost in USD, or `None` if the CLI never reported one.
    fn cost_usd(&self) -> Option<f64> {
        self.has_cost
            .load(Ordering::Relaxed)
            .then(|| self.cost_micro_usd.load(Ordering::Relaxed) as f64 / 1_000_000.0)
    }
}

/// `vibectl agent run -- <cmd...>` — run a CLI under budget, tapping its
/// `stream-json` reasoning into `mem/reasoning/<session>.jsonl`. `run_dir` holds
/// the pid + stop markers. Returns a summary `(json, ok)`.
pub fn agent_run(mem: &Path, run_dir: &Path, opts: AgentRunOpts) -> (Value, bool) {
    if opts.command.is_empty() {
        return (
            json!({"error": "no command to run (expected: agent run -- <cmd>)"}),
            false,
        );
    }
    let start = now_epoch_secs();
    let sid = opts
        .session_id
        .clone()
        .unwrap_or_else(|| supervisor::new_session_id(start, std::process::id()));
    if reasoning::safe_session_id(&sid).is_none() {
        return (json!({"error": "invalid session id"}), false);
    }
    let budget = supervisor::Budget::new(opts.budget_secs, opts.max_calls, opts.max_tokens);

    // START journal event (reserved `autonomous_session` type — unforgeable by agents).
    write_session_journal(
        mem,
        &supervisor::session_journal_event(
            &sid,
            "start",
            &opts.provider,
            &mcp::utc_iso8601(start),
            json!({ "command": opts.command, "budget_secs": opts.budget_secs,
                    "max_calls": opts.max_calls, "max_tokens": opts.max_tokens }),
        ),
        start,
    );

    // Spawn the CLI with stdout piped (stderr inherited so the operator sees it),
    // in its OWN process group. A CLI spawns grandchildren (node, MCP bridges,
    // …) that inherit the stdout pipe; killing only the direct child would leave
    // them holding the write end open, blocking the reader thread until they die
    // on their own. `process_group(0)` lets us terminate the whole group.
    let mut child = match Command::new(&opts.command[0])
        .args(&opts.command[1..])
        .stdout(Stdio::piped())
        .process_group(0)
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            let end = now_epoch_secs();
            write_session_journal(
                mem,
                &supervisor::session_journal_event(
                    &sid,
                    "end",
                    &opts.provider,
                    &mcp::utc_iso8601(end),
                    json!({"reason": "spawn_failed", "error": e.to_string()}),
                ),
                end,
            );
            return (
                json!({"error": format!("cannot spawn '{}': {e}", opts.command[0]), "session": sid}),
                false,
            );
        }
    };

    let _ = std::fs::create_dir_all(run_dir);
    let pidfile = run_dir.join(format!("{sid}.pid"));
    let stopfile = run_dir.join(format!("{sid}.stop"));
    let _ = std::fs::remove_file(&stopfile); // clear a stale stop from a prior run
    let _ = std::fs::write(&pidfile, format!("{}\n", child.id()));

    // Reader thread taps stdout: reasoning -> store, tool_use -> counter. It
    // reports via shared atomics (not a join value) so the supervisor NEVER
    // blocks on it — a misbehaving CLI whose grandchild keeps the stdout pipe
    // open must not hang `agent run`.
    let tool_calls = Arc::new(AtomicU64::new(0));
    let blocks = Arc::new(AtomicU64::new(0));
    let tokens = Arc::new(TokenAtomics::default());
    let reader_done = Arc::new(AtomicBool::new(false));
    if let Some(stdout) = child.stdout.take() {
        let mem = mem.to_path_buf();
        let sid = sid.clone();
        let tool_calls = Arc::clone(&tool_calls);
        let blocks = Arc::clone(&blocks);
        let tokens = Arc::clone(&tokens);
        let reader_done = Arc::clone(&reader_done);
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut buf: Vec<u8> = Vec::new();
            // Bounded line read: a single giant `stream-json` line (model- or
            // agent-influenced — e.g. a huge tool result echoed in one event)
            // must NOT grow the buffer without limit and OOM the supervisor
            // during an unattended run. Over-`STREAM_LINE_CAP` lines are dropped.
            while let Some(over) = read_capped_line(&mut reader, &mut buf, STREAM_LINE_CAP) {
                if over {
                    continue; // over-long line dropped, memory released
                }
                let Ok(line) = std::str::from_utf8(&buf) else {
                    continue;
                };
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                let Ok(ev) = serde_json::from_str::<Value>(line) else {
                    continue;
                };
                tool_calls.fetch_add(supervisor::count_tool_use(&ev) as u64, Ordering::Relaxed);
                tokens.observe(&ev);
                if let Some(block) = supervisor::extract_thinking(&ev) {
                    if reasoning::append_thinking(&mem, &sid, &block, now_epoch_secs()).is_ok() {
                        blocks.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
            reader_done.store(true, Ordering::Release);
        });
    } else {
        reader_done.store(true, Ordering::Release);
    }

    // Poll loop: enforce budget + stop marker; end when the child exits. The
    // wall budget is measured from a MONOTONIC Instant — immune to system-clock
    // steps (NTP correction, VM snapshot resume, boot-with-wrong-RTC then sync) —
    // unlike the epoch timestamps used for journaling. A backward clock step must
    // never let a runaway agent outlive its `--budget`.
    let run_started = std::time::Instant::now();
    let mut reason = "completed";
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break, // the child exited on its own
            Ok(None) => {}        // still running — enforce the budget below
            Err(_) => {
                // Unknown child state: kill best-effort and stop, rather than
                // fall through to a wait() that could block on a still-live child.
                let pid = child.id();
                let _ = child.kill();
                terminate_group(pid);
                reason = "wait_error";
                break;
            }
        }
        let over_wall = budget.wall_expired(run_started.elapsed());
        let over_calls = budget.tool_calls_exhausted(tool_calls.load(Ordering::Relaxed));
        let over_tokens = budget.tokens_exhausted(tokens.total_tokens());
        let stopped = stopfile.exists();
        if over_wall || over_calls || over_tokens || stopped {
            reason = if stopped {
                "operator_stop"
            } else if over_wall {
                "wall_budget"
            } else if over_calls {
                "tool_budget"
            } else {
                "token_budget"
            };
            // Kill the direct child (SIGKILL via std — guaranteed, so the wait()
            // below never blocks) AND its whole group (grandchildren holding the
            // stdout pipe). Order: pid, then kill, then group.
            let pid = child.id();
            let _ = child.kill();
            terminate_group(pid);
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    // The child has exited (completed path) or been killed (budget/stop path); in
    // both cases it is a zombie, NOT yet reaped, so its pid/pgid cannot be reused
    // — capture the pid before reaping.
    let child_pid = child.id();

    // Bounded drain: give the reader a moment to finish the now-closed pipe.
    // Never an unbounded join — cap the wait so the supervisor always returns.
    let drain_deadline = std::time::Instant::now() + Duration::from_secs(2);
    while !reader_done.load(Ordering::Acquire) && std::time::Instant::now() < drain_deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    // If the reader still hasn't finished, a grandchild is holding the stdout
    // write-end open (this can happen on the CLEAN-exit path too, where we did
    // not group-kill). Terminate the whole group to release the pipe and stop
    // the leaked grandchildren — safe because the child is not yet reaped.
    if !reader_done.load(Ordering::Acquire) {
        terminate_group(child_pid);
    }
    let _ = child.wait(); // reap the direct child (dead — never blocks)
    let blocks = blocks.load(Ordering::Relaxed);
    let calls = tool_calls.load(Ordering::Relaxed);

    // Token accounting summary: raw counts (authoritative, from the CLI's
    // per-turn `usage`) plus the derived cache-efficiency metrics that make the
    // spend actionable, and the authoritative USD cost when the CLI reported one.
    let ledger = tokens.ledger();
    let token_summary = json!({
        "input": ledger.totals.input,
        "output": ledger.totals.output,
        "cache_creation": ledger.totals.cache_creation,
        "cache_read": ledger.totals.cache_read,
        "total": ledger.total_tokens(),
        "turns": ledger.turns,
        "cache_hit_ratio": ledger.cache_hit_ratio(),
        "input_equiv_tokens": ledger.input_equiv_tokens(),
        "cache_savings_tokens": ledger.cache_savings_tokens(),
        "cost_usd": tokens.cost_usd(),
    });

    let end = now_epoch_secs();
    write_session_journal(
        mem,
        &supervisor::session_journal_event(
            &sid,
            "end",
            &opts.provider,
            &mcp::utc_iso8601(end),
            json!({ "reason": reason, "reasoning_blocks": blocks, "tool_calls": calls,
                    "duration_secs": end.saturating_sub(start), "tokens": token_summary }),
        ),
        end,
    );
    let _ = std::fs::remove_file(&pidfile);
    let _ = std::fs::remove_file(&stopfile);

    (
        json!({ "session": sid, "reason": reason, "reasoning_blocks": blocks,
                "tool_calls": calls, "duration_secs": end.saturating_sub(start),
                "tokens": token_summary }),
        true,
    )
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
    fn memoized_fold_recomputes_after_an_append() {
        let root = scratch("profile-memo");
        std::fs::create_dir_all(root.join("user")).unwrap();
        let path = root.join("user").join("updates.jsonl");
        std::fs::write(
            &path,
            "{\"ts\":\"t1\",\"key\":\"k\",\"value\":\"v1\",\"source\":\"x\"}\n",
        )
        .unwrap();
        assert_eq!(memory_profile_at(&root)["profile"]["k"], "v1");

        // Append-only growth changes (len, mtime): the memoized fold must
        // recompute, never serve the stale profile.
        {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap();
            f.write_all(b"{\"ts\":\"t2\",\"key\":\"k\",\"value\":\"v2\",\"source\":\"x\"}\n")
                .unwrap();
        }
        assert_eq!(
            memory_profile_at(&root)["profile"]["k"],
            "v2",
            "an appended update must invalidate the memoized fold"
        );
        // An unchanged file returns the same (cached) fold.
        assert_eq!(memory_profile_at(&root)["profile"]["k"], "v2");
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

    // --- memory reset -------------------------------------------------------

    /// A fully populated store: init marker, the three birth files, one file in
    /// every known subdirectory, plus one NESTED directory (the purge must take
    /// whole subtrees, not only flat files).
    fn populated_store(tag: &str) -> std::path::PathBuf {
        let root = scratch(tag);
        for sub in ["user", "projects", "journal", "knowledge", "reasoning"] {
            std::fs::create_dir_all(root.join(sub)).unwrap();
        }
        std::fs::write(root.join(".initialized"), "").unwrap();
        std::fs::write(root.join("identity.toml"), "schema = 1\n").unwrap();
        std::fs::write(root.join("hardware.json"), "{}").unwrap();
        std::fs::write(root.join("personality.toml"), "schema = 1\n").unwrap();
        std::fs::write(root.join("user").join("updates.jsonl"), "{}\n").unwrap();
        std::fs::write(root.join("projects").join("updates.jsonl"), "{}\n").unwrap();
        std::fs::write(root.join("journal").join("2026-07-21.jsonl"), "{}\n").unwrap();
        std::fs::write(root.join("knowledge").join("facts.jsonl"), "{}\n").unwrap();
        std::fs::write(root.join("reasoning").join("sess.jsonl"), "{}\n").unwrap();
        std::fs::create_dir_all(root.join("knowledge").join("topics")).unwrap();
        std::fs::write(root.join("knowledge").join("topics").join("x.md"), "x").unwrap();
        root
    }

    #[test]
    fn memory_reset_refuses_without_confirmation() {
        let root = populated_store("reset-noyes");
        let err = memory_reset_at(&root, false).unwrap_err();
        let msg = err["error"].as_str().unwrap();
        assert!(msg.contains("--yes"), "refusal must point at --yes: {msg}");
        // The refusal says WHAT would be destroyed, so --yes is informed consent.
        let files = err["would_remove"]["files"].as_array().unwrap();
        assert!(files.iter().any(|f| f == "identity.toml"));
        // And nothing was touched.
        assert!(root.join(".initialized").exists());
        assert!(root.join("identity.toml").exists());
        assert!(root.join("user").join("updates.jsonl").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn memory_reset_purges_and_keeps_the_layout() {
        let root = populated_store("reset-purge");
        let report = memory_reset_at(&root, true).unwrap();
        assert_eq!(
            report["errors"].as_array().unwrap().len(),
            0,
            "clean purge: {report}"
        );
        assert_eq!(report["rearmed"], true);
        assert_eq!(report["removed_files"], 4, "marker + 3 birth files");
        assert_eq!(
            report["removed_entries"], 6,
            "5 subdir files + 1 nested directory"
        );
        for f in [
            ".initialized",
            "identity.toml",
            "hardware.json",
            "personality.toml",
        ] {
            assert!(!root.join(f).exists(), "{f} must be gone");
        }
        // Root and subdirectories survive, EMPTY: the mount point is intact and
        // the layout is exactly what the Genesis replay expects.
        assert!(root.is_dir(), "store root must survive");
        for sub in ["user", "projects", "journal", "knowledge", "reasoning"] {
            let dir = root.join(sub);
            assert!(dir.is_dir(), "{sub}/ must survive the reset");
            assert_eq!(
                std::fs::read_dir(&dir).unwrap().count(),
                0,
                "{sub}/ must be empty"
            );
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn memory_reset_is_idempotent() {
        let root = populated_store("reset-idem");
        memory_reset_at(&root, true).unwrap();
        // Second run: nothing left to remove, and that is NOT an error.
        let second = memory_reset_at(&root, true).unwrap();
        assert_eq!(second["removed_files"], 0);
        assert_eq!(second["removed_entries"], 0);
        assert_eq!(second["errors"].as_array().unwrap().len(), 0);
        assert_eq!(second["rearmed"], true, "still re-armed");
        // A store that never existed (unmounted amnesic tmpfs): same contract.
        let absent = scratch("reset-absent");
        let v = memory_reset_at(&absent, true).unwrap();
        assert_eq!(v["removed_files"], 0);
        assert_eq!(v["removed_entries"], 0);
        assert_eq!(v["errors"].as_array().unwrap().len(), 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// CONTRACT (least surprise): the reset destroys ONLY what it knows — the
    /// init marker, the birth files, and the content of the known
    /// subdirectories. An unexpected entry at the store root (an operator's
    /// stray backup, `lost+found` on a dedicated filesystem, a file from a
    /// newer schema) is preserved: a factory reset must never eat data this
    /// code did not write, because until LUKS-level erasure exists (Phase 3)
    /// its mistakes are unrecoverable.
    #[test]
    fn memory_reset_leaves_unknown_root_entries_alone() {
        let root = populated_store("reset-stray");
        std::fs::write(root.join("operator-backup.tar"), "precious").unwrap();
        std::fs::create_dir_all(root.join("lost+found")).unwrap();
        std::fs::write(root.join("lost+found").join("blob"), "x").unwrap();
        let report = memory_reset_at(&root, true).unwrap();
        assert_eq!(report["errors"].as_array().unwrap().len(), 0);
        assert!(
            root.join("operator-backup.tar").exists(),
            "unknown root file preserved"
        );
        assert!(
            root.join("lost+found").join("blob").exists(),
            "unknown root directory preserved, content included"
        );
        assert!(
            !root.join("identity.toml").exists(),
            "known files are still purged"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn audit_verify_reports_ok_for_absent_log() {
        let (report, ok) = audit_verify(Path::new("/nonexistent-audit.jsonl"));
        assert!(ok);
        assert_eq!(report["records"], 0);
    }

    #[test]
    fn read_capped_line_drops_oversized_lines() {
        use std::io::Cursor;
        // A normal line, a giant (over-cap) line, another normal line.
        let mut data = Vec::new();
        data.extend_from_slice(b"small1\n");
        data.extend_from_slice(&[b'x'; 50]);
        data.push(b'\n');
        data.extend_from_slice(b"small2\n");
        let mut reader = BufReader::new(Cursor::new(data));
        let mut buf = Vec::new();
        // cap = 10: "small1" fits; the 50-byte line is DROPPED; "small2" fits.
        assert_eq!(read_capped_line(&mut reader, &mut buf, 10), Some(false));
        assert_eq!(buf, b"small1");
        assert_eq!(
            read_capped_line(&mut reader, &mut buf, 10),
            Some(true),
            "oversized line reported as dropped"
        );
        assert!(buf.is_empty(), "dropped line releases its memory");
        assert_eq!(read_capped_line(&mut reader, &mut buf, 10), Some(false));
        assert_eq!(buf, b"small2");
        assert_eq!(read_capped_line(&mut reader, &mut buf, 10), None, "EOF");
    }

    #[test]
    fn only_root_may_approve_or_deny() {
        assert!(require_root(Some(0)).is_ok(), "root is allowed");
        // A non-root operator is refused with a clear message, not a file error.
        let err = require_root(Some(1000)).unwrap_err();
        assert!(err["error"].as_str().unwrap().contains("must be root"));
        // Fail-closed when euid is unknown.
        assert!(require_root(None).is_err(), "refuse when euid unknown");
    }

    #[test]
    fn parse_effective_uid_picks_the_effective_field_fail_closed() {
        // Normal status: effective uid is the 2nd field, not the real one.
        assert_eq!(
            parse_effective_uid("Name:\tx\nUid:\t1000\t0\t0\t1000\nGid:\t0\t0\t0\t0\n"),
            Some(0),
            "returns the effective uid, not the real uid"
        );
        // Privilege-dropped process (real root, dropped euid): must NOT read as root.
        assert_eq!(
            parse_effective_uid("Uid:\t0\t1000\t1000\t1000\n"),
            Some(1000),
            "euid wins over a real uid of 0"
        );
        // Malformed line without an effective field -> fail closed, never real.
        assert_eq!(parse_effective_uid("Uid:\t1000\n"), None);
        // No Uid line at all -> None.
        assert_eq!(parse_effective_uid("Gid:\t0\t0\t0\t0\n"), None);
    }

    #[test]
    fn a_forged_second_uid_line_fails_closed_instead_of_winning() {
        // `Name:` is the process's comm, i.e. the basename of the executed file,
        // and Linux filenames may contain newlines. Exec'ing a symlink named
        // "\nUid:\t0\t0\t0" is the natural attempt at forging a Uid: line that
        // lands BEFORE the real one — and this parser gates `vibectl approve`.
        //
        // The kernel escapes comm when rendering status, so this shape cannot
        // occur today. That is a kernel guarantee this code should not depend on
        // silently: two Uid: lines means something is forging, so refuse.
        let forged = "Name:\n\
                      Uid:\t0\t0\t0\t0\n\
                      Umask:\t0022\n\
                      Uid:\t1000\t1000\t1000\t1000\n";
        assert_eq!(
            parse_effective_uid(forged),
            None,
            "a status with two Uid: lines must fail closed, not hand back root"
        );
        // ...including the reverse order: never trust EITHER match.
        let forged_after = "Uid:\t1000\t1000\t1000\t1000\nUid:\t0\t0\t0\t0\n";
        assert_eq!(parse_effective_uid(forged_after), None);
        // A single, genuine line still works — the guard must not break the
        // normal path.
        assert_eq!(
            parse_effective_uid("Name:\tvibectl\nUid:\t0\t0\t0\t0\n"),
            Some(0)
        );
    }

    /// The operator's approval decision is made from `approvals_list` output and
    /// NOTHING else: the pending record carries {id, tool, target, tier,
    /// caller_uid} and no arguments. `target` is fully attacker-controlled (it
    /// comes from the agent's own tool arguments).
    ///
    /// So a hostile agent that could smuggle a terminal escape into `target`
    /// could rewrite what the operator reads — `\r` overwrites the line, ANSI
    /// erases it, `\n` forges an extra plausible row. Approving a request you did
    /// not read is the whole ballgame.
    ///
    /// This is closed TODAY, but INCIDENTALLY: the rendering goes through
    /// `serde_json::to_string_pretty`, whose escape table covers all of
    /// 0x00–0x1F. Nothing in the code said so, and the day someone renders this
    /// as a human-friendly table the entire class silently reopens. This test is
    /// that missing statement.
    ///
    /// It deliberately exercises `render_for_operator` — the function the CLI
    /// actually prints through — and NOT `to_string_pretty` directly. Asserting
    /// on `to_string_pretty` would pin serde_json's behaviour, which nobody is
    /// about to change, while the thing that WILL change (how vibectl renders)
    /// walked free. That is how a test becomes theatre.
    #[test]
    fn attacker_controlled_fields_cannot_rewrite_the_operator_s_terminal() {
        let hostile = json!({
            "pending": [{
                "id": "1-2-3",
                "tool": "pkg.install",
                // Every trick at once: CSI erase-line, carriage return, a forged
                // second row, and a bell.
                "target": "\u{1b}[2K\rpkg.install  vim\n  id: 9-9-9  target: vim\u{7}",
                "tier": "T2",
                "caller_uid": 1000,
            }]
        });
        let rendered = render_for_operator(&hostile);

        // No RAW control byte may reach the terminal. This is the invariant.
        for (i, b) in rendered.bytes().enumerate() {
            assert!(
                b >= 0x20 || b == b'\n' || b == b'\t',
                "raw control byte {b:#04x} at offset {i} would reach the operator's \
                 terminal: {rendered}"
            );
        }
        // Specifically: ESC and CR survive only as INERT text, and the forged
        // row's newline cannot start a real line.
        assert!(!rendered.contains('\u{1b}'), "raw ESC present");
        assert!(!rendered.contains('\r'), "raw CR present");
        assert!(!rendered.contains('\u{7}'), "raw BEL present");
        assert!(
            rendered.contains("\\u001b[2K\\rpkg.install"),
            "the escape must be shown escaped, not executed: {rendered}"
        );
        // The forged row is one JSON string, not a line of its own: the only
        // real newlines are the ones to_string_pretty itself emits, and none of
        // them sits inside the target value.
        let target_line = rendered
            .lines()
            .find(|l| l.contains("\"target\""))
            .expect("target rendered on one line");
        assert!(
            target_line.contains("id: 9-9-9"),
            "the forged row must stay INSIDE the target string, on its line: \
             {target_line}"
        );
    }

    // --- agent supervisor (ADR-012/013) ------------------------------------

    fn mem_scratch(tag: &str) -> std::path::PathBuf {
        let dir = scratch(tag);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn agent_run_taps_reasoning_and_journals_the_session() {
        let mem = mem_scratch("agentrun-mem");
        let run = scratch("agentrun-run");
        // Fake CLI: emit stream-json with a thinking delta, a tool_use, then exit.
        let l1 = r#"{"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"let me look"}}"#;
        let l2 = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"fs.read","input":{}}]}}"#;
        let l3 = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"done"}]}}"#;
        let script = format!("printf '%s\\n' '{l1}' '{l2}' '{l3}'");
        let (summary, ok) = agent_run(
            &mem,
            &run,
            AgentRunOpts {
                command: vec!["sh".into(), "-c".into(), script],
                budget_secs: Some(30),
                max_calls: None,
                max_tokens: None,
                session_id: Some("test-sess".into()),
                provider: "fake".into(),
            },
        );
        assert!(ok, "run failed: {summary}");
        assert_eq!(summary["reason"], "completed");
        assert_eq!(
            summary["reasoning_blocks"], 1,
            "one thinking delta captured"
        );
        assert_eq!(summary["tool_calls"], 1, "one tool_use counted");

        // Reasoning store got the thinking block.
        let think = reasoning::read_thinking(&mem, "test-sess", None, None).unwrap();
        assert_eq!(think["count"], 1);
        assert_eq!(think["lines"][0]["block"]["text"], "let me look");

        // Journal has the reserved autonomous_session start + end events.
        let journal_dir = mem.join("journal");
        let entries: Vec<Value> = std::fs::read_dir(&journal_dir)
            .unwrap()
            .flatten()
            .filter_map(|e| std::fs::read_to_string(e.path()).ok())
            .flat_map(|c| {
                c.lines()
                    .filter_map(|l| serde_json::from_str::<Value>(l).ok())
                    .collect::<Vec<_>>()
            })
            .collect();
        let sess: Vec<&Value> = entries
            .iter()
            .filter(|e| e["type"] == "autonomous_session")
            .collect();
        assert_eq!(sess.len(), 2, "start + end");
        assert!(sess.iter().any(|e| e["data"]["phase"] == "start"));
        assert!(sess.iter().any(|e| e["data"]["phase"] == "end"));
        // Markers cleaned up.
        assert!(!run.join("test-sess.pid").exists());
        let _ = std::fs::remove_dir_all(&mem);
        let _ = std::fs::remove_dir_all(&run);
    }

    #[test]
    fn agent_run_enforces_wall_budget() {
        let mem = mem_scratch("agentbudget-mem");
        let run = scratch("agentbudget-run");
        // Child sleeps far longer than the 1s budget -> killed as wall_budget.
        let (summary, ok) = agent_run(
            &mem,
            &run,
            AgentRunOpts {
                command: vec!["sh".into(), "-c".into(), "sleep 30".into()],
                budget_secs: Some(1),
                max_calls: None,
                max_tokens: None,
                session_id: Some("budget-sess".into()),
                provider: "fake".into(),
            },
        );
        assert!(ok);
        assert_eq!(summary["reason"], "wall_budget");
        let _ = std::fs::remove_dir_all(&mem);
        let _ = std::fs::remove_dir_all(&run);
    }

    #[test]
    fn token_atomics_accumulate_usage_and_capture_cost() {
        let t = TokenAtomics::default();
        // Non-usage events are ignored; assistant usage accumulates; result cost
        // is stored (last wins), mirroring supervisor::TokenLedger.
        t.observe(&json!({"type":"system"}));
        t.observe(&json!({"type":"assistant","message":{"usage":{
            "input_tokens":1000,"output_tokens":200,"cache_read_input_tokens":4000}}}));
        t.observe(&json!({"type":"assistant","message":{"usage":{
            "input_tokens":50,"output_tokens":150}}}));
        t.observe(&json!({"type":"result","total_cost_usd":0.25}));
        t.observe(&json!({"type":"result","total_cost_usd":0.37})); // cumulative: last wins
        assert_eq!(t.total_tokens(), 1000 + 200 + 4000 + 50 + 150);
        let led = t.ledger();
        assert_eq!(led.turns, 2);
        assert_eq!(led.totals.cache_read, 4000);
        assert_eq!(t.cost_usd(), Some(0.37));
        // No cost reported -> None, never a bogus 0.0.
        assert!(TokenAtomics::default().cost_usd().is_none());
    }

    #[test]
    fn agent_run_enforces_token_budget() {
        let mem = mem_scratch("agenttok-mem");
        let run = scratch("agenttok-run");
        // The child emits ONE assistant usage event whose tokens (500) exceed the
        // 100-token budget, then sleeps: the reader taps the event, the poll loop
        // sees the ledger over budget and kills the run as `token_budget`.
        let line =
            r#"{"type":"assistant","message":{"usage":{"input_tokens":500,"output_tokens":0}}}"#;
        let (summary, ok) = agent_run(
            &mem,
            &run,
            AgentRunOpts {
                command: vec![
                    "sh".into(),
                    "-c".into(),
                    format!("printf '%s\\n' '{line}'; sleep 30"),
                ],
                budget_secs: None,
                max_calls: None,
                max_tokens: Some(100),
                session_id: Some("tok-sess".into()),
                provider: "fake".into(),
            },
        );
        assert!(ok);
        assert_eq!(summary["reason"], "token_budget");
        assert_eq!(summary["tokens"]["input"], 500);
        assert_eq!(summary["tokens"]["total"], 500);
        let _ = std::fs::remove_dir_all(&mem);
        let _ = std::fs::remove_dir_all(&run);
    }

    #[test]
    fn agent_run_returns_even_when_a_grandchild_holds_the_pipe() {
        // Review finding C2: on the CLEAN-exit path, a backgrounded grandchild
        // can inherit and hold the stdout pipe open, blocking the reader thread.
        // `sh` exits 0 immediately but leaves `sleep` holding the pipe; agent_run
        // must still RETURN (bounded drain -> group-kill cleanup), never hang.
        // The test completing at all IS the assertion (a regression would hang
        // until `sleep` exits, ~20s, past the harness timeout).
        let mem = mem_scratch("agentgc-mem");
        let run = scratch("agentgc-run");
        let start = std::time::Instant::now();
        let (summary, ok) = agent_run(
            &mem,
            &run,
            AgentRunOpts {
                command: vec!["sh".into(), "-c".into(), "sleep 20 & exit 0".into()],
                budget_secs: Some(60),
                max_calls: None,
                max_tokens: None,
                session_id: Some("gc-sess".into()),
                provider: "fake".into(),
            },
        );
        assert!(ok);
        assert_eq!(summary["reason"], "completed", "sh exited cleanly");
        assert!(
            start.elapsed() < std::time::Duration::from_secs(10),
            "must return via the bounded drain, not block on the grandchild"
        );
        assert!(!run.join("gc-sess.pid").exists(), "markers cleaned up");
        let _ = std::fs::remove_dir_all(&mem);
        let _ = std::fs::remove_dir_all(&run);
    }

    #[test]
    fn agent_stop_writes_marker_and_rejects_bad_id() {
        let run = scratch("agentstop");
        let (ok_v, ok) = agent_stop(&run, "sess1");
        assert!(ok);
        assert_eq!(ok_v["stopping"], "sess1");
        assert!(run.join("sess1.stop").exists());
        // Traversal / bad id refused.
        assert!(!agent_stop(&run, "../evil").1);
        let _ = std::fs::remove_dir_all(&run);
    }

    #[test]
    fn agent_run_refuses_empty_command_and_bad_session() {
        let mem = mem_scratch("agentbad-mem");
        let run = scratch("agentbad-run");
        assert!(
            !agent_run(
                &mem,
                &run,
                AgentRunOpts {
                    command: vec![],
                    budget_secs: None,
                    max_calls: None,
                    max_tokens: None,
                    session_id: None,
                    provider: "x".into(),
                },
            )
            .1
        );
        assert!(
            !agent_run(
                &mem,
                &run,
                AgentRunOpts {
                    command: vec!["true".into()],
                    budget_secs: None,
                    max_calls: None,
                    max_tokens: None,
                    session_id: Some("../evil".into()),
                    provider: "x".into(),
                },
            )
            .1
        );
        let _ = std::fs::remove_dir_all(&mem);
        let _ = std::fs::remove_dir_all(&run);
    }
}
