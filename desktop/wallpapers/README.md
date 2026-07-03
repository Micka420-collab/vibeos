# desktop/wallpapers/ — Fonds d'écran VibeOS (sources originales)

> Palette de référence : **« VibeOS Dark »** (fork Catppuccin Mocha, MIT) —
> Base `#1e1e2e`, Mantle `#181825`, Crust `#11111b`, accent **Mauve `#cba6f7`**,
> Lavender `#b4befe`, Blue `#89b4fa`.
> Langage visuel : [docs/DESIGN-SYSTEM.md](../../docs/DESIGN-SYSTEM.md) (§7 dégradés
> signature, §9 motif de l'anneau). Contexte bureau : [docs/DESKTOP.md](../../docs/DESKTOP.md)
> (§3 activités, §5.4 wallpapers & branding).

Ce dossier fournit les **sources vectorielles** des fonds d'écran. Ce sont des
**œuvres originales VibeOS** — aucun asset tiers non redistribuable. Elles sont
copiées dans l'image au build (chantier `os/`, non édité depuis ici) ; à
l'exécution l'OS est immuable et rien n'est écrit sous `/usr`.

---

## Inventaire

| Fichier | Rôle | Activité(s) cible(s) | Dégradé source (§7) |
|---|---|---|---|
| [`vibeos-dark.svg`](vibeos-dark.svg) | **« Genesis »** — mesh aurora mauve→bleu sur fondu Base→Crust, anneaux de focus concentriques. Chaleureux, invitant, « profondeur & focus ». | **Vibe** (défaut), **Review** | `grad-mesh-genesis` |
| [`vibeos-void.svg`](vibeos-void.svg) | **« Void »** — champ quasi-crust, une seule respiration mauve, un unique anneau ouvert. Ardoise vierge, mode amnésique, sans distraction. | **Focus** (+ mode amnésique, [MEMORY.md](../../docs/MEMORY.md)) | `grad-void` |
| [`metadata.json`](metadata.json) | Métadonnées du paquet wallpaper KDE (KPackage / Plasma 6) : `Id = VibeOSDark`, auteur, licence. | — | — |

Les deux fonds sont des SVG **autonomes et parseables** : aucune référence
externe, aucune police, aucun script, viewBox 16:9 (3840×2160). Ils se
rasterisent sans banding grâce à une couche de grain fin (feTurbulence), et se
déclinent en PNG multi-résolutions au build (voir *Pipeline de rendu*).

---

## Emplacements cibles dans l'image (`/usr`, lecture seule)

Structure d'un paquet wallpaper KDE (Plasma 6, `Image` plugin) :

```
/usr/share/wallpapers/VibeOSDark/
├── metadata.json                       # source : desktop/wallpapers/metadata.json
└── contents/
    └── images/
        ├── 3840x2160.png               # rendu de vibeos-dark.svg (Genesis, défaut)
        ├── 2560x1440.png
        └── 1920x1080.png
```

Le fond **Void** peut être livré soit comme second paquet
(`/usr/share/wallpapers/VibeOSVoid/`, même schéma), soit comme image
additionnelle dans `contents/images/` sélectionnée par l'activité **Focus**.
Le choix d'empaquetage final relève de la configuration du Global Theme
(livrable **🛣️ Phase 2** / branding **🛣️ Phase 5**, cf. DESKTOP §2.6, §5.4) —
les **sources** vivent ici, leur activation dans l'image se fait via le
`Containerfile` (chantier `os/`, référencé, non édité ici).

> **Rappel immuabilité.** Les chemins `/usr/share/wallpapers/...` sont posés au
> build de l'image. Ce dossier est la **source** ; à l'exécution seuls `/etc`
> et `$HOME` sont modifiables.

---

## Cohérence avec le langage de design

- **Accent unique.** Un seul Mauve `#cba6f7`, tenu — jamais un arc-en-ciel
  d'accents (DESIGN-SYSTEM §1.4). Le mesh ne dépasse pas 2–3 teintes voisines
  (Mauve↔Lavender↔Blue).
- **Aurora très basse saturation.** Halos radiaux doux, jamais un « écran de
  veille » (DESIGN-SYSTEM §13, §14). Le mouvement est absent : un wallpaper est
  calme par définition.
- **Motif de l'anneau.** Les anneaux de focus (Genesis) et l'anneau unique
  (Void) reprennent la forme signature partagée avec le login, le HUD et le
  logo (DESIGN-SYSTEM §9) : une identité reconnaissable du boot au bureau.
- **Jamais de noir pur.** Le point le plus profond est Crust `#11111b` ; les
  vignettes déscendent à peine en dessous (`#0c0c14`/`#0a0a11`) pour la
  profondeur, sans `#000` (DESIGN-SYSTEM §14).

---

## Pipeline de rendu (référence — à exécuter en toolbox/distrobox, jamais sur `/usr`)

```bash
# Multi-resolution PNGs from each SVG source (librsvg / rsvg-convert):
for res in 3840x2160 2560x1440 1920x1080; do
  w="${res%x*}"; h="${res#*x}"
  rsvg-convert -w "$w" -h "$h" vibeos-dark.svg -o "genesis-${res}.png"
  rsvg-convert -w "$w" -h "$h" vibeos-void.svg -o "void-${res}.png"
done
```

Les rendus se **régénèrent** depuis le SVG, ils ne se retouchent pas
(DESIGN-SYSTEM : cohérence par la source). L'intégration dans l'image
(`COPY` vers `/usr/share/wallpapers/VibeOSDark/`, réglage du wallpaper par
défaut du Global Theme) relève du chantier `os/` — référencé, non édité ici.

---

## Licence

Œuvres **originales VibeOS**, licence **Apache-2.0** (licence du dépôt). La
palette dérive de Catppuccin Mocha (MIT) — l'attribution Catppuccin est
conservée dans [desktop/theme/palette.md](../theme/palette.md). Aucun asset à
provenance douteuse n'entre dans l'image (règle d'originalité, cf.
[installer/branding/README.md](../../installer/branding/README.md)).
