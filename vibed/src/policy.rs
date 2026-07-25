//! Policy engine: every AI tool call is evaluated against TOML rules loaded
//! from `/etc/vibeos/policy.d/*.toml` (lexicographic filename order, rules
//! top-down, **first matching rule wins**).
//!
//! Semantics (security-first, see `security/policy.d/README.md`):
//! - **absolute default-deny**: a tool with no matching rule is denied, and a
//!   tool absent from the registry is denied before rules are even consulted;
//! - `action = "deny"` denies;
//! - `action = "allow"` with tier T0/T1 allows, subject to the rule's
//!   path/service constraints (a denied path/service wins over everything);
//! - `action = "allow"` with tier T2/T3 **always** yields `RequireApproval`:
//!   the tier is a floor that no rule can lower. A T2/T3 allow rule that does
//!   not carry `approval = "human"` is rejected at load time;
//! - **fail-closed loading**: any unreadable or invalid `*.toml` in the policy
//!   directory is a fatal error — the caller (`main.rs`) must exit non-zero
//!   rather than serve with a partial or default policy.
//!
//! Canonical file schema (schema_version = 1):
//!
//! ```toml
//! schema_version = 1
//!
//! [meta]                       # optional, informational
//! name = "default"
//!
//! [[rule]]
//! id      = "fs-read"          # unique across all loaded files
//! tools   = ["fs.read", "fs.stat"]  # glob patterns on tool names
//! tier    = "T0"               # T0 | T1 | T2 | T3
//! action  = "allow"            # allow | deny
//! approval = "none"            # none | human (mandatory "human" for T2/T3 allow)
//! reason  = "free-form audit context"
//!
//! [rule.paths]                 # optional path constraints (glob, see glob.rs)
//! allowed = ["/home/**"]
//! denied  = ["/home/*/.ssh/**"]
//!
//! [rule.services]              # optional service constraints
//! denied  = ["vibed.service"]
//!
//! [rule.domains]               # optional host scope (ADR-017), browser tools
//! only = ["docs.rs", "*.github.com"]
//! ```
//!
//! **`domains.only` scopes, it does not deny.** `paths.allowed`/`services.allowed`
//! are verdicts: a target outside the list is denied on the spot. `domains.only`
//! is a rule predicate: a host outside the list makes THIS rule inapplicable and
//! evaluation moves to the next one — which is how "trusted domains are free,
//! everything else asks a human" is expressed with plain first-match-wins:
//!
//! ```toml
//! [[rule]]                     # 1. trusted hosts: T1, no human
//! id = "browser-trusted"
//! tools = ["browser.navigate"]
//! tier = "T1"
//! action = "allow"
//! [rule.domains]
//! only = ["docs.rs", "*.github.com"]
//!
//! [[rule]]                     # 2. everything else: falls here, T2, human
//! id = "browser-untrusted"
//! tools = ["browser.navigate"]
//! tier = "T2"
//! action = "allow"
//! approval = "human"
//! ```
//!
//! Order matters as always: put the scoped rule first, or the catch-all shadows
//! it.

use std::borrow::Cow;
use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use tracing::{info, warn};

use crate::domain::{domain_matches_any, is_valid_pattern};
use crate::glob::{glob_match, is_valid_path_pattern};

/// Capability tiers of the VibeOS tool model.
///
/// Ordering matters: derived `Ord` follows declaration order, so
/// `tier >= Tier::T2` means "system-impacting or worse".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
pub enum Tier {
    /// Observe: read-only (system status, file reads, memory queries).
    T0,
    /// Modify-user: user files and user configuration.
    T1,
    /// Modify-system: packages, services, system configuration.
    T2,
    /// Destructive: disk, credentials, network identity.
    T3,
}

impl Tier {
    pub fn as_str(self) -> &'static str {
        match self {
            Tier::T0 => "T0",
            Tier::T1 => "T1",
            Tier::T2 => "T2",
            Tier::T3 => "T3",
        }
    }
}

impl fmt::Display for Tier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Outcome of a policy evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Deny,
    RequireApproval,
}

impl Decision {
    pub fn as_str(self) -> &'static str {
        match self {
            Decision::Allow => "allow",
            Decision::Deny => "deny",
            Decision::RequireApproval => "require_approval",
        }
    }
}

/// Action requested by a rule, as written in TOML.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Allow,
    Deny,
}

/// Approval requirement carried by a rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Approval {
    #[default]
    None,
    Human,
}

/// `[rule.paths]` sub-table: glob constraints on the path argument.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PathConstraints {
    /// When present, the resolved path MUST match at least one entry.
    #[serde(default)]
    pub allowed: Option<Vec<String>>,
    /// A resolved path matching any entry is denied (denied wins).
    #[serde(default)]
    pub denied: Vec<String>,
}

/// `[rule.services]` sub-table: constraints on the service/unit argument.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ServiceConstraints {
    /// When present, the unit MUST match at least one entry (default-deny).
    /// This is the unit allowlist ADR-011 requires for `log.read`: an agent may
    /// read only the journals of explicitly listed units, never the full system
    /// journal. `svc.restart` uses only `denied`; `log.read` uses `allowed`.
    #[serde(default)]
    pub allowed: Option<Vec<String>>,
    /// A service matching any entry is denied (denied wins over allowed).
    #[serde(default)]
    pub denied: Vec<String>,
}

/// `[rule.domains]` sub-table: scopes a rule to a set of hosts (ADR-017).
///
/// # This is a PREDICATE, not a verdict — unlike `[rule.services]`
///
/// `services.allowed` is a **verdict**: a unit outside the list is DENIED, right
/// there, before the tier floor (ADR-011 — an agent may read only the journals of
/// listed units, full stop).
///
/// `domains.only` is a **predicate**: a host outside the list denies nothing. It
/// makes the RULE not apply, and evaluation continues to the next rule. That is
/// what ADR-017 decision 1 asks for — trusted domains browse freely, anything
/// else falls through to a T2 rule and asks a human — and it needs no new engine
/// concept, because first-match-wins already does the escalation.
///
/// The key is `only` and not `allowed` precisely so the two never read alike: if
/// you see `allowed`, off-list is refused; if you see `only`, off-list is someone
/// else's problem. Write `[rule.domains] allowed = […]` by muscle memory and the
/// policy fails to LOAD (`deny_unknown_fields`) rather than silently scoping the
/// rule to nothing — see below.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DomainConstraints {
    /// When present, the rule applies ONLY to hosts matching one of these
    /// patterns (`example.com` exact, `*.example.com` any subdomain — see
    /// `domain::domain_match`). A non-matching host, or a call with no host at
    /// all, makes the rule inapplicable rather than denied.
    #[serde(default)]
    pub only: Option<Vec<String>>,
}

