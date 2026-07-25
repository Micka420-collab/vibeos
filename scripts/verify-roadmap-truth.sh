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
import os
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


# --- Check A2 (HARD) : le badge « base » des README == la vraie base ----------
#
# Le 2026-07-15, la base est passée de Fedora Kinoite 42 à 44 (la 42 était EOL
# depuis 49 jours). Les 4 README ont continué d'afficher « Fedora Kinoite 42 »
# EN BADGE, tout en haut de la page d'accueil, dans les 4 langues. Personne ne
# pouvait le voir : un badge est une image, aucun test ne le lit.
#
# Corriger la valeur sans poser ce contrôle, c'est juste attendre qu'elle mente
# à nouveau au prochain rebase. Le badge pointe déjà os/Containerfile — ce check
# ne fait qu'exiger qu'il dise la vérité.
BADGE_RE = re.compile(r"img\.shields\.io/badge/[^)\s]*Kinoite%20(\d+)")
containerfile = (ROOT / "os" / "Containerfile")
if containerfile.exists():
    cf_text = containerfile.read_text(encoding="utf-8", errors="replace")
    m = re.search(r"quay\.io/fedora/fedora-kinoite:(\d+)", cf_text)
    if not m:
        hard.append(
            "os/Containerfile : impossible d'y lire la version de la base "
            "(motif `quay.io/fedora/fedora-kinoite:<N>`) — ce check vient de "
            "devenir aveugle, ce qui est exactement ce qu'il doit empêcher. "
            "Corrige le motif, ne supprime pas le check."
        )
    else:
        real = m.group(1)
        for doc in ("README.md", "README.en.md", "README.es.md", "README.de.md"):
            if not (ROOT / doc).exists():
                continue
            for i, line in lines(doc):
                for shown in BADGE_RE.findall(line):
                    if shown != real:
                        hard.append(
                            f"{doc}:{i}: le badge annonce Fedora Kinoite {shown} "
                            f"alors que os/Containerfile est sur la {real}"
                        )


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

# Annoncé = tout « N tests » cité dans les documents d'ÉTAT COURANT.
#
# STATUS.md est délibérément EXCLU de ce check : c'est un JOURNAL
# CHRONOLOGIQUE. Ses entrées datées (114 → 140 → 145 → 149 …) étaient correctes
# à leur date et ne doivent JAMAIS être réécrites — les corriger serait réécrire
# l'histoire. Or ce check comparait autrefois le plus GRAND compteur de STATUS au
# réel : comme la dernière entrée datée est par construction en retard dès qu'un
# test est ajouté, il criait au loup À CHAQUE FOIS, pour toujours, et sans jamais
# regarder l'endroit où vit vraiment l'affirmation d'état courant (les README).
# Un garde-fou qui avertit en permanence finit ignoré — c'est pire qu'aucun
# garde-fou. (Corrigé le 2026-07-15, après l'avoir observé faire exactement ça.)
#
# ROADMAP.md est exclu pour la MÊME raison, moins évidente : son unique compteur
# vit dans un RÉCIT daté (« faits ce soir, chacun vérifié (148 tests) », tableau
# de dette F6) — de l'histoire, pas une affirmation d'état.
#
# Restent les README : la FACE du projet, le seul endroit qui affirme un total
# COURANT. Les 4 sont vérifiés séparément contre le réel — pas de « plus grand
# gagne » : une TRADUCTION qui a dérivé pendant que le français est juste est
# précisément ce qu'on veut voir (c'est arrivé : #46 figeait 149 dans en/es/de).
CURRENT_STATE_DOCS = ["README.md", "README.en.md", "README.es.md", "README.de.md"]
# Le nombre et le mot « tests » ne se touchent pas dans toutes les langues :
# « 175 tests verts » (fr) mais « 175 GREEN tests » (en), « 175 GRÜNE
# `vibed`-Tests » (de), « 175 pruebas en verde » (es). Un `\s+tests` littéral ne
# voyait donc QUE le français — et ratait les traductions, exactement là où la
# dérive est le plus probable (vérifié : une dérive injectée en anglais passait
# au travers). D'où le lookahead à fenêtre courte.
CLAIM_RE = re.compile(
    r"(\d+)(?=.{0,24}?\b(?:tests?|pruebas?)\b)", re.IGNORECASE
)
if real_tests:
    for doc in CURRENT_STATE_DOCS:
        if not (ROOT / doc).exists():
            continue
        for i, line in lines(doc):
            # PAS de filtre FUTURE_MARKERS ici — contrairement au check B. Un
            # compteur de tests parle du PRÉSENT, quoi que la ligne dise par
            # ailleurs, et la phrase de statut des README énumère justement ce qui
            # « reste à venir » dans la même phrase que le total actuel. Le filtre
            # sautait donc la ligne 30 du README **français** (« Restent à venir »)
            # tout en gardant l'anglaise (« Still to come ») : le check ne voyait
            # pas la langue canonique du projet. Trouvé en injectant une dérive
            # dans les 4 langues — FR passait au travers, les 3 autres non.
            for m in CLAIM_RE.finditer(line):
                claimed = int(m.group(1))
                # Les décompositions (« dont 9 tests d'intégration ») sont des
                # sous-ensembles légitimes : seul un compteur >= 100 se veut le
                # total du crate (le plus petit décompte partiel est loin dessous).
                if claimed >= 100 and claimed != real_tests:
                    warn.append(
                        f"compteur de tests : {doc}:{i} annonce {claimed}, réel "
                        f"#[test]+#[tokio::test] dans vibed/ = {real_tests}. "
                        f"(STATUS.md est un journal daté et n'est pas vérifié ici.)"
                    )


