//! `fs.*` tools — bounded, confined filesystem read / list / write.
//!
//! Extracted verbatim from `mcp.rs` (F6, mechanical split — zero behaviour
//! change): `fs.read` (T0), `fs.list` (T0) and `fs.write` (T1), together with
//! the confinement machinery they share — canonical-path policy re-check,
//! per-uid home confinement (resolved from `/etc/passwd`) and the
//! symlink / hardlink / TOCTOU defenses. The built-in denylist
//! (`crate::mcp::builtin_denied`) and the tier lookup (`crate::mcp::tool_tier`)
//! stay in `mcp.rs` — shared with the call pipeline — and are called back here.

use serde_json::{json, Value};

use crate::audit::Caller;
use crate::glob::normalize_path;
use crate::mcp::{builtin_denied, tool_tier, O_NOFOLLOW};
use crate::policy::{CallContext, Decision, PolicyEngine};

/// Hard cap on file content returned by fs.read.
const MAX_READ_BYTES: usize = 256 * 1024;

/// `O_NONBLOCK` (Linux): open() does not block. Belt-and-suspenders for fs.read
/// against a FIFO — even if a regular file is swapped for a FIFO in the TOCTOU
/// window after the file-type check, the open returns instead of hanging the
/// worker thread forever (a FIFO with no writer would otherwise block).
const O_NONBLOCK: i32 = 0x800;

/// Hard cap on entries returned by one fs.list call.
const MAX_LIST_ENTRIES: usize = 500;

/// fs.write (T1 modify-user) is confined to user-scope prefixes.
/// On Fedora bootc systems /home is a symlink to /var/home, so both spellings
/// must be accepted. Keep in sync with `security/policy.d/default.toml`
/// (rule fs-write-user) and vibed.service (`ReadWritePaths=/var/home`).
const USER_WRITE_PREFIXES: [&str; 2] = ["/home/", "/var/home/"];

/// System locations fs.read / fs.list may reach OUTSIDE the caller's own home.
/// vibed runs as root, so without confinement a T0 read would expose every
/// user's personal files (docs, photos…) to any agent. Reads are therefore
/// restricted to the caller's own home (resolved from SO_PEERCRED) OR these
/// non-personal system trees. The built-in denylist still applies on top (it
/// carves the secrets — /etc/shadow, ~/.ssh, /proc/**/environ… — back out).
/// Deliberately EXCLUDED: /home and /var/home at large (other users), /root,
/// /tmp and /var (other users' state), /boot (denylisted). /var/lib/vibeos is
/// the machine's own state (memory readable here too; audit stays denylisted).
const SYSTEM_READ_PREFIXES: [&str; 6] = [
    "/etc/",
    "/usr/",
    "/proc/",
    "/sys/",
    "/run/",
    "/var/lib/vibeos/",
];

pub(crate) fn fs_read(
    args: &Value,
    policy: &PolicyEngine,
    caller: Caller,
) -> Result<String, String> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;

    let raw = args
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| "missing 'path' argument".to_string())?;
    let path = normalize_path(raw).ok_or_else(|| format!("fs.read: invalid path '{raw}'"))?;
    // Canonicalize (resolves every symlink AND requires existence): a symlink
    // must not smuggle a read into a denied location. handle_tools_call already
    // checked the lexical form before the policy decision.
    let canonical = std::fs::canonicalize(&path).map_err(|e| format!("fs.read {path}: {e}"))?;
    let canonical_str = canonical.to_string_lossy();
    // Bind the validated file's identity (device, inode) as close to
    // canonicalization as possible: the open() below re-verifies against it,
    // so a path component swapped for a symlink between the checks and the
    // open (TOCTOU, vibed runs as root) is detected and refused. Residual
    // window: canonicalize -> this lstat (a few µs); full closure comes with
    // openat2(RESOLVE_NO_SYMLINKS)/per-tool sandboxing in Phase 3.
    let validated =
        std::fs::symlink_metadata(&canonical).map_err(|e| format!("fs.read {path}: {e}"))?;
    // Built-in denylist AND the operator's policy (paths.denied / allowed) are
    // re-checked against BOTH the lexical and the canonical path, so neither a
    // symlink nor a policy blind spot can be used to escape into denied ground.
    for candidate in [path.as_str(), canonical_str.as_ref()] {
        if let Some(pattern) = builtin_denied(candidate, false) {
            return Err(format!(
                "fs.read: '{candidate}' is denied by the built-in denylist ({pattern})"
            ));
        }
    }
    recheck_policy_canonical(policy, "fs.read", &canonical_str)?;
    // Cross-user confinement: a system tree or the caller's OWN home only.
    let scope = confine_read("fs.read", caller, &canonical_str)?;

    // Reject anything that is not a regular file BEFORE opening: a FIFO opened
    // O_RDONLY with no writer BLOCKS the spawn_blocking worker forever (a DoS —
    // enough of them exhaust the blocking pool and stall every user's calls),
    // and a character/block device would exhaust memory. `validated` is the
    // lstat of the already-canonical path (no symlink left), so its type is
    // authoritative — check it here, not after the (potentially blocking) open.
    if !validated.file_type().is_file() {
        return Err(format!("fs.read: '{canonical_str}' is not a regular file"));
    }
    // O_NOFOLLOW: canonicalize already resolved every symlink, so the final
    // component must not be one anymore — a post-canonicalization swap fails
    // with ELOOP. O_NONBLOCK: belt-and-suspenders so a swap to a FIFO in the
    // TOCTOU window returns instead of blocking (regular-file reads ignore it).
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(O_NOFOLLOW | O_NONBLOCK)
        .open(&canonical)
        .map_err(|e| format!("fs.read {path}: {e}"))?;
    let opened = file
        .metadata()
        .map_err(|e| format!("fs.read {path}: {e}"))?;
    if !opened.is_file() {
        return Err(format!("fs.read: '{canonical_str}' is not a regular file"));
    }
    // Hardlink defense (cross-owner): the path-based denylist + home confinement
    // are blind to hardlinks (canonicalize resolves symlinks, not hardlinks), so
    // an agent could hardlink another user's / a root-owned inode into its own
    // home and have root-`vibed` read it. For a read resolved INTO the caller's
    // home, require the opened inode to be owned by the caller — a system-tree
    // read is legitimately root-owned and exempt. (A hardlink to the caller's
    // OWN file stays readable: the agent already owns it via its uid.)
    if scope == ReadScope::Home {
        use std::os::unix::fs::MetadataExt;
        if Some(opened.uid()) != caller.uid {
            return Err(format!(
                "fs.read: '{canonical_str}' is owned by uid {} but the caller is uid {:?}; \
                 refusing (possible cross-owner hardlink)",
                opened.uid(),
                caller.uid
            ));
        }
    }
    // The file actually opened must be the very inode that passed the
    // denylist/policy checks above (fstat on the open fd is authoritative).
    {
        use std::os::unix::fs::MetadataExt;
        if opened.dev() != validated.dev() || opened.ino() != validated.ino() {
            return Err(format!(
                "fs.read: '{canonical_str}' changed between validation and open \
                 (possible symlink race); refusing (fail-closed)"
            ));
        }
    }
    // Bounded read: never pull more than MAX_READ_BYTES + 1 into memory (one
    // extra byte only to detect truncation). This caps the allocation well
    // under 1 MiB regardless of the on-disk size, so a huge or unbounded file
    // cannot OOM the daemon.
    let mut bytes = Vec::new();
    file.by_ref()
        .take((MAX_READ_BYTES as u64) + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("fs.read {path}: {e}"))?;
    let truncated = bytes.len() > MAX_READ_BYTES;
    let end = bytes.len().min(MAX_READ_BYTES);
    let mut text = String::from_utf8_lossy(&bytes[..end]).into_owned();
    if truncated {
        text.push_str("\n[vibed: truncated, file exceeds 256 KiB]");
    }
    Ok(text)
}

