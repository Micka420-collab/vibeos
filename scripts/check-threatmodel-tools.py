#!/usr/bin/env python3
"""check-threatmodel-tools.py — chaque outil MCP exposé a sa ligne THREAT-MODEL.

POURQUOI

docs/THREAT-MODEL.md §6 (« menace → mitigation → phase ») énonce la règle :
« chaque nouvel outil MCP exposé par vibed doit ajouter sa ligne au tableau §6
avant merge ». Cette règle était RÉELLE mais NON EXÉCUTÉE — et elle a dérivé :
la moitié du catalogue (os.status, memory.query, user.model, agent.*,
policy.*, sandbox.probe, deploy.plan, browser.run…) n'avait aucune ligne, la
justification de confinement de chaque outil vivant seulement dans les
commentaires du code, pas dans le document de surface de sécurité.

Une règle de sécurité documentée mais non gardée re-dérive au premier outil
suivant. Ce garde la rend EXÉCUTABLE : ajouter une entrée au catalogue
`build_tool_catalog` sans sa ligne §6 fait ROUGIR la CI, avec le nom de
l'outil manquant.

CE QU'IL VÉRIFIE

  Tout outil exposé par le catalogue MCP (`build_tool_catalog` dans
  vibed/src/mcp.rs — la source de vérité de `tools/list`) est MENTIONNÉ, entre
  backticks, dans docs/THREAT-MODEL.md.

L'extraction du catalogue est robuste aux commentaires entre le nom et le
`Tier::` (cf. browser.run) : un nom d'outil est un littéral `"famille.verbe",`
SEUL sur sa ligne — la 1re position d'un tuple du catalogue. Les chaînes de
schéma (`"type": "object"`), les enums (`"fly"`, `"browser"`) et les noms de
fichiers dans les descriptions (`identity.toml`, `memory/reasoning/x.jsonl`)
ne matchent pas (pas de forme `"x.y",` seule sur sa ligne).

CE QU'IL NE VÉRIFIE PAS

Que la LIGNE décrit correctement la menace/mitigation — ça reste un jugement
humain. Il garantit la présence, pas la justesse. C'est l'essentiel : un outil
gouverné absent du modèle de menace est le trou silencieux que ce garde ferme.

USAGE : python3 scripts/check-threatmodel-tools.py   (0 = tous couverts)
"""
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
MCP = ROOT / "vibed" / "src" / "mcp.rs"
THREAT_MODEL = ROOT / "docs" / "THREAT-MODEL.md"


def die(msg):
    print(f"[FAIL] {msg}", file=sys.stderr)
    sys.exit(1)


def catalog_tools(src: str) -> list[str]:
    """Noms d'outils exposés par build_tool_catalog, dans l'ordre du source."""
    idx = src.find("fn build_tool_catalog")
    if idx < 0:
        die("build_tool_catalog introuvable dans mcp.rs — le catalogue a bougé ?")
    rest = src[idx + len("fn build_tool_catalog") :]
    # Borne la fenêtre à la fonction : la prochaine déclaration `fn` au niveau module.
    nxt = re.search(r"\n(?:pub(?:\(crate\))? )?fn \w", rest)
    body = rest[: nxt.start()] if nxt else rest
    seen, tools = set(), []
    for name in re.findall(r'^\s*"([a-z_]+\.[a-z_]+)",\s*$', body, re.M):
        if name not in seen:
            seen.add(name)
            tools.append(name)
    return tools


def main() -> int:
    if not MCP.is_file():
        die(f"{MCP.relative_to(ROOT)} introuvable")
    if not THREAT_MODEL.is_file():
        die(f"{THREAT_MODEL.relative_to(ROOT)} introuvable")

    tools = catalog_tools(MCP.read_text(encoding="utf-8"))
    if not tools:
        die(
            "aucun outil extrait du catalogue — soit le format des tuples a changé, "
            "soit ce garde est devenu aveugle. Répare l'extraction, ne le supprime pas."
        )

    tm = THREAT_MODEL.read_text(encoding="utf-8")
    missing = [t for t in tools if f"`{t}`" not in tm]

    if missing:
        print("[FAIL] outils MCP exposés SANS ligne dans docs/THREAT-MODEL.md :")
        for t in missing:
            print(f"   - {t}")
        print(
            "\nRègle (THREAT-MODEL.md §6) : chaque outil du catalogue doit ajouter "
            "sa ligne au tableau §6 avant merge. Ajoute une ligne "
            "`| Sx | `<outil>` : <mitigation/confinement> | Phase … |`."
        )
        return 1

    print(f"[OK] les {len(tools)} outils du catalogue MCP ont tous leur ligne THREAT-MODEL.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
