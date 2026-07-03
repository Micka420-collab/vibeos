# 📊 STATUS — État d'avancement de VibeOS

> **Fichier vivant** : mis à jour à chaque session de travail. C'est le point d'entrée pour reprendre le projet — le « où en est-on, que reste-t-il ».
> Dernière mise à jour : **2026-07-03 ~12h40** (session de travail continue).

## Vue d'ensemble

| | |
|---|---|
| **Phase actuelle** | Phase 0 « Fondation » ✅ → Phase 1 « Première ISO » (en cours) |
| **Version** | 0.1.0-dev (pré-alpha) |
| **Dépôt GitHub** | [`Micka420-collab/vibeos`](https://github.com/Micka420-collab/vibeos) (privé) ✅ en ligne |
| **Image OS** | `ghcr.io/micka420-collab/vibeos` (amd64 + arm64) — **build local + CI amd64 verts** ✅ (`bootc lint` : 13 checks OK) |
| **ISO** | **`install.iso` amd64 générée** ✅ 5,6 Go, bootable (ISO 9660 / Fedora installer) — copiée dans `F:\VibeOS-ISO\` |
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

## 🔄 En cours (aujourd'hui)

- **Câblage de la couche terminal vibecoding** dans `os/Containerfile` (Ghostty, fish, Starship, Zellij, yazi, lazygit, atuin, zoxide, bat, eza, btop, mise, nvim) pour livrer l'expérience de `docs/ECOSYSTEM.md` dans l'image — les dotfiles `/etc/skel` sont déjà en place.
- **Test de l'ISO en VM Hyper-V** (à faire côté Windows) : boot jusqu'à SDDM + session Plasma 6.

## 📋 Reste à faire (court terme)

1. Intégrer les correctifs bureau + commit/push (bureau, installateur, dotfiles).
2. Générer la première **ISO** amd64 (bootc-image-builder) depuis l'image locale + test de boot en VM Hyper-V (Gén. 2).
3. Câbler le paquet **Quickshell** et le thème dans `os/Containerfile` (livrer le HUD et VibeOS Dark dans l'image) + ajouter la couche « terminal vibecoding » (Ghostty, fish, Starship, Zellij, yazi, lazygit, atuin, zoxide, bat, eza, btop, mise, opencode, nvim) — voir [docs/ECOSYSTEM.md](docs/ECOSYSTEM.md).
4. Relancer la **CI sur un tag `v0.1.0-dev`** une fois le build validé (build multi-arch + cosign + ISO en artefacts).
5. Rendre le paquet ghcr public ou le lier au dépôt (au choix).
6. Mettre à jour ce fichier + README à chaque jalon.

## 📋 Reste à faire (moyen terme — voir [ROADMAP.md](ROADMAP.md))

- **Phase 1 (v0.1)** : CI verte sur GitHub Actions, ISO amd64+arm64 bootables en VM, validation NVIDIA sur le PC de référence.
- **Phase 2 (v0.2)** : `vibed` livré dans l'image (étage cargo multi-stage), premiers outils T0/T1 utilisables depuis Claude Code via MCP.
- **Phase 3 (v0.3)** : mémoire chiffrée LUKS/TPM2, mode amnésique (generator), interview de naissance câblée.
- **Phase 4 (v0.4)** : durcissement (UKI, SELinux dédiée, sandbox par outil, hash-chaining audit, User=vibed).
- **Phase 5 (v0.5)** : installateur brandé + identité visuelle complète.
- **Phase 6 (v1.0)** : release publique.

## 🧭 Comment reprendre le travail

1. Lire ce fichier, puis [ROADMAP.md](ROADMAP.md) (phases + critères de sortie) et [docs/DECISIONS.md](docs/DECISIONS.md) (ADR).
2. Environnement : Windows 11 + WSL2 Ubuntu (`podman`, `cargo`, `qemu`) ; dépôt dans `F:\je ne sais pas encore`.
3. Build local : voir [docs/BUILD.md](docs/BUILD.md). Tests Rust : `wsl -d Ubuntu -- bash -c "cd '/mnt/f/je ne sais pas encore/vibed' && cargo test"`.
4. Les règles inviolables du projet : [VISION.md](VISION.md) (principes) et [SECURITY.md](SECURITY.md) (jamais affirmer au présent ce qui n'est pas livré).
