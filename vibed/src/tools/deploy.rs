//! `deploy.*` — governed production deployment (ADR-021).
//!
//! This module is the **pure, inert** foundation of the deploy tools: it maps a
//! `(provider, target)` to the exact CLI command that reads a deployment's state
//! — nothing here spawns a process or touches a token. The dangerous execution
//! (running the CLI inside the ADR-019 sandbox with a sealed credential) lands in
//! a later, separately-reviewed increment; the tools are not wired into the MCP
//! catalog yet.
//!
//! The `[rule.deploy]` verdict (policy.rs) already gates WHICH `(provider,
//! target)` an agent may reach; this module builds the READ-ONLY "plan" command
//! for an allowed target. Every command is grounded in the verified 2026 CLI
//! facts (fly/vercel/railway docs + source):
//!   * the **token is NEVER an argv element** (ADR-021 lock 1) — it is passed to
//!     the CLI only through the env var named by [`DeployCommand::token_env`],
//!     set by the helper from the sealed credential;
//!   * the target is pinned by its **immutable id** (Fly app name — Fly has no
//!     rename; Vercel `prj_…`; Railway project token resolves its own project);
//!   * output is machine-readable (`--json`, or `vercel api` whose `list`/
//!     `inspect` lack `--json`);
//!   * telemetry/update endpoints (e.g. `flyctl-metrics.fly.dev`) are a side
//!     effect only over the network, so the sandbox's deny-all egress floor
//!     already blocks them — we simply never add them to the allow-list.

// Inert this increment: `plan_command` is exercised by the tests and becomes the
// entry the `deploy.plan` handler calls in the next (execution) increment. The
// allow is removed then; kept narrow to this foundation module.
#![allow(dead_code)]

/// The concrete CLI invocation for a read-only deploy "plan", minus the token
/// (which the helper injects via the env var). Binary is a NAME; the spawn layer
/// resolves it to the CLI shipped/bind-mounted into the sandbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeployCommand {
    /// CLI binary name (resolved to a path by the spawn layer).
    pub binary: &'static str,
    /// argv AFTER the binary. Never contains the token.
    pub argv: Vec<String>,
    /// The env var the CLI reads the token from (the ONLY channel the token
    /// travels — never argv, never a flag).
    pub token_env: &'static str,
}

/// Build the READ-ONLY plan command for `(provider, target)`. Fails closed on an
/// unknown provider or an unsafe target id. Read-only by construction: `status`
/// / `vercel api` GET only — no `deploy`, no state mutation.
pub(crate) fn plan_command(provider: &str, target: &str) -> Result<DeployCommand, String> {
    validate_target(target)?;
    match provider {
        // `fly status -a <app> --json` — read-only; the app NAME is the immutable
        // id (Fly has no rename). Token via FLY_API_TOKEN; a read-only org token
        // (`fly tokens create readonly`) is the recommended seal for this.
        "fly" => Ok(DeployCommand {
            binary: "flyctl",
            argv: vec![
                "status".to_string(),
                "-a".to_string(),
                target.to_string(),
                "--json".to_string(),
            ],
            token_env: "FLY_API_TOKEN",
        }),
        // `vercel api /v9/projects/<prj_id>` — the raw-JSON GET path. `list` and
        // `inspect` lack `--json`, and running the CLI in an unlinked cwd would
        // write a `.vercel/` dir; `vercel api` avoids both (targets by id, no cwd
        // linking). Token via VERCEL_TOKEN.
        "vercel" => Ok(DeployCommand {
            binary: "vercel",
            argv: vec!["api".to_string(), format!("/v9/projects/{target}")],
            token_env: "VERCEL_TOKEN",
        }),
        // `railway status --json` — with RAILWAY_TOKEN (a project-scoped token)
        // the CLI resolves its own project+environment from the token, so this
        // runs in an ephemeral HOME with no link and no flags. The target id
        // selects WHICH sealed project token is loaded; it is the identity anchor,
        // not an argv element.
        "railway" => Ok(DeployCommand {
            binary: "railway",
            argv: vec!["status".to_string(), "--json".to_string()],
            token_env: "RAILWAY_TOKEN",
        }),
        other => Err(format!(
            "deploy.plan: unknown provider {other:?} (expected fly, vercel, railway)"
        )),
    }
}

/// A target id is pinned into an argv element (fly `-a <target>`) or a URL path
/// (`vercel api /v9/projects/<target>`), so it must be a conservative id — no
/// whitespace, no `/`, no shell/flag/path shapes. `Command` never shell-splits,
/// so this guards against flag- and path-injection, not shell injection.
fn validate_target(target: &str) -> Result<(), String> {
    if target.is_empty() || target.len() > 128 {
        return Err("deploy target must be 1..=128 chars".to_string());
    }
    if target.starts_with('-') {
        return Err(format!("invalid deploy target {target:?} (leading '-')"));
    }
    // `.` is allowed in ids, but `..` is a path-traversal shape in the
    // `vercel api /v9/projects/<target>` URL — reject it outright.
    if target.contains("..") {
        return Err(format!("invalid deploy target {target:?} (contains '..')"));
    }
    if !target
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err(format!(
            "invalid deploy target {target:?} (allowed: alphanumerics and ._-)"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fly_plan_is_read_only_json_targeted_by_id_with_token_in_env() {
        let c = plan_command("fly", "my-app").unwrap();
        assert_eq!(c.binary, "flyctl");
        assert_eq!(c.argv, ["status", "-a", "my-app", "--json"]);
        assert_eq!(c.token_env, "FLY_API_TOKEN");
        // The token is NEVER an argv element (ADR-021 lock 1).
        assert!(!c
            .argv
            .iter()
            .any(|a| a.contains("token") || a.starts_with("--access")));
    }

    #[test]
    fn vercel_plan_uses_api_get_by_project_id() {
        let c = plan_command("vercel", "prj_abc123").unwrap();
        assert_eq!(c.binary, "vercel");
        assert_eq!(c.argv, ["api", "/v9/projects/prj_abc123"]);
        assert_eq!(c.token_env, "VERCEL_TOKEN");
    }

    #[test]
    fn railway_plan_lets_the_token_resolve_the_project() {
        let c = plan_command("railway", "svc-xyz").unwrap();
        assert_eq!(c.binary, "railway");
        assert_eq!(c.argv, ["status", "--json"]);
        assert_eq!(c.token_env, "RAILWAY_TOKEN");
    }

    #[test]
    fn no_command_carries_the_token_in_argv() {
        for (provider, target) in [("fly", "a"), ("vercel", "prj_a"), ("railway", "p")] {
            let c = plan_command(provider, target).unwrap();
            assert!(
                !c.argv.iter().any(|a| a.to_lowercase().contains("token")),
                "{provider}: token must never appear in argv: {:?}",
                c.argv
            );
        }
    }

    #[test]
    fn unknown_provider_is_refused() {
        assert!(plan_command("aws", "x")
            .unwrap_err()
            .contains("unknown provider"));
    }

    #[test]
    fn an_unsafe_target_is_refused_before_building_a_command() {
        for bad in [
            "", "-a", "--app", "app id", "a/b", "app;rm", "app\n", "app$(x)", "..",
        ] {
            assert!(
                plan_command("fly", bad).is_err(),
                "unsafe target {bad:?} must be refused"
            );
        }
        // A very long target is refused.
        assert!(plan_command("fly", &"a".repeat(129)).is_err());
        // Real-shaped ids pass.
        assert!(plan_command("fly", "my-app-123").is_ok());
        assert!(plan_command("vercel", "prj_aBc.123_x").is_ok());
    }
}
