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

/// Lexically normalize an absolute path: collapses `//`, removes `.`,
/// resolves `..` segments. Returns `None` for relative paths and for paths
/// that try to climb above `/` (fail-closed: callers must reject the call).
///
/// Symlinks are NOT resolved here; `fs.read` additionally re-checks the
/// canonicalized path (see `mcp.rs`).
pub fn normalize_path(path: &str) -> Option<String> {
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
        assert!(glob_match("/var/lib/vibeos/audit/**", "/var/lib/vibeos/audit/vibed.jsonl"));
        assert!(glob_match("/var/lib/vibeos/audit/**", "/var/lib/vibeos/audit/a/b/c"));
        // `**` matches zero segments: the directory itself is covered.
        assert!(glob_match("/var/lib/vibeos/audit/**", "/var/lib/vibeos/audit"));
        assert!(!glob_match("/var/lib/vibeos/audit/**", "/var/lib/vibeos/memory/x"));
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
        assert!(glob_match("/home/*/.claude/credentials*", "/home/dev/.claude/credentials.json"));
    }

    #[test]
    fn normalize_resolves_dots_and_rejects_escapes() {
        assert_eq!(normalize_path("/home/dev/../dev2/x").as_deref(), Some("/home/dev2/x"));
        assert_eq!(normalize_path("/home//dev/./x").as_deref(), Some("/home/dev/x"));
        assert_eq!(normalize_path("/home/dev/../../etc/shadow").as_deref(), Some("/etc/shadow"));
        assert_eq!(normalize_path("/").as_deref(), Some("/"));
        assert_eq!(normalize_path("/../etc"), None);
        assert_eq!(normalize_path("relative/path"), None);
    }
}
