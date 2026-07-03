# Politique de sécurité — VibeOS

> Version : 0.1 (2026-07-03) · Statut : projet en développement actif, **non destiné à la production**.
> Documents associés : [docs/THREAT-MODEL.md](docs/THREAT-MODEL.md) · [docs/SECURITY-ARCHITECTURE.md](docs/SECURITY-ARCHITECTURE.md) · [ROADMAP.md](ROADMAP.md)

VibeOS est une distribution Linux immuable, AI-native, dans laquelle des agents IA disposent d'un accès système direct via le daemon `vibed`. Ce modèle rend la sécurité non pas une fonctionnalité, mais **la contrainte structurante de toute l'architecture** : un agent IA est traité comme un *insider non fiable* en permanence.

---

## 1. Principes de sécurité

Toute contribution, toute décision d'architecture et toute revue de code doivent respecter ces cinq principes. Une PR qui en viole un sans justification documentée est refusée.

### 1.1 Immuable
- La racine du système est en lecture seule (bootc/OSTree, dérivé de Fedora Kinoite). L'état exécutable de la machine est **entièrement décrit par une image de conteneur versionnée** (`ghcr.io/micka420-collab/vibeos`).
- Les mises à jour sont atomiques ; tout déploiement défectueux ou compromis est annulable par rollback vers le déploiement précédent.
- Aucun composant de VibeOS ne doit écrire dans `/usr`. L'état mutable est confiné à `/etc` (géré) et `/var`.

### 1.2 Vérifié
- **Livré en v0.1** : les images OS sont signées avec sigstore/cosign (keyless) en CI, à chaque push sur la branche principale.
- **Livré en v0.1** : les dépendances sont réellement épinglées — `vibed/Cargo.lock` commité, CLIs npm/pip installées en versions exactes, image de base référencée par digest (`fedora-kinoite:42@sha256:…`).
- **Cible Phase 4** : chaîne de démarrage mesurée UEFI Secure Boot → UKI → dm-verity/composefs (à terme : ce qui démarre est ce qui a été signé, rien d'autre — la base Fedora bootc apporte déjà Secure Boot via shim et composefs), et vérification de signature exigée côté client avant tout `bootc upgrade`.

### 1.3 Chiffré
- La mémoire de la machine (`/var/lib/vibeos/memory`) est créée au premier démarrage par `vibeos-genesis.service` (source : [memory/genesis.sh](memory/genesis.sh)). **En v0.1 elle est écrite en clair** : le chiffrement **LUKS** (déverrouillage TPM2 en option) est un livrable de la **Phase 3**, tout comme le mode amnésique qui la reconstruira en tmpfs à chaque boot (generator systemd). Documenté honnêtement — voir [docs/MEMORY.md](docs/MEMORY.md) et [ROADMAP.md](ROADMAP.md).
- Règle invariable : les secrets (clés API des fournisseurs IA, jetons) ne sont **jamais stockés en clair**, jamais dans `environment.d`, et jamais dans la mémoire VibeOS : ils passent par `systemd-creds` (scellés TPM2 quand disponible) et le kernel keyring — voir [docs/SECURITY-ARCHITECTURE.md](docs/SECURITY-ARCHITECTURE.md), §4.
- Le chiffrement intégral du disque à l'installation est l'objectif par défaut de l'ISO (installateur : Phase 5).

### 1.4 Audité
- **Chaque appel d'outil MCP** effectué par un agent est journalisé : identité de l'appelant (uid/gid/pid via les peer credentials du socket), outil, digest des arguments (FNV-1a, non cryptographique en v0.1), décision de politique, tier, approbation humaine éventuelle, résultat.
- Le journal d'audit v0.1 est un fichier **JSONL append-only** : `/var/lib/vibeos/audit/vibed.jsonl`. Il est en **lecture interdite et écriture interdite pour les agents**, à la fois par la politique livrée (voir `security/policy.d/default.toml`) et par une denylist codée en dur dans `vibed` (indépendante de la politique).
- L'inviolabilité du journal (chaînage de hachés, réplication journald, scellement TPM) est un livrable **Phase 4** — voir [docs/SECURITY-ARCHITECTURE.md](docs/SECURITY-ARCHITECTURE.md), §8.

### 1.5 Moindre privilège pour les agents IA
- Aucun agent ne parle directement au système : tout passe par le serveur MCP de `vibed` (`/run/vibed/mcp.sock`, socket `root:vibeos-agents` en `0660`), qui applique le moteur de politiques (`/etc/vibeos/policy.d/*.toml` — la première règle qui matche gagne, chargement fail-closed : une politique invalide empêche `vibed` de servir).
- Capacités hiérarchisées : **T0** observation (lecture seule) · **T1** modification utilisateur · **T2** modification système · **T3** destructif. **T2 et T3 exigent toujours une approbation humaine** (le tier est un plancher, jamais abaissable par une règle). Le défaut absolu est le refus.
- **En v0.1, `vibed` tourne en root** — documenté honnêtement ; la bascule vers `User=vibed` avec `CapabilityBoundingSet` en allow-list vide est un livrable **Phase 4**. Le bac à sable d'exécution par outil (systemd-run, seccomp, Landlock) est un livrable **Phase 3**. SELinux est en mode `enforcing` (politique targeted Fedora) ; la politique dédiée à `vibed` est prévue en Phase 4.

---

## 2. Signaler une vulnérabilité

**Ne créez jamais d'issue GitHub publique pour une vulnérabilité.**

### Canal de signalement
1. **Préféré** : GitHub Security Advisories — onglet *Security → Report a vulnerability* du dépôt (signalement privé), dès que le dépôt public existe.
2. **Alternative** : e-mail au mainteneur avec le préfixe de sujet `[VIBEOS-SEC]`. Une clé PGP de contact sera publiée dans ce fichier avant la première release publique.

### Contenu attendu
- Description de la vulnérabilité et composant affecté (`vibed`, moteur de politiques, genesis, image, CI…).
- Étapes de reproduction ou preuve de concept.
- Impact estimé, en particulier : **la vulnérabilité permet-elle à un agent IA de dépasser son tier de capacité, de contourner une approbation humaine, de lire les secrets ou d'altérer l'audit ?** Ces quatre cas sont traités en sévérité critique.

### Engagements
| Étape | Délai visé |
|---|---|
| Accusé de réception | 72 h |
| Première évaluation (triage, sévérité) | 7 jours |
| Correctif ou plan de correction communiqué | 30 jours |
| Divulgation coordonnée | 90 jours max après signalement, ou à la publication du correctif |

Le projet étant pré-1.0 et maintenu bénévolement, ces délais sont des objectifs de bonne foi, pas un SLA contractuel. Aucun programme de bug bounty n'existe à ce stade.

### Versions supportées
| Version | Support sécurité |
|---|---|
| branche `main` (v0.1.x) | ✅ correctifs sur la dernière image publiée |
| toute image antérieure | ❌ mettre à jour via `bootc upgrade` |

---

## 3. Périmètre

### En périmètre (in scope)
- Le daemon `vibed` (Rust) : serveur MCP, moteur de politiques, dispatch des outils, audit.
- Les politiques livrées dans `security/policy.d/` et leur sémantique d'évaluation (tout contournement des tiers T0–T3 ou de l'approbation humaine est critique).
- La séquence Genesis et le sous-système mémoire ([memory/genesis.sh](memory/genesis.sh), `vibeos-genesis.service`, montage LUKS, mode amnésique).
- La définition de l'image OS (Containerfile, unités systemd, configuration par défaut) et la chaîne de build/signature CI (GitHub Actions, cosign, bootc-image-builder).
- **Les contournements de politique par injection de prompt** : une injection qui amène un agent à *demander* une action T2+ n'est pas une vulnérabilité (c'est le scénario nominal que le système doit contenir) ; une injection qui aboutit à une action T2+ **exécutée sans approbation humaine** en est une.

