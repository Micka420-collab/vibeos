# HUD Quickshell — le tableau de bord agents de VibeOS

> Statut : **runtime livré, données live-câblées, validation visuelle en attente.**
> Le runtime Quickshell est désormais **compilé dans l'image** (étage
> `quickshell-builder` d'`os/Containerfile` — le paquet Fedora `quickshell` existe depuis f44 et la base est désormais f44 : cet étage est **redondant** et son retrait est suivi à part — un rebase de sécurité ne change pas aussi la provenance d'un composant livré)
> et le HUD est **auto-démarré** en session Plasma
> (`/etc/skel/.config/autostart/vibeos-hud.desktop` → `/usr/bin/vibeos-hud`).
> Le QML est **branché en direct** sur le socket de `vibed` (`Quickshell.Io`, voir
> §4) — plus aucune donnée mockée dans le HUD ; ce qui reste est la **validation
> visuelle** sur un Plasma booté (pas de chemin headless). Règle D20 : rien ici ne
> prétend être branché sur `vibed` tant que ce n'est pas le cas — le HUD affiche
> « vibed hors ligne » dès que le socket est injoignable.
>
> **Langage visuel.** Le HUD applique à la lettre le système de design
> [`docs/DESIGN-SYSTEM.md`](../../docs/DESIGN-SYSTEM.md) : verre frosté (glass-panel,
> Crust 66 % + flou), pastilles/anneaux de tiers en dégradé, accent Mauve tenu,
> typographie mono pour la donnée, mouvement mesuré. **Aucune valeur en dur** :
> tous les composants référencent le singleton de tokens `Theme.qml` (couleurs,
> rayons, espacements, durées, courbes, dégradés). C'est la clé de la cohérence
> boot → login → bureau → HUD → terminal.

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

La configuration du HUD est **fournie par l'image**, immuable, sous
`/usr/share/vibeos/quickshell/`. Quickshell ne résout les configurations
*nommées* (`-c`) que depuis `$XDG_CONFIG_HOME` : le HUD se lance donc **par
chemin explicite**, via le lanceur livré dans l'image :

```sh
vibeos-hud                     # = quickshell -p /usr/share/vibeos/quickshell
```

Intégration dans l'image (rien de tout cela ne s'écrit dans `/usr` à l'exécution) :

- les fichiers QML de ce répertoire sont **livrés dans l'image** sous
  **`/usr/share/vibeos/quickshell/`** (contenu d'image immuable, jamais copié ni
  modifié à l'exécution). Pour personnaliser : copier le dossier dans
  `~/.config/quickshell/vibeos` et lancer `quickshell -c vibeos` — aucun QML
  n'est déposé dans `/etc/skel` ;
- le démarrage automatique est **livré** : autostart Plasma
  `/etc/skel/.config/autostart/vibeos-hud.desktop` (`Exec=/usr/bin/vibeos-hud`,
  `TryExec=quickshell`, `OnlyShowIn=KDE`) — supprimer ce fichier de son `$HOME`
  désactive le HUD ;
- le **runtime** `quickshell` (LGPL-3.0) est **compilé depuis les sources** dans
  `os/Containerfile` (étage `quickshell-builder`, recette du spec Fedora
  officiel, version épinglée + sha256) : **aucun paquet n'existe pour Fedora
  42** — le paquet officiel Fedora commence à f44, aucun COPR n'a de chroot
  f42 (vérifié 2026-07-08).

Arrêt : `quickshell kill -p /usr/share/vibeos/quickshell` ; relance : `vibeos-hud`.
Le HUD est une couche additionnelle : le supprimer ne casse rien dans Plasma.

## 3. Comment le HUD lit l'état

Source de vérité unique : le **socket MCP de `vibed`**, `/run/vibed/mcp.sock`
(JSON-RPC 2.0 **délimité par lignes** — un objet JSON par ligne, exactement le
transport de `vibed/src/mcp.rs`). Le HUD est un client MCP comme un autre : il n'a
aucun chemin privilégié (invariant n°1 de `docs/ARCHITECTURE.md`).

Échange (implémenté dans `shell.qml`) :

1. `initialize` → le serveur répond `protocolVersion: "2024-11-05"`,
   `serverInfo.name: "vibed"` ;
2. notification `notifications/initialized` ;
3. toutes les ~5 s, `tools/call` (tous T0, lecture seule) :
   - `os.status` → uptime, loadavg, mémoire, montages ;
   - `memory.query` (`{"query": ""}`) → mémoire initialisée ? combien de fichiers ?
   - `agents.list` → roster confiné à l'uid appelant ;
   - `agent.sessions` → liste datée des sessions (métadonnées : début, dernière
     activité, poids) ; `agent.thinking` → raisonnement de la session courante.
     Une session **passée** n'est lue qu'à la sélection de l'utilisateur, jamais
     dans la boucle de poll.

Le format exact des requêtes/réponses et le mappage du raisonnement vivent dans
[`vibed_client.js`](vibed_client.js) — ils doivent rester alignés sur `vibed/src/mcp.rs`.

Prérequis d'accès : le socket est `root:vibeos-agents` en `0660` ; l'utilisateur de
session doit appartenir au groupe **`vibeos-agents`** pour que le HUD puisse s'y
connecter. Sinon → état « hors ligne » (voir §5), jamais une erreur.

## 4. Ce qui est live (données réelles du socket)

| Donnée | Statut | Source |
|---|---|---|
| État du démon | ✅ **live** | connexion réelle au socket (`Quickshell.Io.Socket`), sonde de reconnexion périodique |
| Statut système | ✅ **live** | `tools/call os.status` (T0) via le socket, poll 5 s |
| État mémoire | ✅ **live** | `tools/call memory.query` (T0) via le socket |
| Raisonnement des agents | ✅ **live** | `agent.sessions` (découverte + métadonnées de session) → `agent.thinking` (T0) via le socket ; `[]` tant qu'aucune session autonome n'a capté de raisonnement |
| Historique des sessions | ✅ **live** | `agent.sessions` (T0) : date de début, durée, poids par session, la plus récente d'abord ; sélectionner une session charge son raisonnement via `agent.thinking(session_id)`. Pas de fournisseur/modèle ni de compteur de tours — le store de raisonnement ne les porte pas (cf. ADR-012) |
| Roster des agents + tiers | ✅ **live** | `agents.list` (T0, **confiné à l'uid appelant**, dérivé de l'audit, groupé par pid, soi-même exclu) via le socket ; `name` best-effort `/proc/<pid>/comm` |
| Cadenas « approbation en attente » | ✅ **live** | `awaiting_approval` d'`agents.list` (une demande T2/T3 en attente pour l'uid) |
| Jauge ollama / VRAM | ✅ **live** (probe local) | `GET 127.0.0.1:11434/api/ps` (modèle) + `nvidia-smi` via `Quickshell.Io.Process` (VRAM) — indépendant de vibed, `available:false` en cas d'échec |

