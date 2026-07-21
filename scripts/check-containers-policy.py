#!/usr/bin/env python3
"""check-containers-policy.py — la policy client REJETTE une image non signée.

POURQUOI

Depuis build-os.yml, chaque release de ghcr.io/micka420-collab/vibeos est
signée cosign KEYLESS — mais signer sans que PERSONNE ne vérifie est du
théâtre : rien côté client n'imposait la vérification. La policy STAGED
os/rootfs/usr/share/vibeos/containers/policy-strict.json (+
registries.d/vibeos.yaml) ferme cet écart côté mécanisme. STAGED, pas active :
c/image ne matche aujourd'hui l'identité fulcio que contre un SAN e-mail alors
que les certs keyless GitHub Actions portent un SAN URI — active, l'exigence
rejetterait AUSSI l'image authentique au `bootc upgrade` (activation = `cp`
documenté dans registries.d/vibeos.yaml, machine-gated, et via ce garde).
Or une policy est le pire candidat à la dérive silencieuse : affaiblie
en `insecureAcceptAnything`, un chemin d'ancre cassé, un scope renommé — tout
continue de « marcher » (les pulls passent), simplement sans plus rien
vérifier. Ce garde rend la propriété exécutable : si la policy devient
laxiste, la CI rougit.

CE QU'IL VÉRIFIE

  1. policy.json est du JSON strict (même validation que `python3 -m json.tool`) ;
  2. structure : `default` reste [insecureAcceptAnything] (le podman de
     l'utilisateur n'est pas cassé), et le repo ghcr.io/micka420-collab/vibeos
     exige `sigstoreSigned` keyless (fulcio oidcIssuer GitHub Actions + rekor,
     signedIdentity matchRepository) ;
  3. les ancres de confiance shippées (fulcio_v1.crt.pem, rekor.pub)
     correspondent À L'OCTET près aux cibles sigstore/root-signing épinglées
     ci-dessous ;
  4. registries.d/vibeos.yaml active use-sigstore-attachments pour
     ghcr.io/micka420-collab (sans quoi aucune signature cosign n'est jamais lue) ;
  5. PREUVE MÉCANIQUE DE REJET (skopeo) : une image OCI triviale NON SIGNÉE,
     construite ici même,
       a. PASSE sous la policy shippée telle quelle (contrôle positif : le
          `default` est bien resté permissif ET skopeo parse notre policy) ;
       b. est REJETÉE — « signature » dans l'erreur — quand l'exigence
          sigstoreSigned du repo vibeos, extraite VERBATIM de la policy
          shippée (seuls les chemins d'ancres sont réécrits vers os/rootfs),
          est appliquée en `default`. Exigence retirée ou affaiblie => la
          copie passe => rouge.

CE QU'IL NE VÉRIFIE PAS

L'effet au `bootc upgrade` réel sur une machine déployée (machine-gated), et
l'ACCEPTATION du chemin positif : c/image ne matche aujourd'hui l'identité
fulcio que contre un SAN e-mail, alors que les certificats keyless GitHub
Actions portent un SAN URI — même l'image signée légitime est donc rejetée
(fail-closed strict, jamais une acceptation à tort). Détail honnête dans
os/rootfs/etc/containers/registries.d/vibeos.yaml.

USAGE : python3 scripts/check-containers-policy.py     (0 = conforme)
        VIBEOS_SKIP_REJECT_TEST=1 saute l'étape 5 (machine sans skopeo) —
        la CI ne le positionne JAMAIS ; sans skopeo et sans ce flag : échec.
"""
import copy
import hashlib
import io
import json
import os
import pathlib
import shutil
import subprocess
import sys
import tarfile
import tempfile

ROOT = pathlib.Path(__file__).resolve().parent.parent
# STAGED, deliberately NOT /etc/containers/policy.json: c/image today matches
# the fulcio identity only against an e-mail SAN while GitHub Actions keyless
# certs carry a URI SAN, so an ACTIVE requirement would fail-close the genuine
# image's own `bootc upgrade` path (and muddy the Phase-1 machine validation).
# The strict policy ships under /usr/share/vibeos (immutable image content);
# activation is a documented, machine-gated `cp` (see registries.d/vibeos.yaml).
POLICY = ROOT / "os" / "rootfs" / "usr" / "share" / "vibeos" / "containers" / "policy-strict.json"
# The ACTIVE policy path must stay absent from os/rootfs: shipping an override
# of the containers-common default is a conscious decision that must come back
# through this guard (see check 0 below).
ACTIVE_POLICY = ROOT / "os" / "rootfs" / "etc" / "containers" / "policy.json"
REGISTRIES_D = ROOT / "os" / "rootfs" / "etc" / "containers" / "registries.d" / "vibeos.yaml"
ANCHOR_DIR = ROOT / "os" / "rootfs" / "etc" / "pki" / "containers"

