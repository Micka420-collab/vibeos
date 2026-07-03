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
//! ```

use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use tracing::{info, warn};

use crate::glob::glob_match;

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
    /// A service matching any entry is denied.
    #[serde(default)]
    pub denied: Vec<String>,
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
            PolicyError::Io { path, err } => write!(f, "policy: cannot read {}: {err}", path.display()),
            PolicyError::Parse { path, err } => write!(f, "policy: invalid TOML in {}: {err}", path.display()),
            PolicyError::Invalid { path, err } => write!(f, "policy: rejected {}: {err}", path.display()),
        }
    }
}

impl std::error::Error for PolicyError {}

/// Non-tool context of a call, used to enforce path/service constraints.
#[derive(Debug, Clone, Copy, Default)]
pub struct CallContext<'a> {
    /// Lexically normalized absolute path (see `glob::normalize_path`),
    /// when the tool takes a `path` argument.
    pub path: Option<&'a str>,
    /// Target systemd unit, when the tool takes a `unit` argument.
    pub service: Option<&'a str>,
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
            Err(err) => return Err(PolicyError::Io { path: dir.to_path_buf(), err }),
        };
        let mut paths: Vec<_> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|ext| ext == "toml"))
            .collect();
        paths.sort();

        let mut rules: Vec<Rule> = Vec::new();
        let mut seen_ids: HashSet<String> = HashSet::new();
        for path in paths {
            let src = fs::read_to_string(&path)
                .map_err(|err| PolicyError::Io { path: path.clone(), err })?;
            let file_rules = parse_and_validate(&src)
                .map_err(|err| match err {
                    FileError::Parse(msg) => PolicyError::Parse { path: path.clone(), err: msg },
                    FileError::Invalid(msg) => PolicyError::Invalid { path: path.clone(), err: msg },
                })?;
            for rule in &file_rules {
                if !seen_ids.insert(rule.id.clone()) {
                    return Err(PolicyError::Invalid {
                        path: path.clone(),
                        err: format!("duplicate rule id '{}' (ids must be unique across policy.d)", rule.id),
                    });
                }
            }
            info!("policy: {} rule(s) from {}", file_rules.len(), path.display());
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
            if rule.tools.iter().any(|pattern| glob_match(pattern, tool)) {
                return apply_rule(rule, registry_tier, ctx);
            }
        }
        // No matching rule: absolute default-deny.
        Decision::Deny
    }
}

