#!/usr/bin/env python3
"""check-guards-wired.py — le garde-fou des garde-fous.

POURQUOI

Ce dépôt a accumulé, en un mois, cinq garde-fous mécaniques : `check-base-eol`,
`check-sectools-sync`, `check-log-hygiene`, `check-hud-client`, `verify-roadmap-truth`.
Chacun a été câblé dans la CI À LA MAIN, un par un. Le sixième le sera aussi — et
rien ne le garantit. C'est exactement le motif « garder en synchro manuellement »
qui a déjà mordu partout ailleurs ce mois-ci (les `paths` de ci.yml deux fois, le
manifeste sectools, le compteur de tests, la liste shellcheck).

Un garde-fou qui existe mais que la CI n'exécute jamais est pire qu'aucun : il
donne l'illusion d'une protection. C'est précisément ce qui est arrivé à
`check-base-eol.sh`, exécuté par accident pendant des heures parce que son fichier
déclencheur manquait des `paths`.

CE QU'IL VÉRIFIE

Le CONTRAT du dossier `scripts/`, énoncé et rendu exécutable :

  Tout script `scripts/check-*` ou `scripts/verify-*` (hors ce fichier et les
  bibliothèques `_*`) DOIT être exécuté par au moins un workflow dans
  `.github/workflows/`.

Un nouveau garde-fou non câblé fait ROUGIR la CI, avec le nom du script et
l'instruction. Impossible d'ajouter un check et d'oublier de le brancher.

CE QU'IL NE VÉRIFIE PAS

Que le check est branché sur le BON déclencheur (`paths:`) — ça, seul un humain
qui lit le workflow le sait. Il garantit l'exécution, pas la pertinence du
déclenchement. C'est déjà l'essentiel : un check jamais exécuté est le mode de
panne le plus courant et le plus silencieux.

USAGE : python3 scripts/check-guards-wired.py   (0 = tous câblés)
"""
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
SCRIPTS = ROOT / "scripts"
WORKFLOWS = ROOT / ".github" / "workflows"

SELF = pathlib.Path(__file__).name

errors = []

# Les garde-fous, par convention de nommage. Une bibliothèque partagée
# (`_helpers.py`) n'est pas un garde-fou exécutable : préfixe `_` exclu.
guards = sorted(
    p.name
    for p in SCRIPTS.iterdir()
    if p.is_file()
    and re.match(r"^(check|verify)-.*\.(sh|py|js)$", p.name)
    and p.name != SELF
    and not p.name.startswith("_")
)

if not guards:
    errors.append(
        "aucun garde-fou check-*/verify-* trouvé dans scripts/ — soit le dossier "
        "a été déplacé, soit ce méta-check est devenu aveugle. Répare, ne supprime "
        "pas."
    )

# Concatène tous les workflows : un garde-fou peut être exécuté par n'importe lequel.
workflow_text = ""
if WORKFLOWS.is_dir():
    for wf in sorted(WORKFLOWS.glob("*.yml")) + sorted(WORKFLOWS.glob("*.yaml")):
        workflow_text += wf.read_text(encoding="utf-8", errors="replace") + "\n"
else:
    errors.append(".github/workflows/ introuvable — la CI ne peut exécuter aucun garde-fou")

for g in guards:
    # Cherche le chemin `scripts/<g>` tel qu'un workflow l'invoquerait.
    if f"scripts/{g}" not in workflow_text:
        errors.append(
            f"`scripts/{g}` n'est exécuté par AUCUN workflow. Un garde-fou que la "
            f"CI ne lance jamais ne garde rien. Ajoute une étape qui l'invoque "
            f"(voir scripts/README.md pour le contrat)."
        )

if errors:
    print("[FAIL] garde-fou(s) non câblé(s) en CI :")
    for e in errors:
        print(f"   - {e}")
    sys.exit(1)

print(f"[OK] les {len(guards)} garde-fous check-*/verify-* sont tous exécutés en CI.")
