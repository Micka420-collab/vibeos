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

- ✅ **Tier A validé sur socket vibed LIVE (2026-07-13)** : `scripts/e2e-live-policy.mjs`
  fait tourner le **vrai `checkPolicy`** de l'extension (celui qu'appelle le
  `canUseTool` patché) contre un `vibed` réellement démarré, et prouve le contrat
  de gouvernance de bout en bout côté décision : `fs.read`/`fs.list` (T0) →
  `allow` → auto-allow (pas de prompt éditeur) ; `pkg.install`/`svc.restart` (T2)
  → `require_approval` → **jamais** auto-allow (l'éditeur prompte) ; outil inconnu
  → `deny`. Lançable rootless via les overrides `VIBED_SOCKET`/`VIBED_POLICY_DIR`/
  `VIBED_AUDIT_DIR` — voir `scripts/e2e-zed.sh`.

- ⛔ **Reste bloqué : le Tier B (round-trip éditeur complet)** — un vrai prompt →
  la SDK spawn le binaire Claude natif → un appel d'outil → `canUseTool` →
  `vibeos:policy.check` → `vibed`, avec **Zed qui supprime réellement le prompt**
  pour un Allow et l'affiche pour un `require_approval`. Ce qui manque, précisément :
  1. **Le binaire natif du Claude Agent SDK** (spawné au `session/new`, pas à
     `initialize` — d'où le smoke + le Tier A qui passent sans lui).
  2. **Zed** (éditeur GPU/Wayland, pas prévu headless en WSL) pour piloter la
     session — ou un client ACP maison rejouant `session/new` + `prompt`.
  3. **Nommage exact `mcp__vibeos__*`** + **expansion de `CLAUDE_CONFIG_DIR`** dans
     l'`env` de Zed : à confirmer sur une vraie session (fail-safe : si le nommage
     diffère, on prompte — jamais d'auto-allow erroné).

**Turnkey — prochaine étape** : sur une machine avec Zed, lancer
`zed/vibeos-claude-acp/scripts/e2e-zed.sh` tel quel. Il exécute le Tier A
automatiquement (build + bundle + décisions live) puis écrit un `settings.json`
Zed et imprime la checklist Tier B (ouvrir Zed, session, `fs.read` sans prompt,
`pkg.install` qui prompte, vérif via `vibectl approvals`/audit).

## Rappels (blocages connus, non régressés)

- **Validation VM / ISO** (Phase 1) : boot amd64+arm64, NVIDIA, `ollama run`
  hors-ligne, `bootc upgrade/rollback` — exigent une vraie machine.
- **Câblage de l'extension dans l'image** : délibérément **non fait** tant que le
  end-to-end n'est pas validé (ne pas ship ~148 paquets npm non éprouvés dans
  l'image immuable). Plan supply-chain : voir `docs/DECISIONS.md` ADR-014.