/// fs.list (T0): bounded, non-recursive listing of one directory.
///
/// Same confinement discipline as fs.read: lexical normalization (done by the
/// caller pipeline too), canonicalization, built-in denylist + operator
/// policy re-checked on BOTH the lexical and canonical form. Symlinks inside
/// the directory are REPORTED (type "symlink") but never followed — neither
/// their target type nor size is disclosed.
pub(crate) fn fs_list(
    args: &Value,
    policy: &PolicyEngine,
    caller: Caller,
) -> Result<String, String> {
    let raw = args
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| "missing 'path' argument".to_string())?;
    let limit = match args.get("limit") {
        None | Some(Value::Null) => MAX_LIST_ENTRIES,
        Some(value) => match value.as_u64() {
            Some(n) if n >= 1 => (n as usize).min(MAX_LIST_ENTRIES),
            _ => return Err("fs.list: 'limit' must be an integer >= 1".to_string()),
        },
    };
    let path = normalize_path(raw).ok_or_else(|| format!("fs.list: invalid path '{raw}'"))?;
    let canonical = std::fs::canonicalize(&path).map_err(|e| format!("fs.list {path}: {e}"))?;
    let canonical_str = canonical.to_string_lossy();
    for candidate in [path.as_str(), canonical_str.as_ref()] {
        if let Some(pattern) = builtin_denied(candidate, false) {
            return Err(format!(
                "fs.list: '{candidate}' is denied by the built-in denylist ({pattern})"
            ));
        }
    }
    recheck_policy_canonical(policy, "fs.list", &canonical_str)?;
    // Cross-user confinement: a system tree or the caller's OWN home only.
    confine_read("fs.list", caller, &canonical_str)?;

    // Anti-TOCTOU (same discipline as fs.read; vibed runs as root): capture the
    // directory's identity right before the read and re-check it right after.
    // canonicalize resolved every symlink, so the target must be a real
    // directory now; if a component is swapped for a symlink in the window
    // around read_dir (e.g. `proj` -> /root), dev/ino changes and we refuse.
    // Full closure (parent-component swaps mid-read) awaits openat2
    // (RESOLVE_NO_SYMLINKS) in Phase 3.
    let before =
        std::fs::symlink_metadata(&canonical).map_err(|e| format!("fs.list {path}: {e}"))?;
    if !before.file_type().is_dir() {
        return Err(format!("fs.list: '{canonical_str}' is not a directory"));
    }
    let read_dir = std::fs::read_dir(&canonical).map_err(|e| format!("fs.list {path}: {e}"))?;

    // Keep the `limit` lexicographically-smallest names with O(limit) memory,
    // so a truncated result is a STABLE prefix regardless of readdir order (a
    // sort AFTER truncation would return an arbitrary readdir-order subset).
    struct ByName(String, Value);
    let mut kept: std::collections::BinaryHeap<ByName> = std::collections::BinaryHeap::new();
    impl PartialEq for ByName {
        fn eq(&self, other: &Self) -> bool {
            self.0 == other.0
        }
    }
    impl Eq for ByName {}
    impl PartialOrd for ByName {
        fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
            Some(self.cmp(other))
        }
    }
    impl Ord for ByName {
        fn cmp(&self, other: &Self) -> std::cmp::Ordering {
            self.0.cmp(&other.0)
        }
    }
    let mut total = 0usize;
    for entry in read_dir.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        // file_type() does not follow symlinks; size only for regular files.
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let kind = if file_type.is_dir() {
            "dir"
        } else if file_type.is_file() {
            "file"
        } else if file_type.is_symlink() {
            "symlink"
        } else {
            "other"
        };
        let size = if file_type.is_file() {
            entry.metadata().ok().map(|m| m.len())
        } else {
            None
        };
        total += 1;
        let item = ByName(
            name.clone(),
            json!({"name": name, "type": kind, "size": size}),
        );
        // Max-heap of size `limit`: its root is the largest kept name; a
        // smaller incoming name evicts it, so we retain the smallest `limit`.
        if kept.len() < limit {
            kept.push(item);
        } else if let Some(top) = kept.peek() {
            if item.0 < top.0 {
                kept.pop();
                kept.push(item);
            }
        }
    }

    let after =
        std::fs::symlink_metadata(&canonical).map_err(|e| format!("fs.list {path}: {e}"))?;
    {
        use std::os::unix::fs::MetadataExt;
        if before.dev() != after.dev() || before.ino() != after.ino() {
            return Err(format!(
                "fs.list: '{canonical_str}' changed during the read (possible symlink race); \
                 refusing (fail-closed)"
            ));
        }
    }

    let truncated = total > kept.len();
    let mut sorted = kept.into_vec();
    sorted.sort();
    let listed: Vec<Value> = sorted.into_iter().map(|b| b.1).collect();
    Ok(json!({
        "path": canonical_str,
        "entries": listed,
        "truncated": truncated
    })
    .to_string())
}

