# VALIDATION — valider VibeOS sur une vraie machine

> One-stop guide pour valider une image VibeOS bootée. Trois paliers, du plus
> automatique au plus manuel. Le CI prouve déjà le code (fmt/build/test/clippy,
> smoke Genesis, lint policy, build ISO) ; ce document couvre **ce que seul un
> boot réel prouve**. Rien ici ne tourne en CI faute de matériel (TPM/GPU/UEFI) —
> voir `BLOCKERS.md`.
>
> **Note pile** : le selfcheck (`vibeos-selfcheck`) est livré par CETTE branche
> (base `main`). Les composants Zed E2E, `BLOCKERS.md`, unité agent TPM2/egress et
> `vibectl` arrivent avec la couche sécurité (pile PR #11→#13) ; le selfcheck est
> **tolérant** : il les marque SKIP tant qu'ils sont absents.

## Tier A — automatique, sur la machine bootée (10 min)

### A1. Selfcheck système — `vibeos-selfcheck`

Un seul script embarqué assère d'un coup les invariants systèmes. **Lecture
seule** : aucun redémarrage, aucune écriture, aucune approbation (le seul test T2
est un `policy.check` à blanc, jamais une vraie mutation).

```bash
sudo /usr/libexec/vibeos/vibeos-selfcheck.sh          # table lisible
sudo /usr/libexec/vibeos/vibeos-selfcheck.sh --json    # sortie machine (CI/QEMU)
```

Sortie : `exit 0` si aucun check n'a **FAIL** (les **SKIP** sont normaux quand une
capacité n'existe pas encore dans l'image), `exit 1` sinon. Ce qu'il vérifie :

| Check | Invariant prouvé |
|---|---|
| `vibed-binary` | `/usr/bin/vibed` présent et exécutable |
| `root-readonly` | écrire sous `/usr` échoue (racine immuable) |
| `bootc-status` | déploiement bootc valide |
| `genesis-done` | `…/memory/.initialized` présent (Genesis 1er boot fait) |
| `vibed-service` | `vibed.service` **active (running)** |
| `user-vibed` / `group-agents` | user `vibed` + groupe `vibeos-agents` (sysusers.d) |
| `socket-present` / `socket-perms` | `/run/vibed/mcp.sock` présent, **0660 root:vibeos-agents** |
| `policy-default` | `/etc/vibeos/policy.d/default.toml` livré |
| `mcp-initialize` / `mcp-tools-list` | la surface MCP répond (handshake + catalogue) |
| `mcp-os-status` | `os.status` (T0) répond |
| `mcp-denylist` | `fs.read /etc/shadow` **refusé** (denylist codée en dur, live) |
| `mcp-t2-floor` | `policy.check(svc.restart)` = `require_approval` (plancher T2) |
| `audit-present` / `audit-chain` | journal append-only présent + **chaîne SHA-256 intègre** (`vibectl audit verify`) |

Le script est **tolérant aux versions** : une capacité absente d'une image plus
ancienne (`vibectl`, `policy.check`…) est **SKIP**, jamais FAIL.

### A2. Gouvernance éditeur (extension Zed) — `e2e-zed.sh`

```bash
zed/vibeos-claude-acp/scripts/e2e-zed.sh          # Tier A auto + checklist Tier B
```

Rejoue le vrai `checkPolicy` de l'extension contre un vibed live
(`e2e-live-policy.mjs`) : T0 → allow/auto, T2 (`pkg.install`/`svc.restart`) →
require_approval **jamais** auto, outil inconnu → deny. Voir
`BLOCKERS.md` §Zed pour le Tier B (round-trip complet dans Zed).

## Tier B — semi-manuel : desktop + éditeur (needs Zed, GPU/Wayland)

Ne s'automatise pas (Zed n'est pas headless). Après login SDDM → Plasma 6 :

1. **Bureau** : session Plasma 6 Wayland, thème VibeOS Dark + wallpaper, HUD
   Quickshell au 1er login (données live une fois le socket QML branché).
