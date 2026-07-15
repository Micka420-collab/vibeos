#!/usr/bin/env node
// check-hud-client.js — tests for the HUD's vibed client (desktop/quickshell/
// vibed_client.js), runnable without Qt.
//
// WHY THIS EXISTS
// ---------------
// The HUD is QML + a JS library, and nothing in CI could execute either: the
// Rust suite never touches them, and there is no Quickshell/Qt runtime on a
// runner. So the layer that turns vibed's wire format into what the user
// actually READS on screen shipped entirely unverified — a renamed field or an
// inverted condition would only ever surface on a booted Plasma.
//
// `vibed_client.js` is a QML `.pragma library`: plain ECMAScript, no QML imports,
// no QML types. That makes it loadable under plain node once the pragma line is
// stripped — which is exactly what this script does. It covers the pure data
// mappers (the QML *rendering* stays machine-gated; see desktop/quickshell/
// README.md).
//
// WHAT IT GUARDS, beyond "does it parse":
//   1. No QML `Qt` reference sneaks into the library (a runtime ReferenceError
//      that no import error would announce).
//   2. The mappers match vibed's REAL wire format — the agent.sessions fixture
//      below was captured from vibed's own Rust code, not hand-invented.
//   3. The project's honesty rule holds in the data path: an unknown timestamp
//      must render as unknown, and a truncated listing must say it is truncated.
//   4. Degenerate payloads degrade, never throw (a throw in a QML binding kills
//      the panel).
//
// Usage: node scripts/check-hud-client.js [path/to/vibed_client.js]
"use strict";

const fs = require("fs");
const path = require("path");

const SRC = process.argv[2] ||
  path.join(__dirname, "..", "desktop", "quickshell", "vibed_client.js");

let raw;
try {
  raw = fs.readFileSync(SRC, "utf8");
} catch (e) {
  console.error(`FAIL: cannot read ${SRC}: ${e.message}`);
  process.exit(1);
}

// --- Guard 1: the pragma is the only QML-ism this file is allowed --------------
if (!/^\.pragma library$/m.test(raw)) {
  console.error("FAIL: '.pragma library' not found — vibed_client.js must stay a "
    + "QML JS library (and stay loadable here).");
  process.exit(1);
}
let src = raw.replace(/^\.pragma library$/m, "");

// --- Guard 2: a .pragma library has NO QML engine context ---------------------
// Qt.formatDateTime / Qt.locale / any QML type is a runtime ReferenceError here,
// which is why the formatting below is hand-rolled ECMAScript.
const qtRefs = [];
src.split("\n").forEach((line, i) => {
  const code = line.replace(/\/\/.*$/, "");
  if (/\bQt\s*\./.test(code)) qtRefs.push(`  line ${i + 1}: ${line.trim()}`);
});
if (qtRefs.length) {
  console.error("FAIL: the QML `Qt` object is referenced inside a .pragma library "
    + "(unavailable at runtime):\n" + qtRefs.join("\n"));
  process.exit(1);
}

const EXPORTS = [
  "sessionsToHistory", "historyKey", "sessionsTruncationNote",
  "formatStamp", "formatDuration", "formatBytes", "reasoningToLive",
  "toolsCallRequestId", "toolsCallRequest", "parseToolResult", "parseLine",
  "agentsListToRoster",
];

// The library warns on a corrupt socket line — legitimate behaviour that one
// check below triggers on purpose. Route its console through a mutable shim so
// that expected warning does not look like a failure in CI output.
let quiet = false;
const shim = {
  log: (...a) => { if (!quiet) console.log(...a); },
  warn: (...a) => { if (!quiet) console.warn(...a); },
  error: (...a) => { if (!quiet) console.error(...a); },
};

let V;
try {
  const mod = { exports: {} };
  new Function("module", "exports", "console",
    src + "\n;module.exports={" + EXPORTS.join(",") + "};"
  )(mod, mod.exports, shim);
  V = mod.exports;
} catch (e) {
  console.error(`FAIL: vibed_client.js did not load: ${e.message}`);
  process.exit(1);
}

