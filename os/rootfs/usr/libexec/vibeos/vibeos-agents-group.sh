#!/usr/bin/env bash
# vibeos-agents-group.sh — make the vibed MCP socket usable out of the box.
#
# The socket (/run/vibed/mcp.sock) is root:vibeos-agents 0660, but neither
# Anaconda nor the kickstart adds the human account to vibeos-agents: on a
# fresh install the agent -> vibed chain would be dead until a manual
# usermod. Enrollment policy: every member of `wheel` (the human
# administrators) joins vibeos-agents. They already hold full sudo, so this
# grants them strictly LESS than they have — no privilege escalation.
#
# Idempotent; runs as a oneshot at every boot (vibeos-agents-group.service);
# a freshly added group is effective at the user's next login.

set -euo pipefail

if ! getent group vibeos-agents >/dev/null; then
    echo "group vibeos-agents missing (sysusers.d not applied?); nothing to do" >&2
    exit 0
fi

# Guard the wheel lookup the same graceful way: on a host with no `wheel`
# group, `getent` exits non-zero and pipefail would abort a best-effort script.
if ! getent group wheel >/dev/null 2>&1; then
    echo "group wheel missing; no administrators to enroll" >&2
    exit 0
fi

members="$(getent group wheel | cut -d: -f4 | tr ',' ' ')"
for user in $members; do
    [ -n "$user" ] || continue
    if id -nG "$user" 2>/dev/null | tr ' ' '\n' | grep -qx vibeos-agents; then
        continue
    fi
    # A single non-local member (LDAP/SSSD/AD account absent from /etc/passwd)
    # makes usermod exit non-zero. Do not let that abort the loop and fail the
    # oneshot: this is best-effort enrollment — skip the user and keep going.
    if usermod -aG vibeos-agents "$user"; then
        echo "added $user (wheel) to vibeos-agents"
    else
        echo "warning: could not add $user to vibeos-agents (non-local account?); skipping" >&2
    fi
done