Le branchement live utilise `Quickshell.Io` (`Socket` sur le chemin Unix +
`SplitParser` pour le découpage en lignes) dans `shell.qml` ; les formes de
requête/réponse et le mappage du raisonnement vivent dans `vibed_client.js`.
**Non vérifié au runtime ici** : Quickshell n'est pas lançable headless — le
câblage est construit d'après le pattern documenté et la forme réelle des
payloads `vibed` ; la validation visuelle attend une session graphique.

## 5. Dégradation gracieuse — contrat ferme

Le HUD ne doit **jamais planter ni afficher de fausses données** quand `vibed`
est absent (c'est le cas nominal de la Phase 1) :

- socket inexistant, connexion refusée, groupe manquant, réponse invalide →
  pastille **« vibed hors ligne »** grise, agents remplacés par un texte neutre,
  jauges en état « — », zéro dialogue d'erreur ;
- le HUD réessaie en tâche de fond (timer de reconnexion, sonde 4 s hors-ligne) ;
- si `ollama` ne répond pas, la jauge passe en « — » sans affecter le reste ;
- aucune écriture nulle part : le HUD est strictement **lecteur** (outils T0).

## 6. Fichiers

| Fichier | Rôle |
|---|---|
| `Theme.qml` | **singleton de tokens** (source de vérité unique du design) : palette Mocha invariante, échelle de surfaces/élévation, rôles de texte, verre, rayons, espacements 4pt, dégradés signature/tiers, durées & courbes de mouvement, glows, drapeaux d'accessibilité. Tous les autres fichiers lisent `Theme.*` |
| `shell.qml` | racine Quickshell : `PanelWindow` frosté (barre haute verre, ombre d'élévation, marque + état global + triptyque) qui compose les widgets, détient l'état et le point de branchement Phase 2 |
| `AgentStatus.qml` | chips d'agents élevés : anneau-avatar signature (Mauve→Blue), nom, pastille de tier, élévation au survol, tooltip verre (activité/projet/durée) |
| `PolicyTierIndicator.qml` | pastille de tier en dégradé + anneau conique T0–T3, glyphe cadenas dessiné et pulsation douce quand T2+ attend une approbation |
| `ReasoningPanel.qml` | 3ᵉ pilier « pourquoi » : chip + popup verre du **raisonnement des agents** (live streaming + historique par session), toggle de capture, note de transparence (résumé fournisseur vs brut local). **`live` et `history` câblés** (règle d'honnêteté : ship avec `[]`, alimentés en direct par `agent.sessions`→`agent.thinking`, jamais le transcript CLI — `docs/DECISIONS.md` ADR-012) ; sélectionner une session passée émet `historySelected` → `shell.qml` charge son raisonnement à la demande |
| `OllamaGauge.qml` | anneau VRAM circulaire à dégradé (arc Canvas, cap arrondi) + modèle chargé + Gio, seuils Sky→Peach→Red, « — » honnête hors ligne |
| `vibed_client.js` | formats JSON-RPC du socket MCP (alignés sur `vibed/src/mcp.rs`) : constructeurs de requêtes + parseurs (`os.status`, `memory.query`, `agents.list`, `agent.sessions`/`agent.thinking`) + mappeurs `reasoningToLive` / `sessionsToHistory`. **`.pragma library`** → aucun accès à l'objet QML `Qt` : le formatage (dates/durées/octets) y est en ECMAScript pur, table des mois FR incluse, pour un rendu déterministe |

> **Cohérence par les tokens.** `Theme.qml` transcrit `DESIGN-SYSTEM.md §12.2` et
> l'étend (élévation, glows, dégradés, aides de tiers, drapeaux a11y). Cible image :
> `/usr/share/vibeos/quickshell/Theme.qml`. Quickshell enregistre automatiquement les
> singletons du répertoire de configuration ; les composants voisins y accèdent
> directement via le type `Theme`.
