#!/usr/bin/env bash
# check-base-digest-fresh.sh — le digest de base épinglé résout-il ENCORE ?
#
# POURQUOI
#
# La base est passée de Fedora 42 (EOL, donc gelée, digest stable pour toujours)
# à Fedora 44 (VIVANTE, rebâtie ~chaque nuit). Conséquence non anticipée : quay
# PURGE les anciens index. Le digest qu'on épingle dans os/Containerfile cesse de
# résoudre — « manifest unknown » — au bout d'un ou deux jours, et tout build
# casse d'un coup. C'est arrivé les 2026-07-16 ET 2026-07-17, deux jours de suite.
#
# La CI sur push ne peut rien : la purge arrive SANS push, upstream. Quand
# quelqu'un pousse enfin, le build échoue déjà avec un message clair — trop tard.
# Il faut un contrôle PROGRAMMÉ (cron), indépendant des push, qui détecte la
# dérive et la rende visible AVANT qu'un push ne la découvre.
#
# CE QU'IL FAIT
#
# Extrait le digest `quay.io/fedora/fedora-kinoite:NN@sha256:...` d'os/Containerfile
# et vérifie qu'il résout encore (skopeo). S'il ne résout plus, il ÉCHOUE en
# affichant le digest courant de `:NN` — le remède est alors un simple bump.
#
# LECTURE SEULE. Il n'écrit rien, ne bumpe rien : il notifie. L'auto-bump
# (Renovate, ou une PR automatique) exigerait des permissions d'écriture — une
# décision opérateur, hors de ce script.
#
# USAGE : bash scripts/check-base-digest-fresh.sh [os/Containerfile]
#   0 = le digest épinglé résout encore
#   1 = il a été purgé (ou skopeo indisponible / Containerfile illisible)
set -euo pipefail

CONTAINERFILE="${1:-os/Containerfile}"

die() { printf '\033[31mFAIL\033[0m  %s\n' "$1" >&2; exit 1; }
ok()  { printf '\033[32mok\033[0m    %s\n' "$1"; }

command -v skopeo >/dev/null 2>&1 \
  || die "skopeo absent — ce contrôle ne peut pas résoudre le digest. Installe skopeo (ce n'est PAS un feu vert : sans lui, le contrôle est aveugle)."

[[ -r "$CONTAINERFILE" ]] || die "cannot read ${CONTAINERFILE}"

# Le digest de la base qui SHIP (ligne fedora-kinoite, pas les builders fedora:NN).
ref="$(grep -oE 'quay\.io/fedora/fedora-kinoite:[0-9]+@sha256:[a-f0-9]{64}' "$CONTAINERFILE" | head -1)"
[[ -n "$ref" ]] \
  || die "aucun pin fedora-kinoite:NN@sha256:… trouvé dans ${CONTAINERFILE} — ce contrôle vient de devenir aveugle. Répare le motif, ne supprime pas le contrôle."

tag="${ref%@*}"          # quay.io/fedora/fedora-kinoite:44
pinned="${ref##*@}"      # sha256:...
repo="${tag%:*}"         # quay.io/fedora/fedora-kinoite (sans le tag)

# On résout PAR DIGEST : repo@sha256:… (jamais tag+digest ensemble, que skopeo
# rejette). Si le digest a été purgé, l'inspection échoue.
if skopeo inspect --raw "docker://${repo}@${pinned}" >/dev/null 2>&1; then
  ok "le digest épinglé résout encore : ${ref}"
  exit 0
fi

current="$(skopeo inspect --format '{{.Digest}}' "docker://${tag}" 2>/dev/null || echo '?')"
die "le digest épinglé NE RÉSOUT PLUS (purgé par quay) : ${pinned}
        ${tag} pointe désormais sur : ${current}
        Remède : bumper le digest dans ${CONTAINERFILE} (2 occurrences).
        Ré-résoudre : skopeo inspect --format '{{.Digest}}' docker://${tag}"
