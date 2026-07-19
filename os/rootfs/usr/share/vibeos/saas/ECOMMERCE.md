# Une boutique ecommerce de A à Z — Medusa sur le substrat VibeOS

Le [QUICKSTART](QUICKSTART.md) monte un SaaS générique. Ce runbook monte une vraie
**boutique** : **Medusa** (backend ecommerce headless, **MIT**, v2) branché sur les
modèles compose déjà livrés — **base + cache** (postgres-valkey), **images produit**
(object-storage), **recherche produit** (meilisearch). Tout tourne sous ton `$HOME`.

> Faits vérifiés sur `docs.medusajs.com` (Medusa v2.17, 2026). Les modèles compose
> sont, eux, **smoke-testés**. La colle Medusa↔modèles ci-dessous est *documentée*,
> pas E2E ici — teste un upload d'image et une recherche comme indiqué à la fin.
>
> ⚠️ **v2 uniquement.** Tout tuto qui parle de `medusa-config.js`, `plugins: [...]`
> à l'ancienne, du port 7001, ou de `medusa-plugin-meilisearch`/`medusa-file-s3`
> **non scopés** est du **v1** — à jeter.

## 1. Le substrat (3 conteneurs par projet)

```sh
mkdir -p ~/maboutique && cd ~/maboutique
for m in postgres-valkey object-storage meilisearch; do
  cp -r /usr/share/vibeos/saas/$m ./$m && (cd $m && cp .env.example .env)
done
# éditez chaque .env (secrets — voir 🔑 dans le QUICKSTART : openssl rand)
(cd postgres-valkey && podman compose up -d)   # PostgreSQL 18 + Valkey
(cd object-storage  && podman compose up -d)   # SeaweedFS S3 : http://127.0.0.1:8333
(cd meilisearch     && podman compose up -d)   # Meilisearch  : http://127.0.0.1:7700
# crée le bucket des médias :
aws --endpoint-url http://127.0.0.1:8333 s3 mb s3://medusa-media
```

## 2. Échafauder Medusa

