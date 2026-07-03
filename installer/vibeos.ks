# vibeos.ks — Reference Anaconda kickstart for the VibeOS bootc image
# ============================================================================
# Spec (French): docs/INSTALLER.md — ISO build/test: docs/BUILD.md §4-5
#
# WHAT THIS FILE IS
#   The canonical, version-controlled description of a VibeOS installation:
#   deploy the bootc image with `ostreecontainer` (no RPM transaction), lay
#   out the disk (UEFI + Btrfs, LUKS-ready), create the first user, reboot.
#
# HOW IT IS USED
#   * Network install (works today): boot a Fedora installer medium with
#       inst.ks=https://<your-host>/vibeos.ks
#     The image is pulled from the registry (network required).
#   * Official offline ISO: bootc-image-builder (--type iso) EMBEDS the OCI
#     image and generates its own ostreecontainer stanza. Fragments of this
#     file are meant to be injected through the builder's config.toml
#     ([customizations.installer.kickstart]) — wiring that up is [Phase 5].
#
# PHASE GATING (honesty rule — ROADMAP.md numbering)
#   Everything active below ships with v0.1. Blocks marked [Phase 3] or
#   [Phase 5] are commented out on purpose: DO NOT enable them before the
#   matching phase actually delivers the supporting mechanism.
# ============================================================================

# --- Installer mode ---------------------------------------------------------
# `text` keeps the reference install reproducible and CI-friendly.
# The [Phase 5] guided experience will switch this to `graphical` with the
# VibeOS Dark themed Anaconda (see installer/branding/).
text

# --- Localization (PLACEHOLDERS — adjust per user / per spin) ---------------
# French-first project, but the reference file stays neutral; the interactive
# installer prompts for these when the lines are removed.
lang en_US.UTF-8
keyboard --xlayouts='us'
timezone Etc/UTC --utc

# --- Network ----------------------------------------------------------------
# DHCP on whatever link is up. Only needed to pull the image from the
# registry (network install path). The official ISO installs fully offline.
network --bootproto=dhcp --device=link --activate

# --- OS payload: deploy the bootc image (NOT an RPM installation) -----------
# `ostreecontainer` fetches and deploys the container image as the immutable
# OSTree root. This is the whole "package selection": there is none.
#
# NOTE on signatures: images are cosign-signed in CI since v0.1, but
# CLIENT-SIDE enforcement is a [Phase 4] deliverable (docs/BUILD.md §6).
# Until then --no-signature-verification is the honest setting. Removing it
# (and shipping a containers-policy) is an explicit Phase 4 exit criterion.
ostreecontainer --url=ghcr.io/micka420-collab/vibeos:latest --transport=registry --no-signature-verification

# --- Storage: UEFI + Btrfs, LUKS-ready --------------------------------------
# Layout rationale:
#   * GPT + EFI system partition: UEFI-only target (docs/HARDWARE.md).
#   * Separate /boot: keeps the bootloader happy once the Btrfs container
#     gets LUKS-encrypted (firmware/bootloader read /boot unencrypted).
#   * One Btrfs volume with subvolumes root/var/home: the OSTree model keeps
#     all mutable state under /var (home lives at /var/home on ostree
#     systems); snapshots and factory-reset stay cheap.
#   * "LUKS-ready": encrypting later means adding --encrypted on btrfs.01
#     WITHOUT changing the geometry — that is the whole point of this layout.
zerombr
clearpart --all --initlabel --disklabel=gpt
reqpart

part /boot/efi --fstype=efi   --size=512
part /boot     --fstype=ext4  --size=1024

# The Btrfs container. [Phase 5] flips encryption ON BY DEFAULT by appending:
#   --encrypted --luks-version=luks2
# (passphrase collected by the installer UI; TPM2 enrollment is offered at
# first boot — see docs/MEMORY.md §6).
part btrfs.01 --fstype=btrfs --grow

