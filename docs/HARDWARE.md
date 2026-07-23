# Matériel — cibles et machine de référence

VibeOS est **multi-architecture** : chaque release publie des images `amd64` (x86_64) et `arm64` (aarch64) sous un manifest OCI unique (`ghcr.io/micka420-collab/vibeos`), plus une ISO par architecture.

## Machine de référence n°1 (amd64) — PC du mainteneur

Relevé du 2026-07-03. C'est sur cette machine que VibeOS sera flashé ; toute release doit être validée dessus (d'abord en VM Hyper-V, puis sur SSD dédié en dual-boot, puis bascule complète).

| Composant | Référence | Implications pour VibeOS |
|---|---|---|
| CPU | AMD Ryzen 7 3700X — 8 cœurs / 16 threads, x86_64-v3 | Cible `amd64`. Compilations et inférence CPU confortables. |
| RAM | 16 Go | Modèles locaux 7B–13B quantisés OK via ollama ; garder le bureau sobre. |
| GPU | **NVIDIA GeForce RTX 3070 Ti (8 Go VRAM)** | Driver propriétaire **obligatoire dans l'image** : `akmod-nvidia` + `xorg-x11-drv-nvidia-cuda` (RPM Fusion), technique éprouvée par Bazzite. Débloque CUDA pour ollama (inférence locale GPU). **Seul chemin d'affichage** : le Ryzen 3700X n'a pas d'iGPU, donc si le driver NVIDIA ne monte pas → **écran noir** (pas de repli). Kernel args requis (voir ci-dessous). Secure Boot : modules signés par notre clé MOK (Phase 4). |
| Stockage | NVMe Kingston SA2000 1 To + SSD SATA 512 Go / 256 Go / 120 Go | Cible d'installation recommandée : un SSD dédié (dual-boot sans risque) avant bascule complète sur le NVMe. |
| Firmware | UEFI (GPT), machine sous Windows 11 aujourd'hui | Compatible Secure Boot + TPM → chaîne de boot mesurée prévue Phase 4, déverrouillage LUKS par TPM2 possible. |
| Virtualisation | Hyper-V actif | Banc de test VM local pour chaque ISO avant tout flash. |

### Affichage NVIDIA + Wayland : arguments noyau (correctif écran noir)

**Symptôme observé au premier boot réel (2026-07-23) : écran entièrement noir.**
Cause : sur cette machine le RTX 3070 Ti est le **seul** chemin d'affichage (pas
d'iGPU), et VibeOS est **Wayland-only** (Plasma 6). Le compositeur Wayland exige
le *kernel mode setting* NVIDIA (`nvidia-drm.modeset=1`) ; sans lui, SDDM/Plasma
n'a pas de périphérique DRM utilisable et le greeter ne s'affiche jamais. Il faut
aussi tenir `nouveau` à l'écart pour que le driver propriétaire prenne la carte.

Args noyau appliqués par défaut sur `x86_64` :

```
nvidia-drm.modeset=1  rd.driver.blacklist=nouveau  modprobe.blacklist=nouveau
```

Ils sont posés à **deux** endroits (gardés synchronisés) :
- `os/rootfs/usr/lib/bootc/kargs.d/10-vibeos-nvidia.toml` — mécanisme **canonique
  bootc** : appliqué à `bootc install`, persistant à chaque `bootc upgrade`,
  indépendant de la méthode d'installation ;
- `installer/vibeos.ks` (ligne `bootloader --append=`) — car le chemin
  d'installation **Anaconda** de l'ISO ne consulte pas `kargs.d`.

En complément, les modules `nvidia` sont embarqués dans l'initramfs
(`/usr/lib/dracut/dracut.conf.d/99-vibeos-nvidia.conf` + régénération au build)
pour un KMS précoce (splash Plymouth sur le GPU, transition propre vers Wayland).

> **⚠️ Secure Boot doit être DÉSACTIVÉ** tant que la signature MOK de nos kmods
> n'est pas livrée (**Phase 4**). Le module `akmod-nvidia` de l'image n'est pas
> signé : avec Secure Boot **activé**, le noyau le refuse et l'écran reste noir
> **malgré** ces args. Ordre de bataille sur le PC de référence : désactiver
> Secure Boot dans l'UEFI, booter, valider ; la ré-activation attend la Phase 4.

### Checklist de validation sur cette machine

- [ ] ISO amd64 boote en VM Hyper-V (Gén. 2, Secure Boot désactivé d'abord, puis activé)
- [ ] **Secure Boot désactivé dans l'UEFI** (obligatoire jusqu'à la signature MOK, Phase 4)
- [ ] **Boot graphique jusqu'au greeter SDDM VibeOS** (plus d'écran noir — args NVIDIA ci-dessus)
- [ ] Driver NVIDIA chargé (`nvidia-smi` fonctionnel) sur matériel réel
- [ ] `ollama` utilise le GPU (offload CUDA vérifié)
- [ ] KDE Plasma fluide en Wayland avec le driver propriétaire
- [ ] Splash de boot **VibeOS** (Plymouth) et non le logo Fedora
- [ ] Installation sur SSD dédié en dual-boot avec Windows 11 (GRUB/systemd-boot n'écrase pas le boot Windows)
- [ ] `vibeos-genesis.service` s'exécute au premier boot (mémoire créée, `.initialized` présent)
- [ ] Le greeter nomme le **citoyen IA** né sur la machine (`/run/vibeos/citizen.json`)

## Cible arm64 (aarch64)

Pas encore de machine de référence physique. Cibles visées :

- **Raspberry Pi 5** (8 Go) — première cible arm64 de test (attention : boot Pi = cas particulier, pas d'UEFI standard ; passera par les images bootc aarch64 de Fedora + firmware UEFI Pi).
- Machines ARM UEFI standard (serveurs Ampere, VM `arm64` — dont Apple Silicon via UTM/Parallels).
- Laptops Snapdragon X — support à évaluer (état du support Linux encore mouvant).

Contraintes arm64 :
- Pas de driver NVIDIA : la couche `akmod-nvidia` est **conditionnée à l'architecture** dans le Containerfile (amd64 uniquement).
- Builds arm64 en CI sur runners natifs `ubuntu-24.04-arm` — pas d'émulation qemu (voir [BUILD.md](BUILD.md) §1).
- ollama fonctionne en CPU sur arm64 ; accélération GPU hors périmètre pour l'instant.

## Matrice de build

| Artefact | amd64 | arm64 |
|---|---|---|
| Image bootc `ghcr.io/micka420-collab/vibeos` | ✅ (manifest) | ✅ (manifest) |
| Couche NVIDIA/CUDA | ✅ | ❌ (exclue) |
| ISO installable | ✅ à chaque release | ✅ à chaque release |
| Validation matérielle | PC de référence n°1 | VM arm64 + Raspberry Pi 5 (à acquérir) |
