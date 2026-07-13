# 🧩 Écosystème VibeOS — sélection open-source curée

> Issue d'une veille GitHub multi-sondes (**113 projets candidats**, 7 catégories) suivie d'une curation stricte : licence redistribuable dans une ISO, maintenance active, offline-first, compatibilité OS immuable (bootc), non-redondance (« un seul terminal, un seul prompt, une seule distro Neovim »).
> Datée du **2026-07-03** — à re-vérifier avant chaque intégration (versions, licences, activité).

## ✅ Correction bloquante — appliquée

**Les binaires officiels VS Code sont propriétaires et NON redistribuables dans une ISO.** L'image les remplace par **VSCodium** (MIT, marketplace Open VSX où Cline et Continue sont publiés). → Appliqué dans `os/Containerfile` (paquet `codium`, dépôt paulcarroty) avant la première release — VS Code n'a jamais été embarqué.

## 🎯 L'expérience visée (synthèse du curateur)

On démarre sur un **Plasma 6 brandé** — fork Catppuccin « VibeOS Dark », panels restylés (Panel Colorizer), **HUD Quickshell** (même stack QML que Plasma) affichant l'état des agents, les tiers de policy et les jauges ollama — construit sur le blueprint bootc éprouvé d'Universal Blue/Bazzite. Le cœur du vibecoding est le **terminal** : Ghostty + fish + Starship + Zellij avec des layouts préconfigurés « agent + lazygit + logs » ; yazi/bat/eza/zoxide/btop donnent une visibilité immédiate sur ce que les agents créent ; Atuin (sync désactivée) fournit la piste d'audit de tout ce qu'ils lancent. Les CLIs embarqués (Claude Code, Gemini CLI, Codex, **opencode** — MIT, multi-provider, 100 % local via ollama) couvrent le vibecoding cloud et local ; `aider` n'est plus livré par défaut (il exige Python < 3.13, incompatible avec le Python 3.13 de la base Fedora Kinoite 42) mais reste installable à la demande via `uvx --python 3.12 aider-chat`. L'édition : **Neovim preset « VibeVim »** (LazyVim + codecompanion avec support ACP pilotant les agents depuis l'éditeur) et **VSCodium** ; **Zed** en option GUI AI-native. La couche **MCP par défaut est courte et 100 % offline** (filesystem/git/fetch/memory de référence + **Playwright** pour le navigateur + **Serena** pour la navigation sémantique LSP), enveloppée dans les policy tiers ; secrets gérés par **age + SOPS + systemd-creds** (TPM2). **mise** + distrobox = toolchains reproductibles sans toucher `/usr` ; **sqlite-vec** = mémoire vectorielle embarquée sans démon. Tout le reste s'installe en une commande sans casser l'immuabilité. VibeOS ne ship que ce qu'il a juridiquement le droit de shipper, et tout ce qu'il ship marche offline ou est gated par un tier de policy explicite.

## 🟢 Niveau 1 — Livré dans l'image (`ship_default`)

| Projet | Repo | Licence | Vecteur | Rôle |
|---|---|---|---|---|
| opencode | sst/opencode | MIT | npm (`opencode-ai@1.17.13`) | Agent terminal multi-provider (75+ modèles dont ollama) livré par défaut ; remplace aider comme CLI de pair-programming multi-fournisseur (aider incompatible Python 3.13) |
| **VSCodium** | VSCodium/vscodium | MIT | rpm (dépôt paulcarroty) | **Remplace VS Code** (propriétaire) ; extensions via Open VSX |
| Neovim | neovim/neovim | Apache-2.0 | rpm Fedora | Socle terminal-first de l'écosystème IA nvim |
| LazyVim | LazyVim/LazyVim | Apache-2.0 | template `/etc/skel` | Base du preset **« VibeVim »** livré par défaut |
| codecompanion.nvim | olimorris/codecompanion.nvim | Apache-2.0 | lazy.nvim | IA dans nvim : ollama natif + **ACP** (pilote Claude Code/Gemini CLI) |
| Ghostty | ghostty-org/ghostty | MIT | COPR/Terra | LE terminal (GPU, kitty graphics) ; kitty = plan B documenté |
| fish | fish-shell/fish-shell | GPL-2.0 | rpm Fedora | Shell interactif zéro-config (bash reste `/bin/sh`) |
| Starship | starship/starship | ISC | COPR (atim/starship) | Prompt unique fish/bash/zsh, contexte git visible |
| Zellij | zellij-org/zellij | MIT | COPR (varlad/zellij) | Multiplexeur « humain », layouts KDL agent+lazygit+logs, résurrection post-update |
| Yazi | sxyazi/yazi | MIT | COPR/binaire | File manager TUI, aperçus des fichiers créés par les agents |
| lazygit | jesseduffield/lazygit | MIT | COPR/binaire | Reviewer/committer les diffs des agents |
| Atuin | atuinsh/atuin | MIT | rpm/binaire | Historique = piste d'audit des commandes agents (sync opt-in chiffrée) |
| zoxide | ajeetdsouza/zoxide | MIT | rpm Fedora | Navigation rapide entre projets |
| bat | sharkdp/bat | Apache/MIT | rpm Fedora | Lecture colorée + previewer fzf/yazi |
| eza | eza-community/eza | EUPL-1.2 | rpm Fedora | ls moderne avec statut git (licence OSI, à documenter) |
| btop | aristocratos/btop | Apache-2.0 | rpm Fedora | Monitoring **GPU** pendant l'inférence ollama |
| mise | jdx/mise | MIT | rpm officiel/binaire | Toolchains dans `$HOME`, parfait bootc ; gestionnaire unique |
| systemd-creds | (base Fedora) | LGPL-2.1+ | déjà présent | Scellement TPM2, un secret par service agent |
| Catppuccin KDE | catppuccin/kde | MIT | fichiers dans l'image (schéma `VibeOSDark.colors` livré) | Base MIT forkée en thème **« VibeOS Dark »** |
| uBlue/Bazzite (blueprint) | ublue-os/image-template | Apache-2.0 | modèle GitHub | Le manuel éprouvé du branding Fedora bootc (plymouth, SDDM, Anaconda, ISO CI) |

## 🟡 Niveau 1-bis — Sélectionné, pas encore dans l'image (Phase 2 / à intégrer)

> Retenus pour le défaut mais **absents d'`os/Containerfile` en v0.1** — listés ici pour ne pas les présenter comme livrés.

| Projet | Licence | Cible | Rôle |
|---|---|---|---|
| MCP Reference Servers | MIT | 🛣️ Phase 2 (registre MCP servi par `vibed`) | Socle MCP offline : filesystem, git, fetch, memory, time |
| Playwright MCP | Apache-2.0 | 🛣️ Phase 2 | L'agent teste l'app web qu'il vient d'écrire (snapshots a11y) |
| Serena | MIT | 🛣️ Phase 2 | Navigation/édition **symbolique LSP** (40+ langages) — multiplicateur pour agents |
| Quickshell | LGPL-3.0 | ✅ livré (compilé depuis les sources — aucun paquet f42) | **HUD agents signature** (Qt6/QML comme Plasma) : statut agents, tiers, jauges |
| Plasma Panel Colorizer | GPL-3.0 | 🛣️ Phase 2 (chantier bureau) | Identité visuelle des panels sans quitter Plasma supporté |
| age | BSD-3 | À intégrer (pas encore dans le Containerfile) | Chiffrement minimaliste des secrets |
| SOPS | MPL-2.0 | À intégrer | Secrets chiffrés versionnables (backend age), CNCF |
| sqlite-vec | MIT/Apache | À intégrer | Mémoire vectorielle embarquée, zéro démon (pré-v1 documenté) |

## 🔵 Niveau 2 — En un clic (`offer_optional`)

| Projet | Licence | Vecteur | Rôle |
|---|---|---|---|
| aider | Apache-2.0 | uvx / uv tool (Python 3.12) | Pair-programming CLI conscient de git — **plus livré par défaut** (exige Python < 3.13, incompatible avec la base Python 3.13) ; `uvx --python 3.12 aider-chat` (éphémère) ou `uv tool install --python 3.12 aider-chat` (persistant, `~/.local`), sans toucher l'OS immuable |
| Zed | GPL-3.0+ | flatpak/binaire | Éditeur GUI AI-native, front-end ACP des agents livrés |
| Goose | Apache-2.0 | binaire | Agent MCP-natif Rust (Linux Foundation) |
| OpenHands | MIT (hors enterprise/) | container podman | Agent longue durée sandboxé |
| Cline | Apache-2.0 | Open VSX | Agent en extension, human-in-the-loop |
| Continue | Apache-2.0 | Open VSX | Pont IDE↔ollama : complétion/chat/agent offline |
| Tabby | Apache-2.0 (hors ee/) | container/binaire | Copilot 100 % offline |
| llama.cpp | MIT | rpm/container | Moteur brut power-user (llama-server) |
| RamaLama | MIT | rpm Fedora | LLM en conteneurs OCI (org containers/Red Hat) |
| llama-swap | MIT | binaire | Alternance de modèles pour les **8 Go de VRAM** de la 3070 Ti |
| LiteLLM | MIT (hors enterprise/) | pip/container | Routage local/cloud selon policy, budgets par clé |
| Jan | Apache-2.0 | AppImage | Chat desktop offline (en attendant Alpaka) |
| avante.nvim / claudecode.nvim / mcphub.nvim | Apache/MIT | lazy.nvim | Extras IA du preset VibeVim |
| Helix | MPL-2.0 | rpm Fedora | Éditeur modal d'appoint |
| FastMCP | Apache-2.0 | pip | Générer ses propres serveurs MCP (auto-extension) |
| GitHub MCP Server | MIT | binaire/container | Brique forge (réseau+token → tier supérieur, opt-in) |
| MCP Inspector | MIT | npx | Debug visuel des serveurs MCP |
| Context7 | MIT (backend hébergé) | npx | Docs à jour dans le contexte — **pas offline**, opt-in tier réseau |
| Langflow | MIT | pip/container | Atelier visuel de flows (Flowise écarté : redondant) |
| CrewAI | MIT | pip (toolbox) | Équipes d'agents scriptées |
| Dev Container CLI | MIT | npm+podman | Standard devcontainer.json, complément distrobox |
| direnv | MIT | rpm Fedora | Compat .envrc (mise couvre le besoin en défaut) |
| Chroma | Apache-2.0 | pip/quadlet | Cran au-dessus de sqlite-vec (RAG) |
| Logseq | AGPL-3.0 ⚠️ | flatpak | Mémoire projet en Markdown greppable par agents |
| Kando | MIT (assets ⚠️) | flatpak | Pie menu → actions agents via IPC |
| KDE Material You Colors / Kvantum / cava / Konsave / Smart Video Wallpaper | GPL | pip/rpm/KDE Store | Theming dynamique & immersion (« vibes » exportables) |

## 🟣 Niveau 3 — Roadmap (`phase_later`)

| Projet | Pourquoi plus tard |
|---|---|
| ToolHive (Apache-2.0) | Runtime d'isolation MCP par conteneur = l'implémentation la plus proche de nos policy tiers ; intégration profonde (Phase 2-3) |
| Alpaka (GPL-3.0+) | Chat **KDE natif** branché sur ollama — promouvoir en défaut dès sa release stable |
| LocalAI, Letta, Mem0/OpenMemory | Multimodal + mémoire d'agents persistante (Phase 3 mémoire v2) |
| Temporal, Windmill (AGPL sans EE) | Runs d'agents longue durée / automatisation self-hosted |
| Nix, Qdrant, SWE-agent | Reproductibilité max / mémoire « production » / benchmarks |
| SDDM Astronaut, DankMaterialShell, matugen | Chantier branding Phase 5 (login animé, session « focus », theming propagé) |

## 🔴 Rejetés (licence ou maintenance) — avec raison

| Projet | Raison |
|---|---|
| Crush (charmbracelet) | FSL-1.1 source-available (clause « Competing Use ») — redistribution ISO risquée ; opencode couvre en MIT |
| Open WebUI | Licence non-OSI depuis v0.6.6 (clause branding + CLA) — recette container utilisateur uniquement |
| n8n | Sustainable Use License (non-OSS, redistribution commerciale interdite) — Windmill retenu en roadmap |
| Dify | Apache modifié (multi-tenant interdit, logo verrouillé) — Langflow (MIT) à la place |
| Roo Code | Dépôt **archivé** 05/2026, l'équipe redirige vers Cline |
| Void | Fork VS Code **archivé** 06/2026 |
| GPT4All | Non maintenu depuis 02/2025 |
| AutoGen | Mode maintenance, Microsoft pousse agent-framework |
| WezTerm | Dernière stable 02/2024, signal d'abandon |
| Plymouth adi1090x | Non maintenu + provenance d'assets floue → thème Plymouth **original VibeOS** à créer |
| kitty / Alacritty / zsh / television / Kilo Code / AstroNvim / NvChad / sidekick.nvim / Lapce | Règle de non-redondance (alternatives documentées) |

## Intégration — plan d'action

1. **Phase 1 (image)** : ✅ fait — VSCodium livré (paquet `codium`) ; couche « terminal vibecoding » (Ghostty, fish, Starship, Zellij, yazi, lazygit, atuin, zoxide, bat, eza, btop, mise, opencode, nvim) + `/etc/skel` (preset VibeVim, layouts Zellij, config fish/starship) livrées. **Exception : age/SOPS et sqlite-vec ne sont pas intégrés à l'image** — recalés en cible ultérieure (voir niveau 1-bis).
2. **Phase 2 (vibed+MCP)** : registre MCP par défaut (filesystem/git/fetch/memory + Playwright + Serena) **enveloppé dans les policy tiers** ; systemd-creds pour les clés.
3. **Bureau (chantier dédié)** : thème « VibeOS Dark » (fork Catppuccin), Panel Colorizer presets, HUD Quickshell (statut agents/tiers/ollama), sur le blueprint uBlue.
4. **Phase 5 (branding)** : Plymouth original, SDDM (base Astronaut), session focus optionnelle.
