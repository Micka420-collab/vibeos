# 🌱 Bienvenue sur VibeOS

Cette machine est née **vierge** : aucune mémoire d'usine, aucun compte
fantôme. Sa mémoire a été créée au premier démarrage (séquence *Genesis*)
dans `/var/lib/vibeos/memory` — elle vous appartient, et à personne d'autre.

## Raccourcis vibecoding

| Commande | Effet |
|---|---|
| `vibe` | Session signature Zellij : agent + lazygit + journal vibed |
| `lg` | lazygit — relire et committer ce que les agents produisent |
| `y` | yazi — explorer les fichiers créés par les agents |
| `cd <fragment>` | zoxide — saute vers vos projets fréquents |
| `Ctrl+R` | atuin — recherche dans l'historique (piste d'audit locale) |
| `nvim` | Preset « VibeVim » — IA locale (ollama) + agents ACP |
| `btop` | Monitoring CPU/GPU pendant l'inférence |

Agents en ligne de commande : `claude`, `opencode`, `gemini`,
`codex` — et `ollama` pour les modèles 100 % locaux, utilisables hors ligne.
(`aider` optionnel : `uvx --python 3.12 aider-chat`.)

## Bon à savoir

- **Premier `nvim`** : les plugins s'installent automatiquement — réseau
  requis **une seule fois**, tout fonctionne hors ligne ensuite.
- **v0.1, en toute honnêteté** : le démon `vibed` (policy tiers T0–T3,
  journal d'audit) arrive en **Phase 2**. Le pane « vibed audit » du layout
  `vibe` affiche « hors ligne » d'ici là — c'est prévu, pas un bug.
- **OS immuable** : `npm i -g` installe dans `~/.npm-global`, `mise` gère
  vos toolchains dans `$HOME`, `distrobox` couvre le reste. Rien ne touche
  jamais `/usr`.

## Documentation

- `~/README.md` — contenu de ces dotfiles et comment les réinitialiser
- Dépôt du projet (dossier `docs/`) : architecture, écosystème, mémoire,
  sécurité — <https://github.com/micka420-collab/vibeos>

*Pour revoir ce message : `rm ~/.local/state/vibeos/welcome-shown`.*
