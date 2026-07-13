//! `svc.*` tools — systemd unit observation and restart.
//!
//! Extracted verbatim from `mcp.rs` (F6, mechanical split — zero behaviour
//! change): `svc.status` (T0, read-only `systemctl show`) and `svc.restart`
//! (T2, the real backend behind the human-approval flow). Self-contained:
//! strict unit-name validation, absolute `systemctl` path, cleared environment.

use serde_json::Value;

/// svc.status (T0): read-only state of one systemd unit via `systemctl show`.
///
/// Safety posture (vibed runs as root and this SPAWNS a process):
///   * the unit name is validated by `validate_unit_name` BEFORE anything
///     runs — conservative allow-list of characters, no `/`, no leading `-`,
///     bounded length — so neither options nor paths can be injected;
///   * `--` terminates option parsing as a second fence;
///   * systemctl is invoked by ABSOLUTE path with a cleared environment;
///   * `show` is a pure read (no state change), and its D-Bus client applies
///     its own timeout, so the spawn_blocking worker is not held hostage.
pub(crate) fn svc_status(args: &Value) -> Result<String, String> {
    let raw = args
        .get("unit")
        .and_then(Value::as_str)
        .ok_or_else(|| "missing 'unit' argument".to_string())?;
    let unit = validate_unit_name(raw)?;

    let output = std::process::Command::new("/usr/bin/systemctl")
        .env_clear()
        .args([
            "show",
            "--no-pager",
            "--property=Id,Description,LoadState,ActiveState,SubState,UnitFileState,ActiveEnterTimestamp",
            "--",
            &unit,
        ])
        .output()
        .map_err(|e| format!("svc.status: cannot run systemctl: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let snippet: String = stderr.chars().take(200).collect();
        return Err(format!(
            "svc.status {unit}: systemctl exited with {}: {}",
            output.status,
            snippet.trim()
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut payload = parse_systemctl_show(&stdout);
    payload.insert("unit".to_string(), Value::String(unit));
    Ok(Value::Object(payload).to_string())
}

/// Absolute path of the systemctl binary in the image. A constant (never a
/// PATH lookup) so a compromised environment can never redirect what vibed —
/// running as root — executes. Tests exercise `svc_restart_with` directly with
/// a fake binary instead of overriding this.
const SYSTEMCTL_BIN: &str = "/usr/bin/systemctl";

/// svc.restart (T2): ACTUALLY restart one systemd unit — the real backend
/// behind the human-approval flow (the v0.1 stub restarted nothing).
///
/// This body is only ever reached on the Allow path of `handle_tools_call`,
/// i.e. AFTER a one-shot operator grant for this exact `(svc.restart, unit,
/// uid)` has been consumed (or under a policy that Allows it outright). The
/// approval floor lives entirely upstream in the dispatcher — this function
/// never approves anything and cannot be reached by an agent that was refused.
///
/// Safety posture (vibed runs as root and this CHANGES system state):
///   * the unit name is validated by `validate_unit_name` BEFORE anything runs
///     (conservative allow-list, no `/`, no leading `-`, bounded length) so
///     neither options nor paths can be injected;
///   * `--` terminates option parsing as a second fence;
///   * systemctl is invoked by ABSOLUTE path with a cleared environment;
///   * the operation is bounded by systemd's OWN per-unit job timeout
///     (TimeoutStartSec/TimeoutStopSec, default 90 s each) — the same reliance
///     on systemctl's built-in bounding that `svc.status` documents; a runaway
///     unit cannot hang the worker indefinitely;
///   * on success the unit is READ BACK (`systemctl show`) and its fresh
///     ActiveState/SubState/ActiveEnterTimestamp are returned, so the caller
///     and the audit trail get PROOF the unit actually restarted, not just
///     "the command ran".
pub(crate) fn svc_restart(args: &Value) -> Result<String, String> {
    svc_restart_with(args, SYSTEMCTL_BIN)
}

/// Backend of `svc.restart`, with the systemctl binary injected so the full
/// require_approval -> approve -> execute chain can be tested hermetically
/// against a fake systemctl (no root, no real units, no global state).
fn svc_restart_with(args: &Value, systemctl: &str) -> Result<String, String> {
    let raw = args
        .get("unit")
        .and_then(Value::as_str)
        .ok_or_else(|| "missing 'unit' argument".to_string())?;
    let unit = validate_unit_name(raw)?;

    let output = std::process::Command::new(systemctl)
        .env_clear()
        .args(["restart", "--no-pager", "--", &unit])
        .output()
        .map_err(|e| format!("svc.restart: cannot run systemctl: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let snippet: String = stderr.chars().take(200).collect();
        return Err(format!(
            "svc.restart {unit}: systemctl exited with {}: {}",
            output.status,
            snippet.trim()
        ));
    }

    // Read the unit back so the result PROVES the restart (fresh
    // ActiveEnterTimestamp), rather than merely asserting the command exited 0.
    // A read-back failure does not undo the restart: report it as a soft note
    // rather than an error, so the caller still learns the action succeeded.
    let show = std::process::Command::new(systemctl)
        .env_clear()
        .args([
            "show",
            "--no-pager",
            "--property=Id,ActiveState,SubState,ActiveEnterTimestamp",
            "--",
            &unit,
        ])
        .output();
    let mut payload = match &show {
        Ok(out) if out.status.success() => {
            parse_systemctl_show(&String::from_utf8_lossy(&out.stdout))
        }
        _ => serde_json::Map::new(),
    };
    payload.insert("unit".to_string(), Value::String(unit));
    payload.insert("action".to_string(), Value::String("restarted".to_string()));
    Ok(Value::Object(payload).to_string())
}

/// Conservative systemd unit-name validation. Accepts `name`, `name.service`,
/// `template@instance.service`, `dbus.socket`, systemd-escaped names (`\x2d`);
/// appends `.service` when no type suffix is present (systemctl's own
/// convention). Refuses anything that could be an option or a path.
pub(crate) fn validate_unit_name(raw: &str) -> Result<String, String> {
    // Tool-agnostic messages: shared by svc.status (T0) and svc.restart (T2).
    if raw.is_empty() {
        return Err("'unit' must not be empty".to_string());
    }
    if raw.len() > 255 {
        return Err("unit name exceeds 255 characters".to_string());
    }
    if raw.starts_with('-') {
        return Err(format!("invalid unit name '{raw}' (leading '-')"));
    }
    let valid = raw
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '-' | '_' | '.' | '@' | '\\'));
    if !valid {
        return Err(format!(
            "invalid unit name '{raw}' (allowed: alphanumerics and :-_.@\\)"
        ));
    }
    Ok(if raw.contains('.') {
        raw.to_string()
    } else {
        format!("{raw}.service")
    })
}

/// Parse `systemctl show` KEY=VALUE lines into a JSON object with lowercase
/// snake_case keys. Pure function, unit-tested without spawning anything.
fn parse_systemctl_show(stdout: &str) -> serde_json::Map<String, Value> {
    const KEYS: [(&str, &str); 7] = [
        ("Id", "id"),
        ("Description", "description"),
        ("LoadState", "load_state"),
        ("ActiveState", "active_state"),
        ("SubState", "sub_state"),
        ("UnitFileState", "unit_file_state"),
        ("ActiveEnterTimestamp", "active_enter_timestamp"),
    ];
    let mut map = serde_json::Map::new();
    for line in stdout.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if let Some((_, out_key)) = KEYS.iter().find(|(k, _)| *k == key) {
            map.insert(out_key.to_string(), Value::String(value.to_string()));
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn unit_name_validation_accepts_real_units_and_appends_service() {
        assert_eq!(validate_unit_name("sshd").unwrap(), "sshd.service");
        assert_eq!(validate_unit_name("sshd.service").unwrap(), "sshd.service");
        assert_eq!(validate_unit_name("dbus.socket").unwrap(), "dbus.socket");
        assert_eq!(
            validate_unit_name("getty@tty1.service").unwrap(),
            "getty@tty1.service"
        );
        assert_eq!(
            validate_unit_name("systemd-journald").unwrap(),
            "systemd-journald.service"
        );
        // systemd-escaped names stay intact.
        assert_eq!(
            validate_unit_name("run-media-usb\\x2ddisk.mount").unwrap(),
            "run-media-usb\\x2ddisk.mount"
        );
    }

    #[test]
    fn unit_name_validation_refuses_injection_shapes() {
        for bad in [
            "",
            "-p",            // an option, not a unit
            "--property=Id", // ditto
            "../etc/passwd", // path traversal (contains '/')
            "/usr/bin/true", // absolute path
            "a unit",        // whitespace
            "unit;reboot",   // shell metacharacter
            "unit$(reboot)", // ditto
            "unit\nother",   // newline
        ] {
            assert!(
                validate_unit_name(bad).is_err(),
                "'{bad}' must be refused by unit-name validation"
            );
        }
        let oversized = "a".repeat(256);
        assert!(validate_unit_name(&oversized).is_err());
    }

    /// Write an executable fake `systemctl` that records a `restart` invocation
    /// to `marker` and answers `show` with canned unit properties, so the real
    /// svc.restart backend can be exercised without root or a live systemd.
    #[cfg(unix)]
    fn fake_systemctl(
        dir: &std::path::Path,
        marker: &std::path::Path,
        restart_exit: i32,
    ) -> String {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("systemctl");
        let script = format!(
            "#!/bin/sh\n\
             if [ \"$1\" = restart ]; then echo \"$@\" > '{marker}'; exit {restart_exit}; fi\n\
             if [ \"$1\" = show ]; then \
               printf 'Id=%s\\nActiveState=active\\nSubState=running\\nActiveEnterTimestamp=Mon 2026-07-13 21:00:00 UTC\\n' \"$5\"; \
               exit 0; fi\n\
             exit 2\n",
            marker = marker.display(),
        );
        std::fs::write(&path, script).expect("write fake systemctl");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("chmod fake systemctl");
        path.to_string_lossy().into_owned()
    }

    #[cfg(unix)]
    #[test]
    fn svc_restart_actually_invokes_systemctl_and_confirms_state() {
        let dir = std::env::temp_dir().join(format!("vibed-svcrestart-ok-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let marker = dir.join("restarted");
        let bin = fake_systemctl(&dir, &marker, 0);

        let out = svc_restart_with(&json!({"unit": "vibed"}), &bin).expect("restart succeeds");
        let v: Value = serde_json::from_str(&out).unwrap();

        // The backend really ran `systemctl restart -- vibed.service`...
        assert!(marker.exists(), "systemctl restart was never invoked");
        let recorded = std::fs::read_to_string(&marker).unwrap();
        assert!(
            recorded.contains("restart") && recorded.contains("vibed.service"),
            "restart was not called on the validated unit: {recorded:?}"
        );
        // ...and the read-back PROVES the restart, not just that a command ran.
        assert_eq!(v["action"], "restarted");
        assert_eq!(v["unit"], "vibed.service");
        assert_eq!(v["active_state"], "active");
        assert_eq!(v["sub_state"], "running");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn svc_restart_surfaces_a_systemctl_failure_as_an_error() {
        let dir = std::env::temp_dir().join(format!("vibed-svcrestart-err-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let marker = dir.join("restarted");
        let bin = fake_systemctl(&dir, &marker, 1); // restart arm exits non-zero

        let err = svc_restart_with(&json!({"unit": "vibed"}), &bin)
            .expect_err("a non-zero systemctl exit must be an error");
        assert!(
            err.contains("systemctl exited"),
            "the failure must be surfaced verbatim: {err}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn svc_restart_rejects_a_missing_or_malformed_unit() {
        // Missing 'unit' -> clear error, nothing spawned.
        assert!(svc_restart_with(&json!({}), "/nonexistent/systemctl")
            .unwrap_err()
            .contains("missing 'unit'"));
        // An injection-shaped unit is refused by validation BEFORE any spawn,
        // so the bogus binary path is never reached.
        assert!(
            svc_restart_with(&json!({"unit": "../etc/passwd"}), "/nonexistent/systemctl")
                .unwrap_err()
                .contains("invalid unit name")
        );
    }

    #[test]
    fn systemctl_show_output_parses_to_snake_case_json() {
        let stdout = "Id=sshd.service\n\
                      Description=OpenSSH server daemon\n\
                      LoadState=loaded\n\
                      ActiveState=active\n\
                      SubState=running\n\
                      UnitFileState=enabled\n\
                      ActiveEnterTimestamp=Mon 2026-07-13 08:00:00 UTC\n\
                      IgnoredKey=whatever\n\
                      not-a-kv-line\n";
        let map = parse_systemctl_show(stdout);
        assert_eq!(map["id"], "sshd.service");
        assert_eq!(map["active_state"], "active");
        assert_eq!(map["sub_state"], "running");
        assert_eq!(map["unit_file_state"], "enabled");
        assert_eq!(map["load_state"], "loaded");
        assert!(!map.contains_key("IgnoredKey"));
        assert!(!map.contains_key("ignored_key"));
    }
}
