#!/usr/bin/env python3
"""check-base-digest-sync.py — un seul digest de base, partout, et bumpable.

POURQUOI

Le digest de la base Fedora n'est pas épinglé à un seul endroit. Il vit dans
`os/Containerfile` (le `FROM` et le label OCI) MAIS AUSSI dans
`os/rootfs/usr/lib/vibeos/image-info.json`, la carte d'identité que l'image
expose à l'exécution. `scripts/bump-base-digest.sh` ne réécrivait que le
Containerfile : à chaque purge quay suivie d'un bump, le JSON gardait l'ANCIEN
digest et l'image annonçait au système une base qui n'était pas celle sur
laquelle elle avait été construite. Dérive silencieuse, invisible en CI, fausse
piste garantie le jour d'un audit de provenance.

C'est le même motif qui a déjà mordu ce dépôt partout : deux endroits tenus en
synchro À LA MAIN. Le remède est toujours le même — le rendre mécanique.

CE QU'IL VÉRIFIE

1. TOUS les pins de base du dépôt portent le MÊME repo, le MÊME tag et le MÊME
   digest. Deux formes sont reconnues — l'amont `quay.io/fedora/fedora-kinoite`
   et le MIROIR `ghcr.io/…/vibeos-base` (ADR-031) — et le REPO compte autant que
   le digest : pendant la migration, un site resté sur quay pendant que le
   Containerfile est passé au miroir ferait construire depuis une base et en
   annoncer une autre. Un seul désaccord = rouge, avec les emplacements.
2. Tout fichier porteur d'un pin AUTRE que le Containerfile est déclaré dans
   `EXTRA_PIN_SITES` de CHAQUE réécriveur présent (`bump-base-digest.sh` ET
   `mirror-base.sh`). Sans ça, ce fichier resterait périmé le jour où c'est
   l'autre script qui tourne — exactement le bug d'origine. Ajouter un site
   d'épinglage sans l'apprendre aux deux fait rougir la CI.

CE QU'IL NE VÉRIFIE PAS

Que le digest résout encore — c'est `check-base-digest-fresh.sh` (réseau, cron).
Ici : zéro réseau, cohérence interne pure, exécutable sur chaque push.

ANGLE MORT ASSUMÉ. Un pin caché dans un fichier suivi mais non décodable en
UTF-8 (binaire, ou UTF-16) est invisible : `scan()` passe son chemin. Ce dépôt
n'épingle qu'en UTF-8 et rien ne suggère que ça change ; le noter ici vaut mieux
que de laisser croire à une couverture totale.

USAGE : python3 scripts/check-base-digest-sync.py   (0 = cohérent)
"""

import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
CONTAINERFILE = ROOT / "os" / "Containerfile"
BUMPER = ROOT / "scripts" / "bump-base-digest.sh"
MIRRORER = ROOT / "scripts" / "mirror-base.sh"

# DEUX FORMES de pin coexistent, et ce contrôle doit voir les deux :
#   - l'AMONT quay, avant la bascule ADR-031 ;
#   - le MIROIR ghcr (`…/vibeos-base`), après.
# N'en reconnaître qu'une rendait ce contrôle aveugle le jour de la migration —
# et il se serait éteint en annonçant « aucun pin trouvé », c'est-à-dire au pire
# moment : celui où l'on déplace la base de la chaîne d'approvisionnement.
# Le groupe 1 (repo) est capturé pour pouvoir exiger qu'un SEUL repo règne.
PIN_RE = re.compile(
    r"(quay\.io/fedora/fedora-kinoite|ghcr\.io/[^:\s\"]+/vibeos-base)"
    r":(\d+)@(sha256:[0-9a-f]{64})"
)

errors = []


def tracked_files() -> list[pathlib.Path]:
    """Les fichiers SUIVIS PAR GIT, et eux seuls.

    Marcher l'arborescence à l'aveugle (`rglob`) ramasserait des copies non
    suivies — worktrees d'agents, artefacts de build, checkouts jetables — dont
    les pins périmés feraient rougir la CI pour rien. Le contrat porte sur ce que
    le dépôt EXPÉDIE ; `git ls-files` est exactement cette liste.
    """
    out = subprocess.run(
        ["git", "-C", str(ROOT), "ls-files", "-z"],
        capture_output=True,
        check=True,
        text=True,
    ).stdout
    return [ROOT / name for name in out.split("\0") if name]


