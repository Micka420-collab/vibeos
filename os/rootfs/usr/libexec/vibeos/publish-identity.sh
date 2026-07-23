#!/usr/bin/env bash
#
# publish-identity.sh — project the AI citizen's PUBLIC identity to a
# world-readable runtime marker so the login greeter can name it.
#
# VibeOS treats first boot as the birth of the machine's AI citizen: Genesis
# (memory/genesis.sh) derives a unique name / archetype / tone / traits
# DETERMINISTICALLY from machine_id and writes them to
#   /var/lib/vibeos/memory/personality.toml
# That file lives inside the 0700, root-only memory store — the machine's
# private memory belongs to its human (charter). The SDDM greeter runs as the
# unprivileged `sddm` user and cannot (and must not) read the memory store.
#
# This script runs as root once per boot (vibeos-identity.service, ordered
# After=vibeos-genesis.service Before=display-manager.service) and copies ONLY
# the citizen's PUBLIC identity — its name, archetype, tone, birth-seed and
# birth timestamp, the same fields the birth ceremony already prints to the
# console and stamps in the journal genesis event — to:
#   /run/vibeos/citizen.json   (0644, on tmpfs, regenerated every boot)
# Nothing about the human, the hostname or the machine_id is ever exposed here.
# The greeter reads this best-effort and degrades to a plain "VibeOS" identity
# if it is absent (docs/DESKTOP.md, desktop/sddm/README.md).
#
# Robustness: this can NEVER block or fail login. It exits 0 on every path —
# no personality file, unreadable file, missing fields — leaving the greeter to
# fall back gracefully. `set -u` guards typos; there is no `set -e` precisely so
# a single failed step cannot abort the publish.

set -uo pipefail

MEMORY_DIR="${VIBEOS_MEMORY_DIR:-/var/lib/vibeos/memory}"
PERSONALITY="${MEMORY_DIR}/personality.toml"
RUN_DIR="/run/vibeos"
OUT="${RUN_DIR}/citizen.json"

log() { printf 'vibeos-identity: %s\n' "$*" >&2; }

# Fixed PATH — never the inherited one (same standard as genesis.sh: this runs
# as root, so a bare-name tool must resolve to a trusted location).
PATH=/usr/sbin:/usr/bin:/sbin:/bin
export PATH

# The runtime marker directory is world-traversable so the sddm user can reach
# the file; the file itself is world-readable (public identity, no secrets).
mkdir -p "${RUN_DIR}" 2>/dev/null || true
chmod 0755 "${RUN_DIR}" 2>/dev/null || true

if [ ! -r "${PERSONALITY}" ]; then
    log "no readable personality.toml at ${PERSONALITY} — greeter will use the plain VibeOS identity"
    exit 0
fi

# toml_get KEY — read a top-level `key = "value"` basic string from the [root]
# table of personality.toml (before the first [section] header). Anchored to
# top-level keys so a same-named key under [traits]/[adaptation] is never
# mistaken for the identity value. Prints the raw (still TOML-escaped) value.
toml_get() {
    awk -v key="$1" '
        /^[[:space:]]*\[/ { in_root = 0; next }   # a [section] ends the root table
        NR == 1 { in_root = 1 }
        {
            if (!in_root) next
            line = $0
            # match:  key = "value"
            if (match(line, "^[[:space:]]*" key "[[:space:]]*=[[:space:]]*\"")) {
                rest = substr(line, RSTART + RLENGTH)
                # strip the trailing quote and anything after it
                sub(/\".*$/, "", rest)
                print rest
                exit
            }
        }
    ' "${PERSONALITY}" 2>/dev/null || true
}

# json_string VALUE — emit VALUE as a safe JSON string literal (with quotes).
# The personality fields come from controlled vocabularies (a fixed name pool,
# fixed French archetype/tone strings, a hex seed, an ISO timestamp) and cannot
# contain quotes or backslashes — but we escape defensively all the same, and
# drop control characters JSON cannot carry raw.
json_string() {
    local s
    s=$(printf '%s' "$1" | tr -d '\000-\037')
    s=${s//\\/\\\\}
    s=${s//\"/\\\"}
    printf '"%s"' "${s}"
}

NAME="$(toml_get name)"
ARCHETYPE="$(toml_get archetype)"
TONE="$(toml_get tone)"
SEED="$(toml_get seed)"
BIRTH="$(toml_get birth)"

# A citizen without a name is not one we can surface — degrade silently.
if [ -z "${NAME}" ]; then
    log "personality.toml carried no name — greeter will use the plain VibeOS identity"
    exit 0
fi

TMP="${OUT}.tmp.$$"
{
    printf '{\n'
    printf '  "schema": 1,\n'
    printf '  "name": %s,\n'       "$(json_string "${NAME}")"
    printf '  "archetype": %s,\n'  "$(json_string "${ARCHETYPE}")"
    printf '  "tone": %s,\n'       "$(json_string "${TONE}")"
    printf '  "seed": %s,\n'       "$(json_string "${SEED}")"
    printf '  "birth": %s\n'       "$(json_string "${BIRTH}")"
    printf '}\n'
} > "${TMP}" 2>/dev/null || { log "could not write ${TMP}"; exit 0; }

# Atomic publish + world-readable (public identity, no secrets).
chmod 0644 "${TMP}" 2>/dev/null || true
mv -f "${TMP}" "${OUT}" 2>/dev/null || { rm -f "${TMP}" 2>/dev/null || true; exit 0; }

log "published citizen identity: ${NAME} (${ARCHETYPE}) -> ${OUT}"
exit 0
