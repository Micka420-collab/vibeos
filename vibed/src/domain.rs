//! Domain allow-listing for the browser tool surface (ADR-017).
//!
//! Two primitives, both deliberately STRICT and fail-closed, because they are the
//! only thing standing between "the agent browses the docs you trust" and "the
//! agent browses whatever a poisoned page told it to":
//!
//!   * [`host_of`] — extract the host from a URL. Refuses anything it does not
//!     fully understand rather than guessing.
//!   * [`domain_match`] — match a host against an allow-list pattern.
//!
//! # Why not reuse `glob::glob_match`
//!
//! It splits on `/` — it is a PATH matcher. A domain is dot-separated and reads
//! right-to-left. Feeding domains to a path matcher makes `*` swallow dots, so
//! `*.github.com` would match `evil.attacker.github.com.example.org`-shaped junk
//! depending on the input. Different structure, different matcher.
//!
//! # Why not `host.ends_with(pattern)`
//!
//! Because `"evil-github.com".ends_with("github.com")` is **true**. That single
//! line is the whole allow-list bypass: register `evil-github.com`, and an
//! allow-list of `github.com` waves you through. A wildcard here must anchor on a
//! LABEL boundary — i.e. the dot is part of the suffix, never optional.
//!
//! # Why a hand-written parser and not the `url` crate
//!
//! Same reason `sha256.rs` and `glob.rs` are hand-written here: this crate keeps
//! its dependency surface tiny and pinned (SECURITY.md, supply-chain posture).
//! The trade is deliberate — a permissive parser is a liability on a security
//! boundary, so this one is not permissive: it accepts a narrow, boring shape and
//! returns `None` for everything else. `None` never means "allow": callers treat
//! an unparseable URL as untrusted (no domain ⇒ no allow-list rule can match it).

/// Longest legal DNS name, and longest legal label (RFC 1035).
const MAX_HOST_LEN: usize = 253;
const MAX_LABEL_LEN: usize = 63;

/// Extract the lowercase host from an `http(s)://` URL.
///
/// Returns `None` — never a guess — when anything is off. Deliberately refused:
///
/// * a scheme other than `http`/`https` (`file:`, `data:`, `javascript:`…);
/// * **userinfo** (`https://github.com@evil.tld/`): the host there is `evil.tld`,
///   and this is *the* classic way to make a URL read as a trusted site to a
///   human while resolving elsewhere. Refusing beats parsing it right;
/// * IPv6 literals (`[::1]`) — bracket parsing is a bug farm, and an IP can never
///   match a domain allow-list anyway;
/// * any non-ASCII byte. An IDN must arrive already punycoded (`xn--…`), so a
///   homograph (`gіthub.com` with a Cyrillic і) can never be silently folded onto
///   an ASCII pattern;
/// * empty labels (`a..b`), a trailing dot (`github.com.` — the FQDN root form,
///   which resolves the same but would not compare equal), leading/trailing
///   hyphens, over-long labels or hosts;
/// * a port that is not all digits.
pub fn host_of(url: &str) -> Option<String> {
    // The scheme is the only case-insensitive part we accept.
    let rest = {
        let lower = url
            .get(..8)
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        if lower.starts_with("https://") {
            &url[8..]
        } else if lower.starts_with("http://") {
            &url[7..]
        } else {
            return None;
        }
    };

    // Authority ends at the first '/', '?' or '#'.
    let authority = rest.find(['/', '?', '#']).map_or(rest, |i| &rest[..i]);
    if authority.is_empty() {
        return None;
    }
    // Userinfo: refuse outright (see doc comment — this is the impersonation
    // vector, not an edge case).
    if authority.contains('@') {
        return None;
    }
    // IPv6 literal: refuse rather than half-parse.
    if authority.contains('[') || authority.contains(']') {
        return None;
    }

    // Strip an optional :port, which must be numeric and non-empty.
    let host = match authority.split_once(':') {
        Some((h, port)) => {
            if port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            h
        }
        None => authority,
    };

    let host = host.to_ascii_lowercase();
    if !is_valid_host(&host) {
        return None;
    }
    Some(host)
}

