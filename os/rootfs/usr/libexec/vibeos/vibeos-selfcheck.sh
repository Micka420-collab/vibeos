#!/usr/bin/env bash
# vibeos-selfcheck.sh — Tier-A post-boot self-validation for a VibeOS install.
#
# Runs on a BOOTED VibeOS machine (bare-metal or VM) and asserts, in one shot,
# the invariants only a real boot can prove: vibed.service up, the MCP socket
# present with the right ownership/mode, the policy loaded fail-closed, the
# built-in denylist live, the audit chain intact, Genesis done, the immutable
# root read-only, the shipped AI kernel tuning in effect, and the current
# operating mode (governed vs open, ADR-027).
# Every probe is READ-ONLY (no restart, no write, no approval):
# the single T2 check is a `policy.check` dry-run, never a real mutation.
#
# It is version-TOLERANT: capabilities absent from an older image (vibectl,
# policy.check, ...) are reported SKIP, not FAIL, so the same script grades a
# minimal image and a full one identically where they overlap.
#
# Usage:
#   sudo /usr/libexec/vibeos/vibeos-selfcheck.sh          # human-readable table
#   sudo /usr/libexec/vibeos/vibeos-selfcheck.sh --json   # machine-readable JSON
# Exit code: 0 if no check FAILED (SKIPs are fine), 1 otherwise.
#
# Overrides (CI / simulation against a scratch vibed — see docs/VALIDATION.md):
#   VIBEOS_SELFCHECK_SOCKET / _POLICY_DIR / _MEMORY_DIR / _AUDIT_DIR / _TIMEOUT
#   VIBEOS_SELFCHECK_ASSUME_IMAGE=auto|0|1  force the "is this a VibeOS image?"
#                                           verdict (auto-detects by default);
#                                           0 makes absent system components SKIP
#                                           instead of FAIL (handy off-target).
#
# DELIBERATELY NOT `set -e`: a validator must run EVERY check and report all
# results, so a failing probe is recorded, never fatal. `set -u`/`pipefail` stay.

set -uo pipefail

SOCKET="${VIBEOS_SELFCHECK_SOCKET:-/run/vibed/mcp.sock}"
POLICY_DIR="${VIBEOS_SELFCHECK_POLICY_DIR:-/etc/vibeos/policy.d}"
MEMORY_DIR="${VIBEOS_SELFCHECK_MEMORY_DIR:-/var/lib/vibeos/memory}"
AUDIT_DIR="${VIBEOS_SELFCHECK_AUDIT_DIR:-/var/lib/vibeos/audit}"
TIMEOUT="${VIBEOS_SELFCHECK_TIMEOUT:-4}"
US=$'\x1f' # unit-separator: field delimiter that never appears in a detail

JSON=0
case "${1:-}" in --json) JSON=1 ;; esac
[ "${VIBEOS_SELFCHECK_JSON:-0}" = 1 ] && JSON=1

case "${VIBEOS_SELFCHECK_ASSUME_IMAGE:-auto}" in
    1) ON_IMAGE=1 ;;
    0) ON_IMAGE=0 ;;
    *) if [ -e /usr/bin/vibed ] || [ -d /etc/vibeos ]; then ON_IMAGE=1; else ON_IMAGE=0; fi ;;
esac

pass=0
fail=0
skip=0
ROWS=""

row() { # $1=STATUS $2=check $3=detail
    case "$1" in
        PASS) pass=$((pass + 1)) ;;
        FAIL) fail=$((fail + 1)) ;;
        SKIP) skip=$((skip + 1)) ;;
    esac
    ROWS="${ROWS}${1}${US}${2}${US}${3}"$'\n'
}
# A required component that is absent: FAIL on a real image, SKIP off-target.
missing() { if [ "$ON_IMAGE" = 1 ]; then row FAIL "$1" "$2"; else row SKIP "$1" "$2 (host is not a VibeOS image)"; fi; }
have() { command -v "$1" >/dev/null 2>&1; }