`create-medusa-app` **ne provisionne PAS** Postgres — il utilise celui qui tourne
(étape 1). Node **20+** (Node 24 de l'image convient).

```sh
cd ~/maboutique
npx create-medusa-app@latest store \
  --db-url "postgres://app:app@127.0.0.1:5432/app" \
  --no-browser
cd store
```

## 3. Câbler Medusa sur le substrat (`medusa-config.ts` + `.env`)

`.env` de Medusa :

```sh
DATABASE_URL=postgres://app:app@127.0.0.1:5432/app
REDIS_URL=redis://127.0.0.1:6379          # Valkey (compatible protocole Redis)
EVENTS_REDIS_URL=redis://127.0.0.1:6379
WE_REDIS_URL=redis://127.0.0.1:6379
# stockage objet (SeaweedFS de l'étape 1)
S3_ENDPOINT=http://127.0.0.1:8333
S3_FILE_URL=http://127.0.0.1:8333/medusa-media
S3_BUCKET=medusa-media
S3_ACCESS_KEY_ID=<ta S3_ACCESS_KEY>
S3_SECRET_ACCESS_KEY=<ton S3_SECRET_KEY>
S3_REGION=us-east-1                        # arbitraire mais requis ; SeaweedFS l'ignore
# recherche
MEILISEARCH_HOST=http://127.0.0.1:7700
MEILISEARCH_API_KEY=<ta MEILI_MASTER_KEY>
```

`medusa-config.ts` — modules DB/cache, **fichier S3** (avec le contournement du
piège checksum, voir la note), et le **plugin Meilisearch** :

```ts
module.exports = defineConfig({
  projectConfig: {
    databaseUrl: process.env.DATABASE_URL,
    redisUrl: process.env.REDIS_URL,               // sessions
  },
  modules: [
    { resolve: "@medusajs/medusa/event-bus-redis",
      options: { redisUrl: process.env.EVENTS_REDIS_URL } },
    { resolve: "@medusajs/medusa/workflow-engine-redis",
      options: { redis: { redisUrl: process.env.WE_REDIS_URL } } },
    { resolve: "@medusajs/medusa/file",
      options: { providers: [ {
        resolve: "@medusajs/medusa/file-s3", id: "s3",
        options: {
          file_url: process.env.S3_FILE_URL,
          access_key_id: process.env.S3_ACCESS_KEY_ID,
          secret_access_key: process.env.S3_SECRET_ACCESS_KEY,
          region: process.env.S3_REGION,
          bucket: process.env.S3_BUCKET,
          endpoint: process.env.S3_ENDPOINT,
          additional_client_config: {
            forcePathStyle: true,                  // S3-compatible (SeaweedFS)
            // ⚠️ SANS ces deux lignes, l'AWS SDK v3 envoie des checksums CRC32
            // que SeaweedFS rejette/corrompt → uploads d'images cassés.
            requestChecksumCalculation: "WHEN_REQUIRED",
            responseChecksumValidation: "WHEN_REQUIRED",
          },
        },
      } ] } },
  ],
  plugins: [
    { resolve: "@rokmohar/medusa-plugin-meilisearch",   // v2 ; épinglez-le (voir note)
      options: { config: {
        host: process.env.MEILISEARCH_HOST,
        apiKey: process.env.MEILISEARCH_API_KEY,
      }, settings: { products: {
        type: "products", enabled: true,
        fields: ["id", "title", "description", "handle", "variant_sku", "thumbnail"],
        indexSettings: {
          searchableAttributes: ["title", "description", "variant_sku"],
          filterableAttributes: ["id", "handle"],
        },
        primaryKey: "id",
      } } } },
  ],
})
```

```sh
npm install --save @rokmohar/medusa-plugin-meilisearch
```

## 4. Migrer, seed, admin, lancer

```sh
npx medusa db:migrate                                   # migrations + liens
npm run seed                                            # données de démo
npx medusa user -e admin@example.com -p supersecret     # crée l'admin
npm run dev                                             # http://localhost:9000
```

- API boutique + admin : **http://localhost:9000**
- Dashboard admin : **http://localhost:9000/app** (intégré au serveur en v2, plus de port 7001)

## 5. Vérifier la colle (ne suppose pas — teste)

- **Images → SeaweedFS** : dans l'admin, crée un produit et **upload une image**. Puis
  `aws --endpoint-url http://127.0.0.1:8333 s3 ls s3://medusa-media/` doit la lister.
  Si l'upload échoue : c'est le **piège checksum** — vérifie les deux lignes
  `*ChecksumCalculation`/`*Validation` ci-dessus.
- **Recherche → Meilisearch** : après un `npm run dev`, le plugin indexe ; interroge
  `curl -H "Authorization: Bearer $MEILI_MASTER_KEY" http://127.0.0.1:7700/indexes`
  et tu dois voir un index `products`.

## Notes honnêtes (pièges connus)

- **Valkey** : Medusa ne documente pas Valkey ; la compatibilité tient au **niveau
  client** (ioredis/BullMQ parlent RESP à un fork de Redis 7.2). Solide, mais
  « compatible Redis » plutôt que « supporté Medusa ».
- **Plugin Meilisearch communautaire** : `@rokmohar/medusa-plugin-meilisearch` est
  **v2-natif** mais mono-mainteneur. **Épingle-le avec ta version de Medusa** (un
  bump mineur de Medusa peut le devancer). Il n'existe **pas** de module de recherche
  officiel Medusa. N'utilise **jamais** le `medusa-plugin-meilisearch` non scopé (v1).
- **SeaweedFS + AWS SDK v3** : le piège checksum ci-dessus est **réel** (issues
  SeaweedFS #6548/#6713…). Les deux options `WHEN_REQUIRED` sont l'échappatoire
  documentée. Teste un vrai upload.
- **Redis modules** : sans event-bus/workflow-engine configurés, Medusa retombe en
  **in-memory** — OK en dev, faux en prod.
- **Prod** : `npm run build` puis `npm run start`, derrière le reverse-proxy et selon
  [PRODUCTION.md](PRODUCTION.md) (TLS réel, secrets, systemd, sauvegardes).

Medusa est **MIT**, v2 est la ligne active (v2.17, 2026). Catalogue complet du
substrat : [README.md](README.md).
