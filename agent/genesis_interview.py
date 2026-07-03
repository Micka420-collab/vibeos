#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""VibeOS Genesis interview (Phase 3 prototype — NOT wired in v0.1).

First-boot interview that bootstraps the USER side of the machine's memory:
VibeOS ships blank, and this script creates the initial user profile
(identity of the human, languages, code style, current projects).

Status: prototype for ROADMAP Phase 3 ("Genesis & memoire"). In v0.1 it is
NOT invoked by memory/genesis.sh nor by vibeos-genesis.service — nothing
calls it at first boot. It is only run by hand for testing:
    python3 agent/genesis_interview.py /tmp/vibeos-memory-test

Layout contract (docs/MEMORY.md is the reference). This script writes ONLY:
    MEMORY_DIR/user/profile.toml
    MEMORY_DIR/user/preferences.toml
    MEMORY_DIR/projects/index.json
and appends ONE JSONL line to MEMORY_DIR/journal/<YYYY-MM-DD>.jsonl.
It must NEVER touch identity.toml or hardware.json: those describe the
MACHINE identity and are written once by memory/genesis.sh.

Constraints: Python 3 stdlib only ('anthropic' is OPTIONAL, --with-claude);
offline by default; idempotent (previous answers are re-read from the output
files themselves and offered as defaults); atomic writes (temp file +
os.replace); TTY-safe (falls back to defaults when stdin is not a TTY).
"""

import argparse, json, os, sys, tempfile  # noqa: E401 (compact prototype)
from datetime import datetime, timezone

MAX_PROJECTS = 10

# Files this script is allowed to produce. identity.toml / hardware.json are
# machine identity (genesis.sh territory) and must never appear here.
PROFILE_FILE = os.path.join("user", "profile.toml")
PREFERENCES_FILE = os.path.join("user", "preferences.toml")
PROJECTS_FILE = os.path.join("projects", "index.json")
JOURNAL_DIR = "journal"

DEFAULTS = {
    "profile": {"name": "", "handle": "", "email": "", "timezone": ""},
    "languages": {
        "spoken": ["français", "anglais"],
        "programming": ["python", "rust", "typescript"],
    },
    "code_style": {
        "indent": "4 spaces",
        "max_line_length": 100,
        "comment_language": "english",
        "commit_convention": "conventional-commits",
    },
    "projects": [],
}


def eprint(*args):
    print(*args, file=sys.stderr)


def now_iso():
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def is_interactive():
    return sys.stdin.isatty() and sys.stdout.isatty()


def detect_timezone():
    try:
        with open("/etc/timezone", encoding="utf-8") as fh:
            tz = fh.read().strip()
            if tz:
                return tz
    except OSError:
        pass
    return os.environ.get("TZ", "Europe/Paris")


def ask(label, default="", interactive=True):
    """One question; empty answer keeps the default. Never raises on EOF."""
    if not interactive:
        return default
    suffix = " [{}]".format(default) if default else ""
    try:
        answer = input("{}{} : ".format(label, suffix)).strip()
    except EOFError:
        return default
    return answer or default


def ask_list(label, default_items, interactive=True):
    raw = ask(label + " (séparés par des virgules)", ", ".join(default_items), interactive)
    return [item.strip() for item in raw.split(",") if item.strip()]


def to_int(value, fallback):
    try:
        return int(str(value).strip())
    except (TypeError, ValueError):
        return fallback


# Minimal TOML rendering: flat sections of strings/ints/bools/lists.
def toml_value(value):
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, (int, float)):
        return str(value)
    if isinstance(value, list):
        return "[" + ", ".join(toml_value(item) for item in value) + "]"
    escaped = str(value).replace("\\", "\\\\").replace('"', '\\"')
    return '"{}"'.format(escaped)


def render_toml(sections):
    lines = []
    for section, table in sections.items():
        lines.append("[{}]".format(section))
        for key, value in table.items():
            lines.append("{} = {}".format(key, toml_value(value)))
        lines.append("")
    return "\n".join(lines)


def atomic_write(path, content):
    fd, tmp = tempfile.mkstemp(dir=os.path.dirname(path) or ".", prefix=".genesis-")
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as fh:
            fh.write(content)
        os.replace(tmp, path)
    finally:
        if os.path.exists(tmp):
            os.unlink(tmp)


def load_toml_file(path):
    """Best-effort TOML read for idempotence. tomllib is stdlib (3.11+);
    on older interpreters we simply degrade to the built-in defaults."""
    try:
        import tomllib
    except ImportError:
        return {}
    try:
        with open(path, "rb") as fh:
            return tomllib.load(fh)
    except FileNotFoundError:
        return {}
    except (OSError, ValueError) as exc:
        eprint("[genesis] {} illisible ({}), on repart des défauts".format(path, exc))
        return {}


def load_json_file(path):
    try:
        with open(path, encoding="utf-8") as fh:
            data = json.load(fh)
        return data if isinstance(data, dict) else {}
    except FileNotFoundError:
        return {}
    except (json.JSONDecodeError, OSError) as exc:
        eprint("[genesis] {} illisible ({}), on repart des défauts".format(path, exc))
        return {}


def load_previous(memory_dir):
    """Rebuild the previous interview state from the output files themselves
    (no separate state file: the produced layout IS the canonical state)."""
    previous = {}
    profile_doc = load_toml_file(os.path.join(memory_dir, PROFILE_FILE))
    if isinstance(profile_doc.get("profile"), dict):
        previous["profile"] = profile_doc["profile"]
    meta = profile_doc.get("meta")
    if isinstance(meta, dict) and meta.get("created_at"):
        previous["created_at"] = meta["created_at"]

    prefs_doc = load_toml_file(os.path.join(memory_dir, PREFERENCES_FILE))
    if isinstance(prefs_doc.get("languages"), dict):
        previous["languages"] = prefs_doc["languages"]
    if isinstance(prefs_doc.get("code_style"), dict):
        previous["code_style"] = prefs_doc["code_style"]

    index_doc = load_json_file(os.path.join(memory_dir, PROJECTS_FILE))
    if isinstance(index_doc.get("projects"), list):
        previous["projects"] = index_doc["projects"]
    return previous


def deep_merge(base, override):
    merged = dict(base)
    for key, value in override.items():
        if isinstance(value, dict) and isinstance(merged.get(key), dict):
            merged[key] = deep_merge(merged[key], value)
        else:
            merged[key] = value
    return merged


def interview_profile(prev, interactive):
    if interactive:
        print("\n--- Profil ---")
    return {
        "name": ask("Votre nom (ou pseudonyme public)", prev.get("name", ""), interactive),
        "handle": ask("Votre pseudo court (login, git)", prev.get("handle", ""), interactive),
        "email": ask("Votre e-mail (git, licences)", prev.get("email", ""), interactive),
        "timezone": ask("Votre fuseau horaire", prev.get("timezone") or detect_timezone(), interactive),
    }


def interview_projects(prev, interactive):
    if not interactive:
        return prev
    print("\n--- Projets en cours (nom vide pour terminer) ---")
    projects = []
    if prev:
        keep = ask("{} projet(s) déjà en mémoire, les conserver ? (o/n)".format(len(prev)), "o", True)
        if keep.lower().startswith("o"):
            projects.extend(prev)
    for index in range(len(projects), MAX_PROJECTS):
        name = ask("Projet #{} — nom".format(index + 1), "", True)
        if not name:
            break
        projects.append({
            "name": name,
            "path": ask("  chemin (ex: ~/dev/mon-projet)", "", True),
            "description": ask("  description en une ligne", "", True),
            "stack": ask_list("  stack technique", [], True),
        })
    return projects


def claude_enrich(state, interactive):
    """Optional: one Claude-generated follow-up question. Never breaks Genesis."""
    try:
        import anthropic  # optional dependency, present only if installed
    except ImportError:
        eprint("[genesis] paquet 'anthropic' absent : enrichissement Claude désactivé")
        return None
    if not os.environ.get("ANTHROPIC_API_KEY"):
        eprint("[genesis] ANTHROPIC_API_KEY absente : enrichissement Claude désactivé")
        return None
    try:
        client = anthropic.Anthropic()
        summary = json.dumps(
            {"languages": state["languages"], "code_style": state["code_style"]},
            ensure_ascii=False)
        message = client.messages.create(
            model=os.environ.get("VIBEOS_GENESIS_MODEL", "claude-sonnet-4-5"),
            max_tokens=200,
            messages=[{"role": "user", "content": (
                "Voici le profil d'un développeur qui initialise son OS : " + summary
                + "\nPose UNE seule question courte en français, utile pour "
                "personnaliser son assistant de code. Réponds uniquement par la question.")}])
        question = "".join(
            block.text for block in message.content if getattr(block, "type", "") == "text"
        ).strip()
    except Exception as exc:  # network, auth, quota: fall back silently
        eprint("[genesis] enrichissement Claude indisponible ({})".format(exc))
        return None
    if not question:
        return None
    answer = ask("\n[Claude] " + question, "", interactive)
    if not answer:
        return None
    return {"question": question, "answer": answer, "at": now_iso()}


def write_outputs(memory_dir, state, enrichment):
    """Write ONLY the user-profile subtree + one journal line.

    Never writes identity.toml / hardware.json (machine identity, owned by
    memory/genesis.sh)."""
    ts = now_iso()
    meta = {
        "schema": 1,
        "created_at": state.get("created_at", ts),
        "updated_at": ts,
        "source": "genesis_interview.py",
    }
    for sub in ("user", "projects", JOURNAL_DIR):
        os.makedirs(os.path.join(memory_dir, sub), exist_ok=True)

    atomic_write(os.path.join(memory_dir, PROFILE_FILE),
                 render_toml({"profile": state["profile"], "meta": meta}))
    atomic_write(os.path.join(memory_dir, PREFERENCES_FILE),
                 render_toml({
                     "languages": state["languages"],
                     "code_style": state["code_style"],
                     "assistant": {"reply_language": "français"},
                     "meta": meta,
                 }))
    atomic_write(os.path.join(memory_dir, PROJECTS_FILE),
                 json.dumps({"schema": 1, "updated_at": ts, "projects": state["projects"]},
                            ensure_ascii=False, indent=2) + "\n")

    # Exactly ONE journal line per run (append-only, day-sharded JSONL).
    entry = {
        "type": "genesis_interview",
        "at": ts,
        "source": "genesis_interview.py",
        "files": [PROFILE_FILE.replace(os.sep, "/"),
                  PREFERENCES_FILE.replace(os.sep, "/"),
                  PROJECTS_FILE.replace(os.sep, "/")],
        "projects_count": len(state["projects"]),
    }
    if enrichment:
        entry["enrichment"] = enrichment
    journal_path = os.path.join(memory_dir, JOURNAL_DIR, ts[:10] + ".jsonl")
    with open(journal_path, "a", encoding="utf-8") as fh:
        fh.write(json.dumps(entry, ensure_ascii=False) + "\n")


def main(argv=None):
    parser = argparse.ArgumentParser(
        description="VibeOS Genesis : interview du premier démarrage "
                    "(prototype Phase 3 — non câblé dans genesis.sh en v0.1)")
    parser.add_argument("memory_dir",
                        help="Répertoire mémoire cible, ex: /tmp/vibeos-memory-test")
    parser.add_argument("--offline", action="store_true", default=True,
                        help="Mode hors-ligne (défaut ; seul --with-claude sort du mode)")
    parser.add_argument("--with-claude", dest="with_claude", action="store_true",
                        help="Enrichit l'interview via l'API Anthropic si le paquet "
                             "'anthropic' est importable (repli propre sinon)")
    parser.add_argument("--non-interactive", action="store_true",
                        help="Aucune question : réutilise l'état précédent ou les défauts")
    args = parser.parse_args(argv)

    memory_dir = os.path.abspath(args.memory_dir)
    try:
        os.makedirs(memory_dir, exist_ok=True)
    except OSError as exc:
        eprint("[genesis] impossible de créer {} : {}".format(memory_dir, exc))
        return 1

    interactive = is_interactive() and not args.non_interactive
    previous = load_previous(memory_dir)
    state = deep_merge(DEFAULTS, previous)

    if interactive:
        print("=== VibeOS Genesis — profil utilisateur de cette machine ===")
        print("(Entrée pour accepter la valeur entre crochets ; relançable sans risque)")

    state["profile"] = interview_profile(state["profile"], interactive)

    if interactive:
        print("\n--- Langues ---")
    state["languages"] = {
        "spoken": ask_list("Langues parlées", state["languages"]["spoken"], interactive),
        "programming": ask_list("Langages de programmation préférés",
                                state["languages"]["programming"], interactive),
    }

    if interactive:
        print("\n--- Style de code ---")
    style = state["code_style"]
    style["indent"] = ask("Indentation", style["indent"], interactive)
    style["max_line_length"] = to_int(
        ask("Longueur de ligne max", str(style["max_line_length"]), interactive), 100)
    style["comment_language"] = ask("Langue des commentaires de code",
                                    style["comment_language"], interactive)
    style["commit_convention"] = ask("Convention de commit",
                                     style["commit_convention"], interactive)

    state["projects"] = interview_projects(state.get("projects", []), interactive)

    enrichment = None
    if args.with_claude:
        enrichment = claude_enrich(state, interactive)

    state["created_at"] = previous.get("created_at", now_iso())
    write_outputs(memory_dir, state, enrichment)
    print("[genesis] profil utilisateur écrit dans {} (user/profile.toml, "
          "user/preferences.toml, projects/index.json + 1 ligne de journal)".format(memory_dir))
    return 0


if __name__ == "__main__":
    sys.exit(main())