REPO_SCOPE = "ghcr.io/micka420-collab/vibeos"
OIDC_ISSUER = "https://token.actions.githubusercontent.com"
SIGNER_WORKFLOW = "github.com/Micka420-collab/vibeos/.github/workflows/build-os.yml"

# Empreintes des cibles amont sigstore/root-signing (targets/fulcio_v1.crt.pem,
# targets/rekor.pub), relevées le 2026-07-21. Une ancre qui dérive de l'amont
# est soit une mise à jour délibérée (mettre à jour ICI, en revue), soit une
# altération — dans les deux cas ça doit rougir.
PINNED_ANCHORS = {
    "fulcio_v1.crt.pem": "f989aa23def87c549404eadba767768d2a3c8d6d30a8b793f9f518a8eafd2cf5",
    "rekor.pub": "dce5ef715502ec9f3cdfd11f8cc384b31a6141023d3e7595e9908a81cb6241bd",
}

errors = []


def fail(msg):
    errors.append(msg)


# ---------------------------------------------------------------- 1. + 2. ----
def load_policy():
    raw = POLICY.read_text(encoding="utf-8")
    try:
        return json.loads(raw)  # strict, comme `python3 -m json.tool`
    except json.JSONDecodeError as exc:
        fail(f"{POLICY.relative_to(ROOT)}: JSON invalide: {exc}")
        return None


def check_structure(policy):
    default = policy.get("default")
    if default != [{"type": "insecureAcceptAnything"}]:
        fail(
            "policy.json: `default` doit rester [{type: insecureAcceptAnything}] "
            f"(podman utilisateur intact) — trouvé {default!r}"
        )

    reqs = policy.get("transports", {}).get("docker", {}).get(REPO_SCOPE)
    if not isinstance(reqs, list) or not reqs:
        fail(f"policy.json: aucune exigence pour docker scope {REPO_SCOPE!r}")
        return None

    sig = [r for r in reqs if isinstance(r, dict) and r.get("type") == "sigstoreSigned"]
    if not sig:
        fail(f"policy.json: le scope {REPO_SCOPE!r} n'exige pas `sigstoreSigned`")
        return None
    lax = [r for r in reqs if isinstance(r, dict) and r.get("type") == "insecureAcceptAnything"]
    if lax:
        # Les exigences d'un scope sont un ET logique, mais un
        # insecureAcceptAnything qui se glisse ici est le signe exact de la
        # dérive que ce garde existe pour attraper.
        fail(f"policy.json: `insecureAcceptAnything` présent dans le scope {REPO_SCOPE!r}")

    req = sig[0]
    fulcio = req.get("fulcio")
    if not isinstance(fulcio, dict):
        fail("policy.json: exigence sigstoreSigned sans bloc `fulcio` (keyless requis)")
        return req
    if fulcio.get("oidcIssuer") != OIDC_ISSUER:
        fail(f"policy.json: fulcio.oidcIssuer doit être {OIDC_ISSUER!r} — trouvé {fulcio.get('oidcIssuer')!r}")
    subject = fulcio.get("subjectEmail", "")
    if SIGNER_WORKFLOW not in subject:
        fail(
            "policy.json: l'identité fulcio doit désigner le workflow signeur "
            f"{SIGNER_WORKFLOW!r} — trouvé {subject!r}"
        )
    if not req.get("rekorPublicKeyPath"):
        fail("policy.json: `rekorPublicKeyPath` manquant (keyless = fulcio + rekor)")
    if req.get("signedIdentity") != {"type": "matchRepository"}:
        fail(
            "policy.json: signedIdentity doit être matchRepository (les payloads "
            f"cosign ne portent pas de tag) — trouvé {req.get('signedIdentity')!r}"
        )
    return req