let failures = 0;
function check(name, cond, detail) {
  if (cond) {
    console.log(`  ok   ${name}`);
  } else {
    console.log(`  FAIL ${name}${detail !== undefined ? " — " + detail : ""}`);
    failures++;
  }
}
function group(title) { console.log(`\n[${title}]`); }

// -----------------------------------------------------------------------------
// Fixture: a REAL agent.sessions payload, captured from vibed's own
// reasoning::list_sessions + mcp::agent_sessions (two sessions, one with two
// blocks). Keep in sync with vibed/src/mcp.rs `fn agent_sessions` — the Rust
// side pins the same shape in `agent_sessions_returns_a_well_formed_listing`.
// -----------------------------------------------------------------------------
const WIRE = {
  count: 2,
  latest: "auto-1783468800-4242",
  sessions: [
    { bytes: 196, id: "auto-1783468800-4242", last_unix: 1784093174, started_unix: 1783468800 },
    { bytes: 96, id: "auto-1783300000-99", last_unix: 1784093174, started_unix: 1783300000 },
  ],
  total: 2,
  truncated: false,
};

group("sessionsToHistory against vibed's real wire payload");
const hist = V.sessionsToHistory(WIRE);
check("maps every session", hist.length === WIRE.sessions.length,
  `${hist.length} vs ${WIRE.sessions.length}`);
check("preserves vibed's newest-first order",
  hist[0].sessionId === WIRE.sessions[0].id, hist[0].sessionId);
check("every row carries a sessionId",
  hist.every(h => typeof h.sessionId === "string" && h.sessionId.length > 0));
check("every row carries the three rendered labels",
  hist.every(h => typeof h.startedAt === "string"
    && typeof h.durationLabel === "string"
    && typeof h.sizeLabel === "string"), JSON.stringify(hist[0]));
check("no 'undefined' leaks into a label (a renamed field would show up here)",
  !JSON.stringify(hist).includes("undefined"), JSON.stringify(hist));

group("honesty: what is unknown must READ as unknown");
const nullStart = V.sessionsToHistory({
  sessions: [{ id: "s", started_unix: null, last_unix: 1783468800, bytes: 10 }],
});
check("a null started_unix never renders as a plausible date",
  nullStart[0].startedAt === "début inconnu", nullStart[0].startedAt);
check("an unknown start yields an unknown duration, not 0",
  nullStart[0].durationLabel === "durée inconnue", nullStart[0].durationLabel);
// last_unix is 0 when vibed's stat could not read an mtime (reasoning.rs map_or(0)).
const noMtime = V.sessionsToHistory({
  sessions: [{ id: "s", started_unix: 1783468800, last_unix: 0, bytes: 10 }],
});
check("an unreadable mtime does not produce a negative duration",
  noMtime[0].durationLabel === "durée inconnue", noMtime[0].durationLabel);

group("degenerate payloads degrade, never throw (a throw kills the QML binding)");
for (const bad of [null, undefined, {}, { sessions: [] }, { sessions: [{}] },
                   { sessions: [{ id: "x" }] }]) {
  let threw = null;
  try { V.sessionsToHistory(bad); } catch (e) { threw = e.message; }
  check(`sessionsToHistory(${JSON.stringify(bad) || String(bad)})`, threw === null, threw);
}
for (const bad of [null, undefined, {}, { lines: [] }]) {
  let threw = null;
  try { V.reasoningToLive(bad); } catch (e) { threw = e.message; }
  check(`reasoningToLive(${JSON.stringify(bad) || String(bad)})`, threw === null, threw);
}

group("historyKey: only a REAL change may reset the ListView model");
// Reassigning ListView.model destroys delegates and throws away the user's
// scroll position. agent.sessions is polled every 5s, so an unconditional
// republish would make the list unusable below the fold.
check("identical payload -> identical key",
  V.historyKey(V.sessionsToHistory(WIRE)) === V.historyKey(V.sessionsToHistory(WIRE)));
check("reordered payload -> different key",
  V.historyKey(V.sessionsToHistory({ sessions: WIRE.sessions.slice().reverse() }))
    !== V.historyKey(V.sessionsToHistory(WIRE)));