# ---- MCP transport (line-delimited JSON-RPC over the unix socket) -----------
INIT='{"jsonrpc":"2.0","id":0,"method":"initialize","params":{}}'
# Send one request (carrying a unique numeric id) after an initialize on the
# same connection; echo the single response object whose id matches.
mcp_send() { # $1=request-json $2=id -> stdout response object; rc 2=no transport 5=no answer
    local req="$1" id="$2" out
    have socat || return 2
    have jq || return 2
    [ -S "$SOCKET" ] || return 2
    out="$(printf '%s\n%s\n' "$INIT" "$req" | socat -t"$TIMEOUT" - "UNIX-CONNECT:$SOCKET" 2>/dev/null)" || true
    [ -n "$out" ] || return 5
    printf '%s\n' "$out" | jq -c --argjson id "$id" 'select(.id==$id)' 2>/dev/null | head -n1
}
mcp_tool() { # $1=tool $2=args-json $3=id -> stdout result object (.result); rc from mcp_send
    local resp
    resp="$(mcp_send "$(jq -cn --arg n "$1" --argjson a "$2" --argjson id "$3" \
        '{jsonrpc:"2.0",id:$id,method:"tools/call",params:{name:$n,arguments:$a}}')" "$3")" || return $?
    [ -n "$resp" ] || return 5
    printf '%s' "$resp" | jq -c '.result' 2>/dev/null
}

# ============================ SYSTEM ========================================
if [ -x /usr/bin/vibed ]; then
    row PASS "vibed-binary" "/usr/bin/vibed present and executable"
elif [ -e /usr/bin/vibed ]; then
    row FAIL "vibed-binary" "/usr/bin/vibed present but NOT executable"
else
    missing "vibed-binary" "/usr/bin/vibed missing"
fi

# Immutable root: writing under /usr must fail with a read-only-fs error.
# `{ touch; } 2>&1` groups the redirect so a failed create is captured cleanly,
# never leaked to the terminal.
rotest="/usr/.vibeos-selfcheck.$$"
roerr="$({ touch "$rotest"; } 2>&1)"
if [ -e "$rotest" ]; then
    rm -f "$rotest" 2>/dev/null
    if [ "$ON_IMAGE" = 1 ]; then
        row FAIL "root-readonly" "/usr is WRITABLE — immutable root not enforced"
    else
        row SKIP "root-readonly" "/usr writable (host is not an immutable image)"
    fi
else
    case "$roerr" in
        *"Read-only file system"*) row PASS "root-readonly" "/usr is read-only (immutable root)" ;;
        *"Permission denied"*) row SKIP "root-readonly" "cannot probe /usr as this user; re-run as root" ;;
        *) row PASS "root-readonly" "/usr not writable ($roerr)" ;;
    esac
fi

if have bootc; then
    if bootc status >/dev/null 2>&1; then
        row PASS "bootc-status" "bootc status reports a valid deployment"
    else
        row FAIL "bootc-status" "bootc status failed (not a bootc deployment?)"
    fi
else
    missing "bootc-status" "bootc not installed"
fi

if [ -e "$MEMORY_DIR/.initialized" ]; then
    row PASS "genesis-done" "$MEMORY_DIR/.initialized present (first-boot Genesis ran)"
else
    missing "genesis-done" "$MEMORY_DIR/.initialized absent (Genesis did not complete)"
fi

# ADR-029: once Genesis ran, the AI citizen has a BORN character. Verify the
# soul file exists and carries a schema + name. Skipped before first boot (the
# genesis-done row already covers "Genesis did not run").
persona="$MEMORY_DIR/personality.toml"
if [ ! -e "$MEMORY_DIR/.initialized" ]; then
    row SKIP "ai-personality" "Genesis not run yet — no character born"
elif [ -f "$persona" ] && grep -q '^schema = ' "$persona" && grep -q '^name = "' "$persona"; then
    ai_name="$(sed -n 's/^name = "\(.*\)"$/\1/p' "$persona" | head -n1)"
    ai_arch="$(sed -n 's/^archetype = "\(.*\)"$/\1/p' "$persona" | head -n1)"
    row PASS "ai-personality" "citizen born: ${ai_name:-?} (${ai_arch:-?}) — ADR-029"
else
    missing "ai-personality" "$persona missing or malformed (Genesis ran but no character)"
fi

if have systemctl; then
    state="$(systemctl is-active vibed.service 2>/dev/null || true)"
    case "$state" in
        active) row PASS "vibed-service" "vibed.service is active (running)" ;;
        "") missing "vibed-service" "vibed.service unknown to systemd" ;;
        *) missing "vibed-service" "vibed.service is '$state' (want active)" ;;
    esac
else
    missing "vibed-service" "systemctl unavailable"
fi

# ============================ IDENTITY (sysusers) ============================
if have getent; then
    if getent passwd vibed >/dev/null 2>&1; then
        row PASS "user-vibed" "system user 'vibed' exists"
    else
        missing "user-vibed" "system user 'vibed' missing (sysusers.d not applied?)"
    fi
    if getent group vibeos-agents >/dev/null 2>&1; then
        row PASS "group-agents" "group 'vibeos-agents' exists"
    else
        missing "group-agents" "group 'vibeos-agents' missing (socket would be unusable)"
    fi
