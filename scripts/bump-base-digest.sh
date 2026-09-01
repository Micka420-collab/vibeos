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
# PLUSIEURS SITES D'ÉPINGLAGE
# Le digest n'est pas épinglé QUE dans le Containerfile : `image-info.json`, la
# carte d'identité de l'image lue à l'exécution, porte le même `base_image`. Ce
# script ne réécrivait que le Containerfile — le JSON dérivait donc en silence à
# chaque bump, et annonçait au système une base qui n'était plus celle construite.
# On réécrit désormais TOUS les sites connus (liste ci-dessous), et
# check-base-digest-sync.py fait rougir la CI si un nouveau site apparaît sans
# être ajouté ici.
#
# TOUT OU RIEN
# Les sites sont vérifiés AVANT la première écriture, et chaque écriture est
# relue après coup. Sans ça deux pannes réelles restaient possibles : un site
# manquant laissait le Containerfile DÉJÀ réécrit (arbre à moitié bumpé), et un
# site qui aurait dérivé sur un TROISIÈME digest ne matchait pas le `sed` — le
# script annonçait alors « changed » en laissant ce fichier périmé, et le commit
# d'autobump racontait un bump complet qui n'avait pas eu lieu.
#
# USAGE : bump-base-digest.sh <containerfile>
# SORTIE : imprime "changed <old> -> <new>" et rc 0 si bumpé ; "unchanged" et rc 0
#          si déjà à jour ; rc != 0 sur erreur (digest introuvable, pas multi-arch,
#          site manquant, ou site non réaligné après écriture).
set -euo pipefail

CF="${1:?usage: bump-base-digest.sh <containerfile>}"
REPO="quay.io/fedora/fedora-kinoite"
TAG="44"

# Racine du dépôt déduite de l'emplacement du script : les sites additionnels
# résolvent quel que soit le cwd de l'appelant (la CI appelle depuis la racine,
# un humain depuis n'importe où). `BASH_SOURCE` n'est PAS déréférencé : appelé
# via un lien symbolique, ROOT serait faux — le contrôle « tout ou rien »
# ci-dessous fait alors échouer AVANT toute écriture, au lieu de laisser un
# arbre à moitié bumpé.
ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
EXTRA_PIN_SITES=("$ROOT/os/rootfs/usr/lib/vibeos/image-info.json")

command -v skopeo >/dev/null || {
	echo "erreur: skopeo requis" >&2
	exit 2
}
[ -f "$CF" ] || {
	echo "erreur: introuvable: $CF" >&2
	exit 2
}

# APRÈS LA BASCULE ADR-031, ce script n'a plus d'objet : la base est miroitée
# dans NOTRE registre, et un digest qu'on héberge n'est jamais purgé par un
# tiers. Sortir en SUCCÈS (et non en erreur « aucun pin trouvé ») est délibéré :
# l'auto-bump tourne en cron quotidien, et le faire échouer à perpétuité
# rallumerait précisément le bruit rouge permanent qui a déjà fait ignorer une
# panne de 19 jours. Un job vert et muet est le bon comportement pour une corvée
# devenue sans objet.
if grep -qE 'ghcr\.io/[^:[:space:]"]+/vibeos-base:[0-9]+@sha256:[0-9a-f]{64}' "$CF"; then
	echo "unchanged (base miroitée — ADR-031 : plus de purge amont possible, rien à bumper)"
	exit 0
fi

# Digest épinglé actuellement dans le Containerfile (1re occurrence).
# `if !` : sous `set -e`, un `x="$(pipeline)"` qui échoue quitterait AVANT le
# garde `[ -n ]` — le message d'erreur serait mort. On teste donc l'assignation.
if ! pinned="$(grep -oE "fedora-kinoite:${TAG}@sha256:[0-9a-f]{64}" "$CF" | head -n1 | grep -oE 'sha256:[0-9a-f]{64}')" || [ -z "$pinned" ]; then
	echo "erreur: aucun digest épinglé trouvé dans $CF" >&2
	exit 3
fi

