# VISION — Le manifeste VibeOS

## Pourquoi un OS dédié au vibecoding ?

Les systèmes d'exploitation actuels ont été conçus pour deux acteurs : l'humain et l'application. L'agent IA n'y a pas de place légitime. Il est donc greffé après coup, et toujours mal : soit enfermé dans une application sans pouvoir réel, soit lâché sur un shell avec les pleins pouvoirs et aucune traçabilité. Dans les deux cas, le contrat est cassé — trop peu de capacité pour être utile, ou trop de capacité pour être sûr.

Le vibecoding — développer en dialoguant avec des agents qui lisent, écrivent, compilent, déploient — n'est pas un usage de plus posé sur un OS généraliste. C'est un changement d'acteur principal. Il exige un système où l'action d'un agent est une **primitive de première classe** : déclarée, autorisée, exécutée en bac à sable, journalisée. Il exige aussi un socle qui pardonne : quand un agent autonome se trompe, un OS immuable revient en arrière atomiquement ; un OS mutable, lui, garde la cicatrice.

Enfin, il exige une réponse claire à la question de la mémoire. Une machine qui travaille avec des agents accumule un contexte intime : code, habitudes, secrets, historique de décisions. Cette mémoire doit appartenir à l'utilisateur, être chiffrée, et pouvoir ne jamais exister du tout. Aucun OS existant ne traite cette question comme un principe de conception. VibeOS, si.

---

## Principes fondateurs

### 1. Naissance vierge

**Pas de mémoire d'usine.** L'image de VibeOS est identique pour tout le monde et ne contient aucun état : pas de profil, pas d'historique, pas de contexte pré-embarqué. La mémoire de la machine est **créée au démarrage**, pas livrée avec.

Au premier boot, la séquence **Genesis** (`vibeos-genesis.service`) construit la mémoire à partir de zéro sur `/var/lib/vibeos/memory` — c'est livré dès la v0.1. Cette mémoire est la propriété exclusive de l'utilisateur : jamais synchronisée sans consentement, effaçable d'un geste, et chiffrée au repos sur un volume LUKS à partir de la **Phase 3** (en v0.1, elle naît en clair — nous le disons sans détour). Le **mode amnésique** (Phase 3 lui aussi) poussera le principe à son terme : la mémoire sera reconstruite en tmpfs à chaque démarrage et mourra avec la session, comme sur Tails. Entre la persistance chiffrée et l'oubli total, c'est l'utilisateur qui choisira — pas le système, pas l'éditeur.

### 2. L'IA est un citoyen de l'OS, pas une app

Un citoyen a des droits, des devoirs et des lois. Une app installée n'a que des permissions statiques ; un agent sur un shell root n'a que des pouvoirs. VibeOS refuse les deux extrêmes et institue un **contrat** :

- **Une interface unique et déclarative** : les agents parlent au système via le démon `vibed` et son serveur MCP — jamais par accès brut. Chaque capacité du système est un outil nommé, typé, documenté.
- **Des lois** : le moteur de politiques (`/etc/vibeos/policy.d/`) classe chaque action en niveaux T0 (observer) → T3 (destructif). Modifier le système (T2) ou toucher au disque, aux identifiants, à l'identité réseau (T3) exige l'approbation humaine par défaut.
- **Une mémoire des actes** : chaque appel d'outil est audité (journal JSONL append-only, avec l'identité de l'appelant) — un mécanisme servi par `vibed`, **embarqué et actif dès la v0.1**. On peut ainsi toujours répondre à « qui a fait quoi, quand, et avec quelle autorisation ».
- **Des frontières physiques** : le confinement de l'exécution par outil — systemd-run, seccomp, landlock — est un livrable de la **Phase 3**. À terme, même autorisé, un outil ne sortira pas de son enclos ; en v0.1, la frontière est le contrat MCP + politiques + audit.

L'autonomie des agents n'est pas une faveur qu'on leur accorde : c'est une conséquence directe de la confiance que ce contrat rend possible.

### 3. Sécurité d'abord

La sécurité n'est pas une couche, c'est la fondation — et elle doit être **vérifiable**, pas déclarative. Ce qui impose d'être honnête sur ce qui est livré et ce qui arrive (le tableau « Livré en v0.1 / En route » du [README](README.md) fait foi) :