# --------------------------------------------------------------------- 3. ----
def check_anchors(req):
    for name, want in PINNED_ANCHORS.items():
        path = ANCHOR_DIR / name
        if not path.is_file():
            fail(f"ancre de confiance absente: {path.relative_to(ROOT)}")
            continue
        got = hashlib.sha256(path.read_bytes()).hexdigest()
        if got != want:
            fail(
                f"{path.relative_to(ROOT)}: sha256 {got} != épinglé {want} "
                "(mise à jour amont ? mettre à jour PINNED_ANCHORS en revue)"
            )
    if req:
        # Les chemins pointés par la policy doivent être ceux que l'image
        # shippe réellement (os/rootfs/<chemin> existe).
        for key in ("rekorPublicKeyPath",):
            paths = [req.get(key)]
            for p in paths:
                if p and not (ROOT / "os" / "rootfs" / p.lstrip("/")).is_file():
                    fail(f"policy.json: {key}={p!r} n'existe pas sous os/rootfs/")
        ca = req.get("fulcio", {}).get("caPath")
        if ca and not (ROOT / "os" / "rootfs" / ca.lstrip("/")).is_file():
            fail(f"policy.json: fulcio.caPath={ca!r} n'existe pas sous os/rootfs/")


# --------------------------------------------------------------------- 4. ----
def check_registries_d():
    if not REGISTRIES_D.is_file():
        fail(f"{REGISTRIES_D.relative_to(ROOT)} absent — aucune signature cosign ne serait lue")
        return
    text = REGISTRIES_D.read_text(encoding="utf-8")
    try:
        import yaml  # PyYAML: présent sur ubuntu-latest; repli textuel sinon

        data = yaml.safe_load(text)
        ns = (data or {}).get("docker", {}).get("ghcr.io/micka420-collab", {})
        ok = ns.get("use-sigstore-attachments") is True
    except ImportError:
        lines = [ln.strip() for ln in text.splitlines() if not ln.lstrip().startswith("#")]
        ok = "ghcr.io/micka420-collab:" in lines and "use-sigstore-attachments: true" in lines
    if not ok:
        fail(
            f"{REGISTRIES_D.relative_to(ROOT)}: `use-sigstore-attachments: true` "
            "requis pour le namespace ghcr.io/micka420-collab"
        )


# --------------------------------------------------------------------- 5. ----
def build_unsigned_oci(layout: pathlib.Path, ref: str):
    """Écrit un layout OCI minimal, valide et NON SIGNÉ, à la main.

    À la main plutôt que via podman/buildah : seul skopeo est alors requis, et
    le contrôle positif (5a) prouve que le layout est bien formé — un layout
    cassé ne peut pas faire passer 5b pour un rejet de signature.
    """
    blobs = layout / "blobs" / "sha256"
    blobs.mkdir(parents=True)

    def put(data: bytes) -> tuple[str, int]:
        digest = hashlib.sha256(data).hexdigest()
        (blobs / digest).write_bytes(data)
        return digest, len(data)

    buf = io.BytesIO()
    with tarfile.open(fileobj=buf, mode="w") as tar:
        payload = b"unsigned test image for scripts/check-containers-policy.py\n"
        info = tarfile.TarInfo("unsigned.txt")
        info.size = len(payload)
        info.mtime = 0
        tar.addfile(info, io.BytesIO(payload))
    layer = buf.getvalue()
    layer_digest, layer_size = put(layer)

    config = json.dumps(
        {
            "architecture": "amd64",
            "os": "linux",
            "config": {},
            "rootfs": {"type": "layers", "diff_ids": [f"sha256:{layer_digest}"]},
        }
    ).encode()
    config_digest, config_size = put(config)

    manifest = json.dumps(
        {
            "schemaVersion": 2,
            "mediaType": "application/vnd.oci.image.manifest.v1+json",
            "config": {
                "mediaType": "application/vnd.oci.image.config.v1+json",
                "digest": f"sha256:{config_digest}",
                "size": config_size,
            },
            "layers": [
                {
                    "mediaType": "application/vnd.oci.image.layer.v1.tar",
                    "digest": f"sha256:{layer_digest}",
                    "size": layer_size,
                }
            ],
        }
    ).encode()
    manifest_digest, manifest_size = put(manifest)

    (layout / "oci-layout").write_text('{"imageLayoutVersion": "1.0.0"}\n')
    (layout / "index.json").write_text(
        json.dumps(
            {
                "schemaVersion": 2,
                "manifests": [
                    {
                        "mediaType": "application/vnd.oci.image.manifest.v1+json",
                        "digest": f"sha256:{manifest_digest}",
                        "size": manifest_size,
                        "annotations": {"org.opencontainers.image.ref.name": ref},
                    }
                ],
            }
        )
    )


