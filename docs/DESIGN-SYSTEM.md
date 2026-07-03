# Système de design « VibeOS »

> Version : v0.1 (fondation du langage visuel) — Date : 2026-07-03
> Statut : document de référence **design** du chantier bureau. Amont : [DESKTOP.md](DESKTOP.md) (expérience bureau, triptyque Agent/Contexte/Confiance), [desktop/theme/palette.md](../desktop/theme/palette.md) (palette exacte), [desktop/theme/vibeos-dark.colors](../desktop/theme/vibeos-dark.colors) (schéma Plasma). Prose **FR**, tokens/code/commentaires **EN**.
>
> **Règle d'honnêteté (invariant projet).** Ce document définit le langage de design *cible*, complet et ambitieux. La couche réellement embarquée en v0.1 se limite au schéma de couleurs, aux dotfiles terminal et aux polices. Le **Global Theme**, le **panneau/dock verre**, le **HUD Quickshell** et ses données vives sont des livrables **Phase 2** ; **SDDM**, **Plymouth**, **wallpapers** et branding relèvent de la **Phase 5**. Chaque surface porte sa phase. Aucune donnée du HUD n'est « live » aujourd'hui : elle se conçoit finie, se livre mockée et labellisée.
>
> **Portée & non-régression.** Le design system *élève* l'identité « VibeOS Dark » sans casser ses invariants : palette Mocha verbatim, accent Mauve unique, code couleur des tiers T0–T3 stable dans tout l'OS. Il **ajoute** une grammaire (surfaces, élévation, verre, mouvement, typographie, formes) — il ne remplace ni la palette ni la sémantique existantes.

---

## Sommaire