/// One allowed deploy target inside `[rule.deploy]`: an immutable provider +
/// target-id pair (ADR-021 lock 1).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeployAllow {
    /// `fly` | `vercel` | `railway`.
    pub provider: String,
    /// IMMUTABLE target id (Fly app-id, Vercel project-id, Railway service-id) —
    /// never a display name (renameable → confusable). Its identity is relative
    /// to the org of the sealed token, which the agent cannot swap.
    pub target: String,
}

/// `[rule.deploy]` sub-table: the deploy-target allow-list (ADR-021 lock 1).
///
/// # A VERDICT, not a predicate — and the ONLY gate that can grant a deploy
///
/// `deploy.*` acts OUTSIDE, with a cloud token, irreversibly. So unlike the
/// `[rule.domains]` predicate (off-list → try the next rule), this is a
/// **verdict** like `[rule.services].allowed`: a deploy whose `(provider,
/// target)` is not in `allowed` is DENIED on the spot, before the tier floor.
/// And a deploy-bearing call under a rule that carries NO `[rule.deploy]` is
/// refused too (see `apply_rule`) — so no laxer catch-all rule can ever grant a
/// deploy by rule ordering (the bypass Fable 5 flagged for ADR-021). Listing
/// every allowed pair in ONE rule keeps the verdict and still supports several
/// providers/targets.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeployConstraints {
    /// The exhaustive allow-list. A deploy call must match one pair EXACTLY.
    pub allowed: Vec<DeployAllow>,
}

/// One canonical policy rule. Unknown extra keys are ignored (informational
/// fields like `description` are welcome in policy files), but the required
/// fields and the T2/T3 approval floor are enforced at load time.
#[derive(Debug, Clone, Deserialize)]
pub struct Rule {
    pub id: String,
    /// Glob patterns matched against the MCP tool name.
    pub tools: Vec<String>,
    pub tier: Tier,
    pub action: Action,
    #[serde(default)]
    pub approval: Approval,
    /// Optional human-readable justification (kept for audit context).
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub paths: Option<PathConstraints>,
    #[serde(default)]
    pub services: Option<ServiceConstraints>,
    #[serde(default)]
    pub domains: Option<DomainConstraints>,
    #[serde(default)]
    pub deploy: Option<DeployConstraints>,
}

/// On-disk shape of one policy file (see module docs for the full schema).
#[derive(Debug, Deserialize)]
struct PolicyFile {
    #[serde(default)]
    schema_version: Option<i64>,
    #[serde(default)]
    rule: Vec<Rule>,
}

/// Fatal policy loading error. `main.rs` exits non-zero on any of these:
/// a broken policy must never degrade into a more permissive daemon.
#[derive(Debug)]
pub enum PolicyError {
    Io { path: PathBuf, err: io::Error },
    Parse { path: PathBuf, err: String },
    Invalid { path: PathBuf, err: String },
}

impl fmt::Display for PolicyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PolicyError::Io { path, err } => {
                write!(f, "policy: cannot read {}: {err}", path.display())
            }
            PolicyError::Parse { path, err } => {
                write!(f, "policy: invalid TOML in {}: {err}", path.display())
            }
            PolicyError::Invalid { path, err } => {
                write!(f, "policy: rejected {}: {err}", path.display())
            }
        }
    }
}

impl std::error::Error for PolicyError {}

/// A deploy call's target as the policy sees it: the provider and immutable
/// target id, from a `deploy.*` tool's arguments (see `derive_deploy` in
/// mcp.rs). `Some` for EVERY deploy-bearing call — even one whose args are
/// missing (then the fields are empty and the `[rule.deploy]` verdict refuses
/// it), so a deploy with no establishable target can never slip past the
/// allow-list. `None` for any non-deploy tool.
#[derive(Debug, Clone, Copy)]
pub struct DeployTarget<'a> {
    pub provider: &'a str,
    pub target: &'a str,
}

/// Non-tool context of a call, used to enforce path/service/deploy constraints.
#[derive(Debug, Clone, Copy, Default)]
pub struct CallContext<'a> {
    /// Lexically normalized absolute path (see `glob::normalize_path`),
    /// when the tool takes a `path` argument.
    pub path: Option<&'a str>,
    /// Target systemd unit, when the tool takes a `unit` argument.
    pub service: Option<&'a str>,
    /// Target host, when the tool takes a `url` argument: the validated,
    /// lowercase output of `domain::host_of`, never the raw URL.
    ///
    /// `None` means "no host could be established" — either the tool takes no
    /// URL, or the URL was unparseable. It NEVER means "any host": a rule
    /// carrying `[rule.domains]` does not apply when this is `None`, so an
    /// unparseable URL falls through to the untrusted rule instead of
    /// borrowing a trusted rule's tier. See `rule_domain_applies`.
    pub domain: Option<&'a str>,
    /// Deploy provider + target, for a `deploy.*` call. `None` for non-deploy
    /// tools; `Some` (possibly with empty fields) for every deploy call — the
    /// `[rule.deploy]` verdict then requires an exact allow-list match.
    pub deploy: Option<DeployTarget<'a>>,
}

/// Immutable-after-load rule set. Rebuilt on daemon restart
/// (policy files live in `/etc`, writable on an immutable OS).
pub struct PolicyEngine {
    rules: Vec<Rule>,
}

