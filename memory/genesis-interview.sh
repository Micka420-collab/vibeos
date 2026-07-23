#!/usr/bin/env bash
#
# genesis-interview.sh — VibeOS birth interview (minimal, scripted-testable).
#
# Asks the human its first questions at birth — name, preferred language, work
# domain, desired collaboration style — and records the answers in the HUMAN
# profile store as append-only JSONL lines:
#
#     ${MEMORY_DIR}/user/updates.jsonl        {ts, key, value, source}
#
# the same shape `memory.append` (scope user) writes; the current view is the
# last-write-wins fold per key (docs/MEMORY.md §3.3). Keys written:
#
#     profile.name  profile.lang  profile.domain  preferences.collaboration
#
# all tagged source:"genesis-interview".
#
# Contracts — every non-answering path is a silent, successful no-op, because
# the first boot must NEVER block on a question:
#   * Opt-in only: without VIBEOS_INTERVIEW=1, exit 0 immediately, write NOTHING.
#   * A way to answer is required: stdin must be a TTY (interactive prompt), OR
#     at least one VIBEOS_INTERVIEW_NAME/_LANG/_DOMAIN/_COLLAB variable must be
#     set (scripted mode — how CI drives this script without a TTY). Neither:
#     exit 0, nothing written.
#   * One interview per life: if user/updates.jsonl already carries a line with
#     source "genesis-interview", exit 0 without writing. The file is
#     append-only by contract, so the crash-replay of genesis.sh (sentinel
#     absent) must not record the answers a second time.
#   * Answers are JSON-escaped before embedding (json_escape below), so a
#     hostile answer cannot forge extra JSONL lines or extra keys — the same
#     A1-poisoning concern the escapers in genesis.sh close.
#
# Environment:
#   VIBEOS_MEMORY_DIR        target memory directory
#                            (default: /var/lib/vibeos/memory, like genesis.sh)
#   VIBEOS_INTERVIEW         "1" enables the interview (anything else: no-op)
#   VIBEOS_INTERVIEW_NAME    scripted answer -> key profile.name
#   VIBEOS_INTERVIEW_LANG    scripted answer -> key profile.lang
#   VIBEOS_INTERVIEW_DOMAIN  scripted answer -> key profile.domain
#   VIBEOS_INTERVIEW_COLLAB  scripted answer -> key preferences.collaboration
#
# Exit code: 0 on every normal path (skipped, already recorded, recorded).
# Called best-effort by memory/genesis.sh after the awakening and before the
# .initialized sentinel; also runnable standalone (it mkdir -p's user/).

set -euo pipefail

readonly MEMORY_DIR="${VIBEOS_MEMORY_DIR:-/var/lib/vibeos/memory}"
readonly UPDATES_FILE="${MEMORY_DIR}/user/updates.jsonl"
readonly SOURCE_TAG="genesis-interview"

# || true: stderr may be a terminal that died mid-interview (hangup); a
# failing log line must never turn a recorded interview into a failed run.
log() { printf 'vibeos-genesis-interview: %s\n' "$*" >&2 || true; }

# Fixed PATH — same standard, and same reason, as memory/genesis.sh: this runs
# as root at first boot and must never resolve a tool through a mutable PATH.
PATH=/usr/sbin:/usr/bin:/sbin:/bin
export PATH

# --- Gate 1: explicit opt-in --------------------------------------------------
[ "${VIBEOS_INTERVIEW:-0}" = "1" ] || exit 0

# --- Gate 2: a way to answer --------------------------------------------------
# Scripted mode (any injected answer variable set) needs no TTY. Otherwise a
# real TTY on stdin is required; an unattended boot with neither exits 0
# without writing anything.
SCRIPTED=0
if [ -n "${VIBEOS_INTERVIEW_NAME:-}${VIBEOS_INTERVIEW_LANG:-}${VIBEOS_INTERVIEW_DOMAIN:-}${VIBEOS_INTERVIEW_COLLAB:-}" ]; then
    SCRIPTED=1
elif [ ! -t 0 ]; then
    exit 0
fi

# --- Gate 3: one interview per life -------------------------------------------
# -s: a missing updates.jsonl (the normal first-boot case) is not an error.
# -F: fixed-string match on the exact "source" field this script writes, so
# agent-appended lines carrying other sources never trip the guard.
if grep -qsF "\"source\":\"${SOURCE_TAG}\"" "${UPDATES_FILE}"; then
    log "interview already recorded in ${UPDATES_FILE} — nothing to do"
    exit 0
fi

# Everything recorded here is private to the machine and its human.
umask 077

# --- Helpers ------------------------------------------------------------------

# json_escape STRING — escape STRING for embedding as a JSON string value.
# DUPLICATED from memory/genesis.sh (keep the two in sync by hand): factoring
# one nine-line helper into a shared sourced file would mean shipping and
# pinning a third script in the image — not worth the extra moving part.
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