1. [Principes directeurs](#1-principes-directeurs)
2. [Tokens — couleur & surfaces](#2-tokens--couleur--surfaces)
3. [Tokens — typographie](#3-tokens--typographie)
4. [Tokens — rayons, espacements, formes](#4-tokens--rayons-espacements-formes)
5. [Tokens — élévation & ombres](#5-tokens--élévation--ombres)
6. [Tokens — verre (translucidité & flou)](#6-tokens--verre-translucidité--flou)
7. [Tokens — dégradés signature](#7-tokens--dégradés-signature)
8. [Mouvement](#8-mouvement)
9. [Iconographie & formes](#9-iconographie--formes)
10. [Tiers de policy — pastilles, anneaux, cadenas](#10-tiers-de-policy--pastilles-anneaux-cadenas)
11. [Application par surface](#11-application-par-surface)
12. [Blocs de tokens prêts à copier](#12-blocs-de-tokens-prêts-à-copier)
13. [Références & tendances 2026](#13-références--tendances-2026)
14. [Anti-patterns](#14-anti-patterns)
15. [Accessibilité & contrôle qualité](#15-accessibilité--contrôle-qualité)

---

## 1. Principes directeurs

Sept principes gouvernent chaque décision. En cas de conflit, l'ordre fait foi : **la lisibilité et le calme priment toujours sur l'effet**.

1. **Profondeur & hiérarchie.** L'écran est un espace *stratifié*, pas un aplat. Chaque surface occupe un plan (canvas → chrome → carte → flottant → overlay), signalé par une combinaison cohérente *surface + élévation + ombre + hairline*. On ne décore jamais la profondeur : on l'utilise pour dire *ce qui est important, ce qui est actif, ce qui attend*.

2. **Translucidité maîtrisée (le verre au service du contexte).** Le verre (blur + opacité) n'apparaît que sur les surfaces *flottantes au-dessus du contenu* : panneau, dock, HUD, menus, OSD. Jamais sur le contenu lui-même (terminal, éditeur restent opaques et nets). Le verre laisse *deviner* la profondeur sans jamais nuire à la lecture du texte qu'il porte.

3. **Calme & focus (design pour le vibecoding).** L'écran d'un développeur qui dirige des agents doit *reposer l'œil*. Faible saturation par défaut, contrastes doux entre surfaces, un seul accent, zéro élément clignotant sans raison. Le mouvement et la couleur sont un budget rare qu'on dépense uniquement pour *informer* (un agent attend, une approbation est requise).

4. **L'accent Mauve est la signature — et rien d'autre.** `#cba6f7` = identité, focus, agent actif. Il ne signifie jamais un danger, ne devient jamais un tier, ne se dilue jamais dans un arc-en-ciel d'accents. Une seule couleur de marque, tenue avec discipline, vaut mieux que dix.

5. **Lisibilité mono, d'abord.** La donnée technique (chemins, durées, tokens, tiers, git) est *monospace*, alignée, tabulaire. Le mono n'est pas un choix rétro : c'est la police de la vérité machine. Le sans-serif ne sert qu'aux titres et au texte de confort.

6. **Mouvement intentionnel.** Toute animation répond à une cause (entrée d'un panneau, changement d'état d'un agent, feedback d'un clic). Entrées douces, feedback immédiat, sorties rapides. Jamais de mouvement gratuit, jamais de parallaxe décorative, toujours respectueux de `prefers-reduced-motion`.

7. **Cohérence par les tokens.** Aucune valeur en dur nulle part. Une couleur, un rayon, une durée, une ombre existent *une fois* comme token et se propagent (CSS, QML, Plasma). Un composant qui invente sa propre valeur est un bug de design.

> **Règle d'or de cohérence (rappelée à chaque surface, §11).** Boot, login, bureau, HUD, terminal, éditeur partagent : la **palette**, le **rayon de famille**, l'**accent Mauve**, le **code des tiers**, la **grammaire de mouvement**. Une surface peut simplifier (le boot est minimal) mais ne peut pas *contredire*.

---

## 2. Tokens — couleur & surfaces

### 2.1 Palette de base (invariant — Mocha verbatim)

Reprise intégrale de [palette.md](../desktop/theme/palette.md). **Ne pas modifier** — c'est l'ADN partagé avec le schéma Plasma et les thèmes terminal.

| Rôle | Nom | Hex | RGB |
|---|---|---|---|
| Fond creux | Crust | `#11111b` | 17,17,27 |
| Chrome | Mantle | `#181825` | 24,24,37 |
| Contenu | Base | `#1e1e2e` | 30,30,46 |
| Surface | Surface0 | `#313244` | 49,50,68 |
| Surface+ | Surface1 | `#45475a` | 69,71,90 |
| Surface++ | Surface2 | `#585b70` | 88,91,112 |
| Texte | Text | `#cdd6f4` | 205,214,244 |
| Texte 2 | Subtext1 | `#bac2de` | 186,194,222 |
| Texte 3 | Subtext0 | `#a6adc8` | 166,173,200 |
| Muet | Overlay2 | `#9399b2` | 147,153,178 |
| Muet 2 | Overlay1 | `#7f849c` | 127,132,156 |
| Muet 3 | Overlay0 | `#6c7086` | 108,112,134 |
| **Accent** | **Mauve** | `#cba6f7` | 203,166,247 |
| T0 | Blue | `#89b4fa` | 137,180,250 |
| T1 | Green | `#a6e3a1` | 166,227,161 |
| T2 | Peach | `#fab387` | 250,179,135 |
| T3 | Red | `#f38ba8` | 243,139,168 |
| Attente | Yellow | `#f9e2af` | 249,226,175 |
| Focus 2 | Lavender | `#b4befe` | 180,190,254 |
| Jauges | Sky | `#89dceb` | 137,220,235 |

### 2.2 Échelle de surfaces s0..s4 (l'échelle d'élévation)

L'élévation en dark UI suit une règle physique : **plus une surface est haute, plus elle est claire** (elle se rapproche de la lumière). VibeOS fige une **échelle monotone** de 5 plans, dérivée de Mocha, plus une intercalaire calculée (`s1r`) pour les cartes qui doivent se détacher du canvas sans sauter jusqu'à Surface0.

| Token | Plan | Hex | Dérivation | Usage |
|---|---|---|---|---|
| `surface-0` | **Void** (le plus profond) | `#11111b` (Crust) | verbatim | Backdrop derrière le verre, OSD, écran de verrouillage, puits de HUD, fond « enfoncé » |
| `surface-1` | **Chrome** | `#181825` (Mantle) | verbatim | Panneau, dock, HUD (fond opaque de repli), barres de titre, fond de fenêtre |
| `surface-2` | **Canvas** | `#1e1e2e` (Base) | verbatim | Contenu : terminal, éditeur, listes, corps de dialogue |
| `surface-1r` | **Carte discrète** | `#26263a` | mix(Base, Surface0) — calculé (38,38,58) | Cartes/sections qui se posent *légèrement* sur le canvas (lignes de liste survolables, en-têtes de section) |
| `surface-3` | **Carte / contrôle** | `#313244` (Surface0) | verbatim | Boutons, champs, cartes du HUD, popovers posés |
| `surface-4` | **Flottant haut** | `#45475a` (Surface1) | verbatim | Menus, tooltips solides, survol de carte, éléments au premier plan |

> **Note dérivation `#26263a`.** Milieu perceptuel Base↔Surface0 : `round((30+49)/2, (30+50)/2, (46+68)/2)` = `(40,40,57)` → arrondi esthétique `#26263a` (38,38,58), légèrement retenu vers le froid pour rester discret. C'est la **seule** valeur hors-Mocha de l'échelle ; toute autre dérive doit être documentée ici.

### 2.3 Texte — rôles

| Token | Couleur | Usage | Contraste / Base |
|---|---|---|---|
| `text-primary` | Text `#cdd6f4` | Corps, titres, données actives | ~11,9:1 (AAA) |
| `text-secondary` | Subtext1 `#bac2de` | Labels, sous-titres | ~9,4:1 |
| `text-tertiary` | Subtext0 `#a6adc8` | Métadonnées, en-têtes inactifs | ~7,0:1 |
| `text-muted` | Overlay1 `#7f849c` | Désactivé, commentaires, hors-ligne | ~3,9:1 — **jamais** pour du texte essentiel |
| `text-placeholder` | Overlay2 `#9399b2` | Placeholders, aide | ~5,2:1 |
| `text-on-accent` | Crust `#11111b` | Texte posé sur un aplat Mauve (sélection, bouton primaire) | ~7,4:1 |
| `text-accent` | Mauve `#cba6f7` | Lien de marque, valeur d'agent actif | ~6,9:1 |

### 2.4 Bordures & hairlines

Les séparations se font par **hairline translucide** (dérivée du texte), pas par des lignes opaques dures — c'est plus doux et s'adapte à la surface sous-jacente.

| Token | Valeur | Usage |
|---|---|---|
| `hairline` | `rgba(205,214,244, 0.08)` | Séparateur fin par défaut (1px) |
| `border-subtle` | `rgba(205,214,244, 0.12)` | Contour de carte, de champ au repos |
| `border-strong` | `rgba(205,214,244, 0.20)` | Contour actif, champ focalisé (hors accent) |
| `border-accent` | Mauve `#cba6f7` | Contour de focus/sélection |
| `glass-edge-top` | `rgba(255,255,255, 0.10)` | Arête spéculaire haute du verre (1px) |
| `glass-edge-bottom` | `rgba(0,0,0, 0.28)` | Ombre d'arête basse du verre (1px) |

### 2.5 États d'interaction (hover / active / focus / selected / disabled)

Les états se composent par **overlay** sur la surface de base (méthode « state layer ») — ils fonctionnent quelle que soit la surface dessous.

| État | Token | Valeur | Règle |
|---|---|---|---|
| Repos | — | surface de base | — |
| Survol | `state-hover` | overlay `rgba(205,214,244, 0.05)` | Feedback ≤ 120 ms |
| Pressé/actif | `state-active` | overlay `rgba(17,17,27, 0.35)` (assombrit) | Feedback immédiat ≤ 90 ms |
| Sélectionné | `state-selected` | fill `rgba(203,166,247, 0.16)` + barre gauche 2px Mauve | Mauve = focus/sélection |
| Focus clavier | `state-focus-ring` | anneau 2px `rgba(203,166,247, 0.60)` **+** halo `0 0 0 4px rgba(203,166,247, 0.16)` | Toujours visible, jamais supprimé |
| Désactivé | `state-disabled` | `opacity: 0.38`, texte → `text-muted` | Aucun événement, aucun mouvement |

---

## 3. Tokens — typographie

### 3.1 Familles

| Token | Famille | Licence / statut | Rôle |
|---|---|---|---|
| `font-mono` | **JetBrains Mono** | OFL-1.1 — **dans l'image v0.1** | Terminal, éditeurs, **toute donnée** du HUD (chemins, durées, tiers, compteurs), code |
| `font-mono-alt` | Fira Code | OFL-1.1 — dans l'image v0.1 | Alternative documentée, sélectionnable |
| `font-sans` | **Inter** (variable) | OFL-1.1 — **à packager** (Phase 2/5) | Titres UI, texte de confort, greeting login. *Aspirationnel : doit être ajouté à l'image pour être utilisé.* |
| `font-sans-fallback` | **Noto Sans** | OFL-1.1 — dans l'image (héritage Fedora) | **Fallback shippé** de `font-sans` : tant qu'Inter n'est pas packagé, l'UI reste sur Noto Sans. La pile CSS/QML doit toujours lister Inter *puis* Noto Sans. |

> **Honnêteté typographique.** En v0.1, l'UI Plasma est en **Noto Sans**. Spécifier Inter comme face de titres est une *cible* : elle n'entre en vigueur qu'une fois `google-inter-fonts` (ou l'équivalent variable) déclaré dans `os/Containerfile` — chantier OS séparé. La pile de repli garantit qu'aucune surface ne « casse » sans Inter.

### 3.2 Échelle typographique

Échelle modulaire (tierce mineure ~1.2 sur l'UI), exprimée en `px` (surfaces desktop à densité fixe). `line-height` en unité relative, `letter-spacing` en `em`.

| Token | Taille | Poids | Interligne | Tracking | Famille | Usage |
|---|---|---|---|---|---|---|
| `type-display` | 28px | 600 | 1.15 | −0.02em | sans | Greeting login, grands états vides, moment de marque |
| `type-h1` | 22px | 600 | 1.20 | −0.015em | sans | Titre de panneau (HUD), en-tête de dialogue |
| `type-h2` | 18px | 600 | 1.25 | −0.01em | sans | Sous-section, titre de carte |
| `type-h3` | 15px | 600 | 1.30 | 0 | sans | En-tête de groupe, label fort |
| `type-body` | 13px | 400 | 1.50 | 0 | sans | Texte UI courant, description |
| `type-body-strong` | 13px | 550 | 1.50 | 0 | sans | Emphase dans le corps |
| `type-small` | 12px | 400 | 1.45 | 0 | sans | Texte secondaire, aide |
| `type-caption` | 11px | 550 | 1.40 | +0.06em | sans | Labels de badge, en-têtes de colonne — **UPPERCASE** |
| `type-mono` | 13px | 400 | 1.50 | 0 | mono | Donnée HUD, valeurs, chemins |
| `type-mono-strong` | 13px | 600 | 1.50 | 0 | mono | Donnée mise en avant (agent actif, compteur clé) |
| `type-mono-sm` | 11px | 400 | 1.45 | 0 | mono | Donnée dense, tableaux d'audit |

> **Règle titres vs corps vs mono.** Titre = `font-sans`, poids 600, tracking négatif (le sans se resserre en grand). Corps = `font-sans` 400. **Donnée technique = toujours `font-mono`**, jamais du sans — un chemin, une durée, un tier se lisent en mono, alignés. Le terminal et l'éditeur gardent leur propre taille (11 pt par défaut, cf. DESKTOP §5.2) : l'échelle ci-dessus régit **l'UI et le HUD**, pas le contenu du terminal.

---

## 4. Tokens — rayons, espacements, formes

### 4.1 Rayons (`radius`)

Famille cohérente : contrôles moyens (`md`), cartes (`lg`), grands panneaux verre (`xl`/`2xl`), pastilles/anneaux (`full`).

| Token | Valeur | Usage |
|---|---|---|
| `radius-xs` | 4px | Badges denses, puces, coins de champ intérieur |
| `radius-sm` | 6px | Boutons compacts, chips, tooltips |
| `radius-md` | 10px | Boutons, champs de saisie, contrôles standard |
| `radius-lg` | 14px | Cartes, popovers, lignes de liste sélectionnables |
| `radius-xl` | 20px | Panneaux HUD, cartes de login, dialogues |
| `radius-2xl` | 28px | Grandes surfaces verre flottantes, feuilles modales |
| `radius-full` | 999px | Pastilles de tier, anneaux d'agent, avatars, pilules d'état |

> **Cohérence Breeze.** Les *fenêtres* KWin gardent les coins Breeze (DESKTOP §2.5) : le rayon VibeOS s'applique aux surfaces qu'on dessine (HUD, cartes, badges, SDDM, Plymouth), pas à la décoration de fenêtre native.

### 4.2 Espacements — échelle 4pt (`space`)

Base 4px. Tout gap, padding, marge se prend dans cette échelle — **aucune valeur intermédiaire**.

| Token | px | Usage typique |
|---|---|---|
| `space-0` | 0 | Flush |
| `space-1` | 4 | Écart intra-composant (icône↔label serré) |
| `space-2` | 8 | Padding de chip, gap de liste dense |
| `space-3` | 12 | Padding de bouton, gap standard |
| `space-4` | 16 | Padding de carte, gouttière de base |
| `space-5` | 20 | Gap de section |
| `space-6` | 24 | Padding de panneau, marge de bloc |
| `space-8` | 32 | Séparation de groupes |
| `space-10` | 40 | Respiration large (HUD, dialogue) |
| `space-12` | 48 | Marges de page/login |
| `space-16` | 64 | Grandes zones vides, centrage de moment |

### 4.3 Formes & épaisseurs de trait

- **Grille de layout** : colonnes implicites en multiples de `space-2` (8px). Hauteur de rang HUD = `space-8` (32px) ; hauteur de contrôle standard = `space-8`..`space-10`.
- **Épaisseurs de trait UI** : hairline 1px, bordure focus 2px, barre de sélection 2px, indicateur d'activité 2px, anneau de tier 2px (idle) → 2.5px (actif/pulsé).
- **Densité** : le HUD et les listes d'audit privilégient une densité *confortable-compacte* (rangée 32px) ; les dialogues d'approbation T2/T3 s'aèrent (padding `space-6`) pour ralentir la décision.

---

## 5. Tokens — élévation & ombres

L'élévation = **surface (§2.2) + ombre + hairline** appliqués ensemble. Les ombres restent *profondes et douces* (noir teinté Crust, jamais gris), le budget d'ombre augmente avec le plan. On n'utilise **jamais** `#000` pur : `rgba(0,0,0,·)` sur fond déjà sombre suffit et évite le halo.

| Token | Ombre | Surface associée | Usage |
|---|---|---|---|
| `elevation-0` | aucune | canvas | Contenu à plat, terminal |
| `elevation-1` | `0 1px 2px rgba(0,0,0,0.30)` | `surface-1r` / `surface-3` | Carte au repos, chip |
| `elevation-2` | `0 4px 12px rgba(0,0,0,0.35)` | `surface-3` | Carte surélevée, bouton flottant, dock |
| `elevation-3` | `0 12px 32px rgba(0,0,0,0.45)` | `surface-3` verre | Popover, menu, panneau HUD |
| `elevation-4` | `0 24px 64px rgba(0,0,0,0.55)` | `surface-3`/`4` verre | Dialogue, feuille modale, carte de login |

**Lueurs d'accent** (élévation *sémantique*, pas physique — signalent l'état, pas la hauteur) :

| Token | Valeur | Usage |
|---|---|---|
| `glow-focus` | `0 0 0 4px rgba(203,166,247, 0.16)` | Halo de focus clavier (avec l'anneau 2px, §2.5) |
| `glow-agent-active` | `0 0 24px rgba(203,166,247, 0.22)` | Bloom mauve doux autour de l'indicateur d'agent en cours |
| `glow-attention-t2` | `0 0 20px rgba(250,179,135, 0.30)` | Pulsation Peach — approbation T2 attendue |
| `glow-alert-t3` | `0 0 20px rgba(243,139,168, 0.32)` | Pulsation Red — action T3 / destructive |

> Les lueurs sont **rares** et **animées avec parcimonie** (pulsation lente, §8). Une lueur permanente sur tout devient du bruit ; réservez-les à « quelque chose demande votre attention ».

---

## 6. Tokens — verre (translucidité & flou)

Le verre est la matière des surfaces *flottantes*. Il se compose toujours de **4 couches** : (1) fond translucide, (2) flou d'arrière-plan, (3) arête spéculaire haute + arête d'ombre basse (§2.4), (4) ombre d'élévation (§5). Sans ces couches, ce n'est pas du verre, c'est de la transparence molle.

### 6.1 Recettes de verre

| Token | Fond | Blur | Bord | Élévation | Usage |
|---|---|---|---|---|---|
| `glass-chrome` | `rgba(24,24,37, 0.72)` (Mantle 72%) | `blur(32px) saturate(120%)` | edge-top + hairline | `elevation-2` | Panneau supérieur, dock |
| `glass-panel` | `rgba(17,17,27, 0.66)` (Crust 66%) | `blur(40px) saturate(125%)` | edge-top + edge-bottom | `elevation-3` | Panneau HUD flottant, feuilles d'agents |
| `glass-elevated` | `rgba(30,30,46, 0.78)` (Base 78%) | `blur(24px) saturate(115%)` | border-subtle + edge-top | `elevation-4` | Menus, popovers, carte de login, dialogues |
| `glass-osd` | `rgba(17,17,27, 0.80)` (Crust 80%) | `blur(24px)` | hairline | `elevation-3` | OSD volume/luminosité, toasts |

### 6.2 Paramètres

- **Flou** : 3 niveaux — `blur-thin: 24px` (menus/OSD), `blur-base: 32px` (chrome), `blur-heavy: 40px` (HUD/grandes surfaces). Au-delà de 40px, coût GPU et perte de repères.
- **Opacité du fond** : plage **60–80 %**. En dessous de 60 %, le texte porté devient illisible sur fond chargé ; au-dessus de 80 %, autant faire opaque.
- **`saturate(115–125%)`** : léger regain de saturation du fond flouté → le verre paraît *vivant* sans virer au laiteux.
- **Repli sans compositeur / faibles perfs** : si le flou n'est pas disponible (pas de blur Wayland, `prefers-reduced-transparency`), **tomber sur la surface opaque équivalente** (Mantle/Crust/Base à 100 %) + hairline. Le verre est un rehaussement, jamais une dépendance de lisibilité.
- **Lisibilité du texte sur verre** : viser un fond *effectif* ≥ Mantle. Si le contenu derrière peut être clair, ajouter un voile interne (`inset 0 0 0 100vmax rgba(17,17,27,0.35)`) sous le texte critique.

---

## 7. Tokens — dégradés signature

Les dégradés sont **subtils, faible amplitude, jamais criards**. Ils servent l'accent et la profondeur, pas la décoration. Un dégradé VibeOS parcourt au plus 2–3 teintes voisines dans le cercle chromatique (Mauve↔Lavender↔Blue), jamais des opposés.

| Token | Définition | Usage |
|---|---|---|
| `grad-signature` | `linear-gradient(135deg, #cba6f7 0%, #89b4fa 100%)` | **Le** dégradé de marque : Mauve→Blue. Filet d'accent, barre active, anneau d'agent en cours, arc de progression |
| `grad-signature-soft` | `linear-gradient(135deg, rgba(203,166,247,0.24) 0%, rgba(137,180,250,0.16) 100%)` | Version fond : remplissage de carte « agent actif », survol de HUD |
| `grad-accent-line` | `linear-gradient(90deg, #cba6f7 0%, #b4befe 100%)` | Souligné actif (onglet, activité courante), 2px |
| `grad-hud-veil` | `linear-gradient(180deg, rgba(24,24,37,0) 0%, rgba(17,17,27,0.60) 100%)` | Voile bas du HUD pour asseoir le texte sur fond variable |
| `grad-mesh-genesis` | `radial-gradient(120% 90% at 78% 8%, rgba(203,166,247,0.20) 0%, rgba(137,180,250,0.08) 32%, rgba(30,30,46,0) 60%), linear-gradient(180deg, #1e1e2e 0%, #11111b 100%)` | **Wallpaper « Genesis »** (activités Vibe/Review) : halo mauve→blue en haut-droite, fondu Base→Crust. Aurora douce, très faible saturation |
| `grad-void` | `radial-gradient(100% 80% at 50% 40%, #16161f 0%, #11111b 70%)` | **Wallpaper « Void »** (activité Focus) : Crust quasi uni, respiration minimale |

**Dégradés de tiers** (anneaux, §10) — chacun de sa teinte vers une version assombrie de lui-même, jamais vers une autre teinte :

| Token | Définition |
|---|---|
| `grad-tier-t0` | `conic-gradient(from 220deg, #89b4fa, #74c7ec, #89b4fa)` (Blue) |
| `grad-tier-t1` | `conic-gradient(from 220deg, #a6e3a1, #94e2d5, #a6e3a1)` (Green) |
| `grad-tier-t2` | `conic-gradient(from 220deg, #fab387, #f9e2af, #fab387)` (Peach) |
| `grad-tier-t3` | `conic-gradient(from 220deg, #f38ba8, #eba0ac, #f38ba8)` (Red) |

---

## 8. Mouvement

Le mouvement de VibeOS est **discret, rapide, causé**. Il obéit à trois durées et quatre courbes ; tout le reste en dérive.

### 8.1 Durées (`motion-duration`)

| Token | Durée | Usage |
|---|---|---|
| `duration-instant` | 90ms | Feedback de pression (bouton enfoncé), changement d'état de survol immédiat |
| `duration-fast` | 120ms | Survol, focus, micro-transitions, tooltips |
| `duration-base` | 200ms | Transition standard : ouverture de menu, changement d'onglet, fade de contenu |
| `duration-slow` | 320ms | Entrée/sortie de panneau HUD, dépli `Meta+V`, dialogue |
| `duration-entrance` | 400ms | **Rare** : grands moments (apparition du login, première pose du HUD). Jamais pour du récurrent |

### 8.2 Courbes (`motion-easing`)

| Token | Courbe | Usage |
|---|---|---|
| `ease-standard` | `cubic-bezier(0.4, 0.0, 0.2, 1)` | Transition par défaut (in-out), la plupart des cas |
| `ease-decelerate` | `cubic-bezier(0.0, 0.0, 0.2, 1)` | **Entrées** : l'élément arrive et se pose en douceur |
| `ease-accelerate` | `cubic-bezier(0.4, 0.0, 1.0, 1)` | **Sorties** : l'élément part vite, sans traîner |
| `ease-emphasized` | `cubic-bezier(0.2, 0.9, 0.1, 1)` | Signature « ressort » léger : dépli du HUD, révélation d'un panneau. Une pointe d'élan, zéro rebond gratuit |

### 8.3 Principes de mouvement

- **Entrées douces** : `fade (0→1) + translate 8px` sur `duration-base` / `ease-decelerate`. Les surfaces montent légèrement en apparaissant.
- **Feedback immédiat** : toute interaction produit une réponse ≤ `duration-fast`. Un clic sans retour visuel est un bug.
- **Sorties nettes** : on part en `duration-fast`/`ease-accelerate` — on ne fait pas *attendre* l'utilisateur pour fermer.
- **Pulsations d'état** (approbation T2/T3, agent actif) : **lentes** (cycle 1400–1800ms, opacité 0.6↔1.0, `ease-standard`), pour *appeler l'œil sans agresser*. Jamais de clignotement rapide.
- **Jamais gratuit** : pas de parallaxe décorative, pas d'animation d'idle, pas de « wobbly windows » (DESKTOP §2.5). Le mouvement dit toujours *quelque chose a changé*.
- **`prefers-reduced-motion`** : réduire à des fondus d'opacité sans translation, supprimer les pulsations (remplacées par un état statique visible — ex. anneau plein). Obligatoire.

---

## 9. Iconographie & formes

- **Style** : icônes **linéaires** (outline), pas de remplissage plein par défaut. Trait **1.5px** sur grille **24px** (mise à l'échelle proportionnelle : ~1.75px à 28px, ~1.25px à 20px). Bouts et jonctions **arrondis** (`round` caps/joins), rayon de coin interne ~2px. Familles de référence de l'esprit visé : **Phosphor**, **Lucide**, **Tabler** (lignes nettes, géométriques, chaleureuses).
- **Remplissage** : réservé à l'**état actif/sélectionné** (une icône se remplit quand son élément est actif) et aux **pastilles de tier** (§10).
- **Grille & marge optique** : zone de sécurité 2px, alignement optique privilégié sur l'alignement géométrique (une flèche « paraît » centrée avant d'« être » centrée).
- **Statut v0.1** : le jeu d'icônes/curseurs shippé est **Breeze** (DESKTOP §5.3) ; les icônes dessinées par VibeOS (HUD, tiers, logo) suivent la spec ci-dessus et se généralisent en **Phase 5** (jeu d'icônes complet).
- **Formes de marque** : coins de la famille `radius` (§4.1), jamais de coins parfaitement carrés sur les surfaces dessinées. Le motif signature est l'**anneau** (agent, tier) — cercle ouvert/dégradé, cohérent du HUD au login.

---

## 10. Tiers de policy — pastilles, anneaux, cadenas

Le code couleur des tiers est un **invariant produit** (palette.md §2) : la même couleur signale le même tier *partout* (HUD, terminal, notification, dialogue d'approbation). Le design system fixe **les formes** de ce code.

### 10.1 Vocabulaire visuel

| Élément | Forme | Signification |
|---|---|---|
| **Pastille** (dot) | disque plein `radius-full`, 8px | État *au repos* d'un tier : présence, comptage, légende |
| **Anneau** (ring) | cercle ouvert 2px, dégradé de tier (§7) | Tier *actif* : un appel de ce tier est en cours |
| **Anneau pulsé** | anneau 2.5px + `glow-attention` (§5), pulsation lente (§8.3) | *Attente d'approbation* — réservé T2/T3 |
| **Cadenas** | glyphe cadenas linéaire, superposé à la pastille | Tier **T2+** : passage par une **grille d'approbation humaine** |

### 10.2 Table des tiers

| Tier | Sémantique | Couleur | Pastille | Anneau actif | Cadenas | Règle |
|---|---|---|---|---|---|---|
| **T0** — observe | lecture seule | Blue `#89b4fa` | disque Blue | `grad-tier-t0` | non | Informationnel, jamais alarmant |
| **T1** — modify-user | fichiers utilisateur | Green `#a6e3a1` | disque Green | `grad-tier-t1` | non | Bénin, réversible, journalisé |
| **T2** — modify-system | paquets, services | Peach `#fab387` | disque Peach | `grad-tier-t2` | **oui** | Approbation requise : attire l'œil (`glow-attention-t2`, pulsation) **sans** paniquer |
| **T3** — destructive | disque, credentials | Red `#f38ba8` | disque Red | `grad-tier-t3` | **oui (renforcé)** | Approbation renforcée. Rouge = T3/erreur/suppression, **jamais décoratif** |
| Attente / indéterminé | demande en cours | Yellow `#f9e2af` | disque Yellow | — | — | Agent en pause, décision en cours |
| `vibed` hors ligne | daemon absent (Phase 1) | Overlay1 `#7f849c` | disque gris | — | — | État « offline » propre, jamais un crash |

### 10.3 Règles dures (rappel, non négociables)

1. **Le rouge n'est jamais décoratif.** Rouge à l'écran ⇒ T3, erreur ou suppression.
2. **Le mauve n'est jamais un danger ni un tier.** Identité et focus, un point c'est tout.
3. **Un tier = une couleur = une forme, partout.** Le Peach du badge HUD, la ligne d'audit du terminal et le dialogue Plasma (Phase 2) sont identiques.
4. **T2+ porte toujours le cadenas.** La grille d'approbation se *voit* avant de se lire.

---

## 11. Application par surface

Chaque surface applique les tokens ci-dessus. La **règle d'or** clôt chaque bloc : *ce qui doit rester identique d'une surface à l'autre*.

### 11.1 Boot — Plymouth · 🛣️ Phase 5

- Fond `surface-0` (Crust, quasi noir doux) — pur `#000` interdit.
- Marque VibeOS centrée (glyphe provisoire v0.1 → logo Phase 5), monochrome Text.
- Progression : **arc fin** `grad-signature` (Mauve→Blue), 2px, `radius-full`, rotation lente `ease-standard`. Pas de barre pleine, pas de pourcentage criard.
- Aucun texte hormis, au besoin, une ligne mono `text-tertiary` (état du boot).
- **Règle d'or** : le *même* Crust, le *même* dégradé signature qu'au login. Le boot est le premier accord de la partition — minimal, mais dans la tonalité.

### 11.2 Login — SDDM · 🛣️ Phase 5

- Fond `grad-mesh-genesis` (aurora mauve→blue, très basse saturation) sur Crust.
- **Carte de login en `glass-elevated`** (`radius-xl`, `elevation-4`, arête spéculaire) centrée, largeur contenue (`space-16` de marge).
- Greeting en `type-display` (`font-sans` → Inter/Noto), heure en `type-mono` `text-tertiary`.
- Champ mot de passe : `surface-3`, `radius-md`, focus = anneau Mauve + `glow-focus`. Bouton de session primaire = aplat Mauve, `text-on-accent`.
- Avatar utilisateur cerclé d'un **anneau `grad-signature`** (rappel du motif agent).
- **Règle d'or** : accent Mauve pour le focus, dégradé signature sur l'anneau, verre = même recette que le HUD. On reconnaît VibeOS *avant* d'être connecté.

### 11.3 Bureau — panneau & dock verre · 🛣️ Phase 2

- **Panneau supérieur** : `glass-chrome` (Mantle 72 %, `blur-base`), hairline bas, 30px (DESKTOP §2.2). Accent Mauve sur l'activité courante (souligné `grad-accent-line` 2px), lanceur, horloge mono.
- **Dock bas** : `glass-chrome`, masquage auto (DESKTOP §2.3), icônes Breeze, indicateur d'app active = point Mauve. `radius-lg` sur le conteneur flottant.
- **Pager d'activités** : Vibe (halo mauve) · Focus (Void, notifications off) · Review (teinte verte discrète) — cohérent avec les wallpapers (§7).
- **Règle d'or** : le verre du panneau et du dock partage la recette `glass-chrome` ; l'accent d'activité = Mauve, jamais une autre teinte.

### 11.4 HUD agents — Quickshell · 🛣️ Phase 2 (données mockées, labellisées)

- **Barre HUD** : `glass-panel` (Crust 66 %, `blur-heavy`), ancrée en haut (DESKTOP §2.4), couche additionnelle au panneau Plasma.
- **État global** : point/anneau — gris `Overlay1` (vibed hors ligne) → Mauve `glow-agent-active` (agents actifs) → Peach pulsé (approbation T2 attendue). Voir §10.
- **Panneau Agents** : cartes `surface-3` `radius-lg` `elevation-2` ; chaque agent = anneau `grad-signature` + nom (`type-body-strong`) + projet/durée/dernier outil en `type-mono-sm` `text-tertiary`.
- **Panneau Confiance** : tiers T0–T3 en pastilles/anneaux/cadenas (§10), compteurs mono tabulaires.
- **Panneau Ressources** : jauges CPU/RAM en `grad-signature-soft`, VRAM ollama en Sky.
- **Dégradation gracieuse (impérative)** : toute dépendance absente (vibed, ollama, policy) ⇒ **état affiché propre**, jamais une exception (DESKTOP §6). Les données non live sont **mockées et clairement labellisées « Phase 2 / vibed hors ligne »**.
- **Règle d'or** : le HUD est *la* vitrine du système de design — il concentre verre, tiers, accent, mono, mouvement. Tout token défini ici doit y être exact.

### 11.5 Terminal — Ghostty · ✅ v0.1

- **Opaque et net** : `surface-2` (Base), **pas de verre** (le contenu ne floute jamais). Curseur Rosewater, 16 couleurs ANSI mappées sur la palette (palette.md §3).
- Chrome minimal : le terminal est la scène (DESKTOP §1), le bureau s'efface.
- Prompt Starship : accent Mauve, git Green/Peach, erreurs Red — même sémantique de tiers.
- **Règle d'or** : mêmes couleurs de tiers et même Mauve que le HUD ; le terminal partage l'ADN chromatique, pas le verre (il porte du texte critique en continu).

### 11.6 Éditeurs — VSCodium / Neovim · ✅ v0.1

- Thème **Catppuccin Mocha** mappé (palette.md §3), accent Mauve (preset « VibeVim »).
- Contenu opaque, `surface-2` ; chrome (barres, onglets) peut emprunter `surface-1`.
- **Règle d'or** : l'éditeur ne réinvente pas la palette — il *est* la palette. Cohérence diff (Green/Red), sélection (Mauve), avec le terminal et le HUD.

---

## 12. Blocs de tokens prêts à copier

Deux cibles : **CSS custom properties** (pour tout rendu web/SDDM/docs) et **QML** (Quickshell — source de vérité du HUD). Le schéma **Plasma** encode déjà la palette (`vibeos-dark.colors`) ; les tokens non-couleur (rayon, espacement, durée) s'y expriment côté QML.

### 12.1 CSS custom properties

```css
/* VibeOS Design System — tokens (EN). Dark, immutable-OS friendly. */
/* Target: web renders, SDDM/Plymouth theming, docs. Palette = Catppuccin Mocha (MIT). */
:root {
  /* ---- Base palette (verbatim, invariant) ---- */
  --crust:    #11111b;  --mantle:   #181825;  --base:     #1e1e2e;
  --surface0: #313244;  --surface1: #45475a;  --surface2: #585b70;
  --overlay0: #6c7086;  --overlay1: #7f849c;  --overlay2: #9399b2;
  --subtext0: #a6adc8;  --subtext1: #bac2de;  --text:     #cdd6f4;
  --mauve:    #cba6f7;  --blue:     #89b4fa;  --lavender: #b4befe;
  --green:    #a6e3a1;  --peach:    #fab387;  --red:      #f38ba8;
  --yellow:   #f9e2af;  --sky:      #89dceb;  --teal:     #94e2d5;

  /* ---- Surfaces / elevation ladder (s0..s4 + s1r derived) ---- */
  --surface-0:  var(--crust);    /* void / recessed / backdrop */
  --surface-1:  var(--mantle);   /* chrome: panel, dock, HUD, titlebars */
  --surface-2:  var(--base);     /* content canvas */
  --surface-1r: #26263a;         /* derived: subtle card over canvas */
  --surface-3:  var(--surface0); /* card / control */
  --surface-4:  var(--surface1); /* floating high: menus, tooltips */

  /* ---- Text roles ---- */
  --text-primary:     var(--text);
  --text-secondary:   var(--subtext1);
  --text-tertiary:    var(--subtext0);
  --text-muted:       var(--overlay1);
  --text-placeholder: var(--overlay2);
  --text-on-accent:   var(--crust);
  --text-accent:      var(--mauve);

  /* ---- Borders / hairlines ---- */
  --hairline:          rgba(205,214,244,0.08);
  --border-subtle:     rgba(205,214,244,0.12);
  --border-strong:     rgba(205,214,244,0.20);
  --border-accent:     var(--mauve);
  --glass-edge-top:    rgba(255,255,255,0.10);
  --glass-edge-bottom: rgba(0,0,0,0.28);

  /* ---- Interaction states ---- */
  --state-hover:      rgba(205,214,244,0.05);
  --state-active:     rgba(17,17,27,0.35);
  --state-selected:   rgba(203,166,247,0.16);
  --state-focus-ring: rgba(203,166,247,0.60);
  --state-disabled-opacity: 0.38;

  /* ---- Policy tiers ---- */
  --tier-t0: var(--blue);   --tier-t1: var(--green);
  --tier-t2: var(--peach);  --tier-t3: var(--red);
  --tier-wait: var(--yellow); --tier-offline: var(--overlay1);

  /* ---- Radius ---- */
  --radius-xs: 4px;  --radius-sm: 6px;  --radius-md: 10px;
  --radius-lg: 14px; --radius-xl: 20px; --radius-2xl: 28px;
  --radius-full: 999px;

  /* ---- Spacing (4pt) ---- */
  --space-0: 0;    --space-1: 4px;  --space-2: 8px;  --space-3: 12px;
  --space-4: 16px; --space-5: 20px; --space-6: 24px; --space-8: 32px;
  --space-10: 40px; --space-12: 48px; --space-16: 64px;

  /* ---- Elevation (shadows) ---- */
  --elevation-1: 0 1px 2px rgba(0,0,0,0.30);
  --elevation-2: 0 4px 12px rgba(0,0,0,0.35);
  --elevation-3: 0 12px 32px rgba(0,0,0,0.45);
  --elevation-4: 0 24px 64px rgba(0,0,0,0.55);
  --glow-focus:        0 0 0 4px rgba(203,166,247,0.16);
  --glow-agent-active: 0 0 24px rgba(203,166,247,0.22);
  --glow-attention-t2: 0 0 20px rgba(250,179,135,0.30);
  --glow-alert-t3:     0 0 20px rgba(243,139,168,0.32);

  /* ---- Glass ---- */
  --blur-thin: 24px; --blur-base: 32px; --blur-heavy: 40px;
  --glass-chrome-bg:   rgba(24,24,37,0.72);
  --glass-panel-bg:    rgba(17,17,27,0.66);
  --glass-elevated-bg: rgba(30,30,46,0.78);
  --glass-osd-bg:      rgba(17,17,27,0.80);

  /* ---- Signature gradients ---- */
  --grad-signature:      linear-gradient(135deg, #cba6f7 0%, #89b4fa 100%);
  --grad-signature-soft: linear-gradient(135deg, rgba(203,166,247,0.24) 0%, rgba(137,180,250,0.16) 100%);
  --grad-accent-line:    linear-gradient(90deg, #cba6f7 0%, #b4befe 100%);

  /* ---- Typography ---- */
  --font-mono:  "JetBrains Mono", "Fira Code", monospace;
  --font-sans:  "Inter", "Noto Sans", sans-serif; /* Inter must be added to the image */

  /* ---- Motion ---- */
  --duration-instant: 90ms;  --duration-fast: 120ms; --duration-base: 200ms;
  --duration-slow: 320ms;    --duration-entrance: 400ms;
  --ease-standard:   cubic-bezier(0.4, 0.0, 0.2, 1);
  --ease-decelerate: cubic-bezier(0.0, 0.0, 0.2, 1);
  --ease-accelerate: cubic-bezier(0.4, 0.0, 1.0, 1);
  --ease-emphasized: cubic-bezier(0.2, 0.9, 0.1, 1);
}

/* Example: a glass HUD panel */
.vibe-glass-panel {
  background: var(--glass-panel-bg);
  backdrop-filter: blur(var(--blur-heavy)) saturate(125%);
  border-radius: var(--radius-xl);
  border-top: 1px solid var(--glass-edge-top);
  box-shadow: var(--elevation-3);
  color: var(--text-primary);
}
@media (prefers-reduced-transparency: reduce) {
  .vibe-glass-panel { background: var(--surface-1); backdrop-filter: none; }
}
@media (prefers-reduced-motion: reduce) {
  * { animation-duration: 0.001ms !important; transition-duration: 0.001ms !important; }
}
```

### 12.2 QML — singleton de tokens Quickshell

```qml
// VibeOS Design System — QML tokens singleton (EN).
// Single source of truth for the Quickshell HUD (Phase 2).
// Install target (image build): /usr/share/vibeos/quickshell/Theme/Tokens.qml
// Source lives in the repo: desktop/quickshell/Theme/Tokens.qml (referenced, not created here).
// Usage: import "..." then `Tokens.mauve`, `Tokens.radiusLg`, `Tokens.durBase`, ...
pragma Singleton
import QtQuick

QtObject {
    // ---- Base palette (verbatim, invariant) ----
    readonly property color crust:    "#11111b"
    readonly property color mantle:   "#181825"
    readonly property color base:     "#1e1e2e"
    readonly property color surface0: "#313244"
    readonly property color surface1: "#45475a"
    readonly property color surface2: "#585b70"
    readonly property color overlay1: "#7f849c"
    readonly property color subtext0: "#a6adc8"
    readonly property color subtext1: "#bac2de"
    readonly property color text:     "#cdd6f4"
    readonly property color mauve:    "#cba6f7"
    readonly property color blue:     "#89b4fa"
    readonly property color lavender: "#b4befe"
    readonly property color green:    "#a6e3a1"
    readonly property color peach:    "#fab387"
    readonly property color red:      "#f38ba8"
    readonly property color yellow:   "#f9e2af"
    readonly property color sky:      "#89dceb"

    // ---- Surfaces / elevation ladder ----
    readonly property color surface_0:  crust      // void
    readonly property color surface_1:  mantle     // chrome
    readonly property color surface_2:  base       // content
    readonly property color surface_1r: "#26263a"  // derived subtle card
    readonly property color surface_3:  surface0   // card / control
    readonly property color surface_4:  surface1   // floating high

    // ---- Text roles ----
    readonly property color textPrimary:   text
    readonly property color textSecondary: subtext1
    readonly property color textTertiary:  subtext0
    readonly property color textMuted:     overlay1
    readonly property color textOnAccent:  crust

    // ---- Policy tiers ----
    readonly property color tierT0: blue
    readonly property color tierT1: green
    readonly property color tierT2: peach
    readonly property color tierT3: red
    readonly property color tierWait: yellow
    readonly property color tierOffline: overlay1

    // ---- Borders / states (alpha via Qt.rgba on base colors) ----
    readonly property color hairline:     Qt.rgba(0.803, 0.839, 0.956, 0.08) // text @ 8%
    readonly property color borderSubtle: Qt.rgba(0.803, 0.839, 0.956, 0.12)
    readonly property color borderAccent: mauve
    readonly property color stateHover:   Qt.rgba(0.803, 0.839, 0.956, 0.05)
    readonly property color stateActive:  Qt.rgba(0.066, 0.066, 0.105, 0.35)
    readonly property color stateSelected: Qt.rgba(0.796, 0.651, 0.968, 0.16) // mauve @ 16%
    readonly property real  disabledOpacity: 0.38

    // ---- Glass (color + blur radius; blur via MultiEffect/ShaderEffect) ----
    readonly property color glassChromeBg:   Qt.rgba(0.094, 0.094, 0.145, 0.72) // mantle 72%
    readonly property color glassPanelBg:    Qt.rgba(0.066, 0.066, 0.105, 0.66) // crust 66%
    readonly property color glassElevatedBg: Qt.rgba(0.117, 0.117, 0.180, 0.78) // base 78%
    readonly property int   blurThin: 24
    readonly property int   blurBase: 32
    readonly property int   blurHeavy: 40

    // ---- Radius ----
    readonly property int radiusXs: 4
    readonly property int radiusSm: 6
    readonly property int radiusMd: 10
    readonly property int radiusLg: 14
    readonly property int radiusXl: 20
    readonly property int radius2xl: 28
    readonly property int radiusFull: 999

    // ---- Spacing (4pt) ----
    readonly property int space1: 4
    readonly property int space2: 8
    readonly property int space3: 12
    readonly property int space4: 16
    readonly property int space5: 20
    readonly property int space6: 24
    readonly property int space8: 32
    readonly property int space10: 40
    readonly property int space12: 48
    readonly property int space16: 64

    // ---- Typography ----
    readonly property string fontMono: "JetBrains Mono"
    readonly property string fontSans: "Inter"       // fallback resolved by fontconfig -> Noto Sans
    readonly property int fsDisplay: 28
    readonly property int fsH1: 22
    readonly property int fsH2: 18
    readonly property int fsH3: 15
    readonly property int fsBody: 13
    readonly property int fsSmall: 12
    readonly property int fsCaption: 11
    readonly property int fsMono: 13
    readonly property int fsMonoSm: 11

    // ---- Motion ----
    readonly property int durInstant: 90
    readonly property int durFast: 120
    readonly property int durBase: 200
    readonly property int durSlow: 320
    readonly property int durEntrance: 400
    // Easing: use Easing.BezierSpline with these control points, e.g.
    // standard:   [0.4,0.0, 0.2,1.0, 1,1]
    // decelerate: [0.0,0.0, 0.2,1.0, 1,1]
    // accelerate: [0.4,0.0, 1.0,1.0, 1,1]
    // emphasized: [0.2,0.9, 0.1,1.0, 1,1]
    readonly property var easeStandard:   [0.4, 0.0, 0.2, 1.0]
    readonly property var easeDecelerate: [0.0, 0.0, 0.2, 1.0]
    readonly property var easeAccelerate: [0.4, 0.0, 1.0, 1.0]
    readonly property var easeEmphasized: [0.2, 0.9, 0.1, 1.0]
}
```

### 12.3 Plasma — où vivent les tokens

- **Couleur** : déjà encodée dans [`vibeos-dark.colors`](../desktop/theme/vibeos-dark.colors) (`[Colors:*]`, `[WM]`, `DecorationFocus/Hover = Mauve`). Cible image : `/usr/share/color-schemes/VibeOSDark.colors`. **Ne pas dupliquer** la palette — le schéma est la source Plasma.
- **Panneau/dock** : le verre (`glass-chrome`) se pose via le preset **Panel Colorizer** (Phase 2) : fond Mantle + opacité 72 % + blur + accent Mauve. Preset JSON sous `desktop/`.
- **Rayon / espacement / mouvement** : hors du modèle de couleurs Plasma ; ils s'appliquent dans le **QML Quickshell** (§12.2) et le CSS SDDM/Plymouth (§12.1). Plasma garde les animations Breeze (durée standard), VibeOS n'y touche pas (DESKTOP §2.5).

---

## 13. Références & tendances 2026 (honnête)

Le design VibeOS s'inscrit dans quatre courants **maîtrisés**, pas suivis aveuglément :

- **Glassmorphism raffiné (post-visionOS).** Le verre *matériau* — flou réel + arête spéculaire + saturation légère + ombre profonde — plutôt que la transparence molle des années 2010. On l'emploie sur les surfaces flottantes, jamais sur le contenu. Références d'esprit : matériaux visionOS, panels Raycast, surfaces Arc.
- **Dark premium haute-couture.** L'école Linear / Vercel / Raycast : sombre, sobre, un seul accent, contrastes travaillés, typographie serrée. Le luxe est dans la *retenue*, pas dans l'effet. VibeOS pousse le curseur « calme » plus loin encore (vibecoding = focus long).
- **Gradient-mesh / aurora subtile.** Halos radiaux très basse saturation (Mauve→Blue) pour donner de la profondeur aux fonds (login, wallpaper Genesis) sans surcharger. Amplitude faible, jamais un fond « écran de veille ».
- **Motion doux & expressif (Material 3 expressive, spring léger).** Des courbes qui *décélèrent* à l'arrivée, un soupçon de ressort sur les grands gestes (`ease-emphasized`), et rien d'autre. Le mouvement informe.

Ce qu'on emprunte **et** ce qu'on refuse de ces tendances est explicite : la sophistication est un *choix de retenue*, pas une accumulation d'effets.

---

## 14. Anti-patterns (à proscrire)

- **Néon criard / cyberpunk.** Pas de cyan/magenta saturés à fond, pas de glow permanent, pas de « terminal de film ». VibeOS est premium et calme, pas une démo de synthwave.
- **Sur-animation.** Pas d'animation d'idle, pas de parallaxe décorative, pas de rebond, pas de « wobbly windows », pas de clignotement rapide. Chaque animation a une cause ; sinon elle dégage.
- **Contraste insuffisant.** Verre sur verre, texte muet (`Overlay1`) pour de l'information essentielle, gris sur gris : interdits. Corps de texte ≥ 4,5:1 (§15). Le verre ne doit jamais rendre un label illisible.
- **Dilution de l'accent.** Multiplier les couleurs d'accent (arc-en-ciel) tue la signature Mauve. Un accent, tenu.
- **Rouge décoratif.** Le rouge est réservé T3/erreur/suppression. Un rouge « joli » quelque part casse le langage des tiers.
- **Noir pur `#000`.** Halation, écrasement des ombres, fatigue. On utilise Crust `#11111b`.
- **Flou partout.** Le blur a un coût GPU et brouille les repères ; réservé aux surfaces flottantes, avec repli opaque obligatoire (§6.2).
- **Valeurs en dur.** Un `12px`, un `#cba6f7`, un `200ms` écrits à la main hors token : dette de cohérence, à refactorer.
- **Effet > lisibilité.** Toute décision qui rend « plus joli mais moins lisible/moins calme » est, par principe (§1), la mauvaise.

---

## 15. Accessibilité & contrôle qualité

- **Contraste texte** : corps ≥ 4,5:1, gros titres (≥ 18px/600) ≥ 3:1. `text-primary` sur toute surface `s0..s3` est AA/AAA. `text-muted` (Overlay1, ~3,9:1 sur Base) est **réservé au non-essentiel** (désactivé, hors-ligne, décor).
- **Focus toujours visible** : anneau Mauve 2px + `glow-focus` sur *tout* élément focalisable. Ne jamais `outline: none` sans remplacement.
- **Cibles tactiles/pointeur** : contrôles interactifs ≥ 32px (`space-8`) de hauteur ; zone cliquable ≥ 24px même si le glyphe est plus petit.
- **`prefers-reduced-motion`** : fondus sans translation, pulsations remplacées par un état statique visible (anneau plein plutôt que pulsé). Obligatoire (§8.3).
- **`prefers-reduced-transparency`** : repli sur surfaces opaques équivalentes (§6.2). Le verre n'est jamais requis pour comprendre.
- **Le tier se lit sans la couleur** : jamais *seulement* la couleur — toujours doublée d'une forme (pastille/anneau/cadenas) et d'un label mono (`T0..T3`), pour le daltonisme.
- **Checklist de revue design** (avant d'intégrer une surface) : (1) tokens uniquement, zéro valeur en dur ; (2) accent Mauve = focus/identité seulement ; (3) tiers = couleur + forme + label ; (4) contraste vérifié ; (5) focus visible ; (6) verre a un repli opaque ; (7) mouvement causé + reduced-motion géré ; (8) données non-live labellisées « Phase 2 ».

---

> **En un mot.** VibeOS n'est pas « un thème sombre de plus » : c'est un espace stratifié, calme et précis, où un accent mauve tenu, un verre maîtrisé, une typographie mono honnête et un mouvement mesuré racontent, du boot au HUD, la même histoire — celle d'un poste de commande pour diriger des agents en confiance. La sophistication est dans la discipline, pas dans l'ornement.
