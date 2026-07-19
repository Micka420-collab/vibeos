//! ADR-019 sandbox — hardening profiles for the transient per-tool service.
//!
//! This module is **pure**: it spawns nothing, opens no socket, touches no root
//! state. It turns a [`ToolClass`] + a per-invocation [`UnitSpec`] into the exact
//! list of systemd transient-unit properties that a later increment
//! (`systemd-run`, injected and tested like `tools/svc.rs` injects `systemctl`)
//! applies **before** exec'ing the low-privilege `vibed-tool` helper. `vibed`
//! itself never runs the hostile tool — it only compiles this profile and hands
//! it to pid 1.
//!
//! Encoding the profile as data (not an opaque shell string built at spawn time)
//! is the whole point: the security-critical choices from the Fable-5-hardened
//! ADR-021 (`deploy.*`) and ADR-022 (`browser.*`) become **unit-testable
//! invariants**. The tests below are the specification.
//!
//! ADR-019 point 4 — one lockdown does not fit both:
//!   * **Deploy** wants the maximum: deny ALL namespaces, drop every capability,
//!     `ProcSubset=pid`. But W^X (`MemoryDenyWriteExecute`) is **per-CLI**, not
//!     per-class: flyctl is Go (no JIT) and can take it; vercel/railway are
//!     Node.js and would **crash V8's JIT** under it — hence
//!     [`ToolClass::Deploy`] carries `needs_wx`.
//!   * **Browser** must *relax*: an allow-list of namespaces so Chromium's
//!     unprivileged userns sandbox can initialise, no `MemoryDenyWriteExecute`
//!     (V8 JIT), and a syscall filter that keeps `@sandbox`/`chroot` (its layer-1
//!     needs them) instead of deploy's `~@privileged`.
//!
//! Every value that reaches a `--property=` string is validated here first
//! (fail-closed): a bad CIDR, memory size, credential name or invocation id
//! makes [`TransientUnit::compile`] return `Err` rather than emit a malformed or
//! injectable property.

use std::path::Path;

/// The class of governed tool a transient service will run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolClass {
    /// `deploy.*` — a cloud CLI talking to a provider API with a sealed token.
    /// `needs_wx` is `true` for Node.js CLIs (vercel, railway — V8 JIT needs
    /// writable-executable memory) and `false` for flyctl (Go, no JIT), so the
    /// maximum-confinement profile drops `MemoryDenyWriteExecute` only when the
    /// specific CLI would otherwise crash.
    Deploy { needs_wx: bool },
    /// `browser.*` — `chromium-headless` driven over a CDP pipe.
    Browser,
}

/// A TPM2-sealed credential to hand the helper (ADR-021 lock 3). `vibed` never
/// sees the plaintext: it names the sealed blob and its credstore path, systemd
/// decrypts into the helper's `$CREDENTIALS_DIRECTORY` (distinct uid, 0400).
#[derive(Clone, Debug)]
pub struct Credential {
    /// Credential id (the `$CREDENTIALS_DIRECTORY` filename the CLI reads).
    pub name: String,
    /// Absolute path of the encrypted blob (per-project, so two projects with
    /// different tokens coexist — Fable 5).
    pub path: String,
}

