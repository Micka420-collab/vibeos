# BLOCKERS — VibeOS

Ce qui est **réellement bloqué** (et par quoi), pour que la prochaine session ne
ré-accepte pas un blocage sans le re-vérifier. Mis à jour le 2026-07-13.

## « VibeOS pour Zed » — intégration bout-en-bout

**Re-vérification faite (2026-07-13).** Le blocage annoncé « intégration Zed hors
de portée » était **partiel**. L'extension `zed/vibeos-claude-acp` est un **agent
ACP** (process Node parlant ACP sur stdio), pas un plugin d'éditeur — elle se
valide donc **sans Zed ni display** :

- ✅ **Validé sans Zed** : `tsc` compile contre les vrais types amont ; le patch
  de `ClaudeAcpAgent.prototype.canUseTool` s'applique au runtime (vérifié) ; le
  boot ACP headless répond à un vrai `initialize` sans crash (`npm run smoke`,
  `scripts/smoke-acp.mjs`) ; la logique du gate est déterministe et LLM-free
  (17 tests vitest, dont la preuve de déterminisme + le client MCP socket contre
  un faux vibed).

- ⛔ **Reste bloqué : le end-to-end complet** (un vrai prompt → la SDK spawn le
  binaire Claude natif → un appel d'outil → `canUseTool` → `vibeos:policy.check`
  → `vibed`). Ce qui manque, précisément :
  1. **Le binaire natif du Claude Agent SDK** n'est pas présent dans cet
     environnement (l'adaptateur le résout via `pathToClaudeCodeExecutable` /
     `claudeCliPath()` ; il n'est spawné qu'au `session/new`, pas à `initialize`
     — d'où le smoke qui passe quand même).
  2. **Un `vibed` démarré** avec le socket `/run/vibed/mcp.sock` ET l'outil
     `policy.check` servi (nécessite l'OS booté ou un vibed lancé localement).
  3. **Un client ACP complet** qui pilote un `session/new` + un `prompt` avec un
     appel d'outil réel — soit **Zed** (non installé ici : `which zed` → absent ;
     Zed est un éditeur GPU/Wayland, pas prévu pour tourner headless en WSL), soit
     un harnais ACP maison qui rejoue la séquence.
  4. **Nommage exact `mcp__vibeos__*`** + **expansion de `CLAUDE_CONFIG_DIR`** dans
     l'`env` de Zed : à confirmer sur une vraie session (le fail-safe fait que si
     le nommage diffère, on prompte — jamais d'auto-allow erroné).

**Prochaine étape recommandée** : sur une machine avec Zed + un `vibed` local
(ou l'OS booté), lancer une session, déclencher un appel `vibeos:fs.read` (T0) et
un `pkg.install` (T2), vérifier que le premier ne prompte pas et le second si.
Le harnais headless (`scripts/smoke-acp.mjs`) peut être étendu en client ACP
complet pour automatiser ça sans éditeur, une fois le binaire Claude + un vibed
disponibles.

## Rappels (blocages connus, non régressés)

- **Validation VM / ISO** (Phase 1) : boot amd64+arm64, NVIDIA, `ollama run`
  hors-ligne, `bootc upgrade/rollback` — exigent une vraie machine.
- **Câblage de l'extension dans l'image** : délibérément **non fait** tant que le
  end-to-end n'est pas validé (ne pas ship ~148 paquets npm non éprouvés dans
  l'image immuable). Plan supply-chain : voir `docs/DECISIONS.md` ADR-014.
