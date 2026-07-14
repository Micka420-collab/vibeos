# agent/ — runtime agent de VibeOS

Ce répertoire regroupe ce qui concerne la **couche agent** de VibeOS : la façon dont les agents IA (Claude Code en tête) se connectent au démon système `vibed`, les CLIs IA embarquées dans l'image, et le prototype d'interview « Genesis » (cible Phase 3).

Principe : les agents ne touchent jamais le système directement. Toute action passe par le serveur MCP de `vibed` sur `/run/vibed/mcp.sock`, donc par le moteur de politique (tiers T0–T3, la première règle qui matche gagne) et le journal d'audit. Voir `vibed/README.md` pour le détail du protocole et des outils.

## Connexion au démon vibed

`vibed` parle JSON-RPC 2.0 délimité par lignes sur un socket unix. Claude Code (et la plupart des clients MCP) parlent le transport **stdio** : on fait le pont avec `socat`, présent dans l'image.

**Livré dans l'image (Phase 2)** : la configuration est **pré-déclarée pour Claude Code** via `/etc/skel/.claude.json` (source : `os/rootfs/etc/skel/.claude.json`) — tout nouvel utilisateur a le serveur MCP `vibeos` en portée utilisateur, **sans configuration manuelle**. Seul prérequis : appartenir au groupe `vibeos-agents` (voir plus bas). Contenu livré :

```json
{
  "mcpServers": {
    "vibeos": {
      "command": "socat",
      "args": ["STDIO", "UNIX-CONNECT:/run/vibed/mcp.sock"]
    }
  }
}
```

Le même extrait fonctionne dans un `.mcp.json` à la racine d'un projet (portée projet), ou pour tout autre client MCP stdio (gemini-cli : `~/.gemini/settings.json`, opencode : voir sa doc). Claude Code lance `socat`, qui relaie chaque ligne JSON entre stdio et le socket. Les outils `os.status`, `fs.read`, `fs.write`, `pkg.install`, `svc.restart`, `memory.query`, `memory.append` apparaissent alors comme des outils MCP `vibeos` côté agent.

Points de comportement à connaître :

- **T2+ = approbation humaine, toujours.** `pkg.install` et `svc.restart` répondent `requires_approval` : l'agent reçoit un résultat `isError` explicite, et la demande est tracée dans `/var/lib/vibeos/audit/vibed.jsonl` (avec l'identité de l'appelant : uid/gid/pid). Le workflow d'approbation (`vibectl`) arrive dans une étape ultérieure (voir `ROADMAP.md`).
- **Accès au socket.** Le socket est en `root:vibeos-agents`, mode `0660` : l'utilisateur qui lance l'agent doit appartenir au groupe **`vibeos-agents`**, créé dans l'image par `os/rootfs/usr/lib/sysusers.d/vibeos.conf` (`vibed` applique le groupe au socket à son démarrage). C'est la première barrière, la politique est la seconde.
- **Écritures fichiers (T1).** `fs.write` est limité en v0.1 à `/home/**` et `/var/home/**` (sous Fedora, `/home` est un lien vers `/var/home`). La mémoire VibeOS n'est **pas** inscriptible via `fs.write` (deny codé en dur) : les écritures mémoire passent par **`memory.append`** (T1) — strictement additif, scopes `journal` et `knowledge` (les scopes `user`/`projects` restent Phase 2/3, voir `docs/MEMORY.md` §9).
- **Mémoire.** `memory.query` lit `/var/lib/vibeos/memory`, créée au premier boot par `vibeos-genesis.service` (en clair en v0.1 ; LUKS = Phase 3). Le **mode amnésique** (cible Phase 3) reconstruira cette mémoire en tmpfs à chaque démarrage : les agents doivent tolérer une mémoire vide.

Test manuel rapide, sans client MCP :

```bash
printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' \
  | socat - UNIX-CONNECT:/run/vibed/mcp.sock
```

## CLIs IA embarquées

L'image VibeOS livre un runtime hybride : cloud quand c'est possible, local quand c'est nécessaire (hors-ligne). Toutes les CLIs sont épinglées en versions exactes dans le `Containerfile`.

| CLI | Rôle | Mode |
|---|---|---|
| `claude` | Claude Code + Claude Agent SDK : agent principal de vibecoding, client MCP de `vibed` | Cloud (Anthropic) |
| `gemini` | CLI Gemini : second avis, tâches multimodales | Cloud (Google) |
| `codex` | CLI Codex : alternative OpenAI | Cloud (OpenAI) |
| `opencode` | Agent terminal multi-fournisseur (`opencode-ai`, projet sst/opencode, MIT), peut cibler ollama — 100 % local possible | Cloud ou local |
| `ollama` | Serveur de modèles locaux (ex. modèles de code quantisés) : capacité hors-ligne totale | Local |

