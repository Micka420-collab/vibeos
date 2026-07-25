//! Minimal hand-rolled glob matcher — no external crate dependency.
//!
//! Semantics (kept deliberately small, see `docs/SECURITY-ARCHITECTURE.md`):
//! - `*`  matches any run of characters **within one path segment**
//!   (it never crosses a `/`);
//! - `**` as a full segment matches **zero or more whole segments**
//!   (so `/a/b/**` matches `/a/b` itself as well as anything below it);
//! - everything else is matched literally.
//!
//! The same matcher is used for tool names (single-segment strings, where
//! `*` therefore matches freely, e.g. `os.metrics.*`) and for absolute
//! filesystem paths (segmented on `/`).

/// Returns true when `text` matches `pattern` under the semantics above.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let pat: Vec<&str> = pattern.split('/').collect();
    let txt: Vec<&str> = text.split('/').collect();
    match_segments(&pat, &txt)
}

fn match_segments(pat: &[&str], txt: &[&str]) -> bool {
    match pat.split_first() {
        None => txt.is_empty(),
        Some((&"**", rest)) => {
            // `**` may swallow zero or more whole segments.
            (0..=txt.len()).any(|skip| match_segments(rest, &txt[skip..]))
        }
        Some((first, rest)) => match txt.split_first() {
            Some((seg, txt_rest)) => match_segment(first, seg) && match_segments(rest, txt_rest),
            None => false,
        },
    }
}

/// `*`-wildcard match inside a single segment (never crosses `/`).
fn match_segment(pattern: &str, segment: &str) -> bool {
    fn helper(p: &[u8], s: &[u8]) -> bool {
        match p.split_first() {
            None => s.is_empty(),
            Some((b'*', p_rest)) => (0..=s.len()).any(|skip| helper(p_rest, &s[skip..])),
            Some((c, p_rest)) => s
                .split_first()
                .is_some_and(|(sc, s_rest)| sc == c && helper(p_rest, s_rest)),
        }
    }
    helper(pattern.as_bytes(), segment.as_bytes())
}

/// Is this a well-formed PATH glob pattern — i.e. one that can actually match?
///
/// WHY THIS EXISTS (it closes a policy bypass). Paths always reach the matcher
/// NORMALIZED (`normalize_path` collapses `//`, drops `.`, resolves `..`, strips
/// the trailing `/`), but a pattern from a policy file was matched VERBATIM. The
/// two are split on `/` and compared segment by segment, so any pattern that is
/// not already in canonical form matches **nothing**:
/// `"/srv/keys/"` splits to `["", "srv", "keys", ""]` and its trailing empty
/// segment can never equal `"prod.pem"`, so a `paths.denied = ["/srv/keys/"]`
/// entry silently denies NOTHING and the rule fails OPEN. Same for `//srv/x/**`,
/// `/srv/./x/**`, `/srv/y/../x/**` and the relative `srv/x/**`.
///
/// Direction matters: a dead `allowed` pattern is merely fail-closed, but a dead
/// `denied` pattern is a policy bypass — the operator believes a path is refused
/// while every call to it is allowed. So this is validated at LOAD time and a bad
/// pattern refuses the whole policy, exactly as `domain::is_valid_pattern` does
/// for host patterns and the `[rule.deploy]` checks do for deploy targets: a rule
/// that quietly matches nothing looks identical to one that works.
///
/// Requirements: the pattern must be anchored — either ABSOLUTE (`/etc/**`) or
/// starting with a leading `**` (`**/.ssh/**`), which is the documented idiom for
/// "at any depth" and DOES match absolute paths because `**` swallows the root
/// marker too (see `leading_double_star_matches_any_prefix`). Beyond the anchor,
/// no empty segment (so no `//` and no trailing `/`) and no `.`/`..` segment.
/// Wildcards are untouched — `*` and `**` are ordinary segment content here.
/// Deny-everything is spelled `/**` or `**`, not `/`.
///
/// Applies to PATH patterns only, never to tool-name patterns (`os.metrics.*`),
/// which are single-segment strings with no `/` at all.
pub fn is_valid_path_pattern(pattern: &str) -> bool {
    if pattern.is_empty() || pattern.len() > MAX_PATH_BYTES {
        return false;
    }
    // Anchored either at the root, or by a leading `**` that can absorb it.
    let absolute = pattern.starts_with('/');
    if !absolute && pattern != "**" && !pattern.starts_with("**/") {
        return false;
    }
    // `"/a/b".split('/')` yields ["", "a", "b"]: for an absolute pattern the
    // leading "" is the root marker and is skipped; every remaining segment must
    // be a real, non-dot segment.
    let mut segments = pattern.split('/');
    if absolute {
        segments.next(); // root marker
    }
    let mut saw_segment = false;
    for seg in segments {
        saw_segment = true;
        if seg.is_empty() || seg == "." || seg == ".." {
            return false;
        }
    }
    saw_segment
}

