# WEEKEND-LOG — travail autonome (week-end 2026-07-18 → 20)

> **Pour Micka, à distance.** Tu es parti pour le week-end et m'as confié de pousser
> le projet en autonomie. Ce fichier est ton **hublot** : mis à jour en continu, le
> plus récent en tête. Chaque décision non triviale y est **justifiée** — tu n'es pas
> là pour trancher, donc je tranche et j'explique.
>
> Journal détaillé des sessions précédentes : [SESSION_LOG.md](SESSION_LOG.md).
> État vivant permanent : [STATUS.md](STATUS.md).

## Le cadre que je me suis fixé (pour mériter la confiance)

**Je décide et justifie moi-même** : périmètre des features, curation d'outils,
architecture, priorités.

**Je ne franchis PAS seul, même en autonomie** (invariants de *sécurité*, pas des
préférences) :
- le plancher T2/T3 reste intact ;
- `policy.rs` / `approval.rs` / `audit.rs` : je peux proposer, mais tout changement du
  cœur de confiance reste une **PR ouverte flaggée pour ta revue** — jamais mergée seul ;
- aucune capacité d'exécution nouvelle sans sa gouvernance (tiering + allowlist) conçue
  d'abord ;
- ce que je merge seul : docs, CI, outillage, features **non-TCB**, **vert** et **relu
  par un agent Fable 5 adversarial** avant merge.

**Rythme** : je garde toujours du travail en fond (recherche, builds, revues) qui me
relance — je ne m'arrête pas en t'attendant.

---

## 🎯 Mission du week-end : le touseau SaaS + ecommerce gouverné

Ta demande : *« ajouter dans l'OS tout ce qu'il faut pour faire des SaaS et les mettre
en production, avec des outils d'analyse de performance — un touseau SaaS + ecommerce,
et l'IA citoyenne qui peut développer de A à Z. »*

Ta réponse aux 2 questions de cadrage : **les 4 stacks** (JS/TS, Python, full-stack
agnostique, low-code self-hosted) + **les deux modes de prod** (cloud managé ET
self-hosted).

Traitée comme le navigateur (ADR-017) : **recherche des faits d'abord, ADR de curation
ensuite, implémentation de la partie sûre enfin.**

---

## 📓 Journal (le plus récent en tête)

### 2026-07-18 — mise en route

- **`main` réparé et stable.** #89 mergée (digest de base bumpé vers `7b70f8c6`, vérifié
  qu'il résout encore aujourd'hui). Le build repart.
- **Backlog nettoyé** : #88 (journal) et #90 (alerte cron digest) mergées. 0 PR en
  attente — arbre propre.
- **Incident maîtrisé** : un `git rebase` s'est mal passé sur ce worktree (le `.git`
  pointe un chemin Windows), le force-push a vidé #90. Récupérée par cherry-pick du
  commit détaché, PR rouverte. Leçon : **cherry-pick, pas rebase, sur ce worktree.**
- **Recherche SaaS lancée** (3 agents parallèles). 2/3 rendus :
  - **Socle à embarquer** (dnf-natif, permissif, offline, arm64 par construction) :
    PostgreSQL, **Valkey** (pas Redis), SQLite, Caddy/nginx, mkcert, podman-compose,
    **uv**, **ruff**, mypy.
  - **CLIs de déploiement à embarquer** (petits binaires statiques) : `flyctl`,
    `railway`, `gh`. **À la demande** (Node/lourds) : `vercel`, `wrangler`, `netlify`,
    `aws`/`gcloud`/`az`.
  - **Briques self-hosted = référence seulement** (stacks conteneurs lourdes, jamais
    dans une image immuable) : Supabase, Medusa, Umami…
  - **Pièges de licence évités** (le cœur de la doctrine) : Redis→Valkey (Redis passé
    SSPL/RSALv2), **MinIO exclu** (AGPL + EOL/archivé), **n8n/Directus/Plausible en
    référence-seulement** (licences source-available ou AGPL), Stripe CLI **gaté** (il
    exige le réseau).
  - 3ᵉ agent (perf/observabilité) en cours.

