# memory/ — sources du sous-système mémoire

Ce dossier contient les sources du sous-système mémoire de VibeOS. La
spécification complète (philosophie, layout, cycle de vie, mode amnésique,
chiffrement, outils MCP `memory.*`) est dans [`docs/MEMORY.md`](../docs/MEMORY.md).

| Fichier | Rôle | Destination dans l'image |
|---|---|---|
| `genesis.sh` | Séquence « Genesis » du premier boot : crée la mémoire vierge de la machine | `/usr/libexec/vibeos/genesis.sh`, invoqué par `vibeos-genesis.service` |

**Périmètre exact de `genesis.sh` en v0.1** : le script ne fait **ni
`cryptsetup`, ni `mkfs`, ni aucun montage**. Il peuple un répertoire déjà
présent (`mkdir -p` + écriture de fichiers, **en clair**) et n'accepte aucun
flag `--amnesic` — il lit uniquement la variable d'environnement
`VIBEOS_MEMORY_MODE`. Le chiffrement LUKS du volume (via crypttab/unité de
montage) et le tmpfs du mode amnésique (via un generator systemd) sont des
livrables de la **Phase 3** — voir [`docs/MEMORY.md`](../docs/MEMORY.md),
qui fait foi, et [`ROADMAP.md`](../ROADMAP.md).

L'unité systemd porte le garde-fou
`ConditionPathExists=!/var/lib/vibeos/memory/.initialized` ; le script est en
plus idempotent par lui-même (sortie 0 immédiate si `.initialized` existe).
L'intégration dans l'image bootc est décrite dans [`docs/BUILD.md`](../docs/BUILD.md).

## Tester `genesis.sh` dans WSL2 Ubuntu

L'hôte de développement est Windows 11 ; le script se teste dans WSL2 Ubuntu en
pointant `VIBEOS_MEMORY_DIR` vers un répertoire jetable — **jamais** vers
`/var/lib/vibeos/memory` sur la machine de dev.

```bash
# Depuis Windows : ouvrir WSL2 Ubuntu
wsl -d Ubuntu

# Se placer dans le dépôt (monté par WSL sous /mnt/f, attention aux espaces)
cd "/mnt/f/je ne sais pas encore"

# 1. Exécution dans un répertoire de test
VIBEOS_MEMORY_DIR=/tmp/vibeos-memory-test bash memory/genesis.sh

# 2. Inspecter le résultat
find /tmp/vibeos-memory-test -mindepth 1 | sort
cat /tmp/vibeos-memory-test/identity.toml
jq . /tmp/vibeos-memory-test/hardware.json        # sudo apt install -y jq
cat /tmp/vibeos-memory-test/journal/"$(date -u +%Y-%m-%d)".jsonl

# 3. Vérifier l'idempotence : la seconde exécution sort immédiatement (code 0)
VIBEOS_MEMORY_DIR=/tmp/vibeos-memory-test bash memory/genesis.sh
echo $?

# 4. Simuler le mode amnésique (identity.toml doit contenir mode = "amnesic")
rm -rf /tmp/vibeos-memory-test
VIBEOS_MEMORY_DIR=/tmp/vibeos-memory-test VIBEOS_MEMORY_MODE=amnesic bash memory/genesis.sh
grep '^mode' /tmp/vibeos-memory-test/identity.toml

# 5. Nettoyage
rm -rf /tmp/vibeos-memory-test
```

### Vérifications attendues

- `identity.toml` : `hostname`, `machine_id`, `birth` (ISO 8601), `mode`.
- `hardware.json` : JSON valide (`jq .` ne doit pas échouer), avec des marqueurs
  explicites du type `"(lscpu not available)"` si un outil manque — jamais un crash.
- `journal/<AAAA-MM-JJ>.jsonl` : une ligne, `type` = `genesis`.
- `.initialized` : présent, écrit en **dernier**, contient l'horodatage de naissance.
- Permissions : racine en `0700`, fichiers en `0600` (le script pose `umask 077`).

## Qualité

```bash
shellcheck memory/genesis.sh   # doit passer sans warning (sudo apt install -y shellcheck)
bash -n memory/genesis.sh      # vérification de syntaxe seule
```