pub(crate) fn fs_write(
    args: &Value,
    policy: &PolicyEngine,
    caller: Caller,
) -> Result<String, String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let raw = args
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| "missing 'path' argument".to_string())?;
    let content = args
        .get("content")
        .and_then(Value::as_str)
        .ok_or_else(|| "missing 'content' argument".to_string())?;
    // Normalization resolves `..` lexically, so the prefix check below cannot
    // be escaped with traversal sequences.
    let path = normalize_path(raw).ok_or_else(|| format!("fs.write: invalid path '{raw}'"))?;
    if !USER_WRITE_PREFIXES
        .iter()
        .any(|prefix| path.starts_with(prefix))
    {
        return Err(format!(
            "fs.write is T1 (modify-user): path must start with one of {USER_WRITE_PREFIXES:?}"
        ));
    }

    // Symlink-safety, mirroring fs.read: resolve the PARENT directory through
    // symlinks with canonicalize (it MUST already exist — fs.write does not
    // create intermediate directories) and rebuild the canonical target as
    // "canonical parent + final component". Every confinement check below runs
    // against this CANONICAL path, not the lexical string, so a symlinked
    // parent cannot land the write outside the confinement.
    let path_ref = std::path::Path::new(&path);
    let file_name = path_ref
        .file_name()
        .ok_or_else(|| format!("fs.write: '{path}' has no final path component"))?;
    let parent = path_ref
        .parent()
        .ok_or_else(|| format!("fs.write: '{path}' has no parent directory"))?;
    let canonical_parent = std::fs::canonicalize(parent)
        .map_err(|e| format!("fs.write: parent directory of '{path}' must exist: {e}"))?;
    let canonical_target = canonical_parent.join(file_name);
    let canonical_str = canonical_target.to_string_lossy().into_owned();

    // (a) The canonical target must still sit under a user-write prefix; a
    //     symlinked parent that escaped to /etc, /var/lib, ... is rejected here.
    if !USER_WRITE_PREFIXES
        .iter()
        .any(|prefix| canonical_str.starts_with(prefix))
    {
        return Err(format!(
            "fs.write: canonical path '{canonical_str}' escapes the user-write scope \
             (symlinked parent?)"
        ));
    }
    // (b) Built-in write denylist on BOTH the lexical and canonical path, so a
    //     symlink cannot smuggle a write into the audit trail, the memory
    //     volume or the policy directory.
    for candidate in [path.as_str(), canonical_str.as_str()] {
        if let Some(pattern) = builtin_denied(candidate, true) {
            return Err(format!(
                "fs.write: '{candidate}' is denied by the built-in denylist ({pattern})"
            ));
        }
    }
    // (c) The operator's policy (paths.denied / allowed) re-checked on the
    //     canonical target: an operator's custom deny must hold after symlink
    //     resolution too.
    recheck_policy_canonical(policy, "fs.write", &canonical_str)?;
    // (d) Cross-user confinement: the canonical target MUST be inside the
    //     calling uid's OWN home directory (resolved from /etc/passwd). This is
    //     the check that stops one agent from writing into another user's — or
    //     root's — home, even though vibed itself runs as root.
    confine_to_caller_home(caller, &canonical_str)?;

    // Write the final component with O_NOFOLLOW so that, if the final name is
    // itself a symlink, open() fails with ELOOP instead of following it out of
    // the confinement (canonicalize above deliberately did NOT resolve the
    // final component, precisely so this guard can fire).
    //
    // RESIDUAL TOCTOU (Phase 3): O_NOFOLLOW guards only the FINAL component. The
    // open() below re-walks the whole path, so an agent that swaps an
    // INTERMEDIATE parent directory for a symlink in the tiny window between
    // canonicalize(parent) and this open (vibed runs as root) could still route
    // the create+truncate elsewhere. Unlike fs.read, an O_CREAT|O_TRUNC write
    // cannot be undone by a post-open dev/ino recheck. Full closure needs a
    // single atomic resolve+open — `openat2(RESOLVE_NO_SYMLINKS)` — which lands
    // with per-tool sandboxing in Phase 3. Winning the race is hard (a tight,
    // repeated swap) and the common cases (symlinked final component, symlinked
    // parent already in place at check time) are already refused above.
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .custom_flags(O_NOFOLLOW)
        .open(&canonical_target)
        .map_err(|e| format!("fs.write {canonical_str}: {e}"))?;
    file.write_all(content.as_bytes())
        .map_err(|e| format!("fs.write {canonical_str}: {e}"))?;
    Ok(format!("wrote {} bytes to {canonical_str}", content.len()))
}

/// Re-run the policy decision against a POST-canonicalization path. A symlink
/// (or a policy blind spot) must not be usable to reach a location the loaded
/// policy forbids via `paths.denied`/`paths.allowed`. Anything other than a
/// clean `Allow` is refused. The tool already passed the lexical policy check
/// in `handle_tools_call`, so this can only tighten, never loosen.
fn recheck_policy_canonical(
    policy: &PolicyEngine,
    tool: &str,
    canonical: &str,
) -> Result<(), String> {
    let tier = tool_tier(tool);
    let ctx = CallContext {
        path: Some(canonical),
        service: None,
        domain: None,
        deploy: None,
    };
    match policy.evaluate(tool, tier, ctx) {
        Decision::Allow => Ok(()),
        _ => Err(format!(
            "{tool}: canonical path '{canonical}' is denied by policy after symlink resolution"
        )),
    }
}

