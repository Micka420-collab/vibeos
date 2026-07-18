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