/// Per-invocation parameters. The *shape* of the profile is fixed by the class;
/// these are the values filled in at spawn time.
#[derive(Clone, Debug)]
pub struct UnitSpec {
    /// Unique per-invocation token (a nonce). Isolates the ephemeral HOME and
    /// runtime dir **per call** so two concurrent invocations never share
    /// `/run/vibed-tool` — sharing it would re-couple the distinct dynamic uids
    /// and defeat ADR-021 locks 2 & 3 (Fable 5). Must match `[a-z0-9-]`.
    pub invocation_id: String,
    /// Sealed credential, or `None` for the ephemeral browser (which holds no
    /// credential — ADR-022's ephemeral profile is the whole point).
    pub credential: Option<Credential>,
    /// IP/CIDR egress allow-list, over the `IPAddressDeny=any` floor.
    /// `IPAddressAllow` is by address, never hostname (ADR-017 → ADR-022). For
    /// browser this is the **dedicated proxy IP only** (e.g. `127.66.0.1/32`,
    /// *not* `127.0.0.1/32` which would open every loopback service); all real
    /// egress goes through the CONNECT proxy that evaluates `[rule.domains]`.
    /// (Deploy additionally gets the local DNS resolver opened automatically.)
    pub egress_allow: Vec<String>,
    /// Hard cgroup memory ceiling (`MemoryMax`), e.g. `"1G"`. Measured on the
    /// cgroup, not RSS (ADR-022).
    pub memory_max: String,
    /// Wall-clock ceiling (`RuntimeMaxSec`, seconds): process-per-call must have
    /// a time wall or a hung invocation becomes a zombie (Fable 5).
    pub runtime_max_sec: u32,
    /// Deploy only: the project directory the CLI uploads from, bind-mounted
    /// read-only (`BindReadOnlyPaths`). Without it `ProtectHome=yes` +
    /// `ProtectSystem=strict` leave the helper unable to see any source (Fable
    /// 5). Ignored for the browser.
    pub workspace: Option<String>,
}

/// A compiled set of systemd unit properties, in declaration order. Each entry
/// renders to one `--property=KEY=VALUE` argument to `systemd-run`.
#[derive(Clone, Debug)]
pub struct TransientUnit {
    properties: Vec<(String, String)>,
}

