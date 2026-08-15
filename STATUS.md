# 📊 STATUS — État d'avancement de VibeOS

> 🔎 **Audit d'état du 2026-08-14** — mesuré sur le dépôt, pas relu depuis les docs :
> **0 PR ouverte, 0 issue ouverte** (les #46→#50 listées ici sont mergées depuis
> longtemps), **471 tests `vibed` verts** (454 lib + 14 e2e MCP + 3 politique),
> clippy `-D warnings` + fmt propres, **22 outils** au catalogue MCP, 8 gardes CI
> sur 9 vertes en local (la 9ᵉ exige `skopeo`). **Le dernier travail de fond date
> du 2026-07-26** ; depuis, uniquement du Dependabot.
>
> 🔴 **Et le build d'image était cassé depuis 19 jours** (2026-07-26 → 2026-08-14) :
> le digest de base F44 épinglé avait été **purgé par quay**, donc `build-os` rouge
> sur `main`, **aucune image, aucune ISO** — et par ricochet *tous* les points
> machine-gated bloqués en amont. L'auto-bump censé l'éviter tournait bien mais
> **ne pouvait pas ouvrir sa PR** (réglage « Allow GitHub Actions to create and
> approve pull requests » resté OFF), et son échec quotidien n'a réveillé personne.
> **Corrigé** : pin bumpé vers `2f4f02e0`, et l'auto-bump ouvre désormais une
> **issue** de remédiation quand la PR lui est refusée.
> ⚠️ **Action opérateur qui reste** : activer ce réglage dans
> *Settings → Actions → General*.

