# desktop/plymouth/ — splash de démarrage VibeOS (Plymouth)

Ce dossier contient le **thème Plymouth « VibeOS »** : l'écran de boot affiché pendant le démarrage du noyau et de l'initramfs, avant le greeter SDDM. C'est **le premier accord de la partition** : minimal, mais déjà dans la tonalité VibeOS — fond Crust, cœur qui respire, spirale signature mauve→blue (la galaxie VibeOS).

> **Statut : ✅ ACTIVÉ (2026-07-23, à la demande du mainteneur) — c'est le splash de boot par défaut.** Le thème est copié au build sous `/usr/share/plymouth/themes/vibeos/` **et** posé en défaut par `os/Containerfile` (`plymouth-set-default-theme vibeos` **avec régénération de l'initramfs au build**, étape spécifique à un OS immuable — voir plus bas). Le splash Fedora par défaut est ainsi remplacé. Les trois PNG (`mark`, `ring`, `wordmark`) sont **générés** par [`generate-assets.py`](generate-assets.py) (œuvre originale, reproductible) ; s'ils manquent, le splash retombe sur le simple fond dégradé (dégradation gracieuse réelle).

Référence de design : [docs/DESIGN-SYSTEM.md](../../docs/DESIGN-SYSTEM.md) **§11.1 (Boot)**. Palette : [desktop/theme/palette.md](../theme/palette.md).

---

## Rôle et parti pris

Le splash applique la **règle d'or § 11.1** : *le même Crust, le même dégradé signature qu'au login*. Le boot est volontairement plus dépouillé que l'écran de connexion, mais jamais en contradiction.