impl TransientUnit {
    /// Compile the hardening profile for `class` with the per-invocation `spec`,
    /// or `Err` if any injected value is malformed (fail-closed).
    ///
    /// The **common ADR-019 base** establishes the invariants that make the
    /// helper a low-privilege process distinct from both the agent and `vibed`:
    /// a distinct transient uid (`DynamicUser=yes`, never `User=%i` — the
    /// *bearing* control of ADR-021 lock 2), no new privileges, an ephemeral
    /// tmpfs HOME **keyed by `invocation_id`** (lock 3, concurrency-safe), an
    /// `IPAddressDeny=any` floor with byte accounting, and the full Protect*/
    /// Restrict* surface.
    pub fn compile(class: ToolClass, spec: &UnitSpec) -> Result<Self, String> {
        // ---- validate every injected value BEFORE building (fail-closed) ----
        validate_invocation_id(&spec.invocation_id)?;
        validate_memory_size(&spec.memory_max)?;
        for cidr in &spec.egress_allow {
            validate_cidr(cidr)?;
        }
        if let Some(c) = &spec.credential {
            validate_credential_name(&c.name)?;
            validate_abs_path(&c.path, "credential path")?;
        }
        if let Some(w) = &spec.workspace {
            validate_abs_path(w, "workspace")?;
        }

        let mut p: Vec<(String, String)> = Vec::new();
        let mut set = |k: &str, v: &str| p.push((k.to_string(), v.to_string()));

        let home = format!("/run/vibed-tool/{}", spec.invocation_id);

        // ---- common ADR-019 base -------------------------------------------
        set("DynamicUser", "yes"); // distinct transient uid — NEVER User=%i
        set("NoNewPrivileges", "yes");
        set("UMask", "0077");
        // Ephemeral HOME/runtime, keyed by the invocation nonce so concurrent
        // calls never collide (Fable 5 concurrency fix). RuntimeDirectory =>
        // /run/vibed-tool/<id>, 0700, owned by the dynamic uid, removed on stop.
        set(
            "RuntimeDirectory",
            &format!("vibed-tool/{}", spec.invocation_id),
        );
        set("RuntimeDirectoryMode", "0700");
        set("Environment", &format!("HOME={home}"));
        set("WorkingDirectory", &home);
        set("PrivateTmp", "yes");
        set("PrivateDevices", "yes");
        set("RemoveIPC", "yes");
        set("ProtectSystem", "strict");
        set("ProtectHome", "yes");
        set("ProtectProc", "invisible");
        set("ProtectControlGroups", "yes");
        set("ProtectKernelTunables", "yes");
        set("ProtectKernelModules", "yes");
        set("ProtectKernelLogs", "yes");
        set("ProtectClock", "yes");
        set("ProtectHostname", "yes");
        set("RestrictSUIDSGID", "yes");
        set("RestrictRealtime", "yes");
        set("LockPersonality", "yes");
        set("SystemCallArchitectures", "native");
        set("SocketBindDeny", "any"); // the helper never listens (CDP is a pipe)
                                      // Egress: deny-all floor + byte accounting (ADR-022 volume budget) +
                                      // the allow-list. The filter is a cgroup eBPF program, so it still binds
                                      // inside the browser's child netns — the `net` namespace relaxation does
                                      // not pierce it (Fable 5).
        set("IPAccounting", "yes");
        set("IPAddressDeny", "any");
        for cidr in &spec.egress_allow {
            set("IPAddressAllow", cidr);
        }
        // Time wall + prompt stop.
        set("RuntimeMaxSec", &format!("{}s", spec.runtime_max_sec));
        set("TimeoutStopSec", "10s");
        // Sealed credential (explicit id:path), decrypted into the helper's
        // creds dir; root vibed never handles the plaintext.
        if let Some(c) = &spec.credential {
            set("LoadCredentialEncrypted", &format!("{}:{}", c.name, c.path));
        }
        set("MemoryMax", &spec.memory_max);

        // ---- per-class divergence (ADR-019 point 4) ------------------------
        match class {
            ToolClass::Deploy { needs_wx } => {
                // Maximum confinement: a deploy CLI creates no namespaces.
                set("RestrictNamespaces", "yes"); // deny ALL namespace creation
                set("CapabilityBoundingSet", ""); // drop every capability
                set("ProcSubset", "pid"); // hide non-PID /proc (safe for a CLI)
                set("TasksMax", "64");
                // W^X is PER-CLI: only when the tool has no JIT (flyctl=Go). A
                // Node CLI (vercel/railway) needs W^X or V8 crashes at boot.
                if !needs_wx {
                    set("MemoryDenyWriteExecute", "yes");
                }
                // getifaddrs()/routing tables use NETLINK_ROUTE (Go net, libuv).
                set(
                    "RestrictAddressFamilies",
                    "AF_INET AF_INET6 AF_UNIX AF_NETLINK",
                );
                set("SystemCallFilter", "@system-service");
                set("SystemCallFilter", "~@privileged @resources");
                // DNS: without this, IPAddressDeny=any blocks the resolver
                // (127.0.0.53 = systemd-resolved) and every provider lookup
                // fails (Fable 5). The provider ranges come from egress_allow.
                set("IPAddressAllow", "127.0.0.53/32");
                // The project sources, read-only, re-exposed past ProtectHome.
                if let Some(w) = &spec.workspace {
                    set("BindReadOnlyPaths", w);
                }
            }
            ToolClass::Browser => {
                // RELAXED, and only where Chromium structurally requires it.
                // Namespace allow-list (user/pid/net for the userns sandbox;
                // `mnt` for the layer-1 mount namespace — ADR-022, to reconfirm
                // by strace). NOT deny-all, or the sandbox cannot initialise.
                set("RestrictNamespaces", "user pid net mnt");
                set("CapabilityBoundingSet", ""); // userns needs no host caps
                set("TasksMax", "512"); // pid ns is open: bound the fork surface
                                        // NO MemoryDenyWriteExecute: V8's JIT needs W^X. Its ABSENCE is
                                        // a tested invariant.
                                        // AF_NETLINK: Chromium's NetworkChangeNotifier (AddressTracker)
                                        // reads NETLINK_ROUTE; removing it degrades the network service.
                set(
                    "RestrictAddressFamilies",
                    "AF_INET AF_INET6 AF_UNIX AF_NETLINK",
                );
                // Keep @sandbox (seccomp layer-2) and chroot (layer-1); do NOT
                // strip @privileged the way deploy does — chroot lives there.
                set("SystemCallFilter", "@system-service @sandbox chroot");
                // NO ProcSubset=pid: Chromium reads /proc/cpuinfo and friends.
                // `--no-sandbox` / `--remote-debugging-port` are forbidden at the
                // argv layer (a later browser increment); the systemd sandbox
                // wraps Chromium's own, it does not replace it.
            }
        }

        Ok(TransientUnit { properties: p })
    }