else
    missing "user-vibed" "getent unavailable"
    missing "group-agents" "getent unavailable"
fi

# ============================ SOCKET ========================================
if [ -S "$SOCKET" ]; then
    row PASS "socket-present" "$SOCKET is a unix socket"
    mode="$(stat -c '%a' "$SOCKET" 2>/dev/null)"
    grp="$(stat -c '%G' "$SOCKET" 2>/dev/null)"
    if { [ "$mode" = 660 ] || [ "$mode" = 0660 ]; } && [ "$grp" = vibeos-agents ]; then
        row PASS "socket-perms" "0660 root:vibeos-agents"
    else
        row FAIL "socket-perms" "mode=$mode group=$grp (want 0660 group vibeos-agents)"
    fi
else
    missing "socket-present" "$SOCKET absent (vibed not listening?)"
    row SKIP "socket-perms" "socket absent"
fi

# ============================ POLICY ========================================
if [ -f "$POLICY_DIR/default.toml" ]; then
    row PASS "policy-default" "$POLICY_DIR/default.toml shipped"
else
    missing "policy-default" "$POLICY_DIR/default.toml absent (default-deny everything)"
fi

# ============================ MCP (live, read-only) =========================
if have socat && have jq && [ -S "$SOCKET" ]; then
    # initialize handshake
    if init="$(mcp_send "$INIT" 0)" && [ -n "$init" ] &&
        printf '%s' "$init" | jq -e '.result.protocolVersion // .result' >/dev/null 2>&1; then
        row PASS "mcp-initialize" "initialize handshake answered"
    else
        row FAIL "mcp-initialize" "no valid initialize response over the socket"
    fi

    # tools/list
    tools="$(mcp_send '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' 2)"
    ntools="$(printf '%s' "$tools" | jq -r '.result.tools | length' 2>/dev/null || echo 0)"
    if [ "${ntools:-0}" -gt 0 ] 2>/dev/null; then
        row PASS "mcp-tools-list" "tools/list advertises $ntools tools"
    else
        row FAIL "mcp-tools-list" "tools/list returned no tools"
    fi

    # os.status (T0 read)
    r="$(mcp_tool os.status '{}' 103)"
    if printf '%s' "$r" | jq -e '.isError==false' >/dev/null 2>&1; then
        row PASS "mcp-os-status" "os.status (T0) returns a result"
    else
        row FAIL "mcp-os-status" "os.status did not return a clean result"
    fi

    # built-in denylist: fs.read of /etc/shadow MUST be refused, policy or not
    r="$(mcp_tool fs.read '{"path":"/etc/shadow"}' 104)"
    if printf '%s' "$r" | jq -e '.isError==true' >/dev/null 2>&1; then
        row PASS "mcp-denylist" "fs.read /etc/shadow refused (built-in denylist live)"
    else
        row FAIL "mcp-denylist" "fs.read /etc/shadow was NOT refused — denylist not enforced!"
    fi

    # T2 approval floor, via the policy.check DRY-RUN (no mutation). SKIP if the
    # image predates policy.check.
    if printf '%s' "$tools" | jq -e '.result.tools[]?.name=="policy.check"' >/dev/null 2>&1; then
        r="$(mcp_tool policy.check '{"tool":"svc.restart","target":"sshd"}' 105)"
        dec="$(printf '%s' "$r" | jq -r '.content[0].text' 2>/dev/null | jq -r '.decision' 2>/dev/null)"
        if [ "$dec" = require_approval ]; then
            row PASS "mcp-t2-floor" "policy.check(svc.restart) = require_approval (T2 floor holds)"
        else
            row FAIL "mcp-t2-floor" "policy.check(svc.restart) = '$dec' (want require_approval)"
        fi
    else
        row SKIP "mcp-t2-floor" "policy.check not in this image; T2 floor covered by unit tests"
    fi
else
    for c in mcp-initialize mcp-tools-list mcp-os-status mcp-denylist mcp-t2-floor; do
        row SKIP "$c" "socat/jq/socket unavailable — cannot probe the live MCP surface"
    done
fi

# ============================ AUDIT =========================================
if ls "$AUDIT_DIR"/vibed*.jsonl >/dev/null 2>&1; then
    row PASS "audit-present" "append-only audit log present in $AUDIT_DIR"
else
    missing "audit-present" "no vibed*.jsonl in $AUDIT_DIR (run as root to read it)"
fi

