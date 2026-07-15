#!/usr/bin/env bash
# =============================================================================
# check-base-eol.sh — refuse to ship on an end-of-life Fedora base.
#
# WHY THIS EXISTS
#
# On 2026-07-15 VibeOS was found to be building on Fedora Kinoite 42, which went
# EOL on 2026-05-27 — 49 days of zero security updates. Nothing broke. Nothing
# warned. The CI was green the whole time.
#
# That is not an oversight, it is the designed behaviour of Fedora's mirror
# system: when a release goes EOL, MirrorManager keeps answering the metalink
# and transparently reroutes clients to the ARCHIVE mirrors. `dnf install`
# returns 0 and installs packages frozen at the EOL date. Verified at the time:
#
#   updates-released-f42 metalink -> 200, 11 mirrors, all fedora-archive.*
#   f42 repomd.xml Last-Modified: Wed, 27 May 2026 16:25:54 GMT   (frozen)
#   f44 repomd.xml Last-Modified: Wed, 15 Jul 2026 01:00:29 GMT   (live)
#
# So the build cannot fail on its own, and the image silently ossifies. The only
# way this gets caught is if something asserts it. That is this script.
#
# DESIGN — why a pinned table and not a live lookup
#
# Querying Fedora's schedule at CI time would be self-updating, and tempting.
# It is rejected for three reasons:
#   1. docs.fedoraproject.org sits behind Anubis anti-scraper. A fetch from CI
#      returns an access-denied PAGE, not an error — a naive parser reads "no
#      EOL found" and passes. A guard that fails OPEN is worse than no guard.
#   2. It would make every build depend on a third party being up.
#   3. Fedora's own schedule annotates these dates "changeable" — so a live
#      value is not more authoritative than a reviewed one, just less visible.
#
# A pinned table is deterministic, offline, and — the actual point — it forces a
# human to type the new date in a diff when they rebase. The date becomes a
# reviewable claim instead of an assumption.
#
# It fails BEFORE the EOL, not after: a rebase takes longer than a day.
# =============================================================================
set -euo pipefail

CONTAINERFILE="${1:-os/Containerfile}"
# Days of runway required. Below this the build goes red, deliberately: the
# lesson of 2026-07-15 is that anything short of a red build gets missed.
GRACE_DAYS="${GRACE_DAYS:-30}"

# Fedora release -> EOL date (ISO). Sourced from the Fedora Docs EOL table and
# the per-release schedules. Fedora annotates future dates as "changeable"
# (F42's EOL slipped 2026-05-13 -> 2026-05-27 when F44 slipped), so treat
# unreleased entries as estimates and re-check when you bump.
#   https://docs.fedoraproject.org/en-US/releases/eol/
#   https://fedorapeople.org/groups/schedule/f-44/f-44-key-tasks.html
# Verified 2026-07-15.
declare -A FEDORA_EOL=(
  [40]="2025-05-13"
  [41]="2025-12-15"
  [42]="2026-05-27"  # EOL. This is the one that shipped unnoticed for 49 days.
  [43]="2026-12-09"  # projected
  [44]="2027-06-02"  # projected
)

die() { printf '\033[31mFAIL\033[0m  %s\n' "$1" >&2; exit 1; }
ok()  { printf '\033[32mok\033[0m    %s\n' "$1"; }

[[ -r "$CONTAINERFILE" ]] || die "cannot read ${CONTAINERFILE}"

# The base that SHIPS is the last FROM that is not a named build stage we own:
# the fedora-kinoite line. Builders (FROM fedora:NN AS x) are compile-time only
# and never reach the machine — but a stale builder is still a supply-chain
# smell, so both are checked, with the shipped base being the hard failure.
mapfile -t base_versions < <(
  grep -oE 'quay\.io/fedora/fedora-(kinoite|bootc|silverblue):[0-9]+' "$CONTAINERFILE" \
    | grep -oE '[0-9]+$' | sort -u
)
mapfile -t builder_versions < <(
  grep -oE 'registry\.fedoraproject\.org/fedora:[0-9]+' "$CONTAINERFILE" \
    | grep -oE '[0-9]+$' | sort -u
)

[[ ${#base_versions[@]} -gt 0 ]] \
  || die "no fedora-kinoite/bootc/silverblue base found in ${CONTAINERFILE} — this guard just went blind, which is exactly the failure it exists to prevent. Fix the pattern, do not delete the check."

today_epoch=$(date -u +%s)

check_one() {
  local ver="$1" kind="$2" eol eol_epoch days
  eol="${FEDORA_EOL[$ver]:-}"
  # Unknown release => FAIL. Fail-closed: a base we have no EOL data for is a
  # base nobody reviewed. Add the row (with its source) and move on.
  [[ -n "$eol" ]] \
    || die "${kind} pins Fedora ${ver}, which has no row in FEDORA_EOL. Add it from https://docs.fedoraproject.org/en-US/releases/eol/ — an unreviewed base is how this bug happened."

  eol_epoch=$(date -u -d "$eol" +%s)
  days=$(( (eol_epoch - today_epoch) / 86400 ))

  if (( days < 0 )); then
    die "${kind} pins Fedora ${ver}, EOL since ${eol} (${days#-} days ago). It receives NO security updates, and dnf will NOT tell you: MirrorManager reroutes to the archive and returns 0. Rebase to a supported release."
  elif (( days < GRACE_DAYS )); then
    die "${kind} pins Fedora ${ver}, EOL ${eol} — only ${days} days left (grace: ${GRACE_DAYS}). Rebase now, while it is still a choice."
  fi
  ok "${kind}: Fedora ${ver}, EOL ${eol} (${days} days of runway)"
}

for v in "${base_versions[@]}";    do check_one "$v" "shipped base"; done
for v in "${builder_versions[@]}"; do check_one "$v" "build stage";  done

ok "base is supported"
