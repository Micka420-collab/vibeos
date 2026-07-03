#!/usr/bin/env bash
#
# genesis.sh — VibeOS first-boot memory initialization ("Genesis" sequence).
#
# Creates the machine memory layout specified in docs/MEMORY.md under
# ${VIBEOS_MEMORY_DIR:-/var/lib/vibeos/memory}. In the OS image this script is
# installed at /usr/libexec/vibeos/genesis.sh and invoked by
# vibeos-genesis.service, which carries the systemd guard:
#
#     ConditionPathExists=!/var/lib/vibeos/memory/.initialized
#
# The sentinel file .initialized is written LAST, which makes the sequence
# crash-safe: an interrupted run leaves no sentinel and is replayed in full on
# the next boot. Every step before the sentinel is an idempotent create or
# overwrite.
#
# Environment:
#   VIBEOS_MEMORY_DIR    target directory (default: /var/lib/vibeos/memory)
#   VIBEOS_MEMORY_MODE   "persistent" (default) or "amnesic" (injected by the
#                        amnesic-mode systemd drop-in, see docs/MEMORY.md §5)
#
# Exit code: 0 on success, and 0 immediately if memory is already initialized.

set -euo pipefail

readonly MEMORY_DIR="${VIBEOS_MEMORY_DIR:-/var/lib/vibeos/memory}"
readonly MEMORY_MODE="${VIBEOS_MEMORY_MODE:-persistent}"
readonly SCHEMA_VERSION=1

log() { printf 'vibeos-genesis: %s\n' "$*" >&2; }

# --- Idempotency guard (must remain the very first action) -------------------
if [ -e "${MEMORY_DIR}/.initialized" ]; then
    log "memory already initialized at ${MEMORY_DIR} — nothing to do"
    exit 0
fi

log "starting Genesis sequence (mode=${MEMORY_MODE}, target=${MEMORY_DIR})"

# Everything born here is private to the machine and its human.
umask 077

# --- Helpers ------------------------------------------------------------------

# capture CMD [ARGS...] — print CMD's stdout, or an explicit placeholder when
# the tool is missing or fails. Never returns a non-zero status, so hardware
# collection degrades gracefully on minimal or exotic systems.
capture() {
    if command -v "$1" >/dev/null 2>&1; then
        "$@" 2>/dev/null || printf '(%s failed)' "$1"
    else
        printf '(%s not available)' "$1"
    fi
}

