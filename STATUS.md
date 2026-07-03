# 📊 STATUS — État d'avancement de VibeOS

> **Fichier vivant** : mis à jour à chaque session de travail. C'est le point d'entrée pour reprendre le projet — le « où en est-on, que reste-t-il ».
> Dernière mise à jour : **2026-07-03 ~12h10** (session de travail continue).

## Vue d'ensemble

| | |
|---|---|
| **Phase actuelle** | Phase 0 « Fondation » → transition Phase 1 « Première ISO » |
| **Version** | 0.1.0-dev (pré-alpha) |
| **Dépôt GitHub** | `Micka420-collab/vibeos` (privé) — *création en cours* |
| **Image OS** | `ghcr.io/micka420-collab/vibeos` (amd64 + arm64) — *premier build à venir* |
| **Machine de référence** | Ryzen 7 3700X · RTX 3070 Ti · 16 Go — voir [docs/HARDWARE.md](docs/HARDWARE.md) |

## ✅ Fait

- **2026-07-03** — Décisions d'architecture actées (8 ADR) : base immuable bootc/OSTree sur Fedora Kinoite 42 (KDE Plasma 6), daemon `vibed` (Rust) exposant le contrôle OS via MCP, politique T0–T3 avec approbation humaine T2+, mémoire « Genesis » créée au premier boot, IA hybride cloud+local.
- **2026-07-03** — Fondation du dépôt générée (31+ fichiers) : vision/manifeste, architecture, roadmap pluriannuelle (v0.1 → v1.0 mi-2028 → souveraineté progressive), Containerfile bootc, crate Rust `vibed` complet (policy/mcp/audit + tests), sous-système mémoire (spec + `genesis.sh` testé), modèle de menace + politiques.
- **2026-07-03** — Revue croisée à 3 lentilles (cohérence, réalisme sécurité, complétude) : ~50 problèmes identifiés, dont 9 majeurs (schéma de politique incompatible avec le parseur, politiques non installées dans l'image, `fs.read` sans garde-fous, approbation T2 non modélisée, survente documentaire).
- **2026-07-03** — 20 décisions canoniques figées pour trancher toutes les contradictions (schéma TOML riche, first-match, default-deny absolu, fail-closed, audit `/var/lib/vibeos/audit/vibed.jsonl`, socket `root:vibeos-agents 0660`…).
- **2026-07-03** — Environnement de build local opérationnel : WSL2 Ubuntu 24.04 + podman 4.9 + buildah + qemu-user-static (builds arm64) + rust 1.75 + shellcheck + skopeo.
- **2026-07-03** — Supply chain épinglée : image de base par digest, CLIs IA versionnées (claude-code 2.1.199, agent-sdk 0.3.199, gemini-cli 0.49.0, codex 0.142.5, aider 0.86.2).
- **2026-07-03** — Profil matériel de référence documenté ([docs/HARDWARE.md](docs/HARDWARE.md)) : multi-arch amd64+arm64, couche NVIDIA/CUDA amd64 uniquement, checklist de validation avant flash.

## 🔄 En cours (aujourd'hui)

- **Passe de corrections** : 5 agents appliquent les 50 findings (le moteur de politique Rust est réécrit au schéma canonique et doit passer `cargo test` dans WSL2 ; CI multi-arch + signature cosign ; couche NVIDIA ; honnêteté documentaire « livré v0.1 vs Phase N ») + vérification finale.
- ✅ **Recherche écosystème OSS terminée** (113 candidats → curation) : voir [docs/ECOSYSTEM.md](docs/ECOSYSTEM.md). Stack vibecoding définie : Ghostty+fish+Starship+Zellij, preset « VibeVim » (LazyVim+codecompanion/ACP), opencode, MCP offline (filesystem/git + Playwright + Serena), age/SOPS/systemd-creds, HUD Quickshell + thème « VibeOS Dark » (fork Catppuccin), blueprint uBlue/Bazzite.
- ⚠️ **Correction bloquante à appliquer** : remplacer VS Code (binaires propriétaires, non redistribuables dans une ISO) par **VSCodium** dans `os/Containerfile`.

## 📋 Reste à faire (court terme)

1. Vérifier la passe de corrections (dont `cargo test` vert) et corriger les résidus.
2. `git init` + commit initial + création du dépôt **privé** GitHub `Micka420-collab/vibeos` + push.
3. Premier **build local** de l'image amd64 dans WSL2 (`podman build`) — itérer jusqu'au build vert.
4. Génération de la première **ISO** (bootc-image-builder) + test en VM Hyper-V.
5. Intégrer la sélection OSS curée → concevoir le **bureau vibecoding** (KDE Plasma 6 : layout, thème, palette d'agents) et la **belle interface d'installation** (Phase 5 anticipée en spec).
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