/// A caller "home" (resolved from `/etc/passwd`, then canonicalized) that is the
/// root, a top-level system directory, or the PARENT of the whole home namespace
/// would make `is_within()` trivially true for OTHER users' files or entire
/// system trees — a misconfigured passwd (home = `/var`, `/var/home`, `/home`, …)
/// must never silently disable home confinement. A legitimate home is a directory
/// UNDER the home namespace (the resolved `/var/home/<user>`) or root's own
/// `/var/roothome`; none of those are in this set.
fn home_is_too_broad(home: &str) -> bool {
    const BROAD: &[&str] = &[
        "/",
        "/var",
        "/var/home",
        "/home",
        "/usr",
        "/etc",
        "/run",
        "/tmp",
        "/proc",
        "/sys",
        "/boot",
        "/dev",
        "/opt",
        "/srv",
        "/mnt",
    ];
    let trimmed = home.trim_end_matches('/');
    let trimmed = if trimmed.is_empty() { "/" } else { trimmed };
    BROAD.contains(&trimmed)
}

/// Require that `canonical_target` lives inside the calling uid's own home
/// directory. Fail-closed: an unknown caller uid, a uid with no `/etc/passwd`
/// entry, or a home that cannot be resolved all refuse the write.
fn confine_to_caller_home(caller: Caller, canonical_target: &str) -> Result<(), String> {
    let uid = caller.uid.ok_or_else(|| {
        "fs.write: caller uid unavailable (SO_PEERCRED); refusing (fail-closed)".to_string()
    })?;
    let home = home_dir_for_uid(uid).ok_or_else(|| {
        format!("fs.write: no home directory for uid {uid} in /etc/passwd; refusing")
    })?;
    // Canonicalize the home so the Fedora `/home -> /var/home` symlink (and any
    // other) is resolved to the same space as the (already canonical) target.
    let canonical_home = std::fs::canonicalize(&home)
        .map_err(|e| format!("fs.write: cannot resolve home '{home}' for uid {uid}: {e}"))?;
    let canonical_home_str = canonical_home.to_string_lossy();
    // A home of '/' or any broad ancestor would make is_within() trivially true
    // and disable write confinement — refuse fail-closed.
    if home_is_too_broad(&canonical_home_str) {
        return Err(format!(
            "fs.write: uid {uid}'s home '{canonical_home_str}' is too broad — refusing \
             (would disable confinement)"
        ));
    }
    if !is_within(canonical_target, &canonical_home_str) {
        return Err(format!(
            "fs.write: cross-user write refused: '{canonical_target}' is not within uid {uid}'s \
             home '{canonical_home_str}'"
        ));
    }
    Ok(())
}

/// Which allowed area a read resolved into. The caller applies extra checks per
/// scope (an in-`Home` read must be owned by the caller — hardlink defense —
/// whereas a `System` tree read is legitimately root-owned).
#[derive(PartialEq, Eq, Clone, Copy, Debug)]
enum ReadScope {
    System,
    Home,
}

/// Read confinement for fs.read / fs.list. `canonical` is allowed iff it is a
/// system-read tree (SYSTEM_READ_PREFIXES) OR inside the calling uid's own home
/// (resolved from SO_PEERCRED). Fail-closed: an unknown/unresolvable caller may
/// still read the system trees, but NEVER a home — so a missing peer cred
/// cannot be leveraged to read personal data. The built-in denylist is applied
/// separately (before this) and always wins.
fn confine_read(tool: &str, caller: Caller, canonical: &str) -> Result<ReadScope, String> {
    // A per-process /proc/<pid> tree is confined to the caller's OWN process(es).
    // vibed reads as root, so without this any caller could enumerate every
    // user's process metadata (maps/status/net/... — cross-user reconnaissance).
    // Kernel-memory and environ/cmdline files are denied for ALL owners by the
    // built-in denylist (applied before this); this closes the residual recon
    // over the remaining /proc/<pid> files (adversarial review 2026-07-14).
    if let Some(pid) = proc_pid_of(canonical) {
        return confine_proc_pid(tool, caller, pid, canonical);
    }
    if SYSTEM_READ_PREFIXES
        .iter()
        .any(|p| canonical == p.trim_end_matches('/') || canonical.starts_with(p))
    {
        return Ok(ReadScope::System);
    }
    // Otherwise it must be within the caller's OWN home.
    let uid = caller.uid.ok_or_else(|| {
        format!(
            "{tool}: '{canonical}' is outside the readable system trees and the caller uid is \
             unavailable (SO_PEERCRED); refusing (fail-closed)"
        )
    })?;
    let home = home_dir_for_uid(uid)
        .ok_or_else(|| format!("{tool}: no home for uid {uid} in /etc/passwd; refusing"))?;
    let canonical_home = std::fs::canonicalize(&home)
        .map_err(|e| format!("{tool}: cannot resolve home '{home}' for uid {uid}: {e}"))?;
    let canonical_home_str = canonical_home.to_string_lossy();
    // A home that resolves to "/" (some system accounts) or any broad ancestor
    // (/var, /var/home, /home, … — the parent of the whole home namespace) would
    // make is_within() trivially true — home confinement would silently become a
    // no-op, opening every user's files. Refuse fail-closed instead.
    if home_is_too_broad(&canonical_home_str) {
        return Err(format!(
            "{tool}: uid {uid}'s home '{canonical_home_str}' is too broad — refusing \
             (would disable confinement)"
        ));
    }
    if is_within(canonical, &canonical_home_str) {
        return Ok(ReadScope::Home);
    }
    Err(format!(
        "{tool}: '{canonical}' is neither a readable system path nor inside uid {uid}'s own home; \
         cross-user reads are refused"
    ))
}