- **Fond** : Crust (`grad-void`, §7) — dégradé vertical quasi imperceptible `#16161f → #11111b`. **Jamais `#000` pur** (§14).
- **Cœur central qui respire** : un point lumineux à halo doux, opacité oscillant lentement (~2,4 s, 0,72 ↔ 1,0). Mouvement **calme et causal** (§8.3), jamais un clignotement.
- **Spirale signature** (`grad-signature`, mauve→blue) : la galaxie VibeOS en rotation lente, accélérée subtilement par la progression du boot — elle « avance » sans barre de pourcentage criarde. C'est le **même motif** que les wallpapers et le badge de la marque.
- **Prompt de déverrouillage LUKS** : rendu sobre (`text-tertiary` pour l'invite, `text-primary` pour les puces).
- **Messages de boot** (fsck, services) : ligne discrète en `text-muted`, en bas d'écran — non essentiel (§15).

Le module utilisé est **`script`** : toute l'animation tient dans [`vibeos/vibeos.script`](vibeos/vibeos.script), lisible et auditable, sans plugin C.

---

## Contenu du dossier

| Fichier | Rôle |
|---|---|
| [`vibeos/vibeos.plymouth`](vibeos/vibeos.plymouth) | Descripteur de thème (`ModuleName=script`, `ImageDir`, `ScriptFile`). |
| [`vibeos/vibeos.script`](vibeos/vibeos.script) | Le splash : fond, marque qui respire, anneau en rotation, progression, prompt LUKS, messages. |

### Assets (générés par `generate-assets.py`)

Le script charge trois PNG depuis `ImageDir`, fond **transparent**, produits par [`generate-assets.py`](generate-assets.py) (Pillow) — **œuvre originale VibeOS, reproductible** : les PNG sont commités, le générateur en donne la provenance (aucune source tierce). Regénérer : `python desktop/plymouth/generate-assets.py`.

| Fichier | Spéc. | Couleur | Rôle |
|---|---|---|---|
| **`vibeos/mark.png`** | cœur galactique, 128 px : point lumineux + halo doux | monochrome **Text `#cdd6f4`** | Cœur central (respire) : le noyau calme au centre de la spirale. |
| **`vibeos/ring.png`** | spirale galactique 2 bras, 256 px, avec glow | **dégradé `grad-signature`** mauve `#cba6f7` → blue `#89b4fa` | Spirale signature (rotation lente). Le dégradé donne le « balayage » de la galaxie en tournant — même motif que les wallpapers. |
| **`vibeos/wordmark.png`** | mot « VibeOS », 320 px de large | Text `#cdd6f4`, posé à 55 % d'opacité | Wordmark discret sous la marque. **Optionnel** : le script se garde si l'asset est absent. |

> **Dégradation gracieuse** : si un PNG manque, `GetWidth()` vaut 0 et le sprite reste invisible — le splash retombe sur le simple fond dégradé au lieu de planter. La règle d'honnêteté, appliquée au boot.
> **Rendu visuel non validé en réel** : les assets ont été prévisualisés hors-boot (compositing) mais le splash Plymouth **animé** n'a jamais été rendu sur une machine bootée (pas de boot ici — machine-gated). Le thème est désormais **activé** ; sa validation visuelle réelle se fait au prochain boot d'une image reconstruite (côté mainteneur).
> **Puces LUKS** : les puces du mot de passe sont rendues en texte (`•`) via la police de l'initramfs. Un asset `bullet.png` dédié pourra les remplacer par des points nets si besoin (évolution mineure).

---

## Cible dans l'image (OS immuable)

VibeOS est immuable (bootc/OSTree) : **rien n'est écrit dans `/usr` à l'exécution**. Les sources de ce dossier sont copiées **au build de l'image** :

```
desktop/plymouth/vibeos/*   →   /usr/share/plymouth/themes/vibeos/*
```

La copie est effectuée par le **chantier os** (référencé en commentaire d'en-tête de chaque fichier — on n'édite pas `os/Containerfile` depuis ce dossier).

**Paquets requis — installés (2026-07-15)** : `plymouth-plugin-script` (le thème déclare `ModuleName=script` ; ce module vit dans ce sous-paquet, et le thème de boot par défaut de Fedora ne l'utilise pas, donc **rien d'autre ne le tire**) et `plymouth-plugin-label` (rendu de `Image.Text()` : invite LUKS, messages de boot). Sans eux le thème n'aurait pas pu se rendre une fois activé — Plymouth serait retombé sur le thème par défaut. L'activation elle-même (`plymouth-set-default-theme` + régénération de l'initramfs) est désormais posée dans `os/Containerfile` (ci-dessous).

---

## Activation (✅ posée le 2026-07-23 dans `os/Containerfile`)

Sur un OS immuable, activer un thème Plymouth **implique de régénérer l'initramfs** (le thème y est embarqué), ce qui se fait **au build de l'image**, pas à l'exécution. C'est désormais fait — section « 7ter. Boot identity » du `os/Containerfile` :

```sh
# exécuté DANS le build de l'image, après copie des fichiers :
plymouth-set-default-theme vibeos      # pose le thème par défaut
# puis, par noyau, l'invocation bootc/OSTree éprouvée (BlueBuild/uBlue) :
dracut --force --kver "$KVER" --add ostree --no-hostonly --reproducible \
    /usr/lib/modules/$KVER/initramfs.img
# Une VÉRIFICATION au build (lsinitrd) échoue le build si le thème VibeOS — ou
# le module ostree, ou (amd64) les modules nvidia — manque de l'initramfs :
# un initramfs cassé = build rouge, jamais une image livrée qui ne boote pas.
```

Aperçu hors-image pendant le développement (poste de dev, non immuable) :

```sh
plymouthd --debug --tty=/dev/tty1 ; plymouth --show-splash   # puis plymouth quit
# ou l'utilitaire de prévisualisation de la distribution.
```

---

## Honnêteté (invariant projet)

- **Actif par défaut, avec repli sûr.** Le thème est posé en défaut au build (régénération initramfs incluse, vérifiée) ; les trois PNG sont fournis (générés par `generate-assets.py`) et le script se dégrade proprement s'ils manquent (fond seul, jamais d'erreur).
- **Œuvre originale.** Marque, anneau et mouvement sont dessinés pour VibeOS. Le pack adi1090x est **explicitement écarté** pour provenance d'assets floue ([docs/DESKTOP.md](../../docs/DESKTOP.md) §5.4) : aucun asset tiers non redistribuable n'entre ici.
- **Cohérence de tonalité** : le Crust et le dégradé signature du splash sont **identiques** à ceux de l'écran SDDM (§11.2) et du HUD — boot → login → bureau racontent la même histoire.
- **Dégradation gracieuse** : assets manquants ⇒ fond seul, jamais d'erreur.

---

## Licences

Thème original VibeOS : **Apache-2.0** (licence du dépôt). La palette dérive de Catppuccin Mocha (MIT, attribution conservée dans [palette.md](../theme/palette.md)). Plymouth : GPL-2.0+ (utilisé tel quel, non modifié).
