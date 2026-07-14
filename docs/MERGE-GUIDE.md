# MERGE-GUIDE — 3 PR à merger dans `main`

> **But** : rapatrier dans `main` trois correctifs qui y sont absents. Ouvre ce
> fichier, merge les trois PR ci-dessous, c'est tout. Dernière vérif : 2026-07-14.

## Ce qu'il reste à merger (base = `main`, indépendantes)

| PR | Branche | Contenu | Pourquoi sûr | Ce qui casse sans elle |
|----|---------|---------|--------------|------------------------|
| 🔒 **[#20](https://github.com/Micka420-collab/vibeos/pull/20)** | `security-home-alias-fix` | Fix : sur bootc `/home` est un lien vers `/var/home` ; une règle opérateur `paths.denied` en `/home/…` était contournable via `/var/home/…`. `apply_rule` replie l'alias (`path_glob_match`). | Ne **resserre** que les règles deny (jamais desserre). Test de régression inclus. CI verte. | **La vuln reste vivante sur `main`** : un deny opérateur dans le home est esquivable en changeant d'orthographe. |
| **[#19](https://github.com/Micka420-collab/vibeos/pull/19)** | `f6-fs-extraction` | Refactor F6 : `fs.*` sorti de `mcp.rs` vers `tools/fs.rs` + `test_support.rs`. Déplacement pur, zéro comportement changé. | Relire en `git diff --color-moved`. 149 tests inchangés, verts. | Rien fonctionnellement ; `mcp.rs` reste un gros fichier. Cosmétique, non urgent. |
| 🔒 **[#21](https://github.com/Micka420-collab/vibeos/pull/21)** | `denylist-roothome-alias` | Durcissement : la denylist intégrée ignorait l'alias `/root`→`/var/roothome` (bootc). Ajoute l'entrée canonique en miroir de `/root/**`. | Défense-en-profondeur : ne fait qu'**ajouter** une interdiction, aucun accès légitime cassé. Test de régression, CI verte. | Denylist incohérente avec le matcher de politique (rattrapé par le confinement — non exploitable, mais robustesse moindre). |

**Ordre : quelconque.** Elles touchent des zones **disjointes** (#20 `policy.rs`
+ docs ; #19 déplace `fs.*` dans `mcp.rs` ; #21 ajoute une ligne à la denylist de
`mcp.rs`) — #19 et #21 touchent `mcp.rs` mais des hunks séparés (git auto-merge
vérifié propre). Aucune dépendance. **Priorité au fix sécurité #20.**

> ✅ **Vérifié localement** (2026-07-14) : `main` + #19 + #20 + #21 mergés
> **ensemble** = **0 conflit**, `cargo test` **151 tests verts** (140 unit + 8 mcp
> + 3 policy), clippy `-D warnings` + fmt propres. Les trois coexistent sans souci.

## Pourquoi elles sont hors de `main` (post-mortem, à ne pas reproduire)

Ces correctifs vivaient dans une **pile de PR empilées** :
`main ← #11 ← #12 (F6) ← #13 (fix)`. Le merge s'est fait **dans le désordre** :

- **#11** a été mergé dans `main` alors que sa branche était encore au point de
  gel `9cee9c6` (avant F6 et le fix). ✅ #11 est bien sur `main`.
- **#12** et **#13** ont ensuite été mergées **dans leurs bases intermédiaires**
  (`#12`→`worktree-amelioration-2026-07-13`, `#13`→`f6-fs-refactor`), pas dans
  `main`. GitHub les marque *merged*, mais leur contenu **s'est arrêté dans ces
  branches** et n'a jamais atteint `main`.

**Leçon** : avec des PR empilées, une fois la base (#11) mergée dans `main`, il
faut **recibler les PR suivantes sur `main`** AVANT de les merger — sinon on merge
dans la branche du dessous. Ici, on a préféré **repartir de zéro avec deux PR
indépendantes basées sur `main`** : elles ne peuvent plus s'échouer (merger dans
`main` = contenu dans `main`, point).

## Garde-fous

- **Merge chaque PR dans `main`** (leur base *est* `main`) — ne les merge pas
  l'une dans l'autre.
- L'agent **ne merge ni ne ferme rien** lui-même : ces deux merges sont une action
  humaine explicite.
- Les anciennes branches `worktree-amelioration-2026-07-13`, `f6-fs-refactor`,
  `fs-home-alias-deny` sont **obsolètes** (contenu rapatrié ailleurs) — les
  supprimer après coup est sans risque.
