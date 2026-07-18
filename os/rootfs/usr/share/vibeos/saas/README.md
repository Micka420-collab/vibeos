# Modèles SaaS — le substrat serveur, en conteneurs PAR PROJET

Ces modèles `compose` donnent à un projet SaaS/ecommerce son socle serveur —
base de données, cache, reverse-proxy — **sans rien graver dans l'OS immuable**.

## Pourquoi des conteneurs, pas des services système

VibeOS est immuable et security-first. Un `postgresql-server` gravé dans `/usr`
serait un **service réseau persistant** dans l'image de tout le monde : socket en
écoute, uid dédié, état mutable — une surface d'attaque permanente (voir
[ADR-020](../../../../../../docs/DECISIONS.md)). La bonne réponse bootc est
l'inverse : l'image livre les **clients** et l'**orchestrateur** (`psql`,
`redis-cli`, `podman` + `podman-compose`, déjà présents), et **chaque projet**
lance ses serveurs en conteneurs, sous l'uid de l'utilisateur, avec état sous son
`$HOME`. Rien ne survit dans l'image ; tout est jetable et par projet.

C'est le même choix que pour Supabase/Medusa (catalogue [ECOSYSTEM](../../../../../../docs/ECOSYSTEM.md)) :
des stacks conteneurs qu'on instancie, jamais qu'on grave.

## Usage

Copiez un modèle dans votre projet, adaptez-le, lancez-le :

```sh
mkdir -p ~/monsaas && cp -r /usr/share/vibeos/saas/postgres-valkey ~/monsaas/infra
cd ~/monsaas/infra
# éditez .env (mot de passe, ports)
podman compose up -d
```

L'état (données Postgres, dump Valkey) vit dans des **volumes nommés** du projet,
pas dans l'image. `podman compose down` arrête ; `down -v` efface tout.

## Modèles fournis

| Dossier | Donne |
|---|---|
| `postgres-valkey/` | PostgreSQL 18 + Valkey (cache/broker) — le socle d'un SaaS |
| `reverse-proxy/` | Caddy + TLS local via la CA `mkcert` (0 réseau, `https://` en dev) |

## Gouvernance (rappel)

Lancer ces conteneurs en **dev local** est du **T1** (la machine de l'utilisateur).
Le **déploiement en production** — pousser vers Fly/Vercel/un serveur — est du
**T2/T3** : une capacité gouvernée à venir (`deploy.*` dans `vibed`), qui exigera
une allowlist de cibles et l'approbation humaine sur le contenu déployé. Voir
ADR-020.

> ⚠️ Les images (`docker.io/library/postgres`, `valkey/valkey`, `caddy`) sont
> tirées **par vous** au premier `up`, pas fournies par VibeOS. Épinglez-les par
> digest dans vos projets si vous voulez la reproductibilité.