impl PolicyEngine {
    pub fn from_rules(rules: Vec<Rule>) -> Self {
        Self { rules }
    }

    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    /// Accès en lecture seule aux règles chargées, pour en DÉRIVER un manifeste de
    /// capacités (`policy.capabilities`, T0) sans dupliquer l'état : le rendu vit dans
    /// l'outil (présentation), le moteur reste la source unique. La politique vit dans
    /// `/etc` (config), l'exposer à l'agent ne lui accorde rien — elle décrit ses
    /// propres bornes, et `evaluate` reste seul juge.
    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }

    /// Load every `*.toml` file in `dir`, sorted lexicographically.
    ///
    /// FAIL-CLOSED: any unreadable or invalid file aborts the load with an
    /// error; the daemon must refuse to serve. A missing directory is not an
    /// error (zero rules = absolute default-deny, which is safe).
    pub fn load_dir(dir: &Path) -> Result<Self, PolicyError> {
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                warn!(
                    "policy: {} does not exist; zero rules loaded (everything is denied)",
                    dir.display()
                );
                return Ok(Self::from_rules(Vec::new()));
            }
            Err(err) => {
                return Err(PolicyError::Io {
                    path: dir.to_path_buf(),
                    err,
                })
            }
        };
        // Fail-CLOSED enumeration: a per-entry read error must abort the load,
        // not be silently dropped. `.flatten()` on `ReadDir` (an iterator of
        // `io::Result<DirEntry>`) discards every `Err`, so a `10-deny.toml` whose
        // directory entry errors transiently (concurrent replace on restart, an
        // overlay/NFS hiccup) would vanish and a broader `20-allow.toml` would
        // then govern — the exact fail-OPEN this module's contract forbids ("any
        // unreadable or invalid *.toml is a fatal error"). Propagate instead.
        // The extension match is case-INSENSITIVE so a `.TOML`/`.Toml` drop-in is
        // never silently skipped (another silent fail-open under first-match-wins).
        let mut paths: Vec<PathBuf> = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|err| PolicyError::Io {
                path: dir.to_path_buf(),
                err,
            })?;
            let path = entry.path();
            if path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("toml"))
            {
                paths.push(path);
            }
        }
        paths.sort();

        let mut rules: Vec<Rule> = Vec::new();
        let mut seen_ids: HashSet<String> = HashSet::new();
        for path in paths {
            let src = fs::read_to_string(&path).map_err(|err| PolicyError::Io {
                path: path.clone(),
                err,
            })?;
            let file_rules = parse_and_validate(&src).map_err(|err| match err {
                FileError::Parse(msg) => PolicyError::Parse {
                    path: path.clone(),
                    err: msg,
                },
                FileError::Invalid(msg) => PolicyError::Invalid {
                    path: path.clone(),
                    err: msg,
                },
            })?;
            for rule in &file_rules {
                if !seen_ids.insert(rule.id.clone()) {
                    return Err(PolicyError::Invalid {
                        path: path.clone(),
                        err: format!(
                            "duplicate rule id '{}' (ids must be unique across policy.d)",
                            rule.id
                        ),
                    });
                }
            }
            info!(
                "policy: {} rule(s) from {}",
                file_rules.len(),
                path.display()
            );
            rules.extend(file_rules);
        }
        info!("policy: {} rule(s) active", rules.len());
        Ok(Self::from_rules(rules))
    }

    /// Evaluate a tool call. `tier` is `None` when the tool is not in the
    /// registry (=> default-deny). `ctx` carries the path/service arguments
    /// for constraint enforcement.
    pub fn evaluate(&self, tool: &str, tier: Option<Tier>, ctx: CallContext<'_>) -> Decision {
        // Unknown tool: denied before any rule is consulted.
        let Some(registry_tier) = tier else {
            return Decision::Deny;
        };
        for rule in &self.rules {
            if rule.tools.iter().any(|pattern| glob_match(pattern, tool))
                && rule_domain_applies(rule, ctx)
            {
                return apply_rule(rule, registry_tier, ctx);
            }
        }
        // No matching rule: absolute default-deny.
        Decision::Deny
    }
}

/// On Fedora bootc systems `/home` is a symlink to `/var/home`, so the SAME
/// in-home file is addressable under both spellings. The fs tools canonicalize
/// before their post-resolution policy re-check, and `canonicalize` always
/// yields the `/var/home` form — so an operator glob authored as `/home/...`
/// would silently fail to match a `/var/home/...` target (and vice-versa),
/// letting an agent evade the operator's OWN `paths.denied` rule just by
/// choosing the other spelling. Fold both spellings to the canonical
/// `/var/home` form so a path rule holds regardless of how the path is spelled.
/// The built-in secret denylist is already spelling-agnostic (`**/`-anchored),
/// so this only tightens custom operator path rules — never loosens anything.
fn fold_home_alias(s: &str) -> Cow<'_, str> {
    if s == "/home" {
        return Cow::Borrowed("/var/home");
    }
    match s.strip_prefix("/home/") {
        Some(rest) => Cow::Owned(format!("/var/home/{rest}")),
        None => Cow::Borrowed(s),
    }
}

/// Glob-match a filesystem path against a policy pattern, treating the
/// `/home`↔`/var/home` bootc alias as equivalent (see `fold_home_alias`).
/// Used only for path constraints; service patterns are matched verbatim.
fn path_glob_match(pattern: &str, path: &str) -> bool {
    glob_match(&fold_home_alias(pattern), &fold_home_alias(path))
}

/// Does this rule's `[rule.domains]` scope cover the call? Asked at MATCHING
/// time, not verdict time: a `false` here means "try the next rule", never
/// "deny" (see `DomainConstraints`).
///
/// Fail-closed in the case that matters: a rule scoped to trusted domains does
/// NOT apply to a call with no host — an unparseable or non-http URL must never
/// inherit a trusted rule's tier just because nothing could be extracted from
/// it. It falls through to the untrusted rule and meets a human.
fn rule_domain_applies(rule: &Rule, ctx: CallContext<'_>) -> bool {
    let Some(only) = rule.domains.as_ref().and_then(|d| d.only.as_ref()) else {
        // No domain scope: the rule applies to every host, as before.
        return true;
    };
    let Some(host) = ctx.domain else {
        return false;
    };
    domain_matches_any(only, host)
}

fn apply_rule(rule: &Rule, registry_tier: Tier, ctx: CallContext<'_>) -> Decision {
    match rule.action {
        Action::Deny => Decision::Deny,
        Action::Allow => {
            // Path constraints: denied wins; when an allowed list is present
            // the path must match it. Matching is `/home`↔`/var/home`-aware so
            // a rule cannot be dodged by choosing the alternate bootc spelling.
            if let (Some(path), Some(constraints)) = (ctx.path, rule.paths.as_ref()) {
                if constraints.denied.iter().any(|p| path_glob_match(p, path)) {
                    return Decision::Deny;
                }
                if let Some(allowed) = &constraints.allowed {
                    if !allowed.iter().any(|p| path_glob_match(p, path)) {
                        return Decision::Deny;
                    }
                }
            }
            // Service/unit constraints: denied wins; when an allowed list is
            // present the unit MUST match it (default-deny — the unit allowlist
            // ADR-011 requires for log.read). A unit outside the allowlist is
            // denied here, BEFORE the tier floor, so it never reaches approval.
            if let (Some(service), Some(constraints)) = (ctx.service, rule.services.as_ref()) {
                if constraints.denied.iter().any(|p| glob_match(p, service)) {
                    return Decision::Deny;
                }
                if let Some(allowed) = &constraints.allowed {
                    if !allowed.iter().any(|p| glob_match(p, service)) {
                        return Decision::Deny;
                    }
                }
            }
            // Deploy verdict (ADR-021 lock 1): a `deploy.*` call is granted ONLY
            // by a rule that carries `[rule.deploy]` and whose allow-list holds
            // this exact `(provider, target)` pair. Denied HERE, before the tier
            // floor, so an off-list (or empty) target never reaches approval AND
            // no laxer rule (one without `[rule.deploy]`) can grant a deploy by
            // rule ordering. Targets are matched verbatim (immutable ids).
            if let Some(dt) = ctx.deploy {
                match rule.deploy.as_ref() {
                    None => return Decision::Deny,
                    Some(constraints) => {
                        let ok = constraints
                            .allowed
                            .iter()
                            .any(|a| a.provider == dt.provider && a.target == dt.target);
                        if !ok {
                            return Decision::Deny;
                        }
                    }
                }
            }
            // Tier floor: a rule can never lower the intrinsic tier of a tool,
            // and T2/T3 always require a human in the loop.
            let effective_tier = rule.tier.max(registry_tier);
            if effective_tier >= Tier::T2 {
                Decision::RequireApproval
            } else {
                Decision::Allow
            }
        }
    }
}