btrfs none --label=vibeos btrfs.01
btrfs /          --subvol --name=root LABEL=vibeos
btrfs /var       --subvol --name=var  LABEL=vibeos
btrfs /var/home  --subvol --name=home LABEL=vibeos

# [Phase 3] Dedicated encrypted volume for the machine memory.
# /var/lib/vibeos/memory moves to its own LUKS2 volume (label: vibeos-memory)
# unlocked via crypttab + a systemd mount unit — never by genesis.sh
# (docs/MEMORY.md §6). In v0.1 the memory is a plain 0700 directory under
# /var: DO NOT uncomment before Phase 3 lands.
#part /var/lib/vibeos/memory --fstype=xfs --size=4096 \
#    --encrypted --luks-version=luks2 --label=vibeos-memory

# --- Bootloader --------------------------------------------------------------
# Default kernel args only. The amnesic mode is NOT a default kernel arg:
# it ships as a separate boot menu entry (see %post, [Phase 3]).
bootloader --append="rhgb quiet"

# --- Users -------------------------------------------------------------------
# Root stays locked: administration goes through the wheel user (sudo).
rootpw --lock

# Reference user for UNATTENDED TEST installs ONLY. COMMENTED OUT BY DEFAULT so
# the reference kickstart ships NO active plaintext password: with no `user`
# line, Anaconda prompts interactively for account creation. To run an
# unattended test install, uncomment the line below and replace the password
# (or switch to --sshkey) before any real deployment.
# (Official test ISOs inject the user via bootc-image-builder config.toml
# [[customizations.user]] instead — docs/BUILD.md §4.1.)
#user --name=vibe --groups=wheel --password=changeme --plaintext
#sshkey --username=vibe "ssh-ed25519 AAAA... you@machine"

# --- Post-installation -------------------------------------------------------
%post --erroronfail
set -eu

# 1) Apply VibeOS systemd presets.
#    The image already ships /usr/lib/systemd/system-preset/50-vibeos.preset
#    and presets are applied at image build; re-running here is
#    belt-and-suspenders for units the installer environment may have
#    touched. Note the v0.1 semantics (docs/ARCHITECTURE.md §4.1):
#      * vibeos-genesis.service runs at FIRST BOOT only
#        (ConditionPathExists=!/var/lib/vibeos/memory/.initialized)
#      * vibed.service is enabled but SKIPPED while /usr/bin/vibed does not
#        exist (Phase 1) — "condition failed" is the expected, healthy state.
systemctl preset vibeos-genesis.service vibed.service || true

# 2) Install stamp — /etc is the writable config layer (NEVER write /usr:
#    it is immutable image content).
install -d -m 0755 /etc/vibeos
date -Is > /etc/vibeos/install-date

# 3) [Phase 3] Amnesic boot menu entry.
#    A dedicated BLS entry appends `vibeos.amnesic=1`; a systemd generator
#    (Phase 3, not in the v0.1 image) reads it, mounts a tmpfs over
#    /var/lib/vibeos/memory and exports VIBEOS_MEMORY_MODE=amnesic to
#    Genesis, which then re-runs at EVERY boot (docs/MEMORY.md §5).
#    Enabling this before the generator exists would boot a normal,
#    PERSISTENT system that merely carries a dead kernel arg — misleading,
#    hence commented out.
#cp /boot/loader/entries/*.conf /boot/loader/entries/vibeos-amnesic.conf
#sed -i -e 's/^title .*/title VibeOS (amnesic — memory in RAM, lost at shutdown)/' \
#       -e 's/^options \(.*\)/options \1 vibeos.amnesic=1/' \
#       /boot/loader/entries/vibeos-amnesic.conf

%end

# --- Done --------------------------------------------------------------------
# First boot after this reboot: vibeos-genesis.service creates the machine
# memory in /var/lib/vibeos/memory (identity, hardware profile, journal,
# `.initialized` written last — crash-safe). The birth interview on top of
# Genesis is [Phase 3]. The installer never runs again.
reboot