def scan() -> list[tuple[pathlib.Path, int, str, str, str]]:
    """Tous les pins du dépôt : (fichier, ligne, repo, tag, digest)."""
    found = []
    for path in sorted(tracked_files()):
        if not path.is_file() or path.is_symlink():
            continue
        # Ce fichier-ci ne porte que la REGEX, jamais un pin ; l'exclure évite
        # qu'une future capture d'exemple dans la docstring devienne un faux pin.
        if path.resolve() == pathlib.Path(__file__).resolve():
            continue
        try:
            text = path.read_text(encoding="utf-8")
        except (UnicodeDecodeError, OSError):
            continue  # binaire ou illisible : aucun pin textuel à y trouver
        for lineno, line in enumerate(text.splitlines(), start=1):
            for repo, tag, digest in PIN_RE.findall(line):
                found.append((path, lineno, repo, tag, digest))
    return found


pins = scan()

if not pins:
    errors.append(
        "aucun pin de base (`quay.io/fedora/fedora-kinoite:NN@sha256:…` ou "
        "`ghcr.io/…/vibeos-base:NN@sha256:…`) trouvé dans le dépôt — "
        "soit l'épinglage a disparu (la base n'est plus reproductible), soit ce "
        "contrôle est devenu aveugle. Répare le motif, ne supprime pas le contrôle."
    )

# --- 1. Un seul digest, partout ------------------------------------------------
canonical = None
for path, lineno, repo, tag, digest in pins:
    if path == CONTAINERFILE:
        canonical = (repo, tag, digest)
        break
if canonical is None and pins:
    errors.append(
        f"aucun pin dans {CONTAINERFILE.relative_to(ROOT)} — c'est la source de "
        "vérité du digest de base ; sans lui, rien à comparer."
    )

if canonical is not None:
    ref_repo, ref_tag, ref_digest = canonical
    for path, lineno, repo, tag, digest in pins:
        # Le REPO fait partie de l'identité : pendant la migration, un site resté
        # sur quay pendant que le Containerfile est passé au miroir construirait
        # depuis une base et en annoncerait une autre. C'est la même dérive que
        # le digest désaccordé, en pire — elle traverse deux registres.
        if (repo, tag, digest) != (ref_repo, ref_tag, ref_digest):
            errors.append(
                f"{path.relative_to(ROOT)}:{lineno} épingle "
                f"{repo}:{tag}@{digest}\n"
                f"        alors qu'os/Containerfile épingle "
                f"{ref_repo}:{ref_tag}@{ref_digest}.\n"
                f"        Un seul digest de base doit exister dans le dépôt : "
                f"l'image annoncerait sinon une base qui n'est pas celle construite.\n"
                f"        Remède : bash scripts/mirror-base.sh os/Containerfile "
                f"(ou bump-base-digest.sh avant la bascule ADR-031) — "
                f"les deux réécrivent TOUS les sites."
            )