def run_skopeo(policy_path, src, dst):
    return subprocess.run(
        ["skopeo", "copy", "--policy", str(policy_path), src, dst],
        capture_output=True,
        text=True,
        check=False,
    )


def check_rejection(policy, req):
    if shutil.which("skopeo") is None:
        if os.environ.get("VIBEOS_SKIP_REJECT_TEST") == "1":
            print("[SKIP] preuve de rejet sautée (VIBEOS_SKIP_REJECT_TEST=1, pas de skopeo)")
            return
        fail(
            "skopeo introuvable : la preuve mécanique de rejet ne peut pas tourner. "
            "Installe skopeo, ou VIBEOS_SKIP_REJECT_TEST=1 hors CI."
        )
        return

    with tempfile.TemporaryDirectory(prefix="vibeos-policy-") as tmp:
        tmpdir = pathlib.Path(tmp)
        layout = tmpdir / "unsigned-oci"
        build_unsigned_oci(layout, "unsigned-test")
        src = f"oci:{layout}:unsigned-test"

        # 5a. Contrôle positif : la policy SHIPPÉE, telle quelle. La source est
        # un transport `oci:` => c'est le `default` qui s'applique ; s'il est
        # resté permissif (et si notre layout est bien formé), la copie passe.
        res = run_skopeo(POLICY, src, f"dir:{tmpdir / 'accepted'}")
        if res.returncode != 0:
            fail(
                "contrôle positif: skopeo a refusé l'image de test sous la policy "
                f"shippée (default non permissif, ou layout/policy cassés) :\n{res.stderr.strip()}"
            )
            return

        # 5b. Rejet : la MÊME exigence sigstoreSigned que le repo vibeos,
        # extraite verbatim de la policy shippée, appliquée en `default` (les
        # chemins d'ancres sont réécrits vers les fichiers shippés dans
        # os/rootfs — seuls les chemins changent, jamais l'exigence).
        strict_req = copy.deepcopy(req)
        strict_req["fulcio"]["caPath"] = str(ANCHOR_DIR / "fulcio_v1.crt.pem")
        strict_req["rekorPublicKeyPath"] = str(ANCHOR_DIR / "rekor.pub")
        strict_policy = tmpdir / "strict-policy.json"
        strict_policy.write_text(json.dumps({"default": [strict_req]}))

        res = run_skopeo(strict_policy, src, f"dir:{tmpdir / 'rejected'}")
        if res.returncode == 0:
            fail(
                "PREUVE DE REJET EN ÉCHEC : une image NON SIGNÉE passe sous "
                "l'exigence sigstoreSigned du repo vibeos — la policy est laxiste."
            )
            return
        if "signature" not in res.stderr.lower():
            fail(
                "l'image non signée est refusée, mais pas pour une raison de "
                f"signature — exigence probablement mal formée :\n{res.stderr.strip()}"
            )
            return
        print("[OK] image non signée acceptée par le default, REJETÉE par l'exigence vibeos")
        print(f"     ({res.stderr.strip().splitlines()[-1]})")


def main():
    # Check 0 — the strict policy stays STAGED: os/rootfs must not override the
    # active /etc/containers/policy.json. Activating it (a conscious, machine-
    # gated decision once c/image can match URI SAN identities) must update
    # this guard, so the flip never happens as a silent side effect.
    if ACTIVE_POLICY.exists():
        fail(
            f"{ACTIVE_POLICY.relative_to(ROOT)} présent : la policy stricte doit "
            "rester STAGED (policy-strict.json) tant que l'acceptation de l'image "
            "authentique n'est pas validée sur machine (SAN URI c/image)"
        )
    policy = load_policy()
    req = None
    if policy is not None:
        req = check_structure(policy)
    check_anchors(req)
    check_registries_d()
    if policy is not None and req is not None and not errors:
        check_rejection(policy, req)

    if errors:
        print("[FAIL] policy de signature côté client :")
        for e in errors:
            print(f"   - {e}")
        return 1
    print("[OK] policy-strict.json (staged) + registries.d + ancres épinglées + preuve de rejet")
    return 0


if __name__ == "__main__":
    sys.exit(main())
