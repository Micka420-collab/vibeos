# Registre des décisions d'architecture (ADR)

> Format : **Contexte / Décision / Alternatives considérées / Conséquences**.
> Statut de toutes les ADR ci-dessous : **acceptée** (2026-07-03).
> Une ADR n'est jamais modifiée après acceptation : elle est remplacée par une nouvelle ADR qui la référence. Architecture détaillée : [ARCHITECTURE.md](ARCHITECTURE.md).

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

**Note sur aider** : `aider-chat` n'est plus préinstallé. Il exige Python < 3.13, or la base Fedora Kinoite 42 embarque Python 3.13 — incompatible avec l'image immuable. `opencode` le remplace comme CLI de pair-programming multi-fournisseur livré par défaut. aider reste installable à la demande par l'utilisateur, sans toucher l'OS immuable : `uvx --python 3.12 aider-chat` (éphémère) ou `uv tool install --python 3.12 aider-chat` (persistant, dans `~/.local`).

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

## ADR-011 — Lecture du journal système par un agent (`log.read`, T0) — *proposé, cible Phase 2*

**Statut** : proposé (non implémenté). Ouvert le 2026-07-13. Concrétise le
livrable Phase 2 « lecture du journal » resté ouvert précisément *parce qu'il
demande une conception anti-exfiltration* — ce document est ce garde-fou.

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
auto-diagnostic légitime. La forme (1)–(5) est le compromis retenu ; son
implémentation attend la revue de l'allowlist et du rédacteur.

## ADR-012 — Capture du raisonnement par tap sur le flux, jamais via le transcript du CLI — *implémenté (mécanisme), cible Phase 2.5*

**Statut** : **mécanisme implémenté** (2026-07-13). Livré : le module `reasoning`
(store `memory/reasoning/<session>.jsonl`, `append_thinking`/`read_thinking`,
`safe_session_id` anti-traversal), l'outil MCP **T0 `agent.thinking`**, le
superviseur `vibectl agent run` qui tape le flux `stream-json` et extrait les
blocs `thinking` (`supervisor::extract_thinking`), et le composant HUD
`ReasoningPanel.qml`. **Reste** : le schéma `stream-json` exact par fournisseur
n'est pas contractuel — l'extraction est défensive et doit être vérifiée contre
la version packagée du CLI à l'intégration ; rétention/purge du store à trancher.

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

## ADR-014 — VibeOS pour Zed : gouverner l'agent hébergé via l'adaptateur ACP, jamais le cœur de Zed — *proposé, initiative parallèle*

**Statut** : proposé (investigation en cours le 2026-07-13). La cartographie du
code réel de l'adaptateur est menée avant tout patch (« pas de fork à l'aveugle »)
et sera consignée en § « Structure de l'adaptateur » ci-dessous.

**Contexte.** [Zed](https://zed.dev) est un éditeur rapide dont le panneau agent
parle **ACP** (Agent Client Protocol, `zed-industries/agent-client-protocol`).
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

### Structure de l'adaptateur (investigation)

*À compléter par la cartographie du code réel de `claude-code-acp` avant le premier
patch de la couche 1 (points d'exposition des outils, hook de permission/élicitation,
config MCP/context-servers, mode de permission). En cours le 2026-07-13.*
