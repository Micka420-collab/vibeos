//! Shared unit-test helpers for the vibed crate's in-module tests.
//!
//! Both `mcp::tests` and `tools::fs::tests` build `PolicyEngine`s and
//! synthesize `Caller`s the same way; keeping the constructors in one place
//! avoids drift. Compiled only under `cfg(test)` (see the `mod` in `lib.rs`).

use crate::audit::Caller;
use crate::policy::PolicyEngine;

pub(crate) fn empty_policy() -> PolicyEngine {
    PolicyEngine::from_rules(Vec::new())
}

/// Build a `PolicyEngine` from inline TOML by loading it through the real
/// fail-closed loader (writes a temp drop-in, loads it, cleans up).
pub(crate) fn policy_from_toml(toml_src: &str) -> PolicyEngine {
    use std::sync::atomic::{AtomicU32, Ordering};
    static SEQ: AtomicU32 = AtomicU32::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("vibed-mcp-pol-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create policy test dir");
    std::fs::write(dir.join("00-test.toml"), toml_src).expect("write policy");
    let engine = PolicyEngine::load_dir(&dir).expect("policy loads");
    let _ = std::fs::remove_dir_all(&dir);
    engine
}

/// A policy that mirrors the shipped fs.read/fs.write allow rules so the
/// canonical re-check passes on legitimate in-home paths.
pub(crate) fn permissive_policy() -> PolicyEngine {
    policy_from_toml(
        r#"
        [[rule]]
        id = "fs-read"
        tools = ["fs.read"]
        tier = "T0"
        action = "allow"

        [[rule]]
        id = "fs-write"
        tools = ["fs.write"]
        tier = "T1"
        action = "allow"
        [rule.paths]
        allowed = ["/home/**", "/var/home/**"]
        "#,
    )
}

/// Real uid of the test process, read from `/proc/self/status` (no libc).
pub(crate) fn current_uid() -> u32 {
    let status = std::fs::read_to_string("/proc/self/status").expect("read /proc/self/status");
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("Uid:") {
            let first = rest.split_whitespace().next().expect("uid field present");
            return first.parse().expect("uid parses");
        }
    }
    panic!("no Uid line in /proc/self/status");
}

pub(crate) fn caller_uid(uid: u32) -> Caller {
    Caller {
        uid: Some(uid),
        gid: None,
        pid: None,
    }
}