if have vibectl; then
    aout="$(vibectl audit verify "$AUDIT_DIR" 2>&1)"
    arc=$?
    if [ "$arc" -eq 0 ]; then
        row PASS "audit-chain" "vibectl audit verify: chain intact"
    elif printf '%s' "$aout" | grep -qiE 'usage|unknown|unexpected|expected'; then
        row SKIP "audit-chain" "this vibectl has no 'audit verify' subcommand"
    else
        row FAIL "audit-chain" "audit chain BROKEN: $(printf '%s' "$aout" | head -n1)"
    fi
else
    row SKIP "audit-chain" "vibectl not installed; verify the chain manually (docs/VALIDATION.md)"
fi

# ============================ KERNEL TUNING =================================
# ADR-025: AI-workload kernel tuning shipped as image content in
# /usr/lib/sysctl.d/50-vibeos-ai.conf. Version-tolerant: an image without the
# file SKIPs (the capability does not exist there), never FAILs. Read-only
# probe on two sentinels: vm.page-cluster proves systemd-sysctl applied the
# file, tcp_congestion_control proves the tcp_bbr module AUTOLOADED from the
# sysctl write (the riskiest assumption of the file — see its header).
SYSCTL_AI_CONF="/usr/lib/sysctl.d/50-vibeos-ai.conf"
if [ ! -f "$SYSCTL_AI_CONF" ]; then
    row SKIP "kernel-tuning" "$SYSCTL_AI_CONF not shipped (older image)"
elif ! have sysctl; then
    row SKIP "kernel-tuning" "sysctl unavailable — cannot probe live tunables"
else
    pc="$(sysctl -n vm.page-cluster 2>/dev/null)"
    cc="$(sysctl -n net.ipv4.tcp_congestion_control 2>/dev/null)"
    if [ "$pc" = "0" ] && [ "$cc" = "bbr" ]; then
        row PASS "kernel-tuning" "AI sysctl in effect (vm.page-cluster=0, tcp_bbr autoloaded)"
    else
        row FAIL "kernel-tuning" "shipped $SYSCTL_AI_CONF not in effect (page-cluster=${pc:-?}, tcp=${cc:-?})"
    fi
fi

# ============================ OPERATING MODE ================================
# ADR-027: report the operating mode (governed default vs human-unlocked OPEN
# mode). This is the boot/CLI face of the DANGER PANEL. Open mode is a
# LEGITIMATE state, not a failure — so BOTH governed and open are PASS (the exit
# code must not flip just because autonomy is on), but open is surfaced LOUDLY
# so a `--json` reader or a human scanning the table cannot miss it. Read-only:
# it never sets the mode (the unlock is `vibectl mode open`, root, out-of-band).
if have vibectl && have jq; then
    mode_json="$(vibectl mode status 2>/dev/null)"
    mode_val="$(printf '%s' "$mode_json" | jq -r '.mode // empty' 2>/dev/null)"
    case "$mode_val" in
        governed)
            row PASS "operating-mode" "governed (default; T2/T3 require human approval)" ;;
        open)
            rem="$(printf '%s' "$mode_json" | jq -r '.remaining_secs // "?"' 2>/dev/null)"
            row PASS "operating-mode" \
                "⚠ OPEN/AUTONOMOUS MODE ACTIVE (ADR-027) — T2/T3 auto-granted, ${rem}s left; revert with 'vibectl mode governed'" ;;
        *)
            row SKIP "operating-mode" "could not read mode ('vibectl mode status'); older image?" ;;
    esac
else
    row SKIP "operating-mode" "vibectl/jq unavailable — cannot read the operating mode"
fi

# ============================ REPORT ========================================
if [ "$JSON" = 1 ]; then
    printf '%s' "$ROWS" | jq -R -s \
        --argjson p "$pass" --argjson f "$fail" --argjson s "$skip" --arg sock "$SOCKET" '
        (split("\n") | map(select(length>0)) | map(split("\u001f"))
         | map({status:.[0], check:.[1], detail:.[2]})) as $checks
        | {tool:"vibeos-selfcheck", ok:($f==0),
           summary:{pass:$p, fail:$f, skip:$s}, socket:$sock, checks:$checks}'
else
    printf 'vibeos-selfcheck — socket=%s image=%s\n\n' "$SOCKET" "$ON_IMAGE"
    printf '%s' "$ROWS" | while IFS="$US" read -r st ck dt; do
        [ -n "$st" ] || continue
        printf '  [%-4s] %-16s %s\n' "$st" "$ck" "$dt"
    done
    printf '\n  %d passed, %d failed, %d skipped — %s\n' \
        "$pass" "$fail" "$skip" "$([ "$fail" -eq 0 ] && echo OK || echo FAILED)"
fi

[ "$fail" -eq 0 ]
