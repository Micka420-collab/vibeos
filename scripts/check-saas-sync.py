#!/usr/bin/env python3
"""check-saas-sync.py — le touseau SaaS annoncé == celui installé.

POURQUOI (identique au raisonnement de check-sectools-sync.py)

La couche 1d-ter d'`os/Containerfile` installe les outils SaaS ; `os/saas-tools.txt`
en est le manifeste. Une exigence de synchro entre les deux, laissée en simple
commentaire, dérive à la première main qui touche la liste — c'est arrivé pour la
trousse cybersécu (nbtscan/wfuzz/scalpel retirés du build, jamais du manifeste).
Un commentaire qui demande une synchro n'est pas une synchro. Ce script la rend
exécutable.

CE QU'IL VÉRIFIE

`os/saas-tools.txt` == la liste de paquets de la couche 1d-ter du Containerfile,
à l'identique. C'est LA relation que le commentaire « Keep os/saas-tools.txt in
strict sync » exige.

USAGE : python3 scripts/check-saas-sync.py   (0 = en phase)
"""
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
CONTAINERFILE = ROOT / "os" / "Containerfile"
MANIFEST_TXT = ROOT / "os" / "saas-tools.txt"

errors = []


def containerfile_saas():
    """Les paquets de la couche 1d-ter, telle que le Containerfile les installe.

    Ancrée sur le commentaire qui précède la couche, pas sur des numéros de
    ligne : une couche qui bouge ne doit pas rendre ce check aveugle.
    """
    text = CONTAINERFILE.read_text(encoding="utf-8")
    anchor = "Keep os/saas-tools.txt in strict sync with this list."
    if anchor not in text:
        errors.append(
            "os/Containerfile : ancre introuvable (%r). Ce check vient de "
            "devenir aveugle — c'est exactement ce qu'il doit empêcher. "
            "Répare l'ancre, ne supprime pas le check." % anchor
        )
        return None
    after = text.split(anchor, 1)[1]
    m = re.search(r"RUN dnf -y install[^\n]*\n(.*?)\n\s*&&\s*dnf clean all", after, re.S)
    if not m:
        errors.append(
            "os/Containerfile : couche SaaS introuvable après l'ancre "
            "(motif `RUN dnf -y install ... && dnf clean all`)."
        )
        return None
    pkgs = set()
    for line in m.group(1).split("\n"):
        tok = line.strip().rstrip("\\").strip()
        if not tok or tok.startswith("#") or tok.startswith("--"):
            continue
        if re.fullmatch(r"[a-zA-Z0-9][a-zA-Z0-9._+-]*", tok):
            pkgs.add(tok)
    return pkgs


def manifest_entries(path):
    if not path.exists():
        errors.append("%s : manifeste introuvable" % path.relative_to(ROOT))
        return None
    out = set()
    for line in path.read_text(encoding="utf-8").split("\n"):
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        out.add(line.strip())
    return out


cf = containerfile_saas()
txt = manifest_entries(MANIFEST_TXT)

if cf is not None and txt is not None:
    for p in sorted(cf - txt):
        errors.append(
            "`%s` est installé par la couche 1d-ter du Containerfile mais "
            "ABSENT de os/saas-tools.txt — le touseau livré est plus large que "
            "celui annoncé." % p
        )
    for p in sorted(txt - cf):
        errors.append(
            "`%s` est annoncé par os/saas-tools.txt mais N'EST PAS installé par "
            "la couche 1d-ter — le manifeste promet un outil absent." % p
        )

if errors:
    print("[FAIL] touseau SaaS désynchronisé :")
    for e in errors:
        print("   - %s" % e)
    sys.exit(1)

print(
    "[OK] touseau SaaS en phase : os/saas-tools.txt == couche 1d-ter du "
    "Containerfile (%d outils)." % (len(cf) if cf else 0)
)
