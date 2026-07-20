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

const NO_CTX: CallContext<'_> = CallContext {
    path: None,
    service: None,
    domain: None,
    deploy: None,
};

#[test]
fn shipped_default_policy_loads_with_rules() {
    let dir = repo_policy_dir();
    assert!(
        dir.is_dir(),
        "missing {} — repository layout changed?",
        dir.display()
    );
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

    // fs.list shares the fs-read rule: same T0 read-only surface.
    assert_eq!(
        engine.evaluate("fs.list", Some(Tier::T0), NO_CTX),
        Decision::Allow,
        "fs.list (T0) must be allowed by the shipped policy"
    );

    // svc.status: read-only unit state is T0 observation.
    assert_eq!(
        engine.evaluate("svc.status", Some(Tier::T0), NO_CTX),
        Decision::Allow,
        "svc.status (T0) must be allowed by the shipped policy"
    );

    // sectools.list: read-only toolkit discovery is T0.
    assert_eq!(
        engine.evaluate("sectools.list", Some(Tier::T0), NO_CTX),
        Decision::Allow,
        "sectools.list (T0) must be allowed by the shipped policy"
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

    // sandbox.probe: the ADR-019 confinement self-test is T1 and MUST be allowed
    // by the shipped policy — otherwise the tool is dead behind the catch-all
    // default-deny (it has no target/service, so no context is needed to reach
    // its rule). This is the regression the missing rule would have shipped.
    assert_eq!(
        engine.evaluate("sandbox.probe", Some(Tier::T1), NO_CTX),
        Decision::Allow,
        "sandbox.probe (T1) must be allowed by the shipped policy"
    );

    // policy.capabilities (T0): the derived capability manifest must be readable by
    // the shipped policy — otherwise the planning aid is dead behind default-deny.
    assert_eq!(
        engine.evaluate("policy.capabilities", Some(Tier::T0), NO_CTX),
        Decision::Allow,
        "policy.capabilities (T0) must be allowed by the shipped policy"
    );

    // deploy.plan is T2 and DENIED by the shipped policy: it carries NO
    // [rule.deploy] rule, so a deploy is refused until the operator adds one with
    // their allowed (provider, target) pairs and provisions the sealed token +
    // egress. Fail-closed for the most dangerous capability.
    assert_eq!(
        engine.evaluate("deploy.plan", Some(Tier::T2), NO_CTX),
        Decision::Deny,
        "deploy.plan must be DENIED by the shipped policy (no [rule.deploy] rule)"
    );

    // Agent observability tools are T0 read-only and MUST be allowed by the
    // shipped policy — otherwise the HUD roster / reasoning discovery is dead
    // behind the catch-all default-deny.
    for tool in ["agent.thinking", "agent.sessions", "agents.list"] {
        assert_eq!(
            engine.evaluate(tool, Some(Tier::T0), NO_CTX),
            Decision::Allow,
            "{tool} (T0) must be allowed by the shipped policy"
        );
    }

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

/// The SHIPPED policy must put access-, audit- and approval-critical units OUT
/// OF REACH of svc.restart: not "requires approval" but a hard Deny, decided
/// BEFORE the approval floor (policy.rs checks service.denied first). An agent
/// can never even queue an approval request to restart these.
#[test]
fn shipped_policy_denies_restart_of_critical_units_before_approval() {
    let engine = PolicyEngine::load_dir(&repo_policy_dir()).expect("shipped policy must load");
    let svc = |unit: &'static str| CallContext {
        service: Some(unit),
        ..Default::default()
    };

    // Every unit whose restart would cut the operator's access, the audit trail,
    // or the approval mechanism itself is DENIED outright by the shipped policy.
    for unit in [
        "vibed.service",              // the warden + approval store
        "vibeos-agent@alice.service", // the agent supervisor (glob match)
        "polkit.service",             // authorization backbone
        "systemd-journald.service",   // log pipeline
        "auditd.service",             // audit pipeline
        "sshd.service",               // remote access
        "NetworkManager.service",     // network / remote access
        "systemd-networkd.service",
        "display-manager.service", // the graphical approval session
        "sddm.service",
        "systemd-logind.service", // sessions
        "user@1000.service",      // the operator's per-user manager (glob)
        "dbus-broker.service",    // system bus
        "dbus.service",
        "dbus.socket", // the bus is socket-activated
        // Every VibeOS unit — an agent must never touch what governs it. The
        // egress warden in particular used to slip through: the old glob
        // `vibeos-agent@*.service` cannot match `vibeos-agent-egress@…` (it wants
        // `@` where the text has `-egress@`), so an agent could ask to restart
        // the unit that compiles its own egress allow-list.
        "vibeos-agent-egress@alice.service",
        "vibeos-agents-group.service",
        "vibeos-genesis.service",
        // Axis (3), the inverse of "cut the operator's access": `systemctl
        // restart` on an INACTIVE unit STARTS it. An approved `svc.restart
        // sshd.socket` reads as innocuous while it turns remote SSH ON.
        "sshd.socket",
    ] {
        assert_eq!(
            engine.evaluate("svc.restart", Some(Tier::T2), svc(unit)),
            Decision::Deny,
            "svc.restart on {unit} must be DENIED (not merely require approval)"
        );
    }

    // A non-critical unit still reaches the human-approval floor — the deny-list
    // is a floor of exclusions, not a blanket ban on the tool.
    assert_eq!(
        engine.evaluate("svc.restart", Some(Tier::T2), svc("my-app.service")),
        Decision::RequireApproval,
        "a non-critical unit must still require approval, not be silently allowed or denied"
    );
}