**Prochaine étape** : ADR-020 dès le 3ᵉ rapport, puis implémentation du socle « ship »
dans le Containerfile (build vert obligatoire), revu par Fable 5 avant merge.

### 2026-07-18 (suite) — ADR-020 écrite, en revue Fable 5

- **3 rapports de recherche rendus** (dev toolchains, deploy CLIs + briques self-hosted, perf/observabilité). Faits vérifiés : licences amont, dispo dépôts F44 réels, arm64, offline.
- **Socle « ship » vérifié présent dans F44** (versions réelles) : postgresql 18.3, valkey 9.0.4, sqlite 3.51, caddy 2.10, nginx 1.30, mkcert 1.4.4, podman-compose 1.6, uv 0.11, ruff 0.15, mypy 1.18, + gh 2.94, perf/sysstat/bpftrace/bcc/node-exporter.
- **[ADR-020](docs/DECISIONS.md) ouverte en [PR #92](https://github.com/Micka420-collab/vibeos/pull/92)** : le touseau SaaS est une **seconde trousse gouvernée**, même modèle que la cybersécurité. 3 seaux (embarqué / à la demande / référence), pièges de licence documentés (Redis→Valkey, MinIO exclu, n8n/Directus/Sentry/WebPageTest en référence), dev local T1 / déploiement prod T2/T3 gouverné futur.
- **Pré-vérification arm64 des binaires épinglés** : `oha`, `vegeta`, `flyctl`, `railway` confirmés arm64. **`bpftop` retiré** — il ne publie aucun binaire (source-only), et `bcc`/`bpftrace` (RPM Fedora) couvrent déjà le traçage eBPF. La vérif a fait son travail avant l'implémentation.
- **Revue adversariale Fable 5 lancée** sur l'ADR. Je traite ses trouvailles **avant** de figer et d'implémenter.
- **Note process** : `git rebase` puis `origin/main` périmé m'ont coûté deux reprises (worktree Windows). Discipline adoptée : **cherry-pick, jamais rebase** ; **`git fetch` avant chaque branche**.

**Décision que je te laisse** (notée pour ton retour) : l'outil `vibed` `deploy.*` gouverné a besoin de ton **allowlist de cibles** (quels projets/environnements l'IA peut déployer) — sœur du `[rule.domains]` du navigateur. Je ne le construis pas sans ça.

### 2026-07-18 (suite) — revue Fable 5 intégrée, première brique du socle livrée

