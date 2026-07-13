// vibed_client.js — HUD-side client stub for the vibed MCP socket.
//
// STATUS: this module documents the exact wire format and serves MOCK data.
// No socket is opened anywhere yet — /usr/bin/vibed IS shipped in the image
// and runs at boot (Phase 2), but the HUD live wiring is still to be coded.
// TODO(Phase 2): wire the request builders below to a
// Quickshell.Io Socket in shell.qml (see the sketch in shell.qml's header).
//
// ---------------------------------------------------------------------------
// WIRE FORMAT — must stay in sync with vibed/src/mcp.rs
// ---------------------------------------------------------------------------
// Transport: unix socket /run/vibed/mcp.sock (root:vibeos-agents, 0660),
// LINE-DELIMITED JSON-RPC 2.0 — one JSON object per line, '\n' terminated.
//
// 1) initialize (request -> response):
//    -> {"jsonrpc":"2.0","id":1,"method":"initialize","params":
//        {"protocolVersion":"2024-11-05",
//         "clientInfo":{"name":"vibeos-hud","version":"0.1.0"},
//         "capabilities":{}}}
//    <- {"jsonrpc":"2.0","id":1,"result":
//        {"protocolVersion":"2024-11-05",
//         "serverInfo":{"name":"vibed","version":"0.1.0"},
//         "capabilities":{"tools":{}}}}
//
// 2) initialized (notification, no id => no response):
//    -> {"jsonrpc":"2.0","method":"notifications/initialized","params":{}}
//
// 3) tools/call (the two T0 read-only tools the HUD uses):
//    -> {"jsonrpc":"2.0","id":2,"method":"tools/call","params":
//        {"name":"os.status","arguments":{}}}
//    -> {"jsonrpc":"2.0","id":3,"method":"tools/call","params":
//        {"name":"memory.query","arguments":{"query":""}}}
//
//    Tool responses use the MCP content envelope; the payload is a
//    JSON-ENCODED STRING inside content[0].text (parse it a second time):
//    <- {"jsonrpc":"2.0","id":2,"result":
//        {"content":[{"type":"text","text":"{\"uptime_seconds\":123.4,...}"}],
//         "isError":false}}
//
//    os.status text payload:
//      { uptime_seconds, loadavg_1_5_15: ["0.42","0.31","0.20"],
//        mem_total_kb, mem_available_kb,
//        mounts: [{device,mountpoint,fstype}, ...], note }
//    memory.query text payload:
//      { initialized: true, query, scanned_files, matches: [{file}, ...] }
//      or, before genesis has run:
//      { initialized: false, note }
//
//    Policy denials are NOT protocol errors: they come back as a normal
//    result with isError:true and a "policy: ..." text (deny / builtin
//    denylist / T2-T3 pending approval). Handle them as data, not crashes.
//
// 4) protocol errors:
//    <- {"jsonrpc":"2.0","id":X,"error":{"code":-32601,"message":"..."}}
//    codes seen from vibed: -32700 parse error, -32601 method not found,
//    -32602 invalid params.
// ---------------------------------------------------------------------------

.pragma library

var SOCKET_PATH = "/run/vibed/mcp.sock";
var PROTOCOL_VERSION = "2024-11-05";   // mcp.rs: PROTOCOL_VERSION
var CLIENT_NAME = "vibeos-hud";
var CLIENT_VERSION = "0.1.0";

// Monotonic JSON-RPC id, and an id -> tool-name map so responses can be
// routed back to the right piece of HUD state.
var _nextId = 1;
var _pending = {};

// ---------------------------------------------------------------------------
// Request builders (Phase 2: write these lines to the Socket verbatim)
// ---------------------------------------------------------------------------

function initializeRequest() {
    return JSON.stringify({
        jsonrpc: "2.0",
        id: _nextId++,
        method: "initialize",
        params: {
            protocolVersion: PROTOCOL_VERSION,
            clientInfo: { name: CLIENT_NAME, version: CLIENT_VERSION },
            capabilities: {}
        }
    }) + "\n";
}

// JSON-RPC notification: no id, vibed sends no response.
function initializedNotification() {
    return JSON.stringify({
        jsonrpc: "2.0",
        method: "notifications/initialized",
        params: {}
    }) + "\n";
}

// tools/call for a named vibed tool. The HUD only ever calls T0 read-only
// tools ("os.status", "memory.query"): it is strictly an observer.
function toolsCallRequest(name, args) {
    var id = _nextId++;
    _pending[id] = name;
    return JSON.stringify({
        jsonrpc: "2.0",
        id: id,
        method: "tools/call",
        params: { name: name, arguments: args || {} }
    }) + "\n";
}

// ---------------------------------------------------------------------------
// Response parsing (Phase 2: feed each SplitParser line through these)
// ---------------------------------------------------------------------------

