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

members="$(getent group wheel | cut -d: -f4 | tr ',' ' ')"
for user in $members; do
    [ -n "$user" ] || continue
    if id -nG "$user" | tr ' ' '\n' | grep -qx vibeos-agents; then
        continue
    fi
    usermod -aG vibeos-agents "$user"
    echo "added $user (wheel) to vibeos-agents"
done