/// Is this a syntactically valid, already-normalized ASCII host? Source de vérité UNIQUE de la
/// forme canonique d'un host : `host_of` s'en sert pour valider une URL, et le proxy CONNECT
/// (`crate::proxy`) DOIT s'en servir aussi, sinon deux parseurs valident différemment le même
/// domaine sur un couple de gardes egress (Fable 5). Suppose un host déjà en minuscules.
pub(crate) fn is_valid_host(host: &str) -> bool {
    if host.is_empty() || host.len() > MAX_HOST_LEN {
        return false;
    }
    // Non-ASCII never reaches a comparison: an IDN must be punycoded upstream.
    if !host.is_ascii() {
        return false;
    }
    // A trailing dot resolves identically but compares differently — reject the
    // ambiguity instead of normalizing it away.
    if host.starts_with('.') || host.ends_with('.') {
        return false;
    }
    host.split('.').all(is_valid_label)
}

fn is_valid_label(label: &str) -> bool {
    if label.is_empty() || label.len() > MAX_LABEL_LEN {
        return false;
    }
    if label.starts_with('-') || label.ends_with('-') {
        return false;
    }
    label
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Match a host against one allow-list pattern.
///
/// Two shapes, and only two:
///
/// * `github.com` — exact match, nothing else.
/// * `*.github.com` — any SUBDOMAIN. The leading `*.` stands for one or more
///   whole labels, and the dot is part of the suffix, which is what makes
///   `evil-github.com` fail. It does **not** match the bare `github.com`: list
///   both if you want both, because "the apex is included" is an assumption, and
///   assumptions are what allow-lists exist to remove.
///
/// `host` must already come from [`host_of`] (lowercase, validated). A malformed
/// pattern matches nothing — a typo silently widening the allow-list would be
/// the worst possible failure mode, so the check is: pattern invalid ⇒ no match.
pub fn domain_match(pattern: &str, host: &str) -> bool {
    // ENFORCE the "host must be canonical" precondition, do not merely document
    // it. `host` is meant to arrive from `host_of`/`parse_connect_target`
    // (lowercase, ASCII, legal labels). A weaker guard (`is_ascii && !empty`) let
    // a non-canonical host slip through the wildcard branch: `"x .github.com"`
    // ends_with `".github.com"`, and `"API.github.com"` (uppercase) does too, so
    // a future caller that forgot to validate would silently get an allow-list
    // bypass. `is_valid_host` rejects exactly those (space, uppercase, empty
    // label, over-long), and a genuine `host_of` output always passes it — so
    // this is fail-closed defense-in-depth, never a rejection of a real host.
    if !is_valid_host(host) {
        return false;
    }
    match pattern.strip_prefix("*.") {
        Some(suffix) => {
            // The suffix itself must be a legal host, so `*.` or `*..com` match
            // nothing.
            if !is_valid_host(suffix) {
                return false;
            }
            // Anchored on a label boundary: host must be STRICTLY longer and end
            // with ".<suffix>". `evil-github.com` ends with `-github.com`, not
            // `.github.com` — this is the whole point.
            let dotted = format!(".{suffix}");
            host.len() > dotted.len() && host.ends_with(&dotted)
        }
        None => is_valid_host(pattern) && host == pattern,
    }
}

/// Does `host` match ANY pattern in the list?
pub fn domain_matches_any(patterns: &[String], host: &str) -> bool {
    patterns.iter().any(|p| domain_match(p, host))
}

/// Is this a well-formed allow-list pattern?
///
/// Checked at policy LOAD time so a typo is a startup failure, not a mystery.
/// [`domain_match`] already fails closed on a malformed pattern, but "fails
/// closed" here means the rule quietly matches nothing and every request falls
/// through to the approval rule — safe, yet indistinguishable from a working
/// allow-list until someone wonders why they are approving `docs.rs` all day.
/// `vibed` refuses to start on a bad policy rather than run a subtly different
/// one, so the typo surfaces immediately.
pub fn is_valid_pattern(pattern: &str) -> bool {
    match pattern.strip_prefix("*.") {
        Some(suffix) => is_valid_host(suffix),
        None => is_valid_host(pattern),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_of_extracts_the_ordinary_shapes() {
        assert_eq!(
            host_of("https://github.com/").as_deref(),
            Some("github.com")
        );
        assert_eq!(host_of("https://github.com").as_deref(), Some("github.com"));
        assert_eq!(
            host_of("https://api.github.com/repos/x/y?a=1#frag").as_deref(),
            Some("api.github.com")
        );
        assert_eq!(host_of("http://docs.rs/serde").as_deref(), Some("docs.rs"));
        assert_eq!(
            host_of("https://github.com:443/x").as_deref(),
            Some("github.com")
        );
        // The scheme is the only case-insensitive part; the host is normalized.
        assert_eq!(
            host_of("HTTPS://GitHub.COM/x").as_deref(),
            Some("github.com")
        );
    }

    #[test]
    fn host_of_refuses_the_impersonation_shapes() {
        // THE classic: reads as github.com to a human, resolves to evil.tld.
        assert_eq!(host_of("https://github.com@evil.tld/"), None);
        assert_eq!(host_of("https://user:pass@github.com/"), None);
        // Not http(s): a browser tool has no business here.
        assert_eq!(host_of("file:///etc/passwd"), None);
        assert_eq!(host_of("javascript:alert(1)"), None);
        assert_eq!(host_of("data:text/html,<script>"), None);
        assert_eq!(host_of("ftp://github.com/"), None);
        assert_eq!(host_of("//github.com/"), None);
        assert_eq!(host_of("github.com"), None); // no scheme at all
                                                 // IPv6 literal: refused, not half-parsed.
        assert_eq!(host_of("http://[::1]/"), None);
        // Non-ASCII: an IDN must arrive punycoded, so a homograph cannot fold
        // onto an ASCII pattern.
        assert_eq!(host_of("https://gіthub.com/"), None); // Cyrillic і
        assert_eq!(host_of("https://tést.com/"), None);
        // Backslash is NOT an authority terminator here (only '/','?','#' are),
        // so a browser-style `\`-as-`/` URL either trips the userinfo '@' refusal
        // or the invalid-character check — either way host_of returns None
        // (fail-closed). The browser could route these to a host we never
        // allow-listed, so refusing to extract a host is the safe answer.
        assert_eq!(host_of("https://github.com\\@evil.tld/"), None);
        assert_eq!(host_of("https://github.com\\.evil.tld/"), None);
        // Malformed authorities.
        assert_eq!(host_of("https:///path"), None); // empty host
        assert_eq!(host_of("https://github.com./"), None); // trailing dot
        assert_eq!(host_of("https://a..b/"), None); // empty label
        assert_eq!(host_of("https://-github.com/"), None); // leading hyphen
        assert_eq!(host_of("https://github-.com/"), None); // trailing hyphen
        assert_eq!(host_of("https://github.com:/"), None); // empty port
        assert_eq!(host_of("https://github.com:80x/"), None); // non-numeric port
        assert_eq!(host_of(""), None);
        // Over-long label / host.
        let long_label = "a".repeat(64);
        assert_eq!(host_of(&format!("https://{long_label}.com/")), None);
    }

    #[test]
    fn exact_patterns_match_only_themselves() {
        assert!(domain_match("github.com", "github.com"));
        assert!(!domain_match("github.com", "api.github.com"));
        assert!(!domain_match("github.com", "evil.com"));
        // The bypass an `ends_with` would wave straight through.
        assert!(!domain_match("github.com", "evil-github.com"));
        assert!(!domain_match("github.com", "notgithub.com"));
        // Suffix-of-the-other-side: the pattern is not a substring rule either.
        assert!(!domain_match("github.com", "github.com.evil.tld"));
    }

    #[test]
    fn wildcard_anchors_on_a_label_boundary() {
        assert!(domain_match("*.github.com", "api.github.com"));
        assert!(domain_match("*.github.com", "a.b.github.com"));
        // THE bypass: no dot before the suffix -> not a subdomain.
        assert!(
            !domain_match("*.github.com", "evil-github.com"),
            "a wildcard must anchor on a label boundary, or the allow-list is \
             bypassed by registering `evil-github.com`"
        );
        // A wildcard is not the apex: list both if you want both.
        assert!(!domain_match("*.github.com", "github.com"));
        // Nor a different suffix.
        assert!(!domain_match("*.github.com", "github.com.evil.tld"));
        assert!(!domain_match("*.github.com", "evil.tld"));
        // Degenerate patterns match nothing rather than everything — a typo must
        // never silently widen the allow-list.
        assert!(!domain_match("*.", "github.com"));
        assert!(!domain_match("*", "github.com"));
        assert!(!domain_match("*..com", "a.b.com"));
        assert!(!domain_match("", "github.com"));
    }

    #[test]
    fn a_malformed_pattern_never_widens_the_list() {
        for bad in ["GITHUB.COM", "github..com", "-github.com", ".github.com"] {
            assert!(
                !domain_match(bad, "github.com"),
                "malformed pattern {bad:?} must match nothing"
            );
        }
        // Uppercase in a pattern is a config bug: hosts are lowercased by
        // host_of, so an uppercase pattern can never match. Failing closed makes
        // the bug visible instead of silently disabling a rule.
        assert!(!domain_match("GitHub.com", "github.com"));
    }

    #[test]
    fn a_non_canonical_host_never_matches() {
        // domain_match ENFORCES that the host is canonical (host_of output), not
        // just documents it. Each host below ends_with ".github.com" and would
        // have slipped through the wildcard branch under the old is_ascii-only
        // guard — a bypass for any future caller that skipped host_of. They must
        // all fail closed now.
        assert!(
            !domain_match("*.github.com", "x .github.com"),
            "space in host"
        );
        assert!(
            !domain_match("*.github.com", "API.github.com"),
            "uppercase host"
        );
        assert!(
            !domain_match("*.github.com", ".github.com"),
            "empty leading label"
        );
        assert!(
            !domain_match("github.com", "GITHUB.COM"),
            "uppercase exact host"
        );
        // A genuine canonical host is unaffected — the guard rejects only
        // malformed/non-canonical input, never a real host_of result.
        assert!(domain_match("*.github.com", "api.github.com"));
        assert!(domain_match("github.com", "github.com"));
    }

    #[test]
    fn any_matches_the_first_hit_and_nothing_else() {
        let list = vec![
            "docs.rs".to_string(),
            "*.github.com".to_string(),
            "crates.io".to_string(),
        ];
        assert!(domain_matches_any(&list, "docs.rs"));
        assert!(domain_matches_any(&list, "api.github.com"));
        assert!(domain_matches_any(&list, "crates.io"));
        assert!(!domain_matches_any(&list, "github.com")); // apex not listed
        assert!(!domain_matches_any(&list, "evil-github.com"));
        assert!(!domain_matches_any(&list, "docs.rs.evil.tld"));
        assert!(!domain_matches_any(&[], "docs.rs"));
    }

    #[test]
    fn end_to_end_url_to_verdict() {
        let list = vec!["*.github.com".to_string(), "docs.rs".to_string()];
        let allowed = |u: &str| host_of(u).is_some_and(|h| domain_matches_any(&list, &h));

        assert!(allowed("https://api.github.com/repos"));
        assert!(allowed("https://docs.rs/serde/latest"));
        // Every one of these is a real-world allow-list bypass attempt.
        assert!(!allowed("https://docs.rs@evil.tld/"));
        assert!(!allowed("https://evil-github.com/"));
        assert!(!allowed("https://api.github.com.evil.tld/"));
        assert!(!allowed("https://docs.rs.evil.tld/"));
        assert!(!allowed("file:///etc/shadow"));
        // An unparseable URL yields no host, so no rule can match it: the caller
        // sees "not allow-listed" and escalates. `None` is never "allow".
        assert!(!allowed("¯\\_(ツ)_/¯"));
    }
}
