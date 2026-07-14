//! vibectl — VibeOS operator CLI (thin front-end; logic in `vibed::vibectl`).
//!
//! Commands: read-only memory status (`memory status|mode|profile|projects`) and
//! audit-chain verification (`audit verify`); the operator side of the approval
//! flow (`approvals list`, `approve`/`deny`, root-gated); and the **agent
//! supervisor** (`agent run`/`stop`/`thinking`) that runs a CLI under budget and
//! taps its reasoning stream (ADR-012/013).
//!
//! Truly destructive actions (factory reset = T3) are intentionally absent: they
//! require the full Phase 4 human-approval flow and must never be a bare switch.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use vibed::audit::DEFAULT_AUDIT_DIR;
use vibed::mcp::MEMORY_DIR;
use vibed::vibectl;

/// Memory store root (agent supervisor). `VIBEOS_MEMORY_DIR` overrides the
/// default for tests / advanced setups; production leaves it unset.
fn memory_dir() -> PathBuf {
    std::env::var_os("VIBEOS_MEMORY_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(MEMORY_DIR))
}

/// Runtime directory for live-session pid/stop markers.
fn agent_run_dir() -> PathBuf {
    std::env::var_os("VIBEOS_AGENT_RUN_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/run/vibeos/agent"))
}

/// Print `(report, ok)` as pretty JSON and map `ok` to an exit code.
fn emit(report: serde_json::Value, ok: bool) -> ExitCode {
    println!(
        "{}",
        serde_json::to_string_pretty(&report).unwrap_or_default()
    );
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// `vibectl agent <run|stop|thinking> …` — the agent supervisor (ADR-012/013).
fn agent_dispatch(sub: &[&str]) -> ExitCode {
    match sub {
        ["run", rest @ ..] => {
            // Split flags from the command at `--`: `agent run [flags] -- <cmd...>`.
            let Some(dashdash) = rest.iter().position(|&a| a == "--") else {
                eprintln!("agent run: expected `-- <command>` (e.g. agent run --budget 8h -- claude -p ...)");
                return ExitCode::from(2);
            };
            let (flags, cmd) = (&rest[..dashdash], &rest[dashdash + 1..]);
            if cmd.is_empty() {
                eprintln!("agent run: no command after `--`");
                return ExitCode::from(2);
            }
            let mut budget_secs = None;
            let mut max_calls = None;
            let mut session_id = None;
            let mut provider = "cli".to_string();
            let mut i = 0;
            while i < flags.len() {
                match flags[i] {
                    "--budget" => {
                        // Reject a present-but-unparseable value rather than
                        // silently running unbounded.
                        match flags.get(i + 1) {
                            Some(v) => match vibed::supervisor::parse_duration(v) {
                                Some(secs) => budget_secs = Some(secs),
                                None => {
                                    eprintln!(
                                        "agent run: invalid --budget '{v}' (e.g. 8h, 30m, 45s)"
                                    );
                                    return ExitCode::from(2);
                                }
                            },
                            None => {
                                eprintln!("agent run: --budget needs a value");
                                return ExitCode::from(2);
                            }
                        }
                        i += 2;
                    }
                    "--calls" => {
                        match flags.get(i + 1).map(|s| s.parse::<u64>()) {
                            Some(Ok(n)) if n > 0 => max_calls = Some(n),
                            _ => {
                                eprintln!("agent run: --calls needs a positive integer");
                                return ExitCode::from(2);
                            }
                        }
                        i += 2;
                    }
                    "--session" => {
                        session_id = flags.get(i + 1).map(|s| s.to_string());
                        i += 2;
                    }
                    "--provider" => {
                        provider = flags.get(i + 1).map(|s| s.to_string()).unwrap_or(provider);
                        i += 2;
                    }
                    other => {
                        eprintln!("agent run: unknown flag '{other}'");
                        return ExitCode::from(2);
                    }
                }
            }
            if budget_secs.is_none() && max_calls.is_none() {
                eprintln!(
                    "agent run: WARNING — no --budget/--calls: this run is UNBOUNDED \
                     (only `agent stop` or the CLI exiting will end it)."
                );
            }
            let opts = vibectl::AgentRunOpts {
                command: cmd.iter().map(|s| s.to_string()).collect(),
                budget_secs,
                max_calls,
                session_id,
                provider,
            };
            let (report, ok) = vibectl::agent_run(&memory_dir(), &agent_run_dir(), opts);
            emit(report, ok)
        }
        ["stop", id] => {
            let (report, ok) = vibectl::agent_stop(&agent_run_dir(), id);
            emit(report, ok)
        }
        ["thinking", rest @ ..] => {
            let mut session = None;
            let mut tail = None;
            let mut since = None;
            let mut i = 0;
            while i < rest.len() {
                match rest[i] {
                    "--session" => {
                        session = rest.get(i + 1).copied();
                        i += 2;
                    }
                    "--tail" => {
                        tail = rest.get(i + 1).and_then(|s| s.parse().ok());
                        i += 2;
                    }
                    "--since" => {
                        since = rest.get(i + 1).and_then(|s| s.parse().ok());
                        i += 2;
                    }
                    s if session.is_none() && !s.starts_with('-') => {
                        session = Some(s);
                        i += 1;
                    }
                    other => {
                        eprintln!("agent thinking: unexpected argument '{other}'");
                        return ExitCode::from(2);
                    }
                }
            }
            let Some(sid) = session else {
                eprintln!("agent thinking: a session id is required (--session <id>)");
                return ExitCode::from(2);
            };
            let (report, ok) = vibectl::agent_thinking(&memory_dir(), sid, tail, since);
            emit(report, ok)
        }
        _ => usage(),
    }
}

fn usage() -> ExitCode {
    eprintln!(
        "vibectl — VibeOS operator CLI\n\
         \n\
         USAGE:\n\
         \x20 vibectl memory status         memory store summary (JSON)\n\
         \x20 vibectl memory mode           current memory mode (amnesic|persistent)\n\
         \x20 vibectl memory profile        current user profile (fold of updates)\n\
         \x20 vibectl memory projects       current project index (fold of updates)\n\
         \x20 vibectl audit verify [DIR]    verify the tamper-evident audit chain\n\
         \x20 vibectl approvals list        pending T2/T3 human-approval requests\n\
         \x20 vibectl approve <ID>          grant a pending request (root)\n\
         \x20 vibectl deny <ID>             reject a pending request (root)\n\
         \x20 vibectl agent run [--budget 8h] [--calls N] [--session ID] [--provider P] -- <cmd...>\n\
         \x20                               run a CLI under budget, tapping its reasoning (ADR-012)\n\
         \x20 vibectl agent stop <SESSION>  operator kill-switch for a running session\n\
         \x20 vibectl agent thinking <SESSION> [--tail N] [--since T]\n\
         \x20                               read a session's captured reasoning\n"
    );
    ExitCode::from(2)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let parts: Vec<&str> = args.iter().map(String::as_str).collect();
    match parts.as_slice() {
        ["memory", "status"] => {
            let status = vibectl::memory_status_at(
                Path::new(MEMORY_DIR),
                Path::new(vibectl::MEMORY_MODE_MARKER),
            );
            println!(
                "{}",
                serde_json::to_string_pretty(&status).unwrap_or_default()
            );
            ExitCode::SUCCESS
        }
        ["memory", "mode"] => {
            let status = vibectl::memory_status_at(
                Path::new(MEMORY_DIR),
                Path::new(vibectl::MEMORY_MODE_MARKER),
            );
            println!(
                "{}",
                status
                    .get("mode")
                    .and_then(|m| m.as_str())
                    .unwrap_or("unknown")
            );
            ExitCode::SUCCESS
        }
        ["memory", "profile"] => {
            let out = vibectl::memory_profile_at(Path::new(MEMORY_DIR));
            println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
            ExitCode::SUCCESS
        }
        ["memory", "projects"] => {
            let out = vibectl::memory_projects_at(Path::new(MEMORY_DIR));
            println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
            ExitCode::SUCCESS
        }
        ["audit", "verify"] | ["audit", "verify", _] => {
            let dir = parts.get(2).copied().unwrap_or(DEFAULT_AUDIT_DIR);
            let (report, ok) = vibectl::audit_verify(Path::new(dir));
            println!(
                "{}",
                serde_json::to_string_pretty(&report).unwrap_or_default()
            );
            if ok {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        ["approvals", "list"] => {
            println!(
                "{}",
                serde_json::to_string_pretty(&vibectl::approvals_list()).unwrap_or_default()
            );
            ExitCode::SUCCESS
        }
        ["approve", id] => {
            let (report, ok) = vibectl::approve(id);
            println!(
                "{}",
                serde_json::to_string_pretty(&report).unwrap_or_default()
            );
            if ok {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        ["deny", id] => {
            let (report, ok) = vibectl::deny(id);
            println!(
                "{}",
                serde_json::to_string_pretty(&report).unwrap_or_default()
            );
            if ok {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        ["agent", sub @ ..] => agent_dispatch(sub),
        ["-h"] | ["--help"] | ["help"] => {
            usage();
            ExitCode::SUCCESS
        }
        _ => usage(),
    }
}
