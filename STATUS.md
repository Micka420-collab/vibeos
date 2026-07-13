# 📊 STATUS — État d'avancement de VibeOS

> **Fichier vivant** : mis à jour à chaque session de travail. C'est le point d'entrée pour reprendre le projet — le « où en est-on, que reste-t-il ».
> Dernière mise à jour : **2026-07-13** (journée d'amélioration continue : CI durcie, sécurité vibed, `svc.status`/`fs.list`, chaîne agent→vibed out-of-box, wallpaper officiel par défaut, recalage documentaire — PR #4).

## Vue d'ensemble

| | |
|---|---|
| **Phase actuelle** | Phase 1 « Première ISO » (reste : validation VM + NVIDIA) · Phase 2 « vibed + MCP » 🔄 bien avancée |
| **Version** | 0.1.0-dev (pré-alpha) |
| **Dépôt GitHub** | [`Micka420-collab/vibeos`](https://github.com/Micka420-collab/vibeos) (privé) ✅ en ligne |
| **Image OS** | `ghcr.io/micka420-collab/vibeos:0.1.0-dev` — **manifest multi-arch amd64+arm64 PUBLIÉ + SIGNÉ cosign** ✅ (Rekor #2062451740) |
| **ISO** | **amd64 + arm64 générées** ✅ — artefacts CI `vibeos-iso-amd64` (7,0 Go) / `vibeos-iso-arm64` (6,3 Go) ; ISO amd64 aussi en local `F:\VibeOS-ISO\` |
| **Release** | **v0.1.0-dev — CI entièrement verte** (runners natifs) : build amd64+arm64 → manifest → cosign → 2 ISO |
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

## 📋 Reste à faire (court terme)

1. **Merger les 4 draft PRs empilées** (côté utilisateur, dans l'ordre) : [#1 HUD/thème/MCP client](https://github.com/Micka420-collab/vibeos/pull/1) → [#2 memory.append](https://github.com/Micka420-collab/vibeos/pull/2) → [#3 supply-chain](https://github.com/Micka420-collab/vibeos/pull/3) → [#4 améliorations 2026-07-13](https://github.com/Micka420-collab/vibeos/pull/4).
2. **Tester les ISO en VM** (côté utilisateur) : booter `vibeos-iso-amd64` en VM Hyper-V (Gén. 2) jusqu'à SDDM + session Plasma 6 (désormais : thème VibeOS Dark, wallpaper officiel, HUD au premier login) ; valider NVIDIA sur le PC de référence.
3. **Brancher le HUD en live** : câbler le QML sur le socket (`Quickshell.Io` `Socket` + `SplitParser` ↔ `/run/vibed/mcp.sock`) — le HUD est installé et auto-démarré mais affiche des données mockées (« vibed hors ligne »).
4. Rendre le paquet ghcr public ou le lier au dépôt (au choix).
5. Mettre à jour ce fichier + README à chaque jalon.

## 📋 Reste à faire (moyen terme — voir [ROADMAP.md](ROADMAP.md))

- **Phase 1 (v0.1)** : CI verte sur GitHub Actions, ISO amd64+arm64 bootables en VM, validation NVIDIA sur le PC de référence.
- **Phase 2 (v0.2)** : `vibed` embarqué ✅ · HUD installé + auto-démarré ✅ · Global Theme par défaut ✅ · config MCP client ✅ · `memory.append` + `scope`/`limit` ✅ · tests d'intégration MCP e2e en CI ✅ · `svc.status` + `fs.list` (T0) ✅ · groupe `vibeos-agents` automatique ✅ — restent les **outils T1 réels** supplémentaires, les scopes `user`/`projects` de `memory.append`, la **lecture du journal** (T0, à concevoir contre la fuite de secrets via logs), le **branchement live du HUD** et le preset **Panel Colorizer**.
- **Phase 3 (v0.3)** : mémoire chiffrée LUKS/TPM2, mode amnésique (generator), interview de naissance câblée, **sandbox par outil (systemd-run, seccomp, landlock)**.
- **Phase 4 (v0.4)** : durcissement (UKI / boot mesuré, SELinux dédiée `vibed_t`, hash-chaining audit, `User=vibed`).
- **Phase 5 (v0.5)** : installateur brandé + identité visuelle complète.
- **Phase 6 (v1.0)** : release publique.

## 🧭 Comment reprendre le travail

1. Lire ce fichier, puis [ROADMAP.md](ROADMAP.md) (phases + critères de sortie) et [docs/DECISIONS.md](docs/DECISIONS.md) (ADR).
2. Environnement : Windows 11 + WSL2 Ubuntu (`podman`, `cargo`, `qemu`) ; dépôt dans `F:\je ne sais pas encore`.
3. Build local : voir [docs/BUILD.md](docs/BUILD.md). Tests Rust : `wsl -d Ubuntu -- bash -c "cd '/mnt/f/je ne sais pas encore/vibed' && cargo test"`.
4. Les règles inviolables du projet : [VISION.md](VISION.md) (principes) et [SECURITY.md](SECURITY.md) (jamais affirmer au présent ce qui n'est pas livré).
