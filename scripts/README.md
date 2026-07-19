# `scripts/` — les garde-fous mécaniques de VibeOS

Ce dossier ne contient **que** des garde-fous : des scripts qui attrapent
mécaniquement, à chaque push, une incohérence vérifiable sans ambiguïté. Ils
existent parce que ce projet a dérivé, plusieurs fois, entre ce qu'il *affirme* et
ce qu'il *fait* — et qu'un commentaire qui demande poliment « garder en synchro »
n'est pas une synchro.

## Le contrat

**Tout script `check-*` ou `verify-*` de ce dossier DOIT être exécuté par au moins
un workflow de `.github/workflows/`.** Un garde-fou que la CI ne lance jamais est
pire qu'aucun : il donne l'illusion d'une protection. C'est déjà arrivé
(`check-base-eol.sh` a tourné par accident pendant des heures, son fichier
déclencheur manquant des `paths:`).

Ce contrat n'est pas qu'une convention : `check-guards-wired.py` le **vérifie**.
Ajouter un `check-*` sans le câbler fait rougir la CI. Le garde-fou des garde-fous.

## Convention

- **`check-*`** : vérifie une propriété ponctuelle et binaire (le fichier est là /
  la version colle / la liste est synchrone). Sortie non nulle = échec dur.
- **`verify-*`** : vérifie un ensemble de propriétés, peut distinguer les
  incohérences *dures* (échec) des *heuristiques* (avertissement).
- Un fichier `_*` est une bibliothèque partagée, pas un garde-fou exécutable
  (exclu du contrat).
- Chaque script porte, en tête, **pourquoi il existe** — souvent la dérive réelle
  qui l'a motivé. Un garde-fou sans son histoire finit supprimé par quelqu'un qui
  le croit gratuit.

## Discipline de test

Un garde-fou se teste **par mutation** : on réintroduit le bug qu'il prétend
attraper et on vérifie qu'il rougit ; on casse son ancre et on vérifie qu'il
échoue **fermé** (« ce check vient de devenir aveugle ») plutôt que de passer en
silence. Un check qui devient aveugle et répond vert est le pire des deux mondes.

## Inventaire

| Script | Attrape | Motivé par |
|---|---|---|
| `check-base-eol.sh` | une base Fedora EOL, ou à < 30 j de l'être | l'OS a tourné 49 j sur Fedora 42 EOL, sans aucun signal |
| `check-base-digest-fresh.sh` | le digest de base épinglé purgé par quay (base vivante) | le build a cassé 2 jours de suite, la purge arrive sans push |
| `check-sectools-sync.py` | `security-tools.txt` ≠ couche sectools du Containerfile | 3 outils retirés du build, jamais du manifeste |
| `check-saas-sync.py` | `saas-tools.txt` ≠ couche 1d-ter du Containerfile | même dérive que la trousse cybersécu, prévenue en amont |
| `check-saas-compose.py` | un modèle compose SaaS publiant un port hors loopback (0.0.0.0) | une base exposée au réseau local (souvent sans mdp fort en dev) = fuite |
| `check-log-hygiene.py` | un secret/contenu de fichier loggué en niveau `info` | critère de sortie Phase 2 (ROADMAP §4) |
| `check-hud-client.js` | la couche JS du HUD qui traduit le format `vibed` | seule couche du HUD testable sans Qt |
| `check-guards-wired.py` | un `check-*`/`verify-*` non exécuté en CI | 5 garde-fous câblés à la main, un par un |
| `verify-roadmap-truth.sh` | liens morts, chemins cités absents, badge de base faux, compteur de tests dérivé | la doc de statut a dérivé du dépôt plusieurs fois |

## Ajouter un garde-fou

1. Nomme-le `check-<sujet>.{sh,py,js}` (ou `verify-` s'il agrège).
2. Écris en tête **la dérive réelle** qu'il empêche.
3. **Câble-le** dans `.github/workflows/ci.yml` — sinon `check-guards-wired.py`
   fait rougir la CI (c'est le but).
4. Mutation-teste-le : bug remis → rouge ; ancre cassée → échec fermé.
5. Ajoute une ligne à l'inventaire ci-dessus.
