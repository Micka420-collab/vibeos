//! `vibectl` core — the operator/admin CLI logic, kept in the library so it is
//! unit-testable without spawning the binary. The thin front-end lives in
//! `src/bin/vibectl.rs`.
//!
//! v0.1 perimeter: read-only memory status and audit-chain verification, the
//! operator side of the approval flow (`approve`/`deny`, root-gated), and the
//! **agent supervisor** (`agent run`/`stop`/`thinking`) that runs a CLI in
//! structured mode and taps its reasoning stream (ADR-012/013). Truly
//! destructive actions (factory reset = T3) are deliberately NOT here yet — they
//! require the full human-approval flow (Phase 4) and must never be a bare CLI
//! switch.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use crate::{approval, audit, mcp, reasoning, supervisor};

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

/// Only the operator (root) may grant or deny approvals. The approval store is
/// already root-only at the filesystem level; this makes the trust boundary
/// explicit and turns a would-be opaque "permission denied" into a clear
/// message. Fail-closed: if the euid cannot be determined, refuse.
fn require_root(euid: Option<u32>) -> Result<(), Value> {
    match euid {
        Some(0) => Ok(()),
        Some(uid) => Err(json!({
            "error": format!("must be root to approve/deny approvals (euid={uid}); use sudo")
        })),
        None => Err(json!({
            "error": "cannot determine caller euid (/proc unavailable); refusing to approve/deny"
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
    let budget = supervisor::Budget::new(opts.budget_secs, opts.max_calls);

    // START journal event (reserved `autonomous_session` type — unforgeable by agents).
    write_session_journal(
        mem,
        &supervisor::session_journal_event(
            &sid,
            "start",
            &opts.provider,
            &mcp::utc_iso8601(start),
            json!({ "command": opts.command, "budget_secs": opts.budget_secs, "max_calls": opts.max_calls }),
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
    let reader_done = Arc::new(AtomicBool::new(false));
    if let Some(stdout) = child.stdout.take() {
        let mem = mem.to_path_buf();
        let sid = sid.clone();
        let tool_calls = Arc::clone(&tool_calls);
        let blocks = Arc::clone(&blocks);
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
        let stopped = stopfile.exists();
        if over_wall || over_calls || stopped {
            reason = if stopped {
                "operator_stop"
            } else if over_wall {
                "wall_budget"
            } else {
                "tool_budget"
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

    let end = now_epoch_secs();
    write_session_journal(
        mem,
        &supervisor::session_journal_event(
            &sid,
            "end",
            &opts.provider,
            &mcp::utc_iso8601(end),
            json!({ "reason": reason, "reasoning_blocks": blocks, "tool_calls": calls,
                    "duration_secs": end.saturating_sub(start) }),
        ),
        end,
    );
    let _ = std::fs::remove_file(&pidfile);
    let _ = std::fs::remove_file(&stopfile);

    (
        json!({ "session": sid, "reason": reason, "reasoning_blocks": blocks,
                "tool_calls": calls, "duration_secs": end.saturating_sub(start) }),
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