    /// Value of the first property named `key`, if any.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.properties
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    /// All values for `key` (properties like `IPAddressAllow` and
    /// `SystemCallFilter` legitimately repeat).
    pub fn get_all(&self, key: &str) -> Vec<&str> {
        self.properties
            .iter()
            .filter(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
            .collect()
    }

    /// Render to `systemd-run` arguments: one `--property=KEY=VALUE` per entry,
    /// in declaration order. The spawn increment prepends the fixed, injected
    /// `systemd-run` invocation (absolute path, `--collect`, `--wait`, `--`).
    pub fn to_systemd_run_args(&self) -> Vec<String> {
        self.properties
            .iter()
            .map(|(k, v)| format!("--property={k}={v}"))
            .collect()
    }

    /// Number of compiled properties.
    pub fn len(&self) -> usize {
        self.properties.len()
    }

    /// Whether the profile is empty (never true in practice).
    pub fn is_empty(&self) -> bool {
        self.properties.is_empty()
    }
}

// --------------------------------------------------------------------------
// Fail-closed validation of every value that reaches a `--property=` string.
// Each renders to its own argv (no shell), so the risk is not shell injection
// but a malformed/whitespace-bearing property that smuggles a second directive
// or silently disables one. Conservative allow-lists turn that into an Err.
// --------------------------------------------------------------------------

/// Invocation nonce: a single safe path segment.
fn validate_invocation_id(id: &str) -> Result<(), String> {
    if id.is_empty() || id.len() > 64 {
        return Err("invocation_id must be 1..=64 chars".to_string());
    }
    if id.starts_with('-') {
        return Err("invocation_id must not start with '-'".to_string());
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(format!("invalid invocation_id {id:?} (allowed: [a-z0-9-])"));
    }
    Ok(())
}

/// systemd memory size: a number with an optional K/M/G/T suffix.
fn validate_memory_size(s: &str) -> Result<(), String> {
    let (digits, suffix) = match s.char_indices().find(|(_, c)| !c.is_ascii_digit()) {
        Some((i, _)) => s.split_at(i),
        None => (s, ""),
    };
    if digits.is_empty() {
        return Err(format!("invalid memory size {s:?} (want e.g. 512M, 1G)"));
    }
    if !matches!(suffix, "" | "K" | "M" | "G" | "T") {
        return Err(format!("invalid memory suffix in {s:?} (want K/M/G/T)"));
    }
    Ok(())
}

/// IP or CIDR: conservative charset + no whitespace, so it cannot inject a
/// second property token. Full arithmetic validation is left to systemd (which
/// fails closed on a bad range); this just forbids the injectable shapes.
fn validate_cidr(s: &str) -> Result<(), String> {
    if s.is_empty() || s.len() > 43 {
        return Err(format!("invalid CIDR {s:?} (length)"));
    }
    if !s
        .chars()
        .all(|c| c.is_ascii_hexdigit() || matches!(c, '.' | ':' | '/'))
    {
        return Err(format!(
            "invalid CIDR {s:?} (allowed: hex digits and .:/ — no whitespace)"
        ));
    }
    if !s.chars().any(|c| c.is_ascii_digit()) {
        return Err(format!("invalid CIDR {s:?} (no digits)"));
    }
    Ok(())
}

/// Credential id: a `$CREDENTIALS_DIRECTORY` filename.
fn validate_credential_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.len() > 64 {
        return Err("credential name must be 1..=64 chars".to_string());
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err(format!(
            "invalid credential name {name:?} (allowed: alnum and ._-)"
        ));
    }
    Ok(())
}

