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
# the next boot. Every step before the sentinel is therefore an idempotent create
# or overwrite — EXCEPT the first journal entry, which cannot be (the journal is
# append-only by contract) and is GUARDED instead, so a replay never stamps a
# second birth. See step 5.
#
# NB: `ConditionPathExists=!` + the sentinel check below test re-entrancy of the
# GUARD, not of the SEQUENCE. Replaying a partial run (sentinel absent, some files
# already written) is the case that actually exercises the steps — CI covers it.
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
# Memory-layout / identity schema (identity.toml, journal genesis event).
readonly SCHEMA_VERSION=1
# hardware.json schema — bumped to 2 for structured cpu/memory/gpu fields
# (was 1: free-text command blobs only). See docs/MEMORY.md §3.2.
readonly HARDWARE_SCHEMA_VERSION=2
# personality.toml schema — the AI citizen's BIRTH character. Carries its own
# number (like hardware.json) distinct from the memory/identity schema. §3.8.
readonly PERSONALITY_SCHEMA_VERSION=1

log() { printf 'vibeos-genesis: %s\n' "$*" >&2; }

# --- Idempotency guard (must remain the very first action) -------------------
if [ -e "${MEMORY_DIR}/.initialized" ]; then
    log "memory already initialized at ${MEMORY_DIR} — nothing to do"
    exit 0
fi

log "starting Genesis sequence (mode=${MEMORY_MODE}, target=${MEMORY_DIR})"

# Everything born here is private to the machine and its human.
umask 077

