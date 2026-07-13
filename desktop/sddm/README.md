# desktop/sddm/ — thème de connexion VibeOS (SDDM)

Ce dossier contient le **thème SDDM « VibeOS »** : l'écran de connexion (greeter) qui s'affiche entre le boot (Plymouth) et la session Plasma 6. Il matérialise, *avant même d'être connecté*, l'identité VibeOS — carte de verre sombre, aurore mauve→blue, anneau signature, horloge mono.

> **Statut : copié dans l'image, non actif par défaut — activation 🛣️ Phase 5 (branding).** Le thème est **copié au build** sous `/usr/share/sddm/themes/vibeos/` (sélectionnable dans Réglages système ▸ Écran de connexion) ; le greeter **par défaut** reste **Breeze** (voir [docs/DESKTOP.md](../../docs/DESKTOP.md) §4). Il ne devient l'écran de connexion par défaut qu'une fois **activé en Phase 5** (voir plus bas). Rien ici ne modifie le greeter par défaut.

Référence de design : [docs/DESIGN-SYSTEM.md](../../docs/DESIGN-SYSTEM.md) **§11.2 (Login)**. Palette : [desktop/theme/palette.md](../theme/palette.md).

---

## Rôle et parti pris

L'écran de connexion applique la **règle d'or § 11.2** : *on reconnaît VibeOS avant d'être connecté*. Concrètement :

- **Fond** : l'aurore « Genesis » (`grad-mesh-genesis`, §7) — halo mauve→blue très basse saturation en haut-droite, fondu Base→Crust — **peinte procéduralement** dans un `Canvas` (aucun wallpaper bitmap à livrer, aucun module de flou requis).
- **Carte de connexion** : recette `glass-elevated` (§6.1) — Base à 78 % posée sur l'aurore, `radius-xl` (20px), arête spéculaire haute, élévation profonde. Centrée, largeur contenue.
- **Accent Mauve = focus uniquement** (§4) : anneau + halo de focus sur les champs, bouton primaire en aplat Mauve avec texte `text-on-accent` (Crust).
- **Anneau signature** (`grad-signature`, §7) : le motif de marque — arc ouvert du wordmark, anneau plein de l'avatar — le **même** motif que l'anneau d'agent du HUD et l'anneau du splash Plymouth.
- **Vérité machine en mono** (§3) : l'heure et la date sont en **JetBrains Mono**.
- **Mouvement mesuré** (§8) : une entrée douce (fade + 8px, `duration-entrance`/`ease-decelerate`), une sortie rapide au succès (`ease-accelerate`). Aucun rebond, aucun clignotement.

---

## Contenu du dossier

| Fichier | Rôle |
|---|---|
| [`vibeos/Main.qml`](vibeos/Main.qml) | Le thème lui-même (Qt6/QML) : aurore, carte de verre, champs utilisateur/mot de passe, sélecteur de session, actions d'alimentation, horloge, gestion `loginFailed`/`loginSucceeded`, avertissement Verr. Maj. |
| [`vibeos/theme.conf`](vibeos/theme.conf) | Configuration exposée à `Main.qml` via l'objet `config` (fond optionnel, polices). Toutes les clés ont un repli sûr. |
| [`vibeos/metadata.desktop`](vibeos/metadata.desktop) | Métadonnées SDDM (`MainScript`, `ConfigFile`, `QtVersion=6`, `Screenshot`). |

### Assets à générer (non binaires dans ce dépôt)

- **`vibeos/preview.png`** — capture 1x du rendu de `Main.qml`, utilisée par « Réglages système → Écran de connexion (SDDM) » pour la vignette de la galerie. À produire au moment du branding (Phase 5), p. ex. via `sddm-greeter-qt6 --test-mode --theme .` puis capture.

---

## Cible dans l'image (OS immuable)

VibeOS est immuable (bootc/OSTree) : **rien n'est écrit dans `/usr` à l'exécution**. Les sources de ce dossier sont copiées **au build de l'image** :

```
desktop/sddm/vibeos/*   →   /usr/share/sddm/themes/vibeos/*
```

La copie est effectuée par le **chantier os** (référencé ici en commentaire d'en-tête de chaque fichier — on n'édite pas `os/Containerfile` depuis ce dossier). Le paquet requis est `sddm` (déjà présent, Kinoite hérite de SDDM ; version Qt6 ≥ 0.20).

---

## Activation (🛣️ Phase 5)

Le thème n'est pas activé tant que la Phase 5 ne pose pas la configuration SDDM correspondante. Sur un OS immuable, cela se fait **au build**, via un drop-in livré sous `/usr/lib/sddm/sddm.conf.d/` (ou `/etc/sddm.conf.d/`) :

```ini
# /usr/lib/sddm/sddm.conf.d/10-vibeos.conf   (posé au build — Phase 5)
[Theme]
Current=vibeos
```

Test hors-image pendant le développement (poste de dev, non immuable) :

```sh
# depuis desktop/sddm/vibeos/
sddm-greeter-qt6 --test-mode --theme .
```

---

## Honnêteté (invariant projet)

- **Copié dans l'image, non actif par défaut.** Le thème est livré sous `/usr/share/sddm/themes/vibeos/` (sélectionnable), mais le greeter par défaut reste Breeze tant que la Phase 5 ne pose pas le drop-in SDDM d'activation (ci-dessus).
- **Police `Inter` = aspirationnelle.** `Main.qml` demande `font-sans` = Inter ; tant qu'Inter n'est pas packagé dans l'image, fontconfig substitue **Noto Sans** (§3.1). La pile ne « casse » jamais.
- **Pas de vrai flou.** Un greeter ne doit **jamais** échouer à se charger : seuls les modules Qt6 toujours présents sont importés (`QtQuick`, `QtQuick.Controls.Basic`, `QtQuick.Layouts`). Le « verre » est donc honnêtement **approché** — remplissage translucide sur notre propre aurore (pas des fenêtres vives) + arête spéculaire ; l'élévation est simulée par des sous-couches translucides. Un vrai flou d'arrière-plan exigerait `QtQuick.Effects` / le compositeur, dépendances écartées ici par robustesse.
- **Tokens en dur = choix documenté.** SDDM n'a pas de singleton de thème à importer : chaque hex de `Main.qml` renvoie à un token de [DESIGN-SYSTEM.md](../../docs/DESIGN-SYSTEM.md §12), synchronisé à la main.
- **Rouge = erreur seulement** (§10.3) : la ligne d'erreur d'échec de connexion est en Red ; c'est le seul rouge de l'écran. Mauve n'y est jamais un danger.

---

## Licences

Thème original VibeOS : **Apache-2.0** (licence du dépôt). Ne réutilise aucun asset tiers non redistribuable. La palette dérive de Catppuccin Mocha (MIT, attribution conservée dans [palette.md](../theme/palette.md)). SDDM : GPL-2.0+ (utilisé tel quel, non modifié).
