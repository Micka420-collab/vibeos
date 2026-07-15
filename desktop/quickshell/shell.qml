// VibeOS HUD — Quickshell root (launch with: quickshell -c vibeos)
//
// THE signature surface of VibeOS: a slim, frosted-glass top bar layered ON TOP
// of KDE Plasma 6 (it never replaces the Plasma panel — DESKTOP.md §2.4). It is
// the vitrine of the design system: glass, tiers, the Mauve accent, mono data and
// measured motion, all sourced from Theme.* (DESIGN-SYSTEM.md §11.4). It answers
// three questions at a glance — the vibecoding triptych:
//   1. WHO is working, at what policy tier            -> AgentStatus
//   2. Is a T2/T3 action WAITING for me               -> global state + tier locks
//   3. What is local inference doing (model / VRAM)   -> OllamaGauge
//
// ---------------------------------------------------------------------------
// DATA SOURCE — read this before touching anything (honesty rule)
// ---------------------------------------------------------------------------
// Current state (this file, shipped): the LIVE vibed client is WIRED. This QML
// opens a Quickshell.Io Socket on /run/vibed/mcp.sock (line-delimited JSON-RPC
// 2.0, one object per line — the transport of vibed/src/mcp.rs) and drives every
// value from real T0 tool responses — see the `Socket` + `pollVibed()` +
// `handleVibedLine()` below (os.status, memory.query, agents.list,
// agent.sessions -> agent.thinking). `vibedOnline` is DRIVEN by the socket
// connection state, NOT hardwired: when vibed is unreachable (daemon down, or
// the user is not in the vibeos-agents group) the HUD renders its honest OFFLINE
// state — never a crash, never fake "live" data (DESKTOP.md §6). The OllamaGauge
// keeps its own local probe (ollama HTTP + nvidia-smi), independent of vibed.
//
// NOT YET VALIDATED VISUALLY: this wiring has never run on a booted Plasma +
// Quickshell (no headless path — BLOCKERS.md). The remaining Phase 2 work is the
// on-machine visual check; the dead `mock*` scaffolding has been removed from
// vibed_client.js. Request/response shapes live in vibed_client.js and MUST stay
// in sync with vibed/src/mcp.rs.
//
// Install target (image build, read-only at runtime):
//   /usr/share/vibeos/quickshell/shell.qml
// ---------------------------------------------------------------------------

import QtQuick
import QtQuick.Layouts
import QtQuick.Shapes
import Quickshell
import Quickshell.Io
import "vibed_client.js" as Vibed

