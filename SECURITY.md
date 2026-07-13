# Politique de sécurité — VibeOS

> Version : 0.1.1 (2026-07-08) · Statut : projet en développement actif, **non destiné à la production**.
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
- **Livré en v0.1** : les images OS publiées sont signées avec sigstore/cosign (keyless) en CI — la signature accompagne chaque **release** (tag `v*`, ou dispatch manuel explicitement confirmé) et porte sur le **digest du manifest multi-arch**, celui que référencent les tags de consommation (`latest`, `<sha>`, la version). Les push ordinaires sur la branche principale déclenchent un build de vérification **non publié et non signé**. (Note : les tags par architecture `:<sha>-amd64` / `:<sha>-arm64` poussés pendant une release sont des artefacts intermédiaires du manifest, non signés individuellement ; consommez toujours un tag de manifest et vérifiez sa signature.)
- **Livré en v0.1** : les dépendances sont réellement épinglées — `vibed/Cargo.lock` commité, CLIs npm installées en versions exactes, image de base référencée par digest (`fedora-kinoite:42@sha256:…`), archives binaires (ollama) et sources compilées (quickshell) vérifiées par sha256 avant usage.
- **Livré (2026-07-08, suites de l'audit)** :
  - les **GitHub Actions sont épinglées par SHA de commit** (le tag lisible reste en commentaire ; un tag ne peut plus être déplacé sous la CI) ;
  - les **dépôts COPR sont désactivés dans l'image livrée** — activés uniquement le temps de l'installation au build ; un système déployé ne fait jamais confiance à un COPR à l'exécution ;
  - `npm install` s'exécute avec **`--ignore-scripts`** (les scripts de cycle de vie npm — vecteur classique d'exfiltration — ne s'exécutent pas au build), avec **deux exceptions délibérées et tracées**, rejouées via un `npm rebuild` ciblé : `@anthropic-ai/claude-code` (postinstall = binaire natif premier-parti) et `opencode-ai` (postinstall = câblage du binaire de plateforme livré dans ses propres optionalDependencies) — toutes deux détectées par la couche de vérification, comme prévu. Chaque CLI livrée est **prouvée fonctionnelle** (`--version`) dans une couche qui casse le build sinon, et `claude`/`opencode` sont re-prouvés **après purge** des répertoires de build (les binaires vivent bien dans `/usr`, pas dans un `$HOME` de build) ;
  - un **manifeste NEVRA** (`/usr/share/vibeos/packages-nevra.txt`) enregistre l'inventaire RPM exact de chaque image — diffable entre releases, ré-auditable.
  - **SBOM SPDX** de l'image publiée en artefact de release (`anchore/sbom-action`) et **scan de vulnérabilités Trivy** (advisory : CRITICAL/HIGH signalés, sans bloquer une release précoce — un OS bâti sur Fedora hérite de CVE upstream qu'on suit sans les bloquer). Job CI dédié `supply_chain`.
