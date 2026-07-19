//! `sandbox.probe` (T1) — the on-target proof that the ADR-019 sandbox confines.
//!
//! This is the FIRST tool to actually drive the ADR-019 pipeline end to end:
//! `vibed` (root) compiles a hardening profile (`crate::sandbox`), spawns a
//! transient `systemd-run` service that exec's the low-privilege `vibed-tool`
//! helper in `probe` mode, captures the helper's bounded confinement report, and
//! **judges** it against what the chosen profile must produce. On a real booted
//! VibeOS host a `confined: false` verdict means the sandbox did NOT take — the
//! single most important thing to verify before wiring a tool that runs a hostile
//! CLI or a browser through the same path.
//!
//! It is deliberately benign: no credential, no network beyond the deny-floor, no
//! target, no persistent state — a short-lived self-test, hence T1 (local,
//! ephemeral) rather than an approval-gated tier. The heavy lifting (the profile,
//! the spawn, the capture) is the already-reviewed `crate::sandbox`; this module
//! only picks a representative spec, runs it, and grades the result.

use crate::sandbox::{ToolClass, TransientUnit, UnitSpec};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};

/// systemd's `DynamicUser` allocation range (Fedora). A probe whose uid is
/// outside this ran WITHOUT `DynamicUser=yes` taking — a red flag.
const DYNAMIC_UID_LO: u64 = 61184;
const DYNAMIC_UID_HI: u64 = 65519;

const SYSTEMD_RUN_BIN: &str = "/usr/bin/systemd-run";
const VIBED_TOOL_BIN: &str = "/usr/libexec/vibed-tool";

/// Per-process monotonic counter for the invocation nonce. Uniqueness within a
/// `vibed` run is enough: the runtime dir is reclaimed when the transient unit
/// stops, so an id can never collide with a live one.
static PROBE_SEQ: AtomicU64 = AtomicU64::new(0);

/// sandbox.probe (T1): run the confinement self-test for one profile class.
pub(crate) fn sandbox_probe(args: &Value) -> Result<String, String> {
    sandbox_probe_with(args, SYSTEMD_RUN_BIN, VIBED_TOOL_BIN)
}

/// Backend with the binaries injected, so the full compile→spawn→grade chain can
/// be tested against a fake `systemd-run` (which stands in for the whole
/// pipeline by emitting a canned probe report) — the `tools/svc.rs` discipline.
fn sandbox_probe_with(
    args: &Value,
    systemd_run_bin: &str,
    helper_bin: &str,
) -> Result<String, String> {
    let class_name = args
        .get("class")
        .and_then(Value::as_str)
        .unwrap_or("deploy");
    let class = match class_name {
        // A Go CLI profile is the strict one (deny-all namespaces, W^X); it is the
        // right default to prove maximum confinement.
        "deploy" => ToolClass::Deploy { needs_wx: false },
        "browser" => ToolClass::Browser,
        other => {
            return Err(format!(
                "sandbox.probe: unknown class {other:?} (want \"deploy\" or \"browser\")"
            ))
        }
    };

    let nonce = PROBE_SEQ.fetch_add(1, Ordering::Relaxed);
    let spec = UnitSpec {
        invocation_id: format!("probe-{nonce}"),
        credential: None,         // a probe carries no secret — nothing to seal
        egress_allow: Vec::new(), // no network needed; the deny-floor stands
        memory_max: "128M".to_string(),
        runtime_max_sec: 30,
        workspace: None,
    };
    let unit = TransientUnit::compile(class, &spec)?;

    // The probe ignores stdin (it only reads its own /proc); an empty control
    // payload just closes the channel cleanly.
    let out = crate::sandbox::spawn_transient(
        systemd_run_bin,
        &unit,
        helper_bin,
        &["probe".to_string()],
        b"",
    )?;

    if out.status != Some(0) {
        return Err(format!(
            "sandbox.probe: the transient service exited {:?} (diagnostics: {})",
            out.status,
            out.stderr.trim()
        ));
    }

    let report: Value = serde_json::from_str(out.stdout.trim())
        .map_err(|e| format!("sandbox.probe: helper did not emit a JSON report: {e}"))?;

    let (confined, issues) = assess(class, &report);
    Ok(json!({
        "class": class_name,
        "confined": confined,
        "issues": issues,
        "report": report,
        "diagnostics": out.stderr.trim(),
    })
    .to_string())
}

/// Grade a probe report against what the profile class MUST produce. Returns
/// `(confined, issues)`; `confined` is true only when every check passes.
fn assess(class: ToolClass, r: &Value) -> (bool, Vec<String>) {
    let mut issues = Vec::new();

    // Distinct DynamicUser uid — the bearing control (ADR-021 lock 2).
    match r["uid"].as_u64() {
        Some(u) if (DYNAMIC_UID_LO..=DYNAMIC_UID_HI).contains(&u) => {}
        other => issues.push(format!(
            "uid {other:?} is not in the DynamicUser range {DYNAMIC_UID_LO}..={DYNAMIC_UID_HI} — DynamicUser did not take"
        )),
    }
    // A seccomp filter is installed (Seccomp: 2 = filter mode) — proves SystemCallFilter.
    if r["seccomp"].as_str() != Some("2") {
        issues.push(format!(
            "Seccomp is {:?}, want \"2\" (filter mode) — no syscall filter installed",
            r["seccomp"].as_str()
        ));
    }
    // Every capability dropped (CapBnd all-zero) — proves CapabilityBoundingSet=.
    if !is_all_zero_caps(r["cap_bnd"].as_str()) {
        issues.push(format!(
            "CapBnd {:?} is not all-zero — capabilities were not dropped",
            r["cap_bnd"].as_str()
        ));
    }
    // No new privileges, and the ephemeral HOME really took.
    if r["no_new_privs"].as_str() != Some("1") {
        issues.push(format!(
            "NoNewPrivs is {:?}, want \"1\"",
            r["no_new_privs"].as_str()
        ));
    }
    if r["home_ephemeral"].as_bool() != Some(true) {
        issues.push(
            "home_ephemeral is not true — the tmpfs HOME did not take (uid/mode mismatch?)"
                .to_string(),
        );
    }
    // Behavioural namespace proof, per class: deploy denies all namespace
    // creation (EPERM), the browser must be allowed to create a user namespace.
    let (want, why) = match class {
        ToolClass::Deploy { .. } => (
            "eperm",
            "deploy must DENY unprivileged user-namespace creation",
        ),
        ToolClass::Browser => (
            "ok",
            "browser must ALLOW user-namespace creation (Chromium's sandbox)",
        ),
    };
    if r["unshare_user"].as_str() != Some(want) {
        issues.push(format!(
            "unshare_user is {:?}, want {want:?} — {why}",
            r["unshare_user"].as_str()
        ));
    }

    (issues.is_empty(), issues)
}

