//! `deploy.*` — governed production deployment (ADR-021).
//!
//! This module delivers `deploy.plan` (T2): read a deployment's current state,
//! READ-ONLY and governed. The layering keeps the dangerous surface small:
//!   * [`plan_command`] is **pure** — it maps a `(provider, target)` to the exact
//!     token-free CLI argv that reads state, and validates the target. No process,
//!     no token, no I/O; trivially testable.
//!   * [`deploy_plan`] runs that command inside the ADR-019 sandbox (a transient
//!     hardened `DynamicUser` service via the low-privilege `vibed-tool` helper)
//!     with the TPM2-sealed READ-ONLY token loaded from `$CREDENTIALS_DIRECTORY`,
//!     never argv. vibed never shares an address space with the CLI.
//!
//! Governance gates the call BEFORE we get here: the `[rule.deploy]` verdict
//! (policy.rs) decides WHICH `(provider, target)` an agent may reach, and T2 makes
//! it human-approved. The shipped policy carries no deploy rule, so deploy.plan is
//! denied until the operator adds one and provisions the sealed token + egress
//! CIDRs — fail-closed for the most dangerous capability.
//!
//! Every command is grounded in the verified 2026 CLI facts (fly/vercel/railway
//! docs + source):
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

use crate::sandbox::{Credential, ToolClass, TransientUnit, UnitSpec};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};

/// `systemd-run` and the low-privilege helper (injected for tests).
const SYSTEMD_RUN_BIN: &str = "/usr/bin/systemd-run";
const VIBED_TOOL_BIN: &str = "/usr/libexec/vibed-tool";
/// Operator config root: `creds/<provider>-<target>.cred` (TPM2-sealed read-only
/// tokens) and `<provider>.egress` (the provider API's CIDR allow-list). Provided
/// by Micka; deploy.plan fails closed until it exists.
const CONFIG_DIR: &str = "/etc/vibeos/deploy";
/// Where the deploy CLIs live for the sandbox (a system path the DynamicUser can
/// reach — NOT the user's ~/.local/bin, which the confined uid cannot see). The
/// CLIs must be provisioned here (image layer or a system install).
const CLI_DIR: &str = "/usr/libexec/vibeos/deploy-cli";

/// Per-process nonce for the transient unit's ephemeral HOME/runtime dir.
static SEQ: AtomicU64 = AtomicU64::new(0);

/// deploy.plan (T2): read a deployment's current state, governed. The
/// `[rule.deploy]` verdict has already confirmed this `(provider, target)` is
/// allowed and the human approved (T2 floor). Here we RUN the read-only CLI
/// command inside the ADR-019 sandbox with the sealed read-only token, and return
/// its bounded output. Read-only end to end: `plan_command` builds only a status/
/// GET command, the helper refuses a mutating subcommand, and the token itself
/// must be read-only-scoped (the real guarantee).
pub(crate) fn deploy_plan(args: &Value) -> Result<String, String> {
    deploy_plan_with(args, SYSTEMD_RUN_BIN, VIBED_TOOL_BIN, CONFIG_DIR, CLI_DIR)
}

/// Backend with the binaries and config/CLI dirs injected, so the whole
/// compile→spawn chain is testable against a fake `systemd-run` and a scratch
/// config tree.
fn deploy_plan_with(
    args: &Value,
    systemd_run_bin: &str,
    helper_bin: &str,
    config_dir: &str,
    cli_dir: &str,
) -> Result<String, String> {
    let provider = args
        .get("provider")
        .and_then(Value::as_str)
        .ok_or_else(|| "deploy.plan: missing 'provider'".to_string())?;
    let target = args
        .get("target")
        .and_then(Value::as_str)
        .ok_or_else(|| "deploy.plan: missing 'target'".to_string())?;
    // Validates provider + target and builds the read-only, token-free CLI argv.
    let cmd = plan_command(provider, target)?;

    // The CLI binary: an absolute path from the fixed CLI dir (never PATH). The
    // helper re-checks it is absolute.
    let binary = format!("{cli_dir}/{}", cmd.binary);

    // Egress backstop (ADR-021): the provider API's CIDRs, operator-provided.
    // Fail closed if absent — deploy acts on the network; no allow-list, no reach.
    // The real target/read-only control is the sealed token scope + the verdict.
    let egress = read_egress(config_dir, provider)?;

    // The sealed read-only token for this exact (provider, target).
    let cred_name = "deploy-token";
    let cred_path = format!("{config_dir}/creds/{provider}-{target}.cred");

    let nonce = SEQ.fetch_add(1, Ordering::Relaxed);
    let spec = UnitSpec {
        invocation_id: format!("deploy-{}-{nonce}", std::process::id()),
        credential: Some(Credential {
            name: cred_name.to_string(),
            path: cred_path,
        }),
        egress_allow: egress,
        memory_max: "256M".to_string(),
        runtime_max_sec: 120,
        // A read-only plan needs no project sources.
        workspace: None,
    };
    // flyctl is Go (W^X ok); vercel/railway are Node (V8 JIT needs W^X).
    let class = ToolClass::Deploy {
        needs_wx: provider != "fly",
    };
    let unit = TransientUnit::compile(class, &spec)?;

    // The command for the helper's `deploy` mode (the token is NOT here — the
    // helper loads it from $CREDENTIALS_DIRECTORY/deploy-token).
    let payload = json!({
        "binary": binary,
        "argv": cmd.argv,
        "token_env": cmd.token_env,
        "cred_name": cred_name,
    })
    .to_string();

    let out = crate::sandbox::spawn_transient(
        systemd_run_bin,
        &unit,
        helper_bin,
        &["deploy".to_string()],
        payload.as_bytes(),
    )?;
    if out.status != Some(0) {
        return Err(format!(
            "deploy.plan: the transient service exited {:?} (diagnostics: {})",
            out.status,
            out.stderr.chars().take(2048).collect::<String>()
        ));
    }
    // The helper's stdout is `{exit, stdout, stderr}` of the CLI — DATA, returned
    // verbatim (bounded already by spawn_transient), never interpreted as truth.
    Ok(out.stdout)
}

