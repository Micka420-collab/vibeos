#!/usr/bin/env bash
# verify-roadmap-truth.sh — garde-fou anti-dérive documentaire pour VibeOS.
#
# POURQUOI. La doc de statut (ROADMAP.md, STATUS.md, docs/DECISIONS.md) a dérivé
# plusieurs fois de la réalité du dépôt (fichier annoncé « livré » mais absent,
# compteur de tests figé sur une vieille valeur, HUD « mocké » alors qu'il est
# câblé). Reconstruire l'état vrai à la main à chaque session coûte cher. Ce
# script attrape MÉCANIQUEMENT les incohérences vérifiables sans ambiguïté, à
# chaque push, pour qu'elles meurent tout de suite au lieu de s'accumuler.
#
# CE QU'IL N'EST PAS. Il ne juge PAS le sens. « proposé » vs « en cours » vs
# « fait », « cible » vs « livré » restent des jugements humains : aucune
# regex ne les tranche, et ce script ne prétend pas le faire. Un run vert ne
# veut pas dire « la doc est fidèle » — seulement « elle n'a pas d'incohérence
# mécanique détectable ». Le jugement humain (le mien, celui de Micka) reste
# nécessaire au-dessus de ce garde-fou, il ne le remplace pas.
#
# POLITIQUE ÉCHEC/RAPPORT (justifiée) :
#   - HARD FAIL (sortie != 0, casse la CI) sur les incohérences NON AMBIGUËS :
#       A. un lien markdown relatif qui ne résout vers aucun fichier (lien mort) ;
#       B. un CHEMIN DE FICHIER repo (préfixe connu + extension), cité entre
#          backticks sur une ligne non marquée « futur », qui n'existe pas dans
#          l'arbre. On ne vise que les fichiers : les répertoires et stores
#          runtime (ex. `memory/reasoning/`) sont exclus (faux positifs).
#     Ces deux-là sont binaires : le fichier est là ou il n'y est pas.
#   - WARNING (rapport, n'échoue pas) sur les checks HEURISTIQUES, où un faux
#     positif est possible (entrées de journal datées, merges squash) :
#       C. le plus grand compteur de tests annoncé ne colle pas au réel ;
#       D. une PR citée comme mergée est introuvable dans l'historique de main.
#     Les rendre bloquants produirait du bruit ; on les remonte pour l'œil humain.
#
# USAGE : bash scripts/verify-roadmap-truth.sh   (0 = pas d'incohérence dure)
set -euo pipefail

# Toujours travailler depuis la racine du dépôt (le script est appelable de partout).
ROOT="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
cd "$ROOT"

# Interpréteur portable : en CI Linux c'est python3 ; sur un poste Windows le
# « python3 » du PATH peut être le stub Microsoft Store (qui échoue). On retient
# le premier interpréteur qui EXÉCUTE réellement du code.
PY=""
for cand in python3 python; do
  if "$cand" -c 'import sys' >/dev/null 2>&1; then PY="$cand"; break; fi
done
if [ -z "$PY" ]; then
  echo "verify-roadmap-truth: aucun interpréteur Python fonctionnel (python3/python)" >&2
  exit 2
fi

"$PY" - <<'PY'
import glob
import pathlib
import re
import subprocess
import sys

# Sortie UTF-8 déterministe (un stdout cp1252 sous Windows planterait sur un
# accent ; la CI Linux est déjà en UTF-8). Marqueurs de statut en ASCII pur.
try:
    sys.stdout.reconfigure(encoding="utf-8")
except Exception:
    pass

ROOT = pathlib.Path(".")
DOCS = ["ROADMAP.md", "STATUS.md", "docs/DECISIONS.md"]

# Une ligne portant l'un de ces marqueurs décrit un livrable FUTUR/cible : on ne
# lui reproche pas qu'un chemin cité n'existe pas encore.
FUTURE_MARKERS = [
    "- [ ]", "🛣️", "à créer", "à venir", "prévu", "prévue", "cible", "planifié",
    "planifiée", "TODO", "proposé", "proposée", "reste à", "sera ", "futur",
    "Phase 5", "Phase 6", "Phase 7", "non tranché", "reporté",
]

# Préfixes de chemins RELATIFS au dépôt (on exclut ainsi les chemins-cible
# absolus d'installation comme /usr/... ou /etc/skel/... qui ne sont pas des
# fichiers du dépôt).
REPO_PREFIXES = (
    "vibed/", "os/", "security/", "desktop/", "docs/", "scripts/", "zed/",
    "memory/", "assets/", ".github/",
)

hard = []   # incohérences dures -> exit 1
warn = []   # signalements heuristiques -> rapport seulement


def lines(doc):
    p = ROOT / doc
    if not p.exists():
        hard.append(f"{doc}: document de statut introuvable")
        return []
    return list(enumerate(p.read_text(encoding="utf-8").splitlines(), 1))


# --- Check A (HARD) : les liens markdown relatifs résolvent -------------------
LINK_RE = re.compile(r"\[[^\]]*\]\(([^)]+)\)")
for doc in DOCS:
    p = ROOT / doc
    for i, line in lines(doc):
        for target in LINK_RE.findall(line):
            t = target.strip()
            if t.startswith(("http://", "https://", "mailto:", "#", "/")):
                continue  # lien externe, ancre pure, ou chemin absolu d'install
            t = t.split("#", 1)[0].strip()  # retire l'ancre #section
            if not t:
                continue
            resolved = (p.parent / t)
            if not resolved.exists():
                hard.append(f"{doc}:{i}: lien markdown mort -> {target}")