/// A `/proc/self/status` `CapBnd`/`CapEff` field is a hex mask; "dropped" means
/// every hex digit is `0` (and the field is present).
fn is_all_zero_caps(field: Option<&str>) -> bool {
    matches!(field, Some(s) if !s.is_empty() && s.chars().all(|c| c == '0'))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A canned "fully confined deploy" report — what a correctly-sandboxed probe
    /// emits on target.
    fn confined_deploy_report() -> String {
        json!({
            "tool": "vibed-tool", "mode": "probe",
            "uid": 64152, "euid": 64152,
            "seccomp": "2", "seccomp_filters": "31",
            "cap_bnd": "0000000000000000", "cap_eff": "0000000000000000",
            "no_new_privs": "1", "umask": "0077",
            "ns_mnt": "mnt:[4026532999]",
            "home_ephemeral": true, "unshare_user": "eperm"
        })
        .to_string()
    }

    /// Write a fake `systemd-run` that ignores its args and simply emits `report`
    /// on stdout (standing in for the whole systemd-run→helper pipeline), with a
    /// chosen exit code. Shell builtins only (runs under env_clear).
    #[cfg(unix)]
    fn fake_pipeline(dir: &std::path::Path, report: &str, exit: i32) -> String {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("systemd-run");
        // The report is single-line JSON; embed it directly via a single-quoted
        // heredoc-free printf. It contains no single quotes.
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' '{report}'\nexit {exit}\n",
            report = report,
            exit = exit,
        );
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path.to_string_lossy().into_owned()
    }

    #[cfg(unix)]
    fn tmp(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("vibed-probe-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[cfg(unix)]
    fn probe_retry(args: &Value, bin: &str) -> Result<String, String> {
        for _ in 0..50 {
            match sandbox_probe_with(args, bin, VIBED_TOOL_BIN) {
                Err(e) if e.contains("Text file busy") => {
                    std::thread::sleep(std::time::Duration::from_millis(20))
                }
                other => return other,
            }
        }
        sandbox_probe_with(args, bin, VIBED_TOOL_BIN)
    }

    #[cfg(unix)]
    #[test]
    fn a_fully_confined_deploy_report_is_graded_confined() {
        let dir = tmp("ok");
        let bin = fake_pipeline(&dir, &confined_deploy_report(), 0);
        let out = probe_retry(&json!({"class": "deploy"}), &bin).expect("probe runs");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            v["confined"], true,
            "a fully-confined report must grade confined: {out}"
        );
        assert!(v["issues"].as_array().unwrap().is_empty());
        assert_eq!(v["class"], "deploy");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn an_unconfined_report_is_graded_not_confined_with_reasons() {
        // uid = the agent's (1000), no seccomp, caps present, unshare allowed:
        // exactly what a sandbox that FAILED to take looks like.
        let bad = json!({
            "uid": 1000, "euid": 1000,
            "seccomp": "0", "cap_bnd": "000001ffffffffff",
            "no_new_privs": "0", "home_ephemeral": false,
            "unshare_user": "ok"
        })
        .to_string();
        let dir = tmp("bad");
        let bin = fake_pipeline(&dir, &bad, 0);
        let out = probe_retry(&json!({"class": "deploy"}), &bin).expect("probe runs");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            v["confined"], false,
            "an unconfined report must NOT grade confined"
        );
        let issues = v["issues"].as_array().unwrap();
        assert!(
            issues.len() >= 5,
            "every failed check should be reported: {out}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn browser_class_wants_unshare_ok_not_eperm() {
        // The confined-deploy report has unshare_user=eperm; graded as BROWSER it
        // must flag that (the browser needs userns creation to SUCCEED).
        let dir = tmp("browser");
        let bin = fake_pipeline(&dir, &confined_deploy_report(), 0);
        let out = probe_retry(&json!({"class": "browser"}), &bin).expect("probe runs");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["confined"], false);
        let issues = v["issues"].as_array().unwrap();
        assert!(
            issues
                .iter()
                .any(|i| i.as_str().unwrap().contains("unshare_user")),
            "browser must flag unshare_user=eperm: {out}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_nonzero_service_exit_is_an_error() {
        let dir = tmp("exit");
        let bin = fake_pipeline(&dir, "", 1);
        let err =
            probe_retry(&json!({"class": "deploy"}), &bin).expect_err("nonzero exit must error");
        assert!(err.contains("exited"), "must surface the failure: {err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unknown_class_is_refused() {
        let err = sandbox_probe_with(&json!({"class": "wat"}), "/nonexistent", "/nonexistent")
            .expect_err("unknown class must be refused before spawning");
        assert!(err.contains("unknown class"));
    }
}