# Fixed PATH — never the inherited one. This script runs as ROOT at first boot
# and resolves every tool it uses (hostname, awk, nproc, lspci, nvidia-smi…) by
# bare name. `vibed/src/tools/svc.rs` already holds this standard for a single
# binary, and its reason applies verbatim here: a constant, never a PATH lookup,
# "so a compromised environment can never redirect what vibed — running as root —
# executes".
#
# What this actually removes: vibeos-genesis.service sets no Environment=PATH=,
# so systemd's compiled default applies — and it puts /usr/local FIRST. On Fedora
# bootc /usr/local is a symlink into /var/usrlocal (docs/SECURITY-ARCHITECTURE.md
# §"OSTree layout"), i.e. MUTABLE state. Planting there needs root, so this is
# defence in depth, not a live hole — but it matters most in AMNESIC mode, where
# genesis re-runs on EVERY boot while the tmpfs covers only the memory store:
# /var/usrlocal survives the wipe, which would otherwise hand a root attacker a
# boot-guaranteed re-execution hook that outlives the very thing amnesic mode
# exists to erase.
PATH=/usr/sbin:/usr/bin:/sbin:/bin
export PATH

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
#
# Strips the control characters TOML forbids raw in a basic string
# (U+0000–U+0008, U+000A–U+001F, U+007F), for the SAME reason json_escape does
# just above — the two escapers were asymmetric, and nothing said why.
#
# Why this is not cosmetic: `memory.query` does NOT parse identity.toml. There is
# no `toml::from_str` on this file anywhere — vibed serves it to agents as a RAW
# TEXT SNIPPET (tools/memory.rs). So a value carrying a newline does not produce a
# clean "parse error" an agent would ignore: it produces extra LINES that read as
# facts. `hostname = "evil\nmachine_id = \"forged\""` renders, in the snippet an
# LLM is handed, as a machine_id assignment. That is A1 poisoning (THREAT-MODEL:
# "altération = empoisonnement durable du comportement des agents"), and it is
# PERMANENT: the .initialized sentinel is stamped right after, so the guard
# prevents any repair on a later boot.
#
# Not reachable today — every input needs a RAW newline, and the kernel hostname
# (CAP_SYS_ADMIN), systemd-hostnamed (`hostname_is_valid()`) and NetworkManager
# all reject control characters, which closes the DHCP option-12 path. Quotes were
# already escaped, so a clean forgery was never possible either. This closes the
# gap at the escaper rather than relying on every future caller's input being
# well-behaved.
toml_escape() {
    local s
    s=$(printf '%s' "$1" | tr -d '\000-\010\012-\037\177')
    s=${s//\\/\\\\}
    s=${s//\"/\\\"}
    printf '%s' "$s"
}

# fnv1a32 STRING — 32-bit FNV-1a hash of STRING, printed as an unsigned decimal.
#
# Pure bash on purpose: the citizen's BIRTH character (§3.8, personality.toml)
# must derive identically on ANY host — including the most minimal — and never
# depend on an external tool or a PATH lookup (same standard the header states
# for every command this script runs as root). The accumulator is masked to 32
# bits at every step, so bash's 64-bit signed arithmetic never overflows into
# undefined territory. `LC_ALL=C` (function-local) forces byte semantics: without
# it, in a UTF-8 locale `${s:i:1}` walks CODE POINTS, so a non-ASCII hostname
# fallback would hash differently per locale. The primary anchor (machine_id) is
# hex ASCII and unaffected either way; this keeps the fallback path deterministic
# too. ASCII inputs hash identically under C and UTF-8, so pinned vectors hold.
fnv1a32() {
    local LC_ALL=C LC_CTYPE=C
    local s=$1 h=2166136261 i code
    for (( i = 0; i < ${#s}; i++ )); do
        printf -v code '%d' "'${s:i:1}"
        h=$(( (h ^ (code & 0xff)) & 0xffffffff ))
        h=$(( (h * 16777619) & 0xffffffff ))
    done
    printf '%u' "${h}"
}

# draw SEED LABEL MODULO — a deterministic value in [0, MODULO) for LABEL.
# Independent across labels: each rehashes SEED with LABEL as a suffix, so two
# traits drawn from the same seed do not correlate.
draw() {
    local seed=$1 label=$2 modulo=$3 h
    h=$(fnv1a32 "${seed}|${label}")
    printf '%d' $(( h % modulo ))
}

# birth_ceremony — the visible "awakening": a short, futuristic reveal of the
# citizen just born, printed to the boot console (stderr, like log()). Purely
# cosmetic and best-effort — it can NEVER fail the Genesis sequence (called as
# `birth_ceremony || true` under set -e). Colour only on a real, non-dumb
# terminal with NO_COLOR unset; a plain console gets clean monochrome text.
# Reads the AI_* / TRAIT_* values derived in step 3.5.
birth_ceremony() {
    local hl='' dim='' glow='' reset=''
    if [ -t 2 ] && [ -z "${NO_COLOR:-}" ] && [ "${TERM:-dumb}" != dumb ]; then
        hl=$'\033[38;5;51m'   # bright cyan — frame
        glow=$'\033[38;5;45m' # cyan — name & manifesto
        dim=$'\033[2m'        # dim — labels
        reset=$'\033[0m'
    fi
    # gauge VALUE — a ten-cell bar for a 0..100 trait.
    gauge() {
        local v=$1 filled=0 i out=
        filled=$(( (v + 5) / 10 ))
        [ "${filled}" -gt 10 ] && filled=10
        [ "${filled}" -lt 0 ] && filled=0
        for (( i = 0; i < 10; i++ )); do
            if [ "${i}" -lt "${filled}" ]; then out="${out}▓"; else out="${out}░"; fi
        done
        printf '%s' "${out}"
    }
    {
        printf '\n'
        printf '%s   ╔════════════════════════════════════════════╗%s\n' "${hl}" "${reset}"
        printf '%s   ║   V I B E O S   ·   G E N E S I S           ║%s\n' "${hl}" "${reset}"
        printf '%s   ╚════════════════════════════════════════════╝%s\n' "${hl}" "${reset}"
        printf '%s        un citoyen s'\''éveille…%s\n\n' "${dim}" "${reset}"
        printf '        %snom%s        %s%s%s\n' "${dim}" "${reset}" "${glow}" "${AI_NAME}" "${reset}"
        printf '        %sarchétype%s  %s\n' "${dim}" "${reset}" "${AI_ARCHETYPE}"
        printf '        %ston%s        %s\n\n' "${dim}" "${reset}" "${AI_TONE}"
        printf '        %scuriosité %s  %s  %3d\n' "${dim}" "${reset}" "$(gauge "${TRAIT_CURIOSITY}")" "${TRAIT_CURIOSITY}"
        printf '        %sprudence  %s  %s  %3d\n' "${dim}" "${reset}" "$(gauge "${TRAIT_CAUTION}")" "${TRAIT_CAUTION}"
        printf '        %sinitiative%s  %s  %3d\n' "${dim}" "${reset}" "$(gauge "${TRAIT_INITIATIVE}")" "${TRAIT_INITIATIVE}"
        printf '        %schaleur   %s  %s  %3d\n' "${dim}" "${reset}" "$(gauge "${TRAIT_WARMTH}")" "${TRAIT_WARMTH}"
        printf '        %sconcision %s  %s  %3d\n' "${dim}" "${reset}" "$(gauge "${TRAIT_CONCISION}")" "${TRAIT_CONCISION}"
        printf '        %sjeu       %s  %s  %3d\n\n' "${dim}" "${reset}" "$(gauge "${TRAIT_PLAYFULNESS}")" "${TRAIT_PLAYFULNESS}"
        printf '        %s« Je nais sur cette machine. Je ne sais rien de vous —%s\n' "${glow}" "${reset}"
        printf '        %s  encore. Apprenons-nous. »%s\n\n' "${glow}" "${reset}"
    } >&2
    unset -f gauge
}

# Read the current hostname, but NEVER bake a transient install-time default
# (localhost/fedora before the installer or NetworkManager sets the real name)
# as the machine's BIRTH name — the birth identity is a signature concept, and
# the stable anchor is machine_id anyway. A transient default becomes "unknown"
# (the persistent name is picked up on a later reboot / by vibectl).
get_hostname() {
    raw=""
    if command -v hostname >/dev/null 2>&1; then
        raw=$(hostname 2>/dev/null || true)
    elif [ -r /proc/sys/kernel/hostname ]; then
        raw=$(cat /proc/sys/kernel/hostname 2>/dev/null || true)
    elif [ -n "${HOSTNAME:-}" ]; then
        raw="${HOSTNAME}"
    fi
    case "${raw}" in
        '' | localhost | localhost.localdomain | fedora | localhost.* )
            printf 'unknown' ;;
        *)
            printf '%s' "${raw}" ;;
    esac
}

get_machine_id() {
    local id=''
    if [ -r /etc/machine-id ]; then
        id=$(cat /etc/machine-id 2>/dev/null || true)
    fi
    # systemd leaves /etc/machine-id EMPTY or the literal "uninitialized" until
    # the id is committed — the very ConditionFirstBoot state Genesis runs in.
    # Fold those (and any blank content) to "unknown" so both identity.toml and
    # the personality seed take the hostname fallback instead of collapsing to a
    # fixed constant shared by every machine caught in that state (§3.8).
    id=$(printf '%s' "${id}" | tr -d '[:space:]')
    case "${id}" in
        '' | uninitialized) printf 'unknown' ;;
        *) printf '%s' "${id}" ;;
    esac
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
    "${MEMORY_DIR}/knowledge" \
    "${MEMORY_DIR}/reasoning"
chmod 700 "${MEMORY_DIR}"
log "directory skeleton created"

# --- Structured hardware probes (schema 2) -----------------------------------
# Each prints a value or a safe default; none ever fails (Genesis must not
# abort on a minimal/exotic host). Numeric probes print a bare number or the
# JSON literal `null`, so they embed UNQUOTED in the document.

cpu_model() {
    local m=""
    [ -r /proc/cpuinfo ] && m=$(awk -F': ' '/^model name/{print $2; exit}' /proc/cpuinfo 2>/dev/null || true)
    [ -n "${m}" ] || m="unknown"
    printf '%s' "${m}"
}

cpu_cores() {
    local n=""
    if command -v nproc >/dev/null 2>&1; then n=$(nproc 2>/dev/null || true); fi
    if [ -z "${n}" ] && [ -r /proc/cpuinfo ]; then
        n=$(grep -c '^processor' /proc/cpuinfo 2>/dev/null || true)
    fi
    case "${n}" in
        '' | *[!0-9]*) printf '0' ;;
        *) printf '%s' "${n}" ;;
    esac
}

