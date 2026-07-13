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
// Current state (this file, shipped): every value comes from the MOCK functions
// in vibed_client.js. `vibedOnline` is hardwired false — /usr/bin/vibed IS
// shipped in the image and runs at boot (Phase 2), but this QML does not open
// the socket yet: the live wiring below is the remaining Phase 2 work. The HUD
// MUST render a clean "daemon offline" state:
// never crash, never show fake "live" data (DESKTOP.md §6, graceful degradation).
//
// TODO(Phase 2): replace the mocks with a real client on the vibed MCP socket
// /run/vibed/mcp.sock (line-delimited JSON-RPC 2.0, one object per line — exactly
// the transport of vibed/src/mcp.rs). Reference wiring sketch:
//
//   import Quickshell.Io
//   Socket {
//       id: vibedSocket
//       path: "/run/vibed/mcp.sock"          // root:vibeos-agents 0660 — session
//       connected: true                       // user must be in vibeos-agents group
//       parser: SplitParser { onRead: line => root.handleVibedLine(line) }
//       onConnectionStateChanged: {
//           root.vibedOnline = connected
//           if (connected) { write(Vibed.initializeRequest()); write(Vibed.initializedNotification()) }
//       }
//   }
//   Timer { // poll the two T0 read-only tools; the HUD is strictly an observer
//       interval: 5000; running: root.vibedOnline; repeat: true
//       onTriggered: {
//           vibedSocket.write(Vibed.toolsCallRequest("os.status", {}))
//           vibedSocket.write(Vibed.toolsCallRequest("memory.query", { query: "" }))
//       }
//   }
//   (+ a reconnect timer while offline — degradation must stay graceful)
//
// Request/response shapes live in vibed_client.js and MUST stay in sync with
// vibed/src/mcp.rs.
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
    // No agents.list tool yet — the roster is not derivable from vibed (see
    // AgentStatus.qml). Stays [] until that tool lands; the panel shows offline.
    property var agents: []
    // ollama gauge keeps its own probe (OllamaGauge.qml, ollama API + nvidia-smi),
    // not the vibed socket — honest default is "unavailable".
    property var ollama: Vibed.mockOllama()
    // Live reasoning of the most recent autonomous session (agent.sessions ->
    // agent.thinking). [] until a session exists; ReasoningPanel shows offline.
    property var reasoningLive: []
    property string reasoningSession: ""

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
                // stale "live" data around.
                root.reasoningLive = []
                root.reasoningSession = ""
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
        vibedSocket.write(Vibed.toolsCallRequest("agent.sessions", {}))
        if (root.reasoningSession !== "")
            vibedSocket.write(Vibed.toolsCallRequest("agent.thinking",
                { session_id: root.reasoningSession, tail: 40 }))
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
        if (!res.ok) return
        if (res.tool === "os.status") {
            root.osStatus = res.data
        } else if (res.tool === "memory.query") {
            root.memoryStatus = res.data
        } else if (res.tool === "agent.sessions") {
            // Follow the most recent session; clear reasoning when none.
            const latest = (res.data && res.data.latest) ? res.data.latest : ""
            if (latest !== root.reasoningSession) {
                root.reasoningSession = latest
                if (latest === "") root.reasoningLive = []
            }
        } else if (res.tool === "agent.thinking") {
            root.reasoningLive = Vibed.reasoningToLive(res.data)
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
                // `history` still awaits a listing mode of agent.thinking.
                ReasoningPanel {
                    Layout.alignment: Qt.AlignVCenter
                    online: root.vibedOnline
                    live: root.reasoningLive
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
