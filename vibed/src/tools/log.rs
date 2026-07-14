//! `log.read` (T0, sensible à l'exfiltration) — lecture bornée et allowlistée du
//! journal systemd. Conception : ADR-011.
//!
//! Ce N'EST PAS un `journalctl` générique. Les journaux sont un canal
//! d'exfiltration de premier ordre (secrets échoués par des services), et
//! `journald` agrège tous les utilisateurs. La forme sûre, EXACTEMENT comme le
//! spécifie ADR-011 :
//!   1. **Allowlist d'unités** — l'unité est autorisée par la politique
//!      (`[rule.services].allowed`, évaluée en amont dans `handle_tools_call`) ;
//!      défaut refus. Une unité hors-liste est refusée AVANT d'atteindre ce code.
//!   2. **Sortie bornée** — plafond dur de lignes (≤ 200) et d'octets (anti-DoS,
//!      même discipline que `fs.read`).
//!   3. **Passe de rédaction best-effort** — masque les marqueurs de secrets
//!      connus et les jetons à forte entropie. **Défense en profondeur, jamais
//!      une garantie** : le vrai contrôle est l'allowlist + la borne + l'audit.
//!   4. **Aucun filtre libre** — pas de `grep`/regex fourni par l'agent : l'outil
//!      ne doit pas devenir un chercheur de secrets.
//!   5. **Audité** comme tout appel (unité) via `handle_tools_call`.
//!
//! Posture (vibed tourne en root et ceci SPAWN un process) : nom d'unité validé
//! par `svc::validate_unit_name` AVANT tout ; journalctl par chemin ABSOLU,
//! environnement vidé ; l'unité voyage comme valeur de `--unit` (jamais une
//! option positionnelle), donc ni option ni chemin ne peuvent être injectés.

use serde_json::{json, Value};

/// Absolute path — never a PATH lookup, so a compromised environment cannot
/// redirect what vibed (root) executes. Injected in tests via `log_read_with`.
const JOURNALCTL_BIN: &str = "/usr/bin/journalctl";

/// Hard line cap (ADR-011 §2) — the agent can ask for fewer, never more.
const MAX_LINES: u64 = 200;
/// Default when the caller does not specify `lines`.
const DEFAULT_LINES: u64 = 50;
/// Byte cap on the returned text (anti-DoS; keep the most-recent tail).
const MAX_BYTES: usize = 64 * 1024;

/// Placeholder substituted for a masked secret.
const MASK: &str = "«redacted»";

/// log.read (T0): last N lines of ONE allowlisted unit's journal, bounded and
/// redacted. The unit allowlist is enforced upstream by the policy engine; by
/// the time this runs the unit is already authorized.
pub(crate) fn log_read(args: &Value) -> Result<String, String> {
    log_read_with(args, JOURNALCTL_BIN)
}