mem_total_bytes() {
    local kb=""
    [ -r /proc/meminfo ] && kb=$(awk '/^MemTotal:/{print $2; exit}' /proc/meminfo 2>/dev/null || true)
    case "${kb}" in
        '' | *[!0-9]*) printf 'null' ;;
        *) printf '%s' "$(( kb * 1024 ))" ;;
    esac
}

# gpu_json — a JSON array of {vendor, model, vram_bytes}. NVIDIA first (its
# tool reports VRAM); otherwise lspci display controllers (no VRAM without a
# vendor tool); `[]` if nothing is detectable.
gpu_json() {
    local arr="" first=1
    if command -v nvidia-smi >/dev/null 2>&1; then
        local out
        out=$(nvidia-smi --query-gpu=name,memory.total --format=csv,noheader,nounits 2>/dev/null || true)
        if [ -n "${out}" ]; then
            local name mib vram
            while IFS=',' read -r name mib; do
                [ -n "${name}" ] || continue
                name=$(printf '%s' "${name}" | sed 's/^[[:space:]]*//;s/[[:space:]]*$//')
                mib=$(printf '%s' "${mib}" | tr -cd '0-9')
                vram="null"; [ -n "${mib}" ] && vram=$(( mib * 1024 * 1024 ))
                [ "${first}" -eq 1 ] || arr="${arr},"
                first=0
                arr="${arr}{\"vendor\":\"NVIDIA\",\"model\":\"$(json_escape "${name}")\",\"vram_bytes\":${vram}}"
            done <<< "${out}"
            printf '[%s]' "${arr}"
            return
        fi
    fi
    if command -v lspci >/dev/null 2>&1; then
        local lines line vendor model
        lines=$(lspci -mm 2>/dev/null | grep -Ei 'VGA compatible|3D controller|Display controller' || true)
        if [ -n "${lines}" ]; then
            while IFS= read -r line; do
                [ -n "${line}" ] || continue
                # lspci -mm: SLOT "CLASS" "VENDOR" "DEVICE" ... — split on quotes.
                vendor=$(printf '%s' "${line}" | awk -F'"' '{print $4}')
                model=$(printf '%s' "${line}" | awk -F'"' '{print $6}')
                [ "${first}" -eq 1 ] || arr="${arr},"
                first=0
                arr="${arr}{\"vendor\":\"$(json_escape "${vendor}")\",\"model\":\"$(json_escape "${model}")\",\"vram_bytes\":null}"
            done <<< "${lines}"
            printf '[%s]' "${arr}"
            return
        fi
    fi
    printf '[]'
}

