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

**Nuit (2) — implémentation des 5 briques restantes**, tout poussé sur PR #11 :
- **`svc.restart` (T2) — backend RÉEL** derrière le grant one-shot (n'est atteint
  qu'après `vibectl approve`) : `systemctl restart` (nom validé, `--`, chemin
  absolu, env vidé, borné par le timeout de job systemd) + **relecture d'état**
  pour prouver le redémarrage. `handle_connection` reçoit désormais le répertoire
  d'approbation (injectable) → **test e2e sur socket** : demande→refus T2→approve
  hors bande→ré-appel→grant consommé→audit `started_approved(by_uid=0)`, one-shot
  vérifié. + tests unitaires hermétiques (fake systemctl). THREAT-MODEL à jour.
- **Extension Zed câblée dans l'image (ADR-015)** : étage `zed-agent-builder` —
  `npm ci --ignore-scripts` + **bundle esbuild** vers un unique `.mjs` autonome
  (jamais `node_modules` ni sources TS). **`npm audit --omit=dev` = 0 vuln**
  (les 5 restantes sont dev-only). **Gardé off** (`ARG WITH_ZED_AGENT=0`, ADR-015
  §6) : les deux chemins construisent (podman vérifié) ; à 0 le builder npm est
  hors graphe (marqueur `NOT-INSTALLED.txt`), à 1 seul le bundle est copié.
- **Phase 2.5 — reste livré** : `vibeos-agent@.service` (always-on, `User=%i`
  jamais root, durci sans MDWX car CLI Node), **jeton scellé TPM2**
  (`LoadCredentialEncrypted=` + `vibeos-agent-seal-token.sh`), **allowlist egress
  par nom d'hôte** (`vibeos-agent-egress@.service` + `agent-egress.conf`,
  `getent`→`IPAddressAllow`). shellcheck + `systemd-analyze verify` propres.
- **E2E Zed turnkey** (`scripts/e2e-zed.sh` + `e2e-live-policy.mjs`) : **Tier A
  VALIDÉ sur socket vibed live** — fs.read/fs.list (T0)→allow auto, pkg.install/
  svc.restart (T2)→require_approval, disk.wipe→deny (5/5 PASS, audit écrit).
  Overrides dev `VIBED_SOCKET`/`VIBED_POLICY_DIR`/`VIBED_AUDIT_DIR`. Tier B
  (round-trip éditeur) = checklist, non lancé ici (Zed non headless).
- **HUD branché en LIVE** : `Quickshell.Io.Socket` sur `/run/vibed/mcp.sock` —
  os.status + memory.query + raisonnement (nouvel outil T0 **`agent.sessions`** →
  `agent.thinking`) live ; observateur strict T0, dégradation gracieuse. Roster
  agents + jauge ollama restent hors-ligne (pas d'`agents.list`).
- **F6** inscrit en **dette explicite** (ROADMAP §9 ter, effort 1–2 j).
- **État** : **145 tests vibed verts** (136 unit + 7 e2e MCP + 2 politique) +
  **17 vitest** + smoke ACP + Tier A live ; clippy/fmt propres ; PR #11
  **MERGEABLE, CI Rust verte** (11 checks pass, build image en cours).

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

**Nuit 3 (2026-07-14) — durcissement + agents.list + F6 + ADR**, tout poussé sur PR #11 :
- **PR #11 état FRAIS vérifié** (`gh pr checks`) : `mergeStateStatus: CLEAN`,
  12 pass / 3 skipping / **0 échec** — entièrement verte (build image inclus).
- **CRITIQUE — allowlist de CIBLES svc.restart** : l'allowlist **existait déjà**
  (`[rule.services].denied`, évaluée AVANT le floor T2 dans `policy.rs` → `Deny`,
  pas `require_approval`) mais était **incomplète**. Complétée avec les unités
  d'**accès** (`sshd`, `NetworkManager`/`networkd`, `display-manager`/`sddm`,
  `logind`), d'**approbation** (`vibed`, `vibeos-agent@*`, `polkit`) et le **bus**
  (`dbus-broker`/`dbus`) — refus d'office, hors de portée de la file d'approbation.
  **Test sur la politique livrée** (`shipped_policy_denies_restart_of_critical_units_before_approval`).
