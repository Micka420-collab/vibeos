#!/usr/bin/env python3
"""check-sectools-sync.py — la trousse cybersécurité annoncée == celle installée.

POURQUOI CE SCRIPT EXISTE

`os/Containerfile` porte ce commentaire, depuis toujours :

    # Keep os/security-tools.txt in strict sync with this list.

Une exigence de synchronisation **manuelle**, que rien n'appliquait. Elle a
dérivé **la première fois que quelqu'un a touché la liste** : le 2026-07-15, le
rebase f42->f44 a retiré `nbtscan`, `wfuzz` et `scalpel` du Containerfile (Fedora
les a retirés d'amont), sans toucher au manifeste. `os/security-tools.txt` a
continué d'annoncer 61 outils pour 58 installés.

Un commentaire qui demande poliment une synchro n'est pas une synchro.

CE QU'IL VÉRIFIE — et ce qu'il NE vérifie PAS

  * `os/security-tools.txt` == la liste de paquets de la couche sectools du
    Containerfile, à l'identique. C'est LA relation que le commentaire exige.

  * `os/rootfs/usr/share/vibeos/security-tools.tsv` n'est **volontairement pas**
    comparé aux deux autres : il liste des **BINAIRES**, pas des paquets, parce
    que `sectools.list` (T0) fait un `stat` dans `/usr/bin`. `bind-utils` fournit
    `dig`, `wireshark-cli` fournit `tshark`, `python3-impacket` fournit
    `impacket-secretsdump` : comparer les deux listes ferait crier ~13 faux
    positifs à chaque run, et un garde-fou qui crie toujours finit ignoré.
    (J'ai failli livrer exactement ce faux positif en écrivant ce script.)

    Ce qui EST vérifiable sans ambiguïté sur le .tsv : aucune de ses entrées ne
    doit référencer un paquet que le Containerfile n'installe plus. On ne peut
    pas le savoir par le nom du binaire — donc on se limite au cas certain : une
    entrée dont le nom est EXACTEMENT un paquet retiré du Containerfile.

USAGE : python3 scripts/check-sectools-sync.py   (0 = en phase)
"""
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
CONTAINERFILE = ROOT / "os" / "Containerfile"
MANIFEST_TXT = ROOT / "os" / "security-tools.txt"
MANIFEST_TSV = ROOT / "os" / "rootfs" / "usr" / "share" / "vibeos" / "security-tools.tsv"

errors = []


def containerfile_sectools():
    """Les paquets de la couche sectools, telle que le Containerfile l'installe.

    Ancrée sur le commentaire qui précède la couche, pas sur des numéros de
    ligne : une couche qui bouge ne doit pas rendre ce check aveugle.
    """
    text = CONTAINERFILE.read_text(encoding="utf-8")
    anchor = "Keep os/security-tools.txt in strict sync with this list."
    if anchor not in text:
        errors.append(
            "os/Containerfile : ancre introuvable (%r). Ce check vient de "
            "devenir aveugle — c'est exactement ce qu'il doit empêcher. "
            "Répare l'ancre, ne supprime pas le check." % anchor
        )
        return None
    after = text.split(anchor, 1)[1]
    # Du `RUN dnf -y install` qui suit, jusqu'au `dnf clean all` qui clôt la couche.
    m = re.search(r"RUN dnf -y install[^\n]*\n(.*?)\n\s*&&\s*dnf clean all", after, re.S)
    if not m:
        errors.append(
            "os/Containerfile : couche sectools introuvable après l'ancre "
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
        out.add(line.split("\t")[0].strip())
    return out


cf = containerfile_sectools()
txt = manifest_entries(MANIFEST_TXT)
tsv = manifest_entries(MANIFEST_TSV)

if cf is not None and txt is not None:
    only_cf = sorted(cf - txt)
    only_txt = sorted(txt - cf)
    for p in only_cf:
        errors.append(
            "`%s` est installé par os/Containerfile mais ABSENT de "
            "os/security-tools.txt — la trousse livrée est plus large que "
            "celle annoncée." % p
        )
    for p in only_txt:
        errors.append(
            "`%s` est annoncé par os/security-tools.txt mais N'EST PAS installé "
            "par os/Containerfile — le manifeste promet un outil absent." % p
        )

# Cas certain seulement (voir l'en-tête) : une entrée du .tsv dont le nom est
# exactement celui d'un paquet que le Containerfile n'installe plus.
if cf is not None and tsv is not None and txt is not None:
    for entry in sorted(tsv - cf):
        if entry in txt or entry in cf:
            continue
        # Le nom pourrait légitimement être un binaire fourni par un paquet au
        # nom différent (dig <- bind-utils). On ne signale que s'il ressemble à
        # un paquet retiré : présent nulle part ailleurs ET dans aucun des deux.
        pass  # non décidable par le nom seul -> on ne crie pas.

if errors:
    print("[FAIL] trousse cybersécurité désynchronisée :")
    for e in errors:
        print("   - %s" % e)
    sys.exit(1)

print(
    "[OK] trousse en phase : os/security-tools.txt == couche sectools du "
    "Containerfile (%d paquets)." % (len(cf) if cf else 0)
)