# --- Check B (HARD) : les chemins repo cités entre backticks existent ---------
BT_RE = re.compile(r"`([^`]+)`")
for doc in DOCS:
    for i, line in lines(doc):
        if any(m in line for m in FUTURE_MARKERS):
            continue
        for tok in BT_RE.findall(line):
            tok = tok.strip().rstrip(".,;:)")
            if not tok.startswith(REPO_PREFIXES):
                continue
            if any(c in tok for c in "*?<> \t"):
                continue  # glob ou fragment de prose, pas un chemin littéral
            # On ne vérifie que les CHEMINS DE FICHIER (dernier segment avec une
            # extension). Un répertoire ou une référence runtime (ex.
            # `memory/reasoning/`, un store créé au boot, jamais commité) n'est
            # pas un fichier du dépôt : le flaguer serait un faux positif.
            seg = tok.rstrip("/").split("/")[-1]
            if "." not in seg:
                continue
            if not (ROOT / tok).exists():
                hard.append(f"{doc}:{i}: fichier repo cité mais absent -> `{tok}`")


# --- Check C (WARNING) : compteur de tests vs réel ----------------------------
# Réel = toutes les annotations #[test] / #[tokio::test] de la crate vibed
# (src + tests d'intégration).
real_tests = 0
for f in glob.glob("vibed/**/*.rs", recursive=True):
    txt = pathlib.Path(f).read_text(encoding="utf-8")
    real_tests += len(re.findall(r"#\[(?:tokio::)?test\]", txt))

# Annoncé = le plus GRAND « N tests » cité dans ROADMAP/STATUS (les entrées
# datées passées sont légitimement plus basses ; la valeur courante devrait être
# le maximum et devrait égaler le réel).
claims = []
CLAIM_RE = re.compile(r"(\d+)\s+tests?\b")
for doc in ("ROADMAP.md", "STATUS.md"):
    for i, line in lines(doc):
        for m in CLAIM_RE.finditer(line):
            claims.append((doc, i, int(m.group(1))))
if claims and real_tests:
    top = max(claims, key=lambda c: c[2])
    if top[2] != real_tests:
        warn.append(
            f"compteur de tests : plus grand annoncé = {top[2]} "
            f"({top[0]}:{top[1]}), réel #[test]+#[tokio::test] dans vibed/ = "
            f"{real_tests}. Si {top[2]} se veut le total courant, il a dérivé "
            f"(les entrées datées plus basses sont normales)."
        )


# --- Check D (WARNING) : PR citées « mergées » présentes dans l'historique ----
try:
    log = subprocess.run(
        ["git", "log", "--oneline", "--max-count=4000"],
        capture_output=True, text=True, check=True,
    ).stdout
except Exception as exc:  # pas d'historique (checkout superficiel) : on saute
    log = ""
    warn.append(f"check PR sauté (git log indisponible : {exc})")

if log:
    PR_RE = re.compile(r"#(\d{2,4})\b")
    cited = {}
    # Toute ligne citant une PR n'affirme pas qu'elle est DANS main. Deux cas :
    #  - PR morte/superseded/supprimée ;
    #  - PR explicitement décrite comme OUVERTE / en attente de revue — c'est
    #    précisément ce qu'un STATUS honnête doit lister, et le lui reprocher
    #    apprendrait à ne plus le faire (faux positif observé le 2026-07-15 sur
    #    « PR ouvertes, en attente de revue humaine : #46, #47… », que le
    #    déclencheur « pr » attrapait).
    # On ne garde donc que les lignes qui prétendent réellement à un merge.
    NOT_A_MERGE_CLAIM = (
        # morte / remplacée
        "supprimé", "morte", "mort ", "fermé", "abandonné", "remplacé",
        "ex-#", "obsolète", "superseded", "non mergé",
        # ouverte / en attente
        "ouverte", "ouvertes", "en attente", "à revoir", "à merger",
        "draft", "brouillon", "en cours de revue",
    )
    for doc in ("STATUS.md", "ROADMAP.md"):
        for i, line in lines(doc):
            low = line.lower()
            if not ("pr " in low or "pull request" in low or "merg" in low):
                continue
            if any(d in low for d in NOT_A_MERGE_CLAIM):
                continue
            for m in PR_RE.finditer(line):
                cited.setdefault(m.group(1), (doc, i))
    for pr, (doc, i) in sorted(cited.items(), key=lambda kv: int(kv[0])):
        # Merge classique : « Merge pull request #N ». Squash : le numéro peut
        # apparaître dans le sujet «  (#N) ». On cherche les deux.
        if f"#{pr}" not in log:
            warn.append(
                f"PR #{pr} citée comme mergée ({doc}:{i}) mais absente de "
                f"l'historique de main (possible : merge squash sans le numéro, "
                f"ou mergée sur une mauvaise base — à vérifier à l'œil)."
            )


# --- Rapport ------------------------------------------------------------------
print("== verify-roadmap-truth ==")
print(f"tests vibed réels (#[test]+#[tokio::test]) : {real_tests}")
print()
if warn:
    print(f"[WARN] {len(warn)} signalement(s) heuristique(s) (n'échoue pas la CI) :")
    for w in warn:
        print(f"   - {w}")
    print()
if hard:
    print(f"[FAIL] {len(hard)} incohérence(s) MÉCANIQUE(S) dure(s) :")
    for h in hard:
        print(f"   - {h}")
    print()
    print("Corrige la doc (ou le chemin/lien) : ces cas sont binaires, pas des jugements.")
    sys.exit(1)

print("[OK] Aucune incohérence mécanique dure. (Le sens reste un jugement humain.)")
sys.exit(0)
PY