- **`agents.list` (T0)** : roster HUD dérivé de l'audit, **confiné à l'uid appelant**
  (l'agent de A ne voit jamais B ; soi-même exclu), groupé par pid. Anti-DoS
  (rate-limit, queue/fenêtre bornées). HUD : roster live + jauge ollama (probe
  local XHR/nvidia-smi). Fait sauter le dernier « hors-ligne » du HUD.
- **F6 — 3/4 familles extraites** (mécanique, zéro changement, 147 tests inchangés) :
  `tools/svc.rs`, `tools/sectools.rs`, `tools/memory.rs` (impl **et** tests).
  **mcp.rs 4257 → 2777 lignes (−35 %)**. `fs` reste (entrelacé : 7 internes testés
  + `builtin_denied` partagé + helpers de test partagés) → session dédiée.
- **Docs** : `agent.sessions` spécifié (ADR-012) ; `WITH_ZED_AGENT=0` verrouillé
  comme choix intentionnel (ADR-015 §6, avertissement anti-régression) ;
  **ADR-016** — `pkg.install` backend **reporté** (allowlist paquets/dépôts non
  tranchée sur OS immuable ; stub conservé) ; THREAT-MODEL à jour. Tier B Zed
  relu : **aucun bug**.
- **Revue adversariale indépendante** (sous-agent) du code de la nuit → **3
  défauts réels corrigés, dont 1 HIGH** :
  - **HIGH — bypass de la deny-list svc.restart** : la policy recevait le nom
    d'unité **brut** (`args["unit"]`) mais la canonicalisation (`+ .service`)
    ne tournait qu'**après** la décision → `svc.restart {"unit":"vibed"}`
    passait en `RequireApproval` au lieu de `Deny` (les 13 unités critiques
    redevenaient approuvables). **Mon test d'hier soir ratait le trou** (noms
    qualifiés seulement). Fix : canonicalisation dans `handle_tools_call`
    **avant** l'évaluation + **test e2e socket** (nom nu → `Deny`).
  - **MED** — `agents.list`/`agent.sessions` sans règle allow → default-deny →
    inertes en prod (fix : règle T0 `agent-observability`).
  - **MED** — deny-list complétée (`user@*.service`, `dbus.socket`).
  - Confinement `agents.list`, extraction F6 memory, anti-DoS : **confirmés sains**.
- **Vérif transverse** (déclenchée par le bug HIGH) : aucun autre outil n'a le
  pattern « validation après décision policy ». Seuls `fs.*` (chemin normalisé
  tôt + recheck canonique anti-symlink déjà en place) et `svc.*` (désormais
  canonicalisé) ont une cible policy-pertinente. Pas de bug frère.
- **Durcissement helpers agent-runner** (défense en profondeur) : validation du
  nom d'instance (`%i`) dans les 3 scripts shell Phase 2.5 (rejet hors
  `[A-Za-z0-9._-]` → pas de traversée de chemin). shellcheck propre.
- **État** : **148 tests vibed verts** (137 unit + 8 e2e MCP + 3 politique) +
  17 vitest + smoke ACP + bundle Zed ; clippy/fmt/shellcheck propres ; **CI Rust
  verte sur le commit de fix** ; PR #11 MERGEABLE.

