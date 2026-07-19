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

**Seuls 80/443 et ssh sont publics. La base, le cache, Meilisearch, l'admin
Grafana ne sortent JAMAIS sur l'interface publique.** Les modèles de la trousse
lient déjà `127.0.0.1` : garde ça en prod, et ne publie QUE le reverse-proxy.

⚠️ **Regarde ce qui est DÉJÀ ouvert — n'assume rien.** Une install Fedora Server
laisse souvent `cockpit` (9090, un portail de login) et `dhcpv6-client` actifs :
tu te croirais en 80/443/ssh alors que **cockpit est public**. Les commandes qui
suivent n'ajoutent que ; c'est la liste FINALE qui compte, pas ce que tu tapes.

```sh
sudo firewall-cmd --list-all                 # 1) LIS la liste réelle
sudo firewall-cmd --permanent --remove-service=cockpit   # 2) ferme l'inutile (adapte)
sudo firewall-cmd --permanent --add-service=http --add-service=https --add-service=ssh
sudo firewall-cmd --reload
sudo firewall-cmd --list-all                 # 3) RE-vérifie : http/https/ssh, rien d'autre
```

## 1-bis. Vérifie ce que tu exposes VRAIMENT (audit défensif, avant d'ouvrir)

Le §1 *déclare* une intention de pare-feu ; il ne **prouve** pas qu'elle tient.
VibeOS est security-first et livre une trousse cybersécu — sers-t'en **sur ta
propre infra**, en défensif, avant d'exposer quoi que ce soit :

- **Confirme la surface réseau depuis DEHORS.** `nmap` lancé *sur le serveur
  lui-même* ment : le trafic loopback/local ne traverse pas firewalld comme un
  paquet venu d'Internet. Scanne depuis une **autre machine** (un VPS, ton poste,
  un shell cloud) :
  ```sh
  nmap -Pn -p- <IP-PUBLIQUE-DU-SERVEUR>     # depuis un AUTRE hôte, pas le serveur
  # attendu : 22, 80, 443 open ; TOUT le reste closed/filtered.
  # 5432/6379/7700 (base/cache/search), 3000 (Grafana) ou 9090 (cockpit) ouverts
  # = STOP, reviens au §1 : ta base est publique.
  ```
- **Audite le durcissement de l'hôte.** `lynis` note la config système (SSH,
  noyau, services, permissions) et liste des correctifs concrets :
  ```sh
  sudo lynis audit system            # index de durcissement + warnings/suggestions
  ```
  Traite d'abord les `[WARNING]` ; les `[SUGGESTION]` selon ton modèle de menace.

> ⚖️ **Gouvernance.** Ceci est de l'audit **défensif de TA PROPRE infra** —
> légitime, non gouverné. La trousse contient aussi des outils *offensifs* : les
> pointer sur une cible **tierce** est du **T2/T3** (autorisation requise), pas ce
> dont il s'agit ici. Tu scannes/audites **ce que tu possèdes**.

## 2. Vrai TLS : Caddy en frontal (un seul port public)

En prod, Caddy termine le TLS pour un **vrai domaine** et obtient un certificat
Let's Encrypt tout seul. Remplace le `Caddyfile` de dev par :

```
monsaas.example.com {
    # host.containers.internal = l'hôte vu DEPUIS le conteneur Caddy (podman).
    # `localhost:3000` ne marcherait que si Caddy tournait dans le netns de l'hôte
    # — sinon c'est le loopback DU conteneur (502, pas une fuite, mais ça ne marche pas).
    reverse_proxy host.containers.internal:3000
    encode gzip zstd
}
```

Fais pointer le DNS `A`/`AAAA` de `monsaas.example.com` vers ton serveur ; Caddy
gère le certificat + le renouvellement. **Pré-requis ACME** : 80 et 443 joignables
depuis Internet.

⚠️ **Rootless + ports < 1024.** Ce guide est *rootless* (`systemctl --user`), or
podman rootless ne peut pas lier `:80`/`:443` par défaut. **NE « corrige » PAS ça
avec `sudo podman`** : les ports publiés en **rootful** contournent firewalld
(règles netavark) et rendent le §1 caduc. Autorise plutôt les ports bas en rootless :

