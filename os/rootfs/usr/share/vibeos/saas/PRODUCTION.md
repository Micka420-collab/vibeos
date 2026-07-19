# Mettre un SaaS en production — self-hosted, sur ton serveur

Le [QUICKSTART](QUICKSTART.md) t'amène en **dev local** (loopback, `.env`, mkcert).
La **production self-hosted** est un autre métier : ports publics, vrai TLS, vrais
secrets, démarrage auto, sauvegardes, pare-feu. Ce runbook couvre l'essentiel,
avec les bons réflexes — **pas de raccourci qui expose une base**.

> ⚠️ **Gouvernance (rappel).** Déployer en prod agit dehors, avec des secrets
> réels : c'est du **T2/T3**. La future capacité `vibed` `deploy.*` encadrera ça
> (allowlist + approbation humaine sur le contenu). Tant qu'elle n'est pas là,
> **c'est toi qui réponds de ce qui part en prod.** Ce guide ne dispense pas de
> cette responsabilité.

## 0. Les 5 différences dev → prod

| | Dev (QUICKSTART) | Prod (ce guide) |
|---|---|---|
| Écoute | `127.0.0.1` seulement | Caddy sur `:80/:443` **public**, le reste reste loopback |
| TLS | `mkcert` (CA locale) | **Let's Encrypt automatique** (Caddy, vrai domaine) |
| Secrets | `.env` en clair | **podman secrets** / systemd-creds — jamais dans git |
| Démarrage | `podman compose up` à la main | **unités systemd** (auto-start, restart, boot) |
| État | volumes jetables | volumes **sauvegardés** (dump régulier hors serveur) |

## 1. Le pare-feu d'abord (avant d'exposer quoi que ce soit)

**Seuls 80 et 443 sont publics. La base, le cache, Meilisearch, l'admin Grafana
ne sortent JAMAIS sur l'interface publique.** Les modèles de la trousse lient
déjà `127.0.0.1` : garde ça en prod, et ne publie QUE le reverse-proxy.

```sh
sudo firewall-cmd --permanent --add-service=http --add-service=https
sudo firewall-cmd --permanent --add-service=ssh        # garde ton accès
sudo firewall-cmd --reload
```

## 2. Vrai TLS : Caddy en frontal (un seul port public)

En prod, Caddy termine le TLS pour un **vrai domaine** et obtient un certificat
Let's Encrypt tout seul. Remplace le `Caddyfile` de dev par :

```
monsaas.example.com {
    reverse_proxy localhost:3000       # ton app (ou le conteneur applicatif)
    encode gzip zstd
}
```

Fais pointer l'enregistrement DNS `A`/`AAAA` de `monsaas.example.com` vers ton
serveur, publie Caddy sur `:80`+`:443` (les seuls ports publics), et il gère le
certificat + le renouvellement. **Pré-requis** : 80 et 443 accessibles depuis
Internet (le challenge ACME en a besoin).

## 3. Secrets : jamais en clair, jamais dans git

Le `.env` des modèles est pour le **dev**. En prod, utilise **podman secrets** :

```sh
printf '%s' 'un-vrai-mot-de-passe-fort' | podman secret create pg_password -
```

…et référence-le dans le service (compose) :

```yaml
services:
  db:
    image: docker.io/library/postgres:18
    secrets: [pg_password]
    environment:
      POSTGRES_PASSWORD_FILE: /run/secrets/pg_password   # postgres lit le _FILE
secrets:
  pg_password:
    external: true
```

Alternative VibeOS-native : **systemd-creds** scellé TPM2 (déjà dans l'image, cf.
la trousse sécurité) pour les secrets d'unités. Ne commite **jamais** un secret.

## 4. Démarrage automatique : systemd, pas un shell

Un `podman compose up` dans un terminal meurt au logout. En prod, génère des
unités systemd (Quadlet, la voie moderne de podman) ou :

```sh
# À partir des conteneurs lancés, génère des unités utilisateur :
podman generate systemd --new --files --name monsaas-db
mkdir -p ~/.config/systemd/user && mv container-*.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now container-monsaas-db.service
loginctl enable-linger "$USER"     # les unités user tournent hors session
```

`Restart=on-failure` (posé par `generate systemd`) relance après un crash ; le
`linger` les fait démarrer au boot du serveur.

## 5. Sauvegardes : l'état vit dans des volumes — dumpe-le dehors

Un volume nommé n'est pas une sauvegarde. Dump régulier, **hors du serveur** :

```sh
# PostgreSQL — dump logique, compressé, daté (via l'horloge de l'hôte)
podman exec monsaas-db pg_dump -U app -d app | gzip > "backup-$(date +%F).sql.gz"
# Meilisearch — snapshot via l'API (voir la doc /snapshots) ou copie de /meili_data à l'arrêt
# Objet (SeaweedFS) — réplique le bucket : aws s3 sync s3://uploads ./backup-uploads/
```

Automatise-le (timer systemd) et **copie les dumps ailleurs** (autre machine,
stockage objet distant). Teste une **restauration** au moins une fois — une
sauvegarde jamais restaurée n'en est pas une.

## 6. Mises à jour

```sh
podman pull docker.io/library/postgres:18        # ou l'image épinglée par digest
systemctl --user restart container-monsaas-db.service
```

Épingle tes images **par digest** en prod (reproductibilité), et relis les notes
de version avant un bump majeur (surtout Postgres : une migration majeure de
version n'est pas un simple `pull`).

## 7. Le mode cloud managé (l'autre voie)

Si tu ne veux pas administrer un serveur : `install-saas-tool flyctl` (ou
`railway`), `fly launch` + `fly deploy`. Le fournisseur gère TLS, secrets,
démarrage, backups. Rappel gouvernance : `fly deploy` reste **T2/T3** (voir en
tête). Catalogue et arbitrage des fournisseurs : `docs/ECOSYSTEM.md`.

## Checklist prod

- [ ] Pare-feu : seuls 80/443/ssh ouverts ; DB/cache/search jamais publics
- [ ] Caddy : vrai domaine, DNS pointé, TLS Let's Encrypt automatique
- [ ] Secrets en podman secrets / systemd-creds — rien dans git
- [ ] Unités systemd + `linger` (démarrage auto, restart)
- [ ] Sauvegardes datées, copiées hors serveur, **restauration testée**
- [ ] Images épinglées par digest
- [ ] Tu sais, et tu assumes, **ce qui** tourne en prod
