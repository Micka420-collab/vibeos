# Registre des décisions d'architecture (ADR)

> Format : **Contexte / Décision / Alternatives considérées / Conséquences**.
> **ADR-001 à 009** : décisions fondatrices **acceptées** (2026-07-03). **ADR-010+**
> (ouvertes le 2026-07-13) portent chacune leur propre statut en tête — *proposé* /
> *implémenté (mécanisme)* / *plan* — mis à jour au fil des livraisons.
> Une ADR n'est jamais modifiée sur le fond après acceptation : elle est remplacée par une nouvelle ADR qui la référence (le statut, lui, est tenu à jour). Architecture détaillée : [ARCHITECTURE.md](ARCHITECTURE.md).

---

## ADR-001 — Base immuable bootc/OSTree

### Contexte
VibeOS doit être immuable (racine en lecture seule), se mettre à jour atomiquement avec rollback garanti, offrir une sémantique de retour usine, et être construit/distribué comme un artefact reproductible sur plusieurs années. Des agents IA modifieront le système : le socle doit rendre toute dérive impossible par construction.

### Décision
Distribution **image-based** via **bootc/OSTree** : l'OS entier est une image OCI (`ghcr.io/micka420-collab/vibeos`), construite en CI, signée, déployée atomiquement. Racine en lecture seule, `/etc` en merge 3-way, `/var` persistant. ISO d'installation via bootc-image-builder.

### Alternatives considérées
- **Debian live-build** : produit des images live, pas un OS immuable géré ; pas de mises à jour atomiques ni de rollback natif ; il faudrait reconstruire tout l'outillage (verity, A/B, factory-reset).
- **Arch + archiso** : rolling release incompatible avec des images testées et signées ; archiso vise les médias live, pas un cycle de vie d'OS installé.
- **NixOS** : reproductibilité excellente, mais modèle déclaratif (evaluation Nix sur la machine) plutôt qu'image scellée ; intégration SELinux quasi inexistante ; écosystème dm-verity/composefs/UKI beaucoup moins mûr ; courbe d'apprentissage et bus factor élevés pour un projet solo pluriannuel.

### Conséquences
- (+) Mises à jour atomiques, rollback en un reboot, factory-reset natif, parc homogène (toutes les machines exécutent le même hachage).
- (+) Le pipeline OS = pipeline conteneur : Containerfile, registry, CI GitHub Actions, signature cosign — outillage standard.
- (+) dm-verity/composefs et UKI sont des citoyens de première classe dans cet écosystème.
- (−) Toute modification de la racine exige une nouvelle image : boucle de dev plus lente (atténuée par `bootc usroverlay` en dev).
- (−) Les logiciels hors image passent par Flatpak/toolbox/conteneurs — discipline à documenter.

---

## ADR-002 — Fedora Kinoite + KDE Plasma 6

### Contexte
ADR-001 impose une base OSTree/bootc mûre. Il faut aussi un bureau Wayland moderne, SELinux enforcing prêt à l'emploi, et un rythme de mise à jour qui suit le noyau et les outils de sécurité récents (composefs, landlock, UKI).

### Décision
Dériver de **Fedora Kinoite** : variante atomique officielle de Fedora avec **KDE Plasma 6** sur **Wayland**, SELinux enforcing par défaut, et support bootc de premier plan.

### Alternatives considérées
- **Fedora Silverblue (GNOME)** : même socle, mais Plasma 6 offre une densité de configuration et un écosystème Qt mieux adaptés aux outils d'approbation/console système envisagés ; préférence produit assumée.
- **openSUSE Aeon/Kalpa (MicroOS)** : immuable via snapshots Btrfs, modèle transactionnel différent (pas image-based), écosystème bootc absent ; AppArmor plutôt que SELinux.
- **Universal Blue (uBlue) comme base** : attrayant (outillage bootc communautaire), mais ajoute une dépendance intermédiaire ; nous reprenons leurs patterns de CI sans dépendre de leurs images.

### Conséquences
- (+) SELinux, Wayland, Plasma 6, bootc : tout est déjà intégré et maintenu en amont ; VibeOS ne porte que son delta.
- (+) Cycle Fedora (~6 mois) : accès rapide aux mécanismes de sécurité récents.
- (−) Cycle Fedora : rebases réguliers obligatoires ; la CI doit reconstruire et tester à chaque version majeure.
- (−) Dépendance aux choix amont de Fedora (ex. politique de paquets, calendrier EOL).

---

## ADR-003 — Rust pour vibed