- **Revue adversariale Fable 5 sur ADR-020 → 3 vraies failles corrigées** (commit `1ea89f8`, [PR #92](https://github.com/Micka420-collab/vibeos/pull/92)) :
  1. **Serveur ≠ outil passif.** Postgres/valkey/caddy sont des services réseau persistants, pas des `nmap`. Et j'étais incohérent (Supabase en « conteneur toi-même » mais postgres nu gravé). → les serveurs **sortent de l'image**, deviennent des **conteneurs par projet** (podman-compose + modèles de référence). Le seau embarqué = **outils passifs seulement**.
  2. **`npm install` n'est pas T1 bénin** — c'est le vecteur supply-chain M4 dans le shell non gouverné. Dit tel quel.
  3. **Déploiement gouverné** : l'allowlist borne le *où*, jamais le *quoi* (leçon d'ADR-017) ; + isolation des credentials cloud = 3ᵉ verrou. Le vrai garde-fou = approbation humaine sur le **contenu**.
- **[PR #95](https://github.com/Micka420-collab/vibeos/pull/95) — la couche 1d-ter livrée** : 14 outils dnf-natifs (client psql, sqlite, redis-cli, mkcert, podman-compose, uv, ruff, mypy, ab, perf, sysstat, bpftrace, bcc, gh). **Client postgresql, PAS le serveur** (vérifié). Manifeste `os/saas-tools.txt` + garde-fou `check-saas-sync.py` mutation-testé. Le méta-garde-fou (#87) compte maintenant **7** checks, tous câblés — il valide son propre nouvel usage.
- **Discipline** : chaque PR de cette session est revue (Fable 5 sur le design, mutation-test sur les garde-fous) et attend un build vert avant merge.

**Prochaines briques** (PR sœurs) : binaires épinglés (oha/vegeta/flyctl/railway, arm64 vérifié), modèles `compose` de référence (postgres/valkey/caddy par projet), MàJ `ECOSYSTEM.md`.

### 2026-07-18 (suite) — modèles compose livrés (le socle serveur, hors image)

- **[PR #97](https://github.com/Micka420-collab/vibeos/pull/97) — les modèles `compose` par projet.** C'est la conséquence directe de la faille #1 de la revue Fable 5 : les **serveurs** (postgres/valkey/caddy) sortent de l'image immuable et deviennent des conteneurs **par projet**. Livrés sous `/usr/share/vibeos/saas/` via `COPY os/rootfs/ /` — **zéro** changement de `Containerfile`, donc **indépendant de #95** (pas de conflit de couche).
  - `postgres-valkey/` : PostgreSQL 18 + Valkey. Ports **loopback-only**, mot de passe Postgres **exigé** via `.env` (jamais en clair, jamais commité), healthchecks, volumes nommés (l'état vit dans le projet, pas dans l'image).
  - `reverse-proxy/` : Caddy + TLS local via `mkcert` (déjà livré couche 1d-ter) → un `https://` de dev valide, 100 % offline.
  - README qui explique *pourquoi conteneurs-pas-services*, l'usage, et le rappel de gouvernance (dev local = T1 ; prod = T2/T3 à venir).
- **Cohérence vérifiée avant push** : YAML des deux `compose` valides, liens relatifs du README corrigés (6 niveaux jusqu'à la racine), `verify-roadmap-truth` vert.
- **#95 (couche d'outils 1d-ter)** : build amd64 natif relancé (l'échec précédent était un *flake* CDN quay.io sur le pull de la base finale, pas ma couche). En cours.

**Prochaines briques** : binaires épinglés (oha/vegeta/flyctl/railway) — **séquentielle**, elle touche le `Containerfile`, donc après #95 ; puis catalogue SaaS/ecommerce dans `ECOSYSTEM.md`.

### 2026-07-18 (suite) — trois briques mergées, le catalogue, et une décision qui renverse un plan

Grosse avancée. **La trousse SaaS est en place sur `main`** — outils, substrat serveur, catalogue, et un mécanisme d'install sûr. Détail :

- **✅ #95 mergée — couche d'outils 1d-ter.** Build vert (19 min), 14 outils passifs sur `main`. Guard `check-saas-sync.py` mutation-testé.
- **✅ #97 mergée — modèles compose par projet.** Build vert (16 min). Le socle serveur (postgres/valkey/caddy) vit désormais par projet, jamais gravé.
- **✅ #99 mergée — catalogue SaaS dans `ECOSYSTEM.md`.** Les 3 seaux (embarqué / à la demande / référence self-hosted) + les pièges de licence, en un seul endroit. Ça comble une référence que le header de `saas-tools.txt` promettait déjà.
- **🔎 Fact-check Fable 5 des licences/arch** (23/25 confirmés à la source). **2 corrections appliquées avant merge** : Redis n'est plus « non-OSI » (depuis Redis 8, tri-licence RSALv2/SSPLv1/AGPLv3 — la reco Valkey BSD-3 tient quand même) ; Stripe CLI est Apache-2.0, pas MIT. C'est exactement le rôle de la revue adversariale : elle a rattrapé deux faits périmés dans un projet où la licence est juridiquement sensible.

**La décision du jour (je te l'explique parce que je renverse un plan que je m'étais fixé).**
Le plan pré-week-end disait : « graver oha/vegeta/flyctl/railway dans le `Containerfile` ». En **vérifiant les faits amont** (agent Fable 5 : versions, URLs, tailles, cadence de release), j'ai vu que ce plan était mauvais :
  - l'image livre **déjà `ab`** (ApacheBench) pour le load-test → graver oha/vegeta = redondance (notre doctrine l'interdit) ;
  - **flyctl fait 113 Mo et sort presque tous les jours** → gravé, il serait toujours périmé + gonflerait l'image de tout le monde ;
  - le **déploiement est de toute façon gouverné (T2/T3)** → graver le binaire n'apporte rien, c'est l'usage réseau+credentials qui compte.
  Mon propre catalogue (relu Fable 5) les plaçait d'ailleurs déjà en **Seau B « à la demande »**. Le plan contredisait le catalogue. **J'ai suivi le catalogue.**

- **🔧 #100 ouverte — installeur à la demande.** `/usr/libexec/vibeos/install-saas-tool <outil>` : télécharge, **vérifie le sha256 (fail-closed)**, installe sous `~/.local/bin`. Rien ne touche `/usr`. **Testé de bout en bout** : `oha` s'installe et tourne (`oha 1.15.0`) ; un hash corrompu est **refusé sans rien installer**. shellcheck vert. Build en cours.

**Où en est la mission SaaS.** Le gros est fait pour ce que je peux livrer seul : outils dans l'image, serveurs par projet, deploy CLIs à la demande + vérifiés, catalogue complet, perf/observabilité (perf/sysstat/eBPF + ab). Le seul morceau que je **ne** construis **pas** seul reste `deploy.*` gouverné dans `vibed` — il attend **ton allowlist de cibles** (quels projets/environnements l'IA peut déployer). C'est noté, ADR-020 (#92) t'attend pour cette décision d'archi ; je ne l'auto-merge pas.

**Prochain chantier** (une fois #100 mergée) : je regarde les features non-SaaS en attente (ex. `browser.*` sur `[rule.domains]`, ADR-017) — mais comme ça touche le cœur `vibed`, ce sera une **PR flaggée pour ta revue**, pas un auto-merge.

### 2026-07-18 (fin de journée) — mission SaaS bouclée, docs recalées

- **✅ #100 mergée** — l'installeur à la demande est sur `main`. **Les 5 briques de la trousse SaaS sont livrées** : outils embarqués (#95), serveurs par projet (#97), catalogue (#99), installeur vérifié (#100), + le hublot (#98/#101). Build vert à chaque fois.
- **ADR-020 (#92) recalée sur la réalité** : elle disait encore « graver les binaires » ; je l'ai révisée pour décrire l'installeur à la demande, et je l'ai rebasée par **merge** (pas rebase) sur `main` → c'est maintenant une PR **propre « doc seule »**, prête pour ta revue. **Toujours pas auto-mergée** : elle porte la décision `deploy.*` qui t'appartient.
- **STATUS.md + README recalés** : la trousse SaaS y figure désormais (entrée datée dans STATUS, section dédiée dans le README, en parallèle de la trousse cybersécu), avec l'honnêteté d'usage : *déployer en prod* n'est **pas** livré.
- **Constat de fin** : le backlog restant est **de ton côté** (booter la VM, tester l'ISO, valider NVIDIA) ou du **durcissement TCB** (SELinux, `User=vibed`, sandbox par outil) qui exige un système démarré **et** ta revue — donc pas d'auto-merge. La surface de travail net **sûre et autonome** est, pour l'instant, épuisée avec la trousse SaaS. Je ne vais pas inventer du risque pour paraître occupé : s'il reste une vraie amélioration sûre non-TCB, je la pousse ; sinon je garde de la veille en fond et je te laisse un état net.
- **Mémoire agent** : doctrine « graver vs à la demande » consignée pour les prochaines sessions (churn/taille/gouvernance/redondance → installeur vérifié).
