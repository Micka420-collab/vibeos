//! `vibed-tool` — the low-privilege ADR-019 helper.
//!
//! This binary is the `ExecStart` of the transient hardened service that
//! `vibed` spawns via `systemd-run` (see `vibed::sandbox`). It runs under a
//! distinct `DynamicUser` uid — never root, never the agent's uid — and is the
//! ONLY process that touches hostile input: it will (later increments) run the
//! deploy CLI or drive `chromium-headless`, decode the result, and hand `vibed`
//! back a bounded value over stdio. `vibed`, the policy engine, never shares its
//! address space with any of that.
//!
//! Kept deliberately thin: all logic lives in `vibed::sandbox::helper` so it is
//! unit-tested in the library. `main` only dispatches argv[1] and maps the
//! result to stdout / a non-zero exit.
//!
//! Modes today: `probe` (report our own confinement — the on-target proof the
//! ADR-019 profile applied). `deploy` / `browser` land with their increments.

use std::io::Write;

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_default();
    match vibed::sandbox::helper::run(&mode) {
        Ok(report) => {
            // Bounded result on stdout (the systemd-run `--pipe` result channel).
            // systemd-run's own chatter goes to stderr, so this stays clean.
            let mut out = std::io::stdout();
            let _ = writeln!(out, "{report}");
            let _ = out.flush();
        }
        Err(e) => {
            // Diagnostics on stderr (vibed captures it SEPARATELY from the
            // result channel). Exit 64 (EX_USAGE) so a helper-level refusal is
            // distinguishable from systemd-run's own rc=1 (timeout / missing
            // binary), which vibed cannot otherwise tell apart by code alone.
            eprintln!("vibed-tool: {e}");
            std::process::exit(64);
        }
    }
}
