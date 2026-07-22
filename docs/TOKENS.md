# Gestion des tokens dans VibeOS

> Référence v0.1 — 2026-07-22. Conception : [ADR-030](DECISIONS.md).
> Code : [`vibed/src/supervisor.rs`](../vibed/src/supervisor.rs) (comptage + modes),
> câblage dans [`vibed/src/vibectl.rs`](../vibed/src/vibectl.rs) (`agent run`).

VibeOS présente l'IA comme un **citoyen** qui vit longtemps sur la machine. Un
citoyen qui travaille consomme des **tokens** — la ressource qui coûte, à
l'humain (facture) comme au contexte (fenêtre finie). Ce document répond à deux
questions : **combien on consomme**, et **comment le gérer**.

Principe directeur : **on mesure, on ne devine pas.** Le compte de tokens est
exact (il vient du flux du CLI). Le coût en devise, lui, change ; on journalise
le coût **que le CLI calcule** (`total_cost_usd`, prix toujours à jour) et, pour
le reste, on raisonne en **équivalents-token** via des multiplicateurs
**structurels** (voir §2), jamais en prix codés en dur qui périmeraient.

---

## 1. Les quatre compteurs

Chaque tour d'un agent (un appel d'API) rapporte quatre nombres dans le bloc
`usage` du flux `stream-json`. VibeOS les **sépare** parce qu'ils se facturent
très différemment :

| Compteur | `usage` (stream-json) | Ce que c'est | Coût relatif* |
|---|---|---|---|
| **input** | `input_tokens` | entrée **fraîche** (non cachée) lue ce tour | **1,0×** (référence) |
| **output** | `output_tokens` | tokens **générés** ce tour | le plus cher/token (dépend du modèle) |
| **cache write** | `cache_creation_input_tokens` | entrée **écrite** dans le cache de prompt (TTL 5 min) | **1,25×** un token d'entrée |
| **cache read** | `cache_read_input_tokens` | entrée **servie depuis** le cache | **0,10×** un token d'entrée |

\* Multiplicateurs **structurels** documentés par Anthropic, **indépendants du
modèle** pour le côté entrée : une lecture cache coûte ~10 % d'un token d'entrée
frais, une écriture cache ~125 %. Le ratio output/input, lui, **dépend du
modèle** (≈ 5× sur Opus) — c'est pourquoi on ne le code pas en dur : le coût USD
autoritaire vient du CLI.

---

## 2. Le levier : le cache de prompt

La plus grosse marge de manœuvre sur « combien on consomme » n'est pas de
générer moins, c'est de **ne pas re-payer l'entrée**.

Un contexte de 20 000 tokens (fichiers système, `CLAUDE.md`, mémoire chargée)
renvoyé à chaque tour :

- **sans cache** : 20 000 tokens d'entrée **fraîche** à chaque tour → sur 30
  tours, 600 000 tokens d'entrée facturés plein tarif ;
- **avec cache** : 20 000 en **écriture** au 1ᵉʳ tour (1,25× = 25 000
  équivalents), puis 20 000 en **lecture** aux 29 suivants (0,10× = 2 000
  équivalents chacun) → 25 000 + 58 000 = **83 000 équivalents-token** au lieu
  de 600 000. **≈ 86 % d'économie sur l'entrée.**

VibeOS rend ce levier **visible** : le superviseur journalise le
`cache_hit_ratio` = `cache_read / (input + cache_read)` — la fraction de
l'entrée servie par le cache. **Un ratio élevé = le contexte coûteux est relu
depuis le cache (0,10×) au lieu d'être re-facturé en entrée fraîche (1,0×).**
C'est la première métrique à regarder pour piloter la consommation.

