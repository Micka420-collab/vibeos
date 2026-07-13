// e2e-live-policy.mjs — Tier A of the Zed E2E: exercise the extension's REAL
// vibed integration against a LIVE `vibed` socket, no editor and no Claude
// binary needed. Proves the governed auto-mode's decision path end-to-end over
// the real socket: the same `checkPolicy` the patched `canUseTool` calls, hit
// against a running daemon.
//
//   node scripts/e2e-live-policy.mjs [/run/vibed/mcp.sock]
//
// Requires: `npm run build` first (imports the compiled dist/), and a running
// vibed serving policy.check on the socket. Exit 0 iff every expectation holds.
//
// What it asserts (the governance contract the Zed patch relies on):
//   * fs.read  (T0) -> "allow"            -> auto-allowed, NO editor prompt
//   * fs.list  (T0) -> "allow"            -> auto-allowed, NO editor prompt
//   * pkg.install (T2) -> "require_approval" -> NOT auto-allowed, editor prompts
//   * svc.restart (T2) -> "require_approval" -> NOT auto-allowed, editor prompts
//   * an unknown tool -> "deny" or null   -> never auto-allowed
//
// Tier B (the full editor round-trip: Zed spawns the Claude binary, a real tool
// call flows through canUseTool -> policy.check -> vibed) is the manual checklist
// printed by scripts/e2e-zed.sh — it needs Zed, which is not headless.

import { checkPolicy, DEFAULT_SOCKET } from "../dist/policy-client.js";
import { shouldAutoAllow } from "../dist/gate.js";

const socket = process.argv[2] ?? DEFAULT_SOCKET;

// [tool, target, expected decision, expected auto-allow]
const cases = [
  ["fs.read", "/etc/os-release", "allow", true],
  ["fs.list", "/etc", "allow", true],
  ["pkg.install", "htop", "require_approval", false],
  ["svc.restart", "sshd.service", "require_approval", false],
];

let failures = 0;
const line = (ok, msg) => console.log(`${ok ? "PASS" : "FAIL"}  ${msg}`);

console.log(`E2E Tier A — live policy.check against ${socket}\n`);

// Fail fast with a clear message if the socket is not there.
const probe = await checkPolicy(socket, "fs.read", "/etc/os-release");
if (probe === null) {
  console.error(
    `\nCannot reach vibed at ${socket} (socket absent, or policy.check not served).\n` +
      `Start vibed first (scripts/e2e-zed.sh does this), then re-run.`,
  );
  process.exit(2);
}

for (const [tool, target, expected, expectAuto] of cases) {
  const decision = await checkPolicy(socket, tool, target);
  const auto = shouldAutoAllow(decision);
  const ok = decision === expected && auto === expectAuto;
  if (!ok) failures++;
  line(
    ok,
    `${tool}(${target}) -> ${decision} (auto-allow=${auto}); ` +
      `expected ${expected}/auto=${expectAuto}`,
  );
}

// An unknown tool must never be auto-allowed (default-deny hint).
const unknown = await checkPolicy(socket, "disk.wipe", undefined);
const unknownOk = !shouldAutoAllow(unknown);
if (!unknownOk) failures++;
line(unknownOk, `disk.wipe -> ${unknown} (auto-allow=${shouldAutoAllow(unknown)}); must not auto-allow`);

console.log();
if (failures === 0) {
  console.log("E2E Tier A OK: the governed auto-mode decisions match the policy over the live socket.");
  process.exit(0);
}
console.error(`E2E Tier A FAILED: ${failures} expectation(s) not met.`);
process.exit(1);
