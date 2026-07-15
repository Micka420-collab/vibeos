// vibed_client.js — HUD-side client for the vibed MCP socket.
//
// STATUS: LIVE. shell.qml opens a Quickshell.Io Socket on SOCKET_PATH and drives
// the request builders + parsers below (os.status, memory.query, agent.sessions
// -> agent.thinking). The `reasoningToLive` mapper turns agent.thinking into the
// ReasoningPanel shape. The v0.1 `mock*` scaffolding has been removed now that
// the live wiring drives the HUD — the request builders + parsers below are the
// only data path. (OllamaGauge keeps its own local probe; see its QML header.)
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
    return toolsCallRequestId(name, args).line;
}

// Same request, but also hands back the JSON-RPC id. Needed when two calls to
// the SAME tool are in flight with different arguments and must not be confused:
// the poll tails the live session (agent.thinking, small tail) while a history
// selection reads a chosen session (agent.thinking, larger tail). Routing those
// on the tool name — or even on the session_id in the reply — makes the poll's
// answer overwrite the selection's. The id is the only unambiguous correlation.
function toolsCallRequestId(name, args) {
    var id = _nextId++;
    _pending[id] = name;
    return {
        id: id,
        line: JSON.stringify({
            jsonrpc: "2.0",
            id: id,
            method: "tools/call",
            params: { name: name, arguments: args || {} }
        }) + "\n"
    };
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
//   { tool, id, ok: true,  data: <parsed payload object> }     on success
//   { tool, id, ok: false, error: "<message>" }                 on tool error,
//     policy denial, pending T2/T3 approval, or protocol error.
// `tool` is recovered from the id -> name map filled by toolsCallRequest. `id`
// is the JSON-RPC id echoed by the daemon, carried on EVERY shape (the error
// shapes included — a caller waiting on a specific request must be able to stop
// waiting when that request fails, not just when it succeeds).
function parseToolResult(msg) {
    var id = (msg && msg.id !== undefined) ? msg.id : -1;
    var tool = (msg && msg.id !== undefined) ? _pending[msg.id] : undefined;
    if (tool !== undefined)
        delete _pending[msg.id];

    if (!msg)
        return { tool: tool, id: id, ok: false, error: "empty message" };
    if (msg.error)
        return { tool: tool, id: id, ok: false,
                 error: msg.error.message || ("code " + msg.error.code) };

    var result = msg.result;
    if (!result || !result.content || result.content.length === 0)
        return { tool: tool, id: id, ok: false, error: "malformed tool result" };

    var text = result.content[0].text || "";
    if (result.isError === true)
        return { tool: tool, id: id, ok: false, error: text };

    // The payload is a JSON string inside the MCP text block; some tools
    // could return plain text, so fall back to the raw string.
    try {
        return { tool: tool, id: id, ok: true, data: JSON.parse(text) };
    } catch (e) {
        return { tool: tool, id: id, ok: true, data: text };
    }
}

// Map an agent.thinking payload ({ session_id, lines:[{ts_unix,block:{kind,text}}] })
// into the ReasoningPanel `live` shape ([{ sessionId, provider, model, raw,
// redacted, streaming, text }]). One entry for the tailed session. Defensive:
// unknown block shapes contribute no text; provider/model are not carried by the
// reasoning record (the supervisor knows them, the store does not yet) so they
// stay empty — the panel degrades gracefully. Returns [] when there is nothing.
function reasoningToLive(data) {
    if (!data || !data.lines || data.lines.length === 0) return [];
    var text = "";
    var redacted = false;
    var lastKind = "";
    for (var i = 0; i < data.lines.length; ++i) {
        var block = data.lines[i] && data.lines[i].block;
        if (!block) continue;
        lastKind = block.kind || lastKind;
        if (block.kind === "redacted_thinking") { redacted = true; continue; }
        if (typeof block.text === "string") text += block.text;
    }
    if (text === "" && !redacted) return [];
    return [{
        sessionId: data.session_id || "",
        provider: "",            // not in the reasoning record yet (honest blank)
        model: "",
        raw: false,              // cloud summary; only a local ollama run is raw
        redacted: redacted,
        streaming: lastKind === "thinking_delta",
        text: text
    }];
}

// ---------------------------------------------------------------------------
// Formatting helpers. This file is a `.pragma library`, so the QML `Qt` object
// (Qt.formatDateTime) and Qt.locale are NOT reachable here — everything below is
// plain ECMAScript. The month table keeps the rendering deterministic instead of
// depending on the host locale, and matches the panel's French copy.
// ---------------------------------------------------------------------------
var MONTHS_FR = ["janv.", "févr.", "mars", "avr.", "mai", "juin",
                 "juil.", "août", "sept.", "oct.", "nov.", "déc."];

function formatStamp(unix) {
    var d = new Date(unix * 1000);
    function pad(n) { return (n < 10 ? "0" : "") + n; }
    return d.getDate() + " " + MONTHS_FR[d.getMonth()] + " "
         + pad(d.getHours()) + ":" + pad(d.getMinutes());
}

// Coarse, human duration: seconds under a minute, then minutes, then h+min.
function formatDuration(seconds) {
    if (seconds < 60) return Math.max(0, Math.round(seconds)) + " s";
    if (seconds < 3600) return Math.round(seconds / 60) + " min";
    var h = Math.floor(seconds / 3600);
    var m = Math.round((seconds % 3600) / 60);
    return m === 0 ? (h + " h") : (h + " h " + m);
}

function formatBytes(n) {
    if (n < 1024) return n + " o";
    if (n < 1024 * 1024) return Math.round(n / 1024) + " Kio";
    return (n / (1024 * 1024)).toFixed(1) + " Mio";
}

// Map an agent.sessions payload ({ sessions:[{ id, started_unix, last_unix,
// bytes }], count, total, truncated, latest }) into the ReasoningPanel `history`
// shape ([{ sessionId, startedAt, durationLabel, sizeLabel }]).
//
// HONESTY: vibed sends `started_unix: null` when a session's first line could
// not be read, so the label says so rather than substituting `last_unix` — an
// unknown start must never render as a plausible one. `provider`/`model` are NOT
// produced here: the reasoning store does not hold them (the supervisor writes
// them to the memory journal as an `autonomous_session` record), so the panel
// shows what the store actually knows. There is deliberately no turn count —
// vibed would have to read every session end to end on each poll to produce one.
function sessionsToHistory(data) {
    if (!data || !data.sessions || data.sessions.length === 0) return [];
    return data.sessions.filter(function (s) {
        // A row with no usable id is dropped, not rendered. "" is the panel's
        // sentinel for "the live view", so an empty sessionId would render a row
        // that looks permanently selected and un-clickable (tapping it emits
        // historySelected("") = go live). vibed already guarantees non-empty ids
        // (reasoning::safe_session_id, and list_sessions filters every candidate
        // through it) — but that is a cross-language guarantee, and this side
        // should not depend on it silently.
        return s && typeof s.id === "string" && s.id !== "";
    }).map(function (s) {
        var started = (typeof s.started_unix === "number") ? s.started_unix : null;
        var last = (typeof s.last_unix === "number") ? s.last_unix : null;
        var known = started !== null && last !== null && last >= started;
        return {
            sessionId: s.id || "",
            startedAt: started !== null ? formatStamp(started) : "début inconnu",
            durationLabel: known ? formatDuration(last - started) : "durée inconnue",
            sizeLabel: formatBytes((typeof s.bytes === "number") ? s.bytes : 0)
        };
    });
}

// vibed caps the listing at 200 sessions and reports `total`/`truncated` so a
// partial view can say so. Rendering 200 rows as if they were the whole history
// would be exactly the kind of quiet lie the store-side code refuses to tell —
// so turn that into a visible footer. "" when the view IS complete.
function sessionsTruncationNote(data) {
    if (!data || data.truncated !== true) return "";
    var shown = (typeof data.count === "number") ? data.count : 0;
    var total = (typeof data.total === "number") ? data.total : shown;
    return shown + " sessions les plus récentes sur " + total;
}

// Map an agents.list payload ({ agents:[{ uid,pid,name,tier,activity,
// awaiting_approval,last_seen_unix,idle_seconds,calls }], ... }) into the
// AgentStatus chip shape ([{ name, tier, awaitingApproval, activity, project,
// elapsed }]). The roster is already confined to the caller's uid by vibed, so
// the HUD (running as the session user) receives only that user's agents.
function agentsListToRoster(data) {
    if (!data || !data.agents || data.agents.length === 0) return [];
    return data.agents.map(function (a) {
        var idle = (typeof a.idle_seconds === "number") ? a.idle_seconds : null;
        return {
            name: a.name || "agent",
            tier: (typeof a.tier === "number") ? a.tier : 0,
            awaitingApproval: a.awaiting_approval === true,
            activity: a.activity || "idle",
            project: "",                                   // not derivable yet
            elapsed: idle !== null ? ("actif il y a " + idle + "s") : ""
        };
    });
}