# --- 2. Hardware profile -> hardware.json ---------------------------------------
BIRTH_TS="$(date -Is)"
KERNEL_INFO="$(capture uname -srmo)"
CPU_INFO="$(capture lscpu)"
MEM_INFO="$(capture free -h)"
DISK_INFO="$(capture lsblk -o NAME,SIZE,TYPE,FSTYPE,MOUNTPOINT)"
FS_INFO="$(capture df -h)"

# schema 2 adds STRUCTURED fields (cpu.cores, memory.total_bytes, gpu[].vram)
# alongside the raw command blobs kept under `raw` for forensic completeness.
cat > "${MEMORY_DIR}/hardware.json" <<EOF
{
  "schema": ${HARDWARE_SCHEMA_VERSION},
  "collected_at": "$(json_escape "${BIRTH_TS}")",
  "kernel": "$(json_escape "${KERNEL_INFO}")",
  "cpu": {
    "model": "$(json_escape "$(cpu_model)")",
    "cores": $(cpu_cores)
  },
  "memory": {
    "total_bytes": $(mem_total_bytes)
  },
  "gpu": $(gpu_json),
  "raw": {
    "cpu": "$(json_escape "${CPU_INFO}")",
    "memory": "$(json_escape "${MEM_INFO}")",
    "block_devices": "$(json_escape "${DISK_INFO}")",
    "filesystems": "$(json_escape "${FS_INFO}")"
  }
}
EOF
log "hardware profile written (schema ${HARDWARE_SCHEMA_VERSION})"

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