/// The pid of a per-process `/proc/<pid>[/...]` path, or None for global /proc
/// files (`/proc/cpuinfo`, `/proc/sys/...`) and non-/proc paths. `canonical` is
/// already symlink-resolved, so `/proc/self` never reaches here as literal text.
fn proc_pid_of(canonical: &str) -> Option<u32> {
    canonical
        .strip_prefix("/proc/")?
        .split('/')
        .next()?
        .parse::<u32>()
        .ok()
}

/// Real uid owning process `pid`, from `/proc/<pid>/status` `Uid:` (real uid =
/// the first field). None if the pid is gone or its status is unreadable.
fn proc_pid_owner(pid: u32) -> Option<u32> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("Uid:") {
            return rest.split_whitespace().next()?.parse().ok();
        }
    }
    None
}

/// Confine a `/proc/<pid>` read to the caller's own process(es). Fail-closed: an
/// unknown caller uid, or a vanished/unreadable pid, is refused. A process the
/// caller owns yields `System` scope (it is a /proc file, not the caller's home).
fn confine_proc_pid(
    tool: &str,
    caller: Caller,
    pid: u32,
    canonical: &str,
) -> Result<ReadScope, String> {
    let uid = caller.uid.ok_or_else(|| {
        format!(
            "{tool}: '{canonical}' is a per-process /proc tree and the caller uid is \
             unavailable (SO_PEERCRED); refusing (fail-closed)"
        )
    })?;
    let owner = proc_pid_owner(pid).ok_or_else(|| {
        format!("{tool}: cannot determine the owner of /proc/{pid} (gone or unreadable); refusing")
    })?;
    if owner == uid {
        Ok(ReadScope::System)
    } else {
        Err(format!(
            "{tool}: /proc/{pid} belongs to uid {owner}, not the caller (uid {uid}); \
             cross-user process reads are refused"
        ))
    }
}

/// True when `path` is `base` itself or a descendant of it, comparing whole
/// path segments so `/home/dev2` is NOT treated as inside `/home/dev`.
fn is_within(path: &str, base: &str) -> bool {
    if path == base {
        return true;
    }
    let prefix = if base.ends_with('/') {
        base.to_string()
    } else {
        format!("{base}/")
    };
    path.starts_with(&prefix)
}

/// Resolve the home directory (field 6) of `uid` (field 3) by parsing
/// `/etc/passwd` by hand — no libc / nss dependency, matching the style of
/// `find_group_gid` in main.rs.
fn home_dir_for_uid(uid: u32) -> Option<String> {
    let content = std::fs::read_to_string("/etc/passwd").ok()?;
    home_dir_for_uid_in(&content, uid)
}

