
### 2026-07-18 (suite) — ADR-020 écrite, en revue Fable 5

- **3 rapports de recherche rendus** (dev toolchains, deploy CLIs + briques self-hosted, perf/observabilité). Faits vérifiés : licences amont, dispo dépôts F44 réels, arm64, offline.
- **Socle « ship » vérifié présent dans F44** (versions réelles) : postgresql 18.3, valkey 9.0.4, sqlite 3.51, caddy 2.10, nginx 1.30, mkcert 1.4.4, podman-compose 1.6, uv 0.11, ruff 0.15, mypy 1.18, + gh 2.94, perf/sysstat/bpftrace/bcc/node-exporter.
- **[ADR-020](docs/DECISIONS.md) écrite et ouverte en [PR #92](https://github.com/Micka420-collab/vibeos/pull/92).** Décision (justifiée, en autonomie) : le touseau SaaS est une **seconde trousse gouvernée**, même modèle que la cybersécurité — 3 seaux (embarqué / à la demande / référence), pièges de licence documentés (Redis→Valkey, MinIO exclu, n8n/Directus/Sentry/WebPageTest en référence), et une gouvernance où le **dev local est T1** mais le **déploiement prod est un T2/T3 gouverné futur** (dépend d'ADR-019 + d'une allowlist que tu tranches).
- **Revue adversariale Fable 5 lancée** sur l'ADR (licences, cohérence doctrinale, trous de gouvernance, bloat, arm64). Je traite ses trouvailles **avant** de figer l'ADR et d'implémenter.

**Décision que je te laisse** (pas urgente, notée pour ton retour) : l'outil `vibed` `deploy.*` gouverné a besoin de ton **allowlist de cibles** (quels projets/environnements l'IA peut déployer) — sœur du `[rule.domains]` du navigateur. Je ne le construis pas sans ça.
