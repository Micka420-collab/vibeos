# Global Theme « VibeOS Dark » (Look-and-Feel Plasma 6) + Kvantum

> Statut : **🛣️ Phase 2** — l'ensemble du Global Theme (layout panneau + dock,
> `defaults`, style Kvantum) est **conçu maintenant** mais **câblé dans l'image
> en Phase 2** (voir [docs/DESKTOP.md](../../docs/DESKTOP.md) §2.6 et §9). En v0.1,
> seul le **schéma de couleurs** `VibeOSDark.colors` est réellement embarqué. Ce
> paquet applique le langage visuel défini dans
> [docs/DESIGN-SYSTEM.md](../../docs/DESIGN-SYSTEM.md) — accent Mauve `#cba6f7`,
> profondeur en couches, verre maîtrisé.

---

## 1. Rôle

Ce dossier est la **peau Plasma 6** de VibeOS. Il ne remplace pas Plasma, il
l'**habille** (DESKTOP §1) via deux briques complémentaires :

1. **Le paquet Look-and-Feel `org.vibeos.dark`** — un Global Theme KPackage qui,
   quand l'utilisateur le sélectionne, met en cohérence d'un seul geste :
   schéma de couleurs, style de widgets, thème Plasma, décoration de fenêtre,
   icônes, curseur, polices, **et le layout** (panneau + dock).
2. **Le thème Kvantum `VibeOSDark`** (dossier voisin
   [`desktop/kvantum/VibeOSDark/`](../kvantum/VibeOSDark/)) — le style de widgets
   qui apporte ce que Breeze ne sait pas faire ensemble : **coins arrondis** à la
   famille de rayons (§4.1), **flou réel** sur le chrome flottant (menus/tooltips
   = `glass-elevated`, §6.1) et un **contrôle fin de la palette** (§2).

Ce que ce chantier **ne** fait **pas** : le verre du panneau/dock (preset **Panel
Colorizer**, Phase 2), le **HUD Quickshell** (couche additionnelle,
[`desktop/quickshell/`](../quickshell/), Phase 2), les **wallpapers** et le
**branding** (SDDM/Plymouth/logo, Phase 5).

---

## 2. Contenu et cibles `/usr` (OS immuable)

Rien ne s'écrit dans `/usr` à l'exécution : ces sources sont **copiées au build**
de l'image (référencées depuis `os/Containerfile` — chantier distinct).

| Fichier source (ce dépôt) | Cible dans l'image | Rôle |
|---|---|---|
| `org.vibeos.dark/metadata.json` | `/usr/share/plasma/look-and-feel/org.vibeos.dark/metadata.json` | Identité KPackage du Global Theme (id, nom, auteur, version, licence) |
| `org.vibeos.dark/contents/defaults` | `…/org.vibeos.dark/contents/defaults` | **Câblage des composants** : `ColorScheme=VibeOSDark`, `widgetStyle=kvantum`, thème Plasma, décoration Breeze, icônes/curseur Breeze, polices (§3), toggles de flou compositeur |
| `org.vibeos.dark/contents/layouts/org.kde.plasma.desktop-layout.js` | `…/contents/layouts/org.kde.plasma.desktop-layout.js` | Script de layout : **panneau haut fin flottant** + **dock centré auto-masqué** |
| [`../kvantum/VibeOSDark/VibeOSDark.kvconfig`](../kvantum/VibeOSDark/VibeOSDark.kvconfig) | `/usr/share/Kvantum/VibeOSDark/VibeOSDark.kvconfig` | Géométrie, opacité, palette du style de widgets |
| [`../kvantum/VibeOSDark/VibeOSDark.svg`](../kvantum/VibeOSDark/VibeOSDark.svg) | `/usr/share/Kvantum/VibeOSDark/VibeOSDark.svg` | Atlas SVG 9-slice (**placeholder documenté** ; art complet = Phase 5) |
| [`../theme/vibeos-dark.colors`](../theme/vibeos-dark.colors) *(chantier « thème »)* | `/usr/share/color-schemes/VibeOSDark.colors` | **Source unique** de la palette Plasma — ne pas dupliquer |

