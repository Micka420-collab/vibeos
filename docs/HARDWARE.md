# Matériel — cibles et machine de référence

VibeOS est **multi-architecture** : chaque release publie des images `amd64` (x86_64) et `arm64` (aarch64) sous un manifest OCI unique (`ghcr.io/micka420-collab/vibeos`), plus une ISO par architecture.

## Machine de référence n°1 (amd64) — PC du mainteneur

Relevé du 2026-07-03. C'est sur cette machine que VibeOS sera flashé ; toute release doit être validée dessus (d'abord en VM Hyper-V, puis sur SSD dédié en dual-boot, puis bascule complète).

| Composant | Référence | Implications pour VibeOS |
|---|---|---|
| CPU | AMD Ryzen 7 3700X — 8 cœurs / 16 threads, x86_64-v3 | Cible `amd64`. Compilations et inférence CPU confortables. |
| RAM | 16 Go | Modèles locaux 7B–13B quantisés OK via ollama ; garder le bureau sobre. |
| GPU | **NVIDIA GeForce RTX 3070 Ti (8 Go VRAM)** | Driver propriétaire **obligatoire dans l'image** : `akmod-nvidia` + `xorg-x11-drv-nvidia-cuda` (RPM Fusion), technique éprouvée par Bazzite. Débloque CUDA pour ollama (inférence locale GPU). Secure Boot : modules signés par notre clé MOK (Phase 4). |
| Stockage | NVMe Kingston SA2000 1 To + SSD SATA 512 Go / 256 Go / 120 Go | Cible d'installation recommandée : un SSD dédié (dual-boot sans risque) avant bascule complète sur le NVMe. |
| Firmware | UEFI (GPT), machine sous Windows 11 aujourd'hui | Compatible Secure Boot + TPM → chaîne de boot mesurée prévue Phase 4, déverrouillage LUKS par TPM2 possible. |
| Virtualisation | Hyper-V actif | Banc de test VM local pour chaque ISO avant tout flash. |

### Checklist de validation sur cette machine

- [ ] ISO amd64 boote en VM Hyper-V (Gén. 2, Secure Boot désactivé d'abord, puis activé)
- [ ] Driver NVIDIA chargé (`nvidia-smi` fonctionnel) sur matériel réel
- [ ] `ollama` utilise le GPU (offload CUDA vérifié)
- [ ] KDE Plasma fluide en Wayland avec le driver propriétaire
- [ ] Installation sur SSD dédié en dual-boot avec Windows 11 (GRUB/systemd-boot n'écrase pas le boot Windows)
- [ ] `vibeos-genesis.service` s'exécute au premier boot (mémoire créée, `.initialized` présent)

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