fn apply_rule(rule: &Rule, registry_tier: Tier, ctx: CallContext<'_>) -> Decision {
    match rule.action {
        Action::Deny => Decision::Deny,
        Action::Allow => {
            // Path constraints: denied wins; when an allowed list is present
            // the path must match it.
            if let (Some(path), Some(constraints)) = (ctx.path, rule.paths.as_ref()) {
                if constraints.denied.iter().any(|p| glob_match(p, path)) {
                    return Decision::Deny;
                }
                if let Some(allowed) = &constraints.allowed {
                    if !allowed.iter().any(|p| glob_match(p, path)) {
                        return Decision::Deny;
                    }
                }
            }
            // Service constraints: denied wins.
            if let (Some(service), Some(constraints)) = (ctx.service, rule.services.as_ref()) {
                if constraints.denied.iter().any(|p| glob_match(p, service)) {
                    return Decision::Deny;
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
            return Err(FileError::Invalid(format!("rule '{}': empty 'tools' list", rule.id)));
        }
        if rule.action == Action::Allow && rule.tier >= Tier::T2 && rule.approval != Approval::Human {
            return Err(FileError::Invalid(format!(
                "rule '{}': tier {} with action=allow requires approval=\"human\" \
                 (the tier is a floor; T2/T3 can never bypass the human)",
                rule.id, rule.tier
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

    const NO_CTX: CallContext<'_> = CallContext { path: None, service: None };

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
        assert_eq!(rules[1].services.as_ref().expect("services").denied.len(), 1);
    }

    #[test]
    fn unknown_tool_is_denied_even_with_permissive_rules() {
        let e = engine(
            "[[rule]]\nid = \"all\"\ntools = [\"*\"]\ntier = \"T0\"\naction = \"allow\"\n",
        );
        assert_eq!(e.evaluate("shady.tool", None, NO_CTX), Decision::Deny);
    }

    #[test]
    fn no_matching_rule_means_deny_whatever_the_tier() {
        let e = PolicyEngine::from_rules(vec![]);
        assert_eq!(e.evaluate("os.status", Some(Tier::T0), NO_CTX), Decision::Deny);
        assert_eq!(e.evaluate("fs.write", Some(Tier::T1), NO_CTX), Decision::Deny);
        assert_eq!(e.evaluate("pkg.install", Some(Tier::T2), NO_CTX), Decision::Deny);
    }

    #[test]
    fn first_matching_rule_wins() {
        let e = engine(
            "[[rule]]\nid = \"family\"\ntools = [\"fs.*\"]\ntier = \"T1\"\naction = \"allow\"\n\
             [[rule]]\nid = \"specific\"\ntools = [\"fs.write\"]\ntier = \"T1\"\naction = \"deny\"\n",
        );
        // fs.* comes first, so the later fs.write deny never fires.
        assert_eq!(e.evaluate("fs.write", Some(Tier::T1), NO_CTX), Decision::Allow);
    }

    #[test]
    fn deny_rule_denies() {
        let e = engine("[[rule]]\nid = \"no\"\ntools = [\"os.status\"]\ntier = \"T0\"\naction = \"deny\"\n");
        assert_eq!(e.evaluate("os.status", Some(Tier::T0), NO_CTX), Decision::Deny);
    }

    #[test]
    fn t2_allow_with_human_approval_yields_require_approval() {
        let e = engine(
            "[[rule]]\nid = \"pkg\"\ntools = [\"pkg.install\"]\ntier = \"T2\"\naction = \"allow\"\napproval = \"human\"\n",
        );
        // The tier is a floor: allow on T2 can never mean unattended execution.
        assert_eq!(e.evaluate("pkg.install", Some(Tier::T2), NO_CTX), Decision::RequireApproval);
    }

    #[test]
    fn t2_allow_without_human_approval_is_a_load_error() {
        let msg = parse_err(
            "[[rule]]\nid = \"pkg\"\ntools = [\"pkg.install\"]\ntier = \"T2\"\naction = \"allow\"\n",
        );
        assert!(msg.contains("approval"), "error should mention approval: {msg}");
        let msg = parse_err(
            "[[rule]]\nid = \"pkg\"\ntools = [\"pkg.install\"]\ntier = \"T3\"\naction = \"allow\"\napproval = \"none\"\n",
        );
        assert!(msg.contains("approval"), "error should mention approval: {msg}");
    }

    #[test]
    fn rule_cannot_lower_the_registry_tier() {
        // The rule claims T0, but the registry says the tool is T2:
        // the higher tier wins and approval is still required.
        let e = engine(
            "[[rule]]\nid = \"sneaky\"\ntools = [\"pkg.install\"]\ntier = \"T0\"\naction = \"allow\"\n",
        );
        assert_eq!(e.evaluate("pkg.install", Some(Tier::T2), NO_CTX), Decision::RequireApproval);
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
        let ctx = |p: &'static str| CallContext { path: Some(p), service: None };
        assert_eq!(e.evaluate("fs.write", Some(Tier::T1), ctx("/home/dev/notes.md")), Decision::Allow);
        assert_eq!(e.evaluate("fs.write", Some(Tier::T1), ctx("/var/home/dev/x")), Decision::Allow);
        // Outside the allowed list => deny.
        assert_eq!(e.evaluate("fs.write", Some(Tier::T1), ctx("/etc/passwd")), Decision::Deny);
        // Denied wins even inside the allowed list.
        assert_eq!(
            e.evaluate("fs.write", Some(Tier::T1), ctx("/home/dev/.ssh/authorized_keys")),
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
        let ctx = |p: &'static str| CallContext { path: Some(p), service: None };
        assert_eq!(e.evaluate("fs.read", Some(Tier::T0), ctx("/etc/os-release")), Decision::Allow);
        assert_eq!(e.evaluate("fs.read", Some(Tier::T0), ctx("/etc/shadow")), Decision::Deny);
        assert_eq!(
            e.evaluate("fs.read", Some(Tier::T0), ctx("/var/lib/vibeos/audit/vibed.jsonl")),
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
        let ctx = |s: &'static str| CallContext { path: None, service: Some(s) };
        assert_eq!(e.evaluate("svc.restart", Some(Tier::T2), ctx("vibed.service")), Decision::Deny);
        assert_eq!(
            e.evaluate("svc.restart", Some(Tier::T2), ctx("sshd.service")),
            Decision::RequireApproval
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
        parse_err("[[rule]]\nid = \"x\"\ntools = [\"a\"]\ntier = \"T0\"\naction = \"require_approval\"\n");
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
        assert!(result.is_err(), "an invalid drop-in must abort the load (fail-closed)");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_dir_fails_closed_on_duplicate_ids() {
        let dir = std::env::temp_dir().join(format!("vibed-policy-dup-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create test dir");
        let rule = "[[rule]]\nid = \"same\"\ntools = [\"os.status\"]\ntier = \"T0\"\naction = \"allow\"\n";
        fs::write(dir.join("10-a.toml"), rule).expect("write a");
        fs::write(dir.join("20-b.toml"), rule).expect("write b");

        let result = PolicyEngine::load_dir(&dir);
        assert!(result.is_err(), "duplicate rule ids must abort the load");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_dir_tolerates_missing_directory() {
        let dir = std::env::temp_dir().join(format!("vibed-policy-absent-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let engine = PolicyEngine::load_dir(&dir).expect("missing dir is not an error");
        assert_eq!(engine.rule_count(), 0);
        // Zero rules = absolute default-deny.
        assert_eq!(engine.evaluate("os.status", Some(Tier::T0), NO_CTX), Decision::Deny);
    }
}