# --- Check D (WARNING) : PR citées « mergées » présentes dans l'historique ----
#
# CE CHECK EST AVEUGLE SUR UN CLONE SUPERFICIEL, ET DOIT LE DIRE.
#
# Un `git clone --depth N` tronque l'historique : toute PR mergée AVANT la borne
# est absente de `git log` alors qu'elle est bel et bien dans main. Le check
# signalait alors une « dérive documentaire » parfaitement imaginaire — et
# comme il ne disait pas pourquoi, la seule façon de s'en apercevoir était
# d'aller vérifier la PR à la main sur GitHub.
#
# C'est arrivé : deux avertissements (#33 et #37, mergées le 2026-07-14) sont
# restés affichés pendant des jours sur les clones superficiels, à faire douter
# d'un STATUS.md qui disait vrai. La CI, elle, checkout en `fetch-depth: 0` et
# ne les a jamais vus — donc rien ne poussait à corriger l'illusion.
#
# Un contrôle qui devient aveugle et continue de parler comme s'il voyait est
# pire qu'un contrôle absent (cf. scripts/README.md). On détecte donc la
# troncature et on ANNOTE les avertissements concernés au lieu de les servir
# comme des constats.
shallow = False
try:
    shallow = subprocess.run(
        ["git", "rev-parse", "--is-shallow-repository"],
        capture_output=True, text=True, check=True,
    ).stdout.strip() == "true"
except Exception:
    # Vieux git sans `--is-shallow-repository` : le fichier marqueur fait foi.
    shallow = os.path.exists(os.path.join(".git", "shallow"))

# La borne du clone, pour que l'humain puisse trancher d'un coup d'œil : une PR
# citée plus ancienne que cette date est hors de portée du check, point final.
#
# DEUX PIÈGES, tous deux rencontrés en écrivant ceci :
#   * `git log --reverse --max-count=1` rend le commit le plus RÉCENT, pas le
#     plus ancien — git choisit d'abord les commits (limite comprise) PUIS
#     inverse l'affichage ;
#   * `git rev-list --max-parents=0` rend LES racines (un clone superficiel en a
#     plusieurs, une par greffe) dans l'ordre antichronologique — prendre la
#     première donne encore la plus récente.
# La dernière ligne du log EST, par définition, le plus ancien commit que git
# consent à montrer. C'est exactement la borne qu'on veut annoncer.
horizon = ""
if shallow:
    try:
        histoire = subprocess.run(
            ["git", "log", "--format=%ci %h %s"],
            capture_output=True, text=True, check=True,
        ).stdout.splitlines()
        horizon = histoire[-1].strip() if histoire else ""
    except Exception:
        horizon = ""

try:
    log = subprocess.run(
        ["git", "log", "--oneline", "--max-count=4000"],
        capture_output=True, text=True, check=True,
    ).stdout
except Exception as exc:  # pas d'historique du tout : on saute
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
            if shallow:
                # Le check ne VOIT pas tout l'historique : ce n'est pas un
                # constat de dérive, c'est un angle mort. On le dit ainsi, avec
                # de quoi vérifier (la borne du clone) — jamais comme un doute
                # sur la doc.
                warn.append(
                    f"PR #{pr} citée comme mergée ({doc}:{i}) : NON VÉRIFIABLE ICI — "
                    f"le clone est SUPERFICIEL, l'historique est tronqué"
                    + (f" (le plus ancien commit visible est : {horizon})" if horizon else "")
                    + ". Ce n'est PAS une dérive documentaire : la CI checkout en "
                    "`fetch-depth: 0` et voit l'historique complet. Pour vérifier "
                    "localement : `git fetch --unshallow`."
                )
            else:
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
