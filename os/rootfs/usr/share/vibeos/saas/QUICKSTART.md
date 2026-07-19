# SaaS de A à Z — le runbook

Un parcours concret, du dossier vide au déploiement, **avec les outils déjà là**.
Chaque étape n'utilise que ce que VibeOS livre (couche `1d-ter`) ou installe à la
demande (`install-saas-tool`). Rien ne touche l'OS immuable ; tout vit dans votre
projet et votre `$HOME`.

> Runtimes déjà présents : **Node 24** (`node`/`npm`), **Python 3.13**
> (`python3`/`pip`, + `uv`/`ruff`/`mypy`), **git**, **podman** + **podman-compose**.

## A — Échafauder le projet

```sh
# SaaS JS/TS
npm create next-app@latest monsaas    # ou: npm create vite@latest
# …ou API Python
uv init monapi && cd monapi && uv add fastapi uvicorn
# …ou ecommerce
npx create-medusa-app@latest maboutique
```

## B — Le socle serveur (base + cache), en conteneurs par projet

```sh
cp -r /usr/share/vibeos/saas/postgres-valkey ~/monsaas/infra
cd ~/monsaas/infra
cp .env.example .env      # éditez POSTGRES_PASSWORD (un vrai secret)
podman compose up -d      # PostgreSQL 18 + Valkey, ports loopback-only
psql -h localhost -U app -d app -c '\l'   # le CLIENT psql est dans l'image
```

L'état vit dans des **volumes nommés** du projet. `podman compose down` arrête,
`down -v` efface tout. Voir [README.md](README.md) pour le *pourquoi conteneurs*.

> 🔑 **Générez de vrais secrets**, ne laissez pas les placeholders des `.env` :
> `sed -i "s/change-me.*/$(openssl rand -hex 24)/" .env` (ou éditez à la main).
> Les placeholders sont connus publiquement — un service en loopback avec des
> creds connus reste une cible (DNS-rebinding). Idem pour `MEILI_MASTER_KEY`,
> `S3_*` et `GRAFANA_ADMIN_PASSWORD` des autres modèles.

## C — Développer

```sh
cd ~/monsaas
npm run dev        # ou: uv run uvicorn main:app --reload
ruff check .       # lint Python ; mypy . pour le typage
```

Migrations : au choix de la stack (Prisma/Drizzle via `npm`, Alembic via `uv`) —
installés dans le projet, jamais dans `/usr`.

## C-bis — Services annexes (au besoin)

```sh
# Tester les emails (inscription, reset) sans en envoyer — UI web sur :8025
cp -r /usr/share/vibeos/saas/mailpit ~/monsaas/mail
(cd ~/monsaas/mail && podman compose up -d)   # pointez le SMTP de l'app sur localhost:1025

# Stockage objet S3-compatible (uploads / images produit ecommerce)
cp -r /usr/share/vibeos/saas/object-storage ~/monsaas/s3 && cd ~/monsaas/s3
cp .env.example .env                            # éditez S3_ACCESS_KEY / S3_SECRET_KEY
podman compose up -d                            # endpoint S3 : http://localhost:8333
aws --endpoint-url http://localhost:8333 s3 mb s3://uploads

# Recherche produit / plein-texte (ecommerce) — Meilisearch
cp -r /usr/share/vibeos/saas/meilisearch ~/monsaas/search && cd ~/monsaas/search
cp .env.example .env                            # éditez MEILI_MASTER_KEY (≥16 octets)
podman compose up -d                            # API : http://localhost:7700 (Bearer <clé>)
```

## D — HTTPS local valide (offline)

```sh
cp -r /usr/share/vibeos/saas/reverse-proxy ~/monsaas/proxy && cd ~/monsaas/proxy
mkcert -install                                   # CA locale dans le trust store
mkcert -cert-file certs/local.pem -key-file certs/local-key.pem \
       monsaas.localhost "*.monsaas.localhost"
# éditez le Caddyfile (upstream = votre port de dev), puis :
podman compose up -d                              # https://monsaas.localhost:8443
```

## E — Mesurer la performance

```sh
ab -n 1000 -c 50 http://localhost:3000/           # livré dans l'image (basique)
# montées en gamme, à la demande (épinglées + sha256 vérifié) :
/usr/libexec/vibeos/install-saas-tool oha         # → ~/.local/bin/oha (TUI, HTTP/2)
/usr/libexec/vibeos/install-saas-tool vegeta      # charge à débit constant
oha -z 30s http://localhost:3000/
```

Profil système sous charge : `perf`, `sar`/`pidstat` (`sysstat`), `bpftrace`,
`bcc` — tous dans l'image.

**Métriques applicatives + dashboards** (l'analyse de perf du SaaS dans la durée) :

```sh
cp -r /usr/share/vibeos/saas/observability ~/monsaas/obs && cd ~/monsaas/obs
cp .env.example .env      # éditez GRAFANA_ADMIN_PASSWORD
podman compose up -d      # Grafana : http://localhost:3000 (Prometheus déjà branché)
```

Exposez `/metrics` dans votre app, puis décommentez le job `monapp` dans
`prometheus/prometheus.yml`.

## Z — Déployer

```sh
/usr/libexec/vibeos/install-saas-tool flyctl      # ou railway ; vercel/wrangler via npm
fly launch      # génère fly.toml
fly deploy
```

> ⚠️ **La gouvernance.** Lancer tout ce qui précède en **dev local** est du
> **T1** (votre machine). **Déployer en prod** — `fly deploy`, `vercel --prod`,
> `railway up` — agit dehors avec des **credentials cloud** : c'est **T2/T3**.
> Aujourd'hui ces CLIs tournent dans votre shell ; l'enveloppe **gouvernée**
> (`deploy.*` dans `vibed` : allowlist de cibles + approbation humaine sur le
> **contenu** déployé) est une capacité **à venir** — voir ADR-020. Tant qu'elle
> n'est pas là, c'est à vous de garder la main sur *ce qui* part en prod.

**Pour du self-hosted en production** (vrai TLS, secrets, systemd, sauvegardes,
pare-feu) — un autre métier que le dev local : voir **[PRODUCTION.md](PRODUCTION.md)**.

## La carte complète

Les 3 seaux (embarqué / à la demande / référence self-hosted), les briques
self-hosted (Supabase, Medusa, Umami, Grafana) et les pièges de licence :
`docs/ECOSYSTEM.md` du dépôt.
