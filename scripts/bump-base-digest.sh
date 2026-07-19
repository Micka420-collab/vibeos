#!/usr/bin/env bash
# bump-base-digest.sh — résout le digest courant de la base F44 et met à jour le pin.
#
# POURQUOI
# La base quay.io/fedora/fedora-kinoite:44 est VIVANTE : quay purge les anciens
# index ~chaque jour, et le digest épinglé dans os/Containerfile cesse de résoudre
# (« manifest unknown ») → tout build casse. check-base-digest-fresh.sh le DÉTECTE ;
# ce script APPLIQUE le correctif (résout le digest courant, réécrit le pin), pour
# que le workflow d'auto-bump ouvre une PR au lieu de laisser un humain le faire à
# la main. On épingle toujours le digest quay COURANT — pas de changement de la
# source de confiance, juste l'automatisation d'une corvée.
#
# USAGE : bump-base-digest.sh <containerfile>
# SORTIE : imprime "changed <old> -> <new>" et rc 0 si bumpé ; "unchanged" et rc 0
#          si déjà à jour ; rc != 0 sur erreur (digest introuvable, pas multi-arch).
set -euo pipefail

CF="${1:?usage: bump-base-digest.sh <containerfile>}"
REPO="quay.io/fedora/fedora-kinoite"
TAG="44"

command -v skopeo >/dev/null || {
	echo "erreur: skopeo requis" >&2
	exit 2
}
[ -f "$CF" ] || {
	echo "erreur: introuvable: $CF" >&2
	exit 2
}

# Digest épinglé actuellement dans le Containerfile (1re occurrence).
pinned="$(grep -oE "fedora-kinoite:${TAG}@sha256:[0-9a-f]{64}" "$CF" | head -n1 | grep -oE 'sha256:[0-9a-f]{64}')"
[ -n "$pinned" ] || {
	echo "erreur: aucun digest épinglé trouvé dans $CF" >&2
	exit 3
}

# Digest courant de la manifest-list du tag.
current="$(skopeo inspect --no-tags "docker://${REPO}:${TAG}" --format '{{.Digest}}')"
[ -n "$current" ] || {
	echo "erreur: impossible de résoudre ${REPO}:${TAG}" >&2
	exit 4
}

if [ "$pinned" = "$current" ]; then
	echo "unchanged ($pinned)"
	exit 0
fi

# Sécurité : n'accepter QUE si le nouveau digest est une manifest-list multi-arch
# (amd64+arm64). Un digest mono-arch casserait le build de l'autre arch. On grep
# avec tolérance aux espaces (le JSON brut de skopeo est indenté).
raw="$(skopeo inspect --raw "docker://${REPO}@${current}")"
if ! printf '%s' "$raw" | grep -qE '"mediaType"[[:space:]]*:[[:space:]]*"[^"]*(image\.index|manifest\.list)'; then
	echo "erreur: le digest courant n'est pas une manifest-list multi-arch — abandon" >&2
	exit 5
fi
for arch in amd64 arm64; do
	if ! printf '%s' "$raw" | grep -qE "\"architecture\"[[:space:]]*:[[:space:]]*\"$arch\""; then
		echo "erreur: arch $arch absente de la manifest-list courante — abandon" >&2
		exit 6
	fi
done

# Réécrit toutes les occurrences (FROM + label).
sed -i "s|${pinned}|${current}|g" "$CF"
echo "changed ${pinned} -> ${current}"