### Hors périmètre (out of scope)
- Vulnérabilités de Fedora, du noyau ou des paquets upstream → signaler à Fedora/upstream (nous suivons et intégrons leurs correctifs via rebuild d'image).
- Vulnérabilités des services cloud tiers (API Anthropic, GitHub, ghcr.io) → signaler aux fournisseurs concernés.
- Comportements indésirables *intrinsèques aux modèles* (hallucinations, réponses toxiques) sans franchissement de frontière de sécurité VibeOS.
- Attaques nécessitant un accès root préalable sur la machine cible (le modèle de menace suppose que root hors `vibed` est déjà la fin de partie — voir [docs/THREAT-MODEL.md](docs/THREAT-MODEL.md), §2).
- Déni de service par épuisement de quota/API des fournisseurs IA.

---

## 4. Pratiques de développement sécurisé

- **Rust** pour `vibed` : pas d'`unsafe` sans justification commentée et revue dédiée. La CI livrée (`.github/workflows/ci.yml`) exécute `cargo build`, `cargo test` et `cargo clippy -- -D warnings` en bloquant ; `cargo audit` et `cargo deny` sont une cible Phase 2.
- Dépendances épinglées — c'est effectif en v0.1 : `vibed/Cargo.lock` commité, paquets npm/pip installés en versions exactes, image de base référencée par digest. Les mises à jour de dépendances sont revues comme du code.
- Toute nouvelle capacité exposée aux agents (nouvel outil MCP) doit déclarer son tier, ses contraintes de chemin et son entrée dans le modèle de menace **avant** merge.
- Les tests de non-régression sécurité livrés (refus par défaut, outil inconnu refusé, T2/T3 jamais auto-approuvés, chargement effectif de `security/policy.d/default.toml`, denylist de chemins) sont exécutés par la CI et bloquants ; la couverture s'étend à chaque nouvel outil.

---

*Ce document évolue avec le projet. Les changements de périmètre ou de canal de signalement sont annoncés dans les notes de release.*
