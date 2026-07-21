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

    /// Assemble the full `systemd-run` argv that launches this hardened unit and
    /// exec's the low-privilege helper. No shell — each element is one argv slot,
    /// passed straight to `Command` (the spawn increment injects the binary path
    /// the way `tools/svc.rs` injects `systemctl`).
    ///
    /// Fixed flags:
    ///   * `--pipe --wait` — run to completion, forwarding the caller's stdio so
    ///     the control payload (down) and the bounded result (up) cross the
    ///     pid-1 boundary that a literal `socketpair` cannot: systemd (not
    ///     `vibed`) exec's the service, so `vibed` hands the stdio to
    ///     `systemd-run`, which is its own child. This is ADR-019's one-way IPC
    ///     realised as systemd-run stdio — the mechanism, not a literal socket.
    ///   * `--collect` — GC the transient unit when it exits.
    ///   * `--quiet` — no systemd-run chatter on stdout, so the result channel
    ///     stays clean.
    ///   * **no `--unit=`** — let systemd auto-name `run-<nnn>.service`; a fixed
    ///     unit name would let two concurrent invocations collide (Fable 5).
    ///
    /// A `--` fence terminates option parsing before the helper, so a helper
    /// arg that looks like an option can never be read by `systemd-run`.
    pub fn systemd_run_command(
        &self,
        systemd_run_bin: &str,
        helper_bin: &str,
        helper_args: &[String],
    ) -> Vec<String> {
        let mut argv = vec![
            systemd_run_bin.to_string(),
            "--pipe".to_string(),
            "--wait".to_string(),
            "--collect".to_string(),
            "--quiet".to_string(),
        ];
        argv.extend(self.to_systemd_run_args());
        argv.push("--".to_string());
        argv.push(helper_bin.to_string());
        argv.extend(helper_args.iter().cloned());
        argv
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

/// The captured outcome of a transient-service invocation.
#[derive(Debug)]
pub struct ToolOutput {
    /// Exit code of `systemd-run` (which propagates the service's), or `None`
    /// if it was killed by a signal. `systemd-run` returns non-zero on a helper
    /// failure, a `RuntimeMaxSec` timeout, or a missing binary — vibed also
    /// reads [`stderr`](Self::stderr) to tell these apart (Fable 5).
    pub status: Option<i32>,
    /// The bounded RESULT channel: the helper's stdout, forwarded by `--pipe`.
    pub stdout: String,
    /// Diagnostics: `systemd-run`'s own chatter (it writes to stderr, never
    /// stdout) and the helper's stderr, captured on a SEPARATE pipe and never
    /// merged into the result channel (Fable 5, verified empirically).
    pub stderr: String,
}

/// Cap on the RESULT channel (helper stdout) buffered in root `vibed`'s memory.
/// `MemoryMax` bounds the helper's cgroup, NOT the bytes it writes to a pipe, so
/// without this a hostile helper could flood stdout and OOM the root daemon
/// (Fable 5). 8 MiB is far above any legitimate bounded result.
const STDOUT_CAP: usize = 8 * 1024 * 1024;
/// Cap on captured diagnostics (helper stderr + systemd-run chatter).
const STDERR_CAP: usize = 256 * 1024;

/// Spawn the hardened transient service and capture its outcome.
///
/// `vibed` (root) exec's **`systemd-run`, not the tool**, with the cleared
/// environment and absolute-binary discipline of `tools/svc.rs`: a compromised
/// environment can never redirect what the root daemon launches. `systemd-run`
/// then exec's the low-privilege helper under the compiled hardening profile.
///
/// The `control_payload` is written to the service's stdin via `--pipe` on a
/// dedicated thread while two reader threads drain stdout (the result) and
/// stderr (diagnostics) on **separate**, **capped** buffers — deadlock-free for
/// any size, and the capture can never OOM the root daemon. stdout must be valid
/// UTF-8 (it is a JSON result channel; invalid bytes are a corruption/attack
/// signal, so we fail closed rather than lossily patch them — Fable 5); stderr
/// is lossy (free-form diagnostics). On an over-cap flood the child is killed
/// and the call fails closed.
///
/// Time-bounding is delegated to the profile's `RuntimeMaxSec` + `--wait` (the
/// same reliance on the child's own bound that `svc.rs` documents for systemctl).
///
/// NOTE: the *systemd behaviour* this drives can only be validated on the target
/// (a full-systemd Fedora host); the caller side is exercised against a fake
/// `systemd-run` exactly as `svc.rs` is exercised against a fake systemctl.
pub fn spawn_transient(
    systemd_run_bin: &str,
    unit: &TransientUnit,
    helper_bin: &str,
    helper_args: &[String],
    control_payload: &[u8],
) -> Result<ToolOutput, String> {
    spawn_transient_capped(
        systemd_run_bin,
        unit,
        helper_bin,
        helper_args,
        control_payload,
        STDOUT_CAP,
        STDERR_CAP,
    )
}

/// Backend of [`spawn_transient`] with the output caps injected, so a test can
/// drive the over-cap path with tiny limits instead of megabytes.
fn spawn_transient_capped(
    systemd_run_bin: &str,
    unit: &TransientUnit,
    helper_bin: &str,
    helper_args: &[String],
    control_payload: &[u8],
    stdout_cap: usize,
    stderr_cap: usize,
) -> Result<ToolOutput, String> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let argv = unit.systemd_run_command(systemd_run_bin, helper_bin, helper_args);
    let mut child = Command::new(&argv[0])
        .args(&argv[1..])
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn_transient: cannot exec {}: {e}", argv[0]))?;

    // Take all three pipes. Each `take` is infallible right after `Stdio::piped()`.
    let mut stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");

    // One thread writes the control payload (then drops stdin => EOF); two drain
    // the output pipes with a hard cap. Writing and reading concurrently is
    // deadlock-free for any size; the caps keep a hostile flood out of root
    // vibed's heap. Readers ALWAYS drain to EOF (discarding past the cap) so the
    // child never blocks on a full pipe.
    let payload = control_payload.to_vec();
    let writer = std::thread::spawn(move || stdin.write_all(&payload));
    let out_reader = std::thread::spawn(move || read_capped(stdout, stdout_cap));
    let err_reader = std::thread::spawn(move || read_capped(stderr, stderr_cap));

    let out_joined = out_reader.join();
    let err_joined = err_reader.join();
    let _ = writer.join(); // best-effort; a write failure surfaces as a run failure

    let unwrap_reader = |joined: std::thread::Result<std::io::Result<(Vec<u8>, bool)>>,
                         which: &str|
     -> Result<(Vec<u8>, bool), String> {
        match joined {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(e)) => Err(format!("spawn_transient: reading {which}: {e}")),
            Err(_) => Err(format!("spawn_transient: {which} reader panicked")),
        }
    };
    let out = unwrap_reader(out_joined, "stdout");
    let err = unwrap_reader(err_joined, "stderr");

    // On ANY problem (reader error or over-cap flood) kill before reaping so a
    // long-lived root daemon never leaks a zombie (Fable 5); otherwise the
    // readers already hit EOF, meaning the child has exited.
    let overflow = matches!(&out, Ok((_, true))) || matches!(&err, Ok((_, true)));
    if out.is_err() || err.is_err() || overflow {
        let _ = child.kill();
    }
    let status = child
        .wait()
        .map_err(|e| format!("spawn_transient: reaping systemd-run: {e}"))?;

    let (out_buf, _) = out?;
    let (err_buf, _) = err?;
    if overflow {
        return Err(format!(
            "spawn_transient: helper exceeded output cap (stdout {stdout_cap} / stderr {stderr_cap} bytes) — killed and refused"
        ));
    }

    // Result channel must be valid UTF-8 (JSON); diagnostics may be anything.
    let stdout = String::from_utf8(out_buf).map_err(|_| {
        "spawn_transient: helper stdout is not valid UTF-8 (result channel)".to_string()
    })?;
    let stderr = String::from_utf8_lossy(&err_buf).into_owned();
    Ok(ToolOutput {
        status: status.code(),
        stdout,
        stderr,
    })
}

