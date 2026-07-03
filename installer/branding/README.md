# installer/branding/ — Inventaire des assets d'identité visuelle

> Palette de référence : thème **« VibeOS Dark »**, fork de Catppuccin Mocha (MIT) —
> base `#1e1e2e`, surface `#313244`, texte `#cdd6f4`, **accent mauve `#cba6f7`**,
> lavande `#b4befe`.
> « Comment intégrer » : suivre le blueprint **uBlue/Bazzite**
> (`ublue-os/image-template`, dépôt Bazzite — [docs/ECOSYSTEM.md](../../docs/ECOSYSTEM.md), niveau 1) :
> assets copiés dans l'image par le `Containerfile` (chantier `os/`, séparé —
> ce dossier fournit les **sources** et documente les **emplacements cibles**,
> il ne modifie jamais le Containerfile), thème Plymouth activé au build de
> l'image, SDDM configuré via `/etc/sddm.conf.d/`.

Rappel immuabilité : tout asset livré vit sous **`/usr`** (contenu d'image,
posé au build) ou **`/etc`** (défauts) — jamais écrit à l'exécution.

---

## Inventaire

| # | Asset | Format / spec | Résolutions | Emplacement cible dans l'image | Phase | Statut |
|---|---|---|---|---|---|---|
| 1 | **Logo VibeOS** (monogramme « V », source vectorielle) | SVG 1.1 autonome, aucune référence externe — source : [`vibeos-logo.svg`](vibeos-logo.svg) | vectoriel (viewBox 256×256) | `/usr/share/pixmaps/vibeos/vibeos-logo.svg` | 1 | ✅ **livré** (ce dossier) |
| 1b | Rendus PNG du logo (icônes hicolor) | PNG-32 (alpha), rendus depuis le SVG (`rsvg-convert`/Inkscape) | 16, 22, 32, 48, 64, 128, 256, 512 px | `/usr/share/icons/hicolor/<taille>x<taille>/apps/vibeos.png` | 1 | 🔲 à créer |
| 2 | **Wallpaper « VibeOS Dark »** (défaut Plasma) | Paquet wallpaper KDE : `contents/images/<WxH>.png` + `metadata.json` ; motif géométrique sombre sur base `#1e1e2e`, accent mauve — œuvre **originale** (redistribuable) | 3840×2160 (min), 2560×1440, 1920×1080 ; variantes verticales optionnelles | `/usr/share/wallpapers/VibeOSDark/` (+ réglage par défaut via le global theme du chantier bureau) | 1 | 🔲 à créer |
| 3 | **Thème Plymouth** (splash de boot) | Thème two-step ou script : `vibeos.plymouth` + assets PNG (logo animé, spinner, watermark) ; **création originale** — le thème adi1090x est rejeté (provenance d'assets floue, [ECOSYSTEM.md](../../docs/ECOSYSTEM.md)) ; activation : `plymouth-set-default-theme vibeos` au build de l'image (référence pour le chantier `os/`) | logo ≈ 256×256 px, watermark ≈ 200 px, fond couleur unie `#1e1e2e` (indépendant de la résolution) | `/usr/share/plymouth/themes/vibeos/` | 5 | 🔲 à créer |
| 4 | **Thème SDDM** (écran de connexion) | v0.1 : Breeze + fond VibeOS via `/etc/sddm.conf.d/` (config seulement) ; Phase 5 : thème QML complet (base **SDDM Astronaut**, GPL-3.0 — licence à re-vérifier avant fork) : `Main.qml`, `theme.conf`, assets | fond 3840×2160 (réutiliser l'asset n°2), avatar par défaut 256×256 | `/usr/share/sddm/themes/vibeos/` + défauts `/etc/sddm.conf.d/10-vibeos.conf` | 1 (fond) / 5 (thème) | 🔲 à créer |
| 5 | **Branding Anaconda** (installateur) | Remplacement des pixmaps de l'environnement d'installation (logo latéral, en-tête) — vecteur d'injection : paquet logos custom ou `product.img` ajouté à l'ISO (méthode Bazzite/uBlue) ; l'inventaire exact des fichiers est à confirmer contre la version d'Anaconda embarquée par bootc-image-builder au moment de l'intégration | `sidebar-logo.png` ≈ 180×60 (rendu depuis le SVG n°1), en-tête/topbar aux dimensions du thème Anaconda courant | `/usr/share/anaconda/pixmaps/` (dans l'environnement d'installation de l'ISO, pas dans l'image système) | 1 (logo minimal, si trivial) / 5 (thème complet) | 🔲 à créer |

---

## Règles de production

1. **Originalité & licence** : chaque asset est soit une création originale du
   projet, soit un dérivé d'une base explicitement redistribuable
   (Catppuccin = MIT). Aucun asset à provenance douteuse n'entre dans l'ISO —
   c'est la raison du rejet des thèmes Plymouth adi1090x.
2. **Source vectorielle d'abord** : tout PNG livré doit avoir sa source (SVG,
   fichier de projet) versionnée dans ce dossier — les rendus se régénèrent,
   ils ne se retouchent pas.
3. **Cohérence** : un seul système visuel — palette VibeOS Dark, monogramme
   « V » identique du splash Plymouth au HUD Quickshell.
4. **Sobriété au boot** : Plymouth = fond uni + logo + spinner. Pas
   d'animation lourde : le boot doit rester rapide et lisible.
5. **Honnêteté (D20)** : le statut ci-dessus fait foi. Seule la source SVG du
   logo est livrée aujourd'hui ; tout le reste est **à créer** — ne jamais
   présenter un thème Plymouth/SDDM/Anaconda comme existant tant que la case
   n'est pas passée à ✅.

## Pipeline de rendu (référence)

```bash
# PNG icons from the SVG source (run in a toolbox/distrobox, never on /usr):
for s in 16 22 32 48 64 128 256 512; do
  rsvg-convert -w "$s" -h "$s" vibeos-logo.svg -o "vibeos-${s}.png"
done
```

L'intégration dans l'image (COPY vers `/usr/share/...`, activation Plymouth,
conf SDDM) relève du `Containerfile` — chantier `os/`, à référencer, pas à
éditer depuis ici.
