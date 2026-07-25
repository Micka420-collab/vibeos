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

1. TOUS les pins `quay.io/fedora/fedora-kinoite:NN@sha256:…` du dépôt portent le
   MÊME tag et le MÊME digest. Un seul désaccord = rouge, avec les emplacements.
2. Tout fichier porteur d'un pin AUTRE que le Containerfile est déclaré dans
   `EXTRA_PIN_SITES` de `scripts/bump-base-digest.sh`. Sans ça, l'auto-bump
   laisserait ce fichier périmé au prochain bump — c'est exactement le bug
   d'origine. Ajouter un site d'épinglage SANS l'apprendre au bumpeur fait
   désormais rougir la CI.

CE QU'IL NE VÉRIFIE PAS

Que le digest résout encore — c'est `check-base-digest-fresh.sh` (réseau, cron).
Ici : zéro réseau, cohérence interne pure, exécutable sur chaque push.

USAGE : python3 scripts/check-base-digest-sync.py   (0 = cohérent)
"""

import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
CONTAINERFILE = ROOT / "os" / "Containerfile"
BUMPER = ROOT / "scripts" / "bump-base-digest.sh"

PIN_RE = re.compile(r"quay\.io/fedora/fedora-kinoite:(\d+)@(sha256:[0-9a-f]{64})")

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


def scan() -> list[tuple[pathlib.Path, int, str, str]]:
    """Tous les pins du dépôt : (fichier, ligne, tag, digest)."""
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
            for tag, digest in PIN_RE.findall(line):
                found.append((path, lineno, tag, digest))
    return found


pins = scan()

if not pins:
    errors.append(
        "aucun pin `quay.io/fedora/fedora-kinoite:NN@sha256:…` trouvé dans le dépôt — "
        "soit l'épinglage a disparu (la base n'est plus reproductible), soit ce "
        "contrôle est devenu aveugle. Répare le motif, ne supprime pas le contrôle."
    )

# --- 1. Un seul digest, partout ------------------------------------------------
canonical = None
for path, lineno, tag, digest in pins:
    if path == CONTAINERFILE:
        canonical = (tag, digest)
        break
if canonical is None and pins:
    errors.append(
        f"aucun pin dans {CONTAINERFILE.relative_to(ROOT)} — c'est la source de "
        "vérité du digest de base ; sans lui, rien à comparer."
    )

if canonical is not None:
    ref_tag, ref_digest = canonical
    for path, lineno, tag, digest in pins:
        if (tag, digest) != (ref_tag, ref_digest):
            errors.append(
                f"{path.relative_to(ROOT)}:{lineno} épingle "
                f"fedora-kinoite:{tag}@{digest}\n"
                f"        alors qu'os/Containerfile épingle "
                f"fedora-kinoite:{ref_tag}@{ref_digest}.\n"
                f"        Un seul digest de base doit exister dans le dépôt : "
                f"l'image annoncerait sinon une base qui n'est pas celle construite.\n"
                f"        Remède : bash scripts/bump-base-digest.sh os/Containerfile "
                f"(il réécrit tous les sites)."
            )

# --- 2. Tout site non-Containerfile est connu du bumpeur -----------------------
if BUMPER.is_file():
    bumper_text = BUMPER.read_text(encoding="utf-8")
    for path in sorted({p for p, _, _, _ in pins if p != CONTAINERFILE}):
        rel = path.relative_to(ROOT).as_posix()
        if rel not in bumper_text:
            errors.append(
                f"{rel} porte un pin de base mais n'est pas déclaré dans "
                f"EXTRA_PIN_SITES de scripts/bump-base-digest.sh.\n"
                f"        Au prochain bump automatique, ce fichier garderait "
                f"l'ancien digest — la dérive silencieuse que ce contrôle existe "
                f"pour empêcher.\n"
                f'        Remède : ajoute "$ROOT/{rel}" à EXTRA_PIN_SITES.'
            )
else:
    errors.append(
        "scripts/bump-base-digest.sh introuvable — l'auto-bump ne peut plus "
        "réécrire les pins ; ce contrôle ne peut plus vérifier qu'il les connaît tous."
    )

# --- Verdict -------------------------------------------------------------------
if errors:
    print("\033[31mFAIL\033[0m  digest de base désynchronisé\n", file=sys.stderr)
    for e in errors:
        print(f"  - {e}", file=sys.stderr)
    sys.exit(1)

ref_tag, ref_digest = canonical
sites = len({p for p, _, _, _ in pins})
print(
    f"\033[32mok\033[0m    un seul digest de base, cohérent sur {sites} fichier(s) "
    f"({len(pins)} occurrence(s)) : fedora-kinoite:{ref_tag}@{ref_digest}"
)