/// Read the operator's per-provider egress CIDR allow-list. `#` comments and
/// blank lines are ignored. Bad CIDRs are caught later by `TransientUnit::compile`.
fn read_egress(config_dir: &str, provider: &str) -> Result<Vec<String>, String> {
    let path = format!("{config_dir}/{provider}.egress");
    let content = std::fs::read_to_string(&path).map_err(|e| {
        format!(
            "deploy.plan: no egress allow-list at {path} ({e}) — provision the \
             provider API's CIDRs there (deploy needs network to reach the API)"
        )
    })?;
    let cidrs: Vec<String> = content
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_string)
        .collect();
    if cidrs.is_empty() {
        return Err(format!("deploy.plan: egress allow-list {path} is empty"));
    }
    Ok(cidrs)
}

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
    fn deploy_plan_rejects_missing_or_unknown_provider_target() {
        assert!(
            deploy_plan_with(&json!({"target": "x"}), "/x", "/x", "/tmp", "/x")
                .unwrap_err()
                .contains("missing 'provider'")
        );
        assert!(
            deploy_plan_with(&json!({"provider": "fly"}), "/x", "/x", "/tmp", "/x")
                .unwrap_err()
                .contains("missing 'target'")
        );
        assert!(deploy_plan_with(
            &json!({"provider": "aws", "target": "x"}),
            "/x",
            "/x",
            "/tmp",
            "/x"
        )
        .unwrap_err()
        .contains("unknown provider"));
    }

    #[test]
    fn deploy_plan_fails_closed_without_an_egress_allow_list() {
        // No `<provider>.egress` file => deploy cannot reach the API => refuse
        // BEFORE spawning anything.
        let err = deploy_plan_with(
            &json!({"provider": "fly", "target": "my-app"}),
            "/bin/false",
            "/x",
            "/nonexistent-vibeos-deploy-config",
            "/x",
        )
        .unwrap_err();
        assert!(err.contains("egress allow-list"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn deploy_plan_builds_the_control_payload_and_returns_the_cli_output() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("vibed-dplan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Operator egress allow-list for fly.
        std::fs::write(dir.join("fly.egress"), "# fly api\n66.241.124.0/24\n").unwrap();
        // Fake systemd-run: records the control payload, echoes a canned helper
        // result (what the helper's deploy mode would return for the CLI).
        let argv_m = dir.join("argv");
        let payload_m = dir.join("payload");
        let helper_result = r#"{"exit":0,"stdout":"running","stderr":""}"#;
        let fake = dir.join("systemd-run");
        let script = format!(
            "#!/bin/sh\necho \"$@\" > '{argv}'\nIFS= read -r p\nprintf '%s' \"$p\" > '{payload}'\nprintf '%s\\n' '{result}'\nexit 0\n",
            argv = argv_m.display(),
            payload = payload_m.display(),
            result = helper_result,
        );
        std::fs::write(&fake, script).unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();

        let bin = fake.to_string_lossy().into_owned();
        let cfg = dir.to_string_lossy().into_owned();
        let args = json!({"provider": "fly", "target": "my-app"});
        let out = (|| {
            for _ in 0..50 {
                match deploy_plan_with(
                    &args,
                    &bin,
                    "/usr/libexec/vibed-tool",
                    &cfg,
                    "/usr/libexec/vibeos/deploy-cli",
                ) {
                    Err(e) if e.contains("Text file busy") => {
                        std::thread::sleep(std::time::Duration::from_millis(20))
                    }
                    other => return other,
                }
            }
            deploy_plan_with(
                &args,
                &bin,
                "/usr/libexec/vibed-tool",
                &cfg,
                "/usr/libexec/vibeos/deploy-cli",
            )
        })()
        .expect("deploy_plan runs");
        // The handler returns the helper's stdout (the CLI output) verbatim.
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["exit"], 0);
        // The control payload carried the ABSOLUTE binary, the read-only argv, the
        // token env var and the credential name — never the token itself.
        let payload = std::fs::read_to_string(&payload_m).unwrap();
        let p: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(p["binary"], "/usr/libexec/vibeos/deploy-cli/flyctl");
        assert_eq!(p["token_env"], "FLY_API_TOKEN");
        assert_eq!(p["cred_name"], "deploy-token");
        let argv: Vec<&str> = p["argv"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_str().unwrap())
            .collect();
        assert_eq!(argv, ["status", "-a", "my-app", "--json"]);
        // The hardened profile reached systemd-run (DynamicUser).
        let cli_argv = std::fs::read_to_string(&argv_m).unwrap();
        assert!(
            cli_argv.contains("--property=DynamicUser=yes"),
            "profile: {cli_argv}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

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