En mode hors-ligne, `opencode` pointé sur `ollama` reste la voie de vibecoding fonctionnelle sans aucune clé API.

`aider` n'est **pas** préinstallé : il exige Python < 3.13, incompatible avec le Python 3.13 de la base Fedora Kinoite 42. Il reste installable à la demande, sans toucher l'OS immuable : `uvx --python 3.12 aider-chat` (éphémère) ou `uv tool install --python 3.12 aider-chat` (persistant, `~/.local`).

## Clés API et secrets

| Variable | Consommateur | Remarque |
|---|---|---|
| `ANTHROPIC_API_KEY` | `claude`, `genesis_interview.py --with-claude` | |
| `GEMINI_API_KEY` | `gemini` | |
| `OPENAI_API_KEY` | `codex`, `opencode` (mode OpenAI) | |
| `OLLAMA_HOST` | `opencode`, clients ollama | Local, aucune clé — défaut `http://127.0.0.1:11434` |

**Où vivent ces clés** — le mécanisme de référence est celui de [docs/SECURITY-ARCHITECTURE.md](../docs/SECURITY-ARCHITECTURE.md), §4, qui fait foi :

- via **`systemd-creds`** : credentials chiffrés (scellés TPM2 quand le matériel le permet), exposés aux services uniquement sous `/run/credentials/` ;
- ou via le **kernel keyring** pour les usages de session.

Interdits absolus :

- **jamais dans l'image OS** (elle est immuable, publique et signée) ;
- **jamais dans un dotfile en clair** ;
- **jamais dans `environment.d`** ni aucune variable d'environnement persistée : l'environnement d'un processus fuit via `/proc/<pid>/environ` ;
- **jamais sur le volume mémoire de VibeOS** (`/var/lib/vibeos/memory`) : la mémoire n'est pas un coffre à secrets.

Les variables du tableau ci-dessus sont peuplées au lancement des CLIs depuis ces credentials — elles ne sont jamais stockées telles quelles.

## Interview Genesis (`genesis_interview.py`) — prototype Phase 3

Prototype Python (stdlib uniquement) de l'interview du premier démarrage. **Il n'est pas câblé dans `genesis.sh` en v0.1** : rien ne l'invoque au premier boot, il se teste à la main. Son intégration à la séquence Genesis est un livrable de la **Phase 3** (voir `ROADMAP.md`).

Il construit le **profil utilisateur** de la mémoire machine (identité de l'humain, langues, style de code, projets en cours) — et uniquement cela :

- Écrit : `user/profile.toml`, `user/preferences.toml`, `projects/index.json`, plus **une** ligne JSONL ajoutée à `journal/<AAAA-MM-JJ>.jsonl`.
- Ne touche **jamais** : `identity.toml` ni `hardware.json` (identité machine, écrite une seule fois par `memory/genesis.sh` — voir [docs/MEMORY.md](../docs/MEMORY.md)).

```bash
# test local sans droits root :
python3 agent/genesis_interview.py /tmp/vibeos-memory-test

# enrichissement optionnel des questions via l'API Anthropic
# (nécessite le paquet 'anthropic' et ANTHROPIC_API_KEY) :
python3 agent/genesis_interview.py /tmp/vibeos-memory-test --with-claude

# cible Phase 3 : invocation par la séquence Genesis
# python3 agent/genesis_interview.py /var/lib/vibeos/memory
```

Propriétés :

- **Hors-ligne par défaut** (`--offline` est le défaut ; `--with-claude` est le seul chemin réseau, avec repli propre si le paquet `anthropic` est absent ou la clé manquante).
- **Idempotent** : relançable sans risque ; les réponses précédentes sont relues depuis les fichiers produits eux-mêmes (`user/*.toml`, `projects/index.json`) et proposées comme défauts.
- **Écritures atomiques** (fichier temporaire + `os.replace`) pour les fichiers de profil ; le journal est en append-only.

## Références

- `vibed/README.md` — protocole MCP, outils, politique, audit
- `docs/SECURITY-ARCHITECTURE.md` — secrets (§4), durcissement, modèle de menace
- `docs/MEMORY.md` — layout mémoire de référence (fait foi)
- `memory/genesis.sh` — création de la mémoire au premier boot (en clair en v0.1 ; LUKS et tmpfs amnésique = Phase 3)
- `ROADMAP.md` — vibectl, workflow d'approbation, interview de naissance (Phase 3)
