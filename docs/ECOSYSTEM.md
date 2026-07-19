# 🧩 Écosystème VibeOS — sélection open-source curée

> Issue d'une veille GitHub multi-sondes (**113 projets candidats**, 7 catégories) suivie d'une curation stricte : licence redistribuable dans une ISO, maintenance active, offline-first, compatibilité OS immuable (bootc), non-redondance (« un seul terminal, un seul prompt, une seule distro Neovim »).
> Datée du **2026-07-03** — à re-vérifier avant chaque intégration (versions, licences, activité).

## ✅ Correction bloquante — appliquée

**Les binaires officiels VS Code sont propriétaires et NON redistribuables dans une ISO.** L'image les remplace par **VSCodium** (MIT, marketplace Open VSX où Cline et Continue sont publiés). → Appliqué dans `os/Containerfile` (paquet `codium`, dépôt paulcarroty) avant la première release — VS Code n'a jamais été embarqué.

## 🎯 L'expérience visée (synthèse du curateur)

On démarre sur un **Plasma 6 brandé** — fork Catppuccin « VibeOS Dark », panels restylés (Panel Colorizer), **HUD Quickshell** (même stack QML que Plasma) affichant l'état des agents, les tiers de policy et les jauges ollama — construit sur le blueprint bootc éprouvé d'Universal Blue/Bazzite. Le cœur du vibecoding est le **terminal** : Ghostty + fish + Starship + Zellij avec des layouts préconfigurés « agent + lazygit + logs » ; yazi/bat/eza/zoxide/btop donnent une visibilité immédiate sur ce que les agents créent ; Atuin (sync désactivée) fournit la piste d'audit de tout ce qu'ils lancent. Les CLIs embarqués (Claude Code, Gemini CLI, Codex, **opencode** — MIT, multi-provider, 100 % local via ollama) couvrent le vibecoding cloud et local ; `aider` n'est plus livré par défaut (il exige Python < 3.13, incompatible avec le Python ≥ 3.13 de la base Fedora Kinoite 44) mais reste installable à la demande via `uvx --python 3.12 aider-chat`. L'édition : **Neovim preset « VibeVim »** (LazyVim + codecompanion avec support ACP pilotant les agents depuis l'éditeur) et **VSCodium** ; **Zed** en option GUI AI-native. La couche **MCP par défaut est courte et 100 % offline** (filesystem/git/fetch/memory de référence + **Serena** pour la navigation sémantique LSP), enveloppée dans les policy tiers ; le **navigateur** ne passe PAS par un MCP tiers mais par des outils `browser.*` servis par `vibed` lui-même ([ADR-017](docs/DECISIONS.md)) ; secrets gérés par **age + SOPS + systemd-creds** (TPM2). **mise** + distrobox = toolchains reproductibles sans toucher `/usr` ; **sqlite-vec** = mémoire vectorielle embarquée sans démon. Tout le reste s'installe en une commande sans casser l'immuabilité. VibeOS ne ship que ce qu'il a juridiquement le droit de shipper, et tout ce qu'il ship marche offline ou est gated par un tier de policy explicite.

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
| ~~Playwright MCP~~ | Apache-2.0 | ❌ **écarté** ([ADR-017](DECISIONS.md)) | Remplacé par des outils `browser.*` **dans `vibed`**. Un serveur MCP tiers auquel l'agent parle **directement** n'est pas gouverné : hors politique, hors tiers, hors audit — le reproche exact fait à BrowserOS. Et Playwright se bat avec bootc : Node requis, binaires téléchargés d'un CDN dans un cache **mutable**, Fedora non supportée, pas de Chrome arm64 |
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

## 🚀 Trousse SaaS + ecommerce — la seconde trousse gouvernée

> Ajoutée le **2026-07-18** ([ADR-020](DECISIONS.md), [PR #92](https://github.com/Micka420-collab/vibeos/pull/92)). Même doctrine que la cybersécurité : une **trousse curée et gouvernée**, pas un fourre-tout. Trois seaux selon **où** vit l'outil, jamais **ce qu'il fait** (leçon d'ADR-017).
>
> **Le partage clé (revue Fable 5) : outils passifs vs serveurs.** Un CLIENT (`psql`, `redis-cli`) et un orchestrateur (`podman-compose`) sont des outils passifs → ils vont dans l'image. Un SERVEUR (PostgreSQL, Valkey, Caddy) est un **service réseau persistant à état** → il n'entre **jamais** dans une image immuable ; il tourne en **conteneur par projet**, sous l'uid de l'utilisateur, état dans des volumes du projet.

### Seau A — Outils embarqués dans l'image (`ship_default`, couche 1d-ter)

> Livrés par [PR #95](https://github.com/Micka420-collab/vibeos/pull/95) (couche `1d-ter` du `Containerfile`, manifeste `os/saas-tools.txt` gardé par `check-saas-sync.py`). **Clients et outils passifs uniquement.** Runtimes de dev déjà présents en couche `1a` : `git`, `python3`/`pip`, `nodejs24`/`npm`.

| Outil | Paquet F44 | Licence | Rôle |
|---|---|---|---|
| client PostgreSQL (`psql`) | `postgresql` | PostgreSQL (BSD-like) | Parler à la base d'un projet (`psql -h localhost`) — **client, pas serveur** |
| SQLite | `sqlite` | Domaine public | Base zéro-démon pour prototypes / edge |
| `redis-cli` (compat) | `valkey-compat-redis` | BSD-3 | Client Redis/Valkey — **Valkey** est le fork BSD-3 ; Redis 8 est re-licencié tri-licence (RSALv2/SSPLv1/AGPLv3), Valkey évite les branches non-OSI et le copyleft AGPL |
| mkcert | `mkcert` | BSD-3 | CA locale → `https://` de dev valide, 100 % offline |
| podman-compose | `podman-compose` | GPL-2.0 | Orchestre les serveurs **par projet** (voir modèles compose) |
| uv | `uv` | Apache-2.0/MIT | Gestionnaire Python ultra-rapide (installe/isole sans toucher `/usr`) |
| ruff | `ruff` | MIT | Linter + formateur Python |
| mypy | `python3-mypy` | MIT | Typage statique Python |
| ApacheBench (`ab`) | `httpd-tools` | Apache-2.0 | Test de charge HTTP rapide livré nativement |
| perf | `perf` | GPL-2.0 | Profiling CPU noyau/appli |
| sysstat | `sysstat` | GPL-2.0 | `sar`/`iostat`/`pidstat` — métriques système dans le temps |
| bpftrace | `bpftrace` | Apache-2.0 | Traçage eBPF haut niveau (latences, syscalls) |
| bcc | `bcc-tools` | Apache-2.0 | Boîte à outils eBPF (profilage fin) |
| gh | `gh` | MIT | CLI GitHub (forge, releases, CI) |

**Modèles `compose` livrés** ([PR #97](https://github.com/Micka420-collab/vibeos/pull/97), sous `/usr/share/vibeos/saas/`, via `COPY os/rootfs/ /`) — le socle serveur **par projet**, jamais gravé :

| Modèle | Donne | Notes de sécurité |
|---|---|---|
| `postgres-valkey/` | PostgreSQL 18 + Valkey | Ports **loopback-only**, mot de passe **exigé** via `.env`, healthchecks, volumes nommés |
| `reverse-proxy/` | Caddy 2 + TLS local (mkcert) | `https://` de dev offline ; un seul reverse-proxy (non-redondance) |
| `observability/` | Prometheus (Apache-2.0) + Grafana (AGPL) | Analyse de perf du SaaS ; datasource auto-provisionné, ports loopback-only, mdp Grafana exigé via `.env` |
| `object-storage/` | SeaweedFS (Apache-2.0), S3-compatible | Uploads/images ecommerce ; MinIO écarté (AGPL+archivé) ; port S3 loopback-only, clés via `.env` |
| `mailpit/` | Mailpit (MIT), catcher SMTP de dev | Teste les emails sans les envoyer (UI web) ; ports loopback-only |
| `meilisearch/` | Meilisearch (MIT), recherche | Recherche produit ecommerce ; master key **exigée** en prod (`.env`), port loopback-only |

### Seau B — À la demande (`offer_optional`) — jamais dans l'image

> Trop lourds (Node/Python), ou **exigent le réseau** (donc un tier de policy explicite), ou évoluent trop vite pour être figés dans une base immuable. Installés dans `$HOME` (`npm i -g` dans un préfixe utilisateur, `uv tool`, binaire épinglé).

| Outil | Licence | Vecteur | Rôle | Gouvernance |
|---|---|---|---|---|
| flyctl | Apache-2.0 | binaire épinglé (arm64 ✅) | Déployer sur Fly.io | **Déploiement = T2/T3** (allowlist + approbation, à venir) |
| railway | MIT | binaire épinglé (arm64 ✅) | Déployer sur Railway | idem |
| vercel CLI | Apache-2.0 | npm | Déployer sur Vercel | idem |
| wrangler | MIT/Apache-2.0 | npm | Cloudflare Workers/Pages | idem |
| netlify CLI | MIT | npm | Déployer sur Netlify | idem |
| oha | MIT | binaire épinglé (arm64 ✅) | Test de charge HTTP moderne (TUI, HTTP/2) | T1 (local) |
| vegeta | MIT | binaire épinglé (arm64 ✅) | Test de charge à débit constant + rapports | T1 (local) |
| aws / gcloud / az | Apache-2.0 / propriétaire | rpm/installeur | CLIs cloud lourds | réseau → tier explicite |
| Stripe CLI | Apache-2.0 | binaire | Tester des webhooks paiement | **exige le réseau + clés** → gated |

### Seau C — Référence seulement (`self_host_reference`) — stacks conteneurs

> Des **stacks conteneurs à état**, jamais dans une image immuable. L'IA citoyenne les **instancie par projet** (podman/compose) en suivant la doc amont — VibeOS documente le choix et le piège, ne grave rien.

| Brique | Licence | Rôle | Piège / raison référence |
|---|---|---|---|
| Supabase | Apache-2.0 (cœur) | Backend BaaS (Postgres + Auth + Storage + Realtime) | Stack multi-conteneurs → par projet, jamais gravé |
| Medusa | MIT | Backend **ecommerce** headless (Node) | `npx create-medusa-app` + Postgres (modèle compose) |
| Umami | MIT | Analytics web respectueux (self-host) | Stack conteneur → par projet |
| Grafana + Prometheus | AGPL-3.0 / Apache-2.0 | Observabilité SaaS (dashboards + métriques) | Démons réseau → conteneurs par projet |

### 🔴 Pièges de licence SaaS — écartés, avec raison

> Le cœur de la doctrine : ne **jamais** graver un composant dont la licence interdit la redistribution, ou qui est mort. Ces choix protègent Micka juridiquement.

| Écarté | Licence / état | Remplacé par |
|---|---|---|
| Redis | Tri-licence RSALv2/SSPLv1/AGPLv3 depuis Redis 8 (2025) : 2 branches non-OSI, la 3ᵉ (AGPL) est un copyleft réseau lourd pour une image | **Valkey** (BSD-3, fork Linux Foundation, compatible protocole) : zéro friction |
| MinIO | AGPL-3.0 **+ dépôt archivé/EOL** | Garage / SeaweedFS (self-host), ou stockage S3-compatible du cloud |
| n8n | Sustainable Use (non-OSI) | Windmill (roadmap) — déjà listé dans les Rejetés |
| Directus | MSCL (source-available, non-OSI) | Referencé seulement si l'utilisateur l'assume ; pas gravé |
| Sentry | FSL (Functional Source, clause « competing use ») | GlitchTip (self-host) ou tier réseau opt-in |
| WebPageTest (agent) | Polyform Shield (non-compete) | Lighthouse CI (Apache-2.0) |

**Note de portée.** Le **déploiement en production** (`deploy.*` dans `vibed`) est une **capacité gouvernée à concevoir**, pas un simple binaire : elle attend (a) l'allowlist de cibles de Micka, (b) le patron helper-process d'[ADR-019](DECISIONS.md), (c) l'isolation des credentials cloud. Le catalogue liste les *outils* ; la *capacité* reste T2/T3 derrière approbation humaine sur le contenu déployé.

## Intégration — plan d'action

1. **Phase 1 (image)** : ✅ fait — VSCodium livré (paquet `codium`) ; couche « terminal vibecoding » (Ghostty, fish, Starship, Zellij, yazi, lazygit, atuin, zoxide, bat, eza, btop, mise, opencode, nvim) + `/etc/skel` (preset VibeVim, layouts Zellij, config fish/starship) livrées. **Exception : age/SOPS et sqlite-vec ne sont pas intégrés à l'image** — recalés en cible ultérieure (voir niveau 1-bis).
2. **Phase 2 (vibed+MCP)** : registre MCP par défaut (filesystem/git/fetch/memory + Serena) **enveloppé dans les policy tiers** ; systemd-creds pour les clés. Le **navigateur** est sorti de ce registre : [ADR-017](DECISIONS.md) le livre en outils `browser.*` **dans `vibed`**. Le choix du moteur et du protocole de pilotage fait l'objet d'une ADR dédiée, encore en revue. Dans tous les cas, **aucune exécution avant le bac à sable par outil** (**Phase 3**) : un moteur de rendu qui parse du HTML hostile ne peut pas vivre in-process dans le moteur de politiques.
3. **Bureau (chantier dédié)** : thème « VibeOS Dark » (fork Catppuccin), Panel Colorizer presets, HUD Quickshell (statut agents/tiers/ollama), sur le blueprint uBlue.
4. **Phase 5 (branding)** : Plymouth original, SDDM (base Astronaut), session focus optionnelle.