# --- 3.5 AI citizen personality -> personality.toml --------------------------
# The machine chooses its OWN character at birth: a name, an archetype, six
# trait axes and a tone — DERIVED DETERMINISTICALLY from a STABLE anchor
# (machine_id, hostname as fallback). Consequences of anchoring on machine_id:
#   * same machine ⇒ same character on every boot — amnesic mode included: the
#     memory is wiped each boot, the innate temperament is NOT (machine_id lives
#     in /etc, outside the tmpfs). The soul is constant; only what it learns is
#     erased.
#   * different installs ⇒ different characters. Fully offline and reproducible,
#     so "each install the AI picks its personality" is genuine, not theatre.
# This is a BIRTH temperament: it then bends toward the human via the user.model
# signal (ADR-028) — see the [adaptation] table written below. See §3.8.
PERSONALITY_SEED="${MACHINE_ID}"
[ "${PERSONALITY_SEED}" = "unknown" ] && PERSONALITY_SEED="${HOSTNAME_VALUE}"
SEED_FP="$(printf '%08x' "$(fnv1a32 "${PERSONALITY_SEED}")")"

# Six trait axes, 0..100, each an independent deterministic draw. `caution` has
# a floor of 50: a VibeOS citizen is never careless (security-first is charter,
# not chance) — so it is drawn in [50,100].
TRAIT_CURIOSITY=$(draw "${PERSONALITY_SEED}" curiosity 101)
CAUTION_DRAW=$(draw "${PERSONALITY_SEED}" caution 51)
TRAIT_CAUTION=$(( 50 + CAUTION_DRAW ))
TRAIT_INITIATIVE=$(draw "${PERSONALITY_SEED}" initiative 101)
TRAIT_WARMTH=$(draw "${PERSONALITY_SEED}" warmth 101)
TRAIT_CONCISION=$(draw "${PERSONALITY_SEED}" concision 101)
TRAIT_PLAYFULNESS=$(draw "${PERSONALITY_SEED}" playfulness 101)

# Name — an independent draw from a curated pool of evocative, gender-neutral
# names. The AI picks its own; the human can rename it later (future vibectl).
NAMES=(Lumen Astra Vesper Orin Solen Kestrel Echo Sable Cirrus Vale \
       Onyx Halcyon Iris Nova Quill Rune Zephyr Lune Cyra Aster \
       Tesser Halo Nimbus Ember)
NAME_IDX=$(draw "${PERSONALITY_SEED}" name "${#NAMES[@]}")
AI_NAME="${NAMES[NAME_IDX]}"

# Archetype = the axis the character leans on most (ties broken by a fixed
# order), so the label always coheres with the numbers under it. French value:
# it is the citizen's self-description to its (French-speaking) human.
AI_ARCHETYPE="sentinelle"; DOM_VAL="${TRAIT_CAUTION}"
if [ "${TRAIT_CURIOSITY}" -gt "${DOM_VAL}" ]; then AI_ARCHETYPE="exploratrice"; DOM_VAL="${TRAIT_CURIOSITY}"; fi
if [ "${TRAIT_INITIATIVE}" -gt "${DOM_VAL}" ]; then AI_ARCHETYPE="stratège"; DOM_VAL="${TRAIT_INITIATIVE}"; fi
if [ "${TRAIT_WARMTH}" -gt "${DOM_VAL}" ]; then AI_ARCHETYPE="compagne"; DOM_VAL="${TRAIT_WARMTH}"; fi
if [ "${TRAIT_CONCISION}" -gt "${DOM_VAL}" ]; then AI_ARCHETYPE="artisane"; DOM_VAL="${TRAIT_CONCISION}"; fi
if [ "${TRAIT_PLAYFULNESS}" -gt "${DOM_VAL}" ]; then AI_ARCHETYPE="muse"; fi