/// Absolute path with no whitespace/newline (a property value, and a
/// `BindReadOnlyPaths` src where a space would be parsed as a `:dst` separator).
fn validate_abs_path(pth: &str, what: &str) -> Result<(), String> {
    if pth.is_empty() || pth.len() > 4096 {
        return Err(format!("{what} path has invalid length"));
    }
    if !Path::new(pth).is_absolute() {
        return Err(format!("{what} path must be absolute: {pth:?}"));
    }
    if pth.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(format!("{what} path must not contain whitespace: {pth:?}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deploy_go() -> TransientUnit {
        TransientUnit::compile(
            ToolClass::Deploy { needs_wx: false },
            &UnitSpec {
                invocation_id: "call-abc123".to_string(),
                credential: Some(Credential {
                    name: "deploy-token".to_string(),
                    path: "/var/lib/vibed/creds/myapp.cred".to_string(),
                }),
                egress_allow: vec!["66.241.124.0/24".to_string()],
                memory_max: "256M".to_string(),
                runtime_max_sec: 600,
                workspace: Some("/home/micka/myapp".to_string()),
            },
        )
        .expect("valid deploy(go) spec compiles")
    }

    fn deploy_node() -> TransientUnit {
        TransientUnit::compile(
            ToolClass::Deploy { needs_wx: true },
            &UnitSpec {
                invocation_id: "call-def456".to_string(),
                credential: Some(Credential {
                    name: "deploy-token".to_string(),
                    path: "/var/lib/vibed/creds/myapp.cred".to_string(),
                }),
                egress_allow: vec!["76.76.21.0/24".to_string()],
                memory_max: "512M".to_string(),
                runtime_max_sec: 600,
                workspace: Some("/home/micka/myapp".to_string()),
            },
        )
        .expect("valid deploy(node) spec compiles")
    }

    fn browser() -> TransientUnit {
        TransientUnit::compile(
            ToolClass::Browser,
            &UnitSpec {
                invocation_id: "call-ghi789".to_string(),
                credential: None,
                egress_allow: vec!["127.66.0.1/32".to_string()], // dedicated proxy IP
                memory_max: "1G".to_string(),
                runtime_max_sec: 60,
                workspace: None,
            },
        )
        .expect("valid browser spec compiles")
    }

    /// The single most important invariant: NO class ever runs as the agent's
    /// uid, and no property interpolates `%i`.
    #[test]
    fn no_class_ever_runs_as_the_agent_uid() {
        for u in [deploy_go(), deploy_node(), browser()] {
            assert_eq!(u.get("DynamicUser"), Some("yes"));
            assert_eq!(u.get("User"), None, "a transient tool must never set User=");
            for (k, v) in &u.properties {
                assert!(!v.contains("%i"), "no property may interpolate %i: {k}={v}");
            }
        }
    }

    /// ADR-019 base applies to every class, including the new hardening the
    /// Fable 5 review required.
    #[test]
    fn common_base_is_present_for_every_class() {
        for u in [deploy_go(), deploy_node(), browser()] {
            assert_eq!(u.get("NoNewPrivileges"), Some("yes"));
            assert_eq!(u.get("ProtectSystem"), Some("strict"));
            assert_eq!(u.get("ProtectHome"), Some("yes"));
            assert_eq!(u.get("ProtectProc"), Some("invisible"));
            assert_eq!(u.get("PrivateDevices"), Some("yes"));
            assert_eq!(u.get("RemoveIPC"), Some("yes"));
            assert_eq!(u.get("UMask"), Some("0077"));
            assert_eq!(u.get("SocketBindDeny"), Some("any"));
            // Egress floor + byte accounting (ADR-022 volume budget).
            assert_eq!(u.get("IPAddressDeny"), Some("any"));
            assert_eq!(u.get("IPAccounting"), Some("yes"));
            // Time wall.
            assert!(u.get("RuntimeMaxSec").unwrap().ends_with('s'));
            assert_eq!(u.get("TimeoutStopSec"), Some("10s"));
        }
    }

    /// Concurrency fix: the ephemeral HOME/runtime dir is keyed by the
    /// invocation nonce, so two concurrent calls never share `/run/vibed-tool`.
    #[test]
    fn ephemeral_home_is_keyed_by_invocation_nonce() {
        let b = browser();
        assert_eq!(
            b.get("Environment"),
            Some("HOME=/run/vibed-tool/call-ghi789")
        );
        assert_eq!(
            b.get("WorkingDirectory"),
            Some("/run/vibed-tool/call-ghi789")
        );
        assert_eq!(b.get("RuntimeDirectory"), Some("vibed-tool/call-ghi789"));
        // Different invocations => different HOMEs (no collision).
        assert_ne!(deploy_go().get("Environment"), browser().get("Environment"));
    }

    /// Deploy = maximum confinement (ADR-021).
    #[test]
    fn deploy_profile_is_maximally_confined() {
        let u = deploy_go();
        assert_eq!(u.get("RestrictNamespaces"), Some("yes")); // deny all
        assert_eq!(u.get("CapabilityBoundingSet"), Some(""));
        assert_eq!(u.get("ProcSubset"), Some("pid"));
        assert_eq!(u.get("TasksMax"), Some("64"));
        assert_eq!(
            u.get("LoadCredentialEncrypted"),
            Some("deploy-token:/var/lib/vibed/creds/myapp.cred")
        );
        // Provider range AND the DNS resolver are opened over the floor.
        let allow = u.get_all("IPAddressAllow");
        assert!(
            allow.contains(&"66.241.124.0/24"),
            "provider range: {allow:?}"
        );
        assert!(
            allow.contains(&"127.0.0.53/32"),
            "DNS resolver must be opened: {allow:?}"
        );
        // Workspace re-exposed read-only past ProtectHome.
        assert_eq!(u.get("BindReadOnlyPaths"), Some("/home/micka/myapp"));
        // Syscall filter: allow-list then subtract privileged/resources.
        assert_eq!(
            u.get_all("SystemCallFilter"),
            vec!["@system-service", "~@privileged @resources"]
        );
    }

    /// W^X is per-CLI: flyctl (Go) gets MemoryDenyWriteExecute; a Node CLI does
    /// NOT (it would crash V8) — the A1 bug the Fable 5 review caught.
    #[test]
    fn deploy_wx_is_per_cli_not_per_class() {
        assert_eq!(
            deploy_go().get("MemoryDenyWriteExecute"),
            Some("yes"),
            "a Go CLI (no JIT) must get W^X"
        );
        assert_eq!(
            deploy_node().get("MemoryDenyWriteExecute"),
            None,
            "a Node CLI must NOT get W^X — it would crash the V8 JIT"
        );
    }

    /// Browser = relaxed exactly where Chromium structurally requires it.
    #[test]
    fn browser_profile_relaxes_only_what_chromium_requires() {
        let u = browser();
        let ns = u.get("RestrictNamespaces").unwrap();
        for needed in ["user", "pid", "net", "mnt"] {
            assert!(
                ns.contains(needed),
                "browser ns must allow {needed}: {ns:?}"
            );
        }
        assert_ne!(ns, "yes", "browser must NOT deny all namespaces");
        // V8 JIT needs W^X: absent.
        assert_eq!(u.get("MemoryDenyWriteExecute"), None);
        // No ProcSubset=pid (needs /proc/cpuinfo).
        assert_eq!(u.get("ProcSubset"), None);
        // Syscall filter keeps @sandbox + chroot (layer-1/2), no ~@privileged.
        let scf = u.get_all("SystemCallFilter");
        assert_eq!(scf, vec!["@system-service @sandbox chroot"]);
        assert!(!scf.iter().any(|f| f.contains("~@privileged")));
        // Ephemeral: no credential.
        assert_eq!(u.get("LoadCredentialEncrypted"), None);
        // Egress backstop points only at the dedicated proxy IP, not all
        // loopback (Fable 5 D3).
        assert_eq!(u.get("IPAddressAllow"), Some("127.66.0.1/32"));
    }

    /// The classes genuinely differ on the axes ADR-019 point 4 calls out.
    #[test]
    fn deploy_and_browser_diverge_on_namespaces_and_syscalls() {
        let (d, b) = (deploy_go(), browser());
        assert_ne!(d.get("RestrictNamespaces"), b.get("RestrictNamespaces"));
        assert_ne!(d.get_all("SystemCallFilter"), b.get_all("SystemCallFilter"));
        assert_ne!(d.get("ProcSubset"), b.get("ProcSubset"));
    }

    /// Rendering: one well-formed `--property=` per entry, no agent uid leak.
    #[test]
    fn renders_to_systemd_run_property_args() {
        let args = deploy_go().to_systemd_run_args();
        assert!(args.iter().all(|a| a.starts_with("--property=")));
        assert!(args.contains(&"--property=DynamicUser=yes".to_string()));
        assert!(!args.iter().any(|a| a.contains("%i")));
        assert_eq!(args.len(), deploy_go().len());
    }

    // ---- fail-closed validation ------------------------------------------

    #[test]
    fn compile_rejects_malformed_injected_values() {
        let base = || UnitSpec {
            invocation_id: "ok".to_string(),
            credential: None,
            egress_allow: vec![],
            memory_max: "256M".to_string(),
            runtime_max_sec: 60,
            workspace: None,
        };
        // Bad invocation id (would escape the runtime dir / inject a property).
        let mut s = base();
        s.invocation_id = "../etc".to_string();
        assert!(TransientUnit::compile(ToolClass::Browser, &s).is_err());
        // Whitespace in a CIDR (would smuggle a second property token).
        let mut s = base();
        s.egress_allow = vec!["127.0.0.1/32 --property=User=0".to_string()];
        assert!(TransientUnit::compile(ToolClass::Browser, &s).is_err());
        // Bogus memory size.
        let mut s = base();
        s.memory_max = "lots".to_string();
        assert!(TransientUnit::compile(ToolClass::Browser, &s).is_err());
        // Relative / newline-bearing workspace path.
        let mut s = base();
        s.workspace = Some("relative/path".to_string());
        assert!(TransientUnit::compile(ToolClass::Deploy { needs_wx: false }, &s).is_err());
        let mut s = base();
        s.workspace = Some("/etc/x\n/etc/y".to_string());
        assert!(TransientUnit::compile(ToolClass::Deploy { needs_wx: false }, &s).is_err());
    }

    #[test]
    fn validators_accept_the_real_shapes() {
        assert!(validate_invocation_id("call-abc123").is_ok());
        assert!(validate_memory_size("512M").is_ok());
        assert!(validate_memory_size("1G").is_ok());
        assert!(validate_memory_size("104857600").is_ok());
        assert!(validate_cidr("66.241.124.0/24").is_ok());
        assert!(validate_cidr("2606:2800::/32").is_ok());
        assert!(validate_credential_name("deploy-token").is_ok());
        assert!(validate_abs_path("/var/lib/vibed/creds/x.cred", "credential path").is_ok());
    }
}
