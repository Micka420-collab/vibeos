# MERGE-GUIDE — pile de PR post-gel (#11 → #12 → #13)

> **But** : merger les trois PR en ~10 min sans relire les diffs en détail.
> Ouvre ce fichier, suis l'ordre, c'est tout. Dernière vérif : 2026-07-14.

## La pile (chaque PR est basée sur la précédente)

```
main
 └─ PR #11  worktree-amelioration-2026-07-13   (base: main)
     └─ PR #12  f6-fs-refactor                 (base: worktree-amelioration-2026-07-13)
         └─ PR #13  fs-home-alias-deny         (base: f6-fs-refactor)
```

État au 2026-07-14 : les **trois** sont `MERGEABLE` / `mergeStateStatus: CLEAN`.

## Ordre de merge — impératif : #11, puis #12, puis #13

| Ordre | PR | Contenu (2-3 phrases) | Pourquoi sûr à merger | Ce qui casse si sauté / désordonné |
|------|----|----------------------|----------------------|-----------------------------------|
| 1 | **#11** `worktree-amelioration-2026-07-13` → `main` | Le gros lot déjà revu : Phase 2.5 (superviseur + capture raisonnement), initiative Zed gouvernée, durcissements vibed. 95 commits, gelé à `9cee9c6`. | CI verte (7 pass dont build image), merge local dans `main` simulé sans conflit (`Cargo.toml` en `toml=0.8`). Gelé : aucun commit ajouté depuis vérif. | #12 et #13 sont basées dessus : les merger avant #11 les ferait entrer **dans la branche gelée** et la dé-gèleraient. Toujours #11 en premier. |
| 2 | **#12** `f6-fs-refactor` | F6 : extraction de la famille `fs.*` de `mcp.rs` vers `tools/fs.rs` + `test_support.rs`. **Déplacement pur, zéro changement de comportement** ; `mcp.rs` 2872 → 1754 lignes. | Relire en `git diff --color-moved=zebra` : les blocs sont identiques, logique nouvelle nette ≈ 40 lignes (docs + imports). 149 tests inchangés, verts ; clippy/fmt propres. | Rien ne « casse » fonctionnellement, mais **#13 modifie `policy.rs` et suppose la base #12** ; merger #13 avant #12 sort du fil de dépendance. Garder l'ordre. |
| 3 | **#13** `fs-home-alias-deny` | Fix **LOW** trouvé en revue adversariale : sur bootc `/home`→`/var/home`, une règle opérateur `paths.denied` en `/home/…` était contournable via `/var/home/…`. `apply_rule` replie l'alias (`path_glob_match`). | Ne **resserre** que les règles deny (jamais desserre). Denylist des secrets déjà agnostique → aucun secret n'était exposé. Test de régression ajouté, 150 tests verts. | C'est la feuille de la pile : rien ne dépend d'elle. Peut être mergée en dernier sans effet de bord. |

## Mécanique GitHub (retargeting)

Après avoir mergé **#11** dans `main` **et supprimé** sa branche
`worktree-amelioration-2026-07-13`, GitHub **recible automatiquement** la base
de #12 sur `main`. Le diff de #12 se réduit alors à son seul travail (F6).
Merge #12, supprime `f6-fs-refactor` → #13 se recible sur `main` à son tour.
Merge #13.

Si tu ne supprimes pas les branches, recible manuellement la base de #12 puis
#13 sur `main` (bouton *Edit* → *base* de la PR) avant de merger chacune —
sinon tu mergerais dans la branche du dessous.

## Garde-fous

- **Ne jamais** merger #12/#13 tant que #11 n'est pas dans `main` : ça pollue la
  branche gelée et le diff des PR suivantes.
- Aucune de ces PR ne touche le plancher T2/T3 ni n'ouvre de capacité
  d'exécution : ce sont un lot déjà revu (#11), un refactor mécanique (#12) et un
  durcissement de politique qui ne fait que resserrer (#13).
- Après le merge des trois : marquer **F6 fs terminé** dans `ROADMAP.md`
  (encore listé comme « différé, session dédiée » côté `main`).

## PR indépendante (hors pile)

- **[PR #14](https://github.com/Micka420-collab/vibeos/pull/14)** `vibeos-validation-harness` → **base `main`** : harnais de validation E2E/boot (`vibeos-selfcheck.sh` + `docs/VALIDATION.md`). **Aucune dépendance** avec la pile ci-dessus — merge quand tu veux, dans n'importe quel ordre. Volontairement hors pile pour ne pas allonger la chaîne de merge.