ShellRoot {
    id: root

    // ----- HUD state — LIVE from vibed over /run/vibed/mcp.sock -----
    // vibedOnline is driven by the socket connection state (below): when vibed
    // is unreachable — daemon down, or the user is not in the vibeos-agents
    // group — the HUD renders its honest OFFLINE state and never fake "live"
    // data (DESKTOP.md §6, graceful degradation). Empty initial values, filled
    // only from real tool responses.
    property bool vibedOnline: false
    property var osStatus: ({})
    property var memoryStatus: ({})
    // Filled from the agents.list T0 tool (see handleVibedLine). Stays [] while
    // offline or before any agent has registered, so the panel shows offline.
    property var agents: []
    // ollama gauge has its OWN local probe (below), independent of vibed — it
    // must keep working when vibed is offline. Honest default: unavailable.
    property var ollama: ({ available: false })
    // Live reasoning of the most recent autonomous session (agent.sessions ->
    // agent.thinking). [] until a session exists; ReasoningPanel shows offline.
    property var reasoningLive: []
    property string reasoningSession: ""
    // Past sessions (agent.sessions -> sessionsToHistory), newest first. [] while
    // offline or before any session has been captured. Republished on every poll:
    // ReasoningPanel diffs it into a ListModel in place, so a fresh array costs
    // nothing and the user's scroll survives (see ReasoningPanel.applyHistory).
    property var reasoningHistory: []
    property string reasoningHistoryNote: ""
    // Reasoning of the history entry the user picked, fetched on demand — the
    // poll loop never tails a past session, only the live one.
    property var reasoningSelected: []
    property string reasoningSelectedId: ""
    property bool reasoningSelectedPending: false
    // Why the look-back failed ("" = no failure). Without it the panel would
    // render a failure as "this session captured nothing" — a fabricated fact.
    property string reasoningSelectedError: ""
    // JSON-RPC id of the in-flight look-back. The poll also calls agent.thinking,
    // so the reply is claimed by REQUEST id, never by tool name or session id.
    property int reasoningSelectedReqId: -1

    // Derived global state (DESIGN-SYSTEM §11.4 "État global"):
    //   offline (gray) -> ready -> agents active (mauve) -> approval waiting (peach pulse).
    readonly property int activeAgents: vibedOnline && agents ? agents.length : 0
    readonly property bool anyAwaiting: {
        if (!vibedOnline || !agents) return false
        for (var i = 0; i < agents.length; ++i)
            if (agents[i].awaitingApproval === true && agents[i].tier >= 2) return true
        return false
    }

    // ----- Live vibed client -------------------------------------------------
    // A line-delimited JSON-RPC 2.0 client on the MCP socket. The HUD is
    // STRICTLY AN OBSERVER: it only ever calls T0 read-only tools (os.status,
    // memory.query, agent.sessions, agent.thinking) — never a T1+ tool, never
    // an approval. Request/response shapes live in vibed_client.js.
    Socket {
        id: vibedSocket
        path: Vibed.SOCKET_PATH
        connected: true
        parser: SplitParser { onRead: function (line) { root.handleVibedLine(line) } }
        onConnectionStateChanged: {
            root.vibedOnline = vibedSocket.connected
            if (vibedSocket.connected) {
                vibedSocket.write(Vibed.initializeRequest())
                vibedSocket.write(Vibed.initializedNotification())
                root.pollVibed()
            } else {
                // Lost the daemon: drop to the honest offline state, keep no
                // stale "live" data around. The history goes too — it is a view
                // of a store we can no longer read, not a local cache.
                root.reasoningLive = []
                root.reasoningSession = ""
                root.reasoningHistory = []
                root.reasoningHistoryNote = ""
                root.reasoningSelected = []
                root.reasoningSelectedId = ""
                root.reasoningSelectedPending = false
                root.reasoningSelectedError = ""
                root.reasoningSelectedReqId = -1
            }
        }
    }

    // Poll the read-only tools. os.status + memory.query every tick; discover a
    // reasoning session (agent.sessions) and, if one exists, tail its reasoning
    // (agent.thinking). Observer-only cadence, never a write path.
    function pollVibed() {
        if (!vibedSocket.connected) return
        vibedSocket.write(Vibed.toolsCallRequest("os.status", {}))
        vibedSocket.write(Vibed.toolsCallRequest("memory.query", { query: "" }))
        vibedSocket.write(Vibed.toolsCallRequest("agents.list", {}))
        vibedSocket.write(Vibed.toolsCallRequest("agent.sessions", {}))
        if (root.reasoningSession !== "")
            vibedSocket.write(Vibed.toolsCallRequest("agent.thinking",
                { session_id: root.reasoningSession, tail: 40 }))
    }

    // Fetch one past session's reasoning, on user selection only. Deliberately
    // NOT part of pollVibed: the history list is already fully rendered from
    // agent.sessions metadata, so a session is read only when someone asks to
    // read it. A larger tail than the live view — this is a look back, not a
    // 5s-refresh — still bounded by vibed (REASONING_MAX_TAIL).
    function requestSessionReasoning(sessionId) {
        root.reasoningSelectedId = sessionId
        root.reasoningSelected = []
        root.reasoningSelectedReqId = -1
        root.reasoningSelectedError = ""      // a new selection starts clean
        root.reasoningSelectedPending = sessionId !== ""
        if (!vibedSocket.connected || sessionId === "") {
            // Nothing will ever answer, so do not leave the panel spinning.
            root.reasoningSelectedPending = false
            // Offline with a real selection: say so rather than let the panel
            // conclude the session captured nothing.
            if (sessionId !== "")
                root.reasoningSelectedError = "vibed est hors ligne"
            return
        }
        const req = Vibed.toolsCallRequestId("agent.thinking",
            { session_id: sessionId, tail: 200 })
        root.reasoningSelectedReqId = req.id
        vibedSocket.write(req.line)
    }

    Timer {   // steady poll while online
        interval: 5000; running: root.vibedOnline; repeat: true
        onTriggered: root.pollVibed()
    }
    Timer {   // reconnect probe while offline — degradation stays graceful
        interval: 4000; running: !root.vibedOnline; repeat: true
        onTriggered: vibedSocket.connected = true
    }

    // Route each socket line to the right piece of HUD state. A policy denial or
    // error keeps the last good state (never a crash, never fake data).
    function handleVibedLine(line) {
        const msg = Vibed.parseLine(line); if (!msg) return
        const res = Vibed.parseToolResult(msg)
        if (!res.ok) {
            // A failed look-back (policy denial, rate limit, protocol error,
            // malformed result) must stop the spinner — otherwise the panel
            // claims to be loading forever. And it must RECORD WHY: without an
            // error state the panel falls through to "this session captured
            // nothing", which is a fabricated fact about the agent's history.
            if (res.id === root.reasoningSelectedReqId) {
                root.reasoningSelectedPending = false
                root.reasoningSelectedReqId = -1
                root.reasoningSelectedError = res.error || "erreur inconnue"
            }
            return
        }
        if (res.tool === "os.status") {
            root.osStatus = res.data
        } else if (res.tool === "memory.query") {
            root.memoryStatus = res.data
        } else if (res.tool === "agents.list") {
            root.agents = Vibed.agentsListToRoster(res.data)
        } else if (res.tool === "agent.sessions") {
            // Follow the most recent session; clear reasoning when none.
            const latest = (res.data && res.data.latest) ? res.data.latest : ""
            if (latest !== root.reasoningSession) {
                root.reasoningSession = latest
                if (latest === "") root.reasoningLive = []
            }
            // The same payload already carries every session's metadata, so the
            // history renders without one agent.thinking call per session.
            // Republished unconditionally: ReasoningPanel diffs it into a
            // ListModel and only touches the rows that moved, so the view is
            // never reset and the user's scroll survives.
            root.reasoningHistory = Vibed.sessionsToHistory(res.data)
            root.reasoningHistoryNote = Vibed.sessionsTruncationNote(res.data)
        } else if (res.tool === "agent.thinking") {
            // Both the poll (live session, small tail) and a history look-back
            // (chosen session, larger tail) land here on the same socket. Claim
            // the reply by REQUEST id: routing on session_id would let the poll's
            // 40-line answer overwrite the look-back's 200 lines whenever the
            // user selects the session that happens to be live.
            if (res.id === root.reasoningSelectedReqId) {
                root.reasoningSelected = Vibed.reasoningToLive(res.data)
                root.reasoningSelectedPending = false
                root.reasoningSelectedError = ""   // it answered: no failure
            }
            const sid = (res.data && res.data.session_id) ? res.data.session_id : ""
            if (sid !== "" && sid === root.reasoningSession)
                root.reasoningLive = Vibed.reasoningToLive(res.data)
        }
    }

    // ----- Local ollama + VRAM probe (independent of vibed) ------------------
    // The gauge must keep working when vibed is offline, so it reads two LOCAL,
    // read-only sources directly: ollama's HTTP API for model presence, and
    // nvidia-smi for VRAM. Any failure degrades to available:false (never fake).
    // NOTE: not runtime-verifiable here (no Quickshell/ollama/GPU); built to the
    // documented Phase 2 sources (OllamaGauge.qml header).
    function probeOllama() {
        const xhr = new XMLHttpRequest()
        xhr.onreadystatechange = function () {
            if (xhr.readyState !== XMLHttpRequest.DONE) return
            const prev = root.ollama || {}
            let next = {
                available: false,
                model: "",
                generating: false,
                vram_used_mb: prev.vram_used_mb || 0,
                vram_total_mb: prev.vram_total_mb || 0
            }
            if (xhr.status === 200) {
                try {
                    const j = JSON.parse(xhr.responseText)
                    const models = (j && j.models) || []
                    next.available = true
                    if (models.length > 0) next.model = models[0].name || ""
                } catch (e) { /* malformed -> stay unavailable */ }
            }
            root.ollama = next
        }
        // ollama not running -> connection refused -> onreadystatechange with
        // status 0 -> available:false. Wrapped so a throw can't break the HUD.
        try {
            xhr.open("GET", "http://127.0.0.1:11434/api/ps")
            xhr.send()
        } catch (e) {
            root.ollama = { available: false }
        }
    }

    Process {
        id: vramProbe
        command: ["nvidia-smi", "--query-gpu=memory.used,memory.total",
                  "--format=csv,noheader,nounits"]
        running: false
        stdout: SplitParser {
            onRead: function (line) {
                const parts = line.split(",")
                if (parts.length < 2) return
                const used = parseInt(parts[0])
                const total = parseInt(parts[1])
                if (isNaN(used) || isNaN(total)) return
                const o = root.ollama || {}
                root.ollama = {
                    available: o.available === true,
                    model: o.model || "",
                    generating: o.generating === true,
                    vram_used_mb: used,
                    vram_total_mb: total
                }
            }
        }
    }

    Timer {   // poll the local probes; independent of the vibed connection
        interval: 5000; running: true; repeat: true
        triggeredOnStart: true
        onTriggered: {
            root.probeOllama()
            if (!vramProbe.running) vramProbe.running = true
        }
    }

    PanelWindow {
        id: bar

        // Top edge, full width. The Plasma panel keeps its own edge; the HUD is
        // an additional strut-reserving layer, not a replacement (DESKTOP §2.4).
        anchors { top: true; left: true; right: true }
        // Window is a touch taller than the visible bar: the extra transparent
        // band lets the elevation shadow bleed over the content below.
        implicitHeight: Theme.barHeight + Theme.barShadowPad
        exclusiveZone: Theme.barHeight
        color: "transparent"

        // ---- Elevation shadow: soft gradient bleeding below the glass bar ----
        Rectangle {
            anchors { top: barSurface.bottom; left: parent.left; right: parent.right }
            height: Theme.barShadowPad
            gradient: Gradient {
                GradientStop { position: 0.0; color: Theme.shadow2 }
                GradientStop { position: 1.0; color: "transparent" }
            }
        }

        // ---- The frosted-glass bar surface (glass-panel: Crust 66% + blur-heavy) ----
        // Real backdrop blur is applied by KWin behind this translucent layer
        // surface; opaque fallback under reduced-transparency (DESIGN-SYSTEM §6.2).
        Rectangle {
            id: barSurface
            anchors { top: parent.top; left: parent.left; right: parent.right }
            height: Theme.barHeight
            color: Theme.reducedTransparency ? Theme.surface_1 : Theme.glassPanelBg

            // specular top arris (1px highlight) + hairline bottom edge (glass, §2.4)
            Rectangle { anchors { top: parent.top; left: parent.left; right: parent.right }
                        height: 1; color: Theme.glassEdgeTop }
            Rectangle { anchors { bottom: parent.bottom; left: parent.left; right: parent.right }
                        height: 1; color: Theme.hairline }

            RowLayout {
                anchors.fill: parent
                anchors.leftMargin: Theme.space4
                anchors.rightMargin: Theme.space4
                spacing: Theme.space3

                // ===== LEFT: brand + global state =====

                // ---- Brand mark: signature ring (Mauve->Blue annulus) ----
                Item {
                    id: brand
                    Layout.alignment: Qt.AlignVCenter
                    implicitWidth: 14; implicitHeight: 14
                    Shape {
                        anchors.fill: parent
                        preferredRendererType: Shape.CurveRenderer
                        antialiasing: true
                        ShapePath {
                            fillRule: ShapePath.OddEvenFill
                            strokeWidth: -1
                            fillGradient: LinearGradient {
                                x1: 0; y1: 0; x2: brand.width; y2: brand.height
                                GradientStop { position: 0.0; color: Theme.mauve }
                                GradientStop { position: 1.0; color: Theme.blue }
                            }
                            PathAngleArc { centerX: brand.width / 2; centerY: brand.height / 2
                                radiusX: brand.width / 2; radiusY: brand.height / 2
                                startAngle: 0; sweepAngle: 360; moveToStart: true }
                            PathAngleArc { centerX: brand.width / 2; centerY: brand.height / 2
                                radiusX: brand.width / 2 - 2.5; radiusY: brand.height / 2 - 2.5
                                startAngle: 0; sweepAngle: 360; moveToStart: true }
                        }
                    }
                }

                // ---- Wordmark ----
                Text {
                    Layout.alignment: Qt.AlignVCenter
                    text: "VibeOS"
                    color: Theme.mauve
                    font.family: Theme.fontSans
                    font.pixelSize: Theme.fsBody + 1
                    font.weight: Theme.weightSemibold
                    font.letterSpacing: -0.2
                }

                // ---- Global state pill (offline / ready / active / awaiting) ----
                Rectangle {
                    id: statePill
                    Layout.alignment: Qt.AlignVCenter
                    implicitWidth: stateRow.implicitWidth + Theme.space3
                    implicitHeight: Theme.rowControl
                    radius: Theme.radiusFull
                    color: Theme.surface_3
                    border.width: 1
                    border.color: Theme.hairline

                    // state colour: gray -> mauve (active) -> peach (awaiting)
                    readonly property color stateColor: !root.vibedOnline ? Theme.tierOffline
                                                      : root.anyAwaiting ? Theme.peach
                                                      : root.activeAgents > 0 ? Theme.mauve
                                                      : Theme.overlay2

                    // Slow attention pulse only when an approval is waiting (§8.3).
                    property real pulse: 1.0
                    SequentialAnimation on pulse {
                        running: root.anyAwaiting && !Theme.reducedMotion
                        loops: Animation.Infinite
                        NumberAnimation { from: 1.0; to: 0.5; duration: Theme.durPulse / 2
                            easing.type: Easing.Bezier; easing.bezierCurve: Theme.easeStandard }
                        NumberAnimation { from: 0.5; to: 1.0; duration: Theme.durPulse / 2
                            easing.type: Easing.Bezier; easing.bezierCurve: Theme.easeStandard }
                    }

                    Row {
                        id: stateRow
                        anchors.centerIn: parent
                        spacing: Theme.space2

                        // status dot + soft bloom when active/awaiting
                        Item {
                            anchors.verticalCenter: parent.verticalCenter
                            width: 8; height: 8
                            Rectangle {  // bloom
                                anchors.centerIn: parent
                                width: 14; height: 14; radius: 7
                                color: statePill.stateColor
                                visible: root.vibedOnline && (root.anyAwaiting || root.activeAgents > 0)
                                opacity: Theme.reducedMotion ? 0.3 : (statePill.pulse - 0.5) * 0.6
                            }
                            Rectangle {  // dot
                                anchors.centerIn: parent
                                width: 8; height: 8; radius: 4
                                color: statePill.stateColor
                            }
                        }
                        Text {
                            anchors.verticalCenter: parent.verticalCenter
                            text: !root.vibedOnline ? "vibed hors ligne"
                                : root.anyAwaiting ? "approbation requise"
                                : root.activeAgents > 0 ? (root.activeAgents + " agents actifs")
                                : "prêt"
                            color: !root.vibedOnline ? Theme.textMuted : Theme.textSecondary
                            font.family: Theme.fontMono
                            font.pixelSize: Theme.fsMonoSm
                        }
                    }
                }

                // ---- hairline divider ----
                Rectangle { Layout.alignment: Qt.AlignVCenter; width: 1; height: 16; color: Theme.hairline }

                // ===== CENTRE: agents (fills the bar) =====
                AgentStatus {
                    Layout.fillWidth: true
                    Layout.alignment: Qt.AlignVCenter
                    agents: root.agents
                    online: root.vibedOnline
                }

                // ---- hairline divider ----
                Rectangle { Layout.alignment: Qt.AlignVCenter; width: 1; height: 16; color: Theme.hairline }

                // ---- reasoning (3rd pillar "why" — Phase 2.5) ----
                // LIVE: fed from agent.sessions -> agent.thinking over the vibed
                // socket (tapped from the CLI stream, never the CLI transcript —
                // docs/DECISIONS.md ADR-012). `live` is [] until an autonomous
                // session has captured reasoning; the chip then shows offline.
                // HISTORY: the same agent.sessions payload carries each session's
                // metadata; picking one fetches its reasoning on demand.
                ReasoningPanel {
                    Layout.alignment: Qt.AlignVCenter
                    online: root.vibedOnline
                    live: root.reasoningLive
                    history: root.reasoningHistory
                    historyNote: root.reasoningHistoryNote
                    selected: root.reasoningSelected
                    selectedLoading: root.reasoningSelectedPending
                    selectedError: root.reasoningSelectedError
                    onHistorySelected: function (sessionId) {
                        root.requestSessionReasoning(sessionId)
                    }
                }

                // ---- hairline divider ----
                Rectangle { Layout.alignment: Qt.AlignVCenter; width: 1; height: 16; color: Theme.hairline }

                // ===== RIGHT: resources (ollama + memory + load) =====
                OllamaGauge {
                    Layout.alignment: Qt.AlignVCenter
                    ollama: root.ollama
                }

                Rectangle { Layout.alignment: Qt.AlignVCenter; width: 1; height: 16; color: Theme.hairline }

                // memory (memory.query, T0) — "—" offline, never a fake count
                Column {
                    Layout.alignment: Qt.AlignVCenter
                    spacing: 1
                    Text {
                        text: "MÉMOIRE"
                        color: Theme.textMuted
                        font.family: Theme.fontSans
                        font.pixelSize: Theme.fsCaption - 1
                        font.weight: Theme.weightMedium
                        font.letterSpacing: 0.6
                    }
                    Text {
                        text: !root.vibedOnline ? "—"
                            : (root.memoryStatus && root.memoryStatus.initialized
                                ? (root.memoryStatus.scanned_files + " fichiers")
                                : "non initialisée")
                        color: root.vibedOnline ? Theme.textSecondary : Theme.textMuted
                        font.family: Theme.fontMono
                        font.pixelSize: Theme.fsMonoSm
                    }
                }

                // system load (os.status, T0) — "—" offline
                Column {
                    Layout.alignment: Qt.AlignVCenter
                    spacing: 1
                    Text {
                        text: "CHARGE"
                        color: Theme.textMuted
                        font.family: Theme.fontSans
                        font.pixelSize: Theme.fsCaption - 1
                        font.weight: Theme.weightMedium
                        font.letterSpacing: 0.6
                    }
                    Text {
                        text: {
                            if (!root.vibedOnline) return "—"
                            const s = root.osStatus
                            return (s && s.loadavg_1_5_15 && s.loadavg_1_5_15.length > 0)
                                ? s.loadavg_1_5_15[0] : "—"
                        }
                        color: root.vibedOnline ? Theme.textSecondary : Theme.textMuted
                        font.family: Theme.fontMono
                        font.pixelSize: Theme.fsMonoSm
                    }
                }
            }
        }
    }
}
