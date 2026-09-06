#!/usr/bin/env bash
# mirror-base.sh — miroiter la base Fedora vers NOTRE registre, et y épingler.
#
# POURQUOI (la panne que ce script supprime)
#
# `quay.io/fedora/fedora-kinoite:44` est une base VIVANTE : Fedora la republie
# chaque nuit et quay PURGE les anciens index. Mesuré sur ce dépôt : un digest
# épinglé cesse de résoudre en ~24 h. Conséquence, tout build casse d'un coup —
# arrivé les 16, 17, 19, 20 juillet, puis du 2026-07-26 au 2026-08-14 (19 jours),
# puis ENCORE le 2026-08-15 sur un pin posé la veille.
#
# La leçon de ce dernier cycle est qu'aucun BUMP ne résout le problème : un pin
# qui vit 24 h n'a de valeur que s'il est mergé ET buildé dans la journée. On ne
# répare pas, on court après. `bump-base-digest.sh` + l'auto-bump sont un
# tapis roulant, pas une sortie.
#
# La sortie est d'héberger la base nous-mêmes : un digest que NOUS poussons dans
# NOTRE registre n'est jamais purgé par un tiers. Le pin redevient ce qu'il aurait
# toujours dû être — stable jusqu'à ce qu'on décide de le changer.
#
# CE QUE ÇA CHANGE À LA POSTURE SUPPLY-CHAIN (lire avant de juger)
#
# On ne change PAS la source de confiance : l'image miroitée est l'image Fedora,
# copiée telle quelle. On change QUI LA CONSERVE. Deux propriétés le garantissent :
#
#  1. `skopeo copy --all` recopie le manifeste OCTET POUR OCTET (toutes les
#     arches). Les digests étant adressés par contenu, le digest du miroir est
#     normalement IDENTIQUE à celui de l'amont — la provenance devient une preuve
#     cryptographique, pas une affirmation. Le script le VÉRIFIE au lieu de le
#     supposer, et le dit fort si ça diverge (cas légitime : conversion de format
#     par le registre de destination).
#  2. La provenance est écrite dans `os/base-provenance.json` : ref amont, digest
#     amont, ref miroir, digest miroir, date. Diffable, auditable, versionnée.
#
# Miroiter n'est donc pas « faire confiance à ghcr plutôt qu'à Fedora » : c'est
# garder une copie de ce que Fedora a publié, et pouvoir le prouver.
#
# CE SCRIPT N'EST PAS UN AUTO-UPGRADE. Le lancer adopte le contenu courant de
# `:44` comme nouvelle base — c'est une MONTÉE DE VERSION délibérée, revue en PR,
# pas une corvée nocturne. C'est tout l'intérêt : on cesse de subir le calendrier
# de purge de quay pour reprendre celui du projet.
#
# USAGE : mirror-base.sh <containerfile>
#   MIRROR_BASE_DRY_RUN=1  résout, vérifie et réécrit les pins SANS pousser
#                          (aucun credential requis — utilisé par les tests)
# ENV   : UPSTREAM_REPO (défaut quay.io/fedora/fedora-kinoite)
#         UPSTREAM_TAG  (défaut 44)
#         MIRROR_REPO   (défaut ghcr.io/micka420-collab/vibeos-base)
# SORTIE: "mirrored <upstream-digest> -> <mirror-ref>" et rc 0 ; "unchanged" si le
#         pin porte déjà ce digest miroité ; rc != 0 sur erreur.
#
# PRÉREQUIS : skopeo, et (hors dry-run) une session `skopeo login` sur le registre
# du miroir avec droit d'écriture.
set -euo pipefail

CF="${1:?usage: mirror-base.sh <containerfile>}"
UPSTREAM_REPO="${UPSTREAM_REPO:-quay.io/fedora/fedora-kinoite}"
UPSTREAM_TAG="${UPSTREAM_TAG:-44}"
MIRROR_REPO="${MIRROR_REPO:-ghcr.io/micka420-collab/vibeos-base}"
DRY_RUN="${MIRROR_BASE_DRY_RUN:-0}"

# Motifs de recherche ÉCHAPPÉS. Les noms de registre contiennent des `.`, qui en
# regex matchent n'importe quel caractère : sans échappement, `quay.io` matcherait
# `quayXio`. Anecdotique ici, mais un motif de recherche approximatif sur la
# chaîne d'approvisionnement n'a aucune raison d'exister. Le `#` sert de
# délimiteur sed plus bas — pas `|`, qui entrerait en collision avec l'alternance.
UPSTREAM_RE="${UPSTREAM_REPO//./\\.}"
MIRROR_RE="${MIRROR_REPO//./\\.}"