# json_escape STRING — escape STRING for embedding as a JSON string value.
json_escape() {
    local s
    # Drop control characters JSON cannot carry raw (keep tab, LF, CR).
    s=$(printf '%s' "$1" | tr -d '\000-\010\013\014\016-\037')
    s=${s//\\/\\\\}
    s=${s//\"/\\\"}
    s=${s//$'\t'/\\t}
    s=${s//$'\r'/\\r}
    s=${s//$'\n'/\\n}
    printf '%s' "$s"
}

# toml_escape STRING — escape STRING for a TOML basic (double-quoted) string.
toml_escape() {
    local s=$1
    s=${s//\\/\\\\}
    s=${s//\"/\\\"}
    printf '%s' "$s"
}

get_hostname() {
    if command -v hostname >/dev/null 2>&1; then
        hostname 2>/dev/null || printf 'unknown'
    elif [ -r /proc/sys/kernel/hostname ]; then
        cat /proc/sys/kernel/hostname
    elif [ -n "${HOSTNAME:-}" ]; then
        printf '%s' "${HOSTNAME}"
    else
        printf 'unknown'
    fi
}

get_machine_id() {
    if [ -r /etc/machine-id ]; then
        cat /etc/machine-id
    else
        printf 'unknown'
    fi
}

# write_placeholder SUBDIR BODY — drop a French README placeholder in SUBDIR.
write_placeholder() {
    local subdir=$1 body=$2
    {
        printf '# %s/\n\n' "${subdir}"
        printf '%s\n\n' "${body}"
        printf '%s\n' "Spécification complète : docs/MEMORY.md dans le dépôt VibeOS."
    } > "${MEMORY_DIR}/${subdir}/README.md"
}

# --- 1. Directory skeleton -----------------------------------------------------
mkdir -p \
    "${MEMORY_DIR}/user" \
    "${MEMORY_DIR}/projects" \
    "${MEMORY_DIR}/journal" \
    "${MEMORY_DIR}/knowledge"
chmod 700 "${MEMORY_DIR}"
log "directory skeleton created"

# --- 2. Hardware profile -> hardware.json ---------------------------------------
BIRTH_TS="$(date -Is)"
KERNEL_INFO="$(capture uname -srmo)"
CPU_INFO="$(capture lscpu)"
MEM_INFO="$(capture free -h)"
DISK_INFO="$(capture lsblk -o NAME,SIZE,TYPE,FSTYPE,MOUNTPOINT)"
FS_INFO="$(capture df -h)"

cat > "${MEMORY_DIR}/hardware.json" <<EOF
{
  "schema": ${SCHEMA_VERSION},
  "collected_at": "$(json_escape "${BIRTH_TS}")",
  "kernel": "$(json_escape "${KERNEL_INFO}")",
  "cpu": "$(json_escape "${CPU_INFO}")",
  "memory": "$(json_escape "${MEM_INFO}")",
  "block_devices": "$(json_escape "${DISK_INFO}")",
  "filesystems": "$(json_escape "${FS_INFO}")"
}
EOF
log "hardware profile written"

# --- 3. Machine identity -> identity.toml ---------------------------------------
HOSTNAME_VALUE="$(get_hostname)"
MACHINE_ID="$(get_machine_id)"

cat > "${MEMORY_DIR}/identity.toml" <<EOF
# VibeOS machine identity — written once by the Genesis sequence.
# See docs/MEMORY.md §3.1. Do not edit by hand; use vibectl (future CLI).
schema = ${SCHEMA_VERSION}
hostname = "$(toml_escape "${HOSTNAME_VALUE}")"
machine_id = "$(toml_escape "${MACHINE_ID}")"
birth = "$(toml_escape "${BIRTH_TS}")"
mode = "$(toml_escape "${MEMORY_MODE}")"
EOF
log "identity written (hostname=${HOSTNAME_VALUE}, birth=${BIRTH_TS})"

# --- 4. README placeholders ------------------------------------------------------
write_placeholder user \
    "Profil de l'humain : identité déclarée, préférences, style de code. Sera alimenté au fil de l'eau par vibed via l'outil MCP memory.append (cible Phase 2). Vide à la naissance : la machine ne sait encore rien de vous."
write_placeholder projects \
    "Index des projets connus de la machine (index.json). Sera mis à jour par vibed quand un agent ouvre ou découvre un projet (cible Phase 2)."
write_placeholder journal \
    "Événements append-only : un fichier JSONL par jour UTC (AAAA-MM-JJ.jsonl), une ligne JSON par événement. Ne jamais réécrire une ligne existante ; une correction est un nouvel événement."
write_placeholder knowledge \
    "Faits appris, consolidés depuis le journal (facts.jsonl). Le sous-répertoire embeddings/ est réservé au futur index vectoriel local."
log "placeholders written"

# --- 5. First journal entry --------------------------------------------------------
JOURNAL_FILE="${MEMORY_DIR}/journal/$(date -u +%Y-%m-%d).jsonl"
printf '{"ts":"%s","type":"genesis","source":"genesis.sh","data":{"mode":"%s","hostname":"%s","schema":%s}}\n' \
    "$(json_escape "${BIRTH_TS}")" \
    "$(json_escape "${MEMORY_MODE}")" \
    "$(json_escape "${HOSTNAME_VALUE}")" \
    "${SCHEMA_VERSION}" \
    >> "${JOURNAL_FILE}"
log "first journal entry appended to ${JOURNAL_FILE}"

# --- 6. Sentinel — MUST stay the last write ------------------------------------------
printf '%s\n' "${BIRTH_TS}" > "${MEMORY_DIR}/.initialized"
log "Genesis complete — memory born at ${BIRTH_TS} (mode=${MEMORY_MODE})"
exit 0
