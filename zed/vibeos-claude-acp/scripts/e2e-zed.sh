#!/usr/bin/env bash
# e2e-zed.sh — turnkey End-to-End harness for the VibeOS-for-Zed extension.
#
# Run this AS-IS on a machine that has (or can build) vibed. It is idempotent
# and does two things:
#
#   Tier A (AUTOMATED, no Zed, no Claude binary): builds & bundles the extension,
#   ensures a vibed is listening, and asserts the governed auto-mode's decisions
#   over the REAL socket — fs.read (T0) auto-allowed, pkg.install/svc.restart
#   (T2) require approval. This is the part CI-equivalent machines can run.
#
#   Tier B (MANUAL, needs Zed + the Claude native binary): prints a ready-to-
#   follow checklist and writes a Zed settings.json, to drive the full editor
#   round-trip (Zed -> adapter -> canUseTool -> policy.check -> vibed) by hand.
#
# Nothing here is run automatically in CI or in the build environment; it is the
# script a maintainer runs the day a machine with Zed is available (BLOCKERS.md).
#
# Usage:
#   zed/vibeos-claude-acp/scripts/e2e-zed.sh          # auto-detect / build vibed
#   VIBED_SOCKET=/run/vibed/mcp.sock e2e-zed.sh       # use an already-running vibed

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ext_dir="$(cd "$here/.." && pwd)"
repo_root="$(cd "$ext_dir/../.." && pwd)"

say() { printf '\n=== %s ===\n' "$*"; }
have() { command -v "$1" >/dev/null 2>&1; }

# ---------------------------------------------------------------------------
# 0. Prerequisites
# ---------------------------------------------------------------------------
say "prerequisites"
have node || { echo "node (>=22) is required"; exit 1; }
echo "node $(node --version)"
have zed && echo "zed: $(command -v zed)" || echo "zed: NOT installed (Tier B is manual-only until it is)"

# ---------------------------------------------------------------------------
# 1. Build + bundle the extension (ADR-015 recipe)
# ---------------------------------------------------------------------------
say "build + bundle extension"
cd "$ext_dir"
npm ci --ignore-scripts
npm run build          # tsc -> dist/*.js (used by the Tier A harness)
npm run bundle         # esbuild -> dist/vibeos-claude-acp.mjs (the shipped file)
node scripts/smoke-acp.mjs dist/vibeos-claude-acp.mjs

# ---------------------------------------------------------------------------
# 2. Ensure a vibed is listening
# ---------------------------------------------------------------------------
say "vibed socket"
started_vibed=""
socket="${VIBED_SOCKET:-/run/vibed/mcp.sock}"
if [ -S "$socket" ]; then
    echo "using existing vibed socket: $socket"
else
    echo "no socket at $socket — starting a scratch vibed (rootless)"
    have cargo || { echo "cargo needed to build vibed, or point VIBED_SOCKET at a running one"; exit 1; }
    ( cd "$repo_root/vibed" && cargo build --release --locked )
    run_dir="$(mktemp -d)"
    socket="$run_dir/mcp.sock"
    VIBED_SOCKET="$socket" \
        VIBED_POLICY_DIR="$repo_root/security/policy.d" \
        VIBED_AUDIT_DIR="$run_dir/audit" \
        "$repo_root/vibed/target/release/vibed" &
    started_vibed="$!"
    trap '[ -n "$started_vibed" ] && kill "$started_vibed" 2>/dev/null || true' EXIT
    for _ in $(seq 1 50); do [ -S "$socket" ] && break; sleep 0.1; done
    [ -S "$socket" ] || { echo "vibed did not create $socket"; exit 1; }
    echo "scratch vibed listening on $socket (pid $started_vibed)"
fi

# ---------------------------------------------------------------------------
# 3. Tier A — automated decision check over the live socket
# ---------------------------------------------------------------------------
say "Tier A — live policy decisions"
node "$here/e2e-live-policy.mjs" "$socket"

# ---------------------------------------------------------------------------
# 4. Tier B — write Zed config + print the manual checklist
# ---------------------------------------------------------------------------
say "Tier B — Zed editor round-trip (manual)"
bundle="$ext_dir/dist/vibeos-claude-acp.mjs"
zed_cfg_dir="${XDG_CONFIG_HOME:-$HOME/.config}/zed"
mkdir -p "$zed_cfg_dir"
settings="$zed_cfg_dir/settings.vibeos-e2e.json"
cat > "$settings" <<JSON
{
  "//": "VibeOS-for-Zed E2E — merge into $zed_cfg_dir/settings.json.",
  "agent_servers": {
    "vibeos-claude-acp": {
      "command": "node",
      "args": ["$bundle"],
      "env": { "VIBED_SOCKET": "$socket" }
    }
  },
  "context_servers": {
    "vibed": {
      "command": "socat",
      "args": ["STDIO", "UNIX-CONNECT:$socket"]
    }
  }
}
JSON
echo "wrote $settings"

cat <<CHECKLIST

Follow these on the machine with Zed (each step lists the EXPECTED result):

  1. Merge $settings into $zed_cfg_dir/settings.json.
  2. Ensure the Claude native binary is installed (npm i -g @anthropic-ai/claude-code)
     — the adapter spawns it; without it session/new fails.
  3. Open Zed, open the agent panel, pick the "vibeos-claude-acp" agent, start a session.
  4. Ask it to READ a file, e.g. "read /etc/os-release".
       EXPECT: no editor permission prompt (fs.read is T0 -> policy Allow -> auto).
       VERIFY: vibectl audit shows an fs.read allow for your uid.
  5. Ask it to INSTALL a package, e.g. "install htop".
       EXPECT: it does NOT run silently — a permission prompt / pending appears
               (pkg.install is T2 -> require_approval, never auto-allowed).
       VERIFY: vibectl approvals list shows a pending request; audit shows
               pending_approval. Approving with 'vibectl approve <id>' then lets
               it proceed exactly once.
  6. (Optional) 'vibectl agent stop' style kill-switch + reasoning capture, per ADR-013.

Tier A already proved the decision path over the live socket; Tier B proves the
editor actually suppresses the prompt for Allow and shows it for require_approval.
CHECKLIST

say "done"
echo "Tier A: PASSED (see above). Tier B: manual checklist printed."
