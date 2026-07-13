# desktop/wallpapers/ — Fonds d'écran VibeOS

> Palette de référence : **« VibeOS Dark »** (fork Catppuccin Mocha, MIT) —
> Base `#1e1e2e`, Mantle `#181825`, Crust `#11111b`, accent **Mauve `#cba6f7`**,
> Lavender `#b4befe`, Blue `#89b4fa`.
> Langage visuel : [docs/DESIGN-SYSTEM.md](../../docs/DESIGN-SYSTEM.md) (§7 dégradés
> signature, §9 motif de l'anneau). Contexte bureau : [docs/DESKTOP.md](../../docs/DESKTOP.md)
> (§3 activités, §5.4 wallpapers & branding).

Trois **paquets wallpaper Plasma** (structure KPackage : `metadata.json` +
`contents/images/<LxH>.png` + `contents/screenshot.png`), copiés dans l'image
sous `/usr/share/wallpapers/` par `os/Containerfile`. Ce sont des **œuvres
originales VibeOS** — aucun asset tiers non redistribuable.

## Inventaire

| Paquet | Contenu | Rôle |
|---|---|---|
| [`VibeOS/`](VibeOS/) | Fond officiel (PNG 1536×1024) | **Défaut système** — câblé par le Global Theme `org.vibeos.dark` (`contents/defaults`, section `[Wallpaper] Image=VibeOS`) |
| [`VibeOSDark/`](VibeOSDark/) | **« Genesis »** — mesh aurora mauve→bleu sur fondu Base→Crust, anneaux de focus concentriques (rendu 3840×2160 depuis `src/vibeos-dark.svg`, dégradé `grad-mesh-genesis`) | Sélectionnable dans le picker (activités **Vibe**, **Review**) |
| [`VibeOSVoid/`](VibeOSVoid/) | **« Void »** — champ quasi-crust, une seule respiration mauve, un unique anneau ouvert (rendu 3840×2160 depuis `src/vibeos-void.svg`, dégradé `grad-void`) | Sélectionnable dans le picker (activité **Focus**, mode amnésique — [MEMORY.md](../../docs/MEMORY.md)) |

Plasma affiche les paquets en *scale-and-crop* : les résolutions différentes
de l'écran sont recadrées proprement.

## Sources vectorielles (`src/`)

`src/vibeos-dark.svg` et `src/vibeos-void.svg` sont **autonomes et
parseables** : aucune référence externe, aucune police, aucun script, viewBox
16:9 (3840×2160), couche de grain fin (feTurbulence) contre le banding. Les
SVG ne sont **pas** copiés dans l'image : le plugin wallpaper de Plasma
attend des rasters dans `contents/images/`.

Re-rasterisation après modification d'un SVG (cairosvg : `pip install cairosvg`) :

```bash
# depuis desktop/wallpapers/
python3 -c "import cairosvg; cairosvg.svg2png(url='src/vibeos-dark.svg', \
  write_to='VibeOSDark/contents/images/3840x2160.png', \
  output_width=3840, output_height=2160)"
```

Regénérer aussi `contents/screenshot.png` (vignette ≤ 400 px du picker).

## Changer le défaut

Le défaut out-of-box vit dans
`desktop/look-and-feel/org.vibeos.dark/contents/defaults` (`[Wallpaper]
Image=<IdDuPaquet>`) — surchargeable librement par l'utilisateur dans
Paramètres du Système, comme tout réglage posé par un Global Theme.
