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

- Le pane « vibed audit » du layout `vibe` affiche **« vibed hors ligne »**
  tant que le démon `vibed` n'écrit pas son journal
  (`/var/lib/vibeos/audit/vibed.jsonl`) — c'est normal, il arrive en Phase 2.
- Le **premier lancement de `nvim`** télécharge les plugins du preset VibeVim :
  il faut le réseau **une fois**. Ensuite, tout fonctionne hors ligne
  (modèles locaux via ollama).