> **Autorité du câblage.** Par le modèle KPackage de Plasma, la liste des
> composants vit dans `contents/defaults`, **pas** dans `metadata.json` (le JSON
> ne porte qu'une clé `X-VibeOS-Components` *documentaire*, ignorée par KDE). En
> cas de doute, `defaults` fait foi.

---

## 3. Polices (rappel d'honnêteté)

Le `defaults` fixe la pile du design system (§3.1) :

- **Mono = JetBrains Mono** — dans l'image v0.1 (`jetbrains-mono-fonts`). Toute
  donnée technique (`fixed=…`).
- **Sans = Inter** — **cible** pour les titres/UI. Inter **doit être packagé**
  (`google-inter-fonts`, Phase 2/5, `os/Containerfile`). Tant qu'il ne l'est pas,
  **fontconfig retombe sur Noto Sans** (embarqué, héritage Fedora) : l'UI ne
  casse jamais sans Inter. Le `defaults` écrit `font=Inter,…` — le repli est géré
  par fontconfig, pas par le fichier.

---

## 4. Comment l'activer

Prérequis (Phase 2) : le paquet `kvantum` et le paquet Quickshell déclarés dans
`os/Containerfile`, et les fichiers ci-dessus présents sous `/usr`.

### 4.1 Global Theme complet (recommandé)

```sh
# Applique schéma + style + layout + décoration + icônes + curseur + polices.
lookandfeeltool -a org.vibeos.dark

# Lister les thèmes disponibles pour vérifier l'id :
lookandfeeltool --list
```

Ou via l'interface : **Réglages du système ▸ Couleurs et thèmes ▸ Thème global ▸
VibeOS Dark ▸ Appliquer** (cocher « Utiliser aussi la disposition des bureaux »
pour appliquer `layout.js`).

### 4.2 Activer le style Kvantum (fait par le Global Theme)

Le `defaults` pose déjà `widgetStyle=kvantum`. Il reste à désigner **quel** thème
Kvantum est actif (config séparée, livrée via `/etc/skel` en Phase 2) :

```sh
# ~/.config/Kvantum/kvantum.kvconfig
[General]
theme=VibeOSDark
```

Outil graphique équivalent : `kvantummanager` ▸ *Select a theme* ▸ **VibeOSDark**.

### 4.3 Repli gracieux

Si Kvantum est absent, remplacer dans `defaults` `widgetStyle=kvantum` par
`widgetStyle=Breeze` : le **schéma de couleurs VibeOSDark** habille malgré tout
tout le bureau. Le verre est un **rehaussement**, jamais une dépendance de
lisibilité (design §6.2). De même, les utilisateurs `prefers-reduced-transparency`
retombent sur les surfaces opaques équivalentes.

---

## 5. Ce que fait le layout (`layout.js`)

- **Panneau haut** : fin (**30 px**, DESKTOP §2.2), **flottant** (dalle de verre
  une fois le preset Panel Colorizer posé), **permanent**, pleine largeur.
  Lanceur (Kickoff) · **pager d'activités** Vibe/Focus/Review · **horloge
  centrée 24 h** · zone système.
- **Dock bas** : **centré**, **auto-masqué** (la scène appartient au terminal,
  DESKTOP §1/§2.3), **flottant**, ajusté à ses icônes. Icons-Only Task Manager
  épinglant Ghostty · VSCodium · Firefox · Dolphin · moniteur système (défauts
  éditables, DESKTOP §2.3).
- Chaque dimension renvoie à un **token** du design system (30 px chrome, 48 px =
  `space-12`, etc.) — commentaires en clair dans le fichier.

> **Limite d'API assumée.** Le script Plasma ne pilote **pas** l'opacité/le flou
> d'un panneau : `floating = true` en fait des dalles flottantes, mais le verre
> `glass-chrome` (Mantle 72 % + blur, §6.1) est apporté par le **preset Panel
> Colorizer** (Phase 2). Le `defaults` active en amont les effets **Blur** +
> **Background Contrast** de KWin pour que ce verre floute réellement le fond.

---

## 6. Ce qui est Phase 2 (câblage image)

| Élément | Statut | Note |
|---|---|---|
| Schéma `VibeOSDark.colors` | ✅ v0.1 | Seule couche bureau réellement embarquée aujourd'hui |
| Paquet Look-and-Feel `org.vibeos.dark` (ce dossier) | 🛣️ Phase 2 | Copie sous `/usr/share/plasma/look-and-feel/`, activation par défaut |
| Style + thème Kvantum `VibeOSDark` | 🛣️ Phase 2 | Nécessite le paquet `kvantum` (`os/Containerfile`) ; atlas SVG complet = Phase 5 |
| Preset Panel Colorizer (verre panneau/dock) | 🛣️ Phase 2 | Le verre `glass-chrome` réel |
| Application par défaut du thème (kdeglobals `/etc/skel`) | 🛣️ Phase 2 | Pour que la session s'ouvre déjà en VibeOS Dark |
| Icônes/curseurs VibeOS | 🛣️ Phase 5 | v0.1/Phase 2 : Breeze (design §9) |
| Wallpapers Genesis/Void, logo, SDDM, Plymouth | 🛣️ Phase 5 | Branding |

---

## 7. Licences

Global Theme, `defaults`, `layout.js`, thème Kvantum : **MIT** (dérivé de
Catppuccin Mocha, MIT — attribution conservée). Kvantum (moteur) : GPL-2.0+,
lié/utilisé tel quel. Icônes/curseurs Breeze : LGPL. Tout est redistribuable
dans une ISO — condition d'entrée de [docs/ECOSYSTEM.md](../../docs/ECOSYSTEM.md).