2. **Zed round-trip** : ouvrir Zed, lancer une session agent gouvernée ; vérifier
   qu'un appel T2 déclenche l'invite d'approbation et qu'un refus bloque l'action.
   Prérequis restant : binaire natif Claude Agent SDK + `WITH_ZED_AGENT=1` dans
   l'image (voir `BLOCKERS.md`).

## ⭐ Tier A0 — les 3 derniers critères de sortie de la **Phase 2** (~10 min)

> **À faire en premier sur l'ISO `v0.2.0-dev`.** Les 4 autres critères de Phase 2
> sont déjà vérifiés mécaniquement en CI ([ROADMAP.md](../ROADMAP.md) §4) ; **ces
> trois-là ne sont pas « à faire » mais « à CONSTATER »** — ils exigent un boot
> réel, et rien d'autre ne les bloque.
>
> **Pourquoi le selfcheck (A1) ne suffit pas** : il est **read-only par
> conception** (aucun redémarrage, aucune écriture) et fait son handshake MCP
> **lui-même**. Il ne peut donc prouver ni le `kill -9`, ni une écriture T1, ni
> que **le vrai Claude Code** parle au vrai socket. D'où ce palier.

### C1 — `vibed` sain, et redémarre proprement après `kill -9`

```bash
systemctl status vibed                              # attendu : active (running)
systemctl show vibed -p Restart                     # attendu : Restart=on-failure
PID=$(systemctl show vibed -p MainPID --value); echo "avant : $PID"
sudo kill -9 "$PID"                                 # SIGKILL = échec => on-failure doit relancer
sleep 3
systemctl show vibed -p MainPID --value             # attendu : un PID DIFFÉRENT et non nul
systemctl status vibed                              # attendu : active (running)
test -S /run/vibed/mcp.sock && echo "socket recréé OK"
```

**Réussi si** : nouveau PID ≠ ancien, service `active (running)`, socket recréé.
**Piège à surveiller** : si le service reste `failed`, regarder
`journalctl -u vibed -n 50` — un `exit(1)` fail-closed sur politique invalide est
un SUCCÈS du design, pas un plantage (voir C1bis).

### C2 — Handshake MCP depuis le **vrai** Claude Code

Prérequis : ton utilisateur doit être dans le groupe `vibeos-agents`
(`id -nG | grep vibeos-agents` ; sinon `sudo usermod -aG vibeos-agents $USER`
puis **rouvrir la session**). L'image livre déjà la config MCP
(`/etc/skel/.claude.json` → `~/.claude.json`) : **aucune configuration manuelle**.

```bash
claude          # puis, dans l'invite :
/mcp            # attendu : serveur « vibeos » CONNECTÉ + catalogue d'outils listé
```

**Réussi si** : `vibeos` apparaît connecté et la liste montre les outils T0/T1
(`os.status`, `fs.read`, `fs.list`, `svc.status`, `log.read`, `memory.query`,
`agent.*`, `policy.check`, `fs.write`, `memory.append`).
**Si ça échoue** : `ls -l /run/vibed/mcp.sock` (doit être `0660 root:vibeos-agents`)
et `command -v socat` (le transport de la config livrée).

### C3 — Démo bout-en-bout T0 + T1, les deux tracées à l'audit

Dans la même session `claude` :

1. **T0** — demander : *« lis l'état du système avec l'outil vibeos »*
   → doit répondre **sans aucune demande d'approbation** (T0 = lecture seule).
2. **T1** — demander : *« écris "bonjour vibeos" dans ~/demo-phase2.txt »*
   → doit réussir (`fs.write` est T1, confiné à TON home).

Puis, **hors de l'agent**, constater la trace :

```bash
cat ~/demo-phase2.txt                               # attendu : bonjour vibeos
sudo tail -5 /var/lib/vibeos/audit/vibed-$(date -u +%F).jsonl
sudo vibectl audit verify                           # attendu : "ok": true
```

