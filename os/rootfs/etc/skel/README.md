# Dotfiles VibeOS (`/etc/skel`)

Ces fichiers sont copiés automatiquement dans le dossier personnel de chaque
**nouvel** utilisateur, à sa création. Ils configurent la pile terminal
« vibecoding » de VibeOS, prête à l'emploi dès la première session.

| Fichier | Rôle |
|---|---|
| `.config/fish/config.fish` | Shell fish : starship, zoxide, atuin, mise, alias (`ls`→eza, `cat`→bat), message de bienvenue |
| `.config/starship.toml` | Prompt Starship, thème VibeOS Dark (git, langages via mise, durée) |
| `.config/ghostty/config` | Terminal Ghostty : couleurs VibeOS Dark, police, splits |
| `.config/zellij/config.kdl` | Multiplexeur Zellij : thème et options par défaut |
| `.config/zellij/layouts/vibe.kdl` | Layout signature « agent + lazygit + logs » (commande `vibe`) |
| `.config/nvim/` | Preset Neovim « VibeVim » : lazy.nvim + LazyVim + codecompanion (ollama + ACP) |
| `.config/vibeos/welcome.md` | Mot de bienvenue affiché au tout premier terminal |
| `.config/autostart/vibeos-hud.desktop` | Autostart du **HUD agents** (Quickshell) en session Plasma — lance `/usr/bin/vibeos-hud` ; supprimez ce fichier de votre `$HOME` pour désactiver le HUD |
| `.claude.json` | Config MCP de **Claude Code** : serveur `vibeos` pré-déclaré (pont `socat` → socket `/run/vibed/mcp.sock`) — voir `agent/README.md` |
| `.config/zed/settings.json` | Config **Zed** (couche 0 de « VibeOS pour Zed », ADR-014) : agent ACP `claude-code-acp` + serveur MCP `vibed` (`context_servers`). Scaffolding à valider contre la version Zed packagée ; le fork gouverné (Read/Write/Edit → vibed, mode auto) est couche 1–2 |

## Réinitialiser / régénérer ses dotfiles

L'OS est immuable : `/etc/skel` est fourni par l'image et mis à jour avec
elle, mais il n'écrase **jamais** un dossier personnel existant. Pour
récupérer la version d'origine :

```sh
# un fichier précis (exemple : la config fish)
cp /etc/skel/.config/fish/config.fish ~/.config/fish/config.fish

# tout reprendre (vos versions actuelles sont sauvegardées en *.bak)
cp -r --backup=simple --suffix=.bak /etc/skel/. ~/
```

Pour revoir le message de bienvenue : `rm ~/.local/state/vibeos/welcome-shown`.

## Notes v0.1 (honnêteté)

- L'accès au socket MCP de `vibed` (Claude Code via `.claude.json`, HUD)
  exige d'appartenir au groupe **`vibeos-agents`**. Les **administrateurs
  (`wheel`) y sont enrôlés automatiquement** à chaque boot par
  `vibeos-agents-group.service` (ils ont déjà `sudo`). Un compte **non-`wheel`**
  reste opt-in : `sudo usermod -aG vibeos-agents <user>` puis rouvrir la
  session. Tant que l'utilisateur n'y est pas, le serveur MCP `vibeos` apparaît
  « hors ligne » (voir `agent/README.md`).
- Le **HUD Quickshell** affiche en v0.1 des données de démonstration et une
  pastille « vibed hors ligne » : le branchement live du QML sur le socket
  est le reste du chantier Phase 2 (`desktop/quickshell/README.md` §4).
- Le pane « vibed audit » du layout `vibe` se remplit dès que des appels
  d'outils MCP sont audités dans `/var/lib/vibeos/audit/ (par jour)`
  (le démon `vibed` démarre au boot depuis la v0.1).
- Le **premier lancement de `nvim`** télécharge les plugins du preset VibeVim :
  il faut le réseau **une fois**. Ensuite, tout fonctionne hors ligne
  (modèles locaux via ollama).