/// PATH_MAX on Linux: no real filesystem path exceeds this. Longer inputs
/// cannot name an existing file, so they are policy probes — or DoS attempts
/// against the glob matcher, whose `**` handling is O(n²) in the number of
/// segments (fine at ≤ 4096 bytes, catastrophic on a megabyte-long path fed
/// through the 1 MiB JSON-RPC line budget).
const MAX_PATH_BYTES: usize = 4096;

/// Lexically normalize an absolute path: collapses `//`, removes `.`,
/// resolves `..` segments. Returns `None` for relative paths, for paths that
/// try to climb above `/`, and for paths longer than PATH_MAX (fail-closed:
/// callers must reject the call).
///
/// Symlinks are NOT resolved here; `fs.read` additionally re-checks the
/// canonicalized path (see `mcp.rs`).
pub fn normalize_path(path: &str) -> Option<String> {
    if path.len() > MAX_PATH_BYTES {
        return None;
    }
    if !path.starts_with('/') {
        return None;
    }
    let mut out: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                // Climbing above the root is always a policy probe: reject.
                out.pop()?;
            }
            other => out.push(other),
        }
    }
    Some(format!("/{}", out.join("/")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn star_stays_within_one_segment() {
        assert!(glob_match("/home/*/notes.txt", "/home/dev/notes.txt"));
        assert!(!glob_match("/home/*/notes.txt", "/home/dev/deep/notes.txt"));
        assert!(glob_match("/etc/shadow*", "/etc/shadow"));
        assert!(glob_match("/etc/shadow*", "/etc/shadow-"));
        assert!(!glob_match("/etc/shadow*", "/etc/shadow/sub"));
    }

    #[test]
    fn double_star_crosses_segments_and_matches_zero() {
        assert!(glob_match(
            "/var/lib/vibeos/audit/**",
            "/var/lib/vibeos/audit/vibed.jsonl"
        ));
        assert!(glob_match(
            "/var/lib/vibeos/audit/**",
            "/var/lib/vibeos/audit/a/b/c"
        ));
        // `**` matches zero segments: the directory itself is covered.
        assert!(glob_match(
            "/var/lib/vibeos/audit/**",
            "/var/lib/vibeos/audit"
        ));
        assert!(!glob_match(
            "/var/lib/vibeos/audit/**",
            "/var/lib/vibeos/memory/x"
        ));
    }

    #[test]
    fn leading_double_star_matches_any_prefix() {
        assert!(glob_match("**/.ssh/**", "/home/dev/.ssh/id_ed25519"));
        assert!(glob_match("**/.ssh/**", "/root/.ssh/authorized_keys"));
        assert!(!glob_match("**/.ssh/**", "/home/dev/ssh/notes"));
    }

    #[test]
    fn tool_names_are_single_segment() {
        assert!(glob_match("*", "anything.at.all"));
        assert!(glob_match("os.metrics.*", "os.metrics.cpu"));
        assert!(glob_match("fs.read", "fs.read"));
        assert!(!glob_match("fs.read", "fs.readdir"));
        assert!(!glob_match("fs.*", "os.status"));
        assert!(glob_match("fs.*", "fs.write"));
    }

    #[test]
    fn mid_segment_wildcards() {
        assert!(glob_match("/proc/*/environ", "/proc/1234/environ"));
        assert!(!glob_match("/proc/*/environ", "/proc/1234/task/1/environ"));
        assert!(glob_match(
            "/home/*/.claude/credentials*",
            "/home/dev/.claude/credentials.json"
        ));
    }

    #[test]
    fn double_star_matches_variable_depth_for_proc_bypass() {
        // `/proc/**/environ` must catch the sibling-thread bypass
        // `/proc/<pid>/task/<tid>/environ` (variable depth), where the
        // single-segment `/proc/*/environ` does not.
        assert!(glob_match("/proc/**/environ", "/proc/1234/environ"));
        assert!(glob_match(
            "/proc/**/environ",
            "/proc/1234/task/5678/environ"
        ));
        assert!(glob_match("/proc/**/cmdline", "/proc/1234/cmdline"));
        assert!(glob_match(
            "/proc/**/cmdline",
            "/proc/1234/task/5678/cmdline"
        ));
        // `**` also matches zero intermediate segments.
        assert!(glob_match("/proc/**/environ", "/proc/environ"));
    }

    #[test]
    fn normalize_resolves_dots_and_rejects_escapes() {
        assert_eq!(
            normalize_path("/home/dev/../dev2/x").as_deref(),
            Some("/home/dev2/x")
        );
        assert_eq!(
            normalize_path("/home//dev/./x").as_deref(),
            Some("/home/dev/x")
        );
        assert_eq!(
            normalize_path("/home/dev/../../etc/shadow").as_deref(),
            Some("/etc/shadow")
        );
        assert_eq!(normalize_path("/").as_deref(), Some("/"));
        assert_eq!(normalize_path("/../etc"), None);
        assert_eq!(normalize_path("relative/path"), None);
    }

    #[test]
    fn normalize_rejects_paths_beyond_path_max() {
        // A path longer than PATH_MAX cannot exist on disk; rejecting it at
        // the input boundary also caps the glob matcher's O(n²) `**` cost.
        let deep = format!("/{}", "a/".repeat(4096));
        assert_eq!(normalize_path(&deep), None);
        // At sane lengths, deep paths keep working.
        let ok = format!("/{}", "a/".repeat(100)) + "file";
        assert!(normalize_path(&ok).is_some());
    }

    #[test]
    fn path_patterns_must_be_absolute_and_canonical() {
        // Canonical patterns (the only ones that can actually match a normalized
        // path) are accepted — wildcards are ordinary segment content.
        for good in [
            "/etc/shadow*",
            "/proc/**/environ",
            "/home/*/.ssh/**",
            "/var/lib/vibeos/audit/**",
            "/**",
            // Leading `**` is the documented "at any depth" idiom and DOES match
            // absolute paths (it absorbs the root marker) — the shipped policy
            // relies on it, so rejecting it would stop vibed from booting.
            "**/.ssh/**",
            "**",
        ] {
            assert!(is_valid_path_pattern(good), "{good} must be valid");
        }
        // Every one of these matched NOTHING against a normalized path, so a
        // `paths.denied` entry in this shape silently failed OPEN. They must now
        // be rejected at policy load time instead.
        for bad in [
            "/srv/keys/",        // trailing slash: last segment is "" -> never matches
            "//srv/keys/**",     // empty segment
            "/srv/./keys/**",    // "." segment
            "/srv/x/../keys/**", // ".." segment
            "srv/keys/**",       // relative
            "",                  // empty
            "/",                 // no segment at all; deny-all is "/**"
        ] {
            assert!(!is_valid_path_pattern(bad), "{bad:?} must be rejected");
        }
    }

    #[test]
    fn the_rejected_shapes_really_did_match_nothing() {
        // Proves WHY the validator exists rather than asserting it on faith: the
        // matcher genuinely never matches these against a normalized path, which
        // is what made a denied-path rule fail open.
        assert!(!glob_match("/srv/keys/", "/srv/keys/prod.pem"));
        assert!(!glob_match("//srv/keys/**", "/srv/keys/prod.pem"));
        assert!(!glob_match("/srv/./keys/**", "/srv/keys/prod.pem"));
        assert!(!glob_match("srv/keys/**", "/srv/keys/prod.pem"));
        // ...while the canonical spelling does match, as expected.
        assert!(glob_match("/srv/keys/**", "/srv/keys/prod.pem"));
    }
}