/// Read `r` to EOF, keeping at most `cap` bytes; returns `(kept, overflowed)`.
/// Keeps draining past the cap (discarding) so the writer never blocks on a full
/// pipe — the cap bounds MEMORY, not whether we read to the end.
fn read_capped<R: std::io::Read>(mut r: R, cap: usize) -> std::io::Result<(Vec<u8>, bool)> {
    let mut kept = Vec::new();
    let mut chunk = [0u8; 16 * 1024];
    let mut overflowed = false;
    loop {
        let n = r.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        if kept.len() < cap {
            let take = (cap - kept.len()).min(n);
            kept.extend_from_slice(&chunk[..take]);
            if take < n {
                overflowed = true;
            }
        } else {
            overflowed = true;
        }
    }
    Ok((kept, overflowed))
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

/// The low-privilege `vibed-tool` helper's logic, kept in the library so it is
/// unit-tested here while `src/bin/vibed-tool.rs` stays a thin `main`.
///
/// This runs **inside** the transient hardened service (a distinct dynamic uid).
/// `vibed` (root) never runs the tool itself — it spawns `systemd-run`, which
/// exec's this helper. Modes:
///   * `probe` — report the helper's OWN confinement so the caller can PROVE the
///     ADR-019 profile applied. The **strong** evidence is `/proc/self/status`
///     (`Seccomp: 2` ⇒ a syscall filter is installed, `CapBnd`/`CapEff` all-zero
///     ⇒ the empty bounding set took, `NoNewPrivs: 1`, `Umask: 0077`) plus a
///     **behavioural** namespace test: `unshare(CLONE_NEWUSER)` returns EPERM
///     under the deploy deny-all profile and succeeds under the browser
///     allow-list. The namespace **inode ids** are reported only for the caller
///     to COMPARE against its own — on their own they prove nothing, because
///     `RestrictNamespaces` blocks *creating* namespaces, it does not *move* the
///     service into new ones (verified: a deploy unit shares the host
///     user/pid/net namespace; only `mnt` differs, from `ProtectSystem`).
///   * `deploy`, `browser` — later increments (run the CLI / drive Chromium).
pub mod helper {
    use std::collections::BTreeMap;
    use std::os::unix::fs::MetadataExt;

    /// Dispatch on the mode (argv[1]). Unknown modes fail closed.
    pub fn run(mode: &str) -> Result<String, String> {
        match mode {
            "probe" => Ok(probe_report()),
            "deploy" => run_deploy(),
            // Mode navigateur — unix-only (chromium / CDP sur pipe). Le code vit dans
            // `browser_transport` (`#![cfg(unix)]`) ; l'arm est absente hors unix → « unknown ».
            #[cfg(unix)]
            "browser" => crate::browser_transport::run_browser(),
            other => Err(format!(
                "unknown vibed-tool mode {other:?} (known: probe, deploy, browser)"
            )),
        }
    }

    /// The command `deploy` runs, sent by root `vibed` on this helper's stdin.
    /// `vibed` is the sole writer of that channel (it holds the `systemd-run`
    /// pipe), so the request is trusted; the CLI it names, however, is HOSTILE.
    #[derive(serde::Deserialize)]
    struct DeployRequest {
        /// Absolute CLI binary path (chosen by vibed from the provider).
        binary: String,
        /// argv AFTER the binary. NEVER carries the token (ADR-021 lock 1).
        argv: Vec<String>,
        /// Env var the CLI reads the token from.
        token_env: String,
        /// Credential filename under `$CREDENTIALS_DIRECTORY` holding the sealed
        /// token for this target.
        cred_name: String,
    }

    /// `deploy` mode: run a deploy CLI's read-only command inside the confined
    /// service and return its BOUNDED output as DATA. The CLI is HOSTILE — its
    /// stdout is captured, never trusted or interpreted (unlike `probe`).
    ///
    /// fd hygiene (Fable 5): the control payload is read fully from OUR stdin
    /// FIRST; the CLI is then spawned with stdin CLOSED (it can never see the
    /// residual payload), an env cleared to exactly `{token_env: <sealed token>,
    /// HOME: <ephemeral>}` (the token comes from `$CREDENTIALS_DIRECTORY`, NEVER
    /// argv), and captured stdout/stderr — it inherits none of our descriptors.
    fn run_deploy() -> Result<String, String> {
        use std::io::Read;
        // The control channel is vibed (trusted), but cap the read defensively.
        let mut payload = String::new();
        std::io::stdin()
            .take(1 << 20)
            .read_to_string(&mut payload)
            .map_err(|e| format!("deploy: reading control payload: {e}"))?;
        let req: DeployRequest = serde_json::from_str(payload.trim())
            .map_err(|e| format!("deploy: malformed control payload: {e}"))?;
        let creds = std::env::var("CREDENTIALS_DIRECTORY")
            .map_err(|_| "deploy: no $CREDENTIALS_DIRECTORY (token not sealed in?)".to_string())?;
        // HOME is set by the unit (`Environment=HOME=…`); its absence is an
        // impossible branch — fail closed rather than hand the CLI `HOME=""`.
        let home = std::env::var("HOME")
            .map_err(|_| "deploy: no $HOME in the transient unit".to_string())?;
        run_cli(&req, &creds, &home)
    }

    /// Backend of [`run_deploy`] with the credentials dir + HOME injected, so the
    /// exec path is testable against a fake CLI and a scratch credstore.
    fn run_cli(req: &DeployRequest, creds_dir: &str, home: &str) -> Result<String, String> {
        use std::process::{Command, Stdio};
        // The binary MUST be an absolute path — never a name resolved via PATH
        // (murky under `env_clear`; Fable 5). The spawn layer pins it to an
        // allow-listed CLI path.
        if !std::path::Path::new(&req.binary).is_absolute() {
            return Err(format!(
                "deploy: binary must be an absolute path: {:?}",
                req.binary
            ));
        }
        // Credstore filename must be a bare name, never a path.
        if req.cred_name.is_empty() || req.cred_name.contains('/') || req.cred_name.contains("..") {
            return Err(format!(
                "deploy: invalid credential name {:?}",
                req.cred_name
            ));
        }
        // Defence in depth — NOT the primary read-only control (that is the
        // read-only-scoped sealed token + the [rule.deploy] verdict): refuse an
        // obviously-mutating first subcommand. An INCOMPLETE deny-list by nature;
        // only a read-only token is the real end-to-end guarantee.
        if let Some(sub) = req.argv.first() {
            const MUTATING: &[&str] = &[
                "deploy", "up", "redeploy", "launch", "rm", "remove", "destroy", "delete", "scale",
                "restart", "secrets", "env", "create", "add",
            ];
            if MUTATING.iter().any(|m| sub == m) {
                return Err(format!(
                    "deploy.plan: refusing a mutating subcommand {sub:?} (read-only only)"
                ));
            }
        }
        let token_path = std::path::Path::new(creds_dir).join(&req.cred_name);
        let token = std::fs::read_to_string(&token_path).map_err(|e| {
            format!(
                "deploy: cannot read sealed token {}: {e}",
                token_path.display()
            )
        })?;
        let token = token.trim().to_string();
        if token.is_empty() {
            return Err("deploy: sealed token is empty".to_string());
        }
        // Run the CLI: cleared env + token + ephemeral HOME, stdin CLOSED, output
        // captured. `Command` never shell-splits, so argv reaches the CLI verbatim.
        // TLS still works: fly (Go), vercel (Node) and railway (rustls) all find
        // CA roots without SSL_CERT_* — via compiled paths or an embedded store —
        // and `/etc` stays readable under ProtectSystem=strict (Fable 5, verify on
        // target). Residual exfil: the raw-echo-to-stdout/stderr axis is REDACTED
        // below (a hostile CLI cannot hand the agent its own token that way); a
        // re-encoded or DNS-tunnelled leak remains, bounded by the read-only TOKEN
        // SCOPE — the real guarantee, never this helper alone.
        let mut child = Command::new(&req.binary)
            .args(&req.argv)
            .env_clear()
            .env(&req.token_env, &token)
            .env("HOME", home)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("deploy: cannot exec {}: {e}", req.binary))?;
        // `.env` already copied the token into the child. We keep our copy a moment
        // longer — only to REDACT it out of the CLI's own stdout/stderr below — then
        // drop it. See the redaction note before the `Ok(...)`.

        // Bounded capture (Fable 5): reuse the module's `read_capped` so a
        // flooding CLI gives a deterministic error, not a noisy cgroup OOM.
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let out_h = std::thread::spawn(move || super::read_capped(stdout, 4 * 1024 * 1024));
        let err_h = std::thread::spawn(move || super::read_capped(stderr, 256 * 1024));
        let out = out_h
            .join()
            .map_err(|_| "deploy: stdout reader panicked".to_string())?
            .map_err(|e| format!("deploy: reading CLI stdout: {e}"))?;
        let err = err_h
            .join()
            .map_err(|_| "deploy: stderr reader panicked".to_string())?
            .map_err(|e| format!("deploy: reading CLI stderr: {e}"))?;
        let overflow = out.1 || err.1;
        if overflow {
            let _ = child.kill();
        }
        let status = child
            .wait()
            .map_err(|e| format!("deploy: reaping the CLI: {e}"))?;
        if overflow {
            return Err("deploy: CLI output exceeded cap — killed and refused".to_string());
        }
        // Belt-and-suspenders (Fable 5): a hostile CLI could echo its own token to
        // stdout/stderr to hand the agent the credential. Redact every RAW
        // occurrence of the token here — where the helper still holds it and vibed
        // never will — before the output leaves this process. This closes only the
        // raw-echo axis (not a re-encoded or DNS-tunnelled leak); the real bound
        // stays the read-only token scope. Redact, THEN drop our copy.
        let stdout = redact(&out.0, token.as_bytes());
        let stderr = redact(&err.0, token.as_bytes());
        drop(token);
        // The CLI output is DATA, not a trusted result channel: lossy is fine, and
        // vibed bounds the size again on its side (spawn_transient's cap).
        Ok(serde_json::json!({
            "exit": status.code(),
            "stdout": String::from_utf8_lossy(&stdout),
            "stderr": String::from_utf8_lossy(&stderr),
        })
        .to_string())
    }

    /// Replace every RAW occurrence of `secret` in `data` with a redaction
    /// marker, so a hostile deploy CLI cannot exfiltrate its own token by echoing
    /// it to stdout/stderr. Byte-exact only — a re-encoded (base64, hex…) or
    /// tunnelled leak is out of scope; this is defense-in-depth atop the read-only
    /// token scope, not a substitute for it. Empty `secret` is a no-op.
    fn redact(data: &[u8], secret: &[u8]) -> Vec<u8> {
        const MARK: &[u8] = b"[redacted]";
        if secret.is_empty() || data.len() < secret.len() {
            return data.to_vec();
        }
        let mut out = Vec::with_capacity(data.len());
        let mut i = 0;
        while i < data.len() {
            if data[i..].starts_with(secret) {
                out.extend_from_slice(MARK);
                i += secret.len();
            } else {
                out.push(data[i]);
                i += 1;
            }
        }
        out
    }

    /// Gather this process's confinement facts as JSON. Reads only its own
    /// `/proc/self`, `stat`s `$HOME`, lists `$CREDENTIALS_DIRECTORY` (names only),
    /// and attempts one unprivileged `unshare` **last**. Linux-only.
    pub fn probe_report() -> String {
        let st = status_fields();
        let get = |k: &str| st.get(k).cloned();

        // "Uid:\t<real>\t<eff>\t<saved>\t<fs>" — report real and effective.
        let uid_line = get("Uid").unwrap_or_default();
        let mut uid_parts = uid_line.split_whitespace();
        let uid = uid_parts.next().and_then(|s| s.parse::<u64>().ok());
        let euid = uid_parts.next().and_then(|s| s.parse::<u64>().ok());

        // Namespace inode ids: for COMPARISON by the caller, not proof. A read
        // failure is anomalous (`/proc/self/ns/*` survives ProcSubset=pid), so
        // surface the errno rather than a neutral string (fail-closed signal).
        let ns = |kind: &str| match std::fs::read_link(format!("/proc/self/ns/{kind}")) {
            Ok(p) => p.to_string_lossy().into_owned(),
            Err(e) => format!("error:{}", e.raw_os_error().unwrap_or(-1)),
        };

        // HOME must be an ephemeral tmpfs dir OWNED by us in 0700 — not merely a
        // string under /run (which only proves the env var was set).
        let home = std::env::var("HOME").unwrap_or_default();
        let (home_owner, home_mode) = match std::fs::metadata(&home) {
            Ok(m) => (Some(m.uid()), Some(format!("{:04o}", m.mode() & 0o7777))),
            Err(_) => (None, None),
        };
        let home_ephemeral = home.starts_with("/run/vibed-tool/")
            && !home.contains("..")
            && home_owner.is_some()
            && home_owner.map(u64::from) == euid
            && home_mode.as_deref() == Some("0700");

        let cwd = std::env::current_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();

        // Credential NAMES only — never contents.
        let credentials = std::env::var_os("CREDENTIALS_DIRECTORY").and_then(|d| {
            std::fs::read_dir(&d).ok().map(|rd| {
                rd.filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
                    .collect::<Vec<_>>()
            })
        });

        // Behavioural namespace proof — LAST, since on success we land in a fresh
        // user namespace and then only serialise + exit.
        let unshare_user = unshare_userns_result();

        serde_json::json!({
            "tool": "vibed-tool",
            "mode": "probe",
            "uid": uid,
            "euid": euid,
            "no_new_privs": get("NoNewPrivs"),
            "seccomp": get("Seccomp"),
            "seccomp_filters": get("Seccomp_filters"),
            "cap_bnd": get("CapBnd"),
            "cap_eff": get("CapEff"),
            "umask": get("Umask"),
            "nspid": get("NSpid"),
            "ns_user": ns("user"),
            "ns_pid": ns("pid"),
            "ns_net": ns("net"),
            "ns_mnt": ns("mnt"),
            "home": home,
            "home_owner_uid": home_owner,
            "home_mode": home_mode,
            "home_ephemeral": home_ephemeral,
            "cwd": cwd,
            "credentials": credentials,
            "unshare_user": unshare_user,
        })
        .to_string()
    }

    /// Parse `/proc/self/status` into `{field: value}` (value trimmed).
    fn status_fields() -> BTreeMap<String, String> {
        let mut m = BTreeMap::new();
        if let Ok(s) = std::fs::read_to_string("/proc/self/status") {
            for line in s.lines() {
                if let Some((k, v)) = line.split_once(':') {
                    m.insert(k.trim().to_string(), v.trim().to_string());
                }
            }
        }
        m
    }

    /// Behavioural test of `RestrictNamespaces`: attempt to create a user
    /// namespace. `"eperm"` (deploy deny-all), `"ok"` (browser allow-list), or
    /// `"error:<errno>"`.
    fn unshare_userns_result() -> String {
        // SAFETY: `unshare(CLONE_NEWUSER)` affects only this process, and we call
        // it last — nothing after reads the namespace. On success we sit in a
        // fresh user namespace and immediately serialise + exit.
        let rc = unsafe { libc::unshare(libc::CLONE_NEWUSER) };
        if rc == 0 {
            return "ok".to_string();
        }
        match std::io::Error::last_os_error().raw_os_error() {
            Some(libc::EPERM) => "eperm".to_string(),
            Some(n) => format!("error:{n}"),
            None => "error".to_string(),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn redact_scrubs_every_raw_occurrence_of_the_token() {
            let token = b"fo1_secret-TOKEN.value";
            // A hostile CLI echoes the token — twice, adjacent to real output.
            let hostile = b"before fo1_secret-TOKEN.value middle fo1_secret-TOKEN.value end";
            let cleaned = redact(hostile, token);
            let s = String::from_utf8(cleaned).unwrap();
            assert_eq!(s, "before [redacted] middle [redacted] end");
            assert!(!s.contains("fo1_secret-TOKEN.value"));
            // Back-to-back occurrences (no gap) are both scrubbed.
            let glued = b"fo1_secret-TOKEN.valuefo1_secret-TOKEN.value";
            assert_eq!(
                String::from_utf8(redact(glued, token)).unwrap(),
                "[redacted][redacted]"
            );
            // No token present => byte-identical passthrough.
            assert_eq!(redact(b"clean output", token), b"clean output");
            // Empty secret is a no-op (never turns every gap into a marker).
            assert_eq!(redact(b"abc", b""), b"abc");
        }

        #[test]
        fn probe_report_is_valid_json_with_strong_confinement_keys() {
            let out = run("probe").expect("probe mode runs");
            let v: serde_json::Value = serde_json::from_str(&out).expect("probe emits valid JSON");
            assert_eq!(v["tool"], "vibed-tool");
            assert_eq!(v["mode"], "probe");
            // uid must be a real number, not null (a null would be a real signal).
            assert!(v["uid"].is_u64(), "uid must parse to a number: {out}");
            // The strong-proof keys the on-target checker reads.
            for k in [
                "seccomp",
                "cap_bnd",
                "cap_eff",
                "no_new_privs",
                "umask",
                "ns_mnt",
                "home_ephemeral",
                "unshare_user",
            ] {
                assert!(v.get(k).is_some(), "probe report must carry {k}: {out}");
            }
            // The behavioural namespace test reports one of the known outcomes.
            let u = v["unshare_user"].as_str().unwrap();
            assert!(
                u == "ok" || u == "eperm" || u.starts_with("error:"),
                "unexpected unshare_user {u:?}"
            );
        }

        #[test]
        fn unknown_mode_fails_closed() {
            assert!(run("browse").is_err(), "an unknown mode must fail closed");
            assert!(run("").is_err());
            assert!(run("../etc/passwd").is_err());
        }

        /// `run_cli` execs the CLI with the sealed token in ITS ENV (never argv),
        /// passes argv verbatim, and captures the (hostile) output as data.
        #[cfg(unix)]
        #[test]
        fn run_cli_passes_token_by_env_never_argv_and_captures_output() {
            use std::os::unix::fs::PermissionsExt;
            let dir = std::env::temp_dir().join(format!("vibed-deploy-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            // Fake CLI: prints its argv and the token env var, so the test can prove
            // the token arrived via env and the argv arrived verbatim.
            let cli = dir.join("fakecli");
            std::fs::write(
                &cli,
                "#!/bin/sh\necho \"ARGV:$*\"\necho \"TOK:$FLY_API_TOKEN\"\n",
            )
            .unwrap();
            std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o755)).unwrap();
            std::fs::write(dir.join("deploy-token"), "secret-xyz\n").unwrap();

            let req = DeployRequest {
                binary: cli.to_string_lossy().into_owned(),
                argv: vec!["status".to_string(), "-a".to_string(), "my-app".to_string()],
                token_env: "FLY_API_TOKEN".to_string(),
                cred_name: "deploy-token".to_string(),
            };
            let creds = dir.to_string_lossy().into_owned();
            // Retry the TEST-ONLY ETXTBSY race on the just-written fake.
            let out = (|| {
                for _ in 0..50 {
                    match run_cli(&req, &creds, "/run/vibed-tool/x") {
                        Err(e) if e.contains("Text file busy") => {
                            std::thread::sleep(std::time::Duration::from_millis(20))
                        }
                        other => return other,
                    }
                }
                run_cli(&req, &creds, "/run/vibed-tool/x")
            })()
            .expect("run_cli succeeds");
            let v: serde_json::Value = serde_json::from_str(&out).unwrap();
            assert_eq!(v["exit"], 0);
            let stdout = v["stdout"].as_str().unwrap();
            assert!(
                stdout.contains("ARGV:status -a my-app"),
                "argv verbatim: {stdout}"
            );
            // The child received the token via env (its TOK line is non-empty),
            // but the helper REDACTED the raw value on the way out — a hostile CLI
            // cannot exfiltrate its own token by echoing it to stdout (Fable 5).
            assert!(
                stdout.contains("TOK:[redacted]"),
                "token delivered via env, then redacted: {stdout}"
            );
            assert!(
                !stdout.contains("secret-xyz"),
                "raw token must never leave the helper: {stdout}"
            );
            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn run_cli_rejects_a_credential_path_traversal() {
            let req = DeployRequest {
                binary: "/bin/true".to_string(),
                argv: vec![],
                token_env: "X".to_string(),
                cred_name: "../etc/passwd".to_string(),
            };
            assert!(run_cli(&req, "/tmp", "/tmp")
                .unwrap_err()
                .contains("invalid credential name"));
        }

        #[test]
        fn run_cli_refuses_a_relative_binary_and_a_mutating_subcommand() {
            let req = |bin: &str, sub: &str| DeployRequest {
                binary: bin.to_string(),
                argv: if sub.is_empty() {
                    vec![]
                } else {
                    vec![sub.to_string()]
                },
                token_env: "X".to_string(),
                cred_name: "deploy-token".to_string(),
            };
            // Relative binary => refused (no murky PATH resolution).
            assert!(run_cli(&req("flyctl", "status"), "/tmp", "/tmp")
                .unwrap_err()
                .contains("absolute path"));
            // A mutating first subcommand => refused (defence in depth).
            for m in ["deploy", "up", "destroy", "scale", "secrets"] {
                assert!(
                    run_cli(&req("/usr/bin/flyctl", m), "/tmp", "/tmp")
                        .unwrap_err()
                        .contains("mutating subcommand"),
                    "must refuse {m}"
                );
            }
        }

        /// `env_clear` really strips the parent env: only HOME + the token var
        /// (plus what `sh` itself sets) reach the CLI — a canary leaks nothing.
        #[cfg(unix)]
        #[test]
        fn run_cli_clears_the_parent_env() {
            use std::os::unix::fs::PermissionsExt;
            std::env::set_var("VIBED_LEAK_CANARY", "leaked");
            let dir = std::env::temp_dir().join(format!("vibed-denv-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let cli = dir.join("envcli");
            std::fs::write(&cli, "#!/bin/sh\nenv\n").unwrap();
            std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o755)).unwrap();
            std::fs::write(dir.join("deploy-token"), "sekret\n").unwrap();
            let req = DeployRequest {
                binary: cli.to_string_lossy().into_owned(),
                argv: vec![],
                token_env: "FLY_API_TOKEN".to_string(),
                cred_name: "deploy-token".to_string(),
            };
            let creds = dir.to_string_lossy().into_owned();
            let out = (|| {
                for _ in 0..50 {
                    match run_cli(&req, &creds, "/run/vibed-tool/x") {
                        Err(e) if e.contains("Text file busy") => {
                            std::thread::sleep(std::time::Duration::from_millis(20))
                        }
                        other => return other,
                    }
                }
                run_cli(&req, &creds, "/run/vibed-tool/x")
            })()
            .expect("runs");
            let v: serde_json::Value = serde_json::from_str(&out).unwrap();
            let env = v["stdout"].as_str().unwrap();
            // The token var IS set in the child's env (delivery), but its value is
            // redacted out of the returned stdout (Fable 5 exfil hardening).
            assert!(
                env.contains("FLY_API_TOKEN=[redacted]"),
                "token var present, value redacted: {env}"
            );
            assert!(
                !env.contains("sekret"),
                "raw token must not leave the helper: {env}"
            );
            assert!(
                env.contains("HOME=/run/vibed-tool/x"),
                "home present: {env}"
            );
            assert!(
                !env.contains("VIBED_LEAK_CANARY"),
                "parent env must be cleared: {env}"
            );
            std::env::remove_var("VIBED_LEAK_CANARY");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
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

    /// The full systemd-run argv: fixed hardening flags, every property, a `--`
    /// fence, then the helper — and no fixed `--unit=` (auto-named, no collision).
    #[test]
    fn systemd_run_command_builds_a_fenced_argv() {
        let u = browser();
        let argv = u.systemd_run_command(
            "/usr/bin/systemd-run",
            "/usr/libexec/vibed-tool",
            &["probe".to_string()],
        );
        assert_eq!(argv[0], "/usr/bin/systemd-run");
        for flag in ["--pipe", "--wait", "--collect", "--quiet"] {
            assert!(argv.iter().any(|a| a == flag), "missing {flag}: {argv:?}");
        }
        assert!(
            !argv.iter().any(|a| a.starts_with("--unit")),
            "must not fix a unit name"
        );
        // The properties are carried through.
        assert!(argv.iter().any(|a| a == "--property=DynamicUser=yes"));
        // The `--` fence sits immediately before the helper binary.
        let fence = argv.iter().position(|a| a == "--").expect("a -- fence");
        assert_eq!(argv[fence + 1], "/usr/libexec/vibed-tool");
        assert_eq!(argv[fence + 2], "probe");
        // Everything between the binary (argv[0]) and the fence is a flag/property;
        // the helper argv is after it.
        assert!(argv[1..fence].iter().all(|a| a.starts_with("--")));
    }

    /// The profile must never set `Standard*`: `systemd-run --pipe` relies on the
    /// unit inheriting systemd-run's stdio, and a `StandardInput=`/`Output=`
    /// property would silently clobber the IPC channel (Fable 5).
    #[test]
    fn profile_never_sets_stdio_which_would_clobber_the_pipe() {
        for u in [deploy_go(), deploy_node(), browser()] {
            assert_eq!(u.get("StandardInput"), None);
            assert_eq!(u.get("StandardOutput"), None);
            assert_eq!(u.get("StandardError"), None);
        }
    }

    // ---- spawn (against a fake systemd-run, like svc.rs' fake systemctl) --

    /// Write an executable fake `systemd-run` that records its argv, the control
    /// payload it receives on stdin, and whether `$VIBED_CANARY` survived into
    /// its environment; then writes `stdout_content` (via `printf %b`, so
    /// `\0377` = a raw byte) to stdout and chatter to stderr, and exits with
    /// `exit`. Shell builtins only, so it runs under `env_clear()` (empty PATH).
    #[cfg(unix)]
    fn fake_systemd_run(
        dir: &std::path::Path,
        argv_marker: &std::path::Path,
        payload_marker: &std::path::Path,
        env_marker: &std::path::Path,
        stdout_content: &str,
        exit: i32,
    ) -> String {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("systemd-run");
        let script = format!(
            "#!/bin/sh\n\
             echo \"$@\" > '{argv}'\n\
             printf '%s' \"${{VIBED_CANARY:-absent}}\" > '{env}'\n\
             IFS= read -r payload\n\
             printf '%s' \"$payload\" > '{payload}'\n\
             printf '%b' '{result}'\n\
             echo 'run-uXX.service: chatter to stderr' >&2\n\
             exit {exit}\n",
            argv = argv_marker.display(),
            env = env_marker.display(),
            payload = payload_marker.display(),
            result = stdout_content,
            exit = exit,
        );
        std::fs::write(&path, script).expect("write fake systemd-run");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path.to_string_lossy().into_owned()
    }

    /// Retry around the TEST-ONLY ETXTBSY race (a parallel test's fork() can
    /// transiently hold our write fd to the just-written fake), with the output
    /// caps injected so a test can drive the over-cap path. Production only ever
    /// execs the static `/usr/bin/systemd-run`.
    #[cfg(unix)]
    fn spawn_retry(
        bin: &str,
        unit: &TransientUnit,
        args: &[String],
        payload: &[u8],
        out_cap: usize,
        err_cap: usize,
    ) -> Result<ToolOutput, String> {
        let helper = "/usr/libexec/vibed-tool";
        for _ in 0..50 {
            match spawn_transient_capped(bin, unit, helper, args, payload, out_cap, err_cap) {
                Err(e) if e.contains("Text file busy") => {
                    std::thread::sleep(std::time::Duration::from_millis(20))
                }
                other => return other,
            }
        }
        spawn_transient_capped(bin, unit, helper, args, payload, out_cap, err_cap)
    }

    /// A fresh, unique temp dir for a spawn test's fake + markers.
    #[cfg(unix)]
    fn spawn_tmp(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("vibed-spawn-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[cfg(unix)]
    #[test]
    fn spawn_transient_pipes_payload_and_keeps_channels_separate() {
        let dir = spawn_tmp("ok");
        let (argv_m, payload_m, env_m) = (dir.join("argv"), dir.join("payload"), dir.join("env"));
        let bin = fake_systemd_run(&dir, &argv_m, &payload_m, &env_m, "BOUNDED-RESULT", 0);

        let out = spawn_retry(
            &bin,
            &browser(),
            &["probe".to_string()],
            b"control-snapshot-v1\n",
            1 << 20,
            1 << 20,
        )
        .expect("spawn succeeds");

        // Result on stdout, diagnostics on stderr, never merged.
        assert!(
            out.stdout.contains("BOUNDED-RESULT"),
            "result: {:?}",
            out.stdout
        );
        assert!(out.stderr.contains("chatter"), "chatter: {:?}", out.stderr);
        assert!(!out.stdout.contains("chatter"), "channels must not merge");
        assert_eq!(out.status, Some(0));
        // The control payload reached the child's stdin.
        assert_eq!(
            std::fs::read_to_string(&payload_m).unwrap(),
            "control-snapshot-v1"
        );
        // The hardened profile and the `--` fence reached systemd-run's argv.
        let argv = std::fs::read_to_string(&argv_m).unwrap();
        assert!(
            argv.contains("--property=DynamicUser=yes"),
            "profile: {argv}"
        );
        assert!(
            argv.contains("-- /usr/libexec/vibed-tool probe"),
            "fence: {argv}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn spawn_transient_propagates_a_nonzero_exit() {
        let dir = spawn_tmp("err");
        let (argv_m, payload_m, env_m) = (dir.join("argv"), dir.join("payload"), dir.join("env"));
        let bin = fake_systemd_run(&dir, &argv_m, &payload_m, &env_m, "", 64);

        let out = spawn_retry(
            &bin,
            &browser(),
            &["probe".to_string()],
            b"x\n",
            1 << 20,
            1 << 20,
        )
        .expect("spawn runs");
        assert_eq!(
            out.status,
            Some(64),
            "the helper's exit code must propagate"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `env_clear()` really happens: a canary set in vibed's own environment must
    /// NOT reach the spawned systemd-run — otherwise a dropped `.env_clear()`
    /// would pass every other test unnoticed (Fable 5).
    #[cfg(unix)]
    #[test]
    fn spawn_transient_clears_the_environment() {
        std::env::set_var("VIBED_CANARY", "leaked");
        let dir = spawn_tmp("env");
        let (argv_m, payload_m, env_m) = (dir.join("argv"), dir.join("payload"), dir.join("env"));
        let bin = fake_systemd_run(&dir, &argv_m, &payload_m, &env_m, "ok", 0);

        let _ = spawn_retry(
            &bin,
            &browser(),
            &["probe".to_string()],
            b"x\n",
            1 << 20,
            1 << 20,
        )
        .expect("spawn runs");
        assert_eq!(
            std::fs::read_to_string(&env_m).unwrap(),
            "absent",
            "VIBED_CANARY must be cleared before exec (env_clear)"
        );
        std::env::remove_var("VIBED_CANARY");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A helper that floods stdout past the cap is killed and refused, so a
    /// hostile flood can never OOM root vibed (Fable 5 — the one blocking hole).
    #[cfg(unix)]
    #[test]
    fn spawn_transient_refuses_an_over_cap_flood() {
        let dir = spawn_tmp("flood");
        let (argv_m, payload_m, env_m) = (dir.join("argv"), dir.join("payload"), dir.join("env"));
        let bin = fake_systemd_run(
            &dir,
            &argv_m,
            &payload_m,
            &env_m,
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            0,
        );

        // 40 bytes of result under an 8-byte stdout cap => refused.
        let err = spawn_retry(&bin, &browser(), &["probe".to_string()], b"x\n", 8, 1 << 20)
            .expect_err("an over-cap flood must fail closed");
        assert!(
            err.contains("exceeded output cap"),
            "must refuse the flood: {err}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The result channel must be valid UTF-8: invalid bytes are a
    /// corruption/attack signal, refused rather than lossily patched (Fable 5).
    #[cfg(unix)]
    #[test]
    fn spawn_transient_rejects_non_utf8_result() {
        let dir = spawn_tmp("utf8");
        let (argv_m, payload_m, env_m) = (dir.join("argv"), dir.join("payload"), dir.join("env"));
        // `\0377\0376` = bytes 0xFF 0xFE, never valid UTF-8.
        let bin = fake_systemd_run(&dir, &argv_m, &payload_m, &env_m, "\\0377\\0376", 0);

        let err = spawn_retry(
            &bin,
            &browser(),
            &["probe".to_string()],
            b"x\n",
            1 << 20,
            1 << 20,
        )
        .expect_err("invalid UTF-8 on the result channel must fail closed");
        assert!(
            err.contains("not valid UTF-8"),
            "must refuse non-UTF-8: {err}"
        );

        let _ = std::fs::remove_dir_all(&dir);
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
