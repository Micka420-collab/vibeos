#!/usr/bin/env python3
"""check-saas-compose.py — les modèles compose SaaS n'exposent QUE la loopback.

POURQUOI

VibeOS est security-first. Les modèles `compose` de /usr/share/vibeos/saas/
lancent des serveurs (base, cache, S3, métriques, SMTP) sur la machine de
l'utilisateur. La règle de sécurité est que chaque port publié soit lié à
`127.0.0.1` — jamais `0.0.0.0`, jamais un mapping nu qui, en compose, écoute sur
toutes les interfaces. Un seul modèle qui déroge exposerait une base de données
(souvent sans mot de passe fort en dev) à tout le réseau local. Laissée en
convention de revue, cette règle dérive au premier modèle ajouté. Ce script la
rend exécutable — et fail-closed : une entrée de port qu'il ne sait PAS prouver
loopback échoue, plutôt que de passer en silence.

CE QU'IL VÉRIFIE

Pour chaque `compose.yaml` sous os/rootfs/usr/share/vibeos/saas/ :
  1. chaque entrée publiée d'un bloc `ports:` commence par `127.0.0.1:` ;
  2. il existe au moins un modèle (sinon le garde est devenu aveugle : le
     chemin a bougé → fail-closed).

USAGE : python3 scripts/check-saas-compose.py   (0 = conforme)
"""
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
SAAS_DIR = ROOT / "os" / "rootfs" / "usr" / "share" / "vibeos" / "saas"

errors = []


def indent_of(line):
    return len(line) - len(line.lstrip(" "))


def port_spec(entry):
    """Extrait la spec d'une entrée de liste `- <spec>  # commentaire`.

    Renvoie la chaîne de port (déquotée) ou None si l'entrée n'est pas une
    chaîne simple (forme longue en dict — traitée à part par l'appelant).
    """
    body = entry[1:].strip()  # retire le '-'
    if not body:
        return None
    if body[0] in "\"'":
        quote = body[0]
        end = body.find(quote, 1)
        if end == -1:
            return None
        return body[1:end]
    # non quoté : couper un éventuel commentaire en fin, prendre le 1er token
    return body.split("#", 1)[0].strip()


def check_file(path):
    lines = path.read_text(encoding="utf-8").split("\n")
    rel = path.relative_to(ROOT)
    in_ports = False
    ports_indent = -1
    for n, raw in enumerate(lines, 1):
        if not raw.strip():
            continue
        stripped = raw.strip()
        ind = indent_of(raw)

        if stripped.startswith("#"):
            continue  # ligne entièrement commentée : ignorée

        if in_ports:
            # On reste dans le bloc tant que c'est une entrée de liste. En YAML
            # une entrée de séquence peut être à la MÊME indentation que la clé
            # (`ports:\n- "..."`) — d'où `>=`, pas `>` (sinon ce style passe).
            if ind >= ports_indent and stripped.startswith("-"):
                spec = port_spec(stripped)
                if spec is None:
                    errors.append(
                        "%s:%d — entrée de port en forme longue non vérifiable "
                        "par ce garde ; utilisez la forme \"127.0.0.1:HOST:CONT\" "
                        "ou étendez le garde (fail-closed)." % (rel, n)
                    )
                elif not spec.startswith("127.0.0.1:"):
                    errors.append(
                        "%s:%d — port publié `%s` NON lié à la loopback. "
                        "Exigé : \"127.0.0.1:HOST:CONTENEUR\" (jamais 0.0.0.0 "
                        "ni mapping nu)." % (rel, n, spec)
                    )
                continue
            else:
                in_ports = False  # fin du bloc ports

        # Ouverture d'un bloc `ports:` (clé seule, éventuel commentaire en fin).
        # Une forme INLINE (`ports: ["..."]`) ou un alias (`ports: *ancre`) n'est
        # pas vérifiable ligne à ligne → on ÉCHOUE (fail-closed), on ne l'ignore
        # pas : c'est exactement par là qu'un 0.0.0.0 passerait.
        m = re.match(r"^ports\s*:(.*)$", stripped)
        if m:
            rest = m.group(1).split("#", 1)[0].strip()
            if rest:
                errors.append(
                    "%s:%d — bloc `ports:` en forme inline/alias (`%s`) non "
                    "vérifiable par ce garde ; utilisez la forme bloc "
                    '(- "127.0.0.1:HOST:CONT").' % (rel, n, stripped)
                )
            else:
                in_ports = True
                ports_indent = ind


compose_files = sorted(SAAS_DIR.glob("**/compose.yaml"))
if not compose_files:
    errors.append(
        "Aucun compose.yaml sous %s — le garde vient de devenir aveugle "
        "(le chemin a-t-il bougé ?). Répare le chemin, ne supprime pas le "
        "garde." % SAAS_DIR.relative_to(ROOT)
    )
else:
    for f in compose_files:
        check_file(f)

if errors:
    print("[FAIL] modèles compose SaaS — exposition réseau :")
    for e in errors:
        print("   - %s" % e)
    sys.exit(1)

print(
    "[OK] modèles compose SaaS : %d fichier(s), tous les ports publiés sont "
    "en loopback (127.0.0.1)." % len(compose_files)
)
