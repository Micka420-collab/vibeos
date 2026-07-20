# Déploiement gouverné (`deploy.*`) — guide opérateur

Réf. décisions : **ADR-021** (`deploy.*`) sur le bac à sable **ADR-019** (service
transitoire durci + helper `vibed-tool` de faible privilège). Ce document décrit
**la configuration que l'opérateur (Micka) doit fournir** pour activer les outils
de déploiement. Sans elle, `deploy.*` est **fermé par défaut** (« fail-closed ») :
la politique refuse, et le handler refuse avant même de lancer quoi que ce soit.

## Ce que fait `deploy.plan` (T2, disponible)

Lecture **en lecture seule** de l'état courant d'un déploiement, gouvernée. Le
handler lance la commande d'état du CLI du fournisseur **dans le bac à sable
ADR-019** (service `systemd-run` transitoire, `DynamicUser=yes`, égress deny-all
sauf allow-list, mémoire bornée, W^X quand le CLI le tolère), avec un **token
scellé en lecture seule** chargé depuis `$CREDENTIALS_DIRECTORY` — **jamais** en
argv. `vibed` (root) ne partage jamais son espace d'adressage avec le CLI.

Commandes exactes (aucune ne mute d'état ; la cible est épinglée par son **id
immuable**) :

| Fournisseur | Commande | Env du token | Cible = |
|-------------|----------|--------------|---------|
| `fly`     | `flyctl status -a <target> --json` | `FLY_API_TOKEN`   | nom d'app Fly (pas de rename) |
| `vercel`  | `vercel api /v9/projects/<target>` | `VERCEL_TOKEN`    | id projet `prj_…` |
| `railway` | `railway status --json`            | `RAILWAY_TOKEN`   | le token à charge résout son propre projet |

> `deploy.apply` (écriture, T3) n'existe **pas encore** : il est bloqué tant que
> Micka n'a pas booté l'image et validé `verify-sandbox = CONFINES`.

## Les 4 choses à provisionner

Racine de config : **`/etc/vibeos/deploy/`**. CLIs : **`/usr/libexec/vibeos/deploy-cli/`**.

### 1. La règle `[rule.deploy]` (qui autorise quelle cible)

Sans règle, **tout déploiement est refusé** (verdict par défaut). La règle liste
les paires `(provider, target)` exactes autorisées. Ajouter dans un fichier de
`/etc/vibeos/policy.d/` (T2 minimum requis ; ne peut pas coexister avec
`[rule.domains]` dans la même règle) :

```toml
[[rule]]
tool = "deploy.plan"
tier = "T2"                     # plancher : approbation humaine à chaque appel

[rule.deploy]
allowed = [
  { provider = "fly",     target = "mon-app" },
  { provider = "vercel",  target = "prj_XXXXXXXXXXXXXXXX" },
]
```

Le verdict est **liste blanche** (comme `services.allowed`) : une paire hors liste
est refusée **avant** le plancher de tier. Il n'y a **pas** de règle `deploy`
livrée dans l'image — c'est à vous de l'ajouter.

### 2. Le token scellé en lecture seule (TPM2)

Le token est **scellé par TPM2** via `systemd-creds` et chargé par le service
transitoire (`LoadCredentialEncrypted`) dans `$CREDENTIALS_DIRECTORY/deploy-token`
(0400, uid distinct). Le token n'est **jamais** en argv, ni loggé, ni partagé avec
`vibed`. Chemin attendu :

```
/etc/vibeos/deploy/creds/<provider>-<target>.cred
```

Sceller (exemple fly) :

```sh
# Créer un token à PORTÉE LECTURE SEULE côté fournisseur — c'est LA garantie réelle :
#   fly tokens create readonly            → FLY_API_TOKEN
#   vercel : token de compte à scope lecture
#   railway : token de projet
printf '%s' "$LE_TOKEN_READONLY" \
  | systemd-creds encrypt --name=deploy-token - \
      /etc/vibeos/deploy/creds/fly-mon-app.cred
```

> La lecture-seule de bout en bout **repose sur la portée du token**, pas
> seulement sur la commande : `plan_command` ne construit qu'un `status`/`GET`, et
> le helper refuse tout sous-commande mutante, mais un token à droits d'écriture
> resterait dangereux si un jour la commande changeait. **Scellez toujours un
> token en lecture seule.**

### 3. La liste blanche d'égress (CIDR de l'API)

Le bac à sable a un plancher **égress deny-all**. Le déploiement parle au réseau
(l'API du fournisseur), il faut donc autoriser explicitement les CIDR de cette
API — et **rien d'autre** (la télémétrie type `flyctl-metrics.fly.dev` reste
bloquée, on ne l'ajoute jamais). **Absence de fichier ⇒ refus avant tout spawn.**

```
/etc/vibeos/deploy/<provider>.egress
```

Format : un CIDR par ligne ; `#` en commentaire ; lignes vides ignorées.

```
# /etc/vibeos/deploy/fly.egress — CIDR de l'API Fly.io
66.241.124.0/24
```

Récupérez les plages courantes dans la doc du fournisseur (elles évoluent ; c'est
de la config opérateur, pas une constante gravée dans l'image).

### 4. Le binaire du CLI dans le sandbox

L'uid confiné (`DynamicUser`) ne voit **pas** votre `~/.local/bin`. Le CLI doit
vivre à un chemin système que le sandbox atteint, résolu en **absolu** :

```
/usr/libexec/vibeos/deploy-cli/flyctl
/usr/libexec/vibeos/deploy-cli/vercel
/usr/libexec/vibeos/deploy-cli/railway
```

Provisionnez-les par couche d'image ou installation système. Note W^X :
`flyctl` (Go) tourne avec `MemoryDenyWriteExecute=yes` ; `vercel`/`railway`
(Node/V8 JIT) tournent avec le W^X relâché — le handler choisit le profil selon
le fournisseur, rien à faire côté opérateur.

## Chaîne de contrôle (résumé sécurité)

1. **Politique** — la paire `(provider, target)` doit être dans `[rule.deploy]`,
   sinon refus (verdict liste blanche, avant le plancher de tier).
2. **Approbation humaine** — plancher T2 : chaque `deploy.plan` demande votre OK.
3. **Bac à sable ADR-019** — service transitoire durci, égress limité à
   `<provider>.egress`, mémoire bornée, sortie bornée (pas d'OOM), HOME éphémère.
4. **Token** — scellé TPM2, chargé par env depuis `$CREDENTIALS_DIRECTORY`, jamais
   en argv, à portée **lecture seule**.
5. **Audit** — l'appel est journalisé avec sa cible dérivée `provider:target` dans
   la chaîne d'audit hachée SHA-256.
6. **Sortie** — la sortie du CLI est renvoyée **verbatim comme donnée opaque**,
   jamais interprétée comme vérité ni comme instruction.