**Prolongation (jusqu'à 09h) — CI, README, 2ᵉ revue** :
- **Flaky ETXTBSY corrigé** : le monitoring CI a attrapé un test que j'avais
  introduit (`svc_restart_surfaces_a_systemctl_failure` — exec d'un fake systemctl
  fraîchement écrit → « Text file busy » sous cargo test parallèle, invisible en
  local/MSRV). Fix : retry sur ETXTBSY (artefact de test ; la prod exécute
  `/usr/bin/systemctl` statique). CI re-verte.
- **README (4 langues) mis à jour** et synchronisé (HUD live, svc.restart réel,
  agents.list, Phase 2.5 « largement implémentée ») — corrigé les affirmations
  périmées « HUD mocked »/« Phase 2.5 proposed » dans EN/ES/DE. FR canonique.
- **2ᵉ revue adversariale** (primitives cœur : audit/sha256/ratelimit/approval/
  superviseur) : **saines, aucun défaut exploitable**. 1 vrai bug LOW corrigé —
  **écriture déchirée → fausse rupture `verify_chain`** (rollback de la queue non
  terminée au démarrage + test). Docs rendues honnêtes : portée de la
  tamper-evidence (keyless, troncature de queue non détectée sans ancrage Phase 4),
  budget `--calls` best-effort, petit-fils `setsid()` (fuite jamais hang).
  **Extension Zed** relue : gate de gouvernance sain (fail-safe partout).
- **3ᵉ revue adversariale** (confinement fs — la surface sécurité la plus
  critique) : machinerie symlink/canonicalize/dev-ino **saine** (pas de bypass de
  lecture cross-user), mais **3 gaps corrigés** :
  - **#1 (MED, DoS réel)** — `fs.read` sur un **FIFO** bloquait le worker (guard
    `is_file()` après l'`open()` bloquant) → épuisement du pool = déni cross-tenant.
    Fix : type vérifié **avant** l'open + `O_NONBLOCK` + test FIFO.
  - **#2 (MED, blind spot hardlink)** — denylist path-based aveugle aux hardlinks.
    Fix : lecture confinée au home → inode owned par l'appelant (`fstat st_uid`),
    bloque l'escalade cross-owner ; system reads exemptés (`ReadScope`).
  - **#4 (LOW, fail-open)** — home résolu à `/` → confinement inopérant → refus.
  - Résidu documenté : TOCTOU parent intermédiaire de `fs.write` (openat2 = Phase 3).
  - **Bug e2e** corrigé (relecture `index.ts`) : nom d'env socket (`VIBED_MCP_SOCKET`).
- **Bilan des 3 revues** : 1 HIGH (bypass deny-list svc.restart) + plusieurs
  MED/LOW, tous corrigés. Couverture : code de la nuit, primitives cœur
  (audit/ratelimit/approval/superviseur), confinement fs, extension Zed.
- **État prolongation** : **149 tests vibed verts** + 17 vitest ; clippy/fmt/
  shellcheck propres ; PR #11 MERGEABLE, CI Rust verte.

## 🔧 En cours / non terminé (checkpoint final 2026-07-13 nuit)

- **Zed — E2E Tier B (round-trip éditeur)** : le **Tier A est validé sur socket
  vibed live** (décisions fs.read→allow / pkg.install→require_approval, `scripts/
  e2e-live-policy.mjs`). Reste le Tier B — Zed spawn le binaire Claude → vrai appel
  d'outil → prompt supprimé pour un Allow, affiché pour un require_approval. Non
  lançable ici (Zed non headless). Turnkey prêt : `scripts/e2e-zed.sh`.
- **Zed — expédition dans l'image** : l'étage `zed-agent-builder` est livré et
  **construit** (bundle esbuild vérifié), mais **gardé off** (`WITH_ZED_AGENT=0`,
  ADR-015 §6) jusqu'à la validation du Tier B.
- **Phase 2.5 — enforcement live** : unité `vibeos-agent@`, jeton TPM2, egress
  livrés et statiquement validés ; le **comportement au boot** (unseal TPM2 réel,
  egress BPF) exige une machine bootée.
- **HUD** : os.status/memory.query/raisonnement **+ roster agents (`agents.list`)
  + jauge ollama (probe local)** désormais **live** — plus de « hors-ligne » (QML
  non vérifiable au runtime ici : Quickshell non headless).
- **F6 (découpe de `mcp.rs`)** : **3/4 faits** (svc, sectools, memory ; mcp.rs
  4257 → 2777 l.). **`fs` reste** (entrelacé : 7 internes testés + `builtin_denied`
  partagé + helpers de test partagés) → session dédiée (ROADMAP §9 ter).
- **`pkg.install`** : stub conservé **volontairement** (ADR-016 — allowlist
  paquets/dépôts non tranchée sur OS immuable ; backend = Phase 4).

## 🚧 Blockers (précis)

- **Zed E2E Tier B** : nécessite (1) le **binaire natif du Claude Agent SDK**,
  (2) **Zed** (non installable headless en WSL) ou un client ACP maison. Le Tier A
  (lien extension↔vibed) est **déjà prouvé**. Détail : `BLOCKERS.md`.
- **Validation VM/matériel** (Phase 1) : boot ISO amd64+arm64, NVIDIA, `ollama
  run` hors-ligne, `bootc upgrade/rollback` — exigent une vraie machine.
- **Boot Phase 2.5** : TPM2 réel + egress live + auth abonnement E2E = machine bootée.
- **Merge des PR** : PR #11 (branche → main) est **MERGEABLE + CI Rust verte** ;
  reste la revue + le merge humains (je ne merge jamais). **PR #4 ne se ferme PAS
  automatiquement** (même branche source mais base `phase2-supply-chain` ≠ `main`,
  `deleteBranchOnMerge=false`) → fermeture manuelle après #11.

## ➡️ Prochaine étape recommandée

1. **Merger PR #11 → main** (CI Rust verte ; laisser finir le build image ~15 min),
   puis **fermer manuellement PR #4** (superseded).
2. **Zed E2E Tier B** : sur une machine avec Zed, lancer `zed/vibeos-claude-acp/
   scripts/e2e-zed.sh` tel quel (Tier A auto déjà vert, puis la checklist éditeur).
3. **Activer l'expédition** de l'extension (`WITH_ZED_AGENT=1`) une fois le Tier B ok.
4. **Brancher l'agent-runner** sur une vraie machine : sceller un jeton
   (`vibeos-agent-seal-token.sh`), écrire `agent.d/<user>.conf`, `systemctl enable
   --now vibeos-agent@<user>` — vérifier unseal TPM2 + egress.
5. **F6 — extraire `fs`** (dernière famille) en session dédiée ; `pkg.install`
   réel derrière approbation **une fois l'allowlist tranchée** (ADR-016). Puis
   Phase 3 (LUKS/TPM2, sandbox par outil).
