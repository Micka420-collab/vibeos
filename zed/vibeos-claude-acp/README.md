# `vibeos-claude-acp` — l'agent Zed gouverné par VibeOS

Extension de gouvernance pour l'adaptateur ACP de Claude Code
(`@agentclientprotocol/claude-agent-acp`), qui fait tourner Claude Code comme
agent [ACP](https://agentclientprotocol.com) dans l'éditeur [Zed](https://zed.dev).

Décision et invariants : [`docs/DECISIONS.md` ADR-014](../../docs/DECISIONS.md) ·
feuille de route : [`ROADMAP.md` §9 bis](../../ROADMAP.md).

## Ce que ça fait

Sur VibeOS, **toute action système d'un agent passe par le moteur de politiques
de `vibed`** (tiers T0–T3, audit, approbation humaine T2/T3) — y compris quand
l'agent tourne dans l'éditeur. Cette extension apporte deux choses :

1. **Couche 1 — outils fichiers natifs désactivés (par config)** : `Read`/`Write`/
   `Edit` natifs de Claude Code sont refusés via `permissions.deny`, dans un
   `CLAUDE_CONFIG_DIR` **propre à la session Zed** (le Claude Code en terminal
   garde ses outils). L'agent est orienté vers les outils gouvernés `vibeos:fs.*`/
   `memory.query` (serveur MCP `vibed`).
2. **Couche 2 — mode auto gouverné (le fork)** : le prompt de permission de
   l'éditeur est remplacé par la **décision de `vibed`**. L'extension patche
   `ClaudeAcpAgent.prototype.canUseTool` : pour un appel `vibeos:*`, elle
   interroge l'outil T0 `vibeos:policy.check` ; un `Allow` (T0/T1) **s'exécute
   sans prompt**, tout le reste (`deny`, `require_approval` = T2/T3, ou un échec
   de la vérification) **retombe sur le prompt humain normal**.

## Ce qui rend ça innovant

- **Mode auto piloté par un moteur de politiques, pas par un classifieur de
  modèle.** Le mode `auto` amont approuve/refuse via un classifieur LLM ; ici la
  décision est **déterministe, auditée, par tier** — aucun autre agent ACP ne
  délègue ses permissions à un moteur de politiques système.
- **Fail-safe par construction.** Si `vibed` est injoignable, l'extension
  **prompte** (jamais d'auto-allow) — l'incertitude ne relâche jamais la garde.
- **Plancher T2/T3 jamais levé, même côté éditeur.** Le mode auto ne saute le
  prompt QUE pour ce que la politique classe déjà `Allow` ; il **ne touche jamais**
  le store d'approbation (`approval.rs`) et ne décide jamais lui-même d'un T2/T3.
  C'est un **indice** : `vibed` ré-applique la décision à l'exécution — même un
  `policy.check` erroné ne peut laisser passer un T2/T3 sans humain.
- **Gouvernance unifiée** : la même politique `vibed` gouverne l'agent en terminal
  et dans l'éditeur.

### Fonctionnalités innovantes — feuille de route

Le cœur gouverné ci-dessus est **livré et testé**. Extensions conçues, à câbler
sur une vraie session Zed (rendu d'outils de l'adaptateur `tools.ts`) :

- **Badges de tier inline** : chaque appel gouverné affiche son tier T0–T3
  (couleur) dans le fil d'outils de Zed — la gouvernance visible à chaque action.
- **Panneau de raisonnement** : le raisonnement capté par le superviseur
  (ADR-012, `agent.thinking`) affiché dans l'éditeur.
- **Inbox d'approbation asynchrone** : les demandes T2/T3 en attente présentées
  dans un panneau Zed (l'approbation réelle reste `vibectl`, root — jamais l'agent).
- **Amorçage mémoire** : `vibeos:memory.query` au démarrage de session pour
  réhydrater le contexte machine (« reprends le projet d'hier »).

## Forme du fork (pourquoi un patch de prototype)

Vérifié sur le source amont (ADR-014 § « Structure de l'adaptateur ») :
`ClaudeAcpAgent` est exporté et `canUseTool` est **public**, mais `createSession`
est **privé** et `runAcp()` construit la classe de base en interne avec des
internes **non exportés**. Un sous-classement ne peut donc pas s'injecter dans
`runAcp`. On **patche le prototype de `canUseTool`** — la seule surface publique
nécessaire — puis on appelle `runAcp()`. Surface de fork minimale, **rebasable
par un simple bump** de la dépendance amont.

## Build / test

```sh
npm install            # récupère l'adaptateur amont (Apache-2.0)
npm run build          # tsc -> dist/ (compile contre les vrais types amont)
npm test               # vitest — logique du mode auto + mapping d'outils
```

Statut : **`tsc` compile** contre l'amont et **les tests unitaires passent** ; le
**test d'intégration en session Zed réelle** (nommage exact `mcp__vibeos__*`,
expansion de `CLAUDE_CONFIG_DIR`, boucle de permission) reste à faire — rien ici
n'est décrit comme fonctionnel en bout-en-bout tant que non validé sur Zed.

## Configuration (livrée dans l'image)

- `os/rootfs/etc/skel/.config/zed/settings.json` : `agent_servers` lance
  `vibeos-claude-acp` avec `CLAUDE_CONFIG_DIR` pointant vers…
- `os/rootfs/etc/skel/.config/vibeos/zed-claude/settings.json` : le
  `CLAUDE_CONFIG_DIR` **propre à Zed** — `permissions.deny: [Read, Write, Edit]`
  (couche 1) + le serveur MCP `vibeos`.

Variable d'env : `VIBED_MCP_SOCKET` (défaut `/run/vibed/mcp.sock`).
