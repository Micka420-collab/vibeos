#!/usr/bin/env python3
"""check-log-hygiene.py — no secret may ever reach vibed's logs.

WHY THIS EXISTS
---------------
ROADMAP Phase 2, critère de sortie : « Aucune fuite d'informations sensibles
(tokens, contenus de fichiers) dans les logs de niveau info. »

That criterion held on 2026-07-15 — verified by reading every single tracing call
in `vibed/src`. But it held **by discipline alone**: nothing stopped the next
`info!("call {name} args={args}")` from shipping tool arguments (which carry file
paths, package names, and whatever an agent chose to send) straight into the
journal. A criterion that is true only until someone forgets is not a criterion,
it is a coincidence. This turns it into a check.

WHAT IT ENFORCES
----------------
A tracing macro may never INTERPOLATE a value that can carry caller-controlled
content or a secret. It is the *interpolated values* that are checked, not the
message text: `warn!("oversized JSON-RPC line (> {MAX_LINE_BYTES} bytes)")` is
fine — it mentions "line" but interpolates a constant.

Allowed today, by design: the caller's identity (uid/gid/pid), tool NAMES, unit
NAMES, counts, socket/policy paths, and error values. The subject of a call
belongs in the AUDIT trail (`audit_target`, deliberately bounded and non-secret),
never in the log.

Usage: python3 scripts/check-log-hygiene.py   (0 = clean)
"""
import pathlib
import re
import sys

try:
    sys.stdout.reconfigure(encoding="utf-8")
except Exception:
    pass

# Identifiers that carry caller-controlled content or secrets. Substring match on
# the interpolated expression, so `args`, `&args`, `req.args` all trip it.
FORBIDDEN = (
    "args",       # tool arguments: paths, package names, anything the agent sends
    "argument",
    "content",    # file content
    "body",
    "payload",
    "token",
    "secret",
    "password",
    "passwd",
    "credential",
    "block",      # captured reasoning blocks
    "snippet",
    "stdout",     # raw command output (journalctl, systemctl…)
    "stderr",
)

# Values that LOOK forbidden but are provably safe, with the reason. Keep this
# list tiny and justified — it is the escape hatch, and every entry is a promise.
ALLOWED_EXACT = {
    # (none today: no tracing call in vibed interpolates any of the above)
}

MACRO_RE = re.compile(r"\b(?:info|warn|error|debug|trace)!\s*\(", re.MULTILINE)

root = pathlib.Path(".")
failures = []
checked = 0

for path in sorted(root.glob("vibed/src/**/*.rs")):
    src = path.read_text(encoding="utf-8")
    for m in MACRO_RE.finditer(src):
        # Grab the balanced macro call, so multi-line calls are seen whole.
        i = m.end() - 1
        depth = 0
        for j in range(i, min(len(src), i + 4000)):
            if src[j] == "(":
                depth += 1
            elif src[j] == ")":
                depth -= 1
                if depth == 0:
                    call = src[i : j + 1]
                    break
        else:
            continue
        checked += 1
        line_no = src[: m.start()].count("\n") + 1

        # Everything interpolated: the `{name}` inline captures, plus the
        # positional arguments after the format string.
        interpolated = set(re.findall(r"\{([a-zA-Z_][\w.]*)[:?}]", call))
        first_quote = call.find('"')
        if first_quote != -1:
            # crude but sufficient: args after the closing quote of the format str
            end_quote = first_quote
            k = first_quote + 1
            while k < len(call):
                if call[k] == '"' and call[k - 1] != "\\":
                    end_quote = k
                    break
                k += 1
            tail = call[end_quote + 1 :]
            interpolated |= {
                a.strip().lstrip("&*")
                for a in re.split(r",", tail)
                if a.strip().strip("()").strip()
            }

        for expr in interpolated:
            e = expr.strip().strip("(),").lstrip("&*")
            if not e or e in ALLOWED_EXACT:
                continue
            low = e.lower()
            for bad in FORBIDDEN:
                if bad in low:
                    failures.append(
                        f"{path.as_posix()}:{line_no}: a tracing macro interpolates "
                        f"`{e}` — that can carry caller-controlled content or a "
                        f"secret. The subject of a call belongs in the AUDIT trail "
                        f"(audit_target: bounded, non-secret), never in the log.\n"
                        f"    {call.splitlines()[0][:100]}"
                    )
                    break

print(f"tracing calls checked in vibed/src : {checked}")
if failures:
    print(f"\n[FAIL] {len(failures)} log-hygiene violation(s):")
    for f in failures:
        print(f"   - {f}")
    print(
        "\nROADMAP Phase 2 exit criterion: « Aucune fuite d'informations "
        "sensibles (tokens, contenus de fichiers) dans les logs de niveau info. »"
    )
    sys.exit(1)

print("[OK] No tracing call interpolates caller-controlled content or a secret.")
