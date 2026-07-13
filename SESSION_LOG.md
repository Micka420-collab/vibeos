# SESSION_LOG — session autonome du 2026-07-13

> Journal de la session de travail autonome (matin → 23h). Point d'entrée
> permanent : [STATUS.md](STATUS.md). Branche : `worktree-amelioration-2026-07-13`.
> PR principale : **[PR #11](https://github.com/Micka420-collab/vibeos/pull/11)**
> (branche → `main`, MERGEABLE). La [PR #4](https://github.com/Micka420-collab/vibeos/pull/4)
> (empilée sur les draft PRs Phase 2, cible `phase2-supply-chain`) est **antérieure
> et superseded** par #11 ; son sort (fermeture) est laissé à l'humain.

## ✅ Fait (livré, testé, poussé)

**Session étendue (16h → 23h, 2026-07-13)** :
- **Correctifs auto-signalés** : approval fs I/O (`check_and_consume_grant`/
  `request_approval`) déplacée sur `tokio::spawn_blocking` ; grant-consommé-
  avant-audit **tranché et documenté** (garde la garantie one-shot).
- **ADR-012 implémenté** : module `reasoning` (store `memory/reasoning/
  <session>.jsonl`, `safe_session_id` anti-traversal), outil MCP **T0
  `agent.thinking`**, Genesis crée `reasoning/`.
- **ADR-012/013 — superviseur** `vibectl agent run/stop/thinking` : tap
  `stream-json` → store, budgets wall-clock + nb d'appels, kill-switch
  opérateur (marqueur `.stop`), type de journal réservé `autonomous_session`,
  groupe de processus + group-kill + drain borné (ne se suspend jamais).
  **N'approche jamais `approval.rs`** ; T2/T3 restent gérés par vibed.
- **Revue adversariale indépendante** (sous-agent) : **aucun bug high/medium**,
  contrat de sécurité intact (traversal, denylist, type réservé, plancher T2/T3,
  surface opérateur-only). 5 items availability/robustesse durcis — **traçabilité
  finding → commit → test** :

  | # | Finding (sévérité) | Correctif | Commit | Test qui le couvre |
  |---|---|---|---|---|
  | C1 | Lecture stdout NON bornée → OOM du superviseur (med-low) | `read_capped_line` (cap = `REASONING_MAX_LINE_BYTES`, ligne trop longue = drop) | `7e1f0c3` | `read_capped_line_drops_oversized_lines` (vibectl.rs) |
  | A | `--calls` sous-compte les `tool_use` **parallèles** (low) | `supervisor::count_tool_use` (compte les blocs, pas « any ») | `7e1f0c3` | `count_tool_use_counts_parallel_calls` (supervisor.rs) |
  | C2 | Petit-enfant tenant le pipe → fuite thread lecteur sur sortie **propre** (low) | `terminate_group` après drain si le lecteur traîne (pid capturé avant reap) | `7e1f0c3` (+ test dédié ajouté après) | `agent_run_returns_even_when_a_grandchild_holds_the_pipe` (vibectl.rs) |
  | B | `read_thinking` slurpe tout le fichier pour un tail (low) | `read_tail_string` (lecture bornée ≤ 4 MiB depuis la fin, drapeau `window_bounded`) | `7e1f0c3` | `read_tail_string_bounds_large_files` (reasoning.rs) |
  | C3 | Budget illimité par défaut / valeur invalide silencieusement illimitée (low) | `--budget`/`--calls` invalides → **erreur** (plus de fallback silencieux) + WARNING si run illimité | `7e1f0c3` | `parse_duration_forms` (supervisor.rs — rejette `0`/`abc`/`8x`/`8h30`→None ; le bin transforme None→erreur, glue triviale) |

  Le grant-consommé-si-audit-échoue (relevé low) est **laissé tel quel à dessein** :
  c'est le sens fail-closed voulu du one-shot (documenté en commentaire, `5a165e8`).
- **Durcissement systemd** : genesis + agents-group (options non-mount-namespace,
  contraintes respectées) ; generator amnésique déjà durci.
- **Initiative « VibeOS pour Zed »** (**ADR-014**, cible l'adaptateur
  `claude-code-acp`, jamais le cœur de Zed) :
  - **Investigation** du code réel avant tout patch : `canUseTool` public
    (wrappable), `createSession` **privé**, `runAcp` construit la base en interne
    → forme retenue = **patch de prototype de `canUseTool`** (vérifiée).
  - **Couche 0/1** (config) : `settings.json` Zed + `CLAUDE_CONFIG_DIR` Zed-only
    avec `permissions.deny` (Read/Write/Edit natifs off, terminal non affecté).
  - **Groundwork couche 2** : outil MCP **T0 `policy.check`** dans vibed
    (classification dry-run — allow/deny/require_approval, sans exécuter/approuver,
    ne touche pas `approval.rs`).
  - **Couche 2 (le fork)** : paquet `zed/vibeos-claude-acp` (TypeScript) qui
    patche `canUseTool` → `vibeos:policy.check` (Allow T0/T1 sans prompt, T2/T3
    jamais auto, fail-safe). **Vérifié** : `tsc` compile contre les vrais types
    amont + **12 tests vitest** (logique du mode auto + mapping d'outils + client
    MCP socket testé contre un faux vibed). Innovation : mode auto piloté par
    MOTEUR DE POLITIQUES (pas classifieur LLM). Reste : install image + Zed live.
- **README multilingue** (FR canonique + EN/ES/DE).
- **Hygiène PR** : PR #5 (branche→main) mergée à l'état du matin ; ~44 commits
  d'après-midi orphelins → nouvelle **PR draft #11 (branche → main)** pour les
  rapatrier. Sort de PR #4 (empilée) laissé à l'humain.

**Nuit — nettoyage + vérifications réelles (8 points demandés)**, tout poussé sur PR #11 :
1. **Traçabilité des 5 findings** : table finding→correctif→commit (`7e1f0c3`)→test
   ci-dessous ; **test dédié C2 ajouté** (`agent_run_returns_even_when_a_grandchild_holds_the_pipe`).
2. **Blocage Zed re-qualifié** (`BLOCKERS.md`) : l'extension (agent ACP stdio) se
   valide **sans Zed** — `tsc` + boot ACP headless (`npm run smoke`) + 17 tests ;
   seul le **E2E complet** reste bloqué (liste précise de ce qui manque).
3. **Preuve de déterminisme** (`test/patch.test.ts`) : même entrée ×20 → décision
   identique, 1 `policy.check`/appel, **zéro LLM**.
4. **Kill-switch mesuré** : `agent stop` → **2,636 s** (< 5 s), dernier append
   raisonnement = JSON complet. Critère Phase 2.5 atteint (mesuré).
5. **policy.check anti-DoS confirmé par test** (rate-limité par uid, sortie bornée).
6. **Plan supply-chain npm** (`ADR-015`) + **lockfile commité**.
7. **Passe de cohérence** : 140 Rust + 17 vitest partout, statuts ADR/ROADMAP recalés.
8. **PR #11 rendue vraiment mergeable** : la CI échouait (MSRV 1.75 + cargo audit)
   car **main portait un bump Dependabot `toml 1.1.2`** (→ `serde_spanned 1.1.1`)
   incompatible MSRV 1.75. Merge de main + **revert du bump toml** (garde `0.8`) +
   règle Dependabot. **CI Rust re-verte** ; PR MERGEABLE.
- **État** : **139 tests vibed verts** (132 unit + 5 e2e MCP + 2 politique) +
  outil T0 `policy.check` (groundwork Zed) + **12 tests vitest** de l'extension ;
  clippy `--locked` + fmt propres.

**Analyse + améliorations (matin)** — analyse ultracode (6 agents), revue
adversariale (24 agents, 15 findings corrigés), et une **trousse cybersécurité
gouvernée** (≈ 60 outils pentest/DFIR embarqués + catalogue `docs/SECURITY-TOOLKIT.md`
+ outil MCP `sectools.list` T0). CI durcie (cargo audit/fmt/--locked/clippy
--all-targets, garde `publier`, digest bootc-image-builder, Dependabot).
Sécurité vibed (denylist credentials IA, anti-TOCTOU). Wallpaper `VibeOS.png`
par défaut. ~40 recalages docs. Image bootc construite et inspectée.

**Priorités mémoire/MCP/Genesis (après-midi)** :
- **P1** `memory.append` scopes `user`/`projects` (append-only strict, fold
  last-write-wins) — plus aucun scope agent manquant.
- **P3** événement `tool_call` (réservé système) écrit par vibed dans le
  journal mémoire à chaque action T1+ exécutée.
- **P4** generator systemd du **mode amnésique** (`vibeos.amnesic=1` → tmpfs +
  `VIBEOS_MEMORY_MODE=amnesic` + marqueur), shellcheck + 8 tests fonctionnels.
- **P5** `hardware.json` **schema 2** (cpu/mem/gpu structurés + blobs bruts) +
  smoke test Genesis de non-régression en CI.
- **Audit inviolable** : chaîne de hachés **SHA-256** (`seq`/`prev`/`hash`,
  SHA-256 maison sans dépendance, vecteurs NIST) + `vibed --verify-audit`.
- **vibectl** (2ᵉ binaire) : `memory status`, `memory mode`, `audit verify`.

**Findings de la revue Fable 5** (7/7 traités sauf F6) :
- **F1** `fs.read`/`fs.list` **confinés au home de l'appelant** (SO_PEERCRED) +
  allow-list système (`/etc /usr /proc /sys /run /var/lib/vibeos`) — ferme le
  vrai trou v0.1 (lecture cross-user des données personnelles).
- **F2** `memory.query` rend des **extraits de contenu bornés** (lecture en un
  appel, réaligné sur la spec).
- **F4** **rotation** du journal d'audit par jour UTC, chaîne **continue** entre
  fichiers ; `verify_chain` parcourt tout le répertoire.
- **F5** cohérence doc `vibeos-agents` (wheel auto / non-wheel opt-in) +
  **ADR-010** (identité de l'appelant `[rule.callers]` via `/proc/<pid>/exe`).
- **F7** `CLAUDE.md` dans `/etc/skel/.claude/` (boucle de valeur mémoire :
  memory.query au début, memory.append en fin).
- **F3** **flux d'approbation humaine minimal** T2/T3 : requête → `vibectl
  approve <id>` → **grant à usage unique** (borné (outil,cible,uid), expire
  5 min) consommé au ré-appel → exécution auditée `*_approved`. Store root-only
  + denylisté ; un agent ne peut jamais approuver sa propre requête.
- Cosmétique : Genesis ne bake plus un hostname transitoire (localhost/fedora)
  comme nom de naissance.

**Durcissements complémentaires (fin d'après-midi)** :
- **Supply-chain CI 2026** : job `supply_chain` (SBOM anchore/sbom-action +
  scan Trivy), job **MSRV 1.75** (build+test `--locked`), actions épinglées
  par SHA, Dependabot.
- **Bornage du store d'approbation** : `request_approval` purge les `pending`
  périmés (> 1 h), **déduplique** les requêtes identiques (tool,target,uid) et
  applique un **plafond dur** (64) — un agent ne peut plus remplir le volume
  mémoire en spammant des appels T2/T3 (anti-DoS, analogue à F4).
- **`vibectl approve/deny` réservés à root** (garde `require_root` explicite,
  fail-closed si euid indéterminable).
- **Responsabilité dans l'audit** : l'`outcome` d'un appel approuvé porte l'uid
  de l'opérateur (`ok_approved(by_uid=N)`) — le grant étant supprimé à la
  consommation, le journal inviolable est la seule trace durable de *qui* a
  autorisé le changement système.
- Passe de cohérence docs : chemin d'audit (rotation par jour) et
  approbation/user-projects décrits comme livrés partout.

**Audit Fable 5 (4 points — tous traités)** :
- **n°1/n°2** confinement `fs.read`/`fs.list` au home appelant + allow-list
  système : **déjà livré** (F1).
- **n°3** `source` documenté **non fiable** (auto-déclaré par l'agent, jamais
  une preuve de provenance/autorité) — MEMORY.md §9, THREAT-MODEL §6,
  description de l'outil ; garde-fou avant toute consolidation `knowledge`.
- **n°4** `[rule.callers]` via `/proc/<pid>/exe` : décision **posée** en
  ADR-010 (cible Phase 3/4).
- **n°5** **rate-limiting par uid** (token bucket, module `ratelimit`) : borne
  un agent emballé/compromis (flood audit + mémoire + approbations) ;
  dépassement = refus fail-closed audité `rate_limited`. Rétention/purge du
  journal = politique opérateur (purge = T3) ; rotation par jour déjà en place.

**Revue adversariale du code du jour** (sous-agent, 4 fichiers) : **aucun bug
high/medium** ; 1 MED + points low traités — TOCTOU du plafond `MAX_PENDING`
sous concurrence (verrou de sérialisation + test 128 threads), parse euid
fail-closed (`parse_effective_uid`, uid effectif uniquement). Le grant consommé
si l'audit échoue est laissé tel quel (fail-closed voulu du one-shot).

**Phase 2.5 ajoutée au ROADMAP** (« Autonomie encadrée & accès IA externes »,
proposée) : superviseur d'agent budgété + kill-switch humain, auth abonnement
scellée TPM2, allowlist egress par unité, type réservé `autonomous_session` —
périmètre figé T0/T1. **ADR-011** (log.read T0 anti-exfiltration) posé.

**Extension Phase 2.5 (demande utilisateur)** :
- **Mode autonome « always-on »** (ADR-013) : le superviseur tourne en
  permanence, l'agent enchaîne seul TOUT le T0/T1 sans humain synchrone ; les
  T2/T3 ne bloquent plus mais sont **mis en file** pour approbation asynchrone.
  Le plancher T2/T3 **n'est jamais levé** (invariant §7, THREAT-MODEL S1) —
  interprétation responsable de « autonome pour tout » = autonome sur tout le
  T0/T1 sans babysitting, pas « exécute du destructif sans accord ».
- **Capture du raisonnement** (ADR-012) : tap passif sur le flux `stream-json`
  du CLI (jamais son transcript disque), store `memory/reasoning/`, futur outil
  T0 `agent.thinking`, toggle par session.
- **HUD** : `ReasoningPanel.qml` livré en **scaffolding** (3ᵉ pilier « pourquoi »,
  chip + popup verre, ship avec `[]` — règle d'honnêteté), câblé dans la barre.

**Revue adversariale finale** (2ᵉ passe, tout le code de la session) : **aucun
bug de correction high/medium**. Rust propre (mutex approval, parseur euid,
câblage rate-limit/approbation), pas de régression ; ADR uniques/séquentiels,
invariant T2/T3-sans-bypass préservé partout, tokens `Theme` du QML tous
présents, structure QML bien formée. Corrigé : import `QtQuick.Shapes` inutilisé.
Laissés (non-bugs) : ancrage `PopupWindow` (auto-signalé, à valider sur desktop
booté), I/O bloquante sur reactor + grant-burn-si-audit-échoue (fail-closed,
pattern existant).

**État tests** : **114 tests vibed verts** (107 unitaires + 5 intégration MCP
e2e + 2 politique) ; `clippy --all-targets --locked -D warnings` 0 warning ;
`fmt --check` OK ; `cargo build --locked` des 2 binaires OK ; shellcheck vert.
Images `vibeos:dev-final`, `dev-final2` **et** `dev-final3` (arbre final complet)
construites, `bootc container lint` OK (11 checks, 2 warnings d'hygiène, 0 erreur).

## 🔧 En cours / non terminé (checkpoint 2026-07-13 nuit)

- **Zed — E2E complet** : le cœur du fork est livré et **vérifié sans Zed**
  (`tsc` + 17 tests + boot ACP headless) ; il reste le test bout-en-bout en
  session réelle → voir **`BLOCKERS.md`** (liste précise).
- **Zed — câblage dans l'image** : délibérément **pas fait** tant que l'E2E n'est
  pas validé (ne pas ship ~148 paquets npm non éprouvés). Plan : **ADR-015**.
- **Phase 2.5 — reste** : unité `vibeos-agent@.service` (always-on par défaut),
  **auth par abonnement scellée TPM2**, **allowlist d'egress par unité**.
- **F6 (découpe de `mcp.rs`)** : toujours différé (refactor mécanique ; protocole
  décourage le cosmétique sans gain mesuré).
- **Backends T2 réels** (`pkg.install`/`svc.restart`) : encore des stubs — la
  plomberie d'approbation est prête, l'exécution réelle est **Phase 4**.

## 🚧 Blockers (précis)

- **Zed E2E** : nécessite (1) le **binaire natif du Claude Agent SDK**, (2) un
  **`vibed` démarré** servant `policy.check` sur `/run/vibed/mcp.sock`, (3) un
  **client ACP complet** (Zed — non installable headless ici — ou un harnais
  maison). Détail dans `BLOCKERS.md`.
- **Validation VM/matériel** (Phase 1) : boot ISO amd64+arm64, NVIDIA, `ollama
  run` hors-ligne, `bootc upgrade/rollback` — exigent une vraie machine.
- **Merge des PR** : PR #11 (branche → main) est **MERGEABLE + CI Rust verte** ;
  reste la revue humaine + le merge (je ne merge jamais). Le sort de PR #4
  (empilée, cible `phase2-supply-chain`) est à trancher côté humain.

## ➡️ Prochaine étape recommandée

1. **Merger PR #11 → main** (CI Rust verte ; laisser finir le build image ~15 min),
   puis clore/retirer PR #4 (superseded).
2. **Zed E2E** sur une machine avec Zed + un `vibed` local : lancer une session,
   déclencher un `vibeos:fs.read` (T0, doit passer sans prompt) et un `pkg.install`
   (T2, doit prompter) ; étendre `scripts/smoke-acp.mjs` en client ACP complet
   pour automatiser sans éditeur.
3. **Câbler l'extension dans l'image** selon **ADR-015** (étage npm dédié, `npm ci
   --ignore-scripts --omit=dev`, seul `dist/` copié).
4. **Phase 2.5 reste** : auth abonnement TPM2 (`systemd-creds`), allowlist egress,
   unité always-on.
5. **Backends T2 réels** derrière l'approbation (`svc.restart` via `systemctl`) —
   la démo « l'agent demande, l'humain approuve, l'unité redémarre, l'audit le
   prouve ».
6. **Branchement live du HUD** (`Quickshell.Io`) + Phase 3 (LUKS/TPM2, sandbox
   par outil).
