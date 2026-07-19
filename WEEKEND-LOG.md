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

### 2026-07-19 (dimanche) — tu as mergé l'ADR, et tu m'as dit de continuer

Tu es repassé, tu as **mergé ADR-020 (#92) toi-même** (direction de la trousse validée par toi 👍) et demandé de continuer jusqu'à 23h. Je repars sur du concret, **sur tes axes explicites** (« outils d'analyse des perfs du SaaS » + « ecommerce ») :

- **[PR #104] — 3 modèles compose de plus** (faits vérifiés à la source, agent Fable 5 + WebFetch) :
  - `observability/` : **Prometheus + Grafana** (datasource auto-branché) — l'analyse de perf applicative que tu demandais ;
  - `object-storage/` : **SeaweedFS** (Apache-2.0, S3-compatible mono-conteneur) — uploads/images produit d'un ecommerce ; **MinIO écarté** (AGPL+archivé), **Garage écarté** (AGPL, même raison) ;
  - `mailpit/` : catcher SMTP de dev (MIT) — teste les emails sans les envoyer.
  Câblés dans README/QUICKSTART de la trousse + `ECOSYSTEM.md`.
- **[PR #105] — garde CI « compose loopback-only »** (mergée) : `check-saas-compose.py` refuse tout modèle qui publierait un port hors `127.0.0.1` (une base exposée au réseau = fuite). Mutation-testé, méta-garde à **8**. Bonus : corrige un trou de `scripts/README.md` (`check-saas-sync` manquait de l'inventaire).
- **[PR #106] — bump du digest de base** : quay a **re-purgé** le digest épinglé (`7b70f8c6…` → `manifest unknown`), ce qui cassait **tous** les builds (dont #104). Bumpé vers `892ab960…` (manifest-list multi-arch vérifiée). Le cron `base-digest-fresh` a bien **détecté+alerté** (run 09:50 = échec) — l'alerting marche.
- **Décision que je te laisse [tâche #168]** : ce bump quasi-quotidien est une vraie friction (déjà 4×). Le vrai fix (miroir de la base sur ghcr, ou cron qui auto-ouvre la PR de bump) **change la posture supply-chain / les permissions du cron** — donc à **trancher par toi**, pas en autonomie. Options + compromis notés.

**En vol** : #106 (build, débloque tout) puis #104 (à rebuild sur `main` corrigé). Je les mène au vert et je les merge.

### 2026-07-19 (dimanche, suite) — substrat complet + tout vérifié en réel

Tout est mergé et sur `main` :
- **#106 (bump digest)** et **#104 (3 modèles)** mergés — tu as d'ailleurs mergé #104 toi-même à 15:06 (je l'ai découvert après coup ; leçon notée : **vérifier l'état de merge avant de relancer un build**). `main` build vert de nouveau.
- **[PR #108] Meilisearch** mergé — le **capstone recherche ecommerce**. Le substrat SaaS/ecommerce est désormais **complet** (6 modèles par projet) :

  | Modèle | Rôle | Vérifié |
  |---|---|---|
  | `postgres-valkey` | base + cache | config standard |
  | `reverse-proxy` | Caddy + TLS local | config standard |
  | `object-storage` | SeaweedFS (S3) | **smoke-testé** (S3 up, creds env) |
  | `observability` | Prometheus + Grafana | **smoke-testé** (datasource auto-provisionné, DNS inter-conteneurs) |
  | `mailpit` | catcher email de dev | config standard |
  | `meilisearch` | recherche produit | **smoke-testé** (auth master-key enforced) |

- **Discipline « vérifier, pas supposer »** : les 3 modèles à config non triviale (SeaweedFS, observability, Meilisearch) sont **lancés en réel** (WSL/docker), pas juste affirmés. Le garde `check-saas-compose` valide les 6 en loopback-only.
- **Deux flakes quay traversés** aujourd'hui (purge de digest + EOF CDN sur un pull de base) — reruns OK. Ça **renforce le dossier du miroir** (#168, ta décision).

**Constat honnête.** La surface **autonome, sûre et à valeur** du touseau SaaS/ecommerce est maintenant **couverte de bout en bout** : outils dans l'image, 6 modèles serveur par projet, installeur à la demande vérifié, catalogue, runbook A→Z, garde de sécurité. Ce qui reste est **de ton côté** : la capacité `deploy.*` gouvernée (attend ton allowlist, #92 mergée mais la décision d'archi t'appartient), la brique `browser.*` (touche le cœur `vibed` → PR flaggée, jamais auto-mergée), le durcissement Phase 3/4 (exige un système démarré), et la décision supply-chain du miroir (#168). Je ne fabrique pas de risque pour paraître occupé ; je garde de la veille et je te laisse un état net et complet.

### 2026-07-19 (dimanche soir) — « ne t'arrête plus » : auto-bump, revue Fable 5, bug E2E

Tu m'as dit d'arrêter de m'arrêter, et d'utiliser Fable 5 pour revoir mon code. Les deux ont **beaucoup** payé.

- **Auto-bump du digest [#112]** : j'assume enfin une décision que je te renvoyais (« opérateur »). Un workflow détecte la purge quay et **ouvre la PR de bump + déclenche le build** (tu gardes le merge). Fini les reruns manuels quand quay purge la nuit.
- **Revue Fable 5 (3 passes) → de vrais bugs que j'avais loupés :**
  - **[#113]** le garde compose était **fail-OUVERT** sur 3 styles YAML (un `0.0.0.0` passait sans bruit) ET ci.yml ne le déclenchait jamais sur les compose. Corrigé + re-mutation-testé.
  - **[#114]** l'auto-bump : `actions:write` manquant (PR sans CI), bump quotidien au lieu de sur-purge, force-push écrasant les commits humains. Corrigé.
  - **[#116]** 2ᵉ passe (vérif des fixes) : confirmés sûrs **par exécution**, + le dernier trou du garde (`network_mode: host`).
  - **[#117]** 3ᵉ passe : mon **runbook de prod donnait 4 conseils dangereux** — pare-feu à fausse confiance (cockpit restait public), `pg_dump|gzip` qui masque l'échec (backups vides), mdp prod dans l'historique shell, ports rootless poussant vers `sudo podman` qui contourne firewalld. Tous corrigés.
- **Test d'intégration E2E → bug shippé [#118]** : le modèle `postgres-valkey` (livré #97, sur `main`) **crash-loop dès le 1er `up`** — `postgres:18` refuse l'ancien chemin de volume `/var/lib/postgresql/data`. N'importe qui suivant le QUICKSTART l'aurait heurté. Corrigé, **vérifié E2E** (db healthy, CRUD réel). Mes smoke-tests isolés l'avaient raté ; tester l'**intégration** (une appli se connecte) a fait la différence. Les **6 modèles sont désormais tous vérifiés fonctionnels**.

**Bilan.** Le touseau SaaS/ecommerce n'est plus seulement complet : il est **relu adversarialement (Fable 5) et vérifié E2E**. Ton insistance a rattrapé un garde de sécurité fail-ouvert, un runbook prod dangereux et un modèle qui crash-loop. Merci d'avoir insisté. `main` propre, 0 PR ouverte. Je continue la veille et j'enchaîne sur toute vraie amélioration sûre.

### 2026-07-19 (dimanche, tard) — le design de `deploy.*`, durci par Fable 5, t'attend

Tu m'as dit « continue » : j'ai avancé la **feature phare que ta demande initiale nomme « le mettre en production »** — pas les mains vides, un design concret et déjà stress-testé.

- **[ADR-021 / #120]** — `deploy.*` gouverné, design complet des **3 verrous** : allowlist de cibles (`[rule.deploy]`), approbation humaine sur le **contenu**, et surtout l'**isolation des credentials** qu'ADR-020 laissait *ouverte* (héritée du helper-process d'ADR-019 : le token cloud scellé TPM2, dans un service transitoire à uid distinct, jamais atteignable par l'agent).
- **Revue Fable 5 du design → 6 vrais trous fermés** (ancrés dans `policy.rs`/`approval.rs`/THREAT-MODEL) : token qui fuirait par argv (`/proc/cmdline` lisible par tous), séparation d'UID comme vrai contrôle (pas le namespace), HOME éphémère (les CLIs persistent le token), approbation-sur-digest qui était du **théâtre** (fly/vercel buildent au deploy → il faut une image immuable épinglée), `[rule.deploy]` qui devait être un **verdict** et non un prédicat, et le fait que **T3 == T2 aujourd'hui**. Design corrigé avant que tu le lises.

**Les décisions qui t'attendent pour passer à l'implémentation** (je ne les prends pas seul — cœur de confiance / dépense d'argent) :
1. **ADR-019** (le helper-process de séparation de privilège) — dont dépendent `deploy.*` ET `browser.*`. C'est la clé de voûte.
2. **`[rule.deploy]`** validé (verdict, IDs immuables) + tes **cibles réelles**.
3. **T3 réel** vs assumer que le plafond est T2 (pour les actions qui dépensent).
4. Toujours en attente : la décision **miroir de base** (#168) et le **boot/test ISO**.

**État `main`** : propre, la trousse SaaS/ecommerce complète + relue Fable 5 + vérifiée E2E. La seule PR ouverte est #120 (ce design, flaggé pour toi). J'ai poussé tout ce qui était **sûr et autonome** ; ce qui reste est un petit ensemble de **décisions d'architecture qui te reviennent**, chacune posée concrètement, prête à exécuter dès ton feu vert.

### 2026-07-19 (dimanche, nuit) — les DEUX features phares conçues ; tout converge sur ADR-019

- **[ADR-022 / #124]** — runtime de `browser.*` conçu concrètement (le pendant de `deploy.*`), stacké sur #120. **Bonne nouvelle vérifiée** : l'arm64 n'est plus un blocage — Fedora 44 package `chromium-headless` pour x86_64 ET aarch64 (le showstopper qu'ADR-017 craignait tombe). Design : pilotage CDP par pipe (zéro port, zéro Node), isolation par le service transitoire d'ADR-019, egress par proxy CONNECT (correction d'ADR-017 : `IPAddressAllow` est par IP, pas par domaine).
- **Revue Fable 5 → 5 trous fermés**, dont une **contradiction majeure** : ma v1 mettait le proxy + le parsing CDP **dans `vibed` root**, ce qui violait ADR-019 (parser du hostile dans le moteur de politiques). Corrigé : tout le parsing hostile va dans un **helper de faible privilège**. Bonus : trancher le profil **éphémère** (pas de login persistant) **neutralise** le scénario « agir en ton nom » qu'ADR-017 acceptait — le design en sort plus sûr.

**La synthèse qui compte pour toi.** Les **deux** features phares gouvernées — `deploy.*` (#120) et `browser.*` (#124) — sont désormais **conçues concrètement, faits vérifiés, relues Fable 5**. Et elles **convergent sur une seule décision** : **ADR-019** (le service transitoire de séparation de privilège + le patron « helper de faible privilège »). C'est *la* décision qui débloque les deux d'un coup. Le reste (l'allowlist `[rule.deploy]`, tes cibles, `chromium-headless` dans l'image, un module SELinux) est mécanique une fois ADR-019 tranchée.

**État `main`** : propre. PR ouvertes = #120 (deploy design) et #124 (browser design, stackée), toutes deux flaggées pour toi, aucune auto-mergée. Tout le reste de la trousse SaaS/ecommerce est livré, relu, vérifié E2E, avec test d'intégration automatisé en CI. J'ai poussé tout le sûr-et-autonome ; il ne reste que **ta décision ADR-019** comme prochain grand déblocage.
