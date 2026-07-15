# BOOT-VALIDATION — relevé du premier boot réel

> **Ce document est un GABARIT VIDE.** Il ne contient aucun résultat tant que
> personne n'a booté. Ne pré-remplis **rien** : un `PASS` écrit d'avance est
> exactement le mensonge que ce fichier existe pour empêcher.
>
> Documents associés : [VALIDATION.md](VALIDATION.md) est la **procédure** (comment
> valider) ; ce fichier est le **constat** (ce qui a réellement été observé, sur
> quelle machine, quel jour). Ils ne se remplacent pas.

## Pourquoi ce fichier

Tout ce que VibeOS affirme sur lui-même est aujourd'hui prouvé par des tests, une
CI, et des relectures — c'est-à-dire par du code qui juge du code. **Rien n'a
jamais tourné sur du matériel.** Le HUD Quickshell, le splash, la session
graphique, le driver NVIDIA : aucun de ces éléments n'a jamais été *vu*.

Un boot réel est la seule chose qui puisse trancher. Ce fichier garde ce qu'il
dit — y compris, et surtout, ce qui échoue.

---

## Contexte du boot

| | |
|---|---|
| **Date** | *(à remplir)* |
| **ISO testée** | *(à remplir — nom de l'artefact + run CI)* |
| **Commit de l'image** | *(à remplir)* |
| **Base Fedora** | *(à remplir — F42 pour toute ISO antérieure au rebase)* |
| **Machine** | *(à remplir — VM Proxmox / matériel physique + modèle)* |
| **GPU** | *(à remplir — passthrough ? aucun ? RTX 3070 Ti ?)* |
| **Secure Boot** | *(à remplir — attendu : **désactivé**, la signature MOK est Phase 4)* |

---

## 1. Selfcheck automatique

**La commande — le script n'est PAS dans le `PATH` :**

```bash
sudo /usr/libexec/vibeos/vibeos-selfcheck.sh          # table lisible
sudo /usr/libexec/vibeos/vibeos-selfcheck.sh --json   # sortie machine
```

`exit 0` = aucun **FAIL** (les **SKIP** sont normaux : le script est tolérant aux
versions, une capacité absente d'une image plus ancienne est SKIP, jamais FAIL).

Reporte ici la sortie **telle quelle**. Les 17 checks, dans l'ordre où le script
les émet :

| Check | Invariant prouvé | PASS/FAIL/SKIP | Notes |
|---|---|---|---|
| `vibed-binary` | `/usr/bin/vibed` présent et exécutable | | |
| `root-readonly` | écrire sous `/usr` échoue (racine immuable) | | |
| `bootc-status` | déploiement bootc valide | | |
| `genesis-done` | `…/memory/.initialized` présent (Genesis 1er boot fait) | | |
| `vibed-service` | `vibed.service` **active (running)** | | |
| `user-vibed` | user `vibed` créé (sysusers.d) | | |
| `group-agents` | groupe `vibeos-agents` créé | | |
| `socket-present` | `/run/vibed/mcp.sock` présent | | |
| `socket-perms` | **0660 root:vibeos-agents** | | |
| `policy-default` | `/etc/vibeos/policy.d/default.toml` livré | | |
| `mcp-initialize` | handshake MCP répond | | |
| `mcp-tools-list` | catalogue d'outils répond | | |
| `mcp-os-status` | `os.status` (T0) répond | | |
| `mcp-denylist` | `fs.read /etc/shadow` **refusé** (denylist codée en dur, live) | | |
| `mcp-t2-floor` | `policy.check(svc.restart)` = `require_approval` (plancher T2) | | |
| `audit-present` | journal append-only présent | | |
| `audit-chain` | **chaîne SHA-256 intègre** (`vibectl audit verify`) | | |

**Exit code observé :** *(à remplir)*

### Ordre de démarrage

`vibed.service` déclare `After=vibeos-genesis.service`. Si `vibed` ne démarre pas,
**regarde Genesis d'abord** — c'est sa dépendance, pas l'inverse.

| Vérification | Commande | PASS/FAIL | Notes |
|---|---|---|---|
| Genesis a tourné avant vibed | `systemctl status vibeos-genesis` | | |
| vibed actif | `systemctl status vibed` | | |
| Socket présent | `ls -l /run/vibed/mcp.sock` | | |

---

## 2. Ce que le selfcheck ne peut PAS voir — il faut des yeux

Le selfcheck prouve des invariants système. Il ne rend rien à l'écran. Les lignes
ci-dessous sont **les seules** qu'un humain devant la machine peut trancher, et
c'est pour elles que ce boot compte.

| Élément | Attendu | OBSERVÉ | PASS/FAIL/SKIP | Notes |
|---|---|---|---|---|
| **HUD Quickshell** | Se rend en session Plasma. Autostart via `/etc/skel/.config/autostart/vibeos-hud.desktop` — donc **seulement pour l'utilisateur créé à l'installation**. `Meta+V` déplie/replie. | | | **LA couche jamais vue.** Son branchement au socket est *câblé*, jamais *rendu* |
| **HUD — données live** | Affiche l'état réel (`os.status`, `agents.list`…), pas des valeurs mockées | | | |
| **Splash Plymouth** | **Splash Fedora/Breeze standard.** Le thème VibeOS est copié mais **non activé** (activation = `plymouth-set-default-theme` + initramfs, Phase 5) | | | Voir le splash Fedora = **normal, pas un bug** |
| **SDDM** | Breeze | | | |
| **Session Plasma** | Démarre, Wayland | | | |

---

## 3. NVIDIA — lire la nuance avant de conclure

**Le verdict dépend de la machine.** Ne coche pas FAIL sans avoir tranché ça :

- **VM sans passthrough GPU** → `nvidia-smi` absent/en échec = **SKIP**, pas FAIL.
  Il n'y a pas de GPU à piloter. Aucune information.
- **Machine physique, Secure Boot désactivé** → le kmod doit charger. S'il ne
  charge pas, c'est un **vrai FAIL** et un finding à noter, pas à ignorer.
- **Secure Boot activé** → le module ne chargera pas : **attendu**, la signature
  MOK de nos kmods est Phase 4. Ce n'est pas un finding.

| Vérification | Commande | PASS/FAIL/SKIP | Notes |
|---|---|---|---|
| Module chargé | `lsmod \| grep nvidia` | | |
| Driver fonctionnel | `nvidia-smi` | | |
| CUDA visible par ollama | `ollama run …` (GPU vs CPU) | | |

---

## 4. Le piège de facturation — critère de sortie non coché

`ROADMAP.md` §Phase 2.5 porte ce critère, **non coché** :

> `- [ ]` *« `ANTHROPIC_API_KEY` positionnée globalement dans l'environnement
> système **ne prend pas** le pas silencieusement sur l'auth abonnement du
> superviseur (piège documenté des CLI elles-mêmes — vérifié explicitement). »*

**L'image ne pose cette variable nulle part** (vérifié : aucune occurrence dans
`os/`). Si elle apparaît, elle vient du shell de l'utilisateur.

| Vérification | Commande | Attendu | OBSERVÉ | PASS/FAIL |
|---|---|---|---|---|
| La variable est absente | `echo $ANTHROPIC_API_KEY` | **vide** | | |
| Auth par abonnement | `claude setup-token` (scope inference-only) | mécanisme natif de la CLI | | |
| Le critère tient | après login, l'usage passe par l'abonnement | | | |

---

## 5. Gouvernance réelle de Claude Code — à constater, pas à supposer

`docs/ARCHITECTURE.md` §8 pose comme **invariant n°1** : *« Aucun agent IA ne
contourne `vibed` : le socket MCP est l'unique surface de contrôle système exposée
aux agents. »*

`docs/DECISIONS.md` dit l'inverse pour le terminal : *« décision **Zed-only**, **le
terminal garde ses outils** »*. Les outils natifs `Read`/`Write`/`Edit`/`Bash` de
Claude Code tournent dans le sous-process du SDK — `vibed` n'en voit rien.

Ce boot est l'occasion de **constater** lequel des deux documents décrit la
réalité. Contradiction à trancher côté humain (`permissions.deny` s'appliquerait
via des settings **partagés**, donc au terminal aussi — arbitrage produit ouvert).

| Vérification | Commande | PASS/FAIL | Notes |
|---|---|---|---|
| MCP `vibeos` visible dans Claude Code | `/mcp` | | |
| `vibeos:fs.read ~/.ssh/id_rsa` | via l'outil MCP | **doit être refusé** (denylist) | |
| `Bash("cat ~/.ssh/id_rsa")` | outil natif | *(constat attendu : passe — hors gouvernance)* | Si ça passe, l'invariant n°1 est faux tel qu'écrit |

---

## 6. Findings

> Tout ce qui n'était pas prévu. Un boot qui ne produit aucun finding est
> suspect : c'est la première fois que ce code touche du matériel.

*(à remplir)*

---

## 7. Verdict

| | |
|---|---|
| **Le système boote** | *(à remplir)* |
| **`vibed` gouverne** | *(à remplir)* |
| **Le HUD se rend** | *(à remplir)* |
| **Bloquants découverts** | *(à remplir)* |