**Bonnes pratiques cache** (à la portée d'un agent gouverné VibeOS) :

- garder le **préambule stable** (système, `CLAUDE.md`, mémoire de session) en
  tête de prompt pour qu'il reste caché ;
- éviter de **muter** le début du contexte (une seule modification en tête
  invalide tout le cache en aval) ;
- regrouper les tours dans la **fenêtre de TTL** (5 min) plutôt que d'espacer.

---

## 3. Les modes de consommation

Un **mode** mappe une **intention humaine** (« sois économe ») à un `Budget`
concret (temps + appels + tokens) et à une guidance cache/contexte. Les
préréglages sont des **défauts sûrs** — `--budget`/`--calls`/`--tokens`
explicites **surchargent** l'axe correspondant.

| Mode | Temps mural | Appels d'outils | Tokens (total) | Pour quoi |
|---|---|---|---|---|
| **`frugale`** | 30 min | 60 | 300 000 | travail routinier/répétitif où une erreur est peu coûteuse à refaire ; contexte maigre, tout en cache |
| **`équilibrée`** | 4 h | 400 | 3 000 000 | l'équilibre par défaut portée/coût |
| **`performance`** | 8 h | 1 200 | 12 000 000 | raisonnement dur en un coup, où re-lancer coûterait plus cher que les tokens |

> Ces nombres sont **ronds et défensifs** — un plancher de sûreté pour un run non
> surveillé — pas des SLA optimisés. Ils vivent dans `ConsumptionMode::budget()`
> ([supervisor.rs](../vibed/src/supervisor.rs)), un seul endroit à régler.

Usage :

```console
# Un run frugal borné (temps + appels + tokens du préréglage) :
vibectl agent run --mode frugale -- claude -p --output-format stream-json ...

# Frugal en coût/appels MAIS avec une longue horloge (surcharge d'un axe) :
vibectl agent run --mode frugale --budget 8h -- claude -p ...

# Sans mode : borne explicite au token près.
vibectl agent run --tokens 500000 -- claude -p ...
```

Un `--tokens N` (ou le token cap d'un mode) tue le run avec la raison
`token_budget` dès que le **total** consommé atteint `N`. Sans aucune borne
(`--budget`/`--calls`/`--tokens`/`--mode`), le run est **illimité** — le
superviseur l'avertit sur stderr.

---

## 4. Ce que le superviseur journalise

À la fin de chaque session autonome, `agent run` écrit un événement
`autonomous_session`/`end` (type réservé, infalsifiable par un agent) dont le
champ `tokens` porte le décompte complet :

```json
{
  "input": 42000, "output": 8600,
  "cache_creation": 20000, "cache_read": 540000,
  "total": 610600, "turns": 30,
  "cache_hit_ratio": 0.928,
  "input_equiv_tokens": 121000.0,
  "cache_savings_tokens": 486000.0,
  "cost_usd": 1.87
}
```

- `total` : ce que borne `--tokens` (input + output + write + read).
- `cache_hit_ratio` : le levier (§2) — ici 93 % de l'entrée vient du cache.
- `input_equiv_tokens` : dépense d'entrée en équivalents-token frais
  (`input + 0,10·read + 1,25·write`), relatif **stable** sans devise.
- `cache_savings_tokens` : `0,90·read` — équivalents-token que le cache a
  économisés (ici 486 000, soit ~8× la génération).
- `cost_usd` : coût **autoritaire** du CLI (`total_cost_usd`), ou `null` si le
  CLI ne l'a pas rapporté — jamais un prix inventé côté VibeOS.

Ces événements s'accumulent dans le journal mémoire : « combien ai-je consommé
cette semaine ? » devient une question à laquelle la machine peut répondre sur
ses propres données, sans que rien ne sorte de la machine.

---

## 5. Méthodes de réduction (au-delà du cache)

Par ordre d'impact typique :

1. **Cacher le contexte stable** (§2) — le plus gros levier, presque gratuit.
2. **Classer la mémoire au lieu de tout recharger** — un rappel pertinent
   (récence + importance + pertinence, [ADR-030](DECISIONS.md)/`recall.rs`)
   remonte les 5 souvenirs utiles plutôt que 50 lignes de journal : moins
   d'entrée à chaque session. C'est le pont entre ce document et la mémoire.
3. **Choisir le palier de modèle** selon la tâche (Haiku pour le mécanique,
   Opus pour le raisonnement dur) — le modèle se choisit dans la commande CLI,
   le mode borne la dépense quel qu'il soit.
4. **Contexte maigre** : n'injecter que ce qui sert (le mode `frugale` en fait
   une consigne), éviter d'écho de gros résultats d'outils dans le prompt.
5. **Borner par défaut** : un `--mode` sur tout run non surveillé transforme une
   boucle qui dérape d'un coût illimité en un coût plafonné.

---

## 6. Honnêteté & limites

- **Le comptage n'est pas une frontière de sécurité.** Le schéma `stream-json`
  n'est **pas contractuel** ([ADR-012](DECISIONS.md)) : un CLI qui sous-rapporte
  l'`usage` est sous-compté, donc `--tokens` peut ne pas se déclencher. C'est un
  contrôle de **coût/verbosité**. L'enveloppe de sécurité reste le **temps
  mural** (horloge monotone, plafond dur), l'audit, le rate-limit par uid et
  l'approbation — indépendants de ce que le flux rapporte.
- **Pas de prix en dur.** VibeOS n'embarque aucune table USD/token : elle
  périmerait. Le coût vient du CLI ; le relatif vient des multiplicateurs
  structurels du cache.
- **Le classement mémoire (§5.2) est livré ET câblé** dans `memory.query` (mode
  `rank: true`, opt-in, scopes `journal`/`knowledge` — voir [MEMORY.md §9](MEMORY.md)
  et ADR-030) ; seul le moteur **vectoriel** (production des vecteurs, ollama
  local) reste un incrément suivant.

---

## 7. Références

- Décision : [ADR-030](DECISIONS.md)
- Sous-système mémoire (rappel, embeddings) : [MEMORY.md](MEMORY.md)
- Superviseur (ADR-012/013) : [`vibed/src/supervisor.rs`](../vibed/src/supervisor.rs)
- Amont : [Anthropic — prompt caching](https://docs.anthropic.com/en/docs/build-with-claude/prompt-caching)
