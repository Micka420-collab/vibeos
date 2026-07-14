//! Integration test: load the REAL policy shipped in the repository
//! (`security/policy.d/default.toml`, installed in the image as
//! `/etc/vibeos/policy.d/default.toml`) and assert the canonical decisions.
//!
//! This is the CI guard against the historical failure mode where the shipped
//! policy used a schema the engine could not parse and was silently ignored:
//! any drift between `security/policy.d/*.toml` and the engine now fails the
//! build (the loader is fail-closed).

use std::path::PathBuf;

use vibed::policy::{CallContext, Decision, PolicyEngine, Tier};

fn repo_policy_dir() -> PathBuf {
    // CARGO_MANIFEST_DIR = <repo>/vibed at compile time.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("vibed/ has a repo root parent")
        .join("security")
        .join("policy.d")
}

const NO_CTX: CallContext<'_> = CallContext { path: None, service: None };

#[test]
fn shipped_default_policy_loads_with_rules() {
    let dir = repo_policy_dir();
    assert!(dir.is_dir(), "missing {} — repository layout changed?", dir.display());
    let engine = PolicyEngine::load_dir(&dir)
        .unwrap_or_else(|e| panic!("shipped policy must load (fail-closed engine): {e}"));
    assert!(
        engine.rule_count() > 0,
        "shipped policy must contribute at least one rule"
    );
}

#[test]
fn shipped_default_policy_canonical_decisions() {
    let engine = PolicyEngine::load_dir(&repo_policy_dir()).expect("shipped policy must load");

    // T0 observation is allowed.
    assert_eq!(
        engine.evaluate("os.status", Some(Tier::T0), NO_CTX),
        Decision::Allow,
        "os.status (T0) must be allowed by the shipped policy"
    );

    // The memory tools: T0 read and T1 governed append are both allowed.
    assert_eq!(
        engine.evaluate("memory.query", Some(Tier::T0), NO_CTX),
        Decision::Allow,
        "memory.query (T0) must be allowed by the shipped policy"
    );
    assert_eq!(
        engine.evaluate("memory.append", Some(Tier::T1), NO_CTX),
        Decision::Allow,
        "memory.append (T1) must be allowed by the shipped policy"
    );

    // T2 is a floor: allow + approval=human => RequireApproval, never Allow.
    assert_eq!(
        engine.evaluate("pkg.install", Some(Tier::T2), NO_CTX),
        Decision::RequireApproval,
        "pkg.install (T2) must require human approval"
    );

    // Absolute default-deny, both flavors:
    // - a tool absent from the registry (no tier) is denied outright;
    assert_eq!(
        engine.evaluate("disk.wipe", None, NO_CTX),
        Decision::Deny,
        "a tool without a registry tier must be denied"
    );
    // - a registered-looking tool that no rule (or only the catch-all deny)
    //   matches is denied as well.
    assert_eq!(
        engine.evaluate("totally.unknown.tool", Some(Tier::T0), NO_CTX),
        Decision::Deny,
        "a tool matched by no allow rule must be denied"
    );
}