# Racine déduite de l'emplacement du script (pas de `readlink` : appelé via un
# lien symbolique, ROOT serait faux — le pré-vol « tout ou rien » plus bas fait
# alors échouer AVANT toute écriture plutôt que de laisser un arbre à moitié
# réécrit). Même discipline que bump-base-digest.sh.
ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
EXTRA_PIN_SITES=("$ROOT/os/rootfs/usr/lib/vibeos/image-info.json")
PROVENANCE="$ROOT/os/base-provenance.json"

command -v skopeo >/dev/null || {
	echo "erreur: skopeo requis" >&2
	exit 2
}
[ -f "$CF" ] || {
	echo "erreur: introuvable: $CF" >&2
	exit 2
}

# --- 1. Le pin actuel, quelle que soit sa forme --------------------------------
# Pendant la migration le dépôt épingle encore l'AMONT ; après, le MIROIR. Les
# deux formes doivent être reconnues, sinon le premier passage ne trouve rien et
# le second réécrit à côté.
if ! pinned_ref="$(grep -oE "(${UPSTREAM_RE}|${MIRROR_RE}):[0-9]+@sha256:[0-9a-f]{64}" "$CF" | head -n1)" || [ -z "$pinned_ref" ]; then
	echo "erreur: aucun pin de base reconnu dans $CF (ni amont ni miroir)" >&2
	exit 3
fi

# --- 2. Résoudre le digest amont courant ---------------------------------------
if ! upstream="$(skopeo inspect --no-tags "docker://${UPSTREAM_REPO}:${UPSTREAM_TAG}" --format '{{.Digest}}')" || [ -z "$upstream" ]; then
	echo "erreur: impossible de résoudre ${UPSTREAM_REPO}:${UPSTREAM_TAG}" >&2
	exit 4
fi
# Valider AVANT tout splice dans sed : une valeur inattendue (sha512, chaîne
# vide, sortie d'erreur) produirait un pin corrompu.
if ! [[ "$upstream" =~ ^sha256:[0-9a-f]{64}$ ]]; then
	echo "erreur: digest amont au format inattendu: $upstream" >&2
	exit 4
fi

# --- 3. Refuser un amont mono-arch ---------------------------------------------
# Un digest mono-arch casserait silencieusement l'autre architecture — et on ne
# le découvrirait qu'au moment du tag, en pleine publication. Même garde que
# bump-base-digest.sh.
if ! raw="$(skopeo inspect --raw "docker://${UPSTREAM_REPO}@${upstream}")"; then
	echo "erreur: impossible d'inspecter ${UPSTREAM_REPO}@${upstream}" >&2
	exit 5
fi
if ! printf '%s' "$raw" | grep -qE '"mediaType"[[:space:]]*:[[:space:]]*"[^"]*(image\.index|manifest\.list)'; then
	echo "erreur: l'amont n'est pas une manifest-list multi-arch — abandon" >&2
	exit 5
fi
for arch in amd64 arm64; do
	if ! printf '%s' "$raw" | grep -qE "\"architecture\"[[:space:]]*:[[:space:]]*\"$arch\""; then
		echo "erreur: arch $arch absente de la manifest-list amont — abandon" >&2
		exit 6
	fi
done

# --- 4. Copier vers le miroir (toutes arches) ----------------------------------
if [ "$DRY_RUN" = "1" ]; then
	echo "dry-run: copie vers ${MIRROR_REPO}:${UPSTREAM_TAG} non effectuée"
	mirror_digest="$upstream" # le cas nominal : copie octet pour octet
else
	if ! skopeo copy --all \
		"docker://${UPSTREAM_REPO}@${upstream}" \
		"docker://${MIRROR_REPO}:${UPSTREAM_TAG}"; then
		echo "erreur: la copie vers ${MIRROR_REPO}:${UPSTREAM_TAG} a échoué (droits d'écriture ? skopeo login ?)" >&2
		exit 7
	fi
	# RELIRE le digest réellement obtenu plutôt que de le déduire. `--all` préserve
	# normalement le manifeste (donc le digest), mais un registre peut convertir un
	# format : on épingle ce qui EXISTE, jamais ce qu'on espérait.
	if ! mirror_digest="$(skopeo inspect --no-tags "docker://${MIRROR_REPO}:${UPSTREAM_TAG}" --format '{{.Digest}}')" || [ -z "$mirror_digest" ]; then
		echo "erreur: copie faite mais digest du miroir illisible — ne pas épingler à l'aveugle" >&2
		exit 7
	fi
	if ! [[ "$mirror_digest" =~ ^sha256:[0-9a-f]{64}$ ]]; then
		echo "erreur: digest du miroir au format inattendu: $mirror_digest" >&2
		exit 7
	fi