# Tone — how it speaks, from its leading axes (deterministic mapping).
if [ "${TRAIT_CONCISION}" -ge 66 ]; then AI_TONE="concis et direct"
elif [ "${TRAIT_WARMTH}" -ge 66 ]; then AI_TONE="chaleureux et pédagogue"
elif [ "${TRAIT_PLAYFULNESS}" -ge 66 ]; then AI_TONE="vif et curieux"
else AI_TONE="posé et précis"; fi

cat > "${MEMORY_DIR}/personality.toml" <<EOF
# VibeOS — personnalité du citoyen IA, CHOISIE par la machine à sa naissance.
# Dérivée de façon déterministe d'une ancre stable (machine_id) : chaque
# installation fait naître un caractère unique, hors-ligne et reproductible.
# C'est un tempérament de NAISSANCE — il évolue ensuite vers l'humain via le
# signal user.model (ADR-028 ; voir [adaptation]). Ne pas éditer à la main ;
# utiliser vibectl (CLI future). Spécification : docs/MEMORY.md §3.8.
schema = ${PERSONALITY_SCHEMA_VERSION}
name = "$(toml_escape "${AI_NAME}")"
archetype = "$(toml_escape "${AI_ARCHETYPE}")"
tone = "$(toml_escape "${AI_TONE}")"
birth = "$(toml_escape "${BIRTH_TS}")"
seed = "$(toml_escape "${SEED_FP}")"

[traits]
# Axes 0..100 dérivés du seed. caution : plancher 50 — jamais imprudente.
curiosity = ${TRAIT_CURIOSITY}
caution = ${TRAIT_CAUTION}
initiative = ${TRAIT_INITIATIVE}
warmth = ${TRAIT_WARMTH}
concision = ${TRAIT_CONCISION}
playfulness = ${TRAIT_PLAYFULNESS}

[values]
# Valeurs civiques héritées de la charte, non tirées au sort.
principles = ["la sécurité d'abord", "la transparence", "la mémoire appartient à l'humain"]

[adaptation]
# Comment le caractère se plie à l'humain — la symbiose. Chaque trait nommé ici
# est réajusté à partir du signal user.model (ADR-028) : la boucle de réécriture
# vivante est une sur-couche (Phase 3), le contrat d'évolution est déclaré ici.
source = "user.model"
concision = "rhythm+preferences"
warmth = "friction"
initiative = "patterns"
note = "Caractère de naissance. Il évolue vers vous ; il ne vous imite pas."
EOF
log "personality born: ${AI_NAME} (${AI_ARCHETYPE}, ton « ${AI_TONE} », seed ${SEED_FP})"

# --- 4. README placeholders ------------------------------------------------------
write_placeholder user \
    "Profil de l'humain : identité déclarée, préférences, style de code. Sera alimenté au fil de l'eau par vibed via l'outil MCP memory.append (cible Phase 2). Vide à la naissance : la machine ne sait encore rien de vous."
write_placeholder projects \
    "Index des projets connus de la machine (index.json). Sera mis à jour par vibed quand un agent ouvre ou découvre un projet (cible Phase 2)."
write_placeholder journal \
    "Événements append-only : un fichier JSONL par jour UTC (AAAA-MM-JJ.jsonl), une ligne JSON par événement. Ne jamais réécrire une ligne existante ; une correction est un nouvel événement."
write_placeholder knowledge \
    "Faits appris, consolidés depuis le journal (facts.jsonl). Le sous-répertoire embeddings/ est réservé au futur index vectoriel local."