**Réussi si** : le fichier existe, et l'audit contient **les deux** appels
(`os.status` **et** `fs.write`) avec `decision":"allow"`, ton `caller_uid`, et
`target` = le **chemin réel** du fichier écrit.

**Bonus — vérifier que la frontière tient vraiment** (30 s, aucun risque) :
```
Dans claude, demander : « lis /etc/shadow »        → attendu : REFUSÉ (denylist)
Dans claude, demander : « installe htop »          → attendu : approbation requise,
                                                      JAMAIS exécuté tout seul
sudo vibectl approvals list                        # la demande T2 en attente
```
C'est le cœur du projet : un agent ne franchit pas T2 sans toi.

### Reporter le résultat

Chaque critère constaté → cocher sa case dans [ROADMAP.md](../ROADMAP.md) §4
« Critères de sortie », **avec la preuve** (sortie de commande). Les 3 cochés =
**Phase 2 close**.

## Tier C — matériel : ce que seul le bare-metal prouve

1. **Boot** amd64 et arm64 en VM UEFI (QEMU/OVMF ou Hyper-V Gén.2) jusqu'à
   SDDM+Plasma — procédure VM détaillée dans [BUILD.md](BUILD.md) §« Tester l'ISO
   en VM » (Secure Boot + vTPM `Enable-VMTPM`).
2. **Immuabilité + mises à jour** : `touch /usr/test` échoue ; `bootc status` OK ;
   `bootc upgrade` atomique ; `bootc rollback` restaure le déploiement précédent.
3. **NVIDIA** (amd64) : `nvidia-smi` fonctionnel, offload CUDA `ollama run` hors
   ligne, Plasma fluide en Wayland (voir [HARDWARE.md](HARDWARE.md)).
4. **TPM2 + egress** : unseal du token d'abonnement via `systemd-creds` (le clair
   ne touche jamais le disque) ; agent en `User=%i` (jamais root) ; egress
   default-deny (`IPAddressDeny=any` + allowlist résolue). Enforcement live =
   machine bootée avec un vrai TPM.
5. **Secure Boot / chaîne de confiance** (Phase 4) : UKI + dm-verity ; une image
   altérée **refuse de démarrer**.
6. **Dual-boot** : l'installation ne doit pas écraser Windows.

## Tester le selfcheck sans machine (simulation)

Le selfcheck accepte des overrides pour être rejoué contre un vibed **scratch**
(rootless), utile en dev/CI avant qu'une machine soit disponible :

```bash
# lance un vibed jetable, puis :
VIBEOS_SELFCHECK_SOCKET=/tmp/scratch/mcp.sock \
VIBEOS_SELFCHECK_POLICY_DIR=/tmp/scratch/policy.d \
VIBEOS_SELFCHECK_AUDIT_DIR=/tmp/scratch/audit \
VIBEOS_SELFCHECK_ASSUME_IMAGE=0 \
  /usr/libexec/vibeos/vibeos-selfcheck.sh
```

`ASSUME_IMAGE=0` fait passer en **SKIP** (plutôt que FAIL) les composants système
absents hors d'une vraie image, de sorte que seules les assertions MCP/policy/
audit s'exécutent réellement. C'est ainsi que le script est validé en l'absence de
matériel : `socat`/`jq` + un vibed scratch suffisent pour exercer toute la surface
MCP ; les checks purement systèmes (service actif, droits socket, racine RO) ne
sont vrais que sur la machine cible.

## Références

- `BLOCKERS.md` — ce qui reste bloqué faute de matériel + procédure Zed
- [BUILD.md](BUILD.md) §Tester l'ISO en VM — création de la VM + checklist 1er boot
- [HARDWARE.md](HARDWARE.md) — checklist sur le PC de référence (NVIDIA, dual-boot)
- [ROADMAP.md](../ROADMAP.md) — critères de sortie Phase 1/3/4 (boot, LUKS, Secure Boot)
- `vibed/README.md` §Tester le socket — recette `socat`/Python + comportements attendus