> **Fichier vivant** : mis à jour à chaque session de travail. C'est le point d'entrée pour reprendre le projet — le « où en est-on, que reste-t-il ».
> Dernière mise à jour : **2026-08-14 (session Opus)** — **audit d'état + déblocage du build** (encadré ci-dessus). Aucune fonctionnalité livrée : la session a mesuré l'écart entre les docs et le dépôt, réparé le pin de base purgé et refermé le trou du garde-fou qui avait laissé courir 19 jours de panne. Ce fichier lui-même était le plus dérivé — il annonçait 412 tests (réel : 471), une surface de 20 outils (réel : 22) et 5 PR à merger (toutes mergées).
> *Précédent : **2026-07-23 (session Opus)** — **identité VibeOS & correctif écran noir**.* Le premier boot réel sur le PC de référence (Ryzen 7 3700X + RTX 3070 Ti) donnait un **écran noir** : ajout des args noyau NVIDIA `nvidia-drm.modeset=1` + blacklist `nouveau` (le RTX est le **seul** chemin d'affichage, pas d'iGPU ; Plasma 6 est Wayland-only), posés à la fois via `os/rootfs/usr/lib/bootc/kargs.d/10-vibeos-nvidia.toml` et `installer/vibeos.ks`. **Branding activé** (à la demande du mainteneur, fin de « Phase 5 non actif ») : greeter **SDDM VibeOS** + splash **Plymouth VibeOS** (initramfs régénéré au build, vérifié), `/usr/lib/os-release` réécrit (`ID=vibeos`, `ID_LIKE=fedora`) + logo — **plus de « Fedora »** au boot/login/`fastfetch`. **Login-naissance** : `vibeos-identity.service` publie l'identité **publique** du citoyen IA sous `/run/vibeos/citizen.json` (la mémoire privée reste 0700), et le greeter le **nomme**. ⚠️ **Secure Boot à désactiver** jusqu'à la signature MOK (Phase 4) — sinon le kmod non signé est refusé et l'écran reste noir. Validation réelle **machine-gated** (rebuild ISO + boot côté mainteneur). *Précédent : **2026-07-22 (session Opus)** — gestion des tokens & mémoire structurée ([ADR-030](docs/DECISIONS.md)), **412 tests verts**.*
> *Précédent : **2026-07-21 (session Fable 5)** — **perf couche basse** :*
> chemins chauds `vibed` corrigés (audit/journal hors du réacteur tokio, catalogue
> d'outils en cache) + **première config noyau de l'image**
> ([ADR-025](docs/DECISIONS.md) : `sysctl.d` charges IA + zram `zstd`, chaque valeur
> sourcée) + 18ᵉ invariant `kernel-tuning` du selfcheck — **PR #166 mergée**. Puis,
> en suite de session : **caches incrémentaux** sur les chemins sondés par le HUD
> (queue d'audit, fold mémoire, têtes de sessions) + digest d'audit unique par appel.
> Puis **IA citoyenne** : `agent.activity` (T0, [ADR-026](docs/DECISIONS.md)) — le
> citoyen relit ses propres actes (refus compris), surface 18 → 19. Puis **mode
> autonome / ouvert** ([ADR-027](docs/DECISIONS.md), fondation) : déverrouillage
> humain hors-bande (`vibectl mode open`), T2/T3 auto-accordés en fenêtre bornée,
> panneau danger + kill-switch + audit `*_open_mode`, no-self-escalation. Puis
> **symbiose** : `user.model` (T0, [ADR-028](docs/DECISIONS.md)) — modèle dérivé &
> transparent de *comment vous travaillez* + anticipation déterministe, confiné
> par uid, sans surveillance nouvelle — surface 19 → 20, **346 tests verts**,
> 19ᵉ invariant selfcheck `operating-mode`. La mesure sur cible reste
> machine-gated. *(Précédent : 2026-07-19, trousse SaaS complète dev→prod —
> [WEEKEND-LOG.md](WEEKEND-LOG.md).)*

## Vue d'ensemble

| | |
|---|---|
| **Phase actuelle** | Phase 1 « Première ISO » (reste : validation VM + NVIDIA) · Phase 2 « vibed + MCP » 🔄 bien avancée · Phase 2.5 largement implémentée · **Phase 4 (chemin critique) toujours à zéro** |
| **Version** | 0.1.0-dev (pré-alpha) |
| **Dépôt GitHub** | [`Micka420-collab/vibeos`](https://github.com/Micka420-collab/vibeos) (privé) ✅ en ligne — **0 PR ouverte, 0 issue ouverte** (2026-08-14) |
| **Image OS** | `ghcr.io/micka420-collab/vibeos:0.1.0-dev` — **manifest multi-arch amd64+arm64 PUBLIÉ + SIGNÉ cosign** ✅ (Rekor #2062451740) |
| **ISO** | **amd64 + arm64 générées** ✅ — artefacts CI `vibeos-iso-amd64` (7,0 Go) / `vibeos-iso-arm64` (6,3 Go) ; ISO amd64 aussi en local `F:\VibeOS-ISO\`. ⚠️ **Datent de la release v0.1.0-dev** — aucune ISO reconstruite depuis, le build étant resté cassé du 2026-07-26 au 2026-08-14 |
| **Release** | **v0.1.0-dev — CI entièrement verte** (runners natifs) : build amd64+arm64 → manifest → cosign → 2 ISO |
| **Tests** | **471 verts** (454 lib + 14 e2e MCP + 3 politique) · clippy `-D warnings` + fmt propres — mesuré le 2026-08-14 |
| **Surface MCP** | **22 outils** au catalogue · 21 règles dans la politique livrée |
| **Machine de référence** | Ryzen 7 3700X · RTX 3070 Ti · 16 Go — voir [docs/HARDWARE.md](docs/HARDWARE.md) |

## ✅ Fait

- **2026-07-03** — Décisions d'architecture actées (8 ADR) : base immuable bootc/OSTree sur Fedora Kinoite 42 (KDE Plasma 6), daemon `vibed` (Rust) exposant le contrôle OS via MCP, politique T0–T3 avec approbation humaine T2+, mémoire « Genesis » créée au premier boot, IA hybride cloud+local.
- **2026-07-03** — Fondation du dépôt générée (31+ fichiers) : vision/manifeste, architecture, roadmap pluriannuelle (v0.1 → v1.0 mi-2028 → souveraineté progressive), Containerfile bootc, crate Rust `vibed` complet (policy/mcp/audit + tests), sous-système mémoire (spec + `genesis.sh` testé), modèle de menace + politiques.
- **2026-07-03** — Revue croisée à 3 lentilles (cohérence, réalisme sécurité, complétude) : ~50 problèmes identifiés, dont 9 majeurs (schéma de politique incompatible avec le parseur, politiques non installées dans l'image, `fs.read` sans garde-fous, approbation T2 non modélisée, survente documentaire).
- **2026-07-03** — 20 décisions canoniques figées pour trancher toutes les contradictions (schéma TOML riche, first-match, default-deny absolu, fail-closed, audit `/var/lib/vibeos/audit/vibed.jsonl`, socket `root:vibeos-agents 0660`…).
- **2026-07-03** — Environnement de build local opérationnel : WSL2 Ubuntu 24.04 + podman 4.9 + buildah + qemu-user-static (builds arm64) + rust 1.75 + shellcheck + skopeo.
- **2026-07-03** — Supply chain épinglée : image de base par digest, CLIs IA versionnées (claude-code 2.1.199, agent-sdk 0.3.199, gemini-cli 0.49.0, codex 0.142.5, opencode-ai 1.17.13).
- **2026-07-03** — Profil matériel de référence documenté ([docs/HARDWARE.md](docs/HARDWARE.md)) : multi-arch amd64+arm64, couche NVIDIA/CUDA amd64 uniquement, checklist de validation avant flash.
- **2026-07-03** — Passe de corrections (5 agents, 50 findings) : moteur de politique Rust réécrit au schéma canonique (**33 tests verts**), CI multi-arch + cosign, honnêteté documentaire « livré v0.1 vs Phase N ».
- **2026-07-03** — **Dépôt privé GitHub créé et poussé** : [Micka420-collab/vibeos](https://github.com/Micka420-collab/vibeos) (41 fichiers, commit initial, topics). Workflow `ci` déclenché ; build multi-arch lourd annulé (économie de minutes) au profit du build local.
- **2026-07-03** — Recherche écosystème OSS (113 candidats → curation) : [docs/ECOSYSTEM.md](docs/ECOSYSTEM.md). Stack : Ghostty+fish+Starship+Zellij, preset « VibeVim », opencode, MCP offline, age/SOPS/systemd-creds, HUD Quickshell, thème « VibeOS Dark ».
- **2026-07-03** — **Build local amd64 vert** : Containerfile confronté au réel dans WSL2/podman, 3 pièges ostree corrigés en boucle (npm `HOME`→/tmp, aider→**opencode** pour Python 3.13, nettoyage `/run`/`/tmp`) ; **VS Code → VSCodium** (licence). `bootc container lint` : 13 checks OK, 0 warning.
- **2026-07-03** — **Bureau vibecoding + installateur conçus** (24 fichiers) : [docs/DESKTOP.md](docs/DESKTOP.md) (layout Plasma 6, HUD Quickshell, thème VibeOS Dark), dotfiles `/etc/skel` prêts au 1er boot (fish/Starship/Ghostty/Zellij + preset VibeVim), [docs/INSTALLER.md](docs/INSTALLER.md) + kickstart + logo SVG.

- **2026-07-03** — **Première ISO installable amd64 générée** (bootc-image-builder) : `install.iso` 5,6 Go, bootable (ISO 9660 / installateur Fedora embarquant l'image). A nécessité de porter la RAM de WSL2 à **12 Go + 8 Go swap** (`.wslconfig`) — osbuild OOM à 8 Go. Copiée dans `F:\VibeOS-ISO\` pour test en VM Hyper-V.
- **2026-07-03** — **build-os amd64 vert en CI** (GitHub Actions, 13 min) : l'image build proprement aussi côté CI. Workflow redécoupé : build amd64 sur push/PR, release multi-arch + cosign + ISO sur tag `v*`.
- **2026-07-03** — README élevé au niveau pro (hero, badges live, navigation, section expérience vibecoding).

- **2026-07-03** — **Couche terminal vibecoding livrée dans l'image** : fish, neovim, zoxide, bat, eza, btop, atuin (dépôts Fedora) + starship, zellij, lazygit, ghostty, yazi (COPR) + mise (repo officiel) + polices JetBrains Mono/Fira Code. Les 22 binaires vérifiés présents, tous les dotfiles `/etc/skel` en place.
- **2026-07-03** — **Système de design « VibeOS » + assets livrés** : [docs/DESIGN-SYSTEM.md](docs/DESIGN-SYSTEM.md) (708 lignes : tokens couleur/surfaces/verre/typo/motion, tiers T0–T3, tendances 2026). Assets embarqués dans l'image (sélectionnables ; activation par défaut = Phase 2) : Global Theme Plasma `org.vibeos.dark`, thème Kvantum, wallpapers originaux (Genesis/Void SVG), thème SDDM, splash Plymouth, HUD Quickshell (singleton `Theme.qml` + composants). Maquette visuelle HTML créée (glassmorphism, HUD agents, gouvernance T0→T3).

- **2026-07-03** — **Release v0.1.0-dev publiée** : CI redécoupée en **runners natifs** (fini l'émulation arm64 : ~15 min/arch au lieu de ~2 h). Image multi-arch publiée sur ghcr.io, **signée cosign keyless** (Rekor #2062451740), et **2 ISO** (amd64 7,0 Go + arm64 6,3 Go) en artefacts CI. Trois correctifs de release au passage (`push-to-registry` registry, digest via `--digestfile`, espace disque agressif pour l'ISO amd64).

- **2026-07-03** — **`vibed` branché dans l'image (Phase 2)** : le binaire est **embarqué** (compilé en multi-stage dans `os/Containerfile` → `/usr/bin/vibed`), **`vibed.service` démarre au boot**, **charge et applique la politique** installée (`/etc/vibeos/policy.d/`, 7 règles, fail-closed), **sert le serveur MCP** sur `/run/vibed/mcp.sock` (identité de l'appelant via `SO_PEERCRED`), **audite** sous `/var/lib/vibeos/audit/vibed.jsonl`, et expose l'outil MCP **`memory.query`**. Restent Phase 2 : HUD/quickshell (paquet + autostart), `memory.append` + `scope`/`limit`, outils T1 réels, config MCP côté client. `vibed` tourne encore en **root** (`User=vibed` : Phase 4).

- **2026-07-03** — **Audit de cybersécurité adversarial** (workflow multi-agents, 4 lentilles + vérification) : le cœur du moteur de politique est validé sûr (fail-closed, plancher T2/T3 non abaissable, T2 = refus, pas d'enregistrement dynamique d'outils). **2 failles critiques + 3 hautes corrigées** dans les outils fichiers de vibed (root) : `fs.write` symlink-safe (canonicalisation + `O_NOFOLLOW`) et confiné au home de l'appelant (uid `SO_PEERCRED`), denylist `fs.read` étendue aux secrets (SSH/AWS/kube/NM/gshadow/`/proc/**/environ`), rejet des fichiers spéciaux + lecture bornée (anti-DoS), audit enrichi. 47 tests verts. **Reste (décisions supply-chain)** : ~~épingler les Actions par SHA~~ ✅, ~~COPR désactivés après install + manifeste NEVRA~~ ✅, ~~`npm --ignore-scripts`~~ ✅, ~~recaler `SECURITY.md`~~ ✅ (lot du 2026-07-08 ci-dessous) ; restent `CapabilityBoundingSet` vide et la *vérification* cosign côté client — tous deux **Phase 4** (exigent un système démarré pour être mesurés/testés).


- **2026-07-08** — **Lot « HUD + thème par défaut + MCP client » livré (Phase 2)** : **Quickshell 0.2.1 compilé depuis les sources** dans l'image (étage `quickshell-builder` d'`os/Containerfile` — aucun paquet n'existe pour Fedora 42 : le paquet officiel commence à f44 et aucun COPR n'a de chroot f42, vérifié par API COPR + src.fedoraproject.org ; version épinglée + sha256, recette du spec Fedora officiel, `X11=OFF` — image Wayland-only, garde d'ABI privée Qt par `quickshell --version` dans l'image finale). **HUD auto-démarré** en session Plasma (`/etc/skel/.config/autostart/vibeos-hud.desktop` → `/usr/bin/vibeos-hud`, `TryExec` = dégradation gracieuse). **Global Theme `org.vibeos.dark` par défaut système** (pointeur une-ligne `/etc/xdg/kdeglobals` + garde anti-collision au build ; moteur **Kvantum installé** — couche 1g — avec sélection `VibeOSDark` en skel). **Config MCP Claude Code livrée** (`/etc/skel/.claude.json` : serveur `vibeos` → socat → `/run/vibed/mcp.sock`, portée utilisateur, zéro config manuelle — prérequis : groupe `vibeos-agents`). Honnêteté préservée : le HUD affiche encore des **données mockées** (« vibed hors ligne ») — le branchement live QML ↔ socket reste en Phase 2. Docs recalées (DESKTOP, quickshell, look-and-feel, agent, os, skel). Build local vert (stage quickshell + image complète, `bootc container lint`).

- **2026-07-08** — **`memory.append` + `scope`/`limit` livrés dans `vibed` (Phase 2)** : l'écriture mémoire gouvernée existe. **`memory.append` (T1)** — strictement additif : une ligne JSONL par appel (`O_APPEND` + `O_NOFOLLOW`, mode `0600`, plafond 16 KiB), scopes **`journal`** (types agents `observation`/`decision`/`preference`/`project_seen`/`error` ; les types système `genesis`/`boot`/`tool_call`/`purge` sont refusés) et **`knowledge`** (`subject`/`fact`/`source`[/`confidence` ∈ 0..1]) ; `ts` et `id` posés par `vibed`, aucun argument de chemin, fail-closed si Genesis n'a pas tourné ; scopes `user`/`projects` (fusion structurée) = reste Phase 2/3. **`memory.query`** gagne `scope` (identity/hardware/user/projects/journal/knowledge) et `limit` (+ drapeau `truncated`). Règle de politique `memory-append` ajoutée à `default.toml` (T1, allow). **58 tests verts** (56 unitaires + 2 intégration, +11) ; docs recalées (MEMORY §3/§9, vibed, agent, policy.d, THREAT-MODEL).

- **2026-07-08** — **Lot supply-chain livré (suites de l'audit)** : **GitHub Actions épinglées par SHA de commit** (11 occurrences, tag en commentaire — `ci.yml` + `build-os.yml`) ; **dépôts COPR désactivés dans l'image livrée** (activés uniquement le temps de l'install au build : un système déployé ne fait jamais confiance à un COPR à l'exécution) ; **`npm install --ignore-scripts`** avec **deux exceptions délibérées** (postinstalls de `@anthropic-ai/claude-code` — binaire natif — et `opencode-ai` — câblage du binaire de plateforme — rejoués via `npm rebuild` ciblé ; les deux cassures détectées par la couche de vérification, comme prévu) et **double vérification** : `claude/gemini/codex/opencode --version` au build **puis `claude`/`opencode` re-prouvés après purge** (les binaires vivent dans `/usr`) ; **manifeste NEVRA** `/usr/share/vibeos/packages-nevra.txt` (inventaire RPM exact, diffable entre releases) ; **`SECURITY.md` recalé** (§1.2 « Vérifié » + pratiques). Restent Phase 4 : `CapabilityBoundingSet` vide et vérification cosign côté client (intestables sans système démarré).

- **2026-07-13** — **Analyse ultracode complète + journée d'amélioration continue** ([PR #4](https://github.com/Micka420-collab/vibeos/pull/4), empilée sur #3). Analyse : 6 agents en parallèle (crate vibed, build réel WSL, image OS, CI, docs, périphériques) — a notamment révélé que les 3 lots Phase 2 du 2026-07-08 dormaient en draft PRs non mergées. Livré ensuite :
  - **CI** : `cargo fmt --check` (crate rustfmt-é), `--locked` partout, `clippy --all-targets`, **job `cargo audit`** (critère Phase 2), `timeout-minutes` + `concurrency`, déclencheur `vibed/**` sur build-os, **garde sur `workflow_dispatch`** (taper `publier` pour une release), permissions par job, `bootc-image-builder` épinglé par digest, Dependabot (pins SHA + Cargo.lock).
  - **Sécurité vibed** : denylist étendue aux **credentials des agents IA** (`~/.claude/**`, `.claude.json`, `gh`, `gemini`, `codex`, opencode, ollama, `.npmrc`, `.git-credentials`, SOPS — vibed root pouvait les lire pour tous les utilisateurs) ; `memory.query` : scan réellement borné + walk sans suivi de symlinks ; `fs.read` : atténuation TOCTOU (dev/ino + `O_NOFOLLOW`).
  - **Nouveaux outils T0** : **`svc.status`** (état d'unité systemd, validation anti-injection, env vidé) et **`fs.list`** (listing borné 500 entrées, même denylist que `fs.read`, symlinks jamais suivis). **Tests d'intégration MCP bout-en-bout** sur socketpair réel (handshake, refus T2/denylist audités) — critère de sortie Phase 2. **72 tests verts**.
  - **Chaîne agent→vibed out-of-box** : `vibeos-agents-group.service` enrôle les membres de `wheel` dans `vibeos-agents` à chaque boot (le socket 0660 était inutilisable sans `usermod` manuel).
  - **Wallpaper officiel** : `VibeOS.png` devient le **fond d'écran par défaut** (Global Theme `[Wallpaper] Image=VibeOS`) ; `desktop/wallpapers` restructuré en 3 **paquets Plasma valides** (l'ancien format à plat était inaffichable) avec rendus 4K des SVG Genesis/Void.
  - **Docs** : ~40 incohérences corrigées (SECURITY.md « signé à chaque push », systemd-creds au présent, NVIDIA « validée », qemu vs runners natifs, HUD « offline »…) ; jalons tranchés partout : vérif cosign client = **Phase 4**, approbation humaine T2 = **Phase 4** ; THREAT-MODEL §6 complété pour la surface du jour ; interview Genesis : `umask 077` (0700/0600 vérifiés en réel).
  - **Revue adversariale** du diff du jour (24 agents, 4 lentilles + réfutation) : 15 findings confirmés corrigés (TOCTOU symlink de `fs.list` aligné sur `fs.read`, tri-après-troncature → tas borné, durcissement symlink complété de `memory.query`, annulation de release par dispatch confirmé, `.aws/**`, ~10 recalages docs, artefacts de test retirés) ; audit `fsync` + plafond PATH_MAX ajoutés.
  - **Trousse cybersécurité gouvernée** (recherche web 10 agents, 326 outils recensés → curation) : couche `1d-bis` du Containerfile embarque ≈ 60 outils pentest/DFIR (RPM signés Fedora/RPM Fusion : nmap, hashcat, radare2, aircrack-ng, impacket, sleuthkit, suricata, lynis…) ; **catalogue de référence** [docs/SECURITY-TOOLKIT.md](docs/SECURITY-TOOLKIT.md) (10 domaines dont sécurité IA/LLM, tiers, packaging, cadre d'usage autorisé) ; outil MCP **`sectools.list`** (T0) de découverte en lecture seule + manifeste `security-tools.tsv`. Gouvernance : offensif = T2/T3 approbation humaine ; exécution agent liée au flux d'approbation Phase 4 (aucun agent ne lance d'outil offensif aujourd'hui). **77 tests verts**.

- **2026-07-13 (après-midi, session autonome)** — Poursuite jusqu'à 16h sur les priorités mémoire/MCP/Genesis + best practices 2026, tout vérifié en local (**114 tests vibed** + smoke tests) et poussé en continu ([PR #4](https://github.com/Micka420-collab/vibeos/pull/4)) :
  - **Audit inviolable (Phase 4 avancée)** : journal d'audit **chaîné par hachage SHA-256** (`seq`/`prev`/`hash`), SHA-256 maison sans dépendance (vecteurs NIST), sous-commande `vibed --verify-audit` (détecte altération/suppression/réordonnancement). Reste Phase 4 : ancrage externe TPM/Rekor.
  - **memory.append complet** : scopes **`user`/`projects`** livrés en append-only strict (invariant §4, `updates.jsonl`, fold last-write-wins) — plus aucun scope agent manquant. Événement **`tool_call`** (réservé système) écrit par vibed dans le journal mémoire à chaque action T1+ exécutée.
  - **Mode amnésique (Phase 3)** : **generator systemd** `vibeos-amnesic-generator` livré (tmpfs sur `vibeos.amnesic=1` + injection `VIBEOS_MEMORY_MODE=amnesic` + marqueur `/run/vibeos/memory-mode`), shellcheck + 8 tests fonctionnels.
  - **`hardware.json` schema 2** : champs structurés cpu (model/cores), memory (total_bytes), gpu (vendor/model/vram_bytes via nvidia-smi/lspci) + blobs bruts ; smoke test Genesis de non-régression en CI (layout, schema, idempotence, marqueur amnésique).
  - Fix au passage : bit exécutable manquant sur `vibeos-agents-group.sh`.
  - **`vibectl`** (2ᵉ binaire du crate) : `memory status` / `memory mode` / `audit verify` / `approvals list` / `approve` / `deny`.
  - **Revue Fable 5 (7 findings, 6 traités)** : **F1** `fs.read`/`fs.list` **confinés au home de l'appelant** + allow-list système (ferme le vrai trou v0.1 : lecture cross-user) ; **F2** `memory.query` rend des **extraits de contenu bornés** ; **F4** **rotation** du journal d'audit par jour UTC (chaîne continue) ; **F3** **flux d'approbation humaine minimal** T2/T3 (requête → `vibectl approve` → grant à usage unique expirant → exécution auditée) ; **F5** cohérence doc `vibeos-agents` + ADR-010 (`[rule.callers]`) ; **F7** `CLAUDE.md` dans `/etc/skel`. Reste **F6** (découpe de `mcp.rs`).
  - **Durcissement + audit Fable 5 (4/4)** : **supply-chain CI** (SBOM + Trivy + job MSRV 1.75) ; **rate-limiting par uid** (token bucket, module `ratelimit`, anti-flood audité) ; **store d'approbation borné** (purge/dedup/plafond 64, anti-DoS disque) ; **`vibectl approve/deny` root-only** ; **responsabilité audit** (`ok_approved(by_uid=N)`) ; `source` mémoire documenté **non fiable** (avant consolidation `knowledge`) ; **revue adversariale** du code du jour (aucun bug high/medium ; TOCTOU du plafond d'approbation + parse euid durcis). **114 tests vibed verts** (107 unitaires + 5 e2e MCP + 2 politique), clippy/fmt propres ; images `vibeos:dev-final` + `dev-final2` construites, `bootc container lint` OK. Phase 2.5 ajoutée au ROADMAP.
  - Journal de session : [SESSION_LOG.md](SESSION_LOG.md).

- **2026-07-13 (soir, session autonome étendue jusqu'à 23h)** — Phase 2.5 concrétisée + initiative « VibeOS pour Zed » (fork livré et **vérifié sans Zed**), tout vérifié en local (**140 tests vibed + 17 tests de l'extension**) et poussé ([PR draft #11 → main](https://github.com/Micka420-collab/vibeos/pull/11)) :
  - **Superviseur d'agent** `vibectl agent run/stop/thinking` (**ADR-012/013 implémentés**) : tape le flux `stream-json` d'un CLI → store `memory/reasoning/<session>.jsonl`, budgets wall-clock + nombre d'appels, kill-switch opérateur (marqueur `.stop`), type de journal réservé `autonomous_session`. Robuste : groupe de processus dédié + group-kill + drain borné + lecture de ligne bornée (ne se suspend jamais, pas d'OOM). **N'approche jamais `approval.rs`** ; les T2/T3 restent gérés par vibed (pending non bloquant). Outil MCP **T0 `agent.thinking`** + HUD `ReasoningPanel.qml`.
  - **Correctifs** : approval fs I/O sur `spawn_blocking` ; grant-vs-audit tranché+documenté. **Revue adversariale indépendante** : aucun bug high ; 5 items durcis (lecture stdout bornée anti-OOM, comptage tool_use parallèles, cleanup petits-enfants, `read_thinking` borné, budget invalide→erreur).
  - **Durcissement systemd** : `vibeos-genesis.service` + `vibeos-agents-group.service` (options non-mount-namespace, contraintes documentées respectées ; validé `systemd-analyze`).
  - **Initiative « VibeOS pour Zed »** (**ADR-014**) : cible l'adaptateur `claude-code-acp` (pas le cœur de Zed). **Investigation** du code réel avant tout patch (forme = **patch de prototype `canUseTool`** car `createSession` privé). **Couche 0/1** (config Zed-only : `permissions.deny` dans un `CLAUDE_CONFIG_DIR` propre à Zed). Groundwork **outil T0 `policy.check`** (classification dry-run). **Couche 2 (le fork)** : paquet `zed/vibeos-claude-acp` — `canUseTool` → `vibeos:policy.check` (Allow T0/T1 sans prompt, T2/T3 jamais auto, fail-safe). **Vérifié sans Zed** : `tsc` compile contre l'amont + **17 tests vitest** (dont preuve de déterminisme + client MCP socket) + **boot ACP headless** (`npm run smoke`). Plan supply-chain **ADR-015**. E2E complet = `BLOCKERS.md`. ROADMAP §9 bis.
  - **Kill-switch mesuré** : `vibectl agent stop` arrête une session en **2,636 s** (< 5 s), dernier append raisonnement cohérent — critère Phase 2.5 atteint.
  - **README multilingue** (FR canonique + EN/ES/DE).
  - **Hygiène PR** : PR #5 (branche→main) mergée à l'état du matin ; ~44 commits d'après-midi/soir orphelins → **PR draft #11 (branche → main)** pour les rapatrier.
  - **140 tests vibed verts** (132 unit + 6 e2e MCP + 2 politique) + **17 tests vitest** de l'extension Zed, clippy `--locked` + fmt propres ; images `dev-final`…`dev-final4`, `bootc container lint` 0 erreur.

- **2026-07-13 (nuit, session autonome)** — Nettoyage + concrétisation : les vérifications des 8 points + 5 nouvelles briques livrées et vérifiées, poussées sur [PR #11 → main](https://github.com/Micka420-collab/vibeos/pull/11) (**MERGEABLE, CI Rust verte**) :
  - **`svc.restart` (T2) — backend réel** derrière le grant d'approbation one-shot (n'est atteint qu'après `vibectl approve`) : `systemctl restart` (nom validé, `--`, chemin absolu, env vidé) + relecture d'état pour *prouver* le redémarrage. Chaîne demande→refus T2→approve→grant→redémarrage→audit `started_approved(by_uid)` couverte **e2e sur socket**. `pkg.install` reste un stub.
  - **Extension Zed câblée dans l'image (ADR-015)** : étage `zed-agent-builder` — `npm ci --ignore-scripts` + **bundle esbuild** vers un unique `.mjs` (jamais `node_modules` ni sources TS ; `npm audit --omit=dev` = 0 vuln). **Gardé off** par défaut (`ARG WITH_ZED_AGENT=0`, pas d'expédition avant l'E2E). Les deux chemins construisent (podman vérifié).
  - **Phase 2.5 — reste livré** : unité `vibeos-agent@.service` (always-on, `User=%i` jamais root, durcie) + **jeton scellé TPM2** (`LoadCredentialEncrypted=` + `vibeos-agent-seal-token.sh`) + **allowlist egress par nom d'hôte** (`vibeos-agent-egress@.service`, `getent`→`IPAddressAllow`). shellcheck + `systemd-analyze verify` propres ; enforcement live = machine bootée.
  - **E2E Zed turnkey** : `zed/vibeos-claude-acp/scripts/e2e-zed.sh` (build+bundle, vibed, Tier A auto + checklist Tier B). **Tier A VALIDÉ sur socket vibed live** (fs.read T0→allow auto ; pkg.install/svc.restart T2→require_approval ; disk.wipe→deny). Overrides dev `VIBED_SOCKET`/`VIBED_POLICY_DIR`/`VIBED_AUDIT_DIR`.
  - **HUD branché en live** : `Quickshell.Io.Socket` sur `/run/vibed/mcp.sock` — os.status + memory.query + raisonnement (`agent.sessions`→`agent.thinking`) live ; observateur strict T0, dégradation gracieuse. Nouvel outil T0 **`agent.sessions`**. Roster agents + ollama restent hors-ligne (pas d'`agents.list`).
  - **Docs** : F6 inscrit en dette explicite (ROADMAP) ; THREAT-MODEL à jour (svc.restart, egress, TPM2) ; ADR-014/015 recalés.
  - **145 tests vibed verts** (136 unit + 7 e2e MCP + 2 politique) + **17 tests vitest** + smoke ACP + Tier A live, clippy/fmt propres.

- **2026-07-14 (nuit, session autonome)** — Durcissement sécurité + agents.list + F6 3/4, tout poussé sur [PR #11](https://github.com/Micka420-collab/vibeos/pull/11) (MERGEABLE, CI Rust verte) :
  - **Allowlist de cibles `svc.restart`** : existait déjà (`[rule.services].denied`, refus avant le floor T2) mais **une revue adversariale a trouvé un bypass HIGH** — en omettant `.service`, la deny-list était contournée (la policy voyait le nom brut, canonicalisation seulement après la décision). **Corrigé** (canonicalisation dans `handle_tools_call` avant l'évaluation) + test e2e socket (nom nu → `Deny`). Deny-list complétée (accès/approbation/bus + `user@*`/`dbus.socket`).
  - **`agents.list` (T0)** : roster HUD dérivé de l'audit, **confiné à l'uid appelant** (jamais l'activité d'un autre user), + règle allow (sinon inerte). **HUD roster + jauge ollama live** (probe local) — plus de « hors-ligne ».
  - **F6 : 3/4 familles** (svc, sectools, memory) extraites dans `tools/*.rs` (impl+tests) — **mcp.rs 4257 → 2777 lignes (−35 %)** ; `fs` reste (entrelacé, session dédiée).
  - **ADR-016** : `pkg.install` backend **reporté** (allowlist non tranchée sur OS immuable) ; **agent.sessions** documenté (ADR-012) ; **WITH_ZED_AGENT=0** verrouillé (ADR-015).
  - **Revue adversariale** (sous-agent) → 1 HIGH + 2 MED corrigés ; durcissement des helpers agent-runner (validation d'instance).
  - **149 tests vibed verts** (138 unit + 8 e2e MCP + 3 politique) + **17 vitest** + smoke ACP + bundle Zed ; clippy/fmt/shellcheck propres (incl. durcissements fs + fix audit torn-write).

- **2026-07-14 → 15 (nuit, session autonome)** — Audit sécurité adversarial + `log.read` + garde-fou anti-dérive. **Tout mergé sur `main`** (#33 → #45) :
  - **Garde-fou CI anti-dérive documentaire** ([#33](https://github.com/Micka420-collab/vibeos/pull/33)) : `scripts/verify-roadmap-truth.sh` vérifie **mécaniquement** ce qui est vérifiable — lien markdown mort et fichier annoncé mais absent **échouent la CI** ; compteur de tests annoncé vs réel et PR citée « mergée » mais absente de `git log` **avertissent** seulement. Il attrape les incohérences **mécaniques**, jamais le sens (« proposé » vs « en cours » reste un jugement humain). Il a trouvé de la vraie dérive dès son premier passage.
  - **Note de trajectoire Phase 4** ([#34](https://github.com/Micka420-collab/vibeos/pull/34)) : datée et honnête, en tête de la section Phase 4 du ROADMAP — 11 jours après la Phase 0, SELinux `vibed_t` / boot mesuré / sandbox par outil sont à **zéro**, `vibed` tourne toujours en **root**. **La question de séquencement est posée à Micka, pas tranchée par l'agent.**
  - **`log.read` (T0, ADR-011)** ([#37](https://github.com/Micka420-collab/vibeos/pull/37)) : allowlist d'unités **évaluée avant le plancher de tier** (une unité hors liste est refusée **avant** la file d'approbation, jamais « approbation requise »), sortie bornée (200 lignes / 64 Kio, re-plafonnée **après** rédaction — un masque peut être plus long que le secret qu'il remplace), rédaction heuristique **non garantie** et documentée comme telle.
  - **4 correctifs sécurité RÉELS** issus d'un audit adversarial à 3 vagues (8 sous-agents + revues manuelles) :
    - **[#38] HIGH — denylist mémoire noyau + cross-user** : `/proc/kcore` (RAM physique), `/proc/kallsyms` (défait KASLR), `/proc/*/mem`, `/proc/*/pagemap`, `/run/user/**` (runtime de **tous** les utilisateurs), `/run/secrets/**`, secrets `/etc` (krb5/sssd/ipsec/pki). `vibed` tourne en root : tout ce qui n'est pas denylisté sous les préfixes lisibles était exposé.
    - **[#39] `/proc/<pid>` confiné à l'uid propriétaire** — un agent ne peut plus lire le `/proc` d'un processus d'un autre utilisateur.
    - **[#42] egress anti-SSRF** : l'allowlist par nom d'hôte résolvait le DNS puis autorisait l'IP obtenue — un CNAME vers `169.254.169.254` ou une IP RFC1918 perçait le mur default-deny. Les IP internes/link-local/metadata sont désormais **écartées avant** d'émettre `IPAddressAllow`.
    - **[#43] budget agent sur horloge monotone** : le budget wall-clock comparait des `SystemTime` — **un recul d'horloge laissait un agent emballé survivre à son `--budget`**. Passé sur `Instant`.
  - **Le reste de l'audit : vérifié robuste, aucun bypass** — approval (portail T2/T3, grant one-shot atomique), allowlist policy, pipeline mcp (rate-limit→denylist→policy→approbation→exec), gouvernance Zed (fail-closed), chaîne d'audit SHA-256, Containerfile (supply-chain), glob.rs, memory.rs, sha256.rs (vecteurs NIST présents). **Ne pas refaire cet audit.**
  - **Assets du splash de boot** ([#35](https://github.com/Micka420-collab/vibeos/pull/35)) : générateur Pillow (`desktop/plymouth/generate-assets.py`) — spirale galactique mauve→blue, cœur qui respire, wordmark. **Œuvre originale, reproductible.** Le thème reste **non activé** (Phase 5).
  - **CI/tests/docs** : shellcheck étendu aux scripts sécurité agent (#44), test du garde anti-recul d'horloge du rate-limiter (#41), SECURITY-ARCHITECTURE §3.3 recalé (durcissements fs = **faits**, plus « à faire ») (#45), THREAT-MODEL (log.read + denylist).

- **2026-07-18 (week-end autonome)** — **Trousse SaaS + ecommerce gouvernée livrée** ([ADR-020](docs/DECISIONS.md), même modèle que la trousse cybersécurité). Quatre briques **mergées sur `main`**, chacune build vert + revue/fact-check Fable 5 :
  - **[#95] couche `1d-ter`** : 14 outils **passifs** dnf-natifs (client `psql`, `sqlite`, `redis-cli`, `mkcert`, `podman-compose`, `uv`, `ruff`, `mypy`, `ab`, `perf`, `sysstat`, `bpftrace`, `bcc`, `gh`). **Clients, pas serveurs.** Manifeste `os/saas-tools.txt` gardé par `scripts/check-saas-sync.py` (mutation-testé, câblé CI).
  - **[#97] modèles `compose` par projet** (`/usr/share/vibeos/saas/`) : PostgreSQL 18 + Valkey, et Caddy + TLS local `mkcert`. Les **serveurs** vivent en conteneurs **par projet** (ports loopback-only, mdp exigé via `.env`, volumes nommés), jamais gravés dans l'image.
  - **[#100] installeur à la demande** `/usr/libexec/vibeos/install-saas-tool` : `oha`/`vegeta`/`flyctl`/`railway` téléchargés **épinglés + sha256 fail-closed** vers `~/.local/bin`, hors `/usr`. Testé de bout en bout (install `oha` OK ; hash corrompu **refusé**). Décision : **pas gravés** (redondance avec `ab`, churn/taille de `flyctl`, déploiement gouverné de toute façon).
  - **[#99] catalogue [ECOSYSTEM.md](docs/ECOSYSTEM.md)** : 3 seaux (embarqué / à la demande / référence self-hosted) + pièges de licence. **Fact-check Fable 5** des licences à la source (Redis tri-licence, Stripe Apache-2.0…).
  - **Honnêteté (invariant projet)** : la seule chose *vraiment* neuve demandée — **déployer en prod** — n'est **PAS** livrée. `deploy.*` gouverné dans `vibed` est une **capacité d'exécution nouvelle** qui attend (a) l'**allowlist de cibles de Micka**, (b) le helper-process d'[ADR-019](docs/DECISIONS.md), (c) l'isolation des credentials cloud. **[ADR-020 / PR #92](https://github.com/Micka420-collab/vibeos/pull/92) a été mergée par Micka** (2026-07-19) — la doctrine est validée, mais la *capacité* `deploy.*` (allowlist + credentials) reste sa décision d'archi, non auto-mergée.

- **2026-07-19 (dimanche) — substrat complet dev→prod + vérifié.** Sur demande explicite de Micka de continuer, le substrat SaaS/ecommerce est étendu et bouclé :
  - **3 modèles serveur de plus** ([#104]) : `observability/` (Prometheus + Grafana), `object-storage/` (SeaweedFS S3, MinIO écarté AGPL+archivé), `mailpit/` (catcher email dev) ; puis **`meilisearch/`** ([#108], recherche produit ecommerce). **6 modèles** par projet au total.
  - **Garde CI** ([#105]) : `check-saas-compose.py` — tout modèle doit publier ses ports en **loopback-only** (fail-closed). Méta-garde à 8, mutation-testé.
  - **Runbook production** ([#110]) : `/usr/share/vibeos/saas/PRODUCTION.md` — self-hosted durci (pare-feu, TLS Let's Encrypt, secrets podman/TPM2, systemd, sauvegardes). Couvre le « mettre en production » explicitement demandé.
  - **Vérifié en réel** (WSL/docker) : SeaweedFS, la stack observability (datasource auto-provisionné) et Meilisearch (auth enforced) **smoke-testés**, pas seulement affirmés.
  - **Infra** : digest de base F44 re-bumpé ([#106], quay purge ~quotidienne — cf. la décision structurelle laissée à Micka, miroir vs auto-PR).

- **2026-07-21 (session Fable 5) — perf couche basse : chemins chauds `vibed` + première config noyau de l'image** (branche `claude/session-0v4cwo`) :
  - **`vibed` : trois correctifs du chemin chaud de `tools/call`**, invariants intacts (fsync d'audit et « audit durable avant exécution » inchangés) : l'écriture d'audit passe en `spawn_blocking` **awaité** (elle s'exécutait sur le réacteur tokio — chaque fsync figeait toutes les connexions, sondes HUD comprises) ; le journal mémoire best-effort part en fire-and-forget sur le pool bloquant ; `tool_catalog()` est construit une fois (`OnceLock`) au lieu d'être rebâti à chaque lookup de tier (jusqu'à des milliers de fois par sonde `agents.list`) ; les arguments passent en `Arc` (plus de clone profond d'un payload `fs.write` par appel). **299 tests verts** (287 lib + 9 intégration MCP + 3 politique), clippy/fmt/log-hygiene OK ; compteurs des 4 README recalés 191 → 299.
  - **[ADR-025](docs/DECISIONS.md) : réglage noyau pour charges IA, livré dans l'image** — première `sysctl.d` du dépôt (`os/rootfs/usr/lib/sysctl.d/50-vibeos-ai.conf` : jeu zram cohérent `page-cluster=0`/`swappiness=180`/watermarks, writeback borné 128/256 Mio, `inotify` instances 1024, TCP BBR autochargé) + drop-in zram `zstd` (`os/rootfs/usr/lib/systemd/zram-generator.conf.d/50-vibeos.conf`, fusion par option — le fichier vendor Fedora n'est jamais écrasé). Chaque valeur sourcée (noyau vm.rst, pop-os/default-settings#163, dist-git f44, tcp_cong.c) ; le cargo cult vérifié inutile sur f44 (`max_map_count`, `file-max`) est **absent**. Kernel Fedora générique conservé (config dédiée : Phase 7+), cmdline intouchée (UKI : Phase 4).
  - **Selfcheck : 18ᵉ invariant `kernel-tuning`** (sonde read-only : `vm.page-cluster=0` + module `tcp_bbr` autochargé ; image antérieure = SKIP) — [docs/BOOT-VALIDATION.md](docs/BOOT-VALIDATION.md) recalé. La **mesure sur cible reste machine-gated** : valeurs sourcées, pas encore mesurées sur la machine de référence.

- **2026-07-21 (suite de session) — caches incrémentaux sur les chemins sondés par le HUD** (les « chantiers de suivi » listés dans la PR #166, mergée par la même session) :
  - **Queue d'audit d'`agents.list` incrémentale** : les fichiers étant append-only (écrivain unique sous le mutex de chaîne), chaque sonde ne lit et ne parse plus que le **delta** ajouté depuis la précédente, au lieu de relire 2 × 512 Kio par sonde. Invalidation fail-safe : taille qui régresse (rollback de ligne déchirée au démarrage) = re-scan complet ; rotation quotidienne purgée ; ligne sans `\n` final laissée pour la sonde suivante ; borne dure sur le cache.
  - **`memory.query fold:true` mémoïsé** par `(chemin, taille, mtime)` — c'était le seul coût par appel du démon qui croissait **sans borne** avec l'âge de la machine (re-fold de tout l'historique des updates à chaque sonde) ; désormais O(1) tant que le fichier n'a pas changé, re-fold complet sinon.
  - **Têtes de sessions (`agent.sessions`) mémoïsées** : la première ligne d'un fichier append-only est immuable — son `ts_unix` n'est plus relu à chaque sonde (jusqu'à 200 open+read+parse évités par sonde) ; une tête absente (fichier déchiré) n'est jamais mise en cache et reste re-tentée.
  - **Digest d'audit calculé une fois par appel** (`record_with_digest` + `OnceLock` partagé) : les deux records d'un même appel (`started` + final) portaient déjà le même digest mais re-sérialisaient chacun tout l'arbre d'arguments (jusqu'à ~1 Mio pour `fs.write`).
  - **331 tests verts** (dont 7 nouveaux : incrémentalité, troncature, ligne déchirée, invalidation du fold, stabilité des têtes, parité des digests) ; clippy -D warnings (2 configs), log-hygiene, fmt OK ; compteurs des 4 README recalés 299 → 331.

- **2026-07-21 (suite de session) — mode autonome / ouvert ([ADR-027](docs/DECISIONS.md), fondation livrée)** — sur demande de Micka : un mode où l'IA agit **seule** (T2/T3 sans approbation par action), avec **panneau danger**, bâti *à la manière du projet* plutôt que naïvement.
  - **Module `mode`** : deux états (`governed` défaut / `open`), enregistrement root-only `/var/lib/vibeos/mode.json` (sur la denylist d'**écriture** intégrée — `fs.write` ne peut jamais le forger), lecture **fail-safe** (tout cas incertain ⇒ governed), **auto-expiration** vérifiée à chaque évaluation, plafond dur 24 h.
  - **Câblage politique** : en mode ouvert, un `RequireApproval` (T2/T3) est **auto-accordé** — audité avec l'issue distincte `started_open_mode`/`ok_open_mode` — **hors du réacteur** (même `spawn_blocking` que la vérif de grant). N'affecte **jamais** un `deny` explicite, le default-deny, ni la denylist intégrée (audit, mode, politique, secrets).
  - **Déverrouillage HUMAIN hors-bande** : `vibectl mode open [--minutes N] [--reason R]` (root, même `require_root` qu'`approve`) ; **kill-switch** `vibectl mode governed` ; `vibectl mode status`. L'IA peut *demander*, jamais s'auto-escalader (invariant n°1).
  - **Panneau danger (données livrées)** : `os.status` porte `mode { mode, danger, remaining_secs, set_by_uid, reason }` (le HUD le sonde déjà) ; **19ᵉ invariant selfcheck `operating-mode`** (open = PASS signalé fort). Bandeau Plasma/Quickshell = présentation, à venir.
  - **Sur-couches conçues, non livrées** (ADR-027) : auto-planification crons/battements de cœur **révocables**, usage d'apps (sur `browser.*`/Wayland), auto-modification via `os.propose` (racine immuable → nouvelle image, ADR-024).
  - **340 tests verts** (dont module `mode` : fail-safe, fenêtre, clamp, kill-switch, statut, perms 0600 ; + 2 e2e : auto-grant audité `*_open_mode` & deny inchangé, `os.status`.mode danger) ; clippy -D warnings (2 configs), fmt, log-hygiene, shellcheck OK. Docs : ADR-027, BOOT-VALIDATION (19 checks), 4 README (compteur → 340, 19 invariants).

- **2026-07-21 (suite de session) — symbiose : `user.model` (T0, [ADR-028](docs/DECISIONS.md))** — sur demande de Micka : *l'agent comprend comment l'humain travaille, ce qu'il fait, et anticipe ses actions — une vraie symbiose machine/IA/humain*. Construit **vie-privée-d'abord** : un modèle **dérivé & transparent**, pas une surveillance.
  - **Module pur `tools/user_model.rs`** (`derive_user_model`, déterministe, testé) : `preferences` (fold mémoire `user/`), `patterns` (outils/cibles récurrents), `friction` (actions refusées/approuvées — candidates à pré-arranger), `rhythm` (histogramme par heure UTC), `observed` (notes de journal typées récentes), et **`anticipations`** — prochaines actions gouvernées probables classées par heuristique **fréquence×récence, chacune avec sa raison** (jamais un score opaque).
  - **Sûr & honnête par conception** : **confiné par uid** (SO_PEERCRED, comme `agent.activity`) ; **aucune fuite nouvelle** (mêmes données que `memory.query` + `agent.activity` pour le même uid, agrégées) ; **aucune surveillance nouvelle** (jamais les frappes/l'historique shell) ; **local & possédé** (`/var/lib/vibeos/memory/`) ; anticipation ≠ prédicteur entraîné (le champ `note` le dit).
  - Règle politique `user-model` (T0 allow, confiné uid) ; surface d'outils 19 → **20**.
  - **346 tests verts** (module : fréquence/récence, friction vs exécution, rythme par heure, préférences+observed plafonné, vide bien formé ; + 1 e2e : dérivation depuis de vraies actions auditées & confinement uid) ; clippy -D warnings (2 configs), fmt, log-hygiene, verify-roadmap-truth OK. Docs : ADR-028, 4 README (surface 20, compteur → 346).
  - **Sur-couches conçues (ADR-028)** : signal **opt-in** plus riche, **embeddings locaux** (ollama, `knowledge/embeddings/` réservé), branchement sur le mode ouvert (ADR-027) et `os.propose` (ADR-024) pour une IA qui propose *à bon escient*.

- **2026-07-21 (suite de session) — IA citoyenne : `agent.activity` (T0, [ADR-026](docs/DECISIONS.md))** — le citoyen relit ses **propres actes**. Deuxième brique après `policy.capabilities` (ADR-023) : là où celle-ci donne la carte *statique* des droits et `agent.thinking` la biographie des *pensées*, `agent.activity` rend la biographie des *actes* — les appels gouvernés récents de l'appelant, du plus récent au plus ancien, **refus compris** (`deny`/`require_approval`), pour apprendre par l'expérience les frontières que le manifeste ne décrit qu'en théorie. **Confiné par uid** (SO_PEERCRED), fenêtre + nombre de lignes bornés, réutilise le cache incrémental de la queue d'audit ; **aucune fuite nouvelle** (même donnée qu'`agents.list` pour son propre uid — que `agents.list` excluait justement). Règle ajoutée à `agent-observability` (T0 allow) ; surface d'outils 18 → **19**. **332 tests verts** (dont 2 nouveaux : confinement/chronologie/refus, tier T0) ; test d'intégration de politique étendu ; docs recalées (ADR-026, 4 README).

- **2026-07-22 (session Opus) — Genesis vivant, chantiers gating=none, puis gestion des tokens & mémoire structurée** (branche `claude/session-0v4cwo`, PR #173→#191). Un long arc, chaque incrément vérifié par la « gauntlet » (fmt/clippy `-D warnings`/tests/gardes) et souvent par une **revue adverse multi-angle** (chasseurs + sceptiques) :
  - **Genesis vivant ([ADR-029](docs/DECISIONS.md))** : à la naissance, **le citoyen IA choisit son propre caractère** (`personality.toml` : nom, archétype, 6 traits, ton), **unique par installation** et déterministe (FNV-1a en bash pur sur `machine_id`, hors-ligne), avec un **éveil** imprimé sur la console. Scope mémoire `personality` (6 → **7**). La boucle d'adaptation vivante et la cérémonie graphique restent machine-gated.
  - **Chantiers gating=none** : 4 chantiers OS (bandeau danger HUD, cosign client *staged*, interview de naissance opt-in, retour d'usine `vibectl memory reset`), **couche pure `embeddings` locale** + réparation du job SBOM/CVE, **couche 3 de l'extension Zed** (raisonnement/tier/journal côté éditeur) + job CI `zed`, **5 correctifs sécurité/honnêteté** vérifiés (egress CGNAT, user.model, THREAT-MODEL), **garde-fou CI « chaque outil MCP a sa ligne THREAT-MODEL »** + lock anti-régression `os.propose`.
  - **Déblocage image/ISO** : le digest de base F44 purgé par quay re-bumpé — build image + ISO à nouveau verts.
  - **Gestion des tokens ([ADR-030](docs/DECISIONS.md), [docs/TOKENS.md](docs/TOKENS.md)) — #188** : le superviseur **compte** les tokens réellement consommés (blocs `usage` du flux `stream-json`), expose les métriques d'efficacité du **cache** (le levier), les **borne** par un budget (`--tokens`) et par des **modes de consommation** (`frugale`/`équilibrée`/`performance`), et **journalise** la consommation + le coût USD autoritaire du CLI dans l'événement `autonomous_session`/end. Pas de prix en dur (multiplicateurs structurels seulement). Le temps mural reste le plafond **dur**.
  - **Mémoire structurée ([ADR-030](docs/DECISIONS.md)) — #189/#190** : `memory.query` gagne un **classement de rappel** (mode `rank:true`, opt-in, scopes `journal`/`knowledge`) — récence + importance + pertinence à la *memory stream* des *Generative Agents* (`tools/recall.rs`, pur/testé, moteur lexical câblé ; vectoriel à venir) — et une **consolidation du savoir** (`tools/consolidate.rs` : dédup de `facts.jsonl` par `(subject, fact)` last-write-wins), exposée par `vibectl memory knowledge` et `memory.query fold:true` scope `knowledge`. **Invariant THREAT-MODEL §9** : la consolidation **n'élève jamais** la confiance d'un fait (ni par répétition, ni par `source`) — elle compacte, elle ne juge pas.
  - **Vérification** : **412 tests `vibed` verts** (383 → 412 sur l'arc token+mémoire). Un **audit de correction transverse** (5 chasseurs × 2 sceptiques) sur tout le token+mémoire : **zéro bug de code** ; deux auto-contradictions doc corrigées (#191). Tout reste **T0/T1** ; **aucun nouvel outil MCP** (catalogue et surface de menace inchangés). La mesure sur cible reste machine-gated.

## 📋 Reste à faire (court terme)

> *Les 5 PR (#46→#50) qui occupaient cette liste sont **mergées** ; le dépôt n'a
> plus aucune PR ni issue ouverte. Ordre ci-dessous : ce qui débloque le reste
> d'abord.*

1. ⚠️ **Activer « Allow GitHub Actions to create and approve pull requests »**
   (*Settings → Actions → General*) — **côté utilisateur, 1 minute**. Sans ce
   réglage, `base-digest-autobump` ne peut pas ouvrir sa PR : c'est ce qui a laissé
   courir **19 jours de build cassé**. Le repli sur issue livré le 2026-08-14 rend
   la panne visible, mais ne remplace pas le réglage.
2. **Merger le bump de digest** pour rendre `main` buildable, puis **vérifier que
   `build-os` repasse vert** — tout le reste en dépend (pas d'image → pas d'ISO →
   aucun point machine-gated atteignable).
3. **Tester les ISO en VM** (côté utilisateur) : booter `vibeos-iso-amd64` en VM
   Hyper-V (Gén. 2) jusqu'à SDDM + session Plasma 6 (thème VibeOS Dark, wallpaper
   officiel, HUD au premier login) ; valider NVIDIA sur le PC de référence.
   **Rebâtir l'ISO d'abord** : celles en artefacts datent de la release et ne
   portent ni l'identité VibeOS ni les args noyau NVIDIA.
   ⚠️ **Secure Boot désactivé** jusqu'à la signature MOK (Phase 4).
   Ce seul test décoche les **3 critères Phase 2 restants**, la **Phase 1** entière
   et `/usr/libexec/vibeos/verify-sandbox` (preuve du sandbox ADR-019 sur cible).
4. **Trancher le séquencement Phase 4** — la question posée le 2026-07-14 dans la
   note de trajectoire du ROADMAP §6 est **toujours ouverte un mois plus tard**,
   et c'est la seule décision qui change la nature des mois à venir.
5. **E2E Zed Tier B** (côté utilisateur, machine avec Zed) : lancer
   `zed/vibeos-claude-acp/scripts/e2e-zed.sh` — Tier A auto, puis la checklist
   éditeur (fs.read sans prompt, pkg.install qui prompte). Le Tier A est déjà vérifié.
6. **Activer l'expédition de l'extension Zed** (`WITH_ZED_AGENT=1`) une fois le
   Tier B validé ; **brancher l'agent-runner** sur une vraie machine (TPM2 + egress live).
7. Rendre le paquet ghcr public ou le lier au dépôt (au choix).
8. Mettre à jour ce fichier + README à chaque jalon. *(Ce fichier avait 3 semaines
   de retard au 2026-08-14 — il annonçait 412 tests, 20 outils et 5 PR à merger.)*

### 🔒 Décisions bloquées sur le mainteneur (aucune ne peut avancer côté agent)

| # | Sujet | Ce qui manque, précisément |
|---|---|---|
| 1 | **[ADR-021](docs/DECISIONS.md) `deploy.*`** | `deploy.plan` est livré mais **refusé par défaut** : il manque tes cibles réelles (Fly/Vercel), le token scellé et les CIDR d'egress. Sans `[rule.deploy]`, l'outil est inerte |
| 2 | **[ADR-024](docs/DECISIONS.md) `os.propose`** | Couche de validation **pure** livrée et testée, **délibérément non câblée** (absente du catalogue) — attend la ratification de l'ADR |
| 3 | **[ADR-016](docs/DECISIONS.md) `pkg.install`** | Reste un **stub** : l'allowlist de paquets/dépôts sur OS immuable n'est pas tranchée |
| 4 | **[ADR-022](docs/DECISIONS.md) navigateur** | `chromium-headless` (~300 Mo) dans l'image : oui ou non ? + module SELinux dédié |
| 5 | **Séquencement Phase 4** | Assumer le séquencement (durcir avant la Phase 6) **ou** basculer maintenant sur le cœur Phase 4 en gelant le périmètre fonctionnel |

## 📋 Reste à faire (moyen terme — voir [ROADMAP.md](ROADMAP.md))

- **Phase 1 (v0.1)** : CI verte sur GitHub Actions, ISO amd64+arm64 bootables en VM, validation NVIDIA sur le PC de référence.
- **Phase 2 (v0.2)** : `vibed` embarqué ✅ · HUD installé + auto-démarré ✅ · Global Theme par défaut ✅ · config MCP client ✅ · `memory.append` + `scope`/`limit` ✅ · tests d'intégration MCP e2e en CI ✅ · `svc.status` + `fs.list` (T0) ✅ · groupe `vibeos-agents` automatique ✅ — les scopes `user`/`projects` de `memory.append` ✅ · **lecture du journal `log.read` (T0)** ✅ **livrée et mergée** ([ADR-011](docs/DECISIONS.md), [#37](https://github.com/Micka420-collab/vibeos/pull/37)) : allowlist d'unités évaluée **avant** le plancher de tier, sortie bornée, rédaction heuristique **non garantie** (documentée comme telle) · **branchement live du HUD** ✅ — **LIVRABLES COMPLETS (2026-07-15)**, y compris l'outil T1 que le ROADMAP demande (« écriture dans les fichiers et la configuration de l'utilisateur » = `fs.write`, confiné au home de l'appelant + symlink-safe).
  **Critères de sortie : 4/7 vérifiés**, chacun avec sa preuve dans [ROADMAP.md](ROADMAP.md) §4 — T2/T3 refusé+audité, politique invalide fail-closed, CI verte (clippy+audit), **aucune fuite dans les logs** (26 appels `tracing` relus + garde CI `scripts/check-log-hygiene.py`).
  **Les 3 restants sont MACHINE-GATED**, pas « à faire » : `vibed` sain après `kill -9`, handshake depuis le **vrai** Claude Code sur le **vrai** socket, démo T0+T1 tracée à l'audit. → **objet du test d'ISO**.
  *Hors ROADMAP* : « outils T1 réels **supplémentaires** » et preset **Panel Colorizer** ne sont **pas** des critères Phase 2 (le premier n'est pas au ROADMAP, le second est une revendication de `docs/DESKTOP.md`) — et tous deux sont **gouvernés** : un T1 de plus exige *quelle allowlist de cibles*, Panel Colorizer est un plasmoid tiers GPL-3.0 hors dépôts Fedora (même TOFU de supply-chain que les clés GPG). **Décisions Micka.**
- **Phase 2.5 (v0.2.5) — « Autonomie encadrée & accès IA externes »** *(proposée, ~3-5 semaines)* : superviseur d'agent budgété (`vibectl agent run`, budget wall-clock + nombre d'appels, **kill-switch humain** `vibectl agent stop`), **auth par abonnement** (`claude setup-token`/`codex`) scellée **TPM2** via `systemd-creds`, **allowlist d'egress par unité** (`IPAddressAllow/Deny`), type de journal réservé `autonomous_session`. **Périmètre figé T0/T1** (pas d'anticipation T2/T3). Voir [ROADMAP.md](ROADMAP.md) §4 bis.
- **Phase 3 (v0.3)** : mémoire chiffrée LUKS/TPM2, mode amnésique (generator), interview de naissance câblée, **sandbox par outil (systemd-run, seccomp, landlock)**.
  **État réel mesuré le 2026-08-14** : le **sandbox par outil est implémenté**
  ([ADR-019](docs/DECISIONS.md) : unités systemd transitoires + `SystemCallFilter` +
  helper de faible privilège `vibed-tool` + outil `sandbox.probe`) mais **jamais
  prouvé sur cible** — `/usr/libexec/vibeos/verify-sandbox` attend un boot.
  Le generator amnésique et le retour d'usine (`vibectl memory reset`) sont livrés,
  **non validés au boot**. L'interview de naissance existe
  (`agent/genesis_interview.py`, opt-in) mais n'est pas câblée bout-en-bout.
  ⛔ **LUKS = zéro** : aucun `crypttab` ni `cryptsetup` dans `os/` — c'est le
  **gros du travail Phase 3 restant**, et il n'est bloqué par personne.
- **Phase 4 (v0.4)** : durcissement (UKI / boot mesuré, SELinux dédiée `vibed_t`, hash-chaining audit, `User=vibed`).
  **État réel mesuré le 2026-08-14 — le cœur est toujours à zéro**, un mois après
  la note de trajectoire : **aucun fichier SELinux** (`.te`/`.fc`/`.if` absents du
  dépôt), **aucun UKI / dm-verity / composefs**, et **`vibed` tourne toujours en
  root** (`vibed.service` le documente honnêtement, ligne 11). Restent aussi la
  **vérification cosign côté client** (staged, pas *enforced*), l'**ancrage externe
  TPM/Rekor** de la chaîne d'audit, et la **signature MOK** des kmods NVIDIA — qui
  bloque Secure Boot sur la machine de référence. Seul le durcissement
  *périphérique* (chaîne d'audit SHA-256 + rotation) est fait.
  → **Ne pas démarrer sans un go explicite du mainteneur** (ROADMAP §6).
- **Phase 5 (v0.5)** : installateur brandé + identité visuelle complète.
- **Phase 6 (v1.0)** : release publique.

## 🧭 Comment reprendre le travail

1. Lire ce fichier, puis [ROADMAP.md](ROADMAP.md) (phases + critères de sortie) et [docs/DECISIONS.md](docs/DECISIONS.md) (ADR).
2. Environnement : Windows 11 + WSL2 Ubuntu (`podman`, `cargo`, `qemu`) ; dépôt dans `F:\je ne sais pas encore`.
3. Build local : voir [docs/BUILD.md](docs/BUILD.md). Tests Rust : `wsl -d Ubuntu -- bash -c "cd '/mnt/f/je ne sais pas encore/vibed' && cargo test"`.
4. Les règles inviolables du projet : [VISION.md](VISION.md) (principes) et [SECURITY.md](SECURITY.md) (jamais affirmer au présent ce qui n'est pas livré).
