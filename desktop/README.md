# desktop/ — l'expérience bureau VibeOS

Ce dossier contient les **sources du bureau VibeOS** : thème, palette, et les briques d'habillage de KDE Plasma 6. La spécification complète de l'expérience (« pourquoi » et « comment ») est dans [docs/DESKTOP.md](../docs/DESKTOP.md) — ce README n'est que la carte du dossier.

> **Statut** : chantier « Bureau » (voir [docs/ECOSYSTEM.md](../docs/ECOSYSTEM.md), plan d'action). Le **Global Theme est le défaut système** et le **HUD est installé et auto-démarré** (runtime Quickshell compilé dans l'image) ; la liaison vivante HUD ↔ `vibed` (données live) reste un livrable **Phase 2** ; les thèmes **SDDM** et **Plymouth** sont copiés dans l'image (sélectionnables, **non actifs par défaut**) — leur activation par défaut et le logo restent **Phase 5**.

---

## La philosophie UX en trois lignes

Le bureau VibeOS est organisé autour du **triptyque Agent / Contexte / Confiance** :

1. **Agent** — qui travaille pour moi, sur quoi ? → HUD Quickshell, layouts Zellij « agent + lazygit + logs ».
2. **Contexte** — sur quoi travaillons-nous ? → activités KDE **Vibe / Focus / Review**, mémoire de la machine (via MCP, Phase 2).
3. **Confiance** — qu'ai-je autorisé, qu'a-t-on fait en mon nom ? → code couleur des tiers T0–T3, futurs dialogues d'approbation (Phase 2), journal d'audit.

Trois règles d'exécution : **le terminal (Ghostty) est la scène principale** et le bureau s'efface devant lui ; **Plasma 6 n'est pas remplacé** (on l'habille : Global Theme, Panel Colorizer, HUD Quickshell en couche additionnelle Qt6/QML) ; **tout se dégrade gracieusement** — si `vibed` est arrêté ou indisponible, le HUD affiche un état « daemon hors ligne » propre, jamais un crash.

---

## Contenu du dossier

| Chemin | Contenu | Statut |
|---|---|---|
| [`theme/vibeos-dark.colors`](theme/vibeos-dark.colors) | Schéma de couleurs Plasma 6 **« VibeOS Dark »** — fork de Catppuccin Mocha (MIT), accent Mauve `#cba6f7`. Cible image : `/usr/share/color-schemes/VibeOSDark.colors` | v0.1 |
| [`theme/palette.md`](theme/palette.md) | La palette de référence : table nom/hex/usage, sémantique des tiers T0–T3, propagation vers Ghostty, éditeurs, HUD, prompt | v0.1 (doc) |
| `quickshell/` | Le **HUD agents** (Qt6/QML via Quickshell, LGPL-3.0) : pastille + panneaux Agents / Confiance / Ressources / Mémoire | livré : rendu + autostart + état « vibed hors ligne » · **Phase 2** : données vives via `/run/vibed/mcp.sock` |
| [`look-and-feel/`](look-and-feel/README.md) | Global Theme **`org.vibeos.dark`** (KPackage : `metadata.json`, `defaults`, `layout.js`) — copié dans l'image et **défaut système** via `/etc/xdg/kdeglobals` | ✅ livré (Phase 2) |
| `kvantum/` | Thème de widgets **Kvantum `VibeOSDark`** (coins arrondis, verre sur le chrome flottant) — moteur installé, thème copié, sélection via `/etc/skel/.config/Kvantum/kvantum.kvconfig` | ✅ livré (Phase 2) — atlas SVG complet : Phase 5 |
| [`wallpapers/`](wallpapers/README.md) | Trois paquets wallpaper originaux : « VibeOS » (**défaut** via le Global Theme), « Genesis », « Void » — copiés sous `/usr/share/wallpapers/` | ✅ livré (Phase 2) |
| [`sddm/`](sddm/README.md) | Thème de connexion **original** VibeOS — copié dans l'image (`/usr/share/sddm/themes/vibeos/`), **non actif par défaut** (greeter Breeze) | copié · activation 🛣️ Phase 5 |
| [`plymouth/`](plymouth/README.md) | Thème de boot **original** (l'existant adi1090x est rejeté — provenance d'assets floue) — copié dans l'image (`/usr/share/plymouth/themes/vibeos/`), **non actif par défaut** ; 3 PNG à générer (dégradation gracieuse) | copié · activation 🛣️ Phase 5 |
| *(reste de ce chantier)* preset Panel Colorizer, défauts `/etc/skel` bureau (raccourcis, activités) | voir [docs/DESKTOP.md](../docs/DESKTOP.md) §2.6 et §9 | 🛣️ Phase 2 |

---

## Comment ces fichiers arrivent sur le système (OS immuable)

VibeOS est immuable (bootc/OSTree) : **rien n'écrit dans `/usr` à l'exécution**.

- Les sources de ce dossier sont copiées **au build de l'image** vers `/usr/share/…` (schéma de couleurs, Global Theme, wallpapers, QML du HUD). Les paquets requis (Quickshell, Panel Colorizer, Ghostty, polices JetBrains Mono/Fira Code) sont déclarés dans `os/Containerfile` — **chantier distinct**, ce dossier ne fait que référencer les noms.
- Les défauts par utilisateur (raccourcis `kglobalshortcutsrc`, activités, config Ghostty/Zellij, autostart du HUD) passent par **`/etc/skel`** : copiés dans chaque nouveau `$HOME`, puis propriété de l'utilisateur.
- L'utilisateur reste libre : tout est remappable/désactivable via les Réglages système Plasma standard.

## Licences

Thème **VibeOS Dark** : MIT, dérivé de [Catppuccin](https://github.com/catppuccin/kde) (MIT, attribution conservée dans chaque fichier dérivé). Quickshell : LGPL-3.0. Panel Colorizer : GPL-3.0 (utilisé tel quel). Créations originales du dossier (wallpapers, scripts, QML) : licence du dépôt (Apache-2.0). Tout est redistribuable dans une ISO — condition d'entrée de l'écosystème VibeOS.
