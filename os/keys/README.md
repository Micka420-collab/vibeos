# Clés de signature des dépôts tiers (épinglées)

Ces clés publiques OpenPGP sont **livrées par le dépôt**, pas récupérées au build.
`os/Containerfile` les copie dans `/etc/pki/rpm-gpg/` et les importe **depuis le
disque** ; les fichiers `.repo` écrits dans l'image pointent leur `gpgkey=` sur
ces chemins locaux, jamais sur une URL.

## Pourquoi

`rpm --import <url>` fait confiance à **n'importe quelle clé** servie par cette
URL au moment du build, et `dnf -y` accepte silencieusement une clé proposée par
un `.repo` amont. Une fois cette clé importée, `gpgcheck=1` la valide fidèlement :
la vérification devient un **théâtre**. Un build ultérieur récupérant une URL
compromise (ou victime d'un MITM, ou d'un compte amont perdu) aurait importé une
clé hostile **sans aucun signal**.

Épingler dans le dépôt déplace la confiance vers quelque chose de relisible :
tout changement de clé devient un **diff en revue humaine**, pas un fetch
silencieux. Les clés sont ASCII-armored exprès — un diff binaire ne se relit pas.

## Ce que cet épinglage garantit — et ce qu'il ne garantit PAS

- ✅ **Garantit** : la clé ne peut plus changer entre deux builds sans qu'un
  humain le voie. C'est la propriété qui manquait.
- ❌ **Ne garantit PAS** que la clé était authentique au moment de la capture.
  C'est un **TOFU** (*trust on first use*) capturé et figé, pas une preuve
  d'identité. Il n'existe pas de chaîne de confiance vers ces éditeurs.

Pour renforcer, vérifier les empreintes ci-dessous hors-bande (canal indépendant
de ce dépôt et des URLs ci-dessus) et le confirmer en revue.

## Inventaire

| Fichier | Empreinte de la clé primaire | UID | Source |
|---|---|---|---|
| `RPM-GPG-KEY-vscodium` | `1302DE60231889FE1EBACADC54678CF75A278D9C` | `Pavlo Rudyi <paulcarroty@riseup.net>` | `https://gitlab.com/paulcarroty/vscodium-deb-rpm-repo/-/raw/master/pub.gpg` |
| `RPM-GPG-KEY-mise` | `24853EC9F655CE80B48E6C3A8B81C9D17413A06D` | `mise releases <release@mise.jdx.dev>` | `https://mise.jdx.dev/gpg-key.pub` |

## Vérification effectuée à la capture (2026-07-15)

Chaque clé a été confrontée à la **signature réelle des métadonnées du dépôt**
qu'elle est censée signer (`repomd.xml.asc`) — pas seulement téléchargée :

- **VSCodium** — recoupement **inter-hôtes**, le plus solide des deux : la clé est
  servie par `gitlab.com`, les paquets par `download.vscodium.com`. La signature
  de `https://download.vscodium.com/rpms/repodata/repomd.xml.asc` est
  `Good signature from "Pavlo Rudyi <paulcarroty@riseup.net>"`, émise par
  `1302DE60231889FE1EBACADC54678CF75A278D9C`. Compromettre l'épinglage à la
  capture aurait exigé de contrôler **les deux hôtes**.
- **mise** — recoupement **plus faible, à assumer** : la clé *et* les paquets sont
  servis par le même hôte (`mise.jdx.dev`), donc un hôte compromis aurait pu
  fournir un couple clé/signature cohérent. La signature de
  `https://mise.jdx.dev/rpm/repodata/repomd.xml.asc` est
  `Good signature from "mise releases <release@mise.jdx.dev>"`, émise par
  `24853EC9F655CE80B48E6C3A8B81C9D17413A06D`.

Reproduire la vérification :

```sh
f=os/keys/RPM-GPG-KEY-vscodium
gpg --show-keys --with-colons --fingerprint "$f" | awk -F: '$1=="fpr"{print $10; exit}'
# -> 1302DE60231889FE1EBACADC54678CF75A278D9C

d="$(mktemp -d)"; gpg --homedir "$d" --import "$f"
curl -fsSLO https://download.vscodium.com/rpms/repodata/repomd.xml
curl -fsSLO https://download.vscodium.com/rpms/repodata/repomd.xml.asc
gpg --homedir "$d" --verify repomd.xml.asc repomd.xml
```

## ⚠️ Piège : `gpgkey=file://` + dépôt ACTIVÉ casse la construction de l'ISO

Découvert le 2026-07-15, au tag `v0.2.0-dev` : les deux jobs ISO ont échoué sur

```
Curl error (37): Could not read a file:// file for
file:///etc/pki/rpm-gpg/RPM-GPG-KEY-mise
error: cannot build manifest: cannot depsolve
```

**Pourquoi.** `bootc-image-builder` fait un *depsolve* en lisant les `.repo` de
l'image — mais **depuis son propre conteneur**, où `/etc/pki/rpm-gpg/…` n'existe
pas. La clé est bien dans l'image ; elle n'est pas là où dnf la cherche.

**Correctif** (déjà appliqué) : les dépôts vendeurs sont écrits **`enabled=0`** et
activés seulement pour leur transaction (`--enablerepo=`), exactement comme les
COPR. Un dépôt désactivé n'est jamais résolu → le depsolve l'ignore → l'ISO se
construit, et l'image déployée est **plus stricte** qu'avant. **Ne pas les
réactiver.**

**Pourquoi la CI ne l'a pas vu.** Sur une PR, `build-os` ne fait qu'un **build de
vérification amd64** : le job ISO ne tourne **qu'en release** (tag `v*`). Une
régression d'ISO passe donc la CI au vert et ne sort qu'au moment du tag. À garder
en tête pour toute modification de `os/Containerfile` touchant les dépôts.

## Rotation

Un éditeur qui tourne sa clé **casse le build, bruyamment** — c'est le
comportement voulu : `dnf` refusera des paquets signés par une clé inconnue.
Procédure : récupérer la nouvelle clé, refaire la vérification ci-dessus,
remplacer le fichier, mettre à jour l'empreinte dans ce tableau **et** dans
`os/Containerfile`, et faire relire le diff.
