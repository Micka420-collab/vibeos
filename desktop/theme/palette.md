# Palette « VibeOS Dark »

> Fork de **Catppuccin Mocha** ([catppuccin/kde](https://github.com/catppuccin/kde), licence MIT — redistribuable dans l'ISO, voir [docs/ECOSYSTEM.md](../../docs/ECOSYSTEM.md)).
> Les valeurs hexadécimales sont reprises **verbatim** de Mocha : le fork porte sur le nommage, la sémantique VibeOS (tiers de policy, états d'agents) et les fichiers d'intégration — pas sur les couleurs elles-mêmes. Toute dérive future de la palette devra être documentée ici.
> Fichier machine associé : [`vibeos-dark.colors`](vibeos-dark.colors) (schéma de couleurs Plasma 6).

---

## 1. Palette de référence

### Fonds (du plus profond au plus clair)

| Nom | Hex | RGB | Usage VibeOS |
|---|---|---|---|
| Crust | `#11111b` | 17,17,27 | Fond des OSD, écran de verrouillage, tooltips alternés, panneaux HUD « enfoncés » |
| Mantle | `#181825` | 24,24,37 | **Chrome** : barres de titre, panneau Plasma, en-têtes, fond du HUD Quickshell, fond de fenêtre |
| Base | `#1e1e2e` | 30,30,46 | **Contenu** : zones de vue (éditeur, listes, terminal Ghostty), fond principal |
| Surface0 | `#313244` | 49,50,68 | Boutons, champs de saisie, cartes du HUD |
| Surface1 | `#45475a` | 69,71,90 | Boutons alternés, séparateurs, éléments survolés |
| Surface2 | `#585b70` | 88,91,112 | Bordures actives, scrollbars |

### Textes

| Nom | Hex | RGB | Usage VibeOS |
|---|---|---|---|
| Text | `#cdd6f4` | 205,214,244 | Texte principal (contraste ≈ 11,9:1 sur Base — AAA) |
| Subtext1 | `#bac2de` | 186,194,222 | Texte secondaire, labels |
| Subtext0 | `#a6adc8` | 166,173,200 | Texte tertiaire, en-têtes inactifs |
| Overlay2 | `#9399b2` | 147,153,178 | Placeholders, texte d'aide |
| Overlay1 | `#7f849c` | 127,132,156 | Texte inactif/désactivé, commentaires de code |
| Overlay0 | `#6c7086` | 108,112,134 | Éléments décoratifs discrets, ponctuation de prompt |

### Accents

| Nom | Hex | RGB | Usage VibeOS |
|---|---|---|---|
| **Mauve** | `#cba6f7` | 203,166,247 | **Accent signature VibeOS** : sélection, focus, agent actif, logo, prompt Starship |
| Blue | `#89b4fa` | 137,180,250 | Liens, information, **tier T0** |
| Lavender | `#b4befe` | 180,190,254 | Liens visités, focus clavier secondaire |
| Green | `#a6e3a1` | 166,227,161 | Succès, diff « + », **tier T1** |
| Teal | `#94e2d5` | 148,226,213 | Chaînes de caractères (code), badges « offline OK » |
| Sky | `#89dceb` | 137,220,235 | Jauges ollama (VRAM/tokens) dans le HUD |
| Sapphire | `#74c7ec` | 116,199,236 | Éléments réseau (états de connexion) |
| Yellow | `#f9e2af` | 249,226,175 | Avertissements légers, états « en attente » |
| Peach | `#fab387` | 250,179,135 | Attention requise, **tier T2** (approbation humaine) |
| Maroon | `#eba0ac` | 235,160,172 | Erreurs secondaires, tests en échec |
| Red | `#f38ba8` | 243,139,168 | Erreurs, diff « − », **tier T3** (destructif) |
| Pink | `#f5c2e7` | 245,194,231 | Décoratif (rare), branding Phase 5 |
| Flamingo | `#f2cdcd` | 242,205,205 | Décoratif (rare) |
| Rosewater | `#f5e0dc` | 245,224,220 | Curseur terminal, décoratif |

---

## 2. Sémantique VibeOS : accent et tiers de policy

Le triptyque du bureau (Agent / Contexte / Confiance, voir [docs/DESKTOP.md](../../docs/DESKTOP.md)) s'appuie sur un code couleur **stable dans tout l'OS** — HUD, notifications, terminal, prompt, thème d'éditeur :

| Sémantique | Couleur | Hex | Règle d'usage |
|---|---|---|---|
| Identité / agent actif / accent global | Mauve | `#cba6f7` | La seule couleur « de marque ». Sélection, focus, agent en cours d'exécution. |
| **T0 — observe** (lecture seule) | Blue | `#89b4fa` | Informationnel : l'agent regarde, rien n'est modifié. |
| **T1 — modify-user** (fichiers utilisateur) | Green | `#a6e3a1` | Autorisé et journalisé : action bénigne, visible, réversible. |
| **T2 — modify-system** (paquets, services) | Peach | `#fab387` | **Approbation humaine requise** : tout élément UI T2 doit attirer l'œil sans alarmer. |
| **T3 — destructive** (disque, credentials) | Red | `#f38ba8` | **Approbation renforcée** : réservé à T3, aux erreurs et aux suppressions. Jamais décoratif. |
| État indéterminé / en attente | Yellow | `#f9e2af` | Demande en cours, agent en pause. |
| Daemon hors ligne (`vibed` absent — Phase 1) | Overlay1 | `#7f849c` | Le HUD passe en gris : état « offline » propre, jamais un crash. |

Règles dures :

1. **Le rouge n'est jamais décoratif.** S'il y a du rouge à l'écran, c'est T3, une erreur ou une suppression.
2. **Le mauve n'est jamais sémantique de danger.** C'est l'identité et le focus, rien d'autre.
3. **Un tier = une couleur, partout.** Le badge T2 du HUD, la ligne d'audit dans le terminal et la future boîte de dialogue d'approbation Plasma (Phase 2) utilisent le même Peach.
4. Contraste minimal : texte sur fond ≥ 4,5:1 (Text sur Base = ~11,9:1 ; Base sur Mauve = ~7,4:1 pour les sélections).

---

## 3. Propagation de la palette dans l'OS

La palette n'est **définie qu'une fois** (ce fichier + `vibeos-dark.colors`) et se propage par des fichiers de configuration livrés dans l'image (`/usr/share`) ou dans les défauts utilisateur (`/etc/skel`) — jamais écrits dans `/usr` à l'exécution (OS immuable).

| Cible | Mécanisme | Fichier livré (cible image) | Statut |
|---|---|---|---|
| **Plasma 6** (widgets, fenêtres, notifications) | Schéma de couleurs KColorScheme | `/usr/share/color-schemes/VibeOSDark.colors` (source : [`vibeos-dark.colors`](vibeos-dark.colors)), activé par le Global Theme « VibeOS Dark » | v0.1 |
| **Panneau Plasma** | Preset Panel Colorizer (fond Mantle, accent Mauve) | preset JSON sous `desktop/` + application via `/etc/skel` | v0.1 |
| **Terminal Ghostty** | Couleurs **inline** dans la config Ghostty (bg/fg + 16 couleurs ANSI mappés sur la palette : bg=Base, fg=Text, cursor=Rosewater, ANSI red=Red, green=Green, blue=Blue…) | `/etc/skel/.config/ghostty/config` (pas de fichier de thème séparé) | v0.1 |
| **Prompt Starship / fish** | Variables de palette dans `starship.toml` (accent Mauve, erreurs Red, git Green/Peach) | `/etc/skel/.config/starship.toml` | v0.1 |
| **Zellij** | Thème KDL **inline** dans la config Zellij (bloc `themes { vibeos-dark { … } }` + `theme "vibeos-dark"`) | `/etc/skel/.config/zellij/config.kdl` (pas de fichier de thème séparé) | v0.1 |
| **Neovim (preset VibeVim)** | `catppuccin/nvim` (flavour mocha) avec overrides « vibeos » (accent mauve) | `/etc/skel/.config/nvim/` (chantier VibeVim) | v0.1 |
| **VSCodium** | Extension Catppuccin depuis Open VSX (thème « Catppuccin Mocha ») recommandée par défaut | `/etc/skel/.config/VSCodium/` (settings par défaut) | v0.1 |
| **HUD Quickshell** | Singleton QML `Palette.qml` exposant les constantes (base, mantle, accent, tier0…tier3) — source de vérité unique du HUD | `desktop/quickshell/` (voir ce dossier) | v0.1 (rendu) / Phase 2 (données live) |
| **bat / eza / btop / yazi** | Thèmes Catppuccin Mocha officiels (MIT) livrés tels quels | `/etc/skel/.config/{bat,btop,yazi}/` | v0.1 |
| **SDDM / Plymouth** | Déclinaison de la palette (fond Crust, accent Mauve) | thèmes dédiés — chantier branding | Phase 5 |

> **Note immutabilité** : les chemins `/usr/share/...` ci-dessus sont posés **au build de l'image** (référencés depuis `os/Containerfile`, qui appartient à un autre chantier). Ce dossier `desktop/theme/` est la **source** copiée dans l'image ; à l'exécution, seuls `/etc` et `$HOME` bougent.

---

## 4. Ce que le fork « VibeOS Dark » change par rapport à Catppuccin Mocha

| Aspect | Catppuccin Mocha | VibeOS Dark |
|---|---|---|
| Valeurs hex | référence | **identiques** (v0.1) |
| Accent | au choix (26 déclinaisons) | **figé sur Mauve** `#cba6f7` |
| Sémantique | générique (success/warning/error) | **mappée sur les tiers T0–T3** de `vibed` (§2) |
| Fenêtre vs contenu | variable selon les ports | convention fixe : chrome=Mantle, contenu=Base, creux=Crust |
| Nom du schéma Plasma | `Catppuccin Mocha <Accent>` | `VibeOS Dark` (`ColorScheme=VibeOSDark`) |
| Licence | MIT | MIT (attribution Catppuccin conservée dans chaque fichier dérivé) |