- **Cible Phase 4** : chaîne de démarrage mesurée UEFI Secure Boot → UKI → dm-verity/composefs (à terme : ce qui démarre est ce qui a été signé, rien d'autre — la base Fedora bootc apporte déjà Secure Boot via shim et composefs), et **vérification de signature exigée côté client** avant tout `bootc upgrade` (politique containers `policy.json` sigstore — intestable tant que le flux d'upgrade n'est pas exercé sur machine réelle).

### 1.3 Chiffré
- La mémoire de la machine (`/var/lib/vibeos/memory`) est créée au premier démarrage par `vibeos-genesis.service` (source : [memory/genesis.sh](memory/genesis.sh)). **En v0.1 elle est écrite en clair** : le chiffrement **LUKS** (déverrouillage TPM2 en option) est un livrable de la **Phase 3**, tout comme le mode amnésique qui la reconstruira en tmpfs à chaque boot (generator systemd). Documenté honnêtement — voir [docs/MEMORY.md](docs/MEMORY.md) et [ROADMAP.md](ROADMAP.md).
- Règle invariable (de conception) : les secrets (clés API des fournisseurs IA, jetons) ne doivent **jamais être stockés en clair**, jamais dans `environment.d`, et jamais dans la mémoire VibeOS. Le mécanisme cible est `systemd-creds` (scellés TPM2 quand disponible) + kernel keyring — voir [docs/SECURITY-ARCHITECTURE.md](docs/SECURITY-ARCHITECTURE.md), §4 — **non câblé à ce stade** : aucun composant VibeOS ne collecte ni ne stocke encore ces secrets (les CLIs IA gèrent les leurs). En attendant, la denylist codée en dur de `vibed` interdit aux agents de lire les magasins de credentials, y compris ceux des agents IA eux-mêmes (`~/.claude/`, `~/.config/gh/`, `~/.gemini/`, `~/.codex/`, opencode, ollama…), pour tous les utilisateurs.
- Le chiffrement intégral du disque à l'installation est l'objectif par défaut de l'ISO (installateur : Phase 5).

### 1.4 Audité
- **Chaque appel d'outil MCP** effectué par un agent est journalisé : identité de l'appelant (uid/gid/pid via les peer credentials du socket), outil, digest des arguments (FNV-1a, non cryptographique en v0.1), décision de politique, tier, approbation humaine éventuelle, résultat. Pour une action **approuvée** (T2/T3), l'`outcome` porte l'**uid de l'opérateur** qui a accordé le grant (`ok_approved(by_uid=…)`) : le grant étant consommé/supprimé au ré-appel, le journal inviolable est la **seule trace durable de qui a autorisé** le changement système (responsabilité).
- Le journal d'audit est **JSONL append-only chaîné par hachage**, à raison d'**un fichier par jour UTC** sous `/var/lib/vibeos/audit/` (`vibed-AAAA-MM-JJ.jsonl`). Il est en **lecture interdite et écriture interdite pour les agents**, à la fois par la politique livrée (voir `security/policy.d/default.toml`) et par une denylist codée en dur dans `vibed` (indépendante de la politique).
- **Inviolabilité (tamper evidence) — livré** : chaque enregistrement porte `seq` (compteur monotone), `prev` (SHA-256 de l'enregistrement précédent) et `hash` (SHA-256 de l'enregistrement lui-même). Toute altération, suppression ou réordonnancement casse la chaîne, détecté et localisé par `vibed --verify-audit`. Le SHA-256 est l'implémentation maison sans dépendance de vibed. **Reste Phase 4** : l'ancrage externe de la tête de chaîne (scellement TPM / journal de transparence Rekor) qui fermerait aussi la troncature du dernier enregistrement, et la réplication journald — voir [docs/SECURITY-ARCHITECTURE.md](docs/SECURITY-ARCHITECTURE.md), §8.

### 1.5 Moindre privilège pour les agents IA
- Aucun agent ne parle directement au système : tout passe par le serveur MCP de `vibed` (`/run/vibed/mcp.sock`, socket `root:vibeos-agents` en `0660`), qui applique le moteur de politiques (`/etc/vibeos/policy.d/*.toml` — la première règle qui matche gagne, chargement fail-closed : une politique invalide empêche `vibed` de servir).
- Capacités hiérarchisées : **T0** observation (lecture seule) · **T1** modification utilisateur · **T2** modification système · **T3** destructif. **T2 et T3 exigent toujours une approbation humaine** (le tier est un plancher, jamais abaissable par une règle). Le défaut absolu est le refus.
- **Flux d'approbation humaine (plomberie livrée)** : un appel T2/T3 crée une **requête d'approbation** dans un store root-only (`/var/lib/vibeos/approvals`, sur la denylist — un agent ne peut ni lire ni forger un grant) et répond « en attente, id X ». L'opérateur exécute `vibectl approve X` (le store root-only restreint cette action à root) ; le grant est **à usage unique**, borné à l'exact `(outil, cible, uid)`, et **expire vite** (5 min). Au ré-appel identique, `vibed` consomme le grant et exécute (audité `*_approved`). Un agent ne peut donc **jamais** approuver sa propre requête. **Reste Phase 4** : le dialogue d'approbation Plasma / branchement HUD (présentation) et les backends T2 réels (`pkg.install`/`svc.restart` sont encore des stubs).
- **Trousse cybersécurité gouvernée** : VibeOS embarque une trousse de pentest/DFIR professionnelle (voir [docs/SECURITY-TOOLKIT.md](docs/SECURITY-TOOLKIT.md)). Ces outils doubles usage sont classés par tier : tout ce qui agit **activement contre une cible est T2**, le **destructif est T3** — donc soumis à approbation humaine côté agent. Un agent peut **découvrir** la trousse en lecture seule (`sectools.list`, T0) mais **n'exécute aucun outil** : le chemin d'exécution gouverné est lié au flux d'approbation de la Phase 4. Usage strictement autorisé (systèmes propres, engagements mandatés, CTF, recherche) ; aucun malware/ransomware embarqué.
- **En v0.1, `vibed` tourne en root** — documenté honnêtement ; la bascule vers `User=vibed` avec `CapabilityBoundingSet` en allow-list vide est un livrable **Phase 4**. Le bac à sable d'exécution par outil (systemd-run, seccomp, Landlock) est un livrable **Phase 3**. SELinux est en mode `enforcing` (politique targeted Fedora) ; la politique dédiée à `vibed` est prévue en Phase 4.

---

## 2. Signaler une vulnérabilité

**Ne créez jamais d'issue GitHub publique pour une vulnérabilité.**

### Canal de signalement
> **État présent (dépôt privé)** : tant que le dépôt n'est pas public, seuls ses collaborateurs peuvent le voir — une issue du dépôt (privée de fait) est acceptable pour eux. Les deux canaux ci-dessous décrivent le dispositif à l'ouverture publique ; l'adresse e-mail de contact et la clé PGP seront publiées **dans ce fichier avant** la première release publique (bloquant de la Phase 6).

1. **Préféré** : GitHub Security Advisories — onglet *Security → Report a vulnerability* du dépôt (signalement privé), dès que le dépôt public existe.
2. **Alternative** : e-mail au mainteneur avec le préfixe de sujet `[VIBEOS-SEC]` (adresse et clé PGP publiées ici avant la première release publique).

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

- **Rust** pour `vibed` : pas d'`unsafe` sans justification commentée et revue dédiée. La CI livrée (`.github/workflows/ci.yml`) exécute en bloquant : `cargo fmt --check`, `cargo build --locked`, `cargo test --locked` (dont les tests d'intégration MCP bout-en-bout sur socket), `cargo clippy --all-targets --locked -- -D warnings`, **`cargo audit`** (advisories RustSec, job dédié) et un **job MSRV** qui rebuild+teste sur **Rust 1.75** (le plancher déclaré dans `Cargo.toml`, vérifié et non plus seulement affirmé) ; `cargo deny` reste une cible.
- Dépendances épinglées — c'est effectif : `vibed/Cargo.lock` commité, paquets npm installés en versions exactes **avec `--ignore-scripts`**, image de base référencée par digest, GitHub Actions épinglées par SHA, archives/sources tierces vérifiées par sha256, inventaire NEVRA embarqué dans l'image. Les mises à jour de dépendances (dont les bumps de SHA d'Actions et de digest de base) sont revues comme du code, dans des commits dédiés.
- Toute nouvelle capacité exposée aux agents (nouvel outil MCP) doit déclarer son tier, ses contraintes de chemin et son entrée dans le modèle de menace **avant** merge.
- Les tests de non-régression sécurité livrés (refus par défaut, outil inconnu refusé, T2/T3 jamais auto-approuvés, chargement effectif de `security/policy.d/default.toml`, denylist de chemins) sont exécutés par la CI et bloquants ; la couverture s'étend à chaque nouvel outil.

---

*Ce document évolue avec le projet. Les changements de périmètre ou de canal de signalement sont annoncés dans les notes de release.*