- **Immuable** — livré en v0.1 : racine en lecture seule, mises à jour atomiques, retour d'usine garanti (bootc/OSTree). L'état sain n'est pas restauré, il n'est jamais perdu.
- **Vérifié** : les images OS sont signées avec sigstore/cosign en CI dès la v0.1, et l'image de base comme les outils IA sont épinglés (digest, versions exactes, lockfiles). La chaîne de démarrage mesurée UEFI Secure Boot → UKI → dm-verity/composefs est la cible de la **Phase 4** : à terme, ce qui démarre sera exactement ce qui a été signé, du firmware au système de fichiers.
- **Chiffré** : la mémoire vivra sur LUKS (**Phase 3**). Un disque volé sera un disque muet.
- **Audité et confiné** : **SELinux enforcing** (politique targeted Fedora) dès la v0.1 ; le **journal d'audit JSONL des actions des agents** est livré avec `vibed`, **embarqué dès la v0.1** ; sandboxing systématique des outils en **Phase 3**, politique SELinux dédiée à `vibed` en **Phase 4**.

Un OS qui donne de vrais pouvoirs à des agents autonomes n'a pas le droit d'être moins sûr que les autres. Il doit l'être davantage — et il n'a pas non plus le droit de décrire au présent une protection qui n'existe pas encore.

### 4. Souveraineté progressive

VibeOS assume ses emprunts : Fedora Kinoite comme base, des modèles cloud comme cerveaux, des outils tiers comme membres. C'est le prix d'un départ honnête. Mais la trajectoire est explicite : **année après année, remplacer les briques empruntées par les nôtres**.

Les modèles locaux via ollama sont la première marche — l'image v0.1 embarque tout pour coder hors ligne (la validation formelle « `ollama run` sans réseau » est un critère de sortie de la Phase 1). Suivront nos propres composants système (`vibectl` et au-delà), nos propres politiques de référence, et à mesure que le projet mûrit, une base de plus en plus détenue en propre. La dépendance est un état de départ, jamais une destination. La feuille de route de cette émancipation est dans [ROADMAP.md](ROADMAP.md).

### 5. L'OS se construit lui-même

VibeOS est **vibecodé avec les agents qu'il embarque**. Le dépôt que vous lisez est développé par des agents opérant sous les mêmes principes que ceux imposés par l'OS : actions déclarées, revues, auditées. Chaque limite rencontrée par les agents qui construisent VibeOS devient une exigence pour VibeOS lui-même.

C'est plus qu'un clin d'œil : c'est la boucle de validation la plus honnête qui soit. Si l'OS n'est pas assez bon pour que ses propres agents le construisent, il n'est pas assez bon. Le jour où VibeOS se compile, se teste et se met à jour depuis VibeOS, le projet aura tenu sa promesse fondatrice.

---

## Ce que VibeOS n'est pas

- **Ce n'est pas « une distro avec un chatbot »**. L'assistant installé sur un OS classique reste un invité. Ici, l'intégration est structurelle : démon système, socket, politiques, audit. Retirer l'IA de VibeOS n'est pas désinstaller une app, c'est amputer le système.
- **Ce n'est pas un OS généraliste mutable de plus**. Pas de `dnf install` sur la racine, pas de dérive de configuration. L'immuabilité n'est pas négociable ; elle est la condition de l'autonomie des agents.
- **Ce n'est pas un agent avec les clés du royaume**. Aucun agent n'obtient jamais un accès brut au système. Tout passe par le contrat MCP + politiques + audit, y compris — surtout — quand c'est moins pratique.
- **Ce n'est pas un terminal du cloud**. Les modèles locaux (ollama) sont un pilier, pas un lot de consolation. VibeOS doit rester utile sans connexion et sans compte.
- **Ce n'est pas une machine à télémétrie**. La mémoire naît chez l'utilisateur, sera chiffrée chez l'utilisateur (Phase 3), et pourra mourir à chaque extinction si l'utilisateur le décide (mode amnésique, Phase 3). Aucune donnée ne quitte la machine par défaut.
- **Ce n'est pas une démo de recherche**. C'est un projet d'ingénierie pluriannuel, avec une base immuable éprouvée (bootc/OSTree), une CI, des images signées et une feuille de route. La v0.1 est une fondation, et elle est pensée pour durer.