```sh
echo 'net.ipv4.ip_unprivileged_port_start=80' | sudo tee /etc/sysctl.d/99-vibeos-ports.conf
sudo sysctl --system
```

## 3. Secrets : générés, jamais tapés en clair, jamais dans git

Le `.env` des modèles est pour le **dev**. En prod, **podman secrets** — et
**génère** le mot de passe, ne le tape pas : sur VibeOS l'IA partage ton shell, et
l'historique / les transcripts gardent tout ce que tu saisis.

```sh
openssl rand -base64 32 | podman secret create pg_password -   # généré, jamais tapé
```

Référence-le dans le service, **en retirant** `POSTGRES_PASSWORD` de `environment`
ET du `.env` (les deux à la fois = l'entrée postgres refuse de démarrer) :

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

Note : le driver fichier de podman stocke le secret en base64 (≈ clair) sous
`~/.local/share/containers/...` — mieux qu'un `.env` de projet qu'un agent lit,
mais pas scellé. Pour du vrai scellement, **systemd-creds** TPM2 (déjà dans
l'image, cf. la trousse sécurité). Ne commite **jamais** un secret.

## 4. Démarrage automatique : systemd (Quadlet), pas un shell

Un `podman compose up` dans un terminal meurt au logout. La voie **supportée** sur
podman 5.x (ce que livre F44) est **Quadlet** : un fichier `.container` déclaratif
que systemd gère. `podman generate systemd` marche encore mais est **déprécié**
depuis podman 4.4. Exemple minimal `~/.config/containers/systemd/monsaas-db.container` :

```ini
[Container]
Image=docker.io/library/postgres:18
Secret=pg_password,type=env,target=POSTGRES_PASSWORD
Volume=pgdata.volume:/var/lib/postgresql

[Service]
Restart=on-failure

[Install]
WantedBy=default.target
```

```sh
systemctl --user daemon-reload            # génère l'unité depuis le .container
systemctl --user start monsaas-db.service
loginctl enable-linger "$USER"            # tourne hors session ET au boot du serveur
```

`Restart=on-failure` relance après un crash ; le `linger` démarre l'unité au boot.

## 5. Sauvegardes : l'état vit dans des volumes — dumpe-le dehors, sans mentir

Un volume nommé n'est pas une sauvegarde. **Piège classique** : `pg_dump | gzip >
f.gz` renvoie le code de sortie de `gzip`, PAS de `pg_dump` — un dump échoué (base
coupée, disque plein) produit quand même un `.gz` « réussi » minuscule. Un timer
qui automatise ce one-liner ne verrait **jamais** l'échec : des mois de
sauvegardes vides, découvert à la restauration. Fais un script honnête :

```sh
#!/usr/bin/env bash
set -euo pipefail                       # pipefail : l'échec de pg_dump fait échouer
out="backup-$(date +%F).sql.gz"
tmp="$(mktemp)"
podman exec monsaas-db pg_dump -U app -d app | gzip > "$tmp"
[ -s "$tmp" ] || { echo "dump vide — abandon" >&2; exit 1; }
mv "$tmp" "$out"                        # renomme SEULEMENT si tout a réussi
# + pg_dumpall --globals-only pour les rôles ; CHIFFRE le dump avant de le copier
#   hors serveur (il contient toutes tes données prod).
```

- **Meilisearch** : snapshot via l'API (doc `/snapshots`) ou copie de `/meili_data` à l'arrêt.
- **Objet (SeaweedFS)** : `aws --endpoint-url http://127.0.0.1:8333 s3 sync s3://uploads ./backup-uploads/` (ton endpoint local, pas AWS).

Automatise (timer systemd) **avec alerte à l'échec**, copie les dumps **ailleurs**
(chiffrés), et **teste une restauration** — une sauvegarde jamais restaurée n'en
est pas une.

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
- [ ] Surface réseau **vérifiée depuis l'extérieur** (`nmap`), hôte audité (`lynis`)
- [ ] Caddy : vrai domaine, DNS pointé, TLS Let's Encrypt automatique
- [ ] Secrets en podman secrets / systemd-creds — rien dans git
- [ ] Unités systemd + `linger` (démarrage auto, restart)
- [ ] Sauvegardes datées, copiées hors serveur, **restauration testée**
- [ ] Images épinglées par digest
- [ ] Tu sais, et tu assumes, **ce qui** tourne en prod