# trim STRING — strip leading/trailing whitespace (a lone space is no answer).
trim() {
    local s=$1
    s="${s#"${s%%[![:space:]]*}"}"
    s="${s%"${s##*[![:space:]]}"}"
    printf '%s' "$s"
}

# --- Interview ----------------------------------------------------------------
ANSWER_NAME=""
ANSWER_LANG=""
ANSWER_DOMAIN=""
ANSWER_COLLAB=""

if [ "${SCRIPTED}" -eq 1 ]; then
    ANSWER_NAME="$(trim "${VIBEOS_INTERVIEW_NAME:-}")"
    ANSWER_LANG="$(trim "${VIBEOS_INTERVIEW_LANG:-}")"
    ANSWER_DOMAIN="$(trim "${VIBEOS_INTERVIEW_DOMAIN:-}")"
    ANSWER_COLLAB="$(trim "${VIBEOS_INTERVIEW_COLLAB:-}")"
else
    # Same aesthetic — and same colour gating — as the awakening frame in
    # genesis.sh (birth_ceremony): colour only on a real, non-dumb terminal
    # with NO_COLOR unset; prompts on stderr like every message of Genesis.
    hl='' dim='' glow='' reset=''
    if [ -t 2 ] && [ -z "${NO_COLOR:-}" ] && [ "${TERM:-dumb}" != dumb ]; then
        hl=$'\033[38;5;51m'   # bright cyan — frame
        glow=$'\033[38;5;45m' # cyan — questions & farewell
        dim=$'\033[2m'        # dim — hints
        reset=$'\033[0m'
    fi
    # Console writes in this branch are best-effort (|| true): if the terminal
    # vanishes mid-interview (hangup), a failing prompt printf must not abort
    # the run under set -e — the answers already given still get recorded
    # below instead of being lost.
    {
        printf '\n'
        printf '%s   ╔════════════════════════════════════════════╗%s\n' "${hl}" "${reset}"
        printf '%s   ║   V I B E O S   ·   R E N C O N T R E      ║%s\n' "${hl}" "${reset}"
        printf '%s   ╚════════════════════════════════════════════╝%s\n' "${hl}" "${reset}"
        printf '%s        je viens de naître — et vous ?%s\n' "${dim}" "${reset}"
        printf '%s        (quatre questions ; Entrée pour passer)%s\n\n' "${dim}" "${reset}"
    } >&2 || true

    # ask QUESTION — one question on stderr, one line read from stdin (the
    # TTY). EOF (Ctrl-D) counts as "no answer" and must not abort under set -e.
    ask() {
        local answer=''
        printf '        %s%s%s\n        %s❯%s ' "${glow}" "$1" "${reset}" "${dim}" "${reset}" >&2 || true
        IFS= read -r answer || answer=''
        printf '\n' >&2 || true
        trim "${answer}"
    }

    ANSWER_NAME="$(ask "Votre nom, ou prénom d'usage ?")"
    ANSWER_LANG="$(ask "Votre langue préférée ? (ex. français)")"
    ANSWER_DOMAIN="$(ask "Votre domaine de travail ? (ex. développement, musique…)")"
    ANSWER_COLLAB="$(ask "Quel style de collaboration souhaitez-vous ? (ex. concis et direct)")"

    printf '        %sMerci. Je m'\''en souviendrai.%s\n\n' "${glow}" "${reset}" >&2 || true
fi

# --- Record — append-only, single pass ----------------------------------------
TS="$(date -Is)"

# The payload is composed fully, then appended in ONE printf — a single
# write(2) for these short lines — to minimise (not eliminate) the window
# where an interrupted run records a partial interview. Gate 3 keys on ANY
# recorded line, so the worst case pins a partial answer set; it never
# duplicates one. Keys are fixed literals in the dotted-key charset
# [A-Za-z0-9._-]; only the values are attacker-influenced, and every value
# goes through json_escape.
PAYLOAD=""
emit() {
    local key=$1 value=$2
    [ -n "${value}" ] || return 0   # an unanswered question records nothing
    PAYLOAD="${PAYLOAD}{\"ts\":\"$(json_escape "${TS}")\",\"key\":\"${key}\",\"value\":\"$(json_escape "${value}")\",\"source\":\"${SOURCE_TAG}\"}"$'\n'
}
emit profile.name "${ANSWER_NAME}"
emit profile.lang "${ANSWER_LANG}"
emit profile.domain "${ANSWER_DOMAIN}"
emit preferences.collaboration "${ANSWER_COLLAB}"

if [ -z "${PAYLOAD}" ]; then
    log "no answer given — nothing recorded"
    exit 0
fi

mkdir -p "${MEMORY_DIR}/user"
printf '%s' "${PAYLOAD}" >> "${UPDATES_FILE}"
log "answers recorded in ${UPDATES_FILE} (source ${SOURCE_TAG})"
exit 0
