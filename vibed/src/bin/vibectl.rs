//! vibectl — VibeOS operator CLI (thin front-end; logic in `vibed::vibectl`).
//!
//! v0.1 perimeter (ROADMAP Phase 3 "Ébauche de vibectl"): READ-ONLY.
//!   vibectl memory status                 — memory store summary (JSON)
//!   vibectl memory mode                    — current amnesic/persistent mode
//!   vibectl audit verify [path]            — verify the audit hash chain
//!
//! Destructive actions (factory reset = T3) are intentionally absent: they
//! require the Phase 4 human-approval flow and must never be a bare switch.

use std::path::Path;
use std::process::ExitCode;

use vibed::audit::DEFAULT_AUDIT_DIR;
use vibed::mcp::MEMORY_DIR;
use vibed::vibectl;

fn usage() -> ExitCode {
    eprintln!(
        "vibectl — VibeOS operator CLI (read-only in v0.1)\n\
         \n\
         USAGE:\n\
         \x20 vibectl memory status         memory store summary (JSON)\n\
         \x20 vibectl memory mode           current memory mode (amnesic|persistent)\n\
         \x20 vibectl audit verify [PATH]   verify the tamper-evident audit chain\n"
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
        ["-h"] | ["--help"] | ["help"] => {
            usage();
            ExitCode::SUCCESS
        }
        _ => usage(),
    }
}
