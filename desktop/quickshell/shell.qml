// VibeOS HUD — Quickshell root (launch with: quickshell -c vibeos)
//
// A thin, always-visible top bar layered ON TOP of KDE Plasma 6 (it never
// replaces the Plasma panel). It answers three questions at a glance:
//   1. which agents are working, and at what policy tier (AgentStatus)
//   2. is a T2/T3 action waiting for human approval (PolicyTierIndicator lock)
//   3. what is the local model / VRAM doing (OllamaGauge)
//
// ---------------------------------------------------------------------------
// DATA SOURCE — read this before touching anything
// ---------------------------------------------------------------------------
// v0.1 (this file, shipped): every value below comes from the MOCK functions
// in vibed_client.js. `vibedOnline` is hardwired to false because in Phase 1
// /usr/bin/vibed is not in the image yet. The HUD must render a clean
// "daemon offline" state — never crash, never show fake "live" data.
//
// TODO(Phase 2): replace the mocks with a real client on the vibed MCP socket
// /run/vibed/mcp.sock (line-delimited JSON-RPC 2.0, one JSON object per line,
// exactly the transport of vibed/src/mcp.rs). Intended wiring, kept here as
// the reference sketch:
//
//   import Quickshell.Io
//
//   Socket {
//       id: vibedSocket
//       path: "/run/vibed/mcp.sock"          // root:vibeos-agents 0660 —
//       connected: true                       // session user must be in the
//       parser: SplitParser {                 // vibeos-agents group
//           onRead: line => root.handleVibedLine(line)
//       }
//       onConnectionStateChanged: {
//           root.vibedOnline = connected
//           if (connected) {
//               write(Vibed.initializeRequest())
//               write(Vibed.initializedNotification())
//           }
//       }
//   }
//   Timer { // poll T0 tools; both are read-only, allow-by-default tier T0
//       interval: 5000; running: root.vibedOnline; repeat: true
//       onTriggered: {
//           vibedSocket.write(Vibed.toolsCallRequest("os.status", {}))
//           vibedSocket.write(Vibed.toolsCallRequest("memory.query", { query: "" }))
//       }
//   }
//   (plus a reconnect timer while offline — degradation must stay graceful)
//
// The exact request/response shapes live in vibed_client.js and MUST stay in
// sync with vibed/src/mcp.rs.
// ---------------------------------------------------------------------------

import QtQuick
import QtQuick.Layouts
import Quickshell
import "vibed_client.js" as Vibed

ShellRoot {
    id: root

    // ----- VibeOS Dark palette (fork of Catppuccin Mocha, MIT) -----
    readonly property color colBase: "#1e1e2e"
    readonly property color colMantle: "#181825"
    readonly property color colSurface: "#313244"
    readonly property color colText: "#cdd6f4"
    readonly property color colMuted: "#a6adc8"
    readonly property color colAccent: "#cba6f7"
    readonly property color colGood: "#a6e3a1"
    readonly property color colOffline: "#7f849c"

    // ----- HUD state (v0.1: MOCK — see header comment for Phase 2) -----
    // Hardwired offline in v0.1: honest about Phase 1 (no /usr/bin/vibed).
    // Design preview only: flip to true to see the mocked "online" layout
    // (agent chips, memory pill, load). Never ship it as true.
    property bool vibedOnline: false
    property var osStatus: Vibed.mockOsStatus()
    property var memoryStatus: Vibed.mockMemoryQuery()
    property var agents: Vibed.mockAgents()
    property var ollama: Vibed.mockOllama()

    // v0.1 demo heartbeat: refreshes the mocks so the prototype feels alive.
    // Purely cosmetic; deleted when the Phase 2 socket wiring lands.
    Timer {
        interval: 5000
        running: true
        repeat: true
        onTriggered: {
            root.osStatus = Vibed.mockOsStatus()
            root.memoryStatus = Vibed.mockMemoryQuery()
            root.agents = Vibed.mockAgents()
            root.ollama = Vibed.mockOllama()
        }
    }

    // TODO(Phase 2): parse incoming socket lines here.
    // function handleVibedLine(line) {
    //     const msg = Vibed.parseLine(line)
    //     if (!msg) return
    //     ... route by msg.id to os.status / memory.query state updates,
    //     using Vibed.parseToolResult(msg) to unwrap the MCP content envelope.
    // }

    PanelWindow {
        id: bar

        // Top edge, full width. The Plasma panel keeps its own edge; the HUD
        // is an additional strut-reserving layer, not a replacement.
        anchors {
            top: true
            left: true
            right: true
        }
        implicitHeight: 34
        color: "transparent"

        Rectangle {
            anchors.fill: parent
            color: root.colMantle    // chrome = Mantle (palette.md §4)
            // subtle bottom hairline so the bar reads as a distinct layer
            Rectangle {
                anchors.bottom: parent.bottom
                width: parent.width
                height: 1
                color: root.colSurface
            }

            RowLayout {
                anchors.fill: parent
                anchors.leftMargin: 12
                anchors.rightMargin: 12
                spacing: 14

                // ----- wordmark -----
                Text {
                    text: "VibeOS"
                    color: root.colAccent
                    font.bold: true
                    font.pixelSize: 13
                }

                // ----- daemon status pill (graceful-degradation anchor) -----
                Rectangle {
                    implicitWidth: daemonRow.implicitWidth + 16
                    implicitHeight: 20
                    radius: 10
                    color: root.colSurface
                    Row {
                        id: daemonRow
                        anchors.centerIn: parent
                        spacing: 6
                        Rectangle {
                            width: 8; height: 8; radius: 4
                            anchors.verticalCenter: parent.verticalCenter
                            color: root.vibedOnline ? root.colGood : root.colOffline
                        }
                        Text {
                            text: root.vibedOnline ? "vibed" : "vibed hors ligne"
                            color: root.vibedOnline ? root.colText : root.colMuted
                            font.pixelSize: 11
                        }
                    }
                }

                // ----- agents + policy tiers -----
                AgentStatus {
                    Layout.fillWidth: true
                    Layout.alignment: Qt.AlignVCenter
                    agents: root.agents
                    online: root.vibedOnline
                }

                // ----- ollama / GPU gauge -----
                OllamaGauge {
                    Layout.alignment: Qt.AlignVCenter
                    ollama: root.ollama
                }

                // ----- memory pill (memory.query, T0) -----
                Rectangle {
                    implicitWidth: memRow.implicitWidth + 16
                    implicitHeight: 20
                    radius: 10
                    color: root.colSurface
                    Row {
                        id: memRow
                        anchors.centerIn: parent
                        spacing: 6
                        Text {
                            text: "mem"
                            color: root.colMuted
                            font.pixelSize: 11
                        }
                        Text {
                            // memory.query result: { initialized, scanned_files }
                            // Offline => "—" (never pretend to know).
                            text: !root.vibedOnline ? "—"
                                : (root.memoryStatus && root.memoryStatus.initialized
                                    ? root.memoryStatus.scanned_files + " fichiers"
                                    : "non initialisée")
                            color: root.colText
                            font.pixelSize: 11
                        }
                    }
                }

                // ----- system load (os.status, T0) -----
                Text {
                    // os.status result: { loadavg_1_5_15: ["0.42", ...], ... }
                    text: {
                        if (!root.vibedOnline)
                            return "load —"
                        const s = root.osStatus
                        return (s && s.loadavg_1_5_15 && s.loadavg_1_5_15.length > 0)
                            ? "load " + s.loadavg_1_5_15[0]
                            : "load —"
                    }
                    color: root.colMuted
                    font.pixelSize: 11
                }
            }
        }
    }
}