fi

# La propriété qui rend la provenance PROUVABLE plutôt qu'affirmée. Une
# divergence n'est pas fatale (conversion de format légitime), mais elle doit
# être VUE : sans ce signalement, on perdrait l'égalité en silence et la
# provenance retomberait au rang de déclaration.
if [ "$mirror_digest" = "$upstream" ]; then
	identical="true"
else
	identical="false"
	echo "::warning::le digest du miroir ($mirror_digest) DIFFÈRE de l'amont ($upstream) — le registre a probablement converti le format. La provenance reste tracée mais n'est plus une égalité de digest." >&2
fi

new_ref="${MIRROR_REPO}:${UPSTREAM_TAG}@${mirror_digest}"

if [ "$pinned_ref" = "$new_ref" ]; then
	echo "unchanged ($new_ref)"
	exit 0
fi

# --- 5. Réécriture « tout ou rien » --------------------------------------------
# Pré-vol : tous les sites existent-ils ? Un site absent est une ERREUR, pas un
# silence — vérifié AVANT la première écriture, sinon le Containerfile serait
# déjà réécrit quand on découvre le problème.
for site in "${EXTRA_PIN_SITES[@]}"; do
	[ -f "$site" ] || {
		echo "erreur: site d'épinglage introuvable: $site (EXTRA_PIN_SITES à corriger)" >&2
		exit 8
	}
done

# On produit chaque version réécrite À CÔTÉ, on les valide toutes, et on ne
# publie qu'ensuite. Sans ça, un site ayant dérivé sur un TROISIÈME digest ne
# matcherait pas et le script annoncerait un succès en laissant un fichier
# périmé — exactement le bug historique du JSON oublié.
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT

sites=("$CF" "${EXTRA_PIN_SITES[@]}")
i=0
for site in "${sites[@]}"; do
	# On remplace la RÉFÉRENCE ENTIÈRE (hôte + repo + tag + digest), pas seulement
	# le digest : la migration change aussi le registre. `|` comme séparateur sed
	# car les refs contiennent des `/`.
	sed -E "s#(${UPSTREAM_RE}|${MIRROR_RE}):[0-9]+@sha256:[0-9a-f]{64}#${new_ref}#g" \
		"$site" >"$STAGE/$i"
	grep -qF -- "$new_ref" "$STAGE/$i" || {
		echo "erreur: $site ne porterait pas $new_ref après réécriture — rien n'a été modifié" >&2
		exit 9
	}
	i=$((i + 1))
done

# `cat >` et non `mv` : garde le mode, le propriétaire et l'inode d'origine.
i=0
for site in "${sites[@]}"; do
	cat "$STAGE/$i" >"$site"
	i=$((i + 1))
done

# --- 6. Provenance -------------------------------------------------------------
# Ce fichier est la RAISON pour laquelle miroiter reste honnête : il dit de quelle
# image amont, exactement, le miroir est la copie. Sans lui, « on héberge la base »
# deviendrait « on ne sait plus d'où elle vient ».
cat >"$PROVENANCE" <<JSON
{
  "_comment": "Provenance de la base miroitée — écrit par scripts/mirror-base.sh. Le miroir est une copie de l'image Fedora ci-dessous ; ce fichier est ce qui permet de le PROUVER. Ne pas éditer à la main. NOTE : ref et digest sont des champs SÉPARÉS, délibérément — les concaténer produirait une chaîne 'repo:tag@sha256:...' que check-base-digest-sync.py compterait comme un SITE D'ÉPINGLAGE de plus à tenir en synchro, alors que ce fichier est un relevé de provenance, pas une entrée de build.",
  "state": "mirrored",
  "upstream_ref": "${UPSTREAM_REPO}:${UPSTREAM_TAG}",
  "upstream_digest": "${upstream}",
  "mirror_ref": "${MIRROR_REPO}:${UPSTREAM_TAG}",
  "mirror_digest": "${mirror_digest}",
  "digest_identical": ${identical},
  "mirrored_at": "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
}
JSON

echo "mirrored ${upstream} -> ${new_ref}"
