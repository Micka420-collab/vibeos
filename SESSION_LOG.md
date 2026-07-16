# SESSION_LOG — sessions autonomes (2026-07-13 → )

> Journal des sessions de travail autonomes, **anti-chronologique** (le plus
> récent en tête). Point d'entrée permanent : [STATUS.md](STATUS.md).
> *(Historique : la [PR #11](https://github.com/Micka420-collab/vibeos/pull/11) qui
> portait les sessions des 13-14 juillet a été **mergée** le 2026-07-14, et la
> [PR #4](https://github.com/Micka420-collab/vibeos/pull/4) qu'elle supersedait est
> **fermée**. Le travail passe désormais par des PR petites et indépendantes sur
> `main`.)*

## 💿 Session 2026-07-16 (matin, autonome) — la première ISO F44, et ce que le dépôt affirmait sans preuve

**La première ISO Fedora 44 du projet existe**, construite en local et **vérifiée avant
d'être annoncée** : `ISO 9660 … 'Fedora-S-dvd-x86_64-44' (bootable)`, `EFI/` présent,
image conteneur de 6,9 Go embarquée, `image.title = VibeOS`, `base.name =
fedora-kinoite:44@sha256:2d24c434…`. 7,9 Go, sha256 à côté. Aucun compte pré-cuit —
Anaconda demande le compte à l'installation, comme une ISO de release.

### `main` s'est cassé tout seul, sans que personne ne touche au dépôt

À 01:00 UTC, Fedora a republié `fedora-kinoite:44` et **quay a purgé l'index qu'on
épinglait**. Tous les builds sont morts sur `manifest unknown`, quelques heures après
un build vert avec ce même digest.

**Le pin tenait parce que la base était morte.** F42 était EOL — jamais reconstruite —
donc digest stable pour toujours. Sortir de l'EOL vers une base **vivante** (rebâtie
quasi quotidiennement) rend le pin fragile. C'est un prix que le rebase de la veille a
introduit **sans que je l'anticipe**. Deux exigences se contredisent : la posture
supply-chain veut un digest, l'amont purge ses vieux index. On ne repasse **pas** sur
le tag flottant. Arbitrage écrit dans le fichier, laissé à Micka : bump manuel /
Renovate / miroir ghcr.

### Trois affirmations que rien n'assurait

- **Le badge des 4 README annonçait « Fedora Kinoite 42 »** — une base morte, en haut
  de la page d'accueil, dans 4 langues. Un badge est une **image** : aucun test ne le
  lit, et il pointe vers `os/Containerfile` — un lien qui *donne l'air* d'être vérifié.
  Corrigé + **check A2** dans `verify-roadmap-truth.sh`.
- **Toute image poussée sous le tag `:0.2.1-dev` annonçait
  `org.opencontainers.image.version="0.1.0-dev"`.** Le Containerfile le codait en dur,
  le `version` du workflow ne servait qu'à *nommer* le tag, et `buildah-build`
  n'injecte aucun label (`labels` = input optionnel sans défaut, lu dans son
  `action.yml` au SHA épinglé). **Constaté ensuite dans l'ISO livrée**, pas seulement
  déduit. Défaut choisi : `0.0.0-dev` et non `0.1.0-dev` — mieux vaut une version
  *manifestement* fausse qu'un numéro *plausible* mais périmé ; c'est précisément
  pour ça que personne ne l'avait vu.
- **La trousse cybersécurité annonçait 3 outils qu'elle n'installe plus** (ma dérive :
  le rebase les a retirés du Containerfile, pas des manifestes). Portée honnête :
  `sectools.list` ne ment pas — il fait un `stat` réel et les rapporterait absents.

### Le motif commun : « garder en synchro » n'est pas une synchro

