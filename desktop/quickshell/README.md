# HUD Quickshell — le tableau de bord agents de VibeOS

> Statut : **prototype de design v0.1** — ce code QML est la maquette exécutable du HUD.
> Il n'est **pas garanti fonctionnel** sur une machine réelle tant que le paquet
> Quickshell (COPR, voir `docs/ECOSYSTEM.md`) n'est pas intégré à l'image et que le
> chantier bureau n'a pas fait une passe de test sur Plasma 6/Wayland. Toutes les
> données affichées en v0.1 sont **mockées** (voir §4). Règle D20 : rien ici ne
> prétend être branché sur `vibed` — ce branchement est la Phase 2.

---

## 1. Rôle

Le HUD est la signature visuelle de VibeOS : une barre fine, toujours visible,
**en plus** du panel Plasma (jamais à sa place), qui répond en un coup d'œil aux
trois questions du vibecoding :

1. **Qui travaille ?** — les agents actifs (Claude Code, opencode, aider…) et leur
   tier de policy courant (`AgentStatus.qml` + `PolicyTierIndicator.qml`).
2. **Qu'attendent-ils de moi ?** — un cadenas s'affiche dès qu'une action T2/T3
   attend une approbation humaine (sémantique « le tier est un plancher »,
   voir `docs/ARCHITECTURE.md` §4.2).
3. **Que consomme l'inférence locale ?** — modèle ollama chargé, VRAM, activité
   (`OllamaGauge.qml`), pensé pour les 8 Go d'une RTX 3070 Ti.

S'y ajoute l'état du démon (`vibed` en ligne / hors ligne) et de la mémoire
(`/var/lib/vibeos/memory` initialisée ou non, via `memory.query`).

Couleurs des tiers (fork Catppuccin Mocha « VibeOS Dark », MIT) :

| Tier | Nom | Couleur |
|---|---|---|
| T0 | observe | bleu `#89b4fa` |
| T1 | modify-user | vert `#a6e3a1` |
| T2 | modify-system | ambre `#fab387` + cadenas si approbation en attente |
| T3 | destructive | rouge `#f38ba8` + cadenas si approbation en attente |

## 2. Lancement et installation

Quickshell résout une configuration nommée « vibeos » ; en v0.1 cette
configuration est **fournie par l'image**, immuable. Le HUD se lance avec :

```sh
quickshell -c vibeos
```

Intégration dans l'image (rien de tout cela ne s'écrit dans `/usr` à l'exécution) :

- les fichiers QML de ce répertoire sont **livrés dans l'image** sous
  **`/usr/share/vibeos/quickshell/`** (contenu d'image immuable, jamais copié ni
  modifié à l'exécution). Seuls d'éventuels **réglages utilisateur** iraient dans
  `~/.config` ; aucun QML n'est déposé dans `/etc/skel` ;
- le démarrage automatique passe par un fichier autostart Plasma
  (`/etc/skel/.config/autostart/vibeos-hud.desktop`, `Exec=quickshell -c vibeos`) —
  livré par le chantier bureau, pas par ce répertoire ;
- le **paquet** `quickshell` (COPR, LGPL-3.0) est déclaré dans `os/Containerfile`,
  qui appartient à un autre chantier : ici on ne fait que le référencer.

Arrêt/relance à la main : `quickshell kill -c vibeos` puis relancer. Le HUD est une
couche additionnelle : le supprimer ne casse rien dans Plasma.

## 3. Comment le HUD lit l'état

Source de vérité unique : le **socket MCP de `vibed`**, `/run/vibed/mcp.sock`
(JSON-RPC 2.0 **délimité par lignes** — un objet JSON par ligne, exactement le
transport de `vibed/src/mcp.rs`). Le HUD est un client MCP comme un autre : il n'a
aucun chemin privilégié (invariant n°1 de `docs/ARCHITECTURE.md`).

Échange prévu (Phase 2) :

1. `initialize` → le serveur répond `protocolVersion: "2024-11-05"`,
   `serverInfo.name: "vibed"` ;
2. notification `notifications/initialized` ;
3. toutes les ~5 s, `tools/call` :
   - `os.status` (T0) → uptime, loadavg, mémoire, montages ;
   - `memory.query` (T0, `{"query": ""}`) → mémoire initialisée ? combien de fichiers ?

Le format exact des requêtes/réponses est documenté et implémenté (côté mock) dans
[`vibed_client.js`](vibed_client.js) — il doit rester aligné sur `vibed/src/mcp.rs`.

Prérequis d'accès : le socket est `root:vibeos-agents` en `0660` ; l'utilisateur de
session doit appartenir au groupe **`vibeos-agents`** pour que le HUD puisse s'y
connecter. Sinon → état « hors ligne » (voir §5), jamais une erreur.

## 4. Ce qui est mocké en v0.1 vs live en Phase 2

| Donnée | v0.1 (livré) | Phase 2 (cible) |
|---|---|---|
| État du démon | **mock : toujours « hors ligne »** (honnête : `/usr/bin/vibed` n'est pas dans l'image en Phase 1) | connexion réelle au socket, reconnexion périodique |
| Statut système | mock plausible (`mockOsStatus()`) | `tools/call os.status` via le socket |
| État mémoire | mock plausible (`mockMemoryQuery()`) | `tools/call memory.query` via le socket |
| Roster des agents + tiers | **mock assumé** : `vibed` v0.1 n'expose aucun outil `agents.list` — cette donnée n'est *pas encore dérivable* du démon | outil `agents.list` (ou flux dérivé des peer credentials de l'audit) à spécifier en Phase 2 |
| Cadenas « approbation en attente » | mock (déclenché dans le modèle de démo) | flux d'approbation T2/T3 (dialogue Plasma, Phase 2) |
| Jauge ollama / VRAM | mock (`mockOllama()`) | `ollama ps` (API locale `127.0.0.1:11434`) + `nvidia-smi --query-gpu=...` via `Quickshell.Io.Process` |

Le branchement Phase 2 utilisera `Quickshell.Io` (`Socket` sur le chemin Unix +
`SplitParser` pour le découpage en lignes) ; les emplacements exacts sont marqués
`TODO(Phase 2)` dans `shell.qml` et `vibed_client.js`.

## 5. Dégradation gracieuse — contrat ferme

Le HUD ne doit **jamais planter ni afficher de fausses données** quand `vibed`
est absent (c'est le cas nominal de la Phase 1) :

- socket inexistant, connexion refusée, groupe manquant, réponse invalide →
  pastille **« vibed hors ligne »** grise, agents remplacés par un texte neutre,
  jauges en état « — », zéro dialogue d'erreur ;
- le HUD réessaie en tâche de fond (Phase 2 : timer de reconnexion) ;
- si `ollama` ne répond pas, la jauge passe en « — » sans affecter le reste ;
- aucune écriture nulle part : le HUD est strictement **lecteur** (outils T0).

## 6. Fichiers

| Fichier | Rôle |
|---|---|
| `shell.qml` | racine Quickshell : `PanelWindow` (barre haute) qui compose les widgets, détient l'état et le point de branchement Phase 2 |
| `AgentStatus.qml` | chips des agents actifs + tier courant |
| `PolicyTierIndicator.qml` | badge T0–T3 (couleurs VibeOS) + cadenas d'approbation |
| `OllamaGauge.qml` | activité modèle local + barre VRAM |
| `vibed_client.js` | formats JSON-RPC du socket MCP + données mock v0.1 |
