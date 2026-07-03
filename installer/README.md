# installer/ — Artefacts d'installation de VibeOS

> Spécification de référence : [`docs/INSTALLER.md`](../docs/INSTALLER.md).
> Génération et test de l'ISO : [`docs/BUILD.md`](../docs/BUILD.md) §4–5.

Ce dossier regroupe tout ce qui concerne **l'installation** de VibeOS et
l'habillage de l'installateur — et rien d'autre : le premier démarrage
(Genesis, mémoire) appartient à `memory/`, l'image elle-même à `os/`.

## Contenu

| Entrée | Rôle | Statut |
|---|---|---|
| [`vibeos.ks`](vibeos.ks) | Kickstart Anaconda de **référence** pour déployer l'image bootc `ghcr.io/micka420-collab/vibeos` : partitionnement par défaut (UEFI, Btrfs, LUKS-ready), `ostreecontainer`, `%post` minimal. Fortement commenté ; les parties non livrées sont marquées `[Phase N]`. | ✅ v0.1 |
| [`branding/`](branding/README.md) | Inventaire et sources des assets d'identité visuelle (logo, wallpaper, Plymouth, SDDM, Anaconda) avec formats, résolutions et emplacements cibles dans l'image. | 🟡 logo livré, reste à créer |

## Comment ces fichiers sont utilisés

1. **ISO officielle (CI)** : `bootc-image-builder --type iso` embarque l'image
   OCI et génère son propre kickstart (installation hors ligne). Le
   `config.toml` de build peut injecter des fragments de kickstart via
   `[customizations.installer.kickstart]` — c'est le vecteur prévu pour
   raccorder `vibeos.ks` à l'ISO **[Phase 5]**.
2. **Installation réseau / labo** : `vibeos.ks` s'utilise tel quel avec un
   média de boot Fedora (`inst.ks=https://…/vibeos.ks`) — l'image est alors
   tirée depuis ghcr.io.
3. **Branding** : les assets de `branding/` ont vocation à être copiés dans
   l'image par `os/Containerfile` (chantier séparé — ce dossier fournit les
   sources et documente les emplacements cibles, il ne modifie pas le
   Containerfile).

## Périmètre par phase (résumé)

- **Phase 1 (v0.1)** : ISO brute bootc-image-builder, Anaconda stock,
  kickstart de référence, layout disque LUKS-ready, création utilisateur.
- **Phase 3** : volume LUKS2 `vibeos-memory`, entrée de boot amnésique
  (`vibeos.amnesic=1`), interview de naissance au premier boot.
- **Phase 5** : installateur guidé « vibecoding onboarding » complet,
  chiffrement disque par défaut, choix du mode mémoire à l'installation,
  thème graphique VibeOS Dark de bout en bout.

Détail complet, parcours cible et diagramme : [`docs/INSTALLER.md`](../docs/INSTALLER.md).
