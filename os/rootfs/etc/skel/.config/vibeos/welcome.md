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
- **Le démon `vibed` tourne** : embarqué dans l'image, il démarre au boot
  (politique tiers T0–T3 fail-closed, journal d'audit, serveur MCP sur
  `/run/vibed/mcp.sock`). Les administrateurs (groupe `wheel`) sont enrôlés
  automatiquement dans `vibeos-agents` — l'accès au socket est effectif à la
  connexion suivante. Le journal d'audit est réservé à root, par conception :
  `sudo tail -f /var/lib/vibeos/audit/vibed.jsonl`.
- **OS immuable** : `npm i -g` installe dans `~/.npm-global`, `mise` gère
  vos toolchains dans `$HOME`, `distrobox` couvre le reste. Rien ne touche
  jamais `/usr`.

## Documentation

- `~/README.md` — contenu de ces dotfiles et comment les réinitialiser
- Dépôt du projet (dossier `docs/`) : architecture, écosystème, mémoire,
  sécurité — <https://github.com/micka420-collab/vibeos>

*Pour revoir ce message : `rm ~/.local/state/vibeos/welcome-shown`.*