check("a new session -> different key",
  V.historyKey(V.sessionsToHistory({
    sessions: [{ id: "new", started_unix: 1, last_unix: 2, bytes: 3 }].concat(WIRE.sessions),
  })) !== V.historyKey(V.sessionsToHistory(WIRE)));
check("empty list -> empty key", V.historyKey([]) === "");

group("sessionsTruncationNote: a capped view must admit it");
check("silent when the view is complete",
  V.sessionsTruncationNote({ truncated: false, count: 3, total: 3 }) === "");
check("names the true total when truncated",
  V.sessionsTruncationNote({ truncated: true, count: 200, total: 347 }).includes("347"));
check("names how many are shown when truncated",
  V.sessionsTruncationNote({ truncated: true, count: 200, total: 347 }).includes("200"));
check("no note without data", V.sessionsTruncationNote(null) === "");

group("formatters (deterministic — no host locale, no Qt)");
check("formatBytes(512)", V.formatBytes(512) === "512 o", V.formatBytes(512));
check("formatBytes(2048)", V.formatBytes(2048) === "2 Kio", V.formatBytes(2048));
check("formatBytes(5 MiB)", V.formatBytes(5 * 1024 * 1024) === "5.0 Mio", V.formatBytes(5 * 1024 * 1024));
check("formatDuration(45)", V.formatDuration(45) === "45 s", V.formatDuration(45));
check("formatDuration(600)", V.formatDuration(600) === "10 min", V.formatDuration(600));
check("formatDuration(3600)", V.formatDuration(3600) === "1 h", V.formatDuration(3600));
check("formatDuration(13320)", V.formatDuration(13320) === "3 h 42", V.formatDuration(13320));
check("formatDuration(0)", V.formatDuration(0) === "0 s", V.formatDuration(0));
check("formatStamp shape is 'D mois HH:MM'",
  /^\d{1,2} \S+ \d{2}:\d{2}$/.test(V.formatStamp(1783468800)), V.formatStamp(1783468800));

group("request-id correlation: the poll must not steal the look-back's reply");
// Both the 5s poll and a history selection call agent.thinking with different
// tails. Routing on tool name (or on the session_id echoed back) lets the poll's
// short answer overwrite the user's look-back.
const look = V.toolsCallRequestId("agent.thinking", { session_id: "A", tail: 200 });
const poll = V.toolsCallRequestId("agent.thinking", { session_id: "A", tail: 40 });
check("two calls to one tool get distinct ids", look.id !== poll.id, `${look.id} vs ${poll.id}`);
check("request is valid JSON-RPC tools/call",
  JSON.parse(look.line.trim()).method === "tools/call");
check("arguments travel intact",
  JSON.parse(look.line.trim()).params.arguments.tail === 200);
check("toolsCallRequest still returns just the line (unchanged callers)",
  typeof V.toolsCallRequest("os.status", {}) === "string");

const okReply = JSON.stringify({
  jsonrpc: "2.0", id: look.id,
  result: { content: [{ text: JSON.stringify({ session_id: "A", lines: [], count: 0 }) }] },
});
const okRes = V.parseToolResult(V.parseLine(okReply));
check("success echoes the request id", okRes.id === look.id, `${okRes.id}`);
check("success resolves the tool name", okRes.tool === "agent.thinking", okRes.tool);

const errReply = JSON.stringify({
  jsonrpc: "2.0", id: poll.id, error: { code: -32602, message: "denied" },
});
const errRes = V.parseToolResult(V.parseLine(errReply));
check("an ERROR reply still carries its id (so a spinner can be cleared)",
  errRes.id === poll.id, `${errRes.id}`);
check("an error reply is not ok", errRes.ok === false);
quiet = true;   // the next line warns on purpose
const corrupt = V.parseLine("{not json");
quiet = false;
check("a corrupt line degrades to null, never throws", corrupt === null);

console.log(failures === 0
  ? "\n[OK] HUD client checks passed."
  : `\n[FAIL] ${failures} check(s) failed.`);
process.exit(failures === 0 ? 0 : 1);