/// Backend with the journalctl binary injected, so the bounded-read + redaction
/// path can be tested hermetically against a fake journalctl (no root, no live
/// journald).
fn log_read_with(args: &Value, journalctl: &str) -> Result<String, String> {
    let raw = args
        .get("unit")
        .and_then(Value::as_str)
        .ok_or_else(|| "missing 'unit' argument".to_string())?;
    // Reuse svc's conservative validation (no '/', no leading '-', bounded,
    // appends .service). Refuses option/path injection before any spawn.
    let unit = crate::tools::svc::validate_unit_name(raw)?;

    // Bounded line count: default DEFAULT_LINES, hard-capped at MAX_LINES. This
    // is the ONLY knob (ADR-011 §4: no free filter that turns the tool into a
    // secret grep).
    let lines = match args.get("lines") {
        None | Some(Value::Null) => DEFAULT_LINES,
        Some(v) => v
            .as_u64()
            .filter(|n| *n > 0)
            .ok_or_else(|| "log.read: 'lines' must be a positive integer".to_string())?
            .min(MAX_LINES),
    };

    let lines_arg = lines.to_string();
    let output = std::process::Command::new(journalctl)
        .env_clear()
        .args([
            "--unit",
            &unit,
            "--lines",
            &lines_arg,
            "--no-pager",
            "--output=short-iso",
        ])
        .output()
        .map_err(|e| format!("log.read: cannot run journalctl: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let snippet: String = stderr.chars().take(200).collect();
        return Err(format!(
            "log.read {unit}: journalctl exited with {}: {}",
            output.status,
            snippet.trim()
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let (capped, truncated) = cap_bytes(&stdout, MAX_BYTES);
    let (redacted_text, redacted) = redact(&capped);
    // A redaction mask can be longer than the token it replaces, so re-cap after
    // redaction to keep the returned `output` honestly within MAX_BYTES.
    let (text, retruncated) = cap_bytes(&redacted_text, MAX_BYTES);
    let truncated = truncated || retruncated;

    Ok(json!({
        "unit": unit,
        "lines": lines,
        "output": text,
        "truncated": truncated,
        "redacted": redacted,
    })
    .to_string())
}

/// Keep the most-recent tail within `max` bytes, cut on a line boundary and a
/// valid UTF-8 boundary. Returns `(text, truncated)`.
fn cap_bytes(s: &str, max: usize) -> (String, bool) {
    if s.len() <= max {
        return (s.to_string(), false);
    }
    let mut idx = s.len() - max;
    while idx < s.len() && !s.is_char_boundary(idx) {
        idx += 1;
    }
    let tail = &s[idx..];
    // Drop a partial first line so we never emit half a line.
    let tail = tail.find('\n').map(|p| &tail[p + 1..]).unwrap_or(tail);
    (tail.to_string(), true)
}

/// Best-effort redaction (ADR-011 §3). Line by line: mask `key=value` where the
/// key looks sensitive, the token after `Bearer`, the password in a URI
/// (`scheme://user:pass@host`), standalone high-entropy blobs, and flag PEM key
/// markers. NEVER a guarantee — a whitelisted unit that logs a
/// secret in an unrecognized shape will still leak; the real controls are the
/// allowlist, the byte/line bound, and the audit trail.
fn redact(text: &str) -> (String, bool) {
    let mut any = false;
    let mut out = String::with_capacity(text.len());
    for (i, line) in text.lines().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let (masked, hit) = redact_line(line);
        any |= hit;
        out.push_str(&masked);
    }
    // Preserve a trailing newline if the source had one (lines() drops it).
    if text.ends_with('\n') {
        out.push('\n');
    }
    (out, any)
}

fn redact_line(line: &str) -> (String, bool) {
    let mut hit = false;
    let mut out: Vec<String> = Vec::new();
    let mut mask_next = false;
    for tok in line.split(' ') {
        if mask_next {
            mask_next = false;
            if !tok.is_empty() {
                out.push(MASK.to_string());
                hit = true;
                continue;
            }
        }
        // "Bearer <token>" — mask the following non-empty token.
        if tok.eq_ignore_ascii_case("bearer") {
            out.push(tok.to_string());
            mask_next = true;
            continue;
        }
        // key=value with a sensitive-looking key -> mask the value only.
        if let Some((k, v)) = tok.split_once('=') {
            if !v.is_empty() && key_is_sensitive(k) {
                out.push(format!("{k}={MASK}"));
                hit = true;
                continue;
            }
        }
        // Credentials embedded in a URI: scheme://user:password@host — mask the
        // password only (DB/AMQP connection strings echoed by services are a
        // common leak the key=value / high-entropy passes miss on their own).
        if let Some((scheme, rest)) = tok.split_once("://") {
            if let Some((userinfo, host)) = rest.split_once('@') {
                if let Some((user, _pass)) = userinfo.split_once(':') {
                    out.push(format!("{scheme}://{user}:{MASK}@{host}"));
                    hit = true;
                    continue;
                }
            }
        }
        // Standalone high-entropy blob (long base64/hex-like token).
        if looks_high_entropy(tok) {
            out.push(MASK.to_string());
            hit = true;
            continue;
        }
        out.push(tok.to_string());
    }
    // PEM private-key marker: the base64 body is caught by high-entropy, but flag
    // the presence so `redacted` is truthful.
    if line.contains("PRIVATE KEY") {
        hit = true;
    }
    (out.join(" "), hit)
}

/// Does this `key=` look like it carries a secret? Marker set from ADR-011 plus
/// the usual suspects. Over-matching is acceptable (we mask the VALUE only).
fn key_is_sensitive(key: &str) -> bool {
    // Keep only the last word of the key side (so `export API_KEY` still hits).
    let last = key
        .rsplit([' ', '"', '\'', '(', '\t'])
        .next()
        .unwrap_or(key);
    let k = last.to_ascii_uppercase();
    k.ends_with("_KEY")
        || k.contains("SECRET")
        || k.contains("TOKEN")
        || k.contains("PASSWORD")
        || k.contains("PASSWD")
        || k.contains("APIKEY")
        || k == "PWD"
        || k.starts_with("AWS_")
}

/// A long token drawn from the base64/hex alphabet with a mix of letters and
/// digits — a proxy for an API key / hash / token. Conservative length floor so
/// we don't mask ordinary words or plain numbers.
fn looks_high_entropy(tok: &str) -> bool {
    let t = tok.trim_matches(|c: char| {
        matches!(
            c,
            '"' | '\'' | ',' | ';' | ')' | '(' | '[' | ']' | '{' | '}' | ':'
        )
    });
    if t.len() < 32 {
        return false;
    }
    let alphabet_ok = t
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=' | '_' | '-'));
    if !alphabet_ok {
        return false;
    }
    let has_digit = t.chars().any(|c| c.is_ascii_digit());
    let has_alpha = t.chars().any(|c| c.is_ascii_alphabetic());
    has_digit && has_alpha
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn missing_or_malformed_unit_is_rejected_before_any_spawn() {
        // No 'unit' -> clear error, nothing spawned (bogus binary never reached).
        assert!(log_read_with(&json!({}), "/nonexistent/journalctl")
            .unwrap_err()
            .contains("missing 'unit'"));
        // Injection-shaped unit refused by validation before spawn.
        assert!(
            log_read_with(&json!({"unit": "../etc/passwd"}), "/nonexistent/journalctl")
                .unwrap_err()
                .contains("invalid unit name")
        );
        // A non-positive 'lines' is rejected.
        assert!(log_read_with(
            &json!({"unit": "vibed", "lines": 0}),
            "/nonexistent/journalctl"
        )
        .unwrap_err()
        .contains("positive integer"));
    }

    /// Write an executable fake `journalctl` that echoes canned log lines
    /// (including a secret), so the bounded-read + redaction path runs without
    /// root or a live journald.
    #[cfg(unix)]
    fn fake_journalctl(dir: &std::path::Path) -> String {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("journalctl");
        // Emits three lines; one carries an API key + a bearer token.
        let script = "#!/bin/sh\n\
             printf '2026-07-14 21:00:00 host app[1]: starting up\\n'\n\
             printf '2026-07-14 21:00:01 host app[1]: API_KEY=sk-abcdef0123456789abcdef0123456789 loaded\\n'\n\
             printf '2026-07-14 21:00:02 host app[1]: auth header Bearer eyJrahalnlksdf.tokentokentoken.sig\\n'\n\
             exit 0\n";
        std::fs::write(&path, script).expect("write fake journalctl");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("chmod fake journalctl");
        path.to_string_lossy().into_owned()
    }

    #[cfg(unix)]
    #[test]
    fn reads_bounded_output_and_redacts_obvious_secrets() {
        let dir = std::env::temp_dir().join(format!("vibed-logread-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let bin = fake_journalctl(&dir);

        // A too-large 'lines' is capped to MAX_LINES (never amplified).
        let out = log_read_with(&json!({"unit": "vibed", "lines": 9999}), &bin)
            .expect("log.read succeeds");
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();

        assert_eq!(v["unit"], "vibed.service");
        assert_eq!(v["lines"], MAX_LINES); // 9999 -> 200 hard cap
        assert_eq!(v["redacted"], true);

        let text = v["output"].as_str().unwrap();
        // The secret value and the bearer token are masked...
        assert!(
            text.contains(MASK),
            "expected a redaction marker in: {text}"
        );
        assert!(
            !text.contains("sk-abcdef0123456789abcdef0123456789"),
            "the API key leaked: {text}"
        );
        assert!(
            !text.contains("eyJrahalnlksdf.tokentokentoken.sig"),
            "the bearer token leaked: {text}"
        );
        // ...but ordinary log text survives.
        assert!(text.contains("starting up"), "benign text was lost: {text}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn redactor_masks_markers_but_keeps_plain_lines() {
        let (masked, hit) = redact("connecting to db on port 5432");
        assert!(!hit, "a benign line must not be redacted: {masked}");
        assert_eq!(masked, "connecting to db on port 5432");

        let (m1, h1) = redact("export AWS_SECRET_ACCESS_KEY=AKIAIOSFODNN7EXAMPLEabcdef012345");
        assert!(h1);
        assert!(m1.contains(MASK) && !m1.contains("AKIAIOSFODNN7EXAMPLE"));

        let (m2, h2) = redact("password=hunter2trombone was set");
        assert!(h2 && m2.contains("password=«redacted»"));

        // URI credentials: the password is masked, the rest of the DSN survives.
        let (m4, h4) = redact("DB at postgres://app:s3cr3tp4ss@db.internal:5432/prod ready");
        assert!(h4);
        assert!(
            m4.contains("postgres://app:«redacted»@db.internal:5432/prod")
                && !m4.contains("s3cr3tp4ss"),
            "URI password must be masked, host/db kept: {m4}"
        );

        // A short, low-entropy token is not masked (no false positive on IDs).
        let (m3, h3) = redact("session id=42 user=alice");
        assert!(!h3, "short values must not be masked: {m3}");
        // A plain URL without credentials is left untouched.
        let (m5, h5) = redact("fetched https://example.com/api/v1 ok");
        assert!(!h5, "a credential-free URL must not be redacted: {m5}");
    }

    #[test]
    fn byte_cap_keeps_the_tail_on_a_line_boundary() {
        let big = format!("{}\nLAST LINE\n", "x".repeat(100_000));
        let (capped, truncated) = cap_bytes(&big, 1024);
        assert!(truncated);
        assert!(capped.len() <= 1024);
        assert!(capped.contains("LAST LINE"), "the tail must be preserved");
        // No partial first line: capped starts at a line boundary.
        assert!(!capped.starts_with('x') || capped.lines().next() == Some("LAST LINE"));
    }
}
