# MERGE-GUIDE — ✅ résolu (tout mergé dans `main`)

> **Plus rien à merger.** Les correctifs qui avaient été échoués hors de `main`
> par un merge de pile mal orienté (ex-PR #12/#13) ont été rapatriés en PR
> **indépendantes base=`main`** et **mergés le 2026-07-14** :
>
> - **#19** `f6-fs-extraction` → merge `c001166` (F6 : `fs.*` hors de `mcp.rs`)
> - **#20** `security-home-alias-fix` → merge `ce0fb00` (fix alias politique `/home`↔`/var/home`)
> - **#21** `denylist-roothome-alias` → merge `ab70856` (durcissement denylist `/root`↔`/var/roothome`)
>
> Les 3 branches mortes (`worktree-amelioration-2026-07-13`, `f6-fs-refactor`,
> `fs-home-alias-deny`) ont été supprimées. `main` est à jour.

## Leçon (pour ne pas reproduire)

Avec des **PR empilées** (`main ← #11 ← #12 ← #13`), une fois la base (#11) mergée
dans `main`, il faut **recibler les PR suivantes sur `main` avant de les merger** —
sinon elles se mergent dans la branche du dessous et leur contenu **n'atteint jamais
`main`** (GitHub les marque pourtant *merged*). C'est exactement ce qui s'était
produit. La parade retenue : **des PR indépendantes basées sur `main`**, jamais
empilées.