Trois fois le même défaut, à trois endroits : un **commentaire** demandait poliment une
synchronisation manuelle, et elle a dérivé **la première fois que quelqu'un a touché
la liste**. `os/Containerfile` disait *« Keep os/security-tools.txt in strict sync with
this list »*. Le compteur de tests des README a dérivé **3 fois sur 3** (149→175,
#59, 175→191) — dont une fois où quelqu'un avait déjà *réparé le garde-fou* et l'avait
laissé en WARNING.

Chaque correctif est donc **mécanique**, jamais seulement la valeur — et
**mutation-testé**, y compris le cas « l'ancre est cassée » qui doit échouer **fermé**.

### Ce que j'ai raté, et qui est plus instructif que ce que j'ai trouvé

**`verify-roadmap-truth.sh` signalait la dérive du compteur depuis le début, à chaque
run, en WARNING.** Je ne l'ai jamais vue : mes commandes filtraient sur
`grep -E "^\[(OK|FAIL)\]"` et **jetaient les lignes WARN**. J'ai écrit « roadmap-truth
✅ » dans une dizaine de PR sans lire ce que le script me disait. Son propre en-tête
m'avait pourtant prévenu : *« Un garde-fou qui avertit en permanence finit ignoré —
c'est pire qu'aucun garde-fou. »* Il décrivait exactement ce que j'allais faire.

**Et mon correctif `ci.yml` de la nuit n'a jamais atterri** (`c74ca80`) : poussé après
le merge de #65, comme le fix akmods — sauf que celui-là, je ne l'avais pas récupéré.
Le garde-fou EOL est donc resté **aveugle sur un changement de base** pendant des
heures. Troisième fois le même motif : *du travail vérifié resté en arrière du merge*.

**Cinq fois un outil m'a menti**, et à chaque fois c'est un contre-test qui a sauvé le
résultat : une sonde déclarant `git` absent de f44 (des CR Windows en fin de ligne) ;
un `--build-arg` qui « ne marchait pas » (le cache podman) ; « Landlock absent » (le
`securityfs` n'était pas monté — il est en réalité **ABI v7**) ; un décompte annonçant
13 outils manquants (les listes comparaient des *paquets* à des *binaires*) ; un
commentaire GitHub parti mutilé (bash interprétant mes backticks).

### Résultats négatifs de valeur — ne pas re-auditer

- **Couverture outils/politique : intacte.** Les 15 outils du registre (`tool_catalog`)
  sont tous couverts par `security/policy.d/default.toml`, et aucune règle ne vise un
  outil inexistant. Les deux ensembles sont **identiques**.
- **Tiers : alignés.** Chaque outil déclare le même tier dans le registre et dans la
  politique (`fs.write` T1=T1, `svc.restart` T2=T2, `pkg.install` T2=T2…), plus un
  catch-all `*` = T3 `deny`. Aucune règle ne se lit plus permissive qu'elle n'agit.
- **Migration Node 20→24 de GitHub Actions : non concernés.** Le 16 juin est passé,
  les runners tournent déjà en Node24, et nos 3 actions `redhat-actions/*` (qui
  déclarent `node20`) s'exécutent sans erreur depuis — prouvé par nos propres logs du
  15/07. `ubuntu-24.04-arm` est de l'**arm64**, pas de l'ARM32.

### Le navigateur reste interdit d'exécution

Vérifié : `Landlock ABI v7` + `seccomp` + `systemd-run` sont **disponibles** — le bac à
sable Phase 3 est donc développable. C'est lui qui bloque `browser.*`, et rien ne sera
exécuté avant lui : un moteur de rendu qui parse du HTML hostile ne peut pas vivre
**in-process dans le moteur de politiques**.

---

## 🔴 Session 2026-07-15 (nuit, autonome) — l'OS tournait sur une base morte

**Fedora Kinoite 42 est EOL depuis le 2026-05-27.** L'image livrée — y compris l'ISO
`v0.2.1-dev` — n'avait reçu **aucun correctif de sécurité depuis 49 jours**.

**Ce qui rend ça grave, ce n'est pas la version : c'est le silence.** Le build ne
*peut pas* échouer tout seul. Quand une release Fedora passe EOL, MirrorManager
continue de répondre au metalink et redirige vers les miroirs d'**archive** ;
`dnf install` renvoie **0** et installe des paquets gelés à la date d'EOL. Vérifié :

    f42 repomd.xml Last-Modified: Wed, 27 May 2026 16:25:54 GMT   (gelé, archive)
    f44 repomd.xml Last-Modified: Wed, 15 Jul 2026 01:00:29 GMT   (vivant)

CI verte, ISO qui boote, image qui se fossilise. Personne ne pouvait le voir.

**Micka a contesté la trouvaille** — à raison : une date d'EOL est exactement ce qu'un
modèle hallucine. Sa règle (« EOL ≈ 4 semaines après N+2 ») était la bonne, appliquée
avec une release en trop : F42 (avril 2025) → F43 (**oct. 2025**) → F44 (**28 avril
2026**) → EOL 42 = **27 mai 2026**. Sa propre règle produisait la date exacte.
`endoflife.date` a tranché en 20 secondes. **La bonne direction de confiance** : il a
demandé une vérification, pas une soumission.

**Le vrai correctif n'est pas le numéro de version** — c'est `scripts/check-base-eol.sh`,
qui fait rougir le build si la base est EOL, ou à moins de 30 jours de l'être. Table
d'EOL **épinglée en dépôt**, pas de fetch live, délibérément : `docs.fedoraproject.org`
est derrière **Anubis** ; un fetch CI renverrait une *page* d'accès refusé qu'un parseur
naïf lit « pas d'EOL trouvé » → garde-fou qui échoue **OUVERT**, pire que pas de
garde-fou. **Vérifié en l'exécutant contre le vrai Containerfile de `main`** : il
attrape le bug qui l'a motivé (« EOL since 2026-05-27 (49 days ago) »), et échoue
fermé sur une release inconnue.

**Et j'ai trouvé un défaut dans mon propre garde-fou** : `ci.yml` filtrait sur
`os/rootfs/**` mais pas `os/Containerfile`. Une PR ne touchant que le Containerfile —
donc *précisément* un changement de base — ne déclenchait pas la CI. Le garde-fou n'a
tourné que **par accident**, ma PR touchant aussi `scripts/`. Un garde-fou qui ne
s'exécute pas sur le fichier qu'il garde ne garde rien.

**Le rebase n'était pas un changement de numéro.** Trouvé en interrogeant les vrais
dépôts f44 (`dnf repoquery` en conteneur), pas en attendant 12 min de CI par
découverte : `nbtscan`/`wfuzz`/`scalpel` retirés d'amont ; `nodejs`/`npm` — f44 n'a
plus de métapaquet, que des streams (pris **24** et non 22 : node 22 meurt avril 2027,
la base f44 juin 2027 — shipper un runtime qui meurt avant sa distro, ce serait refaire
le bug en petit) ; `curl-minimal` disparu.

**La sonde a failli me faire livrer 105 faux positifs.** Elle déclarait `git`, `tar`,
`python3` absents de f44. Le fichier était écrit sous Windows, chaque ligne finissait
par un CR, et `grep -qx` ne matchait jamais. Ce qui a sauvé le résultat : un **contrôle
de cohérence dans la sonde elle-même** (« ces paquets DOIVENT résoudre, sinon la sonde
est cassée »). Il disait `git` **ok** pendant que la boucle disait `git` **MISSING** —
même requête, deux réponses → c'est la donnée, pas la requête.

**Le bug akmods : amont, et il échouait déjà sur f42.** `kmodtool` bake dans le `%post`
de tout akmod un appel à `akmods-ostree-post`, sans `|| :`. Ce helper appelle
`akmodsbuild` **directement en root** là où `akmods` passe par `runuser`. Il documente
son hypothèse : *« pretty safe because its happening in the ostree %post sandbox »* —
vrai sous `rpm-ostree compose`, **faux sous buildah**. RHBZ #2459819 (*closed
insufficient_data*), Bazzite a le même. **Ce n'est pas une régression f44** :
`akmodsbuild` est byte-pour-byte identique f42→f44, et le même « Not to be used as
root » est dans les logs des builds f42 **qui ont livré des ISO**. Ce qui a changé,
c'est **dnf5** : f42 marquait « Non-critical error » et poursuivait, f44 fait échouer
la transaction.

### Ce que j'ai cassé, et ce que ça enseigne

**J'ai ouvert la PR du rebase en PR normale alors que son build était rouge.** Elle a
été mergée telle quelle, et le commit qui la rendait verte est resté derrière. `main`
a été cassé. Puis **la même erreur une seconde fois** : le correctif akmods, commité
et vérifié, n'a jamais été poussé — perdu dans un changement de contexte.

Deux fois le même motif : **du travail vérifié qui reste en arrière du merge**. La
règle qui en sort, sans exception : **build rouge → draft**. Et un commit vérifié qui
n'est pas poussé n'existe pas.

**Trois fois j'ai annoncé « vert » à tort** avant vérification : un `grep` avalant
`cargo: command not found` (le build n'avait jamais tourné) ; un `| tail` masquant
l'échec de clippy ; le premier échec de build attribué à f44 alors que c'était un
flake CDN quay.io — 809 lignes de compilation Quickshell étaient passées avant, donc
**Quickshell 0.2.1 compile sur f44**, le gros risque du rebase est levé.
`${PIPESTATUS[0]}` désormais.

### Le navigateur : décidé, mais interdit d'exécution

**ADR-018** tranche ce qu'ADR-017 laissait ouvert : `chromium` + `chromedriver` des
dépôts Fedora, pilotés en **W3C WebDriver (HTTP/JSON)**. Vérifié par `repoquery` sur
les vrais dépôts f44 : les deux arches, même version, `chromedriver` **sous-paquet de
la source `chromium`** (le décalage navigateur/driver, mode de panne classique, ne peut
structurellement pas arriver), codecs **libres** (`libavcodec-free`) — le blocage exact
qui a fait rejeter BrowserOS n'existe pas ici. Décisif : **`vibed` n'a pas de client
WebSocket et CDP en exige un** ; ajouter une pile WebSocket à la TCB pour parler à un
composant qu'on traite comme hostile serait le mauvais échange. Playwright écarté
(Node, binaires d'un CDN dans un cache mutable, Fedora non supportée, pas de Chrome
arm64). Piège nommé : le paquet `chromium-headless` ne contient **pas** le headless
moderne (`repoquery -l` → `headless_shell`, l'ancien).

**Mais rien ne sera exécuté avant la Phase 3.** Un navigateur est la menace M2
incarnée, et les outils de `vibed` tournent **in-process, sans isolation**. Livrer
`browser.*` avant le bac à sable mettrait un moteur de rendu qui parse du HTML hostile
**dans le processus qui EST le moteur de politiques** : une RCE dans le parseur ne
contournerait pas la gouvernance, elle **deviendrait** la gouvernance. Écrit dans
l'ADR. État vérifié : la seule occurrence de `browser.` dans `vibed` est
`url_bearing()`, un garde-fou de politique.

### La contradiction sur l'invariant n°1

`ARCHITECTURE.md` §8 : *« **Aucun agent IA ne contourne `vibed`** : le socket MCP est
l'unique surface de contrôle système exposée aux agents. »*
`DECISIONS.md` : *« décision **Zed-only**, **le terminal garde ses outils** »*.

Les deux ne peuvent pas être vrais. Claude Code en terminal a des `Read`/`Write`/
`Bash` natifs que `vibed` ne voit pas : `vibeos:fs.read ~/.ssh/id_rsa` est **refusé**
par la denylist, `Bash("cat ~/.ssh/id_rsa")` **passe**. L'invariant est écrit trop
large — vrai pour les capacités **privilégiées** (l'agent n'est pas root, `/usr` est
RO), faux comme absolu. Signalé, **non corrigé** : l'arbitrage `permissions.deny`
s'applique via des settings **partagés**, donc au terminal aussi — décision produit qui
revient à Micka.

---

## 🌐 Session 2026-07-15 (soir, autonome) — l'ISO passe, et la première brique du navigateur

**Les deux ISO sont construites.** `Build install ISO (amd64)` et `(arm64)` :
`success`, ~7,4 Go et ~6,7 Go, artefacts du run
[29434851047](https://github.com/Micka420-collab/vibeos/actions/runs/29434851047)
(tag `v0.2.1-dev`, `main` @ `d9a04ab`), expirent le **2026-07-29**. Le correctif des
dépôts vendor (`enabled=0` + `--enablerepo=`) tient : les jobs qui mouraient en ~25 s
sur `cannot depsolve` vont désormais au bout. Pas de release publiée — un tag `-dev`
n'en déclenche pas.

**Réserve honnête sur ce run** : `SBOM + CVE scan (advisory)` est en échec, mais
**dans l'outil** (`syft scan` s'interrompt en cours d'exécution), pas sur une
trouvaille. Ce n'est donc pas un signal de sécurité — mais ça veut dire qu'on n'a
**aucune donnée CVE** pour ce build. Le job est advisory par construction, il ne
bloque pas l'ISO. *À instruire.*

**Première brique d'ADR-017** — [PR #63](https://github.com/Micka420-collab/vibeos/pull/63),
la plomberie de politique **avant** tout outil navigateur (⚠️ touche `policy.rs` →
revue humaine explicite).

La décision de conception : **`[rule.domains] only` est un *prédicat de règle*, pas
un verdict** — volontairement à l'inverse de `[rule.services] allowed`, qui *refuse*
une unité hors liste sur place (ADR-011). Un domaine hors liste ne refuse rien : il
rend la règle inapplicable et l'évaluation continue. La décision n°1 de Micka
(« domaines de confiance libres, tout autre déclenche une approbation T2 ») tombe
alors du `first-match-wins` existant, **sans nouveau concept dans le moteur**. La clé
s'appelle `only` et pas `allowed` pour que les deux sémantiques ne se lisent jamais
pareil.

Deux pièges désamorcés, parce qu'**une allowlist rate en silence** :
- `"evil-github.com".ends_with("github.com")` vaut **`true`**. Cette ligne, c'est
  tout le contournement. Le joker s'ancre donc sur une **frontière de label** — le
  point fait partie du suffixe, jamais optionnel. D'où un `domain.rs` dédié :
  `glob_match` découpe sur `/`, c'est un matcher de *chemins*.
- `deny_unknown_fields` : écrire `allowed =` sous `[rule.domains]` (réflexe venu de
  `[rule.paths]`) laisserait `only = None` → règle scopée à rien = règle s'appliquant
  à **tous** les hôtes en T1. Une allowlist silencieusement désactivée est le pire
  résultat possible → erreur de **chargement**.

`host_of` est strict et fail-closed : userinfo refusé (`https://docs.rs@evil.tld/` —
l'hôte réel est `evil.tld`), non-ASCII refusé (un IDN doit arriver punycodé, aucun
homographe ne se replie sur un motif ASCII), schémas non-http, IPv6, point final,
labels vides. **`None` ne veut jamais dire « autorisé »** : une URL illisible tombe
vers l'humain au lieu d'hériter du T1.

`derive_domain` est la **jumelle de `derive_service`** : la seule dérivation
qu'appellent à la fois `handle_tools_call` et `policy_check`. Ce drift a déjà été
livré une fois pour les unités — il ne peut plus l'être ici par construction.

**La discipline de la journée, tenue** : les 5 garde-fous ont été *mutation-testés*
(bug remis, échec du test constaté). Le plus instructif est le 4ᵉ — « allowlist qui
ne matche rien » : sans lui, une allowlist n'accordant à **personne** passerait tous
les tests « hors-liste escalade » au vert. C'est exactement le piège du test qui
n'asserte que la direction facile, revu une quatrième fois.

**Deux fois où je me suis rattrapé sur du faux vert**, dans cette même session : un
`grep` a avalé un `cargo: command not found` (le build n'avait jamais tourné), et un
`| tail` a masqué le code de sortie de `clippy` (qui échouait). Les deux fois,
l'annonce « vert » était fausse avant vérification. `${PIPESTATUS[0]}` désormais.

---

## 🔧 Session 2026-07-15 (jour, autonome) — le fil rouge : ce qui se dit fait sans l'être

Onze PR, toutes indépendantes sur `main`. **Aucune inventée** : chacune corrige un
écart entre ce que le dépôt *affirme* et ce qu'il *fait*.

**La leçon de la journée, apprise deux fois à mes dépens : j'ai écrit deux tests
qui ne testaient rien.** Le premier était **tautologique** (il passait déjà sans
le correctif) ; le second n'assertait que **la direction facile** d'une propriété
— ses 4 contrôles vérifiaient qu'une clé *change*, jamais qu'elle *reste stable*,
qui était sa seule raison d'être ; c'est pour ça qu'un vrai bug est passé au vert.
Un contrôle qui n'asserte que la direction facile est **pire que pas de contrôle :
il achète de la fausse confiance**. Discipline désormais systématique : **rétablir
le bug et vérifier que le test échoue** — appliqué à chaque correctif ci-dessous.

**Un vrai bug de sécurité, trouvé en faisant relire ma propre PR.**
[PR #49](https://github.com/Micka420-collab/vibeos/pull/49) : `target` n'est pas un
champ de log — c'est **la clé du grant d'approbation** (`check_and_consume_grant`
la compare à l'identique) **et la seule description de l'action que l'opérateur
voit** (la demande en attente ne porte jamais les arguments). Or il était dérivé du
« premier non-nul parmi (chemin, unité, paquet) » pour *tous* les outils, alors que
`path`/`unit` sont lus sur les arguments de *n'importe quel* appel. Donc :
`svc.restart {"unit":"nginx.service","path":"/etc/nginx/nginx.conf"}` → le chemin
gagne → l'opérateur approuve un « redémarrage » d'un **fichier de config**
d'apparence plausible → le grant est keyé sur ce chemin → le **même grant**
autorise ensuite le redémarrage de **n'importe quelle autre unité non-denylistée**
(l'unité n'est jamais comparée au grant). Symétrique pour `pkg.install
{"name":"evil","unit":"vim"}`. **Pas un bypass** (le plancher T2/T3 exige toujours
un humain) mais ça vide la mitigation S1 n°2 — « *l'humain voit l'action réelle* » —
de son sens. Cible désormais dérivée **par outil**, depuis l'argument qu'il utilise
vraiment. Parité **structurelle** via `derive_service()` : un seul endroit dérive
l'unité, appelé par le vrai chemin ET par `policy.check`.
**Mea culpa au passage** : un de mes tests de la veille était **tautologique** (il
passait déjà sans le correctif — il visait `policy_check` alors que le repli fautif
vivait dans `handle_tools_call`). Remplacé. Depuis, chaque test de régression est
**vérifié en restaurant le bug** : il doit échouer.

**Denylist `svc.restart` — l'axe manquant.**
[PR #52](https://github.com/Micka420-collab/vibeos/pull/52) : les deux axes déclarés
(« couper l'ACCÈS », « pipeline d'audit ») raisonnent tous les deux sur du
**retrait** de capacité. Or **`systemctl restart` sur une unité INACTIVE la
DÉMARRE** : `sshd.socket` n'était pas refusé → un redémarrage d'apparence anodine
**allume le SSH distant**. Axe (3) nommé. Et le glob `vibeos-agent@*.service` ne
pouvait pas matcher `vibeos-agent-egress@…` (il attend `@` là où le texte a
`-egress@`) → l'agent pouvait demander le redémarrage de l'unité qui compile **sa
propre allowlist d'egress**. Remplacé par un **espace de noms réservé** `vibeos-*`,
parce que c'est l'énumération qui a échoué. **Question de structure posée, pas
tranchée** : c'est un deny-list alors que la règle canonique est default-deny ;
passer `svc.restart` en allow-list exige de décider *quelles* unités — décision
opérateur.

**Supply chain : `gpgcheck` était un théâtre.**
[PR #48](https://github.com/Micka420-collab/vibeos/pull/48) : `rpm --import <url>`
(VSCodium) et `dnf -y` acceptant la clé annoncée par un `.repo` amont (mise)
faisaient un **TOFU à chaque build** — une URL compromise faisait importer une clé
hostile **sans aucun signal**, et `gpgcheck=1` la validait ensuite fidèlement. Clés
**épinglées dans le dépôt** (ASCII-armored, relisibles en diff), importées depuis
le disque, `gpgkey=` sur un chemin local. **Vérifiées** contre la signature réelle
du `repomd.xml.asc` : VSCodium recoupé **entre deux hôtes distincts**
(clé sur gitlab, paquets sur download.vscodium.com), mise sur un **hôte unique**
(plus faible, documenté). Portée honnête : empêche une clé de **changer** sans
qu'on le voie ; ne **prouve pas** son authenticité à la capture.

**Le HUD : un panneau déclaré mais mort.**
[PR #47](https://github.com/Micka420-collab/vibeos/pull/47) : `ReasoningPanel.history`
était déclaré et jamais lié ; sélectionner une session affichait « (chargement —
Phase 2.5) ». **Aucun outil MCP nouveau** : `agent.sessions` listait déjà tout et
le HUD **jetait la liste**. Outil existant enrichi (métadonnées par session,
**plafonné à 200** avec `total`/`truncated` — l'ADR-012 prétendait une « sortie
bornée » alors qu'elle croissait sans limite ; tri par **mtime** et non lexical —
l'ordre ne *paraissait* chronologique que parce que les ids intègrent un ts de
largeur fixe). **Aucun compteur de tours** : le produire imposerait de relire chaque
fichier à chaque poll (amplification I/O sur un T0 répétable). Relecture
adversariale du QML → **5 défauts user-visible** corrigés, dont la **sélection par
index** alors que la liste est retriée à chaque poll : le panneau aurait attribué le
raisonnement d'une session à une **autre** — le mensonge exact qu'il existe pour
empêcher. **1er contrôle CI du JS du HUD** (`scripts/check-hud-client.js`, 72
contrôles sous node **sans Qt** — `vibed_client.js` est une `.pragma library`) :
cette couche shippait **totalement non testée**.

**Durcissements issus d'un audit des zones jamais couvertes** (`main.rs`,
`tools/svc.rs`, `tools/sectools.rs`, `test_support.rs`, unités systemd) :
- [PR #53](https://github.com/Micka420-collab/vibeos/pull/53) :
  `vibeos-agent-egress@.service` était la **seule unité sans aucun durcissement**,
  alors qu'elle tourne en root et **compile le drop-in qui contient l'allowlist
  d'egress**. `ProtectSystem=full` et non `strict` — délibérément : sa seule cible
  d'écriture est `/run`, que `strict` monterait RO ; choisir la bonne dérogation
  casse sur une machine bootée, pas ici.
- [PR #54](https://github.com/Micka420-collab/vibeos/pull/54) : `VIBED_POLICY_DIR`
  (qui **remplace toute la politique**) était compilé dans le binaire de release
  alors que le commentaire disait « dev **only** ». Passé derrière la feature cargo
  `dev-overrides`, absente par défaut. **Prouvé** : `strings` sur les deux binaires
  → 1 occurrence avec la feature, **0 dans celui que l'image livre**. Lint CI ajouté
  **avec** la feature, sinon ce chemin ne serait plus jamais compilé et pourrirait
  jusqu'à casser le harnais E2E.
- **Résultat négatif de valeur** : `validate_unit_name` a résisté à tout
  (homoglyphes unicode, NUL, espaces, `SSHD.service`, `sshd.service.`, `sshd@`,
  traversée, échappements `\x`, injection d'options). Sa robustesse tient à une
  raison non évidente : **son charset est byte-pour-byte celui de systemd**, donc
  `unit_name_mangle()` ne réécrit jamais rien. `sectools.rs` et `test_support.rs`
  propres. **Ne pas re-auditer.**

**Le boot que tu avais demandé ship inerte.**
[PR #50](https://github.com/Micka420-collab/vibeos/pull/50) : le README réclamait
`plymouth-plugin-script`… **jamais installé**. Le thème déclare `ModuleName=script`
et le thème Fedora par défaut ne l'utilise pas → **rien ne le tirait**. Le jour où
la Phase 5 bascule le défaut, Plymouth serait retombé sur le thème de la distro. Un
piège posé pour plus tard. **N'active rien** : l'allumage (initramfs) reste Phase 5,
non testable sans machine — **décision Micka**.

**Doc.** [PR #51](https://github.com/Micka420-collab/vibeos/pull/51) : STATUS avait
sauté la session du 14→15 et annonçait donc `log.read` « en attente de revue »
(mergé) et « plus rien en attente de merge » (5 PR ouvertes). Le garde-fou
mécanique ne peut pas attraper ça — c'est du sens. Corrigé **un faux positif du
garde-fou** révélé par ce commit même : lister ses PR **ouvertes** est ce qu'un
STATUS honnête doit faire, mais le déclencheur `pr` le lisait comme une réclamation
de merge. *Un garde-fou qui punit l'honnêteté apprend à mentir.*

**Le CLI opérateur — la seule zone jamais auditée, et c'est par là que la décision
humaine entre.**
[PR #56](https://github.com/Micka420-collab/vibeos/pull/56) : **aucun
contournement du plancher** — usurpation d'euid, traversée par l'id (`safe_id` est
même *plus* strict que `safe_session_id`), injection terminal, TOCTOU
liste→approve : les quatre classes sont fermées. **Mais deux de ces défenses
tenaient par accident.** (1) L'injection terminal est bloquée par **effet de bord**
de l'encodage JSON (serde_json échappe tout 0x00–0x1F) — rien ne le disait,
`approvals_list` n'avait **aucun test**, et un futur « beau tableau » rouvrait
toute la classe en silence ; désormais un point de passage **nommé**
(`render_for_operator`), l'invariant énoncé, et un test qui pousse CSI+CR+BEL à
travers `target`. (2) `parse_effective_uid` retournait sur la **première** ligne
`Uid:` — or `Name:` vaut le basename de l'exécutable et un nom de fichier Linux
peut contenir un saut de ligne ; ça ne marche que parce que le **noyau** échappe
`comm`, garantie dont ce code n'a pas à dépendre en silence. Exige désormais une
seule correspondance.
Et [#49](https://github.com/Micka420-collab/vibeos/pull/49) borne la cible **à la
source** : elle n'était plafonnée qu'à 1 Mio × 16 demandes = ~16 Mio de texte
déversés dans le terminal de l'opérateur, avec `target` s'affichant **avant**
`tier`/`tool` (clés JSON alphabétiques) — l'action réelle poussée hors écran.
**Rejeter, pas tronquer** : tronquer donnerait une primitive de tromperie qui
n'existe pas (lire un préfixe, approuver la chaîne entière).

**Mes propres correctifs relus — et deux étaient faux.**
Un correctif est exactement l'endroit où se cache le bug suivant. Sur
[#47](https://github.com/Micka420-collab/vibeos/pull/47) : mon garde anti-reset
**ne gardait rien** (j'avais écrit que les libellés étaient « grossiers » — faux :
précision à la seconde, à l'octet ; la session live est dans la liste, donc la clé
changeait à **100 % des polls** dès qu'un agent écrivait — l'échec exact qu'il
devait empêcher). Corrigé à la racine : le modèle n'est plus remplacé mais
**diffé en place** (ListModel). Et un **échec de lecture devenait une
affirmation** (« aucun raisonnement capté ») — un fait fabriqué, déclenchable par
un simple redémarrage de vibed ; il se dit maintenant échec.

**Reste ouvert (décision Micka, pas de l'agent)** : allumer le splash de boot ;
`svc.restart` en allow-list ? ; quel outil T1 réel + son allowlist ; Phase 4 ;
« l'IA modifie l'OS » ; Mammouth AI ; voix/GUI. **Machine-gated** : boot VM, TPM2
live, egress live, rendu HUD sur Plasma booté, Zed Tier B, NVIDIA.

## 🔧 Session 2026-07-14/15 (nuit, autonome) — log.read + revue adversariale (8 sous-agents, 2 vagues) → 4 vrais bugs sécurité corrigés

**Travail codable du mandat** :
- **[PR #37](https://github.com/Micka420-collab/vibeos/pull/37) `log.read` (T0, ADR-011)** : lecture de journal **allowlistée** (`[rule.services].allowed`, défaut refus, évaluée avant le plancher de tier), **bornée** (≤ 200 lignes + 64 Kio), **rédaction best-effort**, aucun filtre libre, audit de l'unité. **Touche `policy.rs`** (ajout `allowed` à `ServiceConstraints`) → flaggé revue humaine. Doc synchronisée (README surface d'outils + THREAT-MODEL mitigation S2).
- **`pkg.install`** : ADR-016 complet, allowlist non tranchée → rien codé.

**Revue adversariale (4 sous-agents parallèles)** sur le code neuf + la surface d'exfiltration. Résultat : `log.read`, l'allowlist `policy.rs`, et le pipeline `mcp.rs` (rate-limit → denylist → policy → approbation → exec) **CLEAN, aucun bypass** (canonicalisation d'unité identique des deux côtés, allowlist default-deny, plancher T2/T3 intact). `audit.rs`/`sha256.rs` robustes. **Deux vrais bugs trouvés (vibed lit en ROOT, `/proc`/`/run`/`/etc` sont « system read prefixes »)**, vérifiés contre le code puis corrigés :
- **[PR #38](https://github.com/Micka420-collab/vibeos/pull/38)** (HIGH) : denylist étendue — `/proc/kcore`+`/proc/kallsyms` (mémoire noyau + défaite KASLR), `/proc/*/mem`+`/proc/*/pagemap`, `/run/user/**`+`/run/secrets/**` (cross-user), secrets `/etc` non-standard (krb5/sssd/ipsec/pki private). Synchronisé code + miroir `default.toml` + commentaire. Test dédié.
- **[PR #39](https://github.com/Micka420-collab/vibeos/pull/39)** : `confine_read` confine désormais `/proc/<pid>` à l'uid **propriétaire** (fail-closed) — ferme la recon cross-user résiduelle (maps/status/net d'autres users).

**Revue adversariale vague 2** (3 sous-agents : `approval.rs`, `supervisor.rs`, scripts shell `os/rootfs`). `approval.rs` (portail T2/T3 one-shot) **CLEAN** (forge/replay/self-approve/scope/expiry/caps tous fail-closed & race-free). Deux vrais bugs corrigés :
- **[PR #42](https://github.com/Micka420-collab/vibeos/pull/42)** (MEDIUM) : `vibeos-agent-egress.sh` ajoutait toute IP résolue à `IPAddressAllow` sans filtre — un hôte allowlisté résolvant (poisoning/CNAME) vers `169.254.169.254`/RFC1918/loopback perçait le mur egress vers l'interne (SSRF). `is_internal_ip()` les exclut (validé : 12 internes droppées, 9 publiques gardées).
- **[PR #43](https://github.com/Micka420-collab/vibeos/pull/43)** : le budget wall-clock de `agent run --budget` (garde-fou primaire runaway) était mesuré sur `SystemTime` non-monotone → un pas d'horloge en arrière laissait un agent dépasser son budget. Passé à `Instant` monotone (+ arm `try_wait` Err durci).

**Autres livrables nuit** :
- **[PR #41](https://github.com/Micka420-collab/vibeos/pull/41)** : test du garde anti-horloge-inversée du rate-limiter (`saturating_sub`, documenté mais non couvert).
- **[PR #44](https://github.com/Micka420-collab/vibeos/pull/44)** : les scripts sécurité `os/rootfs` (egress/run/seal-token/selfcheck) ajoutés au job shellcheck CI (n'étaient pas couverts).
- **Rédacteur `log.read`** (#37) amélioré : masque aussi les credentials en URI (`scheme://user:pass@host`) + re-cap octets après rédaction.
- **Containerfile** revu : supply-chain disciplinée (base par digest, téléchargements sha256-vérifiés, repos GPG, `npm --ignore-scripts`) — aucun bug.

Mineurs latents notés (mémoire) : check load-time `services.allowed` sur outils sans unité ; TOCTOU pid-reuse de #39 ; épinglage empreinte clé GPG VSCodium. **Aucun merge/fermeture de PR par l'agent** (le classifieur bloque aussi `gh pr merge` de mon côté) ; `policy.rs`/`fs.rs`/`supervisor.rs` flaggés pour revue.

## 🔧 Session 2026-07-14 (soir) — garde-fou anti-dérive + trajectoire + branding boot

Après l'analyse complète du dépôt, deux priorités **avant tout code** :

- **PRIORITÉ 1 — [PR #33](https://github.com/Micka420-collab/vibeos/pull/33) `verify-roadmap-truth` (mergée)** : `scripts/verify-roadmap-truth.sh` + workflow CI `roadmap-truth.yml`. Vérifie **mécaniquement** la doc de statut (ROADMAP/STATUS/DECISIONS) contre le dépôt réel, à chaque push vers `main`. **HARD FAIL** (non ambigu) : lien markdown mort, fichier repo cité mais absent. **WARNING** (heuristique) : plus grand compteur de tests annoncé vs réel `#[test]`, PR citée « mergée » absente de l'historique. L'en-tête **dit explicitement** qu'il n'attrape que le mécanique — « proposé/en cours/fait » reste un jugement humain. Le script a **trouvé 2 vraies dérives dès sa 1re exécution** (`scripts/e2e-zed.sh` mal cité dans STATUS/DECISIONS), corrigées dans la PR.
- **PRIORITÉ 2 — [PR #34](https://github.com/Micka420-collab/vibeos/pull/34) note de trajectoire Phase 4 (mergée)** : note **datée** en tête de la section Phase 4 du ROADMAP. Constat honnête : Phase 4 (durcissement, « chemin critique » auto-déclaré, 4–6 mois) **n'a pas démarré** 11 jours après la Phase 0 (SELinux/boot mesuré/sandbox à zéro, `vibed` encore root) pendant que la Phase 2.5 avançait hors chemin critique. **La note pose la décision de séquencement à Micka sans la trancher** — l'agent ne démarre pas Phase 4 sans go explicite.

**Aussi** :
- **[PR #35](https://github.com/Micka420-collab/vibeos/pull/35) splash de boot Plymouth (mergée)** : les 3 assets manquants (`ring`/`mark`/`wordmark`) **générés** par `desktop/plymouth/generate-assets.py` (œuvre originale, reproductible, Pillow) — **spirale galactique** mauve→blue (le motif de marque des wallpapers). Le thème existant dégradait en fond nu faute d'assets ; il rend maintenant l'animation (spirale qui tourne + cœur qui respire). Rendu Plymouth **animé non validé sur machine bootée** (machine-gated) ; activation reste 🛣️ Phase 5.
- **`pkg.install`** (ADR-016) : allowlist **non tranchée** (choix layering-vs-distrobox), ADR déjà complet avec options concrètes → **rien codé** (conforme).
- **`log.read`** (ADR-011, T0) : **non démarré** — optionnel (« si le temps le permet après 1 et 2 »), prêt à implémenter selon l'ADR.

## 🔧 Session 2026-07-14 (après-midi) — récupération de contenu échoué hors de `main`

**Contexte** : après merge de #11 et #14 dans `main`, les ex-PR **#12 (F6)** et
**#13 (fix sécurité alias `/home`↔`/var/home`)** ont été marquées *merged* par
GitHub **mais dans leurs bases intermédiaires empilées**, pas dans `main`. Vérifié :
`fold_home_alias` et `tools/fs.rs` **absents de `main`** ⇒ la vuln alias était
**vivante sur `main`** et F6 non appliqué. Piège classique des PR empilées mergées
dans le désordre.

**Récupération** (les ex-PR closes non ré-ouvrables ⇒ PR fraîches, **indépendantes,
base=`main`** — ne peuvent plus s'échouer) :
- **[PR #20](https://github.com/Micka420-collab/vibeos/pull/20)** `security-home-alias-fix` — cherry-pick du fix (`policy.rs`), **150 tests verts** sur la base `main` réelle. + bannière ultra-visible en tête de `STATUS.md`, `docs/MERGE-GUIDE.md` réécrit (post-mortem), éval Dependabot, note sécurité §3.3.
- **[PR #19](https://github.com/Micka420-collab/vibeos/pull/19)** `f6-fs-extraction` — cherry-pick de F6 (`mcp.rs`→`tools/fs.rs`), **149 tests**, CI verte. Fichiers **disjoints** de #20 (`mcp.rs`/`tools/` vs `policy.rs`) ⇒ merge quelconque ordre.
- **Rien mergé/fermé par l'agent** (invariant, d'autant plus après la confusion).

**Autres livrables** :
- **Dependabot #15/#16/#17** évaluées **sûres** (bumps CI/Actions, pinning SHA
  maintenu, #16/#17 = re-pin même version, #15 checkout v4→v7 CI verte) — consigné
  dans `STATUS.md` pour une décision de merge informée.
- **Revue adversariale alias OSTree × denylist intégrée** : `builtin_denied` est
  alias-aveugle (`/root/**` ne couvre pas `/var/roothome/**`) mais **aucun exploit
  agent non-root** — `confine_read` (lectures) + `USER_WRITE_PREFIXES`/
  `confine_to_caller_home` (écritures) rattrapent tout (tracé ligne par ligne).
  Durcissements défense-en-profondeur consignés dans `SECURITY-ARCHITECTURE.md`
  §3.3 (à faire après stabilisation de `mcp.rs` sur `main`).
- **Santé de `main` confirmée** post-merges : 149 tests verts, clippy propre.

## ✅ Fait (livré, testé, poussé)

**Session étendue (16h → 23h, 2026-07-13)** :
- **Correctifs auto-signalés** : approval fs I/O (`check_and_consume_grant`/
  `request_approval`) déplacée sur `tokio::spawn_blocking` ; grant-consommé-
  avant-audit **tranché et documenté** (garde la garantie one-shot).
- **ADR-012 implémenté** : module `reasoning` (store `memory/reasoning/
  <session>.jsonl`, `safe_session_id` anti-traversal), outil MCP **T0
  `agent.thinking`**, Genesis crée `reasoning/`.
- **ADR-012/013 — superviseur** `vibectl agent run/stop/thinking` : tap
  `stream-json` → store, budgets wall-clock + nb d'appels, kill-switch
  opérateur (marqueur `.stop`), type de journal réservé `autonomous_session`,
  groupe de processus + group-kill + drain borné (ne se suspend jamais).
  **N'approche jamais `approval.rs`** ; T2/T3 restent gérés par vibed.
- **Revue adversariale indépendante** (sous-agent) : **aucun bug high/medium**,
  contrat de sécurité intact (traversal, denylist, type réservé, plancher T2/T3,
  surface opérateur-only). 5 items availability/robustesse durcis — **traçabilité
  finding → commit → test** :

  | # | Finding (sévérité) | Correctif | Commit | Test qui le couvre |
  |---|---|---|---|---|
  | C1 | Lecture stdout NON bornée → OOM du superviseur (med-low) | `read_capped_line` (cap = `REASONING_MAX_LINE_BYTES`, ligne trop longue = drop) | `7e1f0c3` | `read_capped_line_drops_oversized_lines` (vibectl.rs) |
  | A | `--calls` sous-compte les `tool_use` **parallèles** (low) | `supervisor::count_tool_use` (compte les blocs, pas « any ») | `7e1f0c3` | `count_tool_use_counts_parallel_calls` (supervisor.rs) |
  | C2 | Petit-enfant tenant le pipe → fuite thread lecteur sur sortie **propre** (low) | `terminate_group` après drain si le lecteur traîne (pid capturé avant reap) | `7e1f0c3` (+ test dédié ajouté après) | `agent_run_returns_even_when_a_grandchild_holds_the_pipe` (vibectl.rs) |
  | B | `read_thinking` slurpe tout le fichier pour un tail (low) | `read_tail_string` (lecture bornée ≤ 4 MiB depuis la fin, drapeau `window_bounded`) | `7e1f0c3` | `read_tail_string_bounds_large_files` (reasoning.rs) |
  | C3 | Budget illimité par défaut / valeur invalide silencieusement illimitée (low) | `--budget`/`--calls` invalides → **erreur** (plus de fallback silencieux) + WARNING si run illimité | `7e1f0c3` | `parse_duration_forms` (supervisor.rs — rejette `0`/`abc`/`8x`/`8h30`→None ; le bin transforme None→erreur, glue triviale) |

  Le grant-consommé-si-audit-échoue (relevé low) est **laissé tel quel à dessein** :
  c'est le sens fail-closed voulu du one-shot (documenté en commentaire, `5a165e8`).
- **Durcissement systemd** : genesis + agents-group (options non-mount-namespace,
  contraintes respectées) ; generator amnésique déjà durci.
- **Initiative « VibeOS pour Zed »** (**ADR-014**, cible l'adaptateur
  `claude-code-acp`, jamais le cœur de Zed) :
  - **Investigation** du code réel avant tout patch : `canUseTool` public
    (wrappable), `createSession` **privé**, `runAcp` construit la base en interne
    → forme retenue = **patch de prototype de `canUseTool`** (vérifiée).
  - **Couche 0/1** (config) : `settings.json` Zed + `CLAUDE_CONFIG_DIR` Zed-only
    avec `permissions.deny` (Read/Write/Edit natifs off, terminal non affecté).
  - **Groundwork couche 2** : outil MCP **T0 `policy.check`** dans vibed
    (classification dry-run — allow/deny/require_approval, sans exécuter/approuver,
    ne touche pas `approval.rs`).
  - **Couche 2 (le fork)** : paquet `zed/vibeos-claude-acp` (TypeScript) qui
    patche `canUseTool` → `vibeos:policy.check` (Allow T0/T1 sans prompt, T2/T3
    jamais auto, fail-safe). **Vérifié** : `tsc` compile contre les vrais types
    amont + **12 tests vitest** (logique du mode auto + mapping d'outils + client
    MCP socket testé contre un faux vibed). Innovation : mode auto piloté par
    MOTEUR DE POLITIQUES (pas classifieur LLM). Reste : install image + Zed live.
- **README multilingue** (FR canonique + EN/ES/DE).
- **Hygiène PR** : PR #5 (branche→main) mergée à l'état du matin ; ~44 commits
  d'après-midi orphelins → nouvelle **PR draft #11 (branche → main)** pour les
  rapatrier. Sort de PR #4 (empilée) laissé à l'humain.

**Nuit — nettoyage + vérifications réelles (8 points demandés)**, tout poussé sur PR #11 :
1. **Traçabilité des 5 findings** : table finding→correctif→commit (`7e1f0c3`)→test
   ci-dessous ; **test dédié C2 ajouté** (`agent_run_returns_even_when_a_grandchild_holds_the_pipe`).
2. **Blocage Zed re-qualifié** (`BLOCKERS.md`) : l'extension (agent ACP stdio) se
   valide **sans Zed** — `tsc` + boot ACP headless (`npm run smoke`) + 17 tests ;
   seul le **E2E complet** reste bloqué (liste précise de ce qui manque).
3. **Preuve de déterminisme** (`test/patch.test.ts`) : même entrée ×20 → décision
   identique, 1 `policy.check`/appel, **zéro LLM**.
4. **Kill-switch mesuré** : `agent stop` → **2,636 s** (< 5 s), dernier append
   raisonnement = JSON complet. Critère Phase 2.5 atteint (mesuré).
5. **policy.check anti-DoS confirmé par test** (rate-limité par uid, sortie bornée).
6. **Plan supply-chain npm** (`ADR-015`) + **lockfile commité**.
7. **Passe de cohérence** : 140 Rust + 17 vitest partout, statuts ADR/ROADMAP recalés.
8. **PR #11 rendue vraiment mergeable** : la CI échouait (MSRV 1.75 + cargo audit)
   car **main portait un bump Dependabot `toml 1.1.2`** (→ `serde_spanned 1.1.1`)
   incompatible MSRV 1.75. Merge de main + **revert du bump toml** (garde `0.8`) +
   règle Dependabot. **CI Rust re-verte** ; PR MERGEABLE.
- **État** : **139 tests vibed verts** (132 unit + 5 e2e MCP + 2 politique) +
  outil T0 `policy.check` (groundwork Zed) + **12 tests vitest** de l'extension ;
  clippy `--locked` + fmt propres.

**Nuit (2) — implémentation des 5 briques restantes**, tout poussé sur PR #11 :
- **`svc.restart` (T2) — backend RÉEL** derrière le grant one-shot (n'est atteint
  qu'après `vibectl approve`) : `systemctl restart` (nom validé, `--`, chemin
  absolu, env vidé, borné par le timeout de job systemd) + **relecture d'état**
  pour prouver le redémarrage. `handle_connection` reçoit désormais le répertoire
  d'approbation (injectable) → **test e2e sur socket** : demande→refus T2→approve
  hors bande→ré-appel→grant consommé→audit `started_approved(by_uid=0)`, one-shot
  vérifié. + tests unitaires hermétiques (fake systemctl). THREAT-MODEL à jour.
- **Extension Zed câblée dans l'image (ADR-015)** : étage `zed-agent-builder` —
  `npm ci --ignore-scripts` + **bundle esbuild** vers un unique `.mjs` autonome
  (jamais `node_modules` ni sources TS). **`npm audit --omit=dev` = 0 vuln**
  (les 5 restantes sont dev-only). **Gardé off** (`ARG WITH_ZED_AGENT=0`, ADR-015
  §6) : les deux chemins construisent (podman vérifié) ; à 0 le builder npm est
  hors graphe (marqueur `NOT-INSTALLED.txt`), à 1 seul le bundle est copié.
- **Phase 2.5 — reste livré** : `vibeos-agent@.service` (always-on, `User=%i`
  jamais root, durci sans MDWX car CLI Node), **jeton scellé TPM2**
  (`LoadCredentialEncrypted=` + `vibeos-agent-seal-token.sh`), **allowlist egress
  par nom d'hôte** (`vibeos-agent-egress@.service` + `agent-egress.conf`,
  `getent`→`IPAddressAllow`). shellcheck + `systemd-analyze verify` propres.
- **E2E Zed turnkey** (`scripts/e2e-zed.sh` + `e2e-live-policy.mjs`) : **Tier A
  VALIDÉ sur socket vibed live** — fs.read/fs.list (T0)→allow auto, pkg.install/
  svc.restart (T2)→require_approval, disk.wipe→deny (5/5 PASS, audit écrit).
  Overrides dev `VIBED_SOCKET`/`VIBED_POLICY_DIR`/`VIBED_AUDIT_DIR`. Tier B
  (round-trip éditeur) = checklist, non lancé ici (Zed non headless).
- **HUD branché en LIVE** : `Quickshell.Io.Socket` sur `/run/vibed/mcp.sock` —
  os.status + memory.query + raisonnement (nouvel outil T0 **`agent.sessions`** →
  `agent.thinking`) live ; observateur strict T0, dégradation gracieuse. Roster
  agents + jauge ollama restent hors-ligne (pas d'`agents.list`).
- **F6** inscrit en **dette explicite** (ROADMAP §9 ter, effort 1–2 j).
- **État** : **145 tests vibed verts** (136 unit + 7 e2e MCP + 2 politique) +
  **17 vitest** + smoke ACP + Tier A live ; clippy/fmt propres ; PR #11
  **MERGEABLE, CI Rust verte** (11 checks pass, build image en cours).

**Analyse + améliorations (matin)** — analyse ultracode (6 agents), revue
adversariale (24 agents, 15 findings corrigés), et une **trousse cybersécurité
gouvernée** (≈ 60 outils pentest/DFIR embarqués + catalogue `docs/SECURITY-TOOLKIT.md`
+ outil MCP `sectools.list` T0). CI durcie (cargo audit/fmt/--locked/clippy
--all-targets, garde `publier`, digest bootc-image-builder, Dependabot).
Sécurité vibed (denylist credentials IA, anti-TOCTOU). Wallpaper `VibeOS.png`
par défaut. ~40 recalages docs. Image bootc construite et inspectée.

**Priorités mémoire/MCP/Genesis (après-midi)** :
- **P1** `memory.append` scopes `user`/`projects` (append-only strict, fold
  last-write-wins) — plus aucun scope agent manquant.
- **P3** événement `tool_call` (réservé système) écrit par vibed dans le
  journal mémoire à chaque action T1+ exécutée.
- **P4** generator systemd du **mode amnésique** (`vibeos.amnesic=1` → tmpfs +
  `VIBEOS_MEMORY_MODE=amnesic` + marqueur), shellcheck + 8 tests fonctionnels.
- **P5** `hardware.json` **schema 2** (cpu/mem/gpu structurés + blobs bruts) +
  smoke test Genesis de non-régression en CI.
- **Audit inviolable** : chaîne de hachés **SHA-256** (`seq`/`prev`/`hash`,
  SHA-256 maison sans dépendance, vecteurs NIST) + `vibed --verify-audit`.
- **vibectl** (2ᵉ binaire) : `memory status`, `memory mode`, `audit verify`.

**Findings de la revue Fable 5** (7/7 traités sauf F6) :
- **F1** `fs.read`/`fs.list` **confinés au home de l'appelant** (SO_PEERCRED) +
  allow-list système (`/etc /usr /proc /sys /run /var/lib/vibeos`) — ferme le
  vrai trou v0.1 (lecture cross-user des données personnelles).
- **F2** `memory.query` rend des **extraits de contenu bornés** (lecture en un
  appel, réaligné sur la spec).
- **F4** **rotation** du journal d'audit par jour UTC, chaîne **continue** entre
  fichiers ; `verify_chain` parcourt tout le répertoire.
- **F5** cohérence doc `vibeos-agents` (wheel auto / non-wheel opt-in) +
  **ADR-010** (identité de l'appelant `[rule.callers]` via `/proc/<pid>/exe`).
- **F7** `CLAUDE.md` dans `/etc/skel/.claude/` (boucle de valeur mémoire :
  memory.query au début, memory.append en fin).
- **F3** **flux d'approbation humaine minimal** T2/T3 : requête → `vibectl
  approve <id>` → **grant à usage unique** (borné (outil,cible,uid), expire
  5 min) consommé au ré-appel → exécution auditée `*_approved`. Store root-only
  + denylisté ; un agent ne peut jamais approuver sa propre requête.
- Cosmétique : Genesis ne bake plus un hostname transitoire (localhost/fedora)
  comme nom de naissance.

**Durcissements complémentaires (fin d'après-midi)** :
- **Supply-chain CI 2026** : job `supply_chain` (SBOM anchore/sbom-action +
  scan Trivy), job **MSRV 1.75** (build+test `--locked`), actions épinglées
  par SHA, Dependabot.
- **Bornage du store d'approbation** : `request_approval` purge les `pending`
  périmés (> 1 h), **déduplique** les requêtes identiques (tool,target,uid) et
  applique un **plafond dur** (64) — un agent ne peut plus remplir le volume
  mémoire en spammant des appels T2/T3 (anti-DoS, analogue à F4).
- **`vibectl approve/deny` réservés à root** (garde `require_root` explicite,
  fail-closed si euid indéterminable).
- **Responsabilité dans l'audit** : l'`outcome` d'un appel approuvé porte l'uid
  de l'opérateur (`ok_approved(by_uid=N)`) — le grant étant supprimé à la
  consommation, le journal inviolable est la seule trace durable de *qui* a
  autorisé le changement système.
- Passe de cohérence docs : chemin d'audit (rotation par jour) et
  approbation/user-projects décrits comme livrés partout.

**Audit Fable 5 (4 points — tous traités)** :
- **n°1/n°2** confinement `fs.read`/`fs.list` au home appelant + allow-list
  système : **déjà livré** (F1).
- **n°3** `source` documenté **non fiable** (auto-déclaré par l'agent, jamais
  une preuve de provenance/autorité) — MEMORY.md §9, THREAT-MODEL §6,
  description de l'outil ; garde-fou avant toute consolidation `knowledge`.
- **n°4** `[rule.callers]` via `/proc/<pid>/exe` : décision **posée** en
  ADR-010 (cible Phase 3/4).
- **n°5** **rate-limiting par uid** (token bucket, module `ratelimit`) : borne
  un agent emballé/compromis (flood audit + mémoire + approbations) ;
  dépassement = refus fail-closed audité `rate_limited`. Rétention/purge du
  journal = politique opérateur (purge = T3) ; rotation par jour déjà en place.

**Revue adversariale du code du jour** (sous-agent, 4 fichiers) : **aucun bug
high/medium** ; 1 MED + points low traités — TOCTOU du plafond `MAX_PENDING`
sous concurrence (verrou de sérialisation + test 128 threads), parse euid
fail-closed (`parse_effective_uid`, uid effectif uniquement). Le grant consommé
si l'audit échoue est laissé tel quel (fail-closed voulu du one-shot).

**Phase 2.5 ajoutée au ROADMAP** (« Autonomie encadrée & accès IA externes »,
proposée) : superviseur d'agent budgété + kill-switch humain, auth abonnement
scellée TPM2, allowlist egress par unité, type réservé `autonomous_session` —
périmètre figé T0/T1. **ADR-011** (log.read T0 anti-exfiltration) posé.

**Extension Phase 2.5 (demande utilisateur)** :
- **Mode autonome « always-on »** (ADR-013) : le superviseur tourne en
  permanence, l'agent enchaîne seul TOUT le T0/T1 sans humain synchrone ; les
  T2/T3 ne bloquent plus mais sont **mis en file** pour approbation asynchrone.
  Le plancher T2/T3 **n'est jamais levé** (invariant §7, THREAT-MODEL S1) —
  interprétation responsable de « autonome pour tout » = autonome sur tout le
  T0/T1 sans babysitting, pas « exécute du destructif sans accord ».
- **Capture du raisonnement** (ADR-012) : tap passif sur le flux `stream-json`
  du CLI (jamais son transcript disque), store `memory/reasoning/`, futur outil
  T0 `agent.thinking`, toggle par session.
- **HUD** : `ReasoningPanel.qml` livré en **scaffolding** (3ᵉ pilier « pourquoi »,
  chip + popup verre, ship avec `[]` — règle d'honnêteté), câblé dans la barre.

**Revue adversariale finale** (2ᵉ passe, tout le code de la session) : **aucun
bug de correction high/medium**. Rust propre (mutex approval, parseur euid,
câblage rate-limit/approbation), pas de régression ; ADR uniques/séquentiels,
invariant T2/T3-sans-bypass préservé partout, tokens `Theme` du QML tous
présents, structure QML bien formée. Corrigé : import `QtQuick.Shapes` inutilisé.
Laissés (non-bugs) : ancrage `PopupWindow` (auto-signalé, à valider sur desktop
booté), I/O bloquante sur reactor + grant-burn-si-audit-échoue (fail-closed,
pattern existant).

**État tests** : **114 tests vibed verts** (107 unitaires + 5 intégration MCP
e2e + 2 politique) ; `clippy --all-targets --locked -D warnings` 0 warning ;
`fmt --check` OK ; `cargo build --locked` des 2 binaires OK ; shellcheck vert.
Images `vibeos:dev-final`, `dev-final2` **et** `dev-final3` (arbre final complet)
construites, `bootc container lint` OK (11 checks, 2 warnings d'hygiène, 0 erreur).

**Nuit 3 (2026-07-14) — durcissement + agents.list + F6 + ADR**, tout poussé sur PR #11 :
- **PR #11 état FRAIS vérifié** (`gh pr checks`) : `mergeStateStatus: CLEAN`,
  12 pass / 3 skipping / **0 échec** — entièrement verte (build image inclus).
- **CRITIQUE — allowlist de CIBLES svc.restart** : l'allowlist **existait déjà**
  (`[rule.services].denied`, évaluée AVANT le floor T2 dans `policy.rs` → `Deny`,
  pas `require_approval`) mais était **incomplète**. Complétée avec les unités
  d'**accès** (`sshd`, `NetworkManager`/`networkd`, `display-manager`/`sddm`,
  `logind`), d'**approbation** (`vibed`, `vibeos-agent@*`, `polkit`) et le **bus**
  (`dbus-broker`/`dbus`) — refus d'office, hors de portée de la file d'approbation.
  **Test sur la politique livrée** (`shipped_policy_denies_restart_of_critical_units_before_approval`).
- **`agents.list` (T0)** : roster HUD dérivé de l'audit, **confiné à l'uid appelant**
  (l'agent de A ne voit jamais B ; soi-même exclu), groupé par pid. Anti-DoS
  (rate-limit, queue/fenêtre bornées). HUD : roster live + jauge ollama (probe
  local XHR/nvidia-smi). Fait sauter le dernier « hors-ligne » du HUD.
- **F6 — 3/4 familles extraites** (mécanique, zéro changement, 147 tests inchangés) :
  `tools/svc.rs`, `tools/sectools.rs`, `tools/memory.rs` (impl **et** tests).
  **mcp.rs 4257 → 2777 lignes (−35 %)**. `fs` reste (entrelacé : 7 internes testés
  + `builtin_denied` partagé + helpers de test partagés) → session dédiée.
- **Docs** : `agent.sessions` spécifié (ADR-012) ; `WITH_ZED_AGENT=0` verrouillé
  comme choix intentionnel (ADR-015 §6, avertissement anti-régression) ;
  **ADR-016** — `pkg.install` backend **reporté** (allowlist paquets/dépôts non
  tranchée sur OS immuable ; stub conservé) ; THREAT-MODEL à jour. Tier B Zed
  relu : **aucun bug**.
- **Revue adversariale indépendante** (sous-agent) du code de la nuit → **3
  défauts réels corrigés, dont 1 HIGH** :
  - **HIGH — bypass de la deny-list svc.restart** : la policy recevait le nom
    d'unité **brut** (`args["unit"]`) mais la canonicalisation (`+ .service`)
    ne tournait qu'**après** la décision → `svc.restart {"unit":"vibed"}`
    passait en `RequireApproval` au lieu de `Deny` (les 13 unités critiques
    redevenaient approuvables). **Mon test d'hier soir ratait le trou** (noms
    qualifiés seulement). Fix : canonicalisation dans `handle_tools_call`
    **avant** l'évaluation + **test e2e socket** (nom nu → `Deny`).
  - **MED** — `agents.list`/`agent.sessions` sans règle allow → default-deny →
    inertes en prod (fix : règle T0 `agent-observability`).
  - **MED** — deny-list complétée (`user@*.service`, `dbus.socket`).
  - Confinement `agents.list`, extraction F6 memory, anti-DoS : **confirmés sains**.
- **Vérif transverse** (déclenchée par le bug HIGH) : aucun autre outil n'a le
  pattern « validation après décision policy ». Seuls `fs.*` (chemin normalisé
  tôt + recheck canonique anti-symlink déjà en place) et `svc.*` (désormais
  canonicalisé) ont une cible policy-pertinente. Pas de bug frère.
- **Durcissement helpers agent-runner** (défense en profondeur) : validation du
  nom d'instance (`%i`) dans les 3 scripts shell Phase 2.5 (rejet hors
  `[A-Za-z0-9._-]` → pas de traversée de chemin). shellcheck propre.
- **État** : **148 tests vibed verts** (137 unit + 8 e2e MCP + 3 politique) +
  17 vitest + smoke ACP + bundle Zed ; clippy/fmt/shellcheck propres ; **CI Rust
  verte sur le commit de fix** ; PR #11 MERGEABLE.

**Prolongation (jusqu'à 09h) — CI, README, 2ᵉ revue** :
- **Flaky ETXTBSY corrigé** : le monitoring CI a attrapé un test que j'avais
  introduit (`svc_restart_surfaces_a_systemctl_failure` — exec d'un fake systemctl
  fraîchement écrit → « Text file busy » sous cargo test parallèle, invisible en
  local/MSRV). Fix : retry sur ETXTBSY (artefact de test ; la prod exécute
  `/usr/bin/systemctl` statique). CI re-verte.
- **README (4 langues) mis à jour** et synchronisé (HUD live, svc.restart réel,
  agents.list, Phase 2.5 « largement implémentée ») — corrigé les affirmations
  périmées « HUD mocked »/« Phase 2.5 proposed » dans EN/ES/DE. FR canonique.
- **2ᵉ revue adversariale** (primitives cœur : audit/sha256/ratelimit/approval/
  superviseur) : **saines, aucun défaut exploitable**. 1 vrai bug LOW corrigé —
  **écriture déchirée → fausse rupture `verify_chain`** (rollback de la queue non
  terminée au démarrage + test). Docs rendues honnêtes : portée de la
  tamper-evidence (keyless, troncature de queue non détectée sans ancrage Phase 4),
  budget `--calls` best-effort, petit-fils `setsid()` (fuite jamais hang).
  **Extension Zed** relue : gate de gouvernance sain (fail-safe partout).
- **3ᵉ revue adversariale** (confinement fs — la surface sécurité la plus
  critique) : machinerie symlink/canonicalize/dev-ino **saine** (pas de bypass de
  lecture cross-user), mais **3 gaps corrigés** :
  - **#1 (MED, DoS réel)** — `fs.read` sur un **FIFO** bloquait le worker (guard
    `is_file()` après l'`open()` bloquant) → épuisement du pool = déni cross-tenant.
    Fix : type vérifié **avant** l'open + `O_NONBLOCK` + test FIFO.
  - **#2 (MED, blind spot hardlink)** — denylist path-based aveugle aux hardlinks.
    Fix : lecture confinée au home → inode owned par l'appelant (`fstat st_uid`),
    bloque l'escalade cross-owner ; system reads exemptés (`ReadScope`).
  - **#4 (LOW, fail-open)** — home résolu à `/` → confinement inopérant → refus.
  - Résidu documenté : TOCTOU parent intermédiaire de `fs.write` (openat2 = Phase 3).
  - **Bug e2e** corrigé (relecture `index.ts`) : nom d'env socket (`VIBED_MCP_SOCKET`).
- **Bilan des 3 revues** : 1 HIGH (bypass deny-list svc.restart) + plusieurs
  MED/LOW, tous corrigés. Couverture : code de la nuit, primitives cœur
  (audit/ratelimit/approval/superviseur), confinement fs, extension Zed.
- **État prolongation** : **149 tests vibed verts** + 17 vitest ; clippy/fmt/
  shellcheck propres ; PR #11 MERGEABLE, CI Rust verte.

## 🔧 En cours / non terminé (checkpoint final 2026-07-13 nuit)

- **Zed — E2E Tier B (round-trip éditeur)** : le **Tier A est validé sur socket
  vibed live** (décisions fs.read→allow / pkg.install→require_approval, `scripts/
  e2e-live-policy.mjs`). Reste le Tier B — Zed spawn le binaire Claude → vrai appel
  d'outil → prompt supprimé pour un Allow, affiché pour un require_approval. Non
  lançable ici (Zed non headless). Turnkey prêt : `scripts/e2e-zed.sh`.
- **Zed — expédition dans l'image** : l'étage `zed-agent-builder` est livré et
  **construit** (bundle esbuild vérifié), mais **gardé off** (`WITH_ZED_AGENT=0`,
  ADR-015 §6) jusqu'à la validation du Tier B.
- **Phase 2.5 — enforcement live** : unité `vibeos-agent@`, jeton TPM2, egress
  livrés et statiquement validés ; le **comportement au boot** (unseal TPM2 réel,
  egress BPF) exige une machine bootée.
- **HUD** : os.status/memory.query/raisonnement **+ roster agents (`agents.list`)
  + jauge ollama (probe local)** désormais **live** — plus de « hors-ligne » (QML
  non vérifiable au runtime ici : Quickshell non headless).
- **F6 (découpe de `mcp.rs`)** : **3/4 faits** (svc, sectools, memory ; mcp.rs
  4257 → 2777 l.). **`fs` reste** (entrelacé : 7 internes testés + `builtin_denied`
  partagé + helpers de test partagés) → session dédiée (ROADMAP §9 ter).
- **`pkg.install`** : stub conservé **volontairement** (ADR-016 — allowlist
  paquets/dépôts non tranchée sur OS immuable ; backend = Phase 4).

## 🚧 Blockers (précis)

- **Zed E2E Tier B** : nécessite (1) le **binaire natif du Claude Agent SDK**,
  (2) **Zed** (non installable headless en WSL) ou un client ACP maison. Le Tier A
  (lien extension↔vibed) est **déjà prouvé**. Détail : `BLOCKERS.md`.
- **Validation VM/matériel** (Phase 1) : boot ISO amd64+arm64, NVIDIA, `ollama
  run` hors-ligne, `bootc upgrade/rollback` — exigent une vraie machine.
- **Boot Phase 2.5** : TPM2 réel + egress live + auth abonnement E2E = machine bootée.
- **Merge des PR** : PR #11 (branche → main) est **MERGEABLE + CI Rust verte** ;
  reste la revue + le merge humains (je ne merge jamais). **PR #4 ne se ferme PAS
  automatiquement** (même branche source mais base `phase2-supply-chain` ≠ `main`,
  `deleteBranchOnMerge=false`) → fermeture manuelle après #11.

## ➡️ Prochaine étape recommandée

1. **Merger PR #11 → main** (CI Rust verte ; laisser finir le build image ~15 min),
   puis **fermer manuellement PR #4** (superseded).
2. **Zed E2E Tier B** : sur une machine avec Zed, lancer `zed/vibeos-claude-acp/
   scripts/e2e-zed.sh` tel quel (Tier A auto déjà vert, puis la checklist éditeur).
3. **Activer l'expédition** de l'extension (`WITH_ZED_AGENT=1`) une fois le Tier B ok.
4. **Brancher l'agent-runner** sur une vraie machine : sceller un jeton
   (`vibeos-agent-seal-token.sh`), écrire `agent.d/<user>.conf`, `systemctl enable
   --now vibeos-agent@<user>` — vérifier unseal TPM2 + egress.
5. **F6 — extraire `fs`** (dernière famille) en session dédiée ; `pkg.install`
   réel derrière approbation **une fois l'allowlist tranchée** (ADR-016). Puis
   Phase 3 (LUKS/TPM2, sandbox par outil).
