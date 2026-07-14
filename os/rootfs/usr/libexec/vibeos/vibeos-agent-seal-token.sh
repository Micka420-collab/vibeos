#!/usr/bin/env bash
# vibeos-agent-seal-token.sh — TPM2-seal an AI subscription token for an agent
# instance, so vibeos-agent@<instance>.service can load it with
# LoadCredentialEncrypted= without the plaintext ever touching persistent disk.
#
# Usage (operator, once per instance):
#   claude setup-token            # or: codex login --with-access-token …
#   printf '%s' "$TOKEN" | /usr/libexec/vibeos/vibeos-agent-seal-token.sh alice
#
# The token is read from stdin, encrypted by systemd-creds against the machine's
# TPM2 (Storage Root Key) + host key, and only the SEALED blob is written to
# /etc/vibeos/credentials/<instance>-token.cred. A blob copied off this machine
# is useless: only this TPM can unseal it. Phase 2.5 (ADR-013); the same TPM2
# trust anchor as the Phase 3 LUKS memory volume, brought forward because the
# external-auth story needs it now.

set -euo pipefail

instance="${1:?usage: vibeos-agent-seal-token.sh <instance>  (token on stdin)}"
# Defense in depth: the instance names the output credential file below; reject
# anything but a safe name (no '/', so no path traversal out of credentials/).
case "$instance" in
    '' | *[!A-Za-z0-9._-]*)
        echo "vibeos-agent-seal-token: invalid instance name '${instance}'" >&2
        exit 1
        ;;
esac

if ! command -v systemd-creds >/dev/null 2>&1; then
    echo "systemd-creds not found (systemd too old?)" >&2
    exit 1
fi

cred_dir=/etc/vibeos/credentials
out="${cred_dir}/${instance}-token.cred"
install -d -m 0700 "$cred_dir"

# --with-key=tpm2+host binds the secret to BOTH the TPM and the host key, so it
# unseals only on this machine with its TPM present. On a machine WITHOUT a TPM,
# re-run with VIBEOS_SEAL_KEY=auto (host-only) — documented, weaker, explicit.
key="${VIBEOS_SEAL_KEY:-tpm2+host}"

umask 0077
# Read plaintext from stdin ('-'), never from a file/argument: it stays off disk.
systemd-creds encrypt --name=vibeos-agent-token --with-key="$key" - "$out"
chmod 0600 "$out"

echo "sealed agent token -> $out (--with-key=$key)" >&2
echo "only this machine can unseal it; vibeos-agent@${instance}.service will load it." >&2