/// Pure passwd parser, split out so the uid->home mapping is unit-testable
/// without touching the real `/etc/passwd`.
/// Line format: `name:passwd:uid:gid:gecos:home:shell`.
fn home_dir_for_uid_in(passwd: &str, uid: u32) -> Option<String> {
    for line in passwd.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut fields = line.split(':');
        let _name = fields.next()?;
        let _passwd = fields.next()?;
        let uid_field = fields.next()?;
        let _gid = fields.next()?;
        let _gecos = fields.next()?;
        let home = fields.next()?;
        if uid_field.parse::<u32>().ok() == Some(uid) && !home.is_empty() {
            return Some(home.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{
        caller_uid, current_uid, empty_policy, permissive_policy, policy_from_toml,
    };

    /// A fresh, empty scratch directory inside `uid`'s real home (removed by the
    /// caller). Used by the filesystem-touching tests.
    fn home_scratch(uid: u32, tag: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static SEQ: AtomicU32 = AtomicU32::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let home = home_dir_for_uid(uid).expect("caller uid has a home in /etc/passwd");
        let dir = std::path::Path::new(&home)
            .join(format!(".vibed-test-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create home scratch dir");
        dir
    }

    #[test]
    fn fs_write_rejects_paths_outside_user_prefixes() {
        let policy = empty_policy();
        let caller = Caller {
            uid: Some(1000),
            gid: None,
            pid: None,
        };
        let err = fs_write(
            &json!({"path": "/etc/passwd", "content": "x"}),
            &policy,
            caller,
        )
        .unwrap_err();
        assert!(err.contains("modify-user"), "unexpected error: {err}");
        // /tmp was removed from the v0.1 write scope (D7).
        let err =
            fs_write(&json!({"path": "/tmp/x", "content": "x"}), &policy, caller).unwrap_err();
        assert!(err.contains("modify-user"), "unexpected error: {err}");
    }

    #[test]
    fn fs_write_rejects_traversal_and_memory_volume() {
        let policy = empty_policy();
        let caller = Caller {
            uid: Some(1000),
            gid: None,
            pid: None,
        };
        let err = fs_write(
            &json!({"path": "/home/dev/../../etc/cron.d/evil", "content": "x"}),
            &policy,
            caller,
        )
        .unwrap_err();
        assert!(err.contains("modify-user"), "unexpected error: {err}");
        // The memory volume is under /var/lib, so the prefix check rejects it;
        // the built-in denylist covers it too (checked directly here). This is
        // the same canonical-write denylist that stops a symlink from smuggling
        // a write into the memory/audit/policy areas (see the symlink tests).
        assert!(builtin_denied("/var/lib/vibeos/memory/user/profile.toml", true).is_some());
        assert!(builtin_denied("/var/lib/vibeos/audit/vibed.jsonl", true).is_some());
        assert!(builtin_denied("/etc/vibeos/policy.d/default.toml", true).is_some());
    }

    // -- Fix 1/2: fs.write symlink-safety and cross-user confinement ----------

    #[test]
    fn fs_write_allows_write_within_callers_own_home() {
        let uid = current_uid();
        if uid == 0 {
            return; // root's home is /root, outside the user-write scope
        }
        let base = home_scratch(uid, "ok");
        let target = base.join("notes.md");
        let res = fs_write(
            &json!({"path": target.to_string_lossy(), "content": "hello vibed"}),
            &permissive_policy(),
            caller_uid(uid),
        );
        assert!(res.is_ok(), "self-home write must succeed: {res:?}");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "hello vibed");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn fs_write_rejects_cross_user_write() {
        let uid = current_uid();
        if uid == 0 {
            return;
        }
        let base = home_scratch(uid, "xuser");
        let target = base.join("evil.txt");
        // Caller claims to be root (uid 0, home /root): writing into THIS user's
        // home is cross-user and must be refused, even though the path is a
        // perfectly valid /home path that passes the lexical checks.
        let res = fs_write(
            &json!({"path": target.to_string_lossy(), "content": "x"}),
            &permissive_policy(),
            caller_uid(0),
        );
        let err = res.unwrap_err();
        assert!(
            err.contains("cross-user"),
            "expected cross-user refusal, got: {err}"
        );
        assert!(
            !target.exists(),
            "no file may be created on a cross-user refusal"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn fs_write_rejects_missing_caller_uid() {
        // SO_PEERCRED failed => uid unknown => fail-closed refusal.
        let uid = current_uid();
        if uid == 0 {
            return;
        }
        let base = home_scratch(uid, "nouid");
        let target = base.join("x.txt");
        let res = fs_write(
            &json!({"path": target.to_string_lossy(), "content": "x"}),
            &permissive_policy(),
            Caller::default(),
        );
        assert!(
            res.unwrap_err().contains("uid unavailable"),
            "unknown uid must be refused"
        );
        assert!(!target.exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn fs_write_rejects_symlinked_final_component() {
        let uid = current_uid();
        if uid == 0 {
            return;
        }
        let base = home_scratch(uid, "nofollow");
        let victim =
            std::env::temp_dir().join(format!("vibed-nofollow-victim-{}", std::process::id()));
        let _ = std::fs::remove_file(&victim);
        let link = base.join("link");
        std::os::unix::fs::symlink(&victim, &link).expect("create final-component symlink");
        let res = fs_write(
            &json!({"path": link.to_string_lossy(), "content": "pwned"}),
            &permissive_policy(),
            caller_uid(uid),
        );
        assert!(
            res.is_err(),
            "writing through a symlinked final name must fail (O_NOFOLLOW)"
        );
        assert!(
            !victim.exists(),
            "the symlink target must never be created/written"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn fs_write_rejects_symlinked_parent_escaping_home() {
        let uid = current_uid();
        if uid == 0 {
            return;
        }
        let base = home_scratch(uid, "escape");
        // A directory symlink that escapes the user-write scope to /etc.
        let escape = base.join("escape");
        std::os::unix::fs::symlink("/etc", &escape).expect("create parent symlink to /etc");
        let target = format!("{}/escape/vibed-implant", base.display());
        let res = fs_write(
            &json!({"path": target, "content": "x"}),
            &permissive_policy(),
            caller_uid(uid),
        );
        let err = res.unwrap_err();
        assert!(
            err.contains("escapes the user-write scope"),
            "a symlinked parent escaping /home must be refused: {err}"
        );
        assert!(!std::path::Path::new("/etc/vibed-implant").exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn fs_write_rejects_symlink_into_a_denied_area() {
        // Proxy for "write into audit/memory/policy via symlink": a symlinked
        // parent resolving into an area the policy denies is caught by the
        // canonical policy re-check (the very same mechanism that guards the
        // built-in audit/memory/policy write denylist on canonical paths).
        let uid = current_uid();
        if uid == 0 {
            return;
        }
        let base = home_scratch(uid, "denied");
        let secret = base.join("secret_real");
        std::fs::create_dir_all(&secret).expect("mkdir secret_real");
        let via = base.join("via");
        std::os::unix::fs::symlink(&secret, &via).expect("create symlink into denied area");
        let policy = policy_from_toml(&format!(
            r#"
            [[rule]]
            id = "fs-write"
            tools = ["fs.write"]
            tier = "T1"
            action = "allow"
            [rule.paths]
            allowed = ["/home/**", "/var/home/**"]
            denied = ["{}/**"]
            "#,
            secret.display()
        ));
        let target = format!("{}/via/implant", base.display());
        let res = fs_write(
            &json!({"path": target, "content": "x"}),
            &policy,
            caller_uid(uid),
        );
        let err = res.unwrap_err();
        assert!(
            err.contains("denied by policy"),
            "a symlink into a policy-denied area must be refused: {err}"
        );
        assert!(!secret.join("implant").exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    // -- Fix 4: fs.read special files and bounded reads ----------------------

    #[test]
    fn fs_read_rejects_non_regular_files() {
        let uid = current_uid();
        if uid == 0 {
            return; // root's home is /root, denylisted
        }
        let policy = permissive_policy();
        let caller = caller_uid(uid);
        // A directory is not a regular file — created in the caller's OWN home
        // so confinement lets the attempt reach the regular-file check.
        let base = home_scratch(uid, "read-nonreg");
        let subdir = base.join("adir");
        std::fs::create_dir_all(&subdir).unwrap();
        let err = fs_read(&json!({"path": subdir.to_string_lossy()}), &policy, caller).unwrap_err();
        assert!(
            err.contains("not a regular file"),
            "directory must be refused: {err}"
        );
        // A FIFO in the caller's own home: open(O_RDONLY) with no writer would
        // BLOCK the worker forever without the pre-open type check (the DoS this
        // guards). It must return a refusal instead of hanging the test.
        let fifo = base.join("pipe");
        let made = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if made {
            let err =
                fs_read(&json!({"path": fifo.to_string_lossy()}), &policy, caller).unwrap_err();
            assert!(
                err.contains("not a regular file"),
                "a FIFO must be refused BEFORE the blocking open: {err}"
            );
        }
        // A char device outside the readable trees is refused by confinement.
        if std::path::Path::new("/dev/zero").exists() {
            let err = fs_read(&json!({"path": "/dev/zero"}), &policy, caller).unwrap_err();
            assert!(
                !err.is_empty(),
                "char device outside the trees must be refused"
            );
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn fs_read_reads_regular_file_and_bounds_large_one() {
        let uid = current_uid();
        if uid == 0 {
            return;
        }
        let policy = permissive_policy();
        let caller = caller_uid(uid);
        let dir = home_scratch(uid, "read-ok");

        let small = dir.join("small.txt");
        std::fs::write(&small, "hello vibed").unwrap();
        let out = fs_read(&json!({"path": small.to_string_lossy()}), &policy, caller)
            .expect("read small");
        assert_eq!(out, "hello vibed");

        let big = dir.join("big.bin");
        std::fs::write(&big, vec![b'a'; MAX_READ_BYTES + 4096]).unwrap();
        let out =
            fs_read(&json!({"path": big.to_string_lossy()}), &policy, caller).expect("read big");
        assert!(
            out.contains("truncated"),
            "oversized read must be truncated"
        );
        assert!(
            out.len() <= MAX_READ_BYTES + 64,
            "returned content must stay within the cap (+ notice), got {}",
            out.len()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fs_read_reads_a_system_tree_regardless_of_caller() {
        // /etc is a readable system tree — even an unknown caller (no peer cred)
        // may read it, but NEVER personal data (see confine_read test).
        let policy = permissive_policy();
        if std::path::Path::new("/etc/os-release").is_file() {
            let out = fs_read(
                &json!({"path": "/etc/os-release"}),
                &policy,
                Caller::default(),
            )
            .expect("system read of /etc/os-release");
            assert!(!out.is_empty());
        }
    }

    #[test]
    fn fs_read_still_reads_through_symlinks_via_canonicalization() {
        // O_NOFOLLOW + the dev/ino recheck guard against TOCTOU swaps, but a
        // legitimate read THROUGH a symlink keeps working: canonicalize
        // resolves the link before open, so the final component of the opened
        // path is the real file.
        let uid = current_uid();
        if uid == 0 {
            return;
        }
        let policy = permissive_policy();
        let caller = caller_uid(uid);
        let dir = home_scratch(uid, "read-link");
        let real = dir.join("real.txt");
        std::fs::write(&real, "via symlink").unwrap();
        let link = dir.join("link.txt");
        std::os::unix::fs::symlink(&real, &link).expect("symlink");
        let out = fs_read(&json!({"path": link.to_string_lossy()}), &policy, caller)
            .expect("reading through a symlink must still work");
        assert_eq!(out, "via symlink");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn proc_pid_of_parses_only_per_process_paths() {
        assert_eq!(proc_pid_of("/proc/1234/maps"), Some(1234));
        assert_eq!(proc_pid_of("/proc/1234"), Some(1234));
        assert_eq!(proc_pid_of("/proc/1234/task/56/status"), Some(1234));
        assert_eq!(proc_pid_of("/proc/cpuinfo"), None);
        assert_eq!(proc_pid_of("/proc/sys/kernel/hostname"), None);
        assert_eq!(proc_pid_of("/etc/passwd"), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn proc_pid_read_is_confined_to_the_owning_caller() {
        let me = std::process::id();
        let my_uid = proc_pid_owner(me).expect("own pid status is readable");
        // Own process, own uid -> allowed (System scope, not Home).
        assert_eq!(
            confine_read("fs.read", caller_uid(my_uid), &format!("/proc/{me}/maps")),
            Ok(ReadScope::System)
        );
        // Own process, a DIFFERENT uid -> refused (cross-user recon blocked).
        assert!(confine_read(
            "fs.read",
            caller_uid(my_uid.wrapping_add(1)),
            &format!("/proc/{me}/status")
        )
        .is_err());
        // Unknown caller uid -> refused (fail-closed).
        assert!(confine_read("fs.read", Caller::default(), &format!("/proc/{me}/maps")).is_err());
        // A vanished pid -> refused (fail-closed), never opened.
        assert!(confine_read("fs.read", caller_uid(my_uid), "/proc/4000000/maps").is_err());
        // A GLOBAL /proc file is not per-process and stays readable.
        assert!(confine_read("fs.read", Caller::default(), "/proc/cpuinfo").is_ok());
    }

    #[test]
    fn confine_read_allows_system_and_own_home_refuses_others() {
        let uid = current_uid();
        let caller = caller_uid(uid);
        // System trees are always readable.
        for p in [
            "/etc/os-release",
            "/usr/bin",
            "/proc/uptime",
            "/var/lib/vibeos/x",
            "/etc",
        ] {
            assert!(
                confine_read("fs.read", caller, p).is_ok(),
                "{p} must be readable"
            );
        }
        // A non-system path outside the caller's home is refused (cross-user).
        assert!(confine_read("fs.read", caller, "/tmp/elsewhere").is_err());
        assert!(confine_read("fs.read", caller, "/var/home/someoneelse/secret").is_err());
        // Unknown caller: system OK, home refused (fail-closed — a missing peer
        // cred can never be used to read personal data).
        assert!(confine_read("fs.read", Caller::default(), "/etc/x").is_ok());
        assert!(confine_read("fs.read", Caller::default(), "/tmp/x").is_err());
        // The caller's OWN home is readable.
        if uid != 0 {
            let home = home_dir_for_uid(uid).expect("caller has a home");
            let canon = std::fs::canonicalize(&home).expect("home resolves");
            let inhome = format!("{}/somefile", canon.to_string_lossy());
            assert!(
                confine_read("fs.read", caller, &inhome).is_ok(),
                "own home must be readable"
            );
        }
    }

    // -- passwd / confinement helpers ----------------------------------------

    #[test]
    fn home_dir_for_uid_parses_passwd() {
        let passwd = "root:x:0:0:root:/root:/bin/bash\n\
                      # a comment line\n\
                      \n\
                      micki:x:1000:1000:,,,:/home/micki:/bin/bash\n\
                      svc:x:1001:1001::/var/home/svc:/usr/sbin/nologin\n";
        assert_eq!(home_dir_for_uid_in(passwd, 0).as_deref(), Some("/root"));
        assert_eq!(
            home_dir_for_uid_in(passwd, 1000).as_deref(),
            Some("/home/micki")
        );
        assert_eq!(
            home_dir_for_uid_in(passwd, 1001).as_deref(),
            Some("/var/home/svc")
        );
        assert_eq!(home_dir_for_uid_in(passwd, 4242), None);
    }

    #[test]
    fn is_within_respects_segment_boundaries() {
        assert!(is_within("/home/dev/x", "/home/dev"));
        assert!(is_within("/home/dev", "/home/dev"));
        assert!(!is_within("/home/dev2/x", "/home/dev"));
        assert!(!is_within("/home", "/home/dev"));
    }

    #[test]
    fn home_is_too_broad_rejects_ancestors_but_accepts_real_homes() {
        // A misconfigured passwd home that is root, a top-level dir, or the
        // parent of the whole home namespace would disable confinement.
        for broad in [
            "/",
            "/var",
            "/var/home",
            "/var/home/",
            "/home",
            "/usr",
            "/etc",
            "/tmp",
            "/proc",
            "/opt",
        ] {
            assert!(
                home_is_too_broad(broad),
                "{broad} must be refused as a home"
            );
        }
        // Real per-user homes (and root's own, resolved) must be accepted.
        for ok in [
            "/var/home/dev",
            "/var/home/svc",
            "/home/dev",
            "/var/roothome",
        ] {
            assert!(!home_is_too_broad(ok), "{ok} must be accepted as a home");
        }
    }

    // -- fs.list (T0) ----------------------------------------------------------

    /// A policy allowing fs.list everywhere except an explicit denied subtree,
    /// mirroring the shipped fs-read rule shape.
    fn list_policy() -> PolicyEngine {
        policy_from_toml(
            r#"
            [[rule]]
            id = "fs-read"
            tools = ["fs.read", "fs.list"]
            tier = "T0"
            action = "allow"
            "#,
        )
    }

    #[test]
    fn fs_list_lists_names_types_sizes_sorted() {
        let uid = current_uid();
        if uid == 0 {
            return;
        }
        let policy = list_policy();
        let caller = caller_uid(uid);
        let dir = home_scratch(uid, "list-ok");
        std::fs::create_dir_all(dir.join("sub")).expect("mkdir");
        std::fs::write(dir.join("b.txt"), "12345").unwrap();
        std::fs::write(dir.join("a.txt"), "1").unwrap();
        std::os::unix::fs::symlink("/etc/os-release", dir.join("link")).expect("symlink");
        let payload: Value = serde_json::from_str(
            &fs_list(&json!({"path": dir.to_string_lossy()}), &policy, caller).unwrap(),
        )
        .unwrap();
        let entries = payload["entries"].as_array().unwrap();
        let names: Vec<&str> = entries
            .iter()
            .map(|e| e["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["a.txt", "b.txt", "link", "sub"], "sorted by name");
        assert_eq!(entries[0]["type"], "file");
        assert_eq!(entries[0]["size"], 1);
        assert_eq!(entries[1]["size"], 5);
        assert_eq!(entries[2]["type"], "symlink");
        assert!(
            entries[2]["size"].is_null(),
            "a symlink discloses no target size"
        );
        assert_eq!(entries[3]["type"], "dir");
        assert_eq!(payload["truncated"], false);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fs_list_respects_limit_and_returns_stable_lexicographic_prefix() {
        let uid = current_uid();
        if uid == 0 {
            return;
        }
        let policy = list_policy();
        let caller = caller_uid(uid);
        let dir = home_scratch(uid, "list-lim");
        // Create in a shuffled order so a readdir-order bug would surface.
        for name in ["e", "b", "d", "a", "c"] {
            std::fs::write(dir.join(name), "x").unwrap();
        }
        let payload: Value = serde_json::from_str(
            &fs_list(
                &json!({"path": dir.to_string_lossy(), "limit": 3}),
                &policy,
                caller,
            )
            .unwrap(),
        )
        .unwrap();
        let entries = payload["entries"].as_array().unwrap();
        let names: Vec<&str> = entries
            .iter()
            .map(|e| e["name"].as_str().unwrap())
            .collect();
        // The truncated set must be the SMALLEST `limit` names, sorted — a
        // stable prefix, not an arbitrary readdir-order subset.
        assert_eq!(
            names,
            ["a", "b", "c"],
            "truncation must keep the lexicographic prefix"
        );
        assert_eq!(payload["truncated"], true);
        let err = fs_list(
            &json!({"path": dir.to_string_lossy(), "limit": 0}),
            &policy,
            caller,
        )
        .unwrap_err();
        assert!(err.contains("integer >= 1"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fs_list_refuses_when_target_is_not_a_directory() {
        // canonicalize follows a symlink to its real target; that target must
        // itself be a real directory. A symlink to a regular file is refused.
        let uid = current_uid();
        if uid == 0 {
            return;
        }
        let policy = list_policy();
        let caller = caller_uid(uid);
        let base = home_scratch(uid, "list-sym");
        let file = base.join("regular.txt");
        std::fs::write(&file, "x").unwrap();
        let link = base.join("link-to-file");
        std::os::unix::fs::symlink(&file, &link).expect("symlink");
        let err = fs_list(&json!({"path": link.to_string_lossy()}), &policy, caller).unwrap_err();
        assert!(err.contains("not a directory"), "unexpected error: {err}");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn fs_list_refuses_denylisted_directories() {
        let policy = list_policy();
        // The denylist applies to the DIRECTORY itself: /root and a user's
        // .ssh directory are unlistable, whoever asks.
        for path in ["/root", "/home/dev/.ssh", "/home/dev/.claude"] {
            let err = builtin_denied(path, false);
            assert!(err.is_some(), "{path} must be builtin-denied");
        }
        // End-to-end through fs_list for a path that exists on every test box.
        if std::path::Path::new("/root").exists() {
            let err = fs_list(&json!({"path": "/root"}), &policy, Caller::default()).unwrap_err();
            assert!(err.contains("denylist"), "unexpected error: {err}");
        }
    }
}