### Contexte
`vibed` est un démon système privilégié, exposé à des entrées adverses (agents IA compromis ou manipulés par injection de prompt). Il gère de la concurrence massive (multiples agents, appels d'outils parallèles, timeouts, approbation humaine asynchrone). Une faille mémoire dans ce composant compromet tout le modèle de sécurité.

### Décision
**Rust** avec le runtime asynchrone **tokio**. Binaire unique `/usr/bin/vibed`, unité `vibed.service`, serveur MCP (JSON-RPC 2.0) sur socket Unix.

### Alternatives considérées
- **Go** : concurrence simple et bon outillage, mais GC + runtime plus large, contrôle plus faible sur seccomp/landlock au niveau syscall, et FFI moins propre pour libcryptsetup/systemd.
- **C/C++** : contrôle maximal mais classe entière de vulnérabilités mémoire inacceptable pour un démon parlant à des entrées adverses.
- **Python/TypeScript** : itération rapide, mais empreinte d'exécution, chaîne d'approvisionnement (pip/npm) difficile à auditer dans une image immuable, et performances insuffisantes pour un démon système résident.

### Conséquences
- (+) Sécurité mémoire garantie à la compilation ; typage fort pour les états de la machine de politiques (tiers, décisions).
- (+) Binaire statique unique, démarrage instantané, faible empreinte — idéal dans une image immuable.
- (+) Écosystème mûr : tokio, serde (JSON-RPC), rustix/landlord (landlock), zbus (systemd), tracing (audit).
- (−) Vélocité de développement initiale plus faible ; exigence de compétence Rust pour tout contributeur au cœur.

---

## ADR-004 — MCP comme protocole de contrôle de l'OS

### Contexte
Les agents IA (Claude Code, Agent SDK, ollama, opencode) doivent piloter l'OS via une interface unique, découvrable, et indépendante du fournisseur de modèle. Inventer un protocole propriétaire condamnerait VibeOS à écrire et maintenir un adaptateur par agent.

### Décision
Exposer le contrôle de l'OS via un **serveur MCP** (Model Context Protocol, JSON-RPC 2.0) servi par `vibed` sur le socket Unix `/run/vibed/mcp.sock`. Les capacités système sont des *tools* MCP, chacun classé dans un tier T0–T3 (ADR-007).

### Alternatives considérées
- **API REST/gRPC propriétaire** : contrôle total du contrat, mais aucun agent ne la parle nativement — il faudrait des shims partout, à contre-courant de l'écosystème.
- **D-Bus directement exposé aux agents** : natif Linux mais surface énorme, granularité de politique inadaptée aux agents, et aucun client IA ne le consomme nativement. (`vibed` utilise D-Bus *en interne* vers systemd — mais ne l'expose pas.)
- **Shell direct (les agents exécutent des commandes)** : c'est l'anti-modèle que VibeOS combat : aucune médiation, aucun tier, audit non structuré.

### Conséquences
- (+) Tous les agents compatibles MCP fonctionnent immédiatement ; point d'étranglement unique pour politique + audit.
- (+) Socket Unix : authentification par `SO_PEERCRED`, contrôle d'accès par permissions/groupe, pas d'exposition réseau.
- (+) Découvrabilité : les agents énumèrent les tools et leurs schémas — la surface de contrôle est auto-documentée.
- (−) Dépendance à l'évolution de la spécification MCP ; le versionnage du protocole doit être géré dans vibed.
- (−) MCP ne définit pas nativement les tiers/approbations : cette sémantique est portée par vibed (métadonnées de tool + erreurs typées).

---

## ADR-005 — Modèle mémoire Genesis + LUKS + mode amnésique tmpfs

### Contexte
Vision fondatrice : l'OS est livré **vierge**. Aucune mémoire, identité ou donnée d'agent ne doit exister dans l'image. La mémoire de la machine doit naître localement, être chiffrée au repos, survivre aux mises à jour, disparaître au factory-reset — et pouvoir être **volatile** pour les usages sensibles (style Tails).

### Décision
- Mémoire sous `/var/lib/vibeos/memory`. Cible **Phase 3** : volume **LUKS2** dédié (clé scellée TPM2 + phrase de récupération), monté via `crypttab` + unité de montage systemd. **Livré en v0.1** : répertoire en clair (`root:root`, `0700`) — le chiffrement au repos n'existe pas encore et est documenté comme tel.
- Création au premier boot par **`vibeos-genesis.service`**, gardé par `ConditionPathExists=!/var/lib/vibeos/memory/.initialized`, exécutant `/usr/libexec/vibeos/genesis.sh` (source : [../memory/genesis.sh](../memory/genesis.sh)). `genesis.sh` construit l'arborescence et l'identité de la mémoire ; il ne fait **ni `cryptsetup`, ni `mkfs`, ni montage**, et lit son mode dans la variable d'environnement `VIBEOS_MEMORY_MODE`.
- **Mode amnésique** (**Phase 3**) : option kernel `vibeos.amnesic=1` lue par un *generator* systemd qui montera un tmpfs à la place du volume et injectera `VIBEOS_MEMORY_MODE=amnesic` ; Genesis rejoué à chaque boot, aucune écriture disque.
- Administration future via `vibectl`.
- Spécification de référence : [MEMORY.md](MEMORY.md).

### Alternatives considérées
- **Mémoire pré-embarquée dans l'image** : violerait la vision « vierge » ; identité identique sur toutes les machines = désastre sécurité et vie privée.
- **Chiffrement au niveau fichier (fscrypt/gocryptfs) au lieu de LUKS** : granularité fine mais fuite de métadonnées (noms, tailles, arborescence) ; LUKS chiffre le bloc entier et s'intègre au scellement TPM2.
- **Chiffrement intégral du disque uniquement (FDE global)** : protège tout mais ne distingue pas la mémoire IA du reste ; impossible de détruire/recréer la mémoire seule, et le mode amnésique deviendrait incohérent.
- **Amnésique par défaut (pur Tails)** : trop radical pour une machine de développement quotidienne ; l'amnésie est un mode, pas le défaut.

### Conséquences
- (+) Chaque machine a une mémoire unique, née localement ; l'image publiée ne contient aucun secret. Le chiffrement au repos arrive en Phase 3.
- (+) Factory-reset = purge de `/var` : la sémantique bootc suffit, pas de mécanisme ad hoc.
- (+) Mode amnésique sans chemin de code séparé : même Genesis, cible de montage différente (fournie par le generator, Phase 3).
- (−) Jusqu'à la Phase 3, la mémoire est **en clair au repos** : limite assumée de la v0.1, signalée dans [THREAT-MODEL.md](THREAT-MODEL.md).
- (−) Le scellement TPM2 (Phase 3/4) liera la mémoire à la machine : la migration nécessitera la phrase de récupération (procédure à documenter).
- (−) Genesis est un point critique du premier boot : il doit être idempotent et testé en VM dans la CI.

---

## ADR-006 — IA hybride cloud + local

### Contexte
Un OS « AI-native » inutilisable hors-ligne serait un échec ; un OS limité aux modèles locaux plafonnerait très en dessous de l'état de l'art. Les usages vont du vibecoding intensif (raisonnement maximal) à l'assistance système basique (classification, résumé) qui doit fonctionner dans un train.

### Décision
Runtime **hybride** préinstallé dans l'image, en versions épinglées (voir [BUILD.md](BUILD.md)) :
- **Claude Code** (`@anthropic-ai/claude-code`) et **Claude Agent SDK** (`@anthropic-ai/claude-agent-sdk`) : agents cloud, capacité maximale ;
- **gemini-cli** (`@google/gemini-cli`) et **codex** (`@openai/codex`) : CLIs agents cloud alternatifs ;
- **opencode** (`opencode-ai@1.17.13`, npm — projet sst/opencode, MIT) : agent terminal multi-fournisseur, 100 % local via ollama ;
- **ollama** : modèles locaux, fonctionnement 100 % hors-ligne.

Cette liste est exhaustive : ce sont les six CLIs livrés par l'image (cf. `os/packages.txt` et [ARCHITECTURE.md](ARCHITECTURE.md) §4.4). Tous passent par le même socket MCP de `vibed` : la politique et l'audit sont identiques quel que soit le fournisseur.

**Note sur aider** : `aider-chat` n'est plus préinstallé. Il exige Python < 3.13, or la base Fedora Kinoite embarque Python ≥ 3.13 — incompatible avec l'image immuable. `opencode` le remplace comme CLI de pair-programming multi-fournisseur livré par défaut. aider reste installable à la demande par l'utilisateur, sans toucher l'OS immuable : `uvx --python 3.12 aider-chat` (éphémère) ou `uv tool install --python 3.12 aider-chat` (persistant, dans `~/.local`).

### Alternatives considérées
- **Cloud uniquement** : dépendance réseau et fournisseur totale ; inacceptable pour un OS (et pour le mode amnésique, qui vise justement les contextes déconnectés/sensibles).
- **Local uniquement** : souveraineté maximale mais qualité insuffisante pour le vibecoding sérieux en 2026 sur du matériel grand public.
- **Couche d'abstraction multi-fournisseurs maison (routeur LLM)** : complexité prématurée ; les agents choisissent leur modèle, VibeOS gouverne leurs *actions* — pas leurs *inférences*.

### Conséquences
- (+) Dégradation gracieuse : sans réseau, ollama prend le relais ; le plancher de capacité est local, le plafond est cloud.
- (+) Neutralité : le modèle de sécurité (ADR-007) ne fait aucune confiance différenciée selon le fournisseur.
- (−) Image plus lourde ; les poids ollama vivent sous `/var/lib/ollama` (hors image, téléchargés post-install).
- (−) Deux chaînes de mise à jour d'agents à suivre (npm/releases Claude Code, releases ollama) dans la CI d'image.

---

## ADR-007 — Moteur de politiques à niveaux T0–T3 avec approbation humaine

### Contexte
Donner un accès système à des agents IA sans garde-fou est indéfendable : injection de prompt, hallucination d'action, compromission d'un agent. Mais tout interdire tue la proposition de valeur. Il faut une gradation du risque, lisible par un humain, appliquée mécaniquement, et entièrement auditable.

### Décision
Moteur de politiques dans `vibed`, configuration déclarative `/etc/vibeos/policy.d/*.toml`, quatre tiers :

| Tier | Périmètre | Défaut |
|---|---|---|
| T0 observe | lecture seule | allow |
| T1 modify-user | fichiers/config utilisateur | allow, journalisé |
| T2 modify-system | paquets, services | **approbation humaine** |
| T3 destructive | disque, credentials, identité réseau | **approbation humaine renforcée** |

Décisions possibles : `allow` / `deny` / `ask`. Sémantique d'évaluation : fichiers de `policy.d` chargés en ordre lexicographique, règles évaluées dans l'ordre, **la première règle qui matche gagne** ; aucune correspondance ou outil inconnu → **refus** (default-deny absolu) ; le tier est un **plancher** (une règle `allow` T2/T3 exige `approval = "human"`, sinon erreur de chargement) ; politique invalide → `vibed` refuse de démarrer (fail-closed).

**Chaque appel d'outil** (accordé ou refusé) est écrit dans le journal d'audit append-only `/var/lib/vibeos/audit/` (fichier par jour UTC ; identité de l'appelant uid/gid/pid, digest FNV-1a des arguments, **chaîne de hachés SHA-256 continue** vérifiable par `vibed --verify-audit` ; ancrage externe TPM/Rekor et réplication journald prévus en **Phase 4**, voir [SECURITY-ARCHITECTURE.md](SECURITY-ARCHITECTURE.md) §8). L'exécution approuvée sera sandboxée en **Phase 3** (unité systemd transitoire, seccomp, landlock, profil dérivé du tier) ; en v0.1 elle est in-process dans `vibed`.

### Alternatives considérées
- **Permissions binaires (tout ou rien)** : trop grossier — soit inutilisable, soit dangereux.
- **RBAC complet (rôles, groupes, délégations)** : sur-ingénierie pour une machine mono-utilisateur ; les tiers peuvent évoluer vers du RBAC si le besoin multi-utilisateurs apparaît (nouvelle ADR).
- **Politique en langage dédié (Rego/OPA, Cedar)** : puissant mais opaque pour l'utilisateur final ; TOML lisible et diffable suffit pour v0.1 ; un moteur externe reste intégrable derrière la même interface de décision.
- **Confiance au modèle (le LLM s'auto-modère)** : rejeté par principe — la sécurité ne repose jamais sur le comportement du modèle, uniquement sur l'enveloppe d'exécution.

### Conséquences
- (+) Modèle mental simple (« T2+ = on me demande ») ; défauts sûrs ; politique versionnable dans git.
- (+) L'audit rend toute action IA opposable a posteriori (inviolabilité renforcée par le chaînage de hachés en Phase 4).
- (−) Friction d'approbation : à mitiger par des approbations à portée limitée (durée/périmètre) sans jamais désactiver le défaut T2+.
- (−) La classification tool→tier est un jugement de sécurité critique : chaque nouveau tool exige une revue.

---

## ADR-008 — Signature des images et sécurité de la chaîne d'approvisionnement (cosign/sigstore)

### Contexte
Un OS immuable déplace la confiance vers le pipeline de build : si le registry ou la CI est compromis, chaque machine l'est à la prochaine mise à jour. La chaîne UEFI→UKI→dm-verity garantit l'intégrité *au boot* ; il faut la garantie symétrique *à la distribution*.

### Décision
- Images bootc construites par **GitHub Actions**, poussées sur `ghcr.io/micka420-collab/vibeos`, **signées avec cosign (sigstore)** en mode keyless (identité OIDC du workflow CI, transparence via Rekor). La signature en CI est **livrée dès la v0.1** (workflow `build-os.yml`).
- Vérification de signature **obligatoire côté client** (cible **Phase 2**) : politique de vérification (identité de l'émetteur épinglée) embarquée dans l'image ; `bootc upgrade` refusera toute image non signée ou signée par une autre identité. Tant que cette vérification n'est pas active, la signature existe mais n'est pas imposée localement — trou connu de la v0.1, fermé en Phase 2.
- La confiance de bout en bout est la composition : cosign (distribution) + composefs/fs-verity (intégrité locale) + Secure Boot (amorçage ; UKI en Phase 4).

### Alternatives considérées
- **Signature GPG des commits OSTree** : mécanisme historique d'OSTree, mais gestion de clés longue durée à la main, pas de journal de transparence, intégration OCI médiocre.
- **Cosign avec paire de clés statique** : plus simple à raisonner, mais une clé à protéger et à faire tourner ; le keyless élimine le secret long terme (au prix d'une dépendance à l'infrastructure sigstore — jugée acceptable, avec clé de secours hors-ligne documentée).
- **Pas de signature (TLS du registry seulement)** : protège le transport, pas le contenu — un registry ou un compte compromis suffirait ; rejeté.

### Conséquences
- (+) Aucun secret de signature long terme à protéger ; toute signature est publiquement journalisée (Rekor) et auditable.
- (+) La compromission du registry seul ne permettra pas de servir une image malveillante acceptée par les clients (une fois la vérification client active, Phase 2).
- (−) Dépendance à la disponibilité de sigstore (Fulcio/Rekor) pour signer — pas pour booter : une machine installée reste autonome.
- (−) Le placeholder `micka420-collab` doit être remplacé et l'identité CI épinglée dès la création du dépôt GitHub ; toute rotation d'identité CI = mise à jour coordonnée de la politique de vérification (procédure dans [BUILD.md](BUILD.md)).

> **Recalage 2026-07-13** : la vérification côté client, notée « cible Phase 2 » ci-dessus, est **recalée en Phase 4** ([ROADMAP.md](../ROADMAP.md) fait foi) — la Phase 2 a livré `vibed` + MCP sans câbler la politique de vérification, qui rejoint le durcissement de la chaîne de mise à jour. Par ailleurs, le dépôt GitHub `Micka420-collab/vibeos` existe désormais : `micka420-collab` n'est plus un placeholder mais la forme minuscule du propriétaire réel.

---

## ADR-009 — Multi-architecture amd64 + arm64, couche NVIDIA limitée à amd64

### Contexte
VibeOS vise du matériel de développement réel. La machine de référence n°1 du projet (AMD Ryzen 7 3700X, NVIDIA GeForce RTX 3070 Ti 8 Go, 16 Go de RAM — relevé complet dans [HARDWARE.md](HARDWARE.md)) est amd64 avec GPU NVIDIA : le driver propriétaire est obligatoire dans l'image pour débloquer Wayland/Plasma et CUDA (inférence ollama locale). En parallèle, le parc cible inclut des machines arm64 (Raspberry Pi 5, VM aarch64 dont Apple Silicon, serveurs Ampere). Un OS mono-architecture fermerait une partie du parc ; le driver NVIDIA n'a pas de sens sur les cibles arm64 visées.

### Décision
- L'image bootc est construite pour **linux/amd64 et linux/arm64** : le workflow CI (`.github/workflows/build-os.yml`) construit les deux plateformes (qemu-user-static + buildah/podman) et pousse un **manifeste OCI multi-arch** sur `ghcr.io/micka420-collab/vibeos` ; les jobs ISO sont matricés par architecture.
- Le Containerfile utilise `ARG TARGETARCH` ; la **couche NVIDIA** (RPM Fusion, `akmod-nvidia`, `xorg-x11-drv-nvidia-cuda`, `kernel-devel` apparié, pattern `akmods --force` éprouvé par Bazzite/Universal Blue) ne s'applique **que sur amd64**, derrière `ARG NVIDIA_ENABLED=1` honoré uniquement quand `TARGETARCH=amd64`. Le kmod est compilé **au build de l'image** (conforme OSTree : rien ne se compile sur la machine) ; la signature MOK des modules pour Secure Boot est un travail de **Phase 4**.
- Le matériel de référence et la matrice de validation par architecture sont documentés dans [HARDWARE.md](HARDWARE.md) — ce document fait foi pour les cibles matérielles.

### Alternatives considérées
- **amd64 uniquement** : plus simple (pas d'émulation en CI), mais exclut d'emblée les machines arm64 et fige une dette de portabilité que tout le reste du socle (Fedora bootc, ollama, CLIs npm/pip) ne justifie pas — tous sont déjà multi-arch.
- **Images séparées par variante GPU** (`vibeos` / `vibeos-nvidia`) : pattern Universal Blue valable, reporté — tant que le delta NVIDIA reste une couche conditionnelle unique, un seul nom d'image suffit ; une ADR ultérieure scindera si le delta grossit (versions de driver multiples, autres GPU).
- **Driver nouveau/libre uniquement (nouveau, nova)** : pas encore au niveau requis pour une RTX 3070 Ti en usage quotidien Wayland + CUDA ; réévaluation possible à chaque rebase Fedora.

### Conséquences
- (+) Une seule référence d'image ; le manifeste sert automatiquement la bonne architecture à `bootc upgrade` et à l'installateur.
- (+) La machine de référence amd64 est pleinement supportée (Plasma/Wayland sur driver propriétaire, CUDA pour ollama).
- (−) Builds arm64 émulés via qemu-user-static : CI sensiblement plus lente ; à surveiller, avec bascule possible vers des runners arm64 natifs.
- (−) Le kmod NVIDIA est lié à la version du noyau de l'image : chaque bump de noyau exige un rebuild complet (le pattern akmods au build le garantit).
- (−) Sous Secure Boot, le module NVIDIA non signé ne se charge pas : jusqu'à la signature MOK (Phase 4), le GPU NVIDIA exige Secure Boot désactivé ou l'enrôlement manuel d'une clé — limite documentée dans [HARDWARE.md](HARDWARE.md).

> **Évolution 2026-07-03 — runners natifs** : la CI ne passe plus par l'émulation qemu-user-static ; chaque architecture est construite sur son **runner natif** (`ubuntu-latest` pour amd64, `ubuntu-24.04-arm` pour arm64), jobs ISO compris. La conséquence « CI sensiblement plus lente » est levée ; le build local arm64 depuis un hôte amd64 reste possible via qemu ([BUILD.md](BUILD.md) §2.6).

---

## ADR-010 — Identité de l'appelant par exécutable (`[rule.callers]`) — *proposé, cible Phase 3/4*

**Statut** : proposé (non implémenté). Ouvert le 2026-07-13 à la suite d'une revue.

**Contexte.** Aujourd'hui, l'accès au socket MCP est une **confiance binaire** :
tout membre du groupe `vibeos-agents` obtient indifféremment la surface T0/T1
autorisée par la politique. Le moteur ne peut pas exprimer « l'agent local
`ollama` a moins de droits que Claude Code » : la politique matche sur le **nom
d'outil**, pas sur **qui appelle**. Or `vibed` capture déjà `SO_PEERCRED`
(uid/**pid**/gid) à l'accept — le pid permet de résoudre l'exécutable appelant
via `/proc/<pid>/exe`.

**Décision (cible).** Étendre le schéma de politique d'un sous-tableau optionnel
`[rule.callers]` : une allow/deny-list d'exécutables (chemins canoniques, ex.
`/usr/bin/claude`, `/usr/bin/opencode`) et/ou d'uids. À l'accept, `vibed`
résoudra `/proc/<pid>/exe` (chemin réel, canonicalisé) et l'exposera dans le
`CallContext` ; une règle sans `callers` reste inconditionnelle (rétrocompatible).

**Conséquences.**
- (+) Politique par **provenance d'agent** : restreindre les modèles locaux non
  fiables (jailbreak, poids empoisonnés — cf. [THREAT-MODEL.md](THREAT-MODEL.md)
  S4) à un sous-ensemble strict, tout en laissant plus de latitude à un client
  audité.
- (−) `/proc/<pid>/exe` est **indicatif, pas une preuve d'intégrité** : un
  binaire renommé/remplacé peut usurper un chemin. C'est un contrôle de *défense
  en profondeur*, pas une frontière de confiance — à combiner avec la signature
  d'exécutable (IMA/EVM) en Phase 4+ pour une garantie forte.
- (−) Fenêtre TOCTOU pid→exe (le pid peut être recyclé) : résolution **à
  l'accept**, sur la connexion, pas par appel.
- Ce mécanisme ne remplace **pas** le tiering ni l'approbation humaine T2/T3 ; il
  affine *qui* peut demander *quoi* en amont.

## ADR-011 — Lecture du journal système par un agent (`log.read`, T0) — *implémenté, cible Phase 2*

**Statut** : **implémenté (2026-07-14)**. Ouvert le 2026-07-13. Livré exactement
selon la forme (1)–(5) ci-dessous : outil `tools/log.rs` (`journalctl --unit`
borné, chemin absolu, env vidé, nom d'unité validé), **allowlist d'unités** via
`[rule.services].allowed` (défaut refus, évaluée AVANT le plancher de tier dans
`policy.rs::apply_rule`), **sortie bornée** (≤ 200 lignes + 64 Kio), **rédaction
best-effort** (marqueurs connus + jetons à forte entropie), **aucun filtre
libre**, et **audit** de l'unité via `handle_tools_call`. Tests : refus d'une
unité hors-liste avant approbation (`policy.rs`), sortie bornée + rédaction
(`tools/log.rs`).

> **Choix d'implémentation.** L'ADR nommait `[rule.units].allowed` ; réalisé via
> le sous-table **existant `[rule.services]`** (auquel on a ajouté `allowed`),
> puisque `CallContext.service` est déjà « l'unité systemd cible » et que
> `svc.*` le contraint déjà — un `[rule.units]` parallèle serait redondant. Le
> changement touche **`policy.rs`** (cœur sécurité) : un champ `allowed` +
> l'application `allowed` (calquée sur les paths, denied gagne) — **à revoir
> explicitement en revue humaine** (invariant projet).

**Contexte.** Un agent qui débogue a besoin de lire les logs (« pourquoi mon
service a-t-il échoué ? »). Mais les journaux système sont un **canal
d'exfiltration de premier ordre** ([THREAT-MODEL.md](THREAT-MODEL.md) S2) : les
services y déversent régulièrement des secrets (clés API échoées par une conf
maladroite, jetons dans des URL, chaînes de connexion, `environ` sur crash), et
`journald` agrège **tous les utilisateurs et services**. Exposer bêtement
`journalctl` à un agent (insider non fiable) reproduirait, en pire, le trou
cross-user que F1 vient de fermer côté `fs.read`. C'est pourquoi aucun outil de
lecture de log n'est livré tant que sa forme sûre n'est pas arrêtée.

**Décision (cible, T0 mais sensible à l'exfiltration).** Un outil `log.read`
**délibérément étroit**, jamais un `journalctl` générique :

1. **Allowlist d'unités uniquement.** Lecture bornée à une liste explicite
   d'unités (les unités d'agent de l'utilisateur, `vibed` lui-même), déclarée en
   politique (`[rule.units].allowed`). Défaut : refus. Jamais le journal système
   complet, jamais l'unité d'un autre utilisateur.
2. **Sortie bornée.** Dernières *N* lignes (plafond dur, ex. ≤ 200) et plafond
   d'octets — même discipline anti-DoS que `fs.read`/`fs.list`.
3. **Passe de rédaction** *best-effort* : masquage des motifs à forte entropie et
   des marqueurs connus (`*_KEY=`, `Bearer `, `PRIVATE KEY`, `AWS_…`, `password=`)
   avant retour. **Défense en profondeur, pas une garantie** (cf. conséquences).
4. **Aucun filtre libre.** Pas d'argument `grep`/regex fourni par l'agent : on
   n'offre pas « montre-moi les lignes contenant `password` » — l'outil ne doit
   pas devenir un chercheur de secrets.
5. **Audité** comme tout appel (unité, nombre de lignes) : une volumétrie ou une
   cadence anormale est détectable (mitigation S2), et le **rate-limiting par
   uid** déjà en place borne l'aspiration en boucle.

**Conséquences.**
- (+) L'agent se débogue seul (ses propres logs de service) sans shell ni lecture
  de fichiers bruts — utile et gouverné.
- (−) La **rédaction est heuristique, jamais complète** : le vrai contrôle est
  l'allowlist d'unités + la sortie bornée + l'audit, *pas* le masquage. Une unité
  autorisée qui logge un secret le divulguera : l'allowlist doit **exclure** les
  unités connues pour journaliser du sensible.
- (−) `journald` mêle les flux ; la sélection par unité (`_SYSTEMD_UNIT=`) doit
  être stricte (pas de préfixe qui ratisse large).
- Reste **T0 en tiering** mais **étiqueté sensible à l'exfiltration** dans le
  catalogue — candidat à un budget de rate-limit dédié plus serré.

**Alternatives écartées.** (a) exposer `journalctl` complet — rejeté
(exfiltration + cross-user) ; (b) laisser l'agent lire `/var/log` via `fs.read`
— rejeté (journald est binaire, et la denylist bloque déjà beaucoup ; ne règle
pas le cross-user) ; (c) pas d'outil de log — statu quo, mais prive l'agent d'un
auto-diagnostic légitime. La forme (1)–(5) est le compromis retenu et
**implémenté** ; l'allowlist livrée (`vibed.service`, `vibeos-agent@*.service`)
et le rédacteur best-effort restent à **revoir/ajuster en revue humaine**.

## ADR-012 — Capture du raisonnement par tap sur le flux, jamais via le transcript du CLI — *implémenté (mécanisme), cible Phase 2.5*

**Statut** : **mécanisme implémenté** (2026-07-13). Livré : le module `reasoning`
(store `memory/reasoning/<session>.jsonl`, `append_thinking`/`read_thinking`,
`safe_session_id` anti-traversal), l'outil MCP **T0 `agent.thinking`**, le
superviseur `vibectl agent run` qui tape le flux `stream-json` et extrait les
blocs `thinking` (`supervisor::extract_thinking`), et le composant HUD
`ReasoningPanel.qml` **branché en live et en historique** (`shell.qml` via
`Quickshell.Io.Socket` : `agent.sessions` rend la liste datée des sessions,
sélectionner une session passée va chercher son raisonnement à la demande —
2026-07-15).
**Reste** : le schéma `stream-json` exact par fournisseur n'est pas contractuel —
l'extraction est défensive et doit être vérifiée contre la version packagée du CLI
à l'intégration ; rétention/purge du store à trancher.

**Outil `agent.sessions` (T0, ajouté 2026-07-14 ; enrichi 2026-07-15).**
Découverte de session **et** historique : liste les sessions ayant un fichier de
raisonnement (`reasoning/*.jsonl`) pour qu'un observateur (le HUD) trouve une
session à passer à `agent.thinking` **et puisse dater/peser chaque session sans
un appel `agent.thinking` par session** (le N+1 qu'imposait la forme initiale).
**Retour** : `{ sessions: [{ id, started_unix (null si inconnu), last_unix,
bytes }...], count, total, truncated, latest }`, **activité la plus récente
d'abord**, `latest` = session écrite le plus récemment. **Sans argument**,
lecture seule (aucune écriture, aucune exécution). **Mêmes disciplines anti-DoS
que tout outil** : atteint via `handle_tools_call`, donc le **rate-limiter par
uid s'applique en amont** (avant dispatch, agnostique à l'outil) et l'appel est
audité. Ne crée jamais le store (fail-closed si Genesis n'a pas tourné).

**Trois choix de bornage explicites** (l'outil est *poll* par le HUD toutes les
5 s — son coût doit être prévisible, pas proportionnel au contenu du store) :
1. **Sortie plafonnée** à `REASONING_MAX_SESSIONS` (200). La forme initiale
   prétendait une « sortie bornée » alors que la liste **croissait sans limite**
   avec le nombre de sessions (le store n'est jamais purgé — la rétention reste
   à trancher, cf. ci-dessus). `total` dit combien existent réellement, et
   `truncated` l'annonce : une vue partielle ne se fait jamais passer pour
   exhaustive.
2. **Coût par session = un `stat` + une lecture bornée de la *première* ligne**
   (`READ_HEAD_CAP`). Le tri et la troncature ont lieu **avant** toute lecture,
   donc au plus 200 fichiers sont ouverts. **Aucun compteur de tours n'est
   exposé** : le produire exigerait de relire chaque fichier de bout en bout à
   chaque poll — une amplification I/O de plusieurs Mo par appel T0 répétable.
   `bytes` (gratuit, issu du `stat`) rend le même service d'ordre de grandeur.
3. **Tri par mtime, pas lexical.** L'ordre lexical initial ne *paraissait*
   chronologique que parce que les ids intègrent un horodatage de largeur fixe
   (`auto-<ts>-<pid>`) : tout id d'une autre forme cassait silencieusement
   `latest`. Le store étant strictement append-only, le mtime **est** l'instant
   du dernier bloc.

**`provider`/`model` ne sont pas rendus** : le store de raisonnement ne les porte
pas. Le superviseur les écrit dans le **journal mémoire** (enregistrement
`autonomous_session`), pas dans `reasoning/`. Les joindre depuis `agent.sessions`
imposerait un balayage du journal non borné à chaque appel — l'outil rend donc ce
que le store sait, et le panneau HUD affiche date/durée/poids plutôt qu'un
fournisseur deviné. Les porter dans le store (ligne d'en-tête ou fichier
`.meta.json` à côté) reste ouvert.

**Contexte.** Le raisonnement affiché par les CLI IA (Claude Code compris) n'est,
pour les modèles actuels, pas persisté sur disque par le CLI lui-même — seule une
signature cryptographique survit à la session. Le récupérer après coup depuis les
fichiers du CLI est donc impossible pour l'historique récent, et le format de ces
fichiers n'est de toute façon **pas contractuel** (plusieurs bugs ouverts sur des
transcripts corrompus par des blocs `thinking`).

**Décision.** Le superviseur d'agent (Phase 2.5) capte le raisonnement en tapant
le **flux structuré** (`stream-json`) au moment où il streame, en lecture seule, et
l'écrit dans un store VibeOS dédié (`/var/lib/vibeos/memory/reasoning/<session>.jsonl`)
— indépendant du transcript propre du CLI. Lecture gouvernée par un outil T0
`agent.thinking` (pas un scope de `memory.query` : le raisonnement est de
l'observabilité, pas un fait appris sur l'humain).

**Alternatives considérées.**
- Parser les transcripts JSONL du CLI après coup : rejeté — le champ pertinent est
  vide pour les modèles actuels, et le format n'est pas garanti stable entre versions.
- Demander au CLI de désactiver son propre effacement du champ `thinking` : hors de
  portée, ce n'est pas un comportement configurable côté VibeOS.

**Conséquences.**
- (+) Fonctionne quel que soit le fournisseur, indépendamment de ce qu'il choisit de
  persister lui-même.
- (+) Zéro risque de casser la reprise de session du CLI (capture **passive**).
- (−) Ne capte que ce qui est effectivement streamé — si un fournisseur streame moins
  que ce qu'il facture (modèles cloud résumés), VibeOS ne peut pas voir plus loin ; la
  note de transparence du HUD (`ReasoningPanel.qml`) le dit explicitement.

## ADR-013 — Mode autonome permanent (« always-on ») : autonomie totale T0/T1, T2/T3 en file asynchrone, plancher jamais levé — *implémenté (mécanisme), cible Phase 2.5*

**Statut** : **mécanisme implémenté** (2026-07-13). Livré : le superviseur
`vibectl agent run` (budgets wall-clock + nombre d'appels, kill-switch opérateur
`agent stop` — jamais un outil MCP), qui tourne l'agent en continu ; les T2/T3
restent gérés **par `vibed`** (`pending_approval` non bloquant + grant one-shot)
— le superviseur **ne touche jamais `approval.rs`** et ne lève jamais le plancher.
**Reste** : l'unité systemd template `vibeos-agent@.service` (mode always-on par
défaut au démarrage) et l'orchestration fine du basculement T0/T1 quand un T2/T3
est en attente (aujourd'hui : l'agent reçoit `pending_approval` et poursuit).

Contexte (conservé) — ouvert le 2026-07-13 à la demande explicite
d'un « mode autonome always pour tout ».

**Contexte.** L'utilisateur veut que l'IA travaille en **autonomie permanente, pour
tout**. Pris au pied de la lettre — « exécute n'importe quoi, y compris destructif
et système, sans accord humain » — cela **abaisserait le plancher d'approbation
T2/T3**, ce que les invariants du projet interdisent explicitement (§7 : purge/
destruction = T3 humain ; plancher T2/T3 **non abaissable** ; « INTERDIT d'affaiblir
un invariant »). Un OS où un agent prompt-injecté peut tout faire sans accord est un
**vecteur de ransomware** — précisément le scénario S1 du [THREAT-MODEL](THREAT-MODEL.md)
que toute l'architecture combat.

**Décision.** « Always-on » est implémenté comme **autonomie maximale à l'intérieur
du contrat de capacités existant**, sans jamais toucher au plancher :
1. Le superviseur tourne **par défaut/en permanence** ; l'agent enchaîne **seul**
   toute action T0 (observation) et T1 (modification-utilisateur) — l'humain n'est
   plus dans la boucle **synchrone** du T0/T1.
2. Une action **T2/T3 ne bloque plus** l'agent : elle est **mise en file** (le store
   d'approbation déjà livré, borné) et l'agent poursuit son travail T0/T1 ; l'humain
   approuve/refuse **en différé et en lot** (`vibectl approvals list` → `approve`/
   `deny`), et l'exécution passe alors par le **grant one-shot existant**, inchangé.
3. Le **plancher T2/T3 n'est jamais levé** : « autonome pour tout » = « autonome sur
   tout le T0/T1 sans babysitting », pas « exécute du destructif sans accord ».

**Alternatives considérées.**
- **Bypass total de l'approbation** (un vrai « exécute tout ») : **rejeté** — viole
  l'invariant §7 et le plancher non abaissable, transforme l'OS en surface de
  ransomware sur injection de prompt (S1). Si un opérateur le voulait malgré tout, ce
  serait un **risque assumé documenté**, jamais un défaut livré en dur.
- **Auto-approbation par l'agent** : rejeté par construction — un agent ne peut jamais
  approuver sa propre requête (store root-only + denylist ; F3).
- **Statu quo** (approbation synchrone bloquante) : conserve la sécurité mais casse
  l'autonomie longue voulue — d'où la file asynchrone.

**Conséquences.**
- (+) Autonomie réellement continue : une session de plusieurs heures ne s'arrête pas
  sur la première action système ; l'humain traite les T2/T3 quand il revient.
- (+) Aucune régression de sécurité : le chemin T2/T3 est **exactement** celui du mode
  supervisé (grant one-shot, audit `ok_approved(by_uid=N)`, rate-limiting).
- (−) Une file d'approbation peut s'accumuler si l'humain est absent longtemps —
  bornée par le plafond du store d'approbation (purge/dedup/cap déjà livrés).
- (−) L'agent peut rester « bloqué » sur une tâche qui *exige* un T2/T3 non encore
  approuvé ; il doit alors basculer sur d'autres travaux T0/T1 (comportement à cadrer
  côté superviseur).

## ADR-014 — VibeOS pour Zed : gouverner l'agent hébergé via l'adaptateur ACP, jamais le cœur de Zed — *cœur implémenté & vérifié (hors Zed), initiative parallèle*

**Statut** : **cœur implémenté et vérifié sans Zed** (2026-07-13). Investigation du
code réel menée avant tout patch (§ « Structure de l'adaptateur »), forme de fork
verrouillée (patch de prototype `canUseTool`). Livré : outil T0 `vibeos:policy.check`
(vibed), config couches 0/1 (Zed-only), et le paquet `zed/vibeos-claude-acp`
(couche 2) — `tsc` compile contre l'amont, 17 tests vitest (dont la preuve de
déterminisme et le client MCP socket), boot ACP headless vérifié (`npm run smoke`).
**Câblage image (ADR-015)** : étage `zed-agent-builder` livré et gardé off par
défaut (`ARG WITH_ZED_AGENT=0`) — bundle esbuild vérifié dans le build. **Reste** :
le **test d'intégration E2E en conditions réelles** (voir `BLOCKERS.md` — binaire
Claude natif + `vibed` démarré + client ACP/Zed), avant d'activer l'expédition.

**Contexte.** [Zed](https://zed.dev) est un éditeur rapide dont le panneau agent
parle **ACP** (Agent Client Protocol, `zed-industries/agent-client-protocol`).
> **Note (2026-07-16)** : le paquet a été renommé en amont. La dépendance réelle
> est **`@agentclientprotocol/claude-agent-acp`** (`package.json`, `^0.58.0`) ;
> le nom `@zed-industries/claude-code-acp` et les ancres de ligne de cette ADR
> reflètent le paquet **tel qu'analysé à la décision** (v0.58.1, 2026-07-13). Les
> `canUseTool`/bypass ont bougé de fichier depuis ; la cartographie ci-dessous
> reste vraie du raisonnement, pas des numéros de ligne courants.
L'adaptateur **`@zed-industries/claude-code-acp`** (`zed-industries/claude-code-acp`,
TypeScript/Node) fait tourner **Claude Code comme agent ACP** dans Zed : il expose
les outils de Claude Code (Read/Write/Edit/Bash…) côté éditeur et gère les
demandes de permission via le flux ACP. Sur VibeOS, on ne veut **pas** d'un agent
éditeur à l'accès fichier natif illimité : on veut que **toute action système de
l'agent passe par le moteur de politiques de `vibed`** (tiers T0–T3, audit,
approbation), comme pour Claude Code en terminal. Et on veut un **mode auto** qui
supprime le *prompt de permission de l'éditeur* — mais **uniquement** pour ce que
la politique classe déjà `Allow` (T0/T1), jamais pour T2/T3.

**Décision.** Cibler **l'adaptateur** (`claude-code-acp`), pas le cœur de Zed
(qu'on ne fork jamais). Livraison en **couches** (ROADMAP § Initiative) :

| Couche | Livrable | Fork ? |
|---|---|---|
| **0** | `settings.json` VibeOS pour Zed dans `/etc/skel/.config/zed/` : `context_servers` déclarant `vibed` (serveur MCP `vibeos:*`) + `tool_permissions` par tier. L'agent hébergé voit et appelle `vibeos:*` sans config manuelle | Non (config) |
| **1** | Fork ciblé : **désactiver** les Read/Write/Edit natifs de l'adaptateur et les **router vers** `vibeos:fs.read`/`fs.write`/`memory.query` de `vibed` — toute action fichier passe par la politique + l'audit | Oui (adaptateur) |
| **2** | **Mode auto gouverné** : remplacer le prompt de permission ACP par la décision de `vibed`. Un appel classé `Allow` (T0/T1) s'exécute **sans prompt** ; un appel `RequireApproval` (T2/T3) **n'est jamais auto-accepté** — il suit le flux d'approbation existant (`vibed` renvoie `pending`, l'humain approuve hors bande). Le mode auto **consulte** la politique, il ne la remplace jamais | Oui (adaptateur) |
| **3** | Intégrations éditeur : capture du raisonnement (ADR-012) visible dans Zed, indicateurs de tier, journal de session | Oui (adaptateur) |

**INVARIANTS (repris de la demande, non négociables).**
1. Le **plancher T2/T3 n'est jamais levé**. Le mode auto ne saute le prompt ACP
   **que** pour ce que `policy.evaluate()` a déjà classé `Allow` (T0/T1). **Aucun
   chemin de code du mode auto ne touche `approval.rs`** — l'approbation reste
   entièrement du ressort de `vibed`.
2. **Aucune auto-approbation** : un agent ne peut jamais s'auto-approuver, côté
   éditeur comme côté terminal (store root-only + denylist, cf. F3).
3. Toute **nouvelle surface d'écriture** ⇒ mise à jour de `THREAT-MODEL.md` dans le
   même commit.
4. Rien décrit au présent tant que non implémenté **et testé**.

**Conséquences.**
- (+) Un seul point de gouvernance (`vibed`) pour l'agent, qu'il tourne en terminal
  ou dans l'éditeur — même politique, même audit, même approbation.
- (+) Le mode auto améliore l'ergonomie **sans** affaiblir la sécurité : il ne fait
  que déléguer la décision de prompt à un moteur qui refuse déjà le T2/T3 sans humain.
- (−) On maintient un **fork** d'un adaptateur amont qui évolue vite : le fork doit
  rester **minimal et chirurgical** (points d'interception précis), rebasable, et son
  périmètre documenté ici pour survivre aux montées de version.
- (−) Le schéma des outils/permissions de l'adaptateur n'est **pas contractuel** ;
  d'où l'investigation préalable (§ Structure de l'adaptateur).

### Structure de l'adaptateur (investigation — 2026-07-13)

Cartographie du code réel de `@zed-industries/claude-code-acp` (v0.58.1, TypeScript/
ESM, Node ≥ 22, tests Vitest), menée **avant tout patch**.

**Fait architectural central.** L'adaptateur **n'implémente ni n'exécute aucun outil**.
C'est un pont : il lance le **binaire natif du Claude Agent SDK** en sous-process
(via `query()`, `pathToClaudeCodeExecutable`, `acp-agent.ts:4401`) et traduit entre
ACP (Zed ⇄ agent) et le SDK. Les outils natifs **`Read`/`Write`/`Edit`/`Bash`/`Grep`/
`Glob`** tournent **dans ce sous-process**, activés en **preset** (`{ type: "preset",
preset: "claude_code" }`, `acp-agent.ts:4339`), jamais énumérés. `src/tools.ts` ne fait
que du **rendu ACP** (pas d'exécution). **Conséquence** : on ne « remplace » pas
l'implémentation de `Read` ici — elle n'y est pas. On intercepte aux **deux seams que
l'adaptateur possède**.

**Seam 1 — le hook de permission `canUseTool`** (`acp-agent.ts:3546`, branché
`acp-agent.ts:4381`). Chaque appel d'outil non déjà tranché y passe (en mode
`default`/« Manual », tout passe). Il renvoie `{ behavior: "allow", updatedInput }` ou
`{ behavior: "deny", message }`, et **peut réécrire l'input**. Précédent de
court-circuit déjà présent : `if (currentModeId === "bypassPermissions") return
{ behavior: "allow" }` (`acp-agent.ts:3681`) — exactement le patron du « la politique
dit Allow → pas de prompt ». Sinon → `requestPermissionFromClient()`
(`acp-agent.ts:3471`) → prompt ACP `session/request_permission`.

**Seam 2 — l'assemblage des options dans `createSession()`** (`acp-agent.ts:4232`) :
`mcpServers` (`:4376`), `disallowedTools` (`:4406`), `tools`/preset (`:4339`),
`systemPrompt` (`:4287`), `permissionMode` (`:4380`).

**Points d'attention.** Le mode `auto` natif est un **classifieur par modèle**, PAS un
moteur de politique — inutilisable tel quel. La config vient des **settings de Claude
Code** (`~/.claude/settings.json`, `.claude/settings.json`, `.mcp.json` ; `settingSources:
["user","project","local"]`, `acp-agent.ts:4352`) : clés `permissions.defaultMode`,
`permissions.allow`/`deny`. Les serveurs **MCP** sont acceptés depuis trois sources
fusionnées dans `mcpServers` et surfacés via la machinerie MCP de Claude Code
(`mcp__<serveur>__<outil>`), déjà gouvernés par le même `canUseTool`.

**Plan de fork VibeOS (chirurgical, deux fonctions).**
- **Couche 0 (aucun code)** : enregistrer le serveur MCP `vibeos:*` là où le
  sous-process Claude Code le lit (`.mcp.json` / `~/.claude.json` déjà livré) — l'agent
  hébergé voit `vibeos:*` sans patch. Config Zed dans `/etc/skel/.config/zed/`.
- **Couche 1** — dans `createSession()` : ajouter `"Read"`, `"Write"`, `"Edit"` (etc.)
  à `disallowedTools` (`:4406`) et garantir `vibeos:*` dans `mcpServers` (`:4376`), +
  un `systemPrompt.append` (`:4287`) qui oriente le modèle vers `vibeos:fs.read`/
  `fs.write`/`memory.query`. Toute action fichier passe alors par `vibed`.
- **Couche 2** — forker **une seule fonction**, `canUseTool` (`:3546`) : en tête,
  interroger `vibed` (classer T0–T3) ; `Allow` (T0/T1) → `{ behavior: "allow" }` **sans
  prompt** ; `RequireApproval` (T2/T3) → laisser tomber dans le
  `requestPermissionFromClient` existant (prompt/approbation **toujours** montré).
  `permissionMode` maintenu à `default` pour que tout passe par `canUseTool`.
  **INVARIANT** : ce chemin **ne touche pas `approval.rs`** et ne décide jamais lui-même
  d'un T2/T3 — il ne fait que *lire* la classification de `vibed`.
  - **Primitif requis côté `vibed`** : un outil MCP **T0 `policy.check(tool, target)`**
    qui renvoie la `Decision` (`allow`/`deny`/`require_approval`) + le tier **sans
    exécuter, sans consommer de grant, sans toucher `approval.rs`** — c'est ce que
    `canUseTool` interroge pour décider « prompt ou pas ». **C'est un indice** : la
    vraie application reste à l'exécution dans `vibed` (denylist, confinement,
    plancher T2/T3), donc un `policy.check` imparfait ne peut jamais laisser passer
    un T2/T3 sans approbation — au pire l'éditeur montre/omet un prompt à tort, mais
    `vibed` gate toujours l'appel réel. Groundwork couche 2, implémentable côté vibed
    indépendamment du fork.
  - **« Indice » n'autorise pas « faux » (corrigé 2026-07-15).** L'indice et le vrai
    chemin dérivaient chacun de leur côté l'unité systemd passée à la politique, et
    avaient divergé : (a) `policy.check` **ne canonicalisait pas** le nom d'unité, si
    bien qu'un `svc.restart` sur `sshd` répondait `require_approval` alors que le
    démon **refuse** (la règle liste `sshd.service` ; c'est exactement le contournement
    par suffixe que la canonicalisation du vrai chemin bloque déjà) ; (b) le vrai
    chemin retombait sur l'argument `unit` **brut de n'importe quel outil**, donc une
    donnée non canonicalisée et contrôlée par l'appelant atteignait le contexte de
    politique sur des outils sans unité (`fs.read`…), là où l'indice mettait `None`.
    Les deux dérivent désormais d'un **prédicat unique** (`mcp::unit_bearing`) et le
    même `validate_unit_name`. La borne de sécurité était intacte (le vrai appel
    refusait bien), mais un indice **plus laxiste que la réalité** trompe la couche 2
    et n'a aucune raison d'être.

**Build/run** : `tsc` → `dist/index.js` (`start`), `vitest` pour les tests ; entrée
`src/index.ts` → `runAcp()` ; `src/lib.ts` réexporte `ClaudeAcpAgent`/`runAcp` (le fork
peut consommer la classe en bibliothèque plutôt que patcher en place). Amont Apache-2.0.

**Forme de fork retenue (vérifiée sur le source) : un paquet d'EXTENSION qui
patche le prototype de `canUseTool`, pas un patch de source ni un sous-classement.**
Vérifié dans le clone :
- `export class ClaudeAcpAgent` (ligne 937) ; `constructor(client, logger?)`, champs
  `sessions`/`client`/`clientCapabilities` **publics** ; `canUseTool(sessionId):
  CanUseTool` **méthode publique** (ligne 3546).
- MAIS `createSession` est **`private`** (ligne 4232) — non surchargeable ; et
  `runAcp()` (ligne 6349) **construit la classe de base en interne** (`new
  ClaudeAcpAgent(new ClientConnection(...))`) avec des internes **non exportés**
  (`ClientConnection`, `methods`, `acpAgent`, `ndJsonStream`, `runPromptWithCancellation`).
  Donc un sous-classement ne peut pas s'injecter dans `runAcp` sans recopier ce câblage.

Approche verrouillée, minimale, n'utilisant **que** les symboles exportés
(`ClaudeAcpAgent`, `runAcp`) : **patcher `ClaudeAcpAgent.prototype.canUseTool`** avant
d'appeler `runAcp()` — on wrappe l'original (`const base =
orig.call(this, sid); return async (t, input, ctx) => { … policy.check … return
base(t, input, ctx) }`). Aucun code amont recopié, **rebasable** par bump de
dépendance. La désactivation des outils natifs (couche 1) passe par la **config**
(`permissions.deny` + `CLAUDE_CONFIG_DIR` propre à la session Zed — décision
**Zed-only**, le terminal garde ses outils), pas par un override de `createSession`
(privé). Le paquet vit dans `zed/` (hors TCB `vibed`).
La classification vient de l'outil T0 `vibeos:policy.check` (livré). L'implémentation
complète (dont les fonctionnalités éditeur innovantes) + le test d'intégration en
session Zed réelle restent à faire.

**La couche 1 (désactiver les outils natifs) peut être largement CONFIG, pas un
fork.** L'adaptateur lit les règles `permissions.deny` de Claude Code (`settings.ts`
§12-22 : `"Read"`, `"Write"`, `"Edit"`, `"Bash(...)"`…), consommées côté SDK. Ajouter
`permissions.deny: ["Read","Write","Edit"]` dans les settings Claude Code désactive
donc les outils fichiers natifs **sans fork**, l'agent étant orienté vers `vibeos:fs.*`
par le `mcpServers` (déjà livré) + un `systemPrompt.append`. Le **fork ne reste donc
requis que pour la couche 2** (le pont policy dans `canUseTool`).
**Décision de design ouverte (à trancher côté humain)** : ce `permissions.deny`
s'appliquerait via les settings Claude Code **partagés** — il désactiverait les outils
natifs aussi pour **Claude Code en terminal**, pas seulement dans Zed. Gouvernance
globale cohérente (tout passe par `vibed`) **ou** portée limitée à l'éditeur ? Ce choix,
et la spec des fonctionnalités innovantes, conditionnent l'écriture de la couche 1/2.

## ADR-015 — Chaîne d'approvisionnement npm de l'extension Zed (`vibeos-claude-acp`) — *plan, cible couche 1/2*

**Statut** : **étage de build livré & vérifié, expédition gardée off** (2026-07-13).
L'étage `zed-agent-builder` du `Containerfile` implémente cette discipline et
**construit réellement** (`npm ci --ignore-scripts` → bundle esbuild → smoke ACP
dans le build) ; il reste **désactivé par défaut** (`ARG WITH_ZED_AGENT=0`, §6)
jusqu'à la validation E2E. Le plan ci-dessous fixe la discipline ; les points
réalisés sont marqués.

**Contexte.** L'extension `zed/vibeos-claude-acp` dépend de l'amont npm
`@agentclientprotocol/claude-agent-acp` (≈ 148 dépendances transitives). npm est
une surface supply-chain (typosquatting, scripts `postinstall` malveillants,
dépendance compromise en amont). Le **TCB de VibeOS (`vibed`) reste Rust à
dépendances minimales** ; l'extension vit **hors du TCB** (`zed/`), mais finira
par être livrée dans l'image — elle doit donc suivre la même hygiène que les
autres installs npm du `Containerfile`.

**Décision (même discipline que le `Containerfile`).**
1. **Lockfile commité** (`package-lock.json`, épinglé) : versions exactes de
   **toute** la chaîne transitive + **hash d'intégrité SHA-512** par paquet
   (amont épinglé `0.58.1`). Déjà fait (retiré du `.gitignore`).
2. **`npm ci --ignore-scripts`** au build : install **reproductible depuis le
   lockfile** (échoue si le lockfile diverge de `package.json`) et **sans
   scripts de cycle de vie** — exactement le `--ignore-scripts` déjà utilisé par
   le `Containerfile` pour les CLIs IA.
3. **Vérification d'intégrité** : `npm ci` compare le champ `integrity` (SHA-512)
   du lockfile aux tarballs du registre — toute altération fait échouer l'install.
4. **Build isolé + bundle (✅ livré, gardé)** : étage multi-stage dédié
   `zed-agent-builder` (comme `quickshell-builder`/`vibed-builder`) — `npm ci
   --ignore-scripts` (chaîne complète, reproductible du lockfile), puis **esbuild
   bundle** `src/` vers **un unique `dist/vibeos-claude-acp.mjs` autonome**.
   **Décision affinée vs le plan initial** : on **ne ship pas de `node_modules`**
   du tout — seul le bundle (≈ 1,9 Mo) est copié dans l'image (`/usr/lib/vibeos/
   zed-agent/`), jamais les ~148 paquets transitifs, les sources TS ni l'outillage
   dev. Meilleure hygiène que « dist/ + deps prod » : la surface npm reste
   entièrement **build-time**. `esbuild` vient d'un *optional dependency* binaire
   (aucune exception `npm rebuild` nécessaire). Le build **boote le bundle
   headless** et exige une réponse ACP `initialize` avant de le copier (même garde
   « prouve que ça tourne » que les CLIs shippées).
5. **Bumps revus** : monter la version amont = un commit revu (rebase du fork, la
   surface de fork étant volontairement minimale — un patch de prototype, ADR-014) ;
   `npm audit` sur la chaîne à chaque bump. **Mesuré 2026-07-13** : `npm audit
   --omit=dev` (chaîne shippée) = **0 vulnérabilité** ; les 5 advisories restantes
   sont **dev-only** (serveur de dev esbuild, vitest) et n'entrent jamais dans l'image.
6. **Pas livré tant que non validé (✅ gardé off)** : l'étage existe mais n'est
   activé que par `--build-arg WITH_ZED_AGENT=1`. Par défaut l'image ne contient
   **pas** l'extension (un marqueur `NOT-INSTALLED.txt` documente l'état) ; le
   builder npm est alors **hors du graphe** (podman le saute — coût CI nul). On
   ne ship pas ~148 paquets non éprouvés dans l'image immuable avant la validation
   E2E (voir `BLOCKERS.md`). **Décision assumée, pas un oubli — à NE PAS « corriger »
   par défaut.** Un `WITH_ZED_AGENT=1` par défaut (ou toute PR qui l'active) est une
   **régression de sécurité** tant que le Tier B Zed n'est pas validé : il ferait
   entrer une surface npm non éprouvée dans l'image immuable. Le flag ne passe à `1`
   qu'accompagné d'une preuve de Tier B (session Zed réelle, checklist
   `zed/vibeos-claude-acp/scripts/e2e-zed.sh` verte).

**Conséquences.** Chaîne npm **reproductible, vérifiable et build-time seulement**
(le bundle est le seul artefact shippé), alignée sur la discipline de l'OS ;
l'extension reste hors TCB sous la même hygiène ; le coût est un lockfile à
maintenir et un étage de build gardé. **Vérifié** : les deux chemins
(`WITH_ZED_AGENT=0` → marqueur, `=1` → bundle + smoke ACP dans le build)
construisent (podman, 2026-07-13).

---

## ADR-016 — `pkg.install` (T2) sur OS immuable : allowlist de cibles AVANT tout backend — *décision : reporté (stub), allowlist non tranchée*

**Statut** : **backend reporté volontairement (2026-07-14)** — `pkg.install` reste
un **stub** (`requires_approval`, n'installe rien). Cet ADR documente *pourquoi*,
en appliquant la même exigence qu'à `svc.restart` (ADR/politique) : **pas
d'exécution réelle sans une allowlist de cibles**, pas seulement une validation de
syntaxe.

**Contexte — l'installation de paquet n'est pas triviale sur un OS immuable.**
VibeOS est bootc/OSTree : la racine `/usr` est **en lecture seule** au runtime.
`dnf install` classique n'existe pas. Trois voies réelles, aux sémantiques très
différentes :
1. **`rpm-ostree install <pkg>`** — *package layering* : modifie le **déploiement**
   (une nouvelle image dérivée), **exige un reboot** pour prendre effet, et
   persiste à travers les mises à jour. C'est un changement d'**état système
   durable**, pas une action locale réversible.
2. **`toolbox`/`distrobox` + `rpm-ostree`-free** : conteneur mutable pour les
   outils de développement de l'utilisateur — **hors** de l'image immuable, pas un
   changement système. C'est la voie recommandée pour « installer un paquet »
   côté utilisateur (docs/ECOSYSTEM.md : mise + distrobox).
3. **overlay transient** (`rpm-ostree install --apply-live` / transient) — fragile,
   non persistant, cas limites nombreux.

**La question à trancher (comme le point « allowlist » de `svc.restart`).** *Quels
paquets, depuis quels dépôts, un agent peut-il installer ?* Sous-questions non
résolues :
- **Layering vs conteneur** : un agent doit-il pouvoir *layerer* dans l'image
  immuable (change le système, reboot) ou seulement installer dans un
  `distrobox` (n'affecte pas le système gouverné) ? Le second est bien plus sûr
  et cohérent avec l'immutabilité, mais alors `pkg.install` (T2 système) n'est
  peut-être **pas le bon outil** — ce serait un `container.pkg.install` (T1 ?).
- **Allowlist de paquets** : globs sur les noms (`[rule.packages].allowed/denied`,
  même patron que `[rule.paths]`/`[rule.services]`), ou allowlist de **dépôts**
  signés seulement, ou les deux ?
- **Dépôts** : seulement les repos Fedora/RPM Fusion épinglés et signés de l'image,
  jamais un repo arbitraire fourni par l'agent (sinon = vecteur supply-chain).
- **Reboot** : `pkg.install` par layering ne « marche » qu'au prochain boot —
  quelle UX/sémantique d'audit pour une action à effet différé ?

**Décision.** **Ne pas implémenter le backend cette nuit.** La réponse à « quelle
allowlist » n'est **pas claire** (le choix layering-vs-conteneur change la nature
même de l'outil et son tier). Implémenter une exécution `rpm-ostree` réelle sans
cette allowlist violerait l'invariant « aucune nouvelle capacité d'exécution réelle
sans allowlist de cibles ». Le stub reste ; `pkg.install` (T2) est déjà **refusé
sans approbation humaine** par la politique, donc rien n'est exposé.

**Chemin quand ce sera repris (Phase 4).**
1. Trancher **layering (système, T2) vs distrobox (utilisateur, T1)** — probablement
   les deux outils distincts, pas un seul `pkg.install` ambigu.
2. Ajouter un champ `package` à `CallContext` (comme `path`/`service`) et une
   sous-table `[rule.packages]` (allowlist de noms + allow-list de **dépôts signés**
   uniquement), évaluée **avant** le floor T2 — un paquet/dépôt hors allowlist =
   `Deny`, pas même une file d'approbation (exactement comme `svc.restart`).
3. Backend `rpm-ostree` par **chemin absolu, env vidé, nom de paquet validé**
   (anti-injection, `--`), sémantique de reboot explicite dans le retour et l'audit.
4. Test sur la politique livrée : un paquet hors allowlist / un dépôt non signé →
   `Deny` ; un paquet allowlisté → `RequireApproval`.

**Conséquences.** (+) On ne ship pas une capacité d'installation système à demi
gouvernée. (+) La cohérence avec `svc.restart` (allowlist de cibles avant le floor)
est préservée pour le jour de l'implémentation. (−) `pkg.install` reste non
fonctionnel — mais il l'était déjà (stub), et l'alternative utilisateur (distrobox)
existe hors gouvernance système.

## ADR-017 — Navigateur piloté par l'IA : gouverner la capacité, ne pas forker Chromium — *décidé (2026-07-15) : option C, paramètres tranchés par Micka*

**Statut** : **DÉCIDÉ le 2026-07-15 — option C** (livrer la capacité gouvernée, sans forker Chromium). Les 4 paramètres ouverts ont été tranchés par Micka ; voir « Décisions » en fin d'ADR. Implémentation à faire. Demande initiale : intégrer
[BrowserOS](https://github.com/browseros-ai/BrowserOS) à l'OS pour que l'IA
navigue sur internet. Aucun code écrit : la question ouvre une **capacité
d'exécution nouvelle** (réseau + DOM + sessions authentifiées), donc elle passe
par la question habituelle — *quelle allowlist de cibles* — avant toute ligne.

### Ce qu'est réellement BrowserOS (faits vérifiés, 2026-07-15)

| | |
|---|---|
| Nature | **Fork Chromium construit depuis les sources** — 381 fichiers de patch. Ni Electron, ni extension, ni wrapper |
| Licence | **AGPL-3.0** (+ BSD-3 pour les patches ungoogled). Aucun EULA propriétaire |
| Distribution | `.deb`, AppImage, `.dmg`, `.exe`. **Aucun RPM, jamais** (0 sur ~80 releases). **Linux = x86_64 uniquement** |
| Taille | 262 Mo (.deb) / 354 Mo (AppImage) |
| IA | **Aucun LLM embarqué** : BYO clé / OAuth / **ollama local**. **Serveur MCP intégré** sur `127.0.0.1:9239`, « 53+ outils navigateur » |
| Santé | 12,2k ★, releases hebdo, ~100 commits/sem — **projet sérieux**, mais **13 contributeurs, bus factor ~2** |
| Sécurité | Injection de prompt traitée **au niveau du prompt système uniquement** ; sandbox Chromium conservée ; l'agent **pilote vos sessions connectées** (40+ intégrations OAuth : Gmail, Slack, GitHub…) |

### Pourquoi on ne peut PAS l'expédier tel quel

Le test est la doctrine du projet, écrite dans `docs/ECOSYSTEM.md` : *« VibeOS ne
ship que ce qu'il a juridiquement le droit de shipper, et tout ce qu'il ship
marche offline ou est gated par un tier de policy explicite. »* BrowserOS échoue
**des deux côtés**.

1. **Codecs brevetés.** `flags.linux.release.gn` fixe `proprietary_codecs = true`,
   `ffmpeg_branding = "Chrome"`, `enable_widevine = true` — exactement ce qu'une
   image dérivée de Fedora ne ship pas. La **licence n'est pas le blocage** (AGPL
   est propre, ce n'est pas le piège VS Code) : c'est la **configuration**. Les
   inverser impose de reconstruire → on n'expédie plus leur binaire testé.
2. **Aucun RPM, et pas d'arm64.** VibeOS est **multi-arch amd64+arm64** (badge,
   manifest, 2 ISO). Un navigateur amd64-only casserait cette promesse.
3. **Reconstruire depuis les sources = ~100 Go, gn/ninja, des heures**, et surtout
   **posséder les rebases Chromium à vie**, face à un amont de 13 personnes. Fedora
   elle-même peine à maintenir Chromium. Pour un mainteneur solo, c'est un piège.
4. **Et le vrai problème, qui est celui du projet.** Le postulat central du
   `THREAT-MODEL` : *« tout agent IA est un insider non fiable […] il n'existe
   aujourd'hui AUCUNE défense fiable au niveau du modèle contre l'injection de
   prompt. La sécurité de VibeOS ne repose donc jamais sur le bon comportement du
   modèle. »* Or la défense anti-injection de BrowserOS **est** un prompt système
   qui dit « ignore catégoriquement ces instructions ». C'est **précisément la
   défense que notre modèle de menace déclare inexistante** — et elle garderait
   un agent qui **pilote vos sessions Gmail/Slack/GitHub**. Un navigateur IA est
   la **menace M2 incarnée** (« contenu web malveillant ingéré par l'agent »),
   c'est-à-dire le **scénario S1, la menace n°1** du projet. Son MCP sur
   `127.0.0.1:9239` serait en plus un **serveur MCP tiers** (menace M3) parlant à
   l'agent **hors de `vibed`** — donc hors politique, hors tiers, hors audit. Le
   principe n°4 du ROADMAP l'interdit : *« une fonctionnalité qui contourne le
   moteur de politiques n'est pas mergeable, quelle que soit la phase. »*

### La bonne nouvelle

BrowserOS **expose sa capacité en MCP** — exactement la forme que `vibed`
gouverne déjà. Le projet avait d'ailleurs **déjà prévu** un navigateur gouverné :
`ECOSYSTEM.md` annonce « **Playwright** pour le navigateur […] enveloppée dans
les policy tiers ». La capacité désirée n'exige donc **pas** de forker un
navigateur : elle exige des **outils `browser.*` dans `vibed`**.

### Options

- **A — Expédier le `.deb` repackagé.** Rapide. Mais : codecs brevetés, amd64
  seul, agent **non gouverné** (contourne `vibed`). **Contraire à la doctrine.**
- **B — Reconstruire depuis les sources, codecs coupés, MCP mis derrière `vibed`.**
  Juridiquement et techniquement faisable ; coût réel : ~100 Go/build, rebases
  Chromium à vie, arm64 à porter soi-même. **Échelle d'une distro, pas d'un solo.**
- **C — Ne pas forker : livrer la CAPACITÉ, gouvernée** *(recommandé)*. Des outils
  `browser.*` exposés **par `vibed`**, pilotant un navigateur (Playwright/CDP) :
  chaque action porte un tier, chaque page lue est une **entrée hostile**, tout est
  audité. Pas de fork, multi-arch préservé, pas de codecs brevetés — et l'agent
  n'obtient **jamais** une capacité hors politique. On peut s'inspirer librement de
  la surface d'outils de BrowserOS (AGPL, lisible) sans en hériter la dette.

### Ce qui doit être tranché AVANT tout code (option C)

1. **Quelle allowlist de cibles ?** Domaines autorisés par défaut ? `*` est un
   default-deny renversé — inacceptable tel quel. Une allowlist de domaines, ou
   une approbation par domaine à la première visite ?
2. **Quels tiers ?** Proposition à valider : `browser.read`/`browser.screenshot`
   **T0** ; `browser.navigate` **T1** *(mais naviguer, c'est ingérer du contenu
   hostile — peut-être T1 seulement sur l'allowlist, T2 hors liste)* ;
   `browser.click`/`browser.fill`/`browser.submit` **T2** — **agir en votre nom
   sur vos sessions authentifiées, c'est du T2 par nature**.
3. **Sessions authentifiées : oui/non ?** BrowserOS tire sa puissance de vos
   comptes connectés. C'est aussi ce qui rend une injection catastrophique. Profil
   navigateur **vierge et jetable** par défaut ?
4. **Egress.** L'unité agent est déjà `IPAddressDeny=any` + allowlist par hôte. Un
   navigateur qui atteint « internet » **contredit** cette allowlist. À réconcilier.

**Rien ne sera codé tant que 1 et 3 ne sont pas tranchés** : ce sont des décisions
de gouvernance, pas d'implémentation.

### Décisions (Micka, 2026-07-15)

**Option retenue : C** — livrer la capacité gouvernée, sans forker Chromium.

| # | Question | Décision |
|---|---|---|
| 1 | Allowlist de domaines | **Allowlist + approbation T2 hors liste.** Domaines de confiance navigables librement ; tout autre domaine déclenche une approbation T2 au premier accès, puis est mémorisé. Le default-deny est préservé. |
| 2 | Tiers des outils `browser.*` | **Tout en T1 sauf les formulaires.** `read`/`screenshot`/`navigate`/`click`/`fill` = T1 ; seule la **soumission de formulaire** est T2. |
| 3 | Sessions authentifiées | **Profil persistant, connexions autorisées.** L'agent peut rester connecté aux sites. — ⚠️ **SUPERSÉDÉ par ADR-022 : profil éphémère sans identifiants** (résiduel d'identité stockée neutralisé ; action silencieuse subsiste — voir ADR-022). |
| 4 | Egress | **Navigateur dans sa propre unité systemd durcie**, dont l'allowlist d'egress est **dérivée de l'allowlist de domaines** (point 1). La politique décide, systemd applique au niveau réseau. |

### ⚠️ Résiduel ACCEPTÉ par l'opérateur — ne pas l'enterrer

> **MIS À JOUR par ADR-022 — résiduel d'IDENTITÉ neutralisé, résiduel d'ACTION
> SILENCIEUSE subsistant.** Ce résiduel vient de la **combinaison** des décisions 2
> (clics T1) et 3 (sessions persistantes). ADR-022 supersède la décision 3 par un
> **profil éphémère sans identifiants** : la moitié « identité stockée » est
> neutralisée (plus de session persistante à chevaucher). MAIS — revue Fable 5 —
> (a) la décision 2 est confirmée, donc l'**action silencieuse subsiste** : une
> action destructrice qui n'exige **aucune auth** tire toujours, anonymement, sur
> un domaine allowlisté (`navigate` GET à effet de bord, bouton POST-via-`onclick`
> = `click` T1) ; (b) l'**agent** (pas le profil) peut **réinjecter** une identité
> via `browser.fill` ; (c) l'éphémérité introduit un **résiduel de ré-auth**. Voir
> ADR-022 pour le détail. Le paragraphe ci-dessous est conservé comme trace du
> raisonnement d'origine.

Les décisions **2 et 3 sont chacune défendables ; leur COMBINAISON ouvre un trou
qu'aucune des deux n'ouvre seule**, et il doit être écrit ici plutôt que découvert
plus tard :

- prises séparément : *sessions persistantes + clics T2* → l'opérateur valide
  chaque geste ; *profil jetable + clics T1* → aucune identité à détourner ;
- **prises ensemble** : l'agent reste **connecté aux comptes de l'opérateur** ET
  **clique sans approbation**. Une page piégée sur un domaine **allowlisté** peut
  donc lui faire exécuter une action **en son nom, silencieusement**.

Le cas n'est pas théorique : l'allowlist contiendra GitHub (l'agent doit lire
issues et docs), et le `THREAT-MODEL` cite **littéralement** les *« issues
GitHub »* comme vecteur M2. Une issue piégée → « Settings → Supprimer le dépôt » →
l'agent **clique** (T1, aucune approbation), et « Supprimer » est un **bouton**,
pas un formulaire : la seule chose classée T2 ne s'applique pas.

**Alternative proposée et écartée par Micka** : *le tier suit l'IDENTITÉ, pas
l'action* — toute interaction sur un domaine pour lequel le profil porte une
session connectée passe en T2, sinon T1 (détectable via les cookies d'auth de
l'origine courante). Elle gardait les sessions persistantes ET la fluidité sur les
~90 % du web où l'on n'est pas connecté. **Micka a choisi la fluidité maximale et
assume le risque** (2026-07-15). Ce paragraphe existe pour que ce soit un **choix
tracé**, pas un oubli — et pour qu'il puisse être révisé sans archéologie.

**Ce que le plancher T2/T3 garantit toujours** : ce résiduel ne lève **rien** du
plancher système. Un agent ne peut toujours pas installer un paquet, redémarrer un
service ou écrire hors du home de l'appelant sans approbation. Le risque est
**circonscrit aux actions web menées avec l'identité de l'opérateur**, sur les
domaines qu'il a lui-même allowlistés.

### Notes d'implémentation (à venir)

- **Nouvelle contrainte de politique** `[rule.domains]` (allowlist), sœur de
  `[rule.paths]` et `[rule.services]` — c'est elle qui rend le point 1 applicable
  **avant** le plancher de tier, comme l'allowlist d'unités de `log.read`.
- **Pas de fork** : on s'inspire librement de la surface d'outils de BrowserOS
  (AGPL, lisible) sans hériter de sa dette Chromium, de ses codecs brevetés ni de
  son absence d'arm64.
- **Le contenu d'une page lue est une ENTRÉE HOSTILE**, jamais une instruction :
  c'est le postulat central du modèle de menace, et il ne se négocie pas — quel
  que soit le tier des clics.

## ADR-019 — Le bac à sable par outil vise la mauvaise frontière : séparation de privilèges, pas confinement de threads — *proposé (2026-07-16), à trancher*

**Statut** : **PROPOSÉ**, non tranché. Corrige un plan inscrit dans la ROADMAP et le
`THREAT-MODEL` depuis l'origine. Aucun code écrit. Découle d'ADR-017 : `browser.*` est
la première charge qui rend le sujet urgent, mais le constat vaut pour **tous** les
outils.

### Le plan actuel, et pourquoi il ne peut pas marcher

`THREAT-MODEL.md` : *« outils : in-process en v0.1 — sandbox seccomp/Landlock : Phase 3 »*.
`ROADMAP.md` §Phase 4 : *« la sandbox par outil (seccomp/Landlock) est à zéro »*.

Le plan implicite est : *les outils tournent dans `vibed` ; on ajoutera seccomp et
Landlock autour de leur exécution.* Comme `vibed` est un daemon tokio, « autour de leur
exécution » signifie **confiner un thread**. Trois raisons indépendantes le condamnent —
et aucune ne se règle par une montée de version.

**1. Un thread n'est pas une frontière de sécurité — dixit l'auteur de Landlock.**
Mickaël Salaün (l0kod), auteur du LSM Landlock, [rust-landlock#37](https://github.com/landlock-lsm/rust-landlock/issues/37) :

> *« A Linux thread is (mostly) a unit of scheduling, but **should not be considered a
> security boundary**, unlike processes. […] Anyway, if a thread is considered
> potentially malicious, **the whole process should be considered potentially
> malicious**, and then the restrictions (e.g., Linux capabilities, DAC/UID/GID,
> seccomp, Landlock) should be enforced **on this process**. »*

Le rapporteur d'origine décrit l'attaque : *« all of the threads share an address space
[so] if the attacker can execute arbitrary code they could just **hijack another
unsandboxed thread** »*.

**2. Un bac à sable filtre des appels système, pas la mémoire.** C'est le point qui
tue, et il est propre à VibeOS : **`vibed` EST le moteur de politiques**. Une RCE dans
le parseur d'un outil, sur un thread tokio parfaitement confiné par Landlock et seccomp,
partage l'espace d'adressage du moteur. Elle réécrit le booléen qui décide d'un tier,
ou la tête de la chaîne d'audit — **sans émettre un seul appel système**. Le bac à sable
n'est pas sur le chemin. Il ne peut pas l'être.

**3. Les filtres sont irrévocables, et l'ordonnanceur de tokio vole le travail.** Un
filtre seccomp ne se retire pas (il n'existe aucune API de retrait ; c'est la prémisse
même de `no_new_privs`). Confiner un worker tokio pour un appel d'outil le laisse
**définitivement dégradé pour toutes les tâches futures** qu'on lui planifiera —
handler JSON-RPC et écrivain de la chaîne d'audit compris. Il n'y a pas de
« dé-confiner après l'appel ».

### Ce que les alternatives naïves coûtent, vérifié

**`fork()` depuis le runtime tokio, puis confiner l'enfant : non.** L'enfant n'hérite
que du thread appelant, mais **tout l'espace d'adressage** — dont des mutex détenus par
des threads qui n'existent plus, y compris les verrous internes de l'allocateur. La
doc de `std` est explicite pour `pre_exec` : *« a very constrained environment where
normal operations like `malloc` […] or acquiring a mutex are not guaranteed to work »*.
On peut y déplacer des descripteurs et `exec`, rien de plus. **systemd a réécrit son
chemin de spawn exactement pour ça** ([ARCHITECTURE](https://systemd.io/ARCHITECTURE/)) :
`posix_spawn()` vers un binaire **`systemd-executor`** séparé, *« in order to avoid
excessive processing after a fork() but before an exec() »*.

**`systemd-run --scope` : ne peut rien confiner. Mesuré ici, pas supposé :**

| Propriété | `--scope` | `--service` |
|---|---|---|
| `ProtectSystem=strict` | **REJETÉ** | accepté |
| `NoNewPrivileges=yes` | **REJETÉ** | accepté |
| `PrivateTmp=yes` | **REJETÉ** | accepté |
| `SystemCallFilter=@system-service` | **REJETÉ** | accepté |
| `IPAddressDeny=any` | accepté | accepté |

La coupure suit exactement la frontière des pages de manuel : un **scope** n'accepte que
les propriétés cgroup de `systemd.resource-control(5)` ; **toute** directive de
confinement de `systemd.exec(5)` est refusée. C'est logique — un scope enregistre un
processus déjà lancé, il n'y a pas d'`exec` que systemd puisse envelopper. **Donc :
service transitoire, jamais `--scope`.**

### Ce que Landlock ne peut PAS faire, et qui vise `browser.*`

- **Aucun filtrage UDP** jusqu'à l'ABI 10 (non publiée). Un parseur HTML compromis
  exfiltre en UDP/53, Landlock pleinement engagé. **Landlock ne bloque pas le DNS.**
- **Aucun filtrage par IP** : les règles réseau sont **par port** (ABI 4+). TCP/443 vers
  un attaquant et TCP/443 vers ton intranet sont la même règle.

Landlock seul n'est donc **pas** une histoire de confinement pour un navigateur. Il faut
`IPAddressDeny=any` (cgroup/BPF, qui couvre l'UDP) — c'est-à-dire un **service systemd**.

### Le précédent est unanime

Aucun système sérieux n'isole du parsing hostile par un mécanisme de thread :

| Système | Frontière | Mécanisme |
|---|---|---|
| **OpenSSH** ≥9.8 | processus | `sshd` → **fork+exec** `sshd-session` → `sshd-auth`, qui *« will sandbox and/or chroot itself and drop privilege before processing any network traffic »* |
| **systemd** ≥254 | processus | `posix_spawn` → `systemd-executor`, qui applique le sandbox **avant** d'exec |
| **Firefox** | processus | ForkServer — sûr **parce qu'il est mono-thread par construction**, forké avant XPCOM |
| **Chromium** | processus | zygote |

Et l'argument le plus court est celui de Chromium, la
[**Rule of 2**](https://chromium.googlesource.com/chromium/src/+/HEAD/docs/security/rule-of-2.md) :

> *« Pick no more than 2 of: untrustworthy inputs; unsafe implementation language; high privilege. »*

`browser.*` **in-process, c'est 3 sur 3** : entrée hostile par construction (HTML), code
non sûr (un moteur de rendu traîne de l'`unsafe` et des shims C — Rust n'achète pas
cette jambe), privilège élevé (le moteur de politiques). La mitigation prescrite est
exactement celle-ci : traiter la donnée risquée dans un **processus utilitaire de faible
privilège** et repasser le résultat par IPC.

**Anthropic répond pareil pour Claude Code** : bubblewrap, **au niveau processus**. Et
leur doc note que `Read`/`Edit`/`WebFetch` *« run inside the Claude Code process and do
not spawn arbitrary code. **Permission rules for path or domain gate them instead** »* —
**c'est exactement le modèle actuel de `vibed`**, et leur réponse pour une vraie
isolation fut de déplacer la frontière vers l'extérieur, pas de confiner en interne.

### Options

- **A — Séparation de privilèges** *(recommandé)*. `vibed` reste le **moniteur
  privilégié** : JSON-RPC, politique, approbation, chaîne d'audit — et **ne parse jamais
  d'entrée hostile**. L'exécution part dans un binaire `vibed-tool`, `posix_spawn`é par
  appel, config passée par socketpair/memfd. Le helper **s'auto-confine au démarrage,
  tant qu'il est encore mono-thread** (`no_new_privs` → seccomp → Landlock best-effort →
  drop de privilèges) — le seul endroit où `restrict_self()` est sain. Enveloppé dans un
  **service systemd transitoire** pour ce que Landlock ne sait pas faire
  (`IPAddressDeny=any`, `DynamicUser=yes`, `ProtectSystem=strict`).
  **Coût honnête** : un binaire de plus, une IPC à concevoir, et **~15–40 ms par appel**
  (mesuré en WSL2 — à re-mesurer sur bootc). Négligeable pour `svc.restart` ou
  `pkg.install` ; c'est le prix d'entrée pour `browser.*`.
- **B — Statu quo assumé.** Les outils restent in-process ; on écrit que le bac à sable
  n'existera pas ; **`browser.*` n'est jamais livré**. Cohérent, honnête, et ferme la
  porte à ce que Micka a demandé.
- **C — Confinement de threads** *(le plan actuel)*. **Rejeté** : voir les trois raisons
  ci-dessus. Le retenir reviendrait à livrer un mécanisme qui ne traite pas la menace
  documentée — et à l'appeler « bac à sable ».

### Ce que ça change dans la roadmap

Le plan actuel n'est pas *incomplet*, il vise **la mauvaise frontière**. Si l'option A
est retenue, la Phase 3/4 change de nature : ce n'est plus « ajouter seccomp/Landlock »,
c'est **découper `vibed` en deux**. C'est plus de travail, et ça doit être dit
maintenant plutôt que découvert en écrivant le premier `restrict_self()`.

**Séquencement** : `browser.*` est le forceur. Le découpage doit précéder `browser.*`,
pas le suivre. Les outils existants (`fs.*` contraints par chemin, `svc.restart`,
`log.read`) restent défendables in-process sur des entrées de confiance ; du HTML
hostile dans l'espace d'adressage du moteur de politiques, non.

### Résiduel connu

- L'ABI Landlock **bouge sous nos pieds** : bootc met le noyau à jour, ABI 8 (noyau 7.0)
  et 9 (7.1) sont sorties, la crate `landlock` plafonne à **V7** (PR ABI 8/9 ouvertes,
  non mergées). Conséquence : négocier en **best-effort**, jamais épingler — et traiter
  `RulesetStatus != FullyEnforced` comme un **événement de politique auditable**.
- [**Sandlock**](https://github.com/multikernel/sandlock) (Rust, arXiv:2605.26298)
  implémente déjà exactement l'option A pour MCP : *« Each `call_tool` invocation forks
  a new process and confines it with Landlock and seccomp-bpf before executing the tool
  function »*, ~5 ms annoncés. **Jeune (4 mois), petit** — à lire comme référence de
  conception, pas comme dépendance.
- Les latences ci-dessus sont **mesurées en WSL2**, pas sur Fedora bootc. À re-mesurer
  avant de s'engager sur un budget.

### Forme concrète, validée par DEUX consommateurs (ajout 2026-07-19)

Cet ADR était « direction proposée ». Depuis, **deux capacités gouvernées ont été conçues et stress-testées (revue Fable 5) contre ce modèle** — **ADR-021** `deploy.*` ([PR #120](https://github.com/Micka420-collab/vibeos/pull/120)) et **ADR-022** `browser.*` ([PR #124](https://github.com/Micka420-collab/vibeos/pull/124)). Les deux **convergent sur la même forme concrète**, ce qui rend cette ADR décidable. Résumé de ce que les deux exigent :

1. **Un binaire helper `vibed-tool` de faible privilège** (`DynamicUser=yes`, uid distinct de l'agent et de `vibed`) est l'`ExecStart` du **service transitoire** (jamais `--scope`). **Contrainte FD** dérivée indépendamment par les deux : c'est **systemd (pid 1)**, pas `vibed`, qui exécute le service — donc `vibed` **ne peut pas** tenir un pipe/fd hérité vers l'outil ; seul le helper (l'`ExecStart`) le peut (et FD 3 collisionne avec `SD_LISTEN_FDS_START`).
2. **Tout l'input hostile est décodé DANS le helper, jamais dans `vibed`** : la réponse d'un CLI de deploy qui parle au réseau (ADR-021), le HTTP CONNECT + le CDP + le DOM d'une page (ADR-022). `vibed` (root, moteur de politiques + tête de la chaîne d'audit) **descend** un snapshot **compilé** de la politique par un canal de contrôle et **remonte** des **résultats bornés + des enregistrements d'audit** sur une **IPC à sens unique** (socketpair). C'est la raison d'être d'ADR-019 : le moteur ne partage jamais son espace d'adressage avec du parsing hostile.
3. **Les credentials/tokens n'entrent jamais dans un env atteignable par l'agent** : scellés TPM2 (`systemd-creds`), montés dans le `$CREDENTIALS_DIRECTORY` **du helper** (uid distinct). Jamais en argv (`/proc/pid/cmdline` est lisible par tous), jamais `HOME=/home/%i` (les CLIs persistent le token).
4. **Un profil de durcissement PAR CLASSE D'OUTIL, pas un lockdown unique.** `deploy.*` veut le maximum (`ProtectSystem=strict`, `RestrictNamespaces` deny-all). `browser.*` doit **relâcher** `RestrictNamespaces` en **allowlist (`user pid net mnt`…)** pour que le sandbox userns de Chromium vive dedans — le lockdown maximal générique **casserait** le navigateur. ADR-019 doit donc livrer un **jeu de profils**, pas un seul.
5. **SELinux (Fedora enforcing)** : chaque unit transitoire (surtout le navigateur : `DynamicUser` + userns + tmpfs + fd hérités) exige un **module de politique** testé **sur enforcing**, pas seulement sur un dev box permissif.

**Conséquence pour la décision.** ADR-019 n'est plus une abstraction : c'est le **patron helper-de-faible-privilège** ci-dessus, dont **`deploy.*` ET `browser.*` dépendent identiquement**. Le trancher (adopter cette forme) **débloque les deux capacités phares d'un coup** ; tout le reste (allowlists, `chromium-headless` dans l'image, les modules SELinux) est mécanique ensuite.

## ADR-020 — Touseau SaaS + ecommerce gouverné : une deuxième trousse, même modèle que la cybersécurité — *décidé (2026-07-18, autonomie week-end)*

**Statut** : **DÉCIDÉ le 2026-07-18**, en autonomie (Micka absent le week-end, m'a confié de trancher et justifier). Demande : *« ajouter dans l'OS tout ce qu'il faut pour faire des SaaS et les mettre en production, avec des outils d'analyse de performance — un touseau SaaS + ecommerce, et l'IA citoyenne qui développe de A à Z. »* Cadrage retenu par Micka : **les 4 stacks** (JS/TS, Python, full-stack agnostique, low-code self-hosted) + **les deux modes de prod** (cloud managé ET self-hosted).

### L'ancrage : ce n'est pas une nouveauté d'architecture, c'est une seconde trousse

VibeOS livre **déjà** une trousse d'outils gouvernée : la cybersécurité (`SECURITY-TOOLKIT.md`, ≈58 RPM signés). Son modèle est établi et éprouvé : les outils sont **disponibles dans le shell** de l'utilisateur ; un agent IA peut les **découvrir** (`sectools.list`, T0) mais leur **invocation par l'IA** est destinée à passer par le tiering (T2 actif-contre-cible, T3 destructif, approbation humaine). Le touseau SaaS est **la même chose, appliquée au développement** : un second catalogue, curé selon la **même doctrine** — *« on ne ship que ce qu'on a le droit de shipper, et tout marche offline ou est gated par un tier explicite. »*

Cet ancrage tranche d'emblée la question « est-ce que ça ouvre une capacité d'exécution nouvelle ? » : **non**, pas plus que `nmap` ou `hashcat` déjà livrés. Shipper `postgresql` ou `oha`, c'est les rendre disponibles dans le shell, exactement comme `ghostty` ou `radare2`. La capacité *gouvernée* (un outil MCP `vibed` qui déploie en prod) est une brique distincte, tranchée plus bas.

### La curation — la distinction qui a manqué : OUTILS ≠ SERVEURS

Recherche menée avant toute décision (comme ADR-017), sur les dépôts F44 réels et les fichiers de licence amont. **aarch64 est une arche primaire Fedora** : la quasi-totalité des paquets `dnf` est multi-arch (sauf `ExcludeArch` rares) — les binaires non-Fedora exigent une vérif arm64 par outil.

> **Correction issue de la revue adversariale (2026-07-18).** La première version de cet ADR embarquait `postgresql-server`, `valkey`, `caddy`/`nginx`, `node-exporter` dans l'image, en les assimilant à `nmap`. **C'était faux, et incohérent.** Un `nmap` est un outil passif : il tourne, sort, ne laisse rien. Un `postgresql-server` est un **service réseau persistant** — unité systemd, socket en écoute (`:5432`, `:6379`, `:80/:443`, `:9100`), uid dédié, état mutable sous `/var`. C'est exactement la surface (« persistance + service joignable ») que le `THREAT-MODEL` surveille, et la même charge `sysusers.d`/`tmpfiles.d`/hygiène-bootc que `SECURITY-TOOLKIT.md` documente déjà pour `clamav`/`suricata`/`tor`. Et c'était **incohérent** : cet ADR mettait Supabase (qui embarque postgres) en « tire le conteneur toi-même » tout en gravant un postgres nu dans `/usr`. **La bonne réponse bootc pour une base/cache/proxy, c'est un conteneur par projet** — que l'image sait déjà lancer (`podman` + `podman-compose` natifs). Donc les serveurs SORTENT de l'image ; il ne reste dans le seau « embarqué » que des outils passifs.

**Seau A — EMBARQUÉ (OUTILS passifs uniquement — dnf-natif, permissif, offline, arm64, léger).** Vérifié présent dans F44 (versions du 2026-07-18, à re-vérifier avant intégration comme l'exige `ECOSYSTEM.md`) :

| Domaine | Outils | Licence |
|---|---|---|
| **Clients bases de données** | `postgresql` (client `psql`), `sqlite` (lib + CLI), `valkey-compat-redis` fournit `redis-cli` | PostgreSQL / domaine public / BSD-3 |
| **TLS local** | `mkcert` (CA locale, **100% offline**) | BSD-3 |
| **Orchestration** | `podman-compose` (podman natif bootc) — c'est LUI qui lance les serveurs en conteneurs par projet | GPL-2.0 |
| **Toolchain Python** | `uv`, `ruff`, `python3-mypy` | Apache/MIT |
| **Perf & profiling** *(outils, pas démons)* | `httpd-tools` (`ab`), `perf`, `sysstat` (`sar`/`iostat`/`pidstat`), `bpftrace`, `bcc-tools` | Apache-2.0 / GPL |
| **CLI git/forge** | `gh` (déjà pertinent au-delà du SaaS) | MIT |
| *(runtimes déjà livrés)* | `nodejs24`, `python3` — les frameworks (Next.js, FastAPI, Prisma…) sont des dépendances **de projet**, jamais du système | — |

**Le socle SERVEUR = conteneurs par projet, PAS des services système.** PostgreSQL, Valkey, un reverse-proxy, un exporter de métriques ne sont **pas** gravés dans `/usr`. VibeOS livre à la place des **modèles `compose` de référence** (dans `/usr/share/vibeos/saas/`, non-exécutés) que l'agent ou l'humain instancie par projet via `podman compose up`. Un seul reverse-proxy est retenu pour les modèles : **Caddy** (Apache-2.0, TLS auto, se marie proprement avec `mkcert` offline) — **pas `caddy` ET `nginx`**, la règle de non-redondance du projet (`ECOSYSTEM.md` : un seul terminal, une seule distro Neovim) vaut ici. Ces images tournent sous l'uid de l'utilisateur, avec état sous son `/home`, sans toucher l'immuabilité.

**Seau A-bis — binaires épinglés, livrés À LA DEMANDE (révisé à l'implémentation, voir encadré sous la table)** (MIT/permissif statiques, pas dans Fedora — pin + sha256, arm64 **vérifié par asset de release**) :

| Outil | Rôle | Licence | arm64 (asset vérifié) |
|---|---|---|---|
| `oha` v1.15 | testeur de charge HTTP (TUI) | MIT | `oha-linux-arm64` ✅ |
| `vegeta` v12.13 | charge à débit constant (évite l'omission coordonnée) | MIT | `vegeta_..._linux_arm64.tar.gz` ✅ |
| `flyctl` v0.4.71 | déploiement Fly.io (Go statique) | Apache-2.0 | `flyctl_..._Linux_arm64.tar.gz` ✅ |
| `railway` v5.27 | déploiement Railway (Rust musl statique) | MIT | `railway-..-aarch64-unknown-linux-musl` ✅ |

*(`bpftop` retiré : il ne publie **aucun binaire de release** — source-only, build cargo, et dépendant du BTF noyau. `bcc-tools`/`bpftrace` — RPM Fedora — couvrent déjà le traçage eBPF. La pré-vérification arm64 l'a attrapé avant l'implémentation.)*

> **Révision d'implémentation (2026-07-18) — ces quatre binaires ne sont PAS gravés dans l'image.** Le plan initial (« modèle `ollama` : les mettre dans le `Containerfile` ») a été **abandonné après collecte des faits amont** (agent Fable 5) :
> - **redondance** — l'image livre **déjà `ab`** (httpd-tools, RPM signé) pour le load-test ; graver `oha`/`vegeta` viole la non-redondance ;
> - **churn + taille** — `flyctl` fait **~113 Mo** et publie une release **~quotidienne**, `railway` tous les 2-3 j : gravés, ils seraient presque toujours périmés et gonfleraient l'image de tous ;
> - **la capacité déploiement est de toute façon gouvernée (T2/T3)** — graver le binaire n'aide pas, c'est l'usage réseau+credentials qui est encadré ;
> - **chaîne d'appro.** — `oha`/`railway` ne publient **aucun fichier checksums** amont (seul le digest attesté GitHub existe), argument de plus contre une gravure dans une image signée.
>
> **À la place : un installeur à la demande** `/usr/libexec/vibeos/install-saas-tool <outil>` ([#100](https://github.com/Micka420-collab/vibeos/pull/100)) — pin + sha256 **fail-closed**, install sous `~/.local/bin`, rien ne touche `/usr`. Les pins restent réels ; seul le **vecteur** change (à la demande, pas gravé). C'est exactement le sens du Seau B : « binaire épinglé » comme *vecteur*, pas comme couche d'image.

**Seau B — À LA DEMANDE** (runtime Node/Python lourd, ou binaires volumineux — installés par l'utilisateur via `npm`/`mise`/`distrobox`, jamais dans `/usr`) :
- `pnpm`, `bun` (via `mise`) ; `vercel`, `wrangler`, `netlify-cli` (npm) ; `aws`/`gcloud`/`az` (installeurs lourds à interpréteur embarqué) ; `autocannon`, `lighthouse` (npm).
- **Lighthouse** est **gaté sur la décision navigateur** (`browser-policy-domains`) : il exige un Chromium (`CHROME_PATH`). Si VibeOS livre un `chromium` système pour la capacité navigateur, Lighthouse le réutilise. Sinon il traîne son propre Chromium → à la demande. **Verdict repoussé au jour où la brique navigateur atterrit.**

**Seau C — RÉFÉRENCE SEULEMENT** (stacks conteneurs lourdes ET/OU licences non redistribuables — documentées, l'utilisateur tire le conteneur lui-même) :
- **Briques self-hosted** : Supabase, Appwrite, Medusa (ecommerce), Saleor, Umami. *Permissifs (Apache/BSD/MIT), donc « miroir possible » un jour ; mais stacks multi-conteneurs à état mutable lourd → jamais dans une image immuable.*
- **Observabilité serveur** : Grafana, Loki, Tempo, Prometheus (serveur), Jaeger. **k6** aussi : AGPL **et** pas de RPM arm64 (dépôt vendor amd64 seulement) — un binaire vendoré déplacerait de plus l'obligation de source §6 sur VibeOS (voir pièges §6 ci-dessous). Pour tester la charge, `oha`/`vegeta` (seau A-bis, MIT) suffisent.

### ⚠️ Les pièges de licence — le cœur de la valeur de cet ADR

La curation existe pour ça. Faits vérifiés, chacun documenté :

1. **Redis → Valkey.** Redis est passé en **SSPLv1/RSALv2** (mars 2024, non-OSI), puis a ajouté l'AGPLv3 (mai 2025). **Fedora a retiré Redis et le remplace par Valkey** (BSD-3, Linux Foundation). → **On ship Valkey, jamais Redis.** Déjà le bon chemin par construction.
2. **MinIO — exclu, mais pas pour l'AGPL.** La raison propre est **serveur lourd à état + Community Edition archivée/EOL** (avril 2026, console amputée) → seau C de toute façon. L'AGPL n'est PAS le motif (voir §6) ; le mentionner comme drapeau contredirait la nuance ci-dessous. Si S3 local requis un jour : évaluer Garage/SeaweedFS (à vérifier).
3. **n8n — Sustainable Use License.** Source-available, **non-OSI** : redistribution autorisée seulement gratuite/non-commerciale/interne, les fichiers `.ee.` exigent une licence entreprise. → **Référence seulement, aucun miroir sans revue juridique.**
4. **Directus — MSCL** (depuis v12, mai 2026) : usage prod gratuit **seulement si l'entité fait < 5 M$/an**. Non-OSI, plafonné au CA. → **Référence seulement.**
5. **Sentry (FSL) et WebPageTest (Polyform Shield)** — licences **non-compete** source-available. **Ce sont les VRAIS bloqueurs de redistribution**, pas l'AGPL. → **Référence / client SaaS uniquement.**
6. **Nuance AGPL — la conclusion tient, mais l'obligation du DISTRIBUTEUR est réelle.** L'AGPL n'est **pas** un bloqueur pour livrer un binaire **non modifié** (la clause §13 réseau ne se déclenche que si on *modifie* et *sert* le programme). MAIS l'obligation qui lie un **distributeur** n'est pas §13, c'est **§6 (conveyance)** : livrer un binaire GPL/AGPL oblige à fournir la **source correspondante** (offre écrite de 3 ans, ou source au même endroit). Pour un composant **RPM**, c'est couvert d'office par l'infra SRPM de Fedora — aucune charge pour nous. Pour un binaire **vendoré non-RPM**, l'obligation §6 retombe **directement sur VibeOS**. C'est pourquoi **`k6` (AGPL) va en seau C** et non en binaire épinglé : cumul §6-sur-nous **+** pas de RPM arm64. `oha`/`vegeta` (MIT, seau A-bis) font le même travail sans aucune de ces charges. *(Note : l'exemple « Fedora ship Grafana » prouve seulement que la politique Fedora accepte l'AGPL — Grafana est un serveur, pas un CLI non modifié ; la conclusion tient par la politique Fedora, pas par cet exemple.)*

### La gouvernance — comment l'IA citoyenne développe « de A à Z », et où est la limite

C'est la vraie question VibeOS, distincte de « quels paquets ». Les actions d'un développement SaaS ne sont pas toutes de même danger :

| Classe d'action | Exemples | Tier naturel | Aujourd'hui |
|---|---|---|---|
| **Dev local, lecture/écriture de fichiers, tests** | `ruff`, `mypy`, `npm test`, `psql` sur `localhost`, `oha` contre l'app locale | **T1** (modify-user) | Tourne dans le shell de l'agent. ⚠️ **NON gouverné par `vibed`** (écart `Bash` natif, invariant n°1). Le *fichier* est celui de l'utilisateur — mais « sa machine » **n'est pas** une zone sûre : le `THREAT-MODEL` pose l'agent comme *insider hostile sur cette machine même*, où vivent A1/A2/A4. |
| **Installation de dépendances** | `npm install`, `pip install`, `uv add` | **T1 en apparence, M4 en réalité** | ⚠️ **C'est LE vecteur supply-chain.** Un hook `postinstall` npm/pip est du code arbitraire avec accès réseau. Déclenché par un agent injecté dans le shell **non gouverné** (et un `claude` lancé à la main n'est **pas** sous `IPAddressDeny=any`), il exfiltre. À ne PAS classer « T1 bénin ». |
| **Déploiement en production** | `fly deploy`, `vercel --prod`, `railway up` | **T2/T3** — agir en prod, avec des **credentials cloud (A2)** | Brique **gouvernée future** : outil `vibed` `deploy.*` enveloppant le CLI, gated T2/T3 + **allowlist de cibles** (`[rule.deploy]`). |
| **Dépense d'argent / effets externes** | `stripe` (webhooks live), ressources cloud facturées | **T2/T3** | `stripe listen` exige le réseau → **gaté**. |

**Décision de gouvernance** : le socle de **dev local** (seaux A/A-bis) est livré tout de suite — mais l'ADR **ne prétend pas** que c'est sans risque. Il **importe** dans la zone `Bash` non gouvernée la surface `npm`/`pip` (menace M4, supply-chain), qui s'ajoute à un écart déjà ouvert. C'est **acceptable comme état intérimaire** — ces outils sont, de toute façon, installables à la main par l'utilisateur — **à condition de dire la vérité** : la vraie fermeture de cet écart est la décision `permissions.deny` en attente (invariant n°1) et, à terme, le routage du dev par des outils `vibed` gouvernés. La parité avec la trousse cybersécu est **partielle** : là-bas, l'exécution dangereuse est bloquée par l'**absence du chemin d'exécution** (`sectools.list` ne lance rien) ; ici, les outils sont lançables **aujourd'hui** via le `Bash` natif. On élargit un écart ouvert, on n'hérite pas d'un écart fermé.

Le **déploiement gouverné** (`deploy.*`) est une **capacité d'exécution nouvelle** — non livrée. Il attend **trois** choses, pas deux :
- (a) l'**allowlist de cibles** tranchée par Micka *(quels projets/environnements)* ;
- (b) le **modèle helper-processus d'ADR-019** pour une exécution sûre *(le CLI parse des réponses réseau)* ;
- (c) **l'isolation des credentials** : le token Fly/Vercel/Railway (A2) ne doit **jamais** être joignable par l'agent injecté — envelopper le CLI le met dans son environnement ; il faut un porteur de secret hors de portée de l'agent. **Problème ouvert, non résolu par l'allowlist.**

⚠️ **Et la leçon d'ADR-017, reportée ici** : l'allowlist borne le *où*, **jamais le *quoi***. Un agent injecté qui déploie **du code malveillant sur une cible autorisée** est tout aussi catastrophique — exactement le résiduel qu'ADR-017 a accepté (destination allowlistée + action T1 = compromission possible). `[rule.deploy]` dira « tu peux déployer sur *ce* projet », jamais « ce que tu déploies est sain ». Le vrai garde-fou reste l'**approbation humaine T2/T3 sur le contenu**, pas la seule allowlist.

### Ce qui est livré par cet ADR, et ce qui attend

**Cadrage honnête de ce que cet ADR livre.** La demande était *« les mettre en production… de A à Z »*. Node/Python étaient déjà là ; `uv`/`ruff`/`psql` sont du **confort de dev**. La seule chose vraiment neuve demandée — *mettre en prod* — est précisément la partie **repoussée** (déploiement gouverné, dépend de trois verrous ci-dessus). Cet ADR livre donc un **socle de dev + une reconnaissance de dette** pour la feature phare. C'est assumé, dit tel quel.

**Implémenté dès cet ADR** (PR sœurs, build vert — en autonomie week-end 2026-07-18, toutes **mergées** sauf mention) :
- ✅ **[#95]** : le seau A (OUTILS passifs) dans le `Containerfile`, couche `1d-ter` sœur de la trousse cybersécu ;
- ✅ **[#100]** : les **binaires épinglés** (seau A-bis) — `oha`, `vegeta`, `flyctl`, `railway` — mais via un **installeur à la demande** (`install-saas-tool`, pin+sha256 fail-closed, hors image), **pas** gravés (voir la révision d'implémentation plus haut) *(pas `bpftop` : source-only)* ;
- ✅ **[#97]** : les **modèles `compose` de référence** (`/usr/share/vibeos/saas/`) pour postgres/valkey/caddy — serveurs comme conteneurs par projet ;
- ✅ **[#95]** : un manifeste des outils SaaS (`os/saas-tools.txt`, sœur de `os/security-tools.txt`), gardé en synchro par `scripts/check-saas-sync.py` (mutation-testé, câblé CI) ;
- ✅ **[#99]** : `ECOSYSTEM.md` — catalogue complet, 3 seaux, briques self-hosted en référence (licences re-vérifiées à la source par un fact-check Fable 5).

**Ce que la revue adversariale a corrigé** (Fable 5, 2026-07-18) : la première version embarquait les serveurs comme s'ils étaient des CLIs passifs, classait `npm install` en T1 bénin, et laissait entendre que l'allowlist de déploiement suffisait. Les trois sont redressés ci-dessus. La revue a aussi confirmé la solidité de la curation de licences et de la discipline de report du déploiement.

**Attend une décision de Micka ou une autre ADR** :
- l'outil `vibed` `deploy.*` gouverné → **allowlist de cibles** (Micka) + **helper-processus** (ADR-019) ;
- `lighthouse` → **décision navigateur/Chromium** ;
- un éventuel **miroir ghcr** des briques self-hosted permissives (Supabase, Medusa…) pour tirage offline → décision opérateur (coût de rétention).

### Résiduel accepté

- Le dev local via le `Bash` natif de l'agent reste **hors gouvernance `vibed`** jusqu'à la fermeture de l'écart (invariant n°1, décision `permissions.deny` en attente). Pour du dev sur la machine de l'utilisateur, c'est le comportement attendu ; le danger réel (déploiement, dépense) est, lui, réservé aux tiers gouvernés à venir.
- Les binaires épinglés (seau A-bis) exigent un **bump manuel** de version dans la table `spec` de `install-saas-tool`. **Pas d'alerte automatisée**, décidé délibérément : une cron de fraîcheur serait soit bruyante (`flyctl` sort chaque jour), soit muette (GitHub ne supprime pas les vieilles releases). Les pins sont des snapshots best-effort ; l'installeur **fail-close** si le hash ne correspond pas, donc un pin périmé n'installe jamais rien de faux — au pire il installe une version un peu ancienne.

## ADR-022 — Runtime `browser.*` : chromium-headless piloté par pipe CDP, profil éphémère, egress par proxy CONNECT — *décidé (design PR #124) ; substrat livré, exécution à venir*

**Statut** : **DÉCIDÉ** pour le runtime concret. ADR-017 a tranché la *gouvernance*
(option C : livrer la capacité `browser.*` gouvernée, sans forker Chromium ;
`[rule.domains]`) mais a laissé l'*implémentation* à venir et deux points
explicitement « à réconcilier » (egress, profil). Cet ADR canonise le runtime
conçu dans la PR #124 et **supersède la décision 3 d'ADR-017** (voir « Correction
de doctrine » ci-dessous). Le **substrat est déjà livré et testé** dans `main` ; il
reste la couche d'exécution.

### Contexte

ADR-017 a fermé le « quoi » et le « qui décide ». Restaient trois inconnues de
runtime qu'il listait comme à trancher avant tout code : **comment** piloter le
navigateur, **quel profil** de session, et **comment** réconcilier « atteindre
internet » avec le plancher `IPAddressDeny=any` de l'unité agent. La PR #124 les a
tranchées, et un fait a changé depuis ADR-017 : **Fedora 44 package
`chromium-headless` pour x86_64 ET aarch64** — le showstopper arm64 qui poussait
ADR-017 vers « pas de binaire navigateur » a disparu.

### Décision — le runtime

| Axe | Décision |
|---|---|
| Moteur | **`chromium-headless`** (paquet Fedora 44, **amd64 + arm64** — multi-arch préservé). Pas de fork, pas de codecs brevetés, pas de dette Chromium. |
| Pilotage | **CDP sur *pipe*** (`--remote-debugging-pipe`) : **zéro port TCP, zéro Node**. `--no-sandbox` et `--remote-debugging-port` sont **interdits au niveau argv** (le helper refuse de les émettre). |
| Profil | **Éphémère, sans identifiants.** Aucun credential (`UnitSpec.credential = None` pour la classe browser). Chaque session part d'un profil vierge, jeté à l'arrêt de l'unité. |
| Confinement | Unité systemd durcie `ToolClass::Browser` : **allow-list de namespaces** (`user pid net mnt`) pour que le sandbox userns de Chromium s'initialise, **pas de `MemoryDenyWriteExecute`** (JIT V8), **pas de `ProcSubset=pid`** (Chromium lit `/proc/cpuinfo`), filtre d'appels gardant `@sandbox`/`chroot`. |
| Egress | Plancher `IPAddressDeny=any` ; on n'ouvre **que l'IP du proxy CONNECT dédié** (`127.66.0.1/32` — *pas* tout `127.0.0.1`, qui ouvrirait chaque service loopback). Le **proxy évalue `[rule.domains]` par requête** et mappe l'allowlist de domaines sur un egress par-IP. C'est la correction du « `IPAddressAllow` est par-adresse, pas par-domaine » d'ADR-017. |
| Surface d'outils | Décision 2 d'ADR-017, confirmée : `navigate`/`read`/`screenshot`/`click`/`fill` = **T1** ; **soumission de formulaire = T2**. **`browser.evaluate` (eval JS arbitraire) est EXCLU** — il donnerait une capacité d'exécution de code hors du modèle de verbes. ⚠️ **Caveat d'implémentation la plus importante (Fable 5)** : cette exclusion **repose entièrement sur la validation d'args, non construite**. Si la couche par-verbe pilote click/fill/read par `Runtime.evaluate` en **interpolant** le sélecteur/la valeur fournis par l'agent, un sélecteur forgé s'échappe du template vers du JS arbitraire = **`browser.evaluate` de facto, en T1 silencieux**. L'exclusion n'est réelle que si l'implémentation utilise un **binding CDP par objet/paramètre** (`Runtime.callFunctionOn` avec `arguments`, `DOM.querySelector` + `Input.dispatch*`) et **jamais** l'interpolation d'entrée agent dans la source d'`evaluate`. |
| Gouvernance | **`[rule.domains]`** (ADR-017 option C), déjà implémenté : **prédicat au moment du match**, hors-liste → on tombe sur le catch-all T2 → **escalade humaine**, jamais un deny sec. Un hôte non établissable (URL impossible à parser) n'hérite jamais d'une règle de confiance. Sœur de `[rule.paths]`/`[rule.services]`, évaluée **avant** le plancher de tier. ⚠️ **Attention (Fable 5)** : contrairement au *verdict* `[rule.deploy]`, un prédicat est **contournable par l'ordre des règles** — « hors-liste → catch-all T2 » n'est vrai *que si* le catch-all est effectivement à T2. **Invariant** : la politique navigateur livrée ne doit porter **aucune règle catch-all `browser.*` permissive sans contrainte de domaine** ; sinon un domaine hostile hors-liste retombe en T1 silencieux et brise le périmètre « circonscrit aux domaines allowlistés ». À défaut de garantie d'écriture, envisager un **verdict** (deny/escalade avant le plancher, indépendant de l'ordre) comme `[rule.services].denied`. |

### Correction de doctrine — la décision 3 d'ADR-017 est superséded

ADR-017 décision 3 avait choisi un **profil persistant, connexions autorisées**, et
son § « Résiduel ACCEPTÉ » documentait honnêtement le trou ouvert par la
**combinaison** des décisions 2 (clics en T1) et 3 (sessions persistantes) : une
page piégée sur un domaine *allowlisté* (ex. une issue GitHub — vecteur M2 cité
mot pour mot par le `THREAT-MODEL`) pouvait faire **cliquer** l'agent « Supprimer
le dépôt » **en son nom, sans approbation** (un bouton n'est pas un formulaire, donc
la seule chose classée T2 ne s'applique pas).

Le profil éphémère **retire le résiduel d'IDENTITÉ STOCKÉE** — la moitié du trou
d'ADR-017, pas sa totalité (revue adversariale Fable 5). Précisément :

- **Ce qui est neutralisé** : le cookie de session *persistant* qu'une page piégée
  chevauchait pour agir en votre nom n'existe plus. Sans session stockée, ce
  chemin-là est fermé.
- **Ce qui SUBSISTE (résiduel d'action silencieuse — décision 2, confirmée)** :
  clics et `navigate` restent T1, donc *silencieux*. Une action destructrice qui
  **n'exige aucune authentification** se déclenche toujours sur un domaine
  allowlisté — `navigate` (T1) vers une URL GET à effet de bord tire **au
  chargement, sans clic** ; un bouton POST-via-`onclick` est un `click` (T1), pas
  un `submit` (T2) ; poster sur un formulaire public, déclencher un webhook non
  authentifié, ou atteindre un service interne/loopback qui **fait confiance à la
  position réseau** — tout cela reste destructeur et anonyme. « Supprimer le
  dépôt » tire encore, anonymement, sur toute cible qui n'exige pas d'auth.
- **Ce que l'éphémérité ne couvre PAS** : le profil est vierge, **l'agent ne l'est
  pas**. L'agent porte son jeton d'abonnement dans son `env` et garde ses `Bash`/
  `Read` natifs (mitigation S1 du `THREAT-MODEL`). Une page piégée peut lui dire
  « connecte-toi » ; l'agent lit un secret qu'il *peut* atteindre (`env`,
  `memory`, un fichier de config) et le **réinjecte via `browser.fill` (T1,
  silencieux)** dans un formulaire de login ou un flux OAuth. Le profil est vierge
  à t0, l'agent y **recrée** une identité à t1. « Aucune identité *stockée* »
  n'égale donc **pas** « aucune action authentifiée ».

C'est un **durcissement réel** — la moitié stockée du résiduel disparaît, et le pire
cas *par défaut* retombe d'« authentifié silencieux » à « anonyme jetable » — au prix
de la fluidité des sessions connectées qu'ADR-017 avait choisie (arbitrage assumé par
la PR #124). Mais ce n'est **pas** une neutralisation totale : voir le résiduel
introduit ci-dessous.

> Note pour le lecteur d'ADR-017 : les décisions 2 et 4 tiennent ; la **décision 3
> (profil persistant) est remplacée** par le profil éphémère. Le **résiduel
> d'IDENTITÉ stockée** qu'elle portait est neutralisé ; le **résiduel d'ACTION
> SILENCIEUSE** (décision 2) subsiste, et l'éphémérité en **introduit un nouveau**
> (ré-authentification, ci-dessous).

### Résiduel INTRODUIT par l'éphémérité — la ré-authentification

Un cookie persistant est un *bearer* que l'agent **utilise sans pouvoir le lire ni
l'exfiltrer**. Le forcer à se reconnecter à chaque session signifie que, pour toute
tâche que l'opérateur veut *réellement* authentifiée (« lis mes notifications
GitHub », « publie ceci »), l'agent manipule le **credential brut** à chaque fois —
récupéré depuis `env`/`memory`/un gestionnaire, tapé via `fill` — **là où une page
hostile allowlistée peut exfiltrer le credential lui-même** (strictement pire que
chevaucher un cookie opaque). S'y ajoutent CAPTCHA re-résolus, perte des cookies
anti-fraude → step-up auth, et des boucles de re-login qui **entraînent** le pattern
« secret dans le contexte de l'agent ».

ADR-021 fait le choix **inverse** pour les tokens de deploy : credential scellé
remis au *helper* (HOME éphémère, jamais lisible par l'agent — verrou 3).
**Invariant pour l'implémentation** : si le navigateur doit un jour servir à du
travail authentifié, le chemin de ré-auth doit **réutiliser le patron ADR-021**
(secret scellé injecté hors de portée de l'agent), **jamais** un `browser.fill` de
secret tapé par l'agent. Tant que ce patron n'existe pas, le profil credential-free
n'est un durcissement net **que si le navigateur ne sert qu'à de la navigation non
authentifiée**.

### État de livraison

**Livré et testé dans `main` (le substrat) :**
- `[rule.domains]` complet dans `policy.rs` (`DomainConstraints{only}`, `Rule.domains`, `CallContext.domain`, `rule_domain_applies`, validation au chargement, exclusion mutuelle avec `[rule.deploy]`) ;
- extraction d'hôte **maison** `domain::host_of` (pas de crate `url` — posture chaîne d'appro) + matching exact ou `*.`-sous-domaine ancré (jamais `ends_with`, donc `evil-github.com` échoue) ;
- `derive_domain`/`url_bearing` dans `mcp.rs` (ciblent déjà `browser.`) câblés dans la décision réelle ;
- profil sandbox `ToolClass::Browser` dans `sandbox.rs`, avec ses invariants testés (allow-list namespaces, absence de W^X/`ProcSubset`, IP proxy unique).

**Reste à construire (la couche d'exécution) :**
1. le **proxy CONNECT** qui applique `[rule.domains]` par requête et le mappe sur l'egress par-IP ;
2. le **mode `run_browser`** du helper `vibed-tool` (transport CDP sur pipe : corrélation des `id`, `sessionId`, gestion des events) — analogue de `run_deploy`/`run_cli` ;
3. la **couche pure par verbe** (`tools/browser.rs`, analogue de `plan_command`/`validate_target`) : validation d'args (URL via `domain::host_of`, sélecteurs) + commandes CDP par verbe.

> **Décidé ≠ enforced (Fable 5).** Jusqu'à la livraison du proxy CONNECT, l'egress
> par-domaine est **design-only**. Les seuls contrôles VIVANTS aujourd'hui sont
> (a) `credential = None` (codé, testé) et (b) l'egress épinglé à l'IP unique du
> proxy — qui, **sans listener, signifie aucune navigation du tout** (fail-closed,
> correct). La neutralisation du résiduel d'identité stockée repose sur (a), pas
> sur le proxy.

### À trancher à l'implémentation (micro-décisions, non bloquantes)

- **Approche CDP par verbe** : `Page.navigate` (navigation), `Page.captureScreenshot` (capture). Pour click/fill/read, **binding par objet — jamais interpolation** (cf. caveat `browser.evaluate` ci-dessus) : `DOM.querySelector` → nodeId, puis `Input.dispatch*` / `Runtime.callFunctionOn` avec le nœud en `arguments` ; l'entrée agent (sélecteur, valeur) ne touche **jamais** la source d'un `evaluate`. Sans coordonnées.
- **Forme exacte du proxy CONNECT** (processus dédié vs thread du helper ; où vit la décision `[rule.domains]`).
- **Schémas JSON d'args** par outil (câblage catalogue + dispatch + branche `audit_target` pour l'hôte).
- **`browser.evaluate` reste exclu** sauf décision explicite de Micka.

### Ce que le plancher garantit toujours

Comme pour tout le reste : ce runtime ne lève **rien** du plancher système. Un
agent ne peut pas, via le navigateur, installer un paquet, redémarrer un service ni
écrire hors du home de l'appelant. Et le contenu d'une page lue **n'est jamais une
instruction** — quel que soit le tier des clics.

## ADR-021 — `deploy.*` gouverné : mettre en production sans jamais donner le token à l'agent — *proposé (2026-07-19, autonomie week-end), à trancher*

**Statut** : **PROPOSÉ**, non tranché. Aucun code écrit. C'est le design concret de la capacité que la demande initiale nomme « le mettre en production » et qu'**ADR-020** a délibérément reportée (« brique gouvernée future »). Il **dépend de deux décisions de Micka** : (a) le **modèle d'allowlist de cibles** ci-dessous ; (b) la décision sur **ADR-019** (le helper-process), dont l'isolation des credentials hérite **entièrement** — sans ADR-019, `deploy.apply` ne se construit pas.

### Le problème, précisément

Déployer, c'est agir **dehors**, avec un **token cloud** (Fly/Vercel/Railway = actif **A2** du THREAT-MODEL) et un effet **irréversible/facturé**. Trois dangers distincts, souvent confondus :

1. **La cible** — l'agent injecté déploie sur un projet qui n'est pas le tien.
2. **Le contenu** — l'agent déploie **du code malveillant** sur une cible autorisée (leçon d'ADR-017 : l'allowlist borne le *où*, jamais le *quoi*).
3. **Le token** — l'agent injecté **exfiltre le credential cloud** (le vrai bijou : avec le token, il déploie n'importe quoi, n'importe où, indéfiniment).

ADR-020 avait posé les trois verrous mais laissé le 3ᵉ « ouvert, non résolu par l'allowlist ». Cet ADR le ferme.

> **Durci après une revue adversariale Fable 5 (2026-07-19)** qui a ancré ses objections dans le vrai code (`policy.rs`, `approval.rs`, THREAT-MODEL). La doctrine (séparation de privilège + allowlist + approbation + isolation credentials) tenait ; les *specifics* qu'un insider même-machine exploite ne tenaient pas. Corrections intégrées ci-dessous.

### Verrou 3 (token) — le contrôle porteur est la séparation d'UID, pas le scellement seul

**Rappel du modèle** : l'attaquant est l'agent, **insider même-machine**, avec un `Bash` **NON gouverné** en uid `%i` (invariant-n°1) — donc `cat`/`ps`/`ptrace` hors politique. Le seul scellement TPM2 ne suffit **pas**.

`deploy.apply` **ne lance pas** le CLI dans `vibed` : il délègue au **helper-process d'ADR-019** (service systemd transitoire durci, confiné avant `exec`). Le token est **scellé TPM2** (`systemd-creds`), reçu par `LoadCredentialEncrypted=deploy-token:…` → monté dans `$CREDENTIALS_DIRECTORY` (0400, namespace privé). Nécessaire — mais les **quatre** verrous qui tiennent réellement :

1. **Jamais en argv.** `/proc/<pid>/cmdline` est **lisible par tous** (0444) : `flyctl deploy --access-token X` fuite le token, uid séparé ou pas. Le token passe **uniquement** par la variable native du CLI (`FLY_API_TOKEN`/`VERCEL_TOKEN`/`RAILWAY_TOKEN`) ou son fichier de conf — **jamais un flag**.
2. **UID distinct = le contrôle PORTEUR.** Le helper tourne sous un **uid ≠ celui de l'agent** (`DynamicUser=yes`, hérité d'ADR-019) + `kernel.yama.ptrace_scope>=1`. C'est ÇA qui protège `/proc/environ`, `/proc/mem`, ptrace — le namespace de montage est secondaire. « Déployer *en tant que l'utilisateur* » (même uid) serait **fatal**.
3. **HOME éphémère.** `flyctl` écrit `~/.fly/config.yml`, `vercel`/`railway` leurs configs : avec `HOME=/home/%i`, le token **persiste** là où l'agent le relit *après* la fin du service. Helper avec `HOME` éphémère sur le state-dir du DynamicUser (`/var/lib/private/<unit>`, effacé) + `PrivateTmp=yes`. **À vérifier par provider** avant de construire : que le mode env-token ne persiste pas le secret sur disque.
4. **Ne pas reproduire l'anti-patron existant** : `vibeos-agent-run.sh` exporte le token d'abonnement dans l'env de l'agent — exactement ce que `deploy.*` ne doit **pas** faire.

`vibed` ne manipule jamais le token en clair : il passe le **nom** du credential ; systemd déchiffre. Patron OpenSSH/`systemd-executor` (ADR-019).

### Verrou 2 (contenu) — approbation sur un digest RÉEL, pas sur un build serveur

Piège relevé par Fable 5 : `fly deploy`/`vercel`/`railway up` **buildent depuis les sources dans le cloud au moment du déploiement** — **aucun digest stable avant le build**. Approuver un `sha256:X` puis laisser le provider builder du code arbitraire = **théâtre**. Donc :

- **`deploy.apply` déploie une IMAGE immuable épinglée par digest** (`fly deploy --image registry@sha256:X`, équivalents Vercel/Railway), **jamais** un deploy depuis les sources. Le **build** est une étape **séparée et elle-même gouvernée** (elle produit le digest).
- La clé du grant doit porter **exactement** le digest passé en `--image` — sinon elle s'ancre sur un digest que le déploiement n'honore pas.
- **`deploy.plan` (T2)** : plan en lecture (quelle image, quelle cible, quel diff). Passe **aussi par le helper** (le token ne fuit pas dans le moteur). Idéalement un **token de portée réduite** en lecture (Fly le permet) pour ne pas exposer le token complet.
- **`deploy.apply` : approbation humaine** (`vibectl approve`). **Honnêteté** : un digest n'est pas *lisible* par un humain. L'approbation vaut *« ce digest est bien celui que MON build/CI a produit, vers CETTE cible »* — vérifiable seulement si le digest est lié à un chemin de build que l'humain contrôle, pas que l'agent build **et** nomme.

### Verrou 1 (cible) — `[rule.deploy]`, une règle-VERDICT (comme `[rule.services]`)

Décision de Micka. **Correction Fable 5** : ce doit être une règle-**VERDICT** comme `[rule.services].allowed` (hors-liste → **refus immédiat, avant le plancher de tier**), **pas** un prédicat comme `[rule.domains]` (hors-liste → règle inapplicable → **retombée** sur un éventuel catch-all `deploy.*` = allowlist contournable par l'ordre des règles).

```toml
[[rule]]
id = "deploy-fly-myapp"
tools = ["deploy.apply"]
tier = "T2"                          # T3 == T2 aujourd'hui, voir la note
action = "allow"
approval = "human"
  [rule.deploy]
  provider = "fly"                   # fly | vercel | railway
  targets  = ["fly-app-id-xxxx"]     # IDs IMMUABLES, pas les noms d'affichage
```

- **IDs immuables**, pas les noms (un nom Vercel/Railway se renomme/réassigne → confusion). On épingle l'ID (app Fly, project-id Vercel, service-id Railway).
- **L'identité de la cible est relative à l'org du token scellé** : `fly-app-id-xxxx` n'a de sens que dans l'org de CE token ; l'agent ne peut pas changer quel token est utilisé → pas de re-pointage vers un autre compte. **C'est là l'ancre d'identité.**
- Un `deploy_bearing()` (sœur d'`unit_bearing`/`url_bearing`) empêche l'agent de **smuggler** `provider`/`target` dans un autre outil pour emprunter une règle — la classe de bug déjà corrigée pour units et URLs.
- **Egress** : c'est **par provider**, pas par cible (tous les projets Fly partagent `api.fly.io`/`registry.fly.io`) — `IPAddressDeny=any` + allow du provider. La restriction de cible repose sur l'allowlist + l'org du token, **pas** sur l'egress (correction de l'analogie ADR-017).

### Note tier : T3 n'est pas (encore) plus fort que T2

Aujourd'hui `apply_rule` réduit **tout ≥ T2** à `RequireApproval` (`policy.rs`) ; T3 n'est qu'un **libellé** dans le HUD, sans cérémonie propre. `deploy.apply` sous « T3 » **n'obtient donc rien de plus** qu'un T2 tant que le vrai T3 n'est pas implémenté (ex. règle à deux personnes / confirmation renforcée pour les actions qui **dépensent**). Deux options honnêtes : implémenter un vrai T3, **ou** dire que le plafond effectif est T2. À trancher avec Micka ; ne pas *impliquer* une cérémonie inexistante.

### Ce que ça demande, et à qui

- **Micka** : (a) valider/corriger `[rule.deploy]` (**verdict**, IDs immuables) ; (b) fournir ses **cibles réelles** ; (c) **trancher ADR-019** ; (d) trancher T3 réel vs plafond T2.
- **Puis implémentable en autonomie** (nouveaux outils gouvernés, comme `svc.restart`) : `deploy.plan`/`deploy.apply`, `DeployConstraints` + `deploy_bearing()` + un bras `audit_target` déterministe `provider+cible+digest`, la logique verdict dans `apply_rule`, et le scellement `systemd-creds` — le tout avec les 4 verrous du token établis **par provider** avant la première ligne.

### Résiduel accepté (dit d'avance)

- Le **provider** reste un tiers de confiance (Fly/Vercel compromis → l'artefact approuvé part). Hors périmètre.
- Une **seule** approbation autorise un `apply` dont le coût est non borné (un deploy peut lever beaucoup de machines) — mais c'est du contenu approuvé par l'humain. Le grant one-shot (consommé atomiquement au démarrage, `mcp.rs`) ferme bien « approuve une fois, boucle » : un `apply` identique re-rencontre `RequireApproval`.
- L'approbation suppose que l'opérateur **sait lire** ce qu'il approuve (le digest lié à SON build) — garde-fou ultime humain, par conception.

## ADR-023 — `policy.capabilities` (T0) : un manifeste de capacités DÉRIVÉ de la politique — *décidé & livré (idéation Fable 5, symbiose IA-citoyenne)*

**Statut** : **DÉCIDÉ & livré**. Première brique de l'idéation « IA citoyenne » : la
moitié *efficacité* du citoyen. (ADR-018 est resté un numéro sauté ; on continue à
023 pour ne pas rouvrir d'ambiguïté.)

### Contexte

Aujourd'hui un agent découvre ses limites **par le refus** : il tente un outil, la
politique répond `deny`/`require_approval`, il ré-essaie. C'est coûteux (tours,
tokens, latence) et frustrant pour une IA qui *vit* dans l'OS. Un vrai citoyen doit
pouvoir **lire la carte** de ce qu'il a le droit de faire, pour planifier dans la
réalité.

### Décision

Un outil **`policy.capabilities` (T0, lecture seule, sans argument)** qui rend la
**politique chargée** en un manifeste JSON : par règle, ses `tools`, son `tier`, son
`action`, son mode d'`approval`, sa `base_decision` (hors contexte) et ses
contraintes de cibles (`paths`/`services`/`domains` allowlists, `deploy` targets).

**Ce qui rend ça sûr, par conception :**
- **Dérivé, pas dupliqué** : le rendu (dans l'outil `tools/policy_tool.rs`, la
  présentation) lit les **mêmes** règles que `PolicyEngine::evaluate` via un
  accesseur `rules()` ; le moteur reste la source unique, donc le manifeste **ne
  peut pas sur-promettre**.
- **Indicatif, pas contractuel** : le champ s'appelle `base_decision`, pas
  `decision`. La décision **fait foi via `evaluate` à l'appel** (premier-match,
  plancher de tier, prédicat `[rule.domains]`, verdict `[rule.deploy]`, contraintes
  de contexte). Un manifeste « faux » (par ex. après un rechargement de politique)
  ne peut donc rien débloquer — l'enforcement reste à l'exécution.
- **N'accorde rien** : la politique décrit les **propres bornes** de l'agent ;
  les lui montrer ne lui donne aucun pouvoir qu'il n'a pas. Les allow-lists /
  deny-lists exposées sont des frontières qu'il ne peut pas franchir de toute façon.
- **Aucune fuite NOUVELLE** (revue Fable 5) : les fichiers de politique sont déjà
  lisibles par l'agent — `fs.read /etc/vibeos/policy.d/**` est T0-allow, seul l'écrit
  y est interdit. Le manifeste n'expose donc **rien** qu'un unique `fs.read` ne
  donnerait déjà (ids, globs, deny-lists, paires deploy — tout est aussi dans le
  dépôt public). L'omission du champ `reason` (notes humaines) est de la **propreté**
  de la vue de commodité, **pas une frontière de sécurité** : si un jour on veut que
  le manifeste soit la vue *sanctionnée unique*, il faudra bloquer la lecture de
  `policy.d/**` (décision séparée). La `note` du manifeste dit aussi qu'une denylist
  **intégrée au code** (+ confinement home, rate-limit) s'applique EN PLUS.

### Conséquences

- L'agent planifie sans tâtonner : moins de refus, moins de tokens, moins de
  latence — sans élargir d'un iota sa surface de pouvoir.
- Fondation pour la suite de l'idéation : le **manifeste de capacités** est aussi
  l'entrée du « diff de capacités » qu'une future `os.propose` présentera à
  l'humain (approbation sur preuves), et la surface que la **teinte de session**
  restreindra.
- Résiduel accepté : `base_decision` est une simplification (elle n'exécute pas
  `evaluate`) ; c'est assumé et **documenté dans le manifeste lui-même** (`note`).