enum FileError {
    Parse(String),
    Invalid(String),
}

/// Parse one policy file and enforce the load-time invariants:
/// - `schema_version`, when present, must be 1;
/// - `id` non-empty, `tools` non-empty;
/// - a T2/T3 rule with `action = "allow"` MUST carry `approval = "human"`.
fn parse_and_validate(src: &str) -> Result<Vec<Rule>, FileError> {
    let file: PolicyFile = toml::from_str(src).map_err(|e| FileError::Parse(e.to_string()))?;
    if let Some(version) = file.schema_version {
        if version != 1 {
            return Err(FileError::Invalid(format!(
                "unsupported schema_version {version} (expected 1)"
            )));
        }
    }
    for rule in &file.rule {
        if rule.id.trim().is_empty() {
            return Err(FileError::Invalid("rule with empty 'id'".to_string()));
        }
        if rule.tools.is_empty() {
            return Err(FileError::Invalid(format!(
                "rule '{}': empty 'tools' list",
                rule.id
            )));
        }
        if rule.action == Action::Allow && rule.tier >= Tier::T2 && rule.approval != Approval::Human
        {
            return Err(FileError::Invalid(format!(
                "rule '{}': tier {} with action=allow requires approval=\"human\" \
                 (the tier is a floor; T2/T3 can never bypass the human)",
                rule.id, rule.tier
            )));
        }
        // A malformed domain pattern matches nothing, which would silently scope
        // the rule to zero hosts instead of the intended allow-list. Refuse to
        // start: an allow-list that quietly grants nothing looks exactly like an
        // allow-list that works.
        if let Some(only) = rule.domains.as_ref().and_then(|d| d.only.as_ref()) {
            if only.is_empty() {
                return Err(FileError::Invalid(format!(
                    "rule '{}': empty 'domains.only' — the rule could never \
                     apply; remove the constraint or list a domain",
                    rule.id
                )));
            }
            if let Some(bad) = only.iter().find(|p| !is_valid_pattern(p)) {
                return Err(FileError::Invalid(format!(
                    "rule '{}': invalid domain pattern {bad:?} (expected \
                     'example.com' or '*.example.com', lowercase ASCII; an IDN \
                     must be punycoded)",
                    rule.id
                )));
            }
        }
        // A path pattern that is not already canonical matches NOTHING (paths
        // arrive normalized, patterns were matched verbatim), so a `paths.denied`
        // entry like "/srv/keys/" or "//srv/keys/**" silently denies nothing and
        // the rule fails OPEN — the operator believes a path is refused while
        // every call to it is allowed. Refuse to start, same stance as the domain
        // allow-list above. See `glob::is_valid_path_pattern`.
        if let Some(paths) = rule.paths.as_ref() {
            let all = paths
                .allowed
                .iter()
                .flatten()
                .chain(paths.denied.iter())
                .collect::<Vec<_>>();
            if let Some(bad) = all.into_iter().find(|p| !is_valid_path_pattern(p)) {
                return Err(FileError::Invalid(format!(
                    "rule '{}': invalid path pattern {bad:?} — a path pattern must \
                     be absolute and canonical (no '//', no trailing '/', no '.' or \
                     '..' segment), or it matches nothing and the rule silently \
                     fails open; deny-everything is spelled '/**'",
                    rule.id
                )));
            }
        }
        // A `[rule.deploy]` verdict that allowed nothing (or listed an unknown
        // provider / empty target) would silently grant or block deploys in ways
        // the operator never intended. Refuse to start — same fail-closed stance
        // as the domain allow-list above (ADR-021 lock 1).
        if let Some(deploy) = rule.deploy.as_ref() {
            if deploy.allowed.is_empty() {
                return Err(FileError::Invalid(format!(
                    "rule '{}': empty '[rule.deploy].allowed' — a deploy verdict \
                     that grants nothing; list at least one (provider, target)",
                    rule.id
                )));
            }
            for a in &deploy.allowed {
                if !matches!(a.provider.as_str(), "fly" | "vercel" | "railway") {
                    return Err(FileError::Invalid(format!(
                        "rule '{}': unknown deploy provider {:?} (expected fly, \
                         vercel or railway)",
                        rule.id, a.provider
                    )));
                }
                if a.target.trim().is_empty() {
                    return Err(FileError::Invalid(format!(
                        "rule '{}': empty deploy target for provider {:?} (use the \
                         IMMUTABLE id, never a display name)",
                        rule.id, a.provider
                    )));
                }
            }
        }
        // A `[rule.deploy]` allow rule MUST be T2+ (Fable 5, defence in depth):
        // deploy acts OUTSIDE with a cloud token, so the human-approval floor must
        // apply. Without this, the day a `deploy.*` tool is registered, a
        // mis-tiered rule could grant a LISTED target with no human in the loop —
        // the floor would rest entirely on the tool's registry tier.
        if rule.deploy.is_some() && rule.action == Action::Allow && rule.tier < Tier::T2 {
            return Err(FileError::Invalid(format!(
                "rule '{}': '[rule.deploy]' with action=allow requires tier T2+ \
                 (deploy acts outside with a cloud token; the human-approval floor \
                 must apply)",
                rule.id
            )));
        }
        // `[rule.deploy]` and `[rule.domains]` can never both be meaningful: a
        // deploy tool carries no host (`derive_deploy` sets no domain), so the
        // domain scope would silently make the whole rule dead. Refuse it — same
        // "an allow-list that quietly grants nothing" stance as above (Fable 5).
        if rule.deploy.is_some() && rule.domains.is_some() {
            return Err(FileError::Invalid(format!(
                "rule '{}': carries both '[rule.deploy]' and '[rule.domains]' — a \
                 deploy tool has no host, so the domain scope would make the rule \
                 silently inapplicable; split them into separate rules",
                rule.id
            )));
        }
    }
    Ok(file.rule)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine(toml_src: &str) -> PolicyEngine {
        PolicyEngine::from_rules(parse(toml_src))
    }

    fn parse(toml_src: &str) -> Vec<Rule> {
        match parse_and_validate(toml_src) {
            Ok(rules) => rules,
            Err(FileError::Parse(msg)) | Err(FileError::Invalid(msg)) => {
                panic!("test policy must parse: {msg}")
            }
        }
    }

    fn parse_err(toml_src: &str) -> String {
        match parse_and_validate(toml_src) {
            Ok(_) => panic!("test policy must NOT parse"),
            Err(FileError::Parse(msg)) | Err(FileError::Invalid(msg)) => msg,
        }
    }

    const NO_CTX: CallContext<'_> = CallContext {
        path: None,
        service: None,
        domain: None,
        deploy: None,
    };

    /// A deploy call context for `(provider, target)`.
    fn deploy_ctx<'a>(provider: &'a str, target: &'a str) -> CallContext<'a> {
        CallContext {
            deploy: Some(DeployTarget { provider, target }),
            ..Default::default()
        }
    }

    #[test]
    fn deploy_verdict_grants_only_listed_pairs() {
        let e = engine(
            r#"
            [[rule]]
            id = "deploy"
            tools = ["deploy.apply", "deploy.plan"]
            tier = "T2"
            action = "allow"
            approval = "human"
            [rule.deploy]
            allowed = [
              { provider = "fly", target = "app-A" },
              { provider = "vercel", target = "proj-X" },
            ]
            "#,
        );
        // A listed pair reaches the T2 approval floor (never Allow outright), and
        // ONE rule covers several providers.
        assert_eq!(
            e.evaluate("deploy.apply", Some(Tier::T2), deploy_ctx("fly", "app-A")),
            Decision::RequireApproval
        );
        assert_eq!(
            e.evaluate(
                "deploy.plan",
                Some(Tier::T2),
                deploy_ctx("vercel", "proj-X")
            ),
            Decision::RequireApproval
        );
        // Off-list target, off-list provider, and an empty (missing-arg) target
        // are all DENIED by the verdict, before the tier floor.
        assert_eq!(
            e.evaluate("deploy.apply", Some(Tier::T2), deploy_ctx("fly", "app-B")),
            Decision::Deny
        );
        assert_eq!(
            e.evaluate(
                "deploy.apply",
                Some(Tier::T2),
                deploy_ctx("railway", "app-A")
            ),
            Decision::Deny
        );
        assert_eq!(
            e.evaluate("deploy.apply", Some(Tier::T2), deploy_ctx("fly", "")),
            Decision::Deny
        );
    }

    #[test]
    fn a_deploy_under_a_rule_without_deploy_constraints_is_denied() {
        // A broad allow rule carrying NO [rule.deploy] must never grant a deploy
        // — no bypass by rule ordering (ADR-021 lock 1).
        let e = engine(
            r#"
            [[rule]]
            id = "broad"
            tools = ["deploy.*", "*"]
            tier = "T2"
            action = "allow"
            approval = "human"
            "#,
        );
        assert_eq!(
            e.evaluate("deploy.apply", Some(Tier::T2), deploy_ctx("fly", "app-A")),
            Decision::Deny
        );
    }

    #[test]
    fn deploy_rule_load_validation_is_fail_closed() {
        let base = |dep: &str| {
            format!(
                "[[rule]]\nid=\"d\"\ntools=[\"deploy.apply\"]\ntier=\"T2\"\n\
                 action=\"allow\"\napproval=\"human\"\n[rule.deploy]\n{dep}"
            )
        };
        assert!(parse_err(&base("allowed=[]")).contains("empty '[rule.deploy].allowed'"));
        assert!(
            parse_err(&base("allowed=[{provider=\"aws\", target=\"x\"}]"))
                .contains("unknown deploy provider")
        );
        assert!(
            parse_err(&base("allowed=[{provider=\"fly\", target=\"\"}]"))
                .contains("empty deploy target")
        );
        // An unknown key in the pair is refused (deny_unknown_fields).
        assert!(parse_err(&base(
            "allowed=[{provider=\"fly\", target=\"x\", region=\"eu\"}]"
        ))
        .contains("unknown"));
        // A [rule.deploy] allow rule below T2 is refused — the human floor MUST
        // apply to a cloud deploy (Fable 5, defence in depth).
        assert!(parse_err(
            "[[rule]]\nid=\"d\"\ntools=[\"deploy.apply\"]\ntier=\"T1\"\naction=\"allow\"\n\
             [rule.deploy]\nallowed=[{provider=\"fly\", target=\"x\"}]"
        )
        .contains("requires tier T2+"));
        // A rule carrying both deploy and domains is refused (dead-rule trap).
        assert!(parse_err(
            "[[rule]]\nid=\"d\"\ntools=[\"deploy.apply\"]\ntier=\"T2\"\naction=\"allow\"\n\
             approval=\"human\"\n[rule.deploy]\nallowed=[{provider=\"fly\", target=\"x\"}]\n\
             [rule.domains]\nonly=[\"x.com\"]"
        )
        .contains("both"));
    }

    fn host_ctx(host: &str) -> CallContext<'_> {
        CallContext {
            domain: Some(host),
            ..Default::default()
        }
    }

    /// The two-rule shape ADR-017 decision 1 asks for: trusted hosts browse at
    /// T1, everything else falls through to a human at T2.
    const BROWSER_POLICY: &str = r#"
        schema_version = 1

        [[rule]]
        id = "browser-trusted"
        tools = ["browser.*"]
        tier = "T1"
        action = "allow"
        [rule.domains]
        only = ["docs.rs", "*.github.com"]

        [[rule]]
        id = "browser-untrusted"
        tools = ["browser.*"]
        tier = "T2"
        action = "allow"
        approval = "human"
    "#;

    #[test]
    fn trusted_domain_browses_without_a_human() {
        let e = engine(BROWSER_POLICY);
        // Both halves matter. If the allow-list silently matched nothing, every
        // host would escalate and an "off-list escalates" test would still pass
        // — so assert the list actually GRANTS on-list hosts too.
        for host in ["docs.rs", "api.github.com", "raw.github.com"] {
            assert_eq!(
                e.evaluate("browser.navigate", Some(Tier::T1), host_ctx(host)),
                Decision::Allow,
                "{host} is allow-listed: it must browse at T1, no human"
            );
        }
    }

    #[test]
    fn off_list_domain_escalates_to_a_human_rather_than_being_denied() {
        let e = engine(BROWSER_POLICY);
        for host in ["example.com", "github.com", "evil.tld"] {
            assert_eq!(
                e.evaluate("browser.navigate", Some(Tier::T1), host_ctx(host)),
                Decision::RequireApproval,
                "{host} is off-list: ADR-017 asks for approval, NOT a refusal \
                 (that is what makes `domains.only` a predicate, not a verdict)"
            );
        }
    }

    #[test]
    fn a_lookalike_host_cannot_borrow_the_trusted_rule() {
        let e = engine(BROWSER_POLICY);
        // Each of these would be waved through at T1 by a naive `ends_with`,
        // which is the whole reason `domain_match` exists.
        for host in [
            "evil-github.com",
            "api.github.com.evil.tld",
            "docs.rs.evil.tld",
            "notdocs.rs",
        ] {
            assert_eq!(
                e.evaluate("browser.navigate", Some(Tier::T1), host_ctx(host)),
                Decision::RequireApproval,
                "{host} only LOOKS allow-listed: it must not inherit T1"
            );
        }
    }

    #[test]
    fn a_call_with_no_host_never_inherits_the_trusted_rule() {
        let e = engine(BROWSER_POLICY);
        // `domain: None` = the URL could not be parsed (`host_of` failed) or the
        // tool takes no URL. A rule scoped to trusted hosts must NOT apply: an
        // unparseable URL falls through to the human, it does not get a free T1.
        assert_eq!(
            e.evaluate("browser.navigate", Some(Tier::T1), NO_CTX),
            Decision::RequireApproval,
            "no host established => the scoped rule must not apply"
        );
    }

    #[test]
    fn the_tier_floor_still_wins_over_a_trusted_domain() {
        // An allow-listed domain buys a rule match, never a tier cut: a T2 tool
        // stays T2 on docs.rs. The floor is not negotiable (invariant, ADR-004).
        let e = engine(BROWSER_POLICY);
        assert_eq!(
            e.evaluate("browser.click", Some(Tier::T2), host_ctx("docs.rs")),
            Decision::RequireApproval,
            "the registry tier is a floor; an allow-listed host cannot lower it"
        );
    }

    #[test]
    fn a_domain_scope_leaves_other_rules_alone() {
        // Regression guard: adding the predicate must not change evaluation for
        // the ~all rules that carry no [rule.domains] at all.
        let e = engine(
            r#"
            schema_version = 1
            [[rule]]
            id = "fs-read"
            tools = ["fs.read"]
            tier = "T0"
            action = "allow"
        "#,
        );
        assert_eq!(
            e.evaluate("fs.read", Some(Tier::T0), NO_CTX),
            Decision::Allow
        );
        // Even when a host happens to be in context, an unscoped rule applies.
        assert_eq!(
            e.evaluate("fs.read", Some(Tier::T0), host_ctx("evil.tld")),
            Decision::Allow
        );
    }

    #[test]
    fn a_typo_in_a_domain_pattern_refuses_to_load() {
        // A bad pattern matches nothing, so the rule would quietly scope to zero
        // hosts and every call would escalate — an allow-list that grants
        // nothing is indistinguishable from one that works. Fail at load.
        for bad in ["GITHUB.COM", "github..com", "https://github.com", "*."] {
            let msg = parse_err(&format!(
                r#"
                schema_version = 1
                [[rule]]
                id = "r"
                tools = ["browser.*"]
                tier = "T1"
                action = "allow"
                [rule.domains]
                only = ["{bad}"]
            "#
            ));
            assert!(
                msg.contains("invalid domain pattern"),
                "pattern {bad:?} must be rejected at load, got: {msg}"
            );
        }
    }

    #[test]
    fn an_empty_domain_scope_refuses_to_load() {
        let msg = parse_err(
            r#"
            schema_version = 1
            [[rule]]
            id = "r"
            tools = ["browser.*"]
            tier = "T1"
            action = "allow"
            [rule.domains]
            only = []
        "#,
        );
        assert!(msg.contains("empty 'domains.only'"), "got: {msg}");
    }

    #[test]
    fn writing_allowed_instead_of_only_refuses_to_load() {
        // The footgun this guards: `allowed` is the spelling used by
        // [rule.paths] and [rule.services], so it WILL get typed here. Without
        // deny_unknown_fields serde would ignore it, leaving `only = None` — a
        // rule scoped to nothing, i.e. one that applies to EVERY host at T1.
        // A silently-disabled allow-list is the worst outcome available, so the
        // daemon refuses to start instead.
        let msg = parse_err(
            r#"
            schema_version = 1
            [[rule]]
            id = "r"
            tools = ["browser.*"]
            tier = "T1"
            action = "allow"
            [rule.domains]
            allowed = ["docs.rs"]
        "#,
        );
        assert!(
            msg.contains("allowed") || msg.contains("unknown field"),
            "a misspelled key must be a load error, got: {msg}"
        );
    }

    #[test]
    fn rich_schema_parses_with_all_fields() {
        let rules = parse(
            r#"
            schema_version = 1

            [meta]
            name = "test"
            description = "informational tables are tolerated"

            [[rule]]
            id = "fs-write-user"
            description = "extra keys are ignored"
            tools = ["fs.write", "fs.mkdir"]
            tier = "T1"
            action = "allow"
            reason = "user scope"

            [rule.paths]
            allowed = ["/home/**", "/var/home/**"]
            denied = ["/home/*/.ssh/**"]

            [[rule]]
            id = "svc"
            tools = ["svc.*"]
            tier = "T2"
            action = "allow"
            approval = "human"

            [rule.services]
            denied = ["vibed.service"]
            "#,
        );
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].id, "fs-write-user");
        assert_eq!(rules[0].tier, Tier::T1);
        assert_eq!(rules[0].approval, Approval::None);
        let paths = rules[0].paths.as_ref().expect("paths sub-table");
        assert_eq!(paths.allowed.as_ref().map(Vec::len), Some(2));
        assert_eq!(paths.denied.len(), 1);
        assert_eq!(rules[1].approval, Approval::Human);
        assert_eq!(
            rules[1].services.as_ref().expect("services").denied.len(),
            1
        );
    }

    #[test]
    fn unknown_tool_is_denied_even_with_permissive_rules() {
        let e =
            engine("[[rule]]\nid = \"all\"\ntools = [\"*\"]\ntier = \"T0\"\naction = \"allow\"\n");
        assert_eq!(e.evaluate("shady.tool", None, NO_CTX), Decision::Deny);
    }

    #[test]
    fn no_matching_rule_means_deny_whatever_the_tier() {
        let e = PolicyEngine::from_rules(vec![]);
        assert_eq!(
            e.evaluate("os.status", Some(Tier::T0), NO_CTX),
            Decision::Deny
        );
        assert_eq!(
            e.evaluate("fs.write", Some(Tier::T1), NO_CTX),
            Decision::Deny
        );
        assert_eq!(
            e.evaluate("pkg.install", Some(Tier::T2), NO_CTX),
            Decision::Deny
        );
    }

    #[test]
    fn first_matching_rule_wins() {
        let e = engine(
            "[[rule]]\nid = \"family\"\ntools = [\"fs.*\"]\ntier = \"T1\"\naction = \"allow\"\n\
             [[rule]]\nid = \"specific\"\ntools = [\"fs.write\"]\ntier = \"T1\"\naction = \"deny\"\n",
        );
        // fs.* comes first, so the later fs.write deny never fires.
        assert_eq!(
            e.evaluate("fs.write", Some(Tier::T1), NO_CTX),
            Decision::Allow
        );
    }

    #[test]
    fn deny_rule_denies() {
        let e = engine(
            "[[rule]]\nid = \"no\"\ntools = [\"os.status\"]\ntier = \"T0\"\naction = \"deny\"\n",
        );
        assert_eq!(
            e.evaluate("os.status", Some(Tier::T0), NO_CTX),
            Decision::Deny
        );
    }

    #[test]
    fn t2_allow_with_human_approval_yields_require_approval() {
        let e = engine(
            "[[rule]]\nid = \"pkg\"\ntools = [\"pkg.install\"]\ntier = \"T2\"\naction = \"allow\"\napproval = \"human\"\n",
        );
        // The tier is a floor: allow on T2 can never mean unattended execution.
        assert_eq!(
            e.evaluate("pkg.install", Some(Tier::T2), NO_CTX),
            Decision::RequireApproval
        );
    }

    #[test]
    fn t2_allow_without_human_approval_is_a_load_error() {
        let msg = parse_err(
            "[[rule]]\nid = \"pkg\"\ntools = [\"pkg.install\"]\ntier = \"T2\"\naction = \"allow\"\n",
        );
        assert!(
            msg.contains("approval"),
            "error should mention approval: {msg}"
        );
        let msg = parse_err(
            "[[rule]]\nid = \"pkg\"\ntools = [\"pkg.install\"]\ntier = \"T3\"\naction = \"allow\"\napproval = \"none\"\n",
        );
        assert!(
            msg.contains("approval"),
            "error should mention approval: {msg}"
        );
    }

    #[test]
    fn rule_cannot_lower_the_registry_tier() {
        // The rule claims T0, but the registry says the tool is T2:
        // the higher tier wins and approval is still required.
        let e = engine(
            "[[rule]]\nid = \"sneaky\"\ntools = [\"pkg.install\"]\ntier = \"T0\"\naction = \"allow\"\n",
        );
        assert_eq!(
            e.evaluate("pkg.install", Some(Tier::T2), NO_CTX),
            Decision::RequireApproval
        );
    }

    #[test]
    fn path_allowed_and_denied_logic() {
        let e = engine(
            r#"
            [[rule]]
            id = "fs-write-user"
            tools = ["fs.write"]
            tier = "T1"
            action = "allow"
            [rule.paths]
            allowed = ["/home/**", "/var/home/**"]
            denied = ["/home/*/.ssh/**"]
            "#,
        );
        let ctx = |p: &'static str| CallContext {
            path: Some(p),
            ..Default::default()
        };
        assert_eq!(
            e.evaluate("fs.write", Some(Tier::T1), ctx("/home/dev/notes.md")),
            Decision::Allow
        );
        assert_eq!(
            e.evaluate("fs.write", Some(Tier::T1), ctx("/var/home/dev/x")),
            Decision::Allow
        );
        // Outside the allowed list => deny.
        assert_eq!(
            e.evaluate("fs.write", Some(Tier::T1), ctx("/etc/passwd")),
            Decision::Deny
        );
        // Denied wins even inside the allowed list.
        assert_eq!(
            e.evaluate(
                "fs.write",
                Some(Tier::T1),
                ctx("/home/dev/.ssh/authorized_keys")
            ),
            Decision::Deny
        );
    }

    /// Regression: on bootc `/home` is a symlink to `/var/home`, and the fs
    /// tools canonicalize to the `/var/home` spelling before re-checking policy.
    /// A `paths.denied` rule authored in EITHER spelling must therefore hold no
    /// matter which spelling the agent uses to address the same file — otherwise
    /// the operator's own deny is dodged by choosing the alternate name (the
    /// symmetric, path-flavoured cousin of the svc.restart raw-name bypass).
    #[test]
    fn home_var_home_alias_is_matched_in_both_spellings() {
        let ctx = |p: &'static str| CallContext {
            path: Some(p),
            ..Default::default()
        };
        // Deny authored with the `/home` spelling.
        let e_home = engine(
            r#"
            [[rule]]
            id = "fs-read"
            tools = ["fs.read"]
            tier = "T0"
            action = "allow"
            [rule.paths]
            allowed = ["/home/**", "/var/home/**"]
            denied = ["/home/*/private/**"]
            "#,
        );
        for spelling in ["/home/dev/private/secret", "/var/home/dev/private/secret"] {
            assert_eq!(
                e_home.evaluate("fs.read", Some(Tier::T0), ctx(spelling)),
                Decision::Deny,
                "deny '/home/*/private/**' must catch {spelling}"
            );
        }
        // ...and symmetrically when the deny is authored with `/var/home`.
        let e_var = engine(
            r#"
            [[rule]]
            id = "fs-read"
            tools = ["fs.read"]
            tier = "T0"
            action = "allow"
            [rule.paths]
            allowed = ["/home/**", "/var/home/**"]
            denied = ["/var/home/*/private/**"]
            "#,
        );
        for spelling in ["/home/dev/private/secret", "/var/home/dev/private/secret"] {
            assert_eq!(
                e_var.evaluate("fs.read", Some(Tier::T0), ctx(spelling)),
                Decision::Deny,
                "deny '/var/home/*/private/**' must catch {spelling}"
            );
        }
        // A single-spelling ALLOW now covers both spellings of the alias too
        // (the shipped policy still lists both, but one suffices).
        let e_allow = engine(
            r#"
            [[rule]]
            id = "fs-read"
            tools = ["fs.read"]
            tier = "T0"
            action = "allow"
            [rule.paths]
            allowed = ["/home/**"]
            "#,
        );
        assert_eq!(
            e_allow.evaluate("fs.read", Some(Tier::T0), ctx("/var/home/dev/ok.txt")),
            Decision::Allow,
            "allow '/home/**' must also cover the /var/home spelling"
        );
        // A path that is neither /home nor /var/home is unaffected by folding.
        assert_eq!(
            e_allow.evaluate("fs.read", Some(Tier::T0), ctx("/etc/os-release")),
            Decision::Deny
        );
    }

    #[test]
    fn denied_paths_apply_without_an_allowed_list() {
        let e = engine(
            r#"
            [[rule]]
            id = "fs-read"
            tools = ["fs.read"]
            tier = "T0"
            action = "allow"
            [rule.paths]
            denied = ["/var/lib/vibeos/audit/**", "/etc/shadow*"]
            "#,
        );
        let ctx = |p: &'static str| CallContext {
            path: Some(p),
            ..Default::default()
        };
        assert_eq!(
            e.evaluate("fs.read", Some(Tier::T0), ctx("/etc/os-release")),
            Decision::Allow
        );
        assert_eq!(
            e.evaluate("fs.read", Some(Tier::T0), ctx("/etc/shadow")),
            Decision::Deny
        );
        assert_eq!(
            e.evaluate(
                "fs.read",
                Some(Tier::T0),
                ctx("/var/lib/vibeos/audit/vibed.jsonl")
            ),
            Decision::Deny
        );
    }

    #[test]
    fn denied_services_win_over_allow() {
        let e = engine(
            r#"
            [[rule]]
            id = "svc"
            tools = ["svc.restart"]
            tier = "T2"
            action = "allow"
            approval = "human"
            [rule.services]
            denied = ["vibed.service", "systemd-journald.service"]
            "#,
        );
        let ctx = |s: &'static str| CallContext {
            service: Some(s),
            ..Default::default()
        };
        assert_eq!(
            e.evaluate("svc.restart", Some(Tier::T2), ctx("vibed.service")),
            Decision::Deny
        );
        assert_eq!(
            e.evaluate("svc.restart", Some(Tier::T2), ctx("sshd.service")),
            Decision::RequireApproval
        );
    }

    #[test]
    fn unit_allowlist_denies_units_outside_the_list_before_approval() {
        // ADR-011: log.read is T0-allow but gated by a unit allowlist. A unit not
        // on the list is DENIED outright (not allowed, and never sent to the
        // approval queue), exactly like svc.restart's deny-list refuses upstream.
        let e = engine(
            r#"
            [[rule]]
            id = "log-read"
            tools = ["log.read"]
            tier = "T0"
            action = "allow"
            [rule.services]
            allowed = ["vibed.service", "vibeos-agent@*.service"]
            "#,
        );
        let ctx = |s: &'static str| CallContext {
            service: Some(s),
            ..Default::default()
        };
        // Allowlisted units: allowed (T0, no approval).
        assert_eq!(
            e.evaluate("log.read", Some(Tier::T0), ctx("vibed.service")),
            Decision::Allow
        );
        assert_eq!(
            e.evaluate("log.read", Some(Tier::T0), ctx("vibeos-agent@0001.service")),
            Decision::Allow
        );
        // A unit OUTSIDE the allowlist is denied — never the full system journal.
        assert_eq!(
            e.evaluate("log.read", Some(Tier::T0), ctx("sshd.service")),
            Decision::Deny
        );
    }

    #[test]
    fn schema_version_other_than_one_is_rejected() {
        let msg = parse_err("schema_version = 2\n");
        assert!(msg.contains("schema_version"), "unexpected error: {msg}");
    }

    #[test]
    fn missing_required_fields_are_parse_errors() {
        // No `tier`.
        parse_err("[[rule]]\nid = \"x\"\ntools = [\"a\"]\naction = \"allow\"\n");
        // No `tools`.
        parse_err("[[rule]]\nid = \"x\"\ntier = \"T0\"\naction = \"allow\"\n");
        // Empty `tools`.
        parse_err("[[rule]]\nid = \"x\"\ntools = []\ntier = \"T0\"\naction = \"allow\"\n");
        // Legacy pre-canonical action value.
        parse_err(
            "[[rule]]\nid = \"x\"\ntools = [\"a\"]\ntier = \"T0\"\naction = \"require_approval\"\n",
        );
    }

    #[test]
    fn load_dir_fails_closed_on_invalid_file() {
        let dir = std::env::temp_dir().join(format!("vibed-policy-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create test dir");
        fs::write(
            dir.join("10-good.toml"),
            "[[rule]]\nid = \"ok\"\ntools = [\"os.status\"]\ntier = \"T0\"\naction = \"allow\"\n",
        )
        .expect("write good file");
        fs::write(dir.join("20-broken.toml"), "this is [ not TOML").expect("write broken file");

        let result = PolicyEngine::load_dir(&dir);
        assert!(
            result.is_err(),
            "an invalid drop-in must abort the load (fail-closed)"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_dir_fails_closed_on_duplicate_ids() {
        let dir = std::env::temp_dir().join(format!("vibed-policy-dup-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create test dir");
        let rule =
            "[[rule]]\nid = \"same\"\ntools = [\"os.status\"]\ntier = \"T0\"\naction = \"allow\"\n";
        fs::write(dir.join("10-a.toml"), rule).expect("write a");
        fs::write(dir.join("20-b.toml"), rule).expect("write b");

        let result = PolicyEngine::load_dir(&dir);
        assert!(result.is_err(), "duplicate rule ids must abort the load");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_non_canonical_path_pattern_refuses_to_load() {
        // THE BYPASS this closes: paths reach the matcher normalized, patterns were
        // matched verbatim, so a `denied` entry that is not already canonical
        // matched NOTHING — the operator believed a path was refused while every
        // call to it was allowed (a dead `denied` pattern is a policy bypass, not
        // merely a dead rule). Each shape below must now refuse the whole policy.
        for bad in [
            "/home/dev/finance/",   // trailing slash
            "//srv/secrets/**",     // empty segment
            "/srv/./secrets/**",    // "." segment
            "/srv/x/../secrets/**", // ".." segment
            "srv/secrets/**",       // relative
        ] {
            let src = format!(
                "schema_version = 1\n\
                 [[rule]]\nid = \"fs-read\"\ntools = [\"fs.read\"]\n\
                 tier = \"T0\"\naction = \"allow\"\n\
                 [rule.paths]\ndenied = [\"{bad}\"]\n"
            );
            let err = parse_and_validate(&src).expect_err(&format!("{bad} must be rejected"));
            let msg = match err {
                FileError::Invalid(m) | FileError::Parse(m) => m,
            };
            assert!(
                msg.contains("path pattern"),
                "the error must name the bad path pattern: {msg}"
            );
        }
        // An `allowed` list is validated the same way (a dead allow is fail-closed,
        // but it is still a misconfiguration the operator must see).
        let src = "schema_version = 1\n\
                   [[rule]]\nid = \"r\"\ntools = [\"fs.read\"]\n\
                   tier = \"T0\"\naction = \"allow\"\n\
                   [rule.paths]\nallowed = [\"/home/dev/\"]\n";
        assert!(parse_and_validate(src).is_err());
        // Canonical patterns — including wildcards — still load untouched.
        let ok = "schema_version = 1\n\
                  [[rule]]\nid = \"r\"\ntools = [\"fs.read\"]\n\
                  tier = \"T0\"\naction = \"allow\"\n\
                  [rule.paths]\nallowed = [\"/home/**\"]\ndenied = [\"/home/*/.ssh/**\", \"/**\"]\n";
        // `FileError` has no Debug impl, so match rather than `expect`.
        let rules = match parse_and_validate(ok) {
            Ok(r) => r,
            Err(FileError::Invalid(m) | FileError::Parse(m)) => {
                panic!("canonical patterns must load: {m}")
            }
        };
        assert_eq!(rules.len(), 1);
    }

    #[test]
    fn load_dir_matches_toml_extension_case_insensitively() {
        let dir = std::env::temp_dir().join(format!("vibed-policy-ext-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create test dir");
        // An uppercase-extension drop-in must NOT be silently skipped: under
        // first-match-wins, a skipped deny rule is a fail-open.
        fs::write(
            dir.join("10-deny.TOML"),
            "[[rule]]\nid = \"deny-pkg\"\ntools = [\"pkg.install\"]\ntier = \"T2\"\naction = \"deny\"\n",
        )
        .expect("write .TOML file");
        let engine = PolicyEngine::load_dir(&dir).expect("loads a .TOML drop-in");
        assert_eq!(
            engine.rule_count(),
            1,
            "an uppercase .TOML drop-in must be loaded, not skipped"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_dir_tolerates_missing_directory() {
        let dir = std::env::temp_dir().join(format!("vibed-policy-absent-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let engine = PolicyEngine::load_dir(&dir).expect("missing dir is not an error");
        assert_eq!(engine.rule_count(), 0);
        // Zero rules = absolute default-deny.
        assert_eq!(
            engine.evaluate("os.status", Some(Tier::T0), NO_CTX),
            Decision::Deny
        );
    }
}
