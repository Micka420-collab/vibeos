#!/usr/bin/env bash
# vibeos-agent-run.sh — ExecStart wrapper for vibeos-agent@<instance>.service.
#
# Wires the TPM2-unsealed subscription token to the CLI's native env var (never
# to disk), then hands off to the vibectl supervisor (budget, reasoning capture,
# operator kill-switch — ADR-012/013). The supervisor, NOT this wrapper, is the
# governance boundary; this only assembles the command line.

set -euo pipefail

instance="${1:?usage: vibeos-agent-run.sh <instance>}"
# Defense in depth: the instance (systemd %i) is interpolated into the config
# path below; reject anything but a safe name (no '/', no path traversal).
case "$instance" in
    '' | *[!A-Za-z0-9._-]*)
        echo "vibeos-agent-run: invalid instance name '${instance}'" >&2
        exit 1
        ;;
esac

conf="/etc/vibeos/agent.d/${instance}.conf"
if [ ! -r "$conf" ]; then
    echo "no agent config for instance '${instance}' (${conf})" >&2
    exit 1
fi

# The config is sourced bash. It must define AGENT_COMMAND as an ARRAY (the CLI
# to supervise, passed after `--`). Optional: AGENT_BUDGET (default 8h), TOKEN_ENV
# (the env var the CLI reads its subscription token from).
AGENT_BUDGET=8h
TOKEN_ENV=""
AGENT_COMMAND=()
# shellcheck source=/dev/null
. "$conf"

if [ "${#AGENT_COMMAND[@]}" -eq 0 ]; then
    echo "${conf} must define AGENT_COMMAND=(…) — the CLI to supervise" >&2
    exit 1
fi

# systemd decrypted the token into $CREDENTIALS_DIRECTORY (unit-private tmpfs,
# wiped at stop). Export it to the CLI's native variable if the config asks —
# the plaintext lives only in this process's environment, never on disk.
if [ -n "$TOKEN_ENV" ]; then
    tok="${CREDENTIALS_DIRECTORY:?unit must set LoadCredentialEncrypted}/vibeos-agent-token"
    if [ ! -r "$tok" ]; then
        echo "token credential missing (seal it with vibeos-agent-seal-token.sh ${instance})" >&2
        exit 1
    fi
    export "${TOKEN_ENV}=$(cat "$tok")"
fi

exec vibectl agent run --budget "$AGENT_BUDGET" -- "${AGENT_COMMAND[@]}"