# --- 2. Tout site non-Containerfile est connu du bumpeur -----------------------
rewriters = [p for p in (BUMPER, MIRRORER) if p.is_file()]
if rewriters:
    # Un site doit être connu de TOUS les réécriveurs présents : il suffit qu'un
    # seul l'ignore pour que le jour où c'est LUI qui tourne, le fichier reste
    # périmé. Exiger « au moins un » rouvrirait exactement le bug d'origine.
    for path in sorted({p for p, _, _, _, _ in pins if p != CONTAINERFILE}):
        rel = path.relative_to(ROOT).as_posix()
        # On exige la forme EXACTE que le bumpeur utilise pour déclarer un site,
        # `"$ROOT/<rel>"`, guillemets compris — pas le chemin nu quelque part
        # dans le fichier. Chercher la sous-chaîne nue rendait le contrôle
        # satisfiable par un simple COMMENTAIRE : retirer une entrée du tableau
        # en laissant `# TODO: réajouter os/…/image-info.json` gardait la CI
        # verte alors que le bumpeur ne réécrivait plus ce fichier — soit
        # précisément la dérive que ce contrôle existe pour interdire.
        missing = [
            r.name
            for r in rewriters
            if f'"$ROOT/{rel}"' not in r.read_text(encoding="utf-8")
        ]
        if missing:
            errors.append(
                f"{rel} porte un pin de base mais n'est pas déclaré dans "
                f"EXTRA_PIN_SITES de : {', '.join(missing)}.\n"
                f"        Au prochain bump automatique, ce fichier garderait "
                f"l'ancien digest — la dérive silencieuse que ce contrôle existe "
                f"pour empêcher.\n"
                f'        Remède : ajoute littéralement "$ROOT/{rel}" (guillemets '
                f"compris) à EXTRA_PIN_SITES de chacun ; une mention en commentaire "
                f"ne compte pas."
            )
else:
    errors.append(
        "ni scripts/bump-base-digest.sh ni scripts/mirror-base.sh — plus rien ne "
        "réécrit les pins ; ce contrôle ne peut plus vérifier qu'ils sont tous connus."
    )

# --- 3. La provenance ne ment pas ------------------------------------------------
# `os/base-provenance.json` dit de quelle image amont l'image est bâtie. Il ne
# porte PAS de référence `repo:tag@digest` (ref et digest y sont des champs
# séparés, délibérément), donc il échappe au balayage ci-dessus : sans ce contrôle,
# un bump le laisserait désigner une base purgée que plus rien ne construit — un
# relevé de provenance qui ment, pire que pas de relevé.
PROVENANCE = ROOT / "os" / "base-provenance.json"
if canonical is not None and PROVENANCE.is_file():
    import json

    try:
        prov = json.loads(PROVENANCE.read_text(encoding="utf-8"))
    except (json.JSONDecodeError, OSError) as exc:
        errors.append(f"os/base-provenance.json illisible ({exc}) — provenance invérifiable.")
    else:
        state = prov.get("state")
        if state == "not_mirrored":
            # Avant la bascule : le pin EST l'amont, la provenance doit le refléter.
            if prov.get("upstream_digest") != ref_digest:
                errors.append(
                    f"os/base-provenance.json déclare upstream_digest="
                    f"{prov.get('upstream_digest')}\n"
                    f"        alors que le pin est {ref_digest}.\n"
                    f"        La provenance désignerait une base qui n'est pas celle "
                    f"construite.\n"
                    f"        Remède : bash scripts/bump-base-digest.sh os/Containerfile "
                    f"(il recale la provenance depuis ce correctif)."
                )
        elif state == "mirrored":
            # Après la bascule : le pin est le MIROIR, la provenance doit le refléter.
            if prov.get("mirror_digest") != ref_digest:
                errors.append(
                    f"os/base-provenance.json déclare mirror_digest="
                    f"{prov.get('mirror_digest')}\n"
                    f"        alors que le pin est {ref_digest}.\n"
                    f"        Remède : bash scripts/mirror-base.sh os/Containerfile."
                )
        else:
            errors.append(
                f"os/base-provenance.json porte state={state!r} — attendu "
                f"\"not_mirrored\" ou \"mirrored\". Un état inconnu rend la "
                f"provenance invérifiable ; répare le fichier, pas ce contrôle."
            )

# --- Verdict -------------------------------------------------------------------
if errors:
    print("\033[31mFAIL\033[0m  digest de base désynchronisé\n", file=sys.stderr)
    for e in errors:
        print(f"  - {e}", file=sys.stderr)
    sys.exit(1)

ref_repo, ref_tag, ref_digest = canonical
sites = len({p for p, _, _, _, _ in pins})
kind = "miroir" if ref_repo.startswith("ghcr.io/") else "amont quay"
print(
    f"\033[32mok\033[0m    un seul digest de base ({kind}), cohérent sur {sites} "
    f"fichier(s) ({len(pins)} occurrence(s)) : {ref_repo}:{ref_tag}@{ref_digest}"
)