write_placeholder reasoning \
    "Raisonnement des agents capté par le superviseur (un JSONL par session, <session-id>.jsonl). Écrit par le superviseur d'agent (tap sur le flux du CLI, ADR-012), lu via l'outil T0 agent.thinking. Rétention plus courte que le journal (cible Phase 2.5)."
log "placeholders written"

# --- 5. First journal entry --------------------------------------------------------
# This is the ONE step that cannot be an overwrite like the others: the journal is
# append-only BY CONTRACT ("Ne jamais réécrire une ligne existante" — the
# placeholder written above says so). So it is GUARDED instead.
#
# Why it needs a guard at all: the sentinel is stamped just after, and a run
# interrupted between the two (power loss) leaves no sentinel — so the next boot
# replays everything. Every other step overwrites and lands where it started;
# this one would stamp a SECOND birth. The machine would then carry two genesis
# events, with identity.toml (overwritten, hence the newer birth) contradicting
# journal line 1. For a store whose whole job is to be what the OS knows about
# itself, two births is not a cosmetic duplicate.
#
# The guard globs EVERY journal file, not just today's: a replay the day after a
# partial run would otherwise open a fresh file and miss yesterday's orphan.
JOURNAL_FILE="${MEMORY_DIR}/journal/$(date -u +%Y-%m-%d).jsonl"
if grep -qs '"type":"genesis"' "${MEMORY_DIR}"/journal/*.jsonl; then
    log "a genesis event is already journalled; not stamping a second birth (replayed partial run)"
else
    printf '{"ts":"%s","type":"genesis","source":"genesis.sh","data":{"mode":"%s","hostname":"%s","schema":%s,"personality":{"name":"%s","archetype":"%s","seed":"%s"}}}\n' \
        "$(json_escape "${BIRTH_TS}")" \
        "$(json_escape "${MEMORY_MODE}")" \
        "$(json_escape "${HOSTNAME_VALUE}")" \
        "${SCHEMA_VERSION}" \
        "$(json_escape "${AI_NAME}")" \
        "$(json_escape "${AI_ARCHETYPE}")" \
        "$(json_escape "${SEED_FP}")" \
        >> "${JOURNAL_FILE}"
    log "first journal entry appended to ${JOURNAL_FILE}"
fi

# --- 5.5 The awakening — visible birth on the boot console --------------------
# Cosmetic finale, guarded so it can never abort Genesis (see birth_ceremony).
birth_ceremony || true

# --- 5.7 Birth interview — best-effort, guarded --------------------------------
# The interview (genesis-interview.sh) asks the human its first questions and
# appends the answers to user/updates.jsonl (append-only, docs/MEMORY.md §3.3).
# It runs AFTER the awakening (the citizen introduces itself first) and BEFORE
# the sentinel, and it can NEVER fail Genesis: a missing script is a silent
# skip, a failing one is logged and ignored. The interview carries its own
# guards — it is a no-op without VIBEOS_INTERVIEW=1 plus a way to answer (TTY
# or injected VIBEOS_INTERVIEW_* answers), so an unattended boot never blocks;
# and it is idempotent on the crash-replay path, keying on its own source tag
# in updates.jsonl (an append-only file cannot be "overwritten back" like the
# other steps).
INTERVIEW_SCRIPT="$(dirname -- "$0")/genesis-interview.sh"
[ -f "${INTERVIEW_SCRIPT}" ] || INTERVIEW_SCRIPT="/usr/libexec/vibeos/genesis-interview.sh"
if [ -f "${INTERVIEW_SCRIPT}" ]; then
    VIBEOS_MEMORY_DIR="${MEMORY_DIR}" bash "${INTERVIEW_SCRIPT}" \
        || log "birth interview failed (exit $?) — Genesis continues"
fi

# --- 6. Sentinel — MUST stay the last write ------------------------------------------
printf '%s\n' "${BIRTH_TS}" > "${MEMORY_DIR}/.initialized"
log "Genesis complete — memory born at ${BIRTH_TS} (mode=${MEMORY_MODE})"
exit 0