# On ne bumpe QUE sur PURGE réelle. La base f44 est republiée chaque nuit (le tag
# bouge tous les jours), mais l'ancien digest épinglé reste RÉSOLVABLE jusqu'à ce
# que quay le purge. Bumper à chaque dérive de tag ouvrirait une PR + un build
# COMPLET quotidiens pour rien. Donc : si le pin résout encore, on ne touche à rien.
if skopeo inspect --raw "docker://${REPO}@${pinned}" >/dev/null 2>&1; then
	echo "unchanged (${pinned} résout encore)"
	exit 0
fi

# Le pin ne résout plus (purgé) → résoudre le digest courant du tag.
if ! current="$(skopeo inspect --no-tags "docker://${REPO}:${TAG}" --format '{{.Digest}}')" || [ -z "$current" ]; then
	echo "erreur: impossible de résoudre ${REPO}:${TAG}" >&2
	exit 4
fi

# Valider le format AVANT tout splice dans sed (anti-corruption : un sha512 ou une
# valeur inattendue produirait un pin corrompu).
if ! [[ "$current" =~ ^sha256:[0-9a-f]{64}$ ]]; then
	echo "erreur: digest courant au format inattendu: $current" >&2
	exit 4
fi

if [ "$pinned" = "$current" ]; then
	# Défensif : le pin ne résolvait pas mais le tag pointe le même digest.
	echo "unchanged ($pinned)"
	exit 0
fi

# Sécurité : n'accepter QUE si le nouveau digest est une manifest-list multi-arch
# (amd64+arm64). Un digest mono-arch casserait le build de l'autre arch. On grep
# avec tolérance aux espaces (le JSON brut de skopeo est indenté).
if ! raw="$(skopeo inspect --raw "docker://${REPO}@${current}")"; then
	echo "erreur: impossible d'inspecter le manifeste ${REPO}@${current}" >&2
	exit 5
fi
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

# PRÉ-VOL : tous les sites existent-ils ? Un site absent du disque est une
# ERREUR, pas un silence — la liste ci-dessus est le contrat, et un fichier
# renommé doit casser bruyamment. On vérifie AVANT la première écriture : sinon
# le Containerfile serait déjà réécrit quand on découvre le site manquant, et
# l'arbre resterait à moitié bumpé (le cas se produit aussi quand ROOT est faux,
# p. ex. un appel du script via un lien symbolique).
for site in "${EXTRA_PIN_SITES[@]}"; do
	[ -f "$site" ] || {
		echo "erreur: site d'épinglage introuvable: $site (liste EXTRA_PIN_SITES à corriger)" >&2
		exit 7
	}
done

# Réécriture EN DEUX TEMPS, pour que « tout ou rien » soit vrai et pas seulement
# annoncé : on produit d'abord chaque version réécrite À CÔTÉ, on les VALIDE
# toutes, et on ne touche aux fichiers du dépôt qu'ensuite.
#
# La validation attrape le cas où un site aurait dérivé sur un TROISIÈME digest
# (le bug historique : le JSON restait sur un pin périmé pendant que le
# Containerfile avançait). Ce site ne matche pas `$pinned`, donc rien n'y est
# réécrit — sans ce contrôle le script rendrait « changed » en laissant un
# fichier désaligné, et le commit d'autobump raconterait un bump qui n'a pas eu
# lieu. Avec l'écriture différée, il ne laisse pas non plus un arbre à moitié
# bumpé derrière lui.
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT

sites=("$CF" "${EXTRA_PIN_SITES[@]}")
i=0
for site in "${sites[@]}"; do
	sed "s|${pinned}|${current}|g" "$site" >"$STAGE/$i"
	grep -q -- "$current" "$STAGE/$i" || {
		echo "erreur: $site ne porterait pas ${current} après réécriture (digest divergent ?) — rien n'a été modifié" >&2
		exit 8
	}
	i=$((i + 1))
done

# Tout est validé : on publie. `cat >` (et non `mv`) pour garder le mode, le
# propriétaire et l'inode d'origine des fichiers du dépôt.
i=0
for site in "${sites[@]}"; do
	cat "$STAGE/$i" >"$site"
	i=$((i + 1))
done
echo "changed ${pinned} -> ${current}"