// One raw socket line -> parsed message, or null (never throws: a corrupt
// line must degrade to "offline", not crash the shell).
function parseLine(line) {
    try {
        return JSON.parse(line);
    } catch (e) {
        console.warn("vibed_client: unparseable line from socket:", e);
        return null;
    }
}

// Unwraps a tools/call response into:
//   { tool, ok: true,  data: <parsed payload object> }        on success
//   { tool, ok: false, error: "<message>" }                    on tool error,
//     policy denial, pending T2/T3 approval, or protocol error.
// `tool` is recovered from the id -> name map filled by toolsCallRequest.
function parseToolResult(msg) {
    var tool = (msg && msg.id !== undefined) ? _pending[msg.id] : undefined;
    if (tool !== undefined)
        delete _pending[msg.id];

    if (!msg)
        return { tool: tool, ok: false, error: "empty message" };
    if (msg.error)
        return { tool: tool, ok: false,
                 error: msg.error.message || ("code " + msg.error.code) };

    var result = msg.result;
    if (!result || !result.content || result.content.length === 0)
        return { tool: tool, ok: false, error: "malformed tool result" };

    var text = result.content[0].text || "";
    if (result.isError === true)
        return { tool: tool, ok: false, error: text };

    // The payload is a JSON string inside the MCP text block; some tools
    // could return plain text, so fall back to the raw string.
    try {
        return { tool: tool, ok: true, data: JSON.parse(text) };
    } catch (e) {
        return { tool: tool, ok: true, data: text };
    }
}

// ---------------------------------------------------------------------------
// MOCK DATA — v0.1 only. TODO(Phase 2): delete everything below once the
// socket wiring in shell.qml replaces it.
// Shapes mirror the real payloads above so the QML bindings will not change
// when the live data arrives.
// ---------------------------------------------------------------------------

// Mirrors the os.status text payload of mcp.rs.
function mockOsStatus() {
    return {
        uptime_seconds: 8423.5,
        loadavg_1_5_15: [
            (0.2 + Math.random() * 0.8).toFixed(2), "0.35", "0.28"
        ],
        mem_total_kb: 32768000,
        mem_available_kb: 18200000 + Math.floor(Math.random() * 2000000),
        mounts: [
            { device: "/dev/nvme0n1p3", mountpoint: "/sysroot", fstype: "btrfs" }
        ],
        note: "MOCK v0.1 — not read from vibed"
    };
}

// Mirrors the memory.query text payload of mcp.rs (initialized store).
function mockMemoryQuery() {
    return {
        initialized: true,
        query: "",
        scanned_files: 12,
        matches: [
            { file: "identity.toml" },
            { file: "hardware.json" },
            { file: "journal/2026-07-03.jsonl" }
        ],
        note: "MOCK v0.1 — not read from vibed"
    };
}

// HONESTY NOTE: vibed v0.1 exposes NO agents.list tool — an agent roster is
// not derivable from the daemon yet. This mock exists purely so the HUD
// design can be seen with data in it. TODO(Phase 2): specify agents.list
// (or an audit-derived stream keyed by SO_PEERCRED pid) with the vibed track,
// then replace this. The extra `project`/`elapsed` fields feed the AgentStatus
// hover tooltip; they mirror what an audit-derived roster would expose.
function mockAgents() {
    var approvalPending = Math.random() < 0.3; // demo: occasional T2 lock
    return [
        { name: "claude-code", tier: 1, awaitingApproval: false,
          activity: "fs.write ~/projects/app/src/main.rs (T1)",
          project: "~/projects/app", elapsed: "4m12s" },
        { name: "opencode", tier: 0, awaitingApproval: false,
          activity: "fs.read journalctl output (T0)",
          project: "~/projects/api", elapsed: "1m03s" },
        { name: "opencode", tier: approvalPending ? 2 : 0,
          awaitingApproval: approvalPending,
          activity: approvalPending
              ? "pkg.install ripgrep — EN ATTENTE D'APPROBATION HUMAINE (T2)"
              : "idle",
          project: "~/projects/tools", elapsed: "0m48s" }
    ];
}

// v0.1 HONEST DEFAULT: report ollama as UNAVAILABLE so the gauge renders
// "ollama —" instead of inventing a loaded model and random VRAM figures.
// TODO(Phase 2): replace with GET http://127.0.0.1:11434/api/ps (ollama) +
// nvidia-smi via Quickshell.Io Process — see OllamaGauge.qml header.
function mockOllama() {
    return { available: false };

    // TODO(Phase 2) design-preview demo data ONLY — never shipped as the
    // default. Flip the early return above to preview the "online" gauge:
    // return {
    //     available: true,
    //     model: "qwen2.5-coder:7b",
    //     generating: Math.random() < 0.4,
    //     vram_used_mb: 4800 + Math.floor(Math.random() * 1200),
    //     vram_total_mb: 8192   // RTX 3070 Ti budget (docs/ECOSYSTEM.md)
    // };
}
