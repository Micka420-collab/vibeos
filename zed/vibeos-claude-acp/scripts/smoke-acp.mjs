// Headless ACP boot smoke test (no Zed, no display).
//
// Spawns the BUILT extension (`dist/index.js`), sends a real ACP `initialize`
// request over stdio, and asserts it answers with a valid initialize response —
// proving the extension loads, patches `canUseTool`, starts `runAcp()` and
// speaks ACP without crashing, in real conditions but without an editor.
//
// This does NOT cover the full end-to-end (session/new -> the SDK spawns the
// Claude native binary -> a tool call -> canUseTool -> vibeos:policy.check ->
// vibed). That needs the Claude Code native binary + a booted vibed socket +
// a full ACP prompt client (or Zed). See ../BLOCKERS.md.

import { spawn } from "node:child_process";

// Entry to boot: defaults to the tsc output (dist/index.js); the image build
// passes the single bundled artifact (dist/vibeos-claude-acp.mjs) so the SHIPPED
// file is the one proven to boot, not just the dev build.
const entry = process.argv[2] ?? "dist/index.js";
const child = spawn("node", [entry], { stdio: ["pipe", "pipe", "inherit"] });
let out = "";

const fail = (msg) => {
  console.error("SMOKE FAIL:", msg);
  child.kill();
  process.exit(1);
};

const timer = setTimeout(() => fail("no ACP response within 10s"), 10_000);

child.on("error", (e) => fail("spawn error: " + e.message));

child.stdout.on("data", (chunk) => {
  out += chunk.toString();
  const nl = out.indexOf("\n");
  if (nl < 0) return;
  clearTimeout(timer);
  let msg;
  try {
    msg = JSON.parse(out.slice(0, nl));
  } catch (e) {
    return fail("non-JSON response: " + out.slice(0, nl));
  }
  child.kill();
  if (msg.id === 1 && msg.result && msg.result.protocolVersion) {
    console.log("SMOKE OK: extension booted and answered ACP initialize (agent:", msg.result.agentInfo?.name + ")");
    process.exit(0);
  }
  fail("unexpected ACP response: " + JSON.stringify(msg));
});

child.stdin.write(
  JSON.stringify({
    jsonrpc: "2.0",
    id: 1,
    method: "initialize",
    params: { protocolVersion: 1, clientCapabilities: {} },
  }) + "\n",
);
