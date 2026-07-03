// AgentStatus — the "who is working" panel of the triptych: running agents as
// elevated chips. Each chip carries the VibeOS signature ring (Mauve->Blue
// avatar, DESIGN-SYSTEM §11.4), the agent name (sans, body-strong), its current
// policy tier (PolicyTierIndicator), and — on hover — a glass tooltip with the
// live activity line. All spacing/colour/motion comes from Theme (no magic values).
//
// Model shape (one entry per agent, from vibed_client.js -> Phase 2 vibed):
//   { name: "claude-code",       // agent binary / CLI name (avatar = initial)
//     tier: 1,                   // current policy tier 0..3 = HIGHEST in-flight call
//     awaitingApproval: false,   // true when a T2/T3 call awaits human approval (lock)
//     activity: "editing files", // short status line (tooltip)
//     project: "~/projects/app", // optional: working dir (tooltip)
//     elapsed: "4m12s" }         // optional: session age (tooltip)
//
// ---------------------------------------------------------------------------
// DATA SOURCE — v0.1 vs Phase 2 (honesty rule)
// ---------------------------------------------------------------------------
// v0.1: MOCK data from vibed_client.js (mockAgents()). vibed v0.1 exposes NO
// tool that lists connected agents — the roster is not derivable from the daemon
// yet, and we say so (offline placeholder) instead of faking a wire.
// TODO(Phase 2): feed from vibed over /run/vibed/mcp.sock — an `agents.list` T0
// tool, or an audit-derived stream keyed by caller pid (SO_PEERCRED). Same path
// as os.status, through vibed_client.js.
// ---------------------------------------------------------------------------

import QtQuick
import QtQuick.Layouts
import QtQuick.Shapes
import QtQuick.Effects

Item {
    id: agentStatus

    // [{ name, tier, awaitingApproval, activity, project, elapsed }] — see header.
    property var agents: []
    // false while vibed is unreachable: show a neutral placeholder, no chips.
    property bool online: false

    implicitHeight: 26
    implicitWidth: row.implicitWidth

    RowLayout {
        id: row
        anchors.verticalCenter: parent.verticalCenter
        spacing: Theme.space2

        // ---- Offline / empty placeholder — the graceful-degradation path ----
        RowLayout {
            visible: !agentStatus.online || agentStatus.agents.length === 0
            spacing: Theme.space2
            // small muted dot echoing the daemon status
            Rectangle {
                Layout.alignment: Qt.AlignVCenter
                width: 6; height: 6; radius: 3
                color: Theme.tierOffline
            }
            Text {
                Layout.alignment: Qt.AlignVCenter
                text: agentStatus.online
                    ? "aucun agent actif"
                    : "agents — en attente de vibed (Phase 2)"
                color: Theme.textMuted
                font.family: Theme.fontSans
                font.pixelSize: Theme.fsSmall
                font.italic: true
            }
        }

        // ---- Live agent chips ----
        Repeater {
            model: agentStatus.online ? agentStatus.agents : []

            delegate: Rectangle {
                id: chip
                required property var modelData

                readonly property bool awaiting: chip.modelData.awaitingApproval === true

                Layout.alignment: Qt.AlignVCenter
                implicitWidth:  chipRow.implicitWidth + Theme.space3
                implicitHeight: 26
                radius: Theme.radiusFull

                // Surface lifts on hover: surface-3 -> surface-4 (state layer, §2.5).
                color: hover.hovered ? Theme.surface_4 : Theme.surface_3
                border.width: 1
                border.color: hover.hovered ? Theme.borderSubtle : Theme.hairline
                // Subtle rise on hover — 8px grammar reduced to a 1px lift.
                y: hover.hovered ? -1 : 0

                Behavior on color  { ColorAnimation { duration: Theme.durFast
                                     easing.type: Easing.Bezier; easing.bezierCurve: Theme.easeStandard } }
                Behavior on y      { NumberAnimation { duration: Theme.durFast
                                     easing.type: Easing.Bezier; easing.bezierCurve: Theme.easeDecelerate } }

                // Soft elevation shadow, deepening on hover (elevation-1 -> 2, §5).
                layer.enabled: true
                layer.effect: MultiEffect {
                    shadowEnabled: true
                    shadowColor: chip.hover.hovered ? Theme.shadow2 : Theme.shadow1
                    shadowBlur: chip.hover.hovered ? Theme.elevBlur2 : Theme.elevBlur1
                    shadowVerticalOffset: chip.hover.hovered ? Theme.elevY2 : Theme.elevY1
                    autoPaddingEnabled: true
                    Behavior on shadowBlur { NumberAnimation { duration: Theme.durFast } }
                }

                HoverHandler { id: hover }

                Row {
                    id: chipRow
                    anchors.centerIn: parent
                    spacing: Theme.space2

                    // ---- Avatar: signature ring (Mauve->Blue) + initial ----
                    Item {
                        id: avatar
                        anchors.verticalCenter: parent.verticalCenter
                        width: 18; height: 18

                        Shape {
                            anchors.fill: parent
                            preferredRendererType: Shape.CurveRenderer
                            antialiasing: true
                            ShapePath {
                                fillRule: ShapePath.OddEvenFill
                                strokeWidth: -1
                                fillGradient: LinearGradient {  // grad-signature 135deg (§7)
                                    x1: 0; y1: 0; x2: avatar.width; y2: avatar.height
                                    GradientStop { position: 0.0; color: Theme.mauve }
                                    GradientStop { position: 1.0; color: Theme.blue }
                                }
                                PathAngleArc {
                                    centerX: avatar.width / 2; centerY: avatar.height / 2
                                    radiusX: avatar.width / 2; radiusY: avatar.height / 2
                                    startAngle: 0; sweepAngle: 360; moveToStart: true
                                }
                                PathAngleArc {
                                    centerX: avatar.width / 2; centerY: avatar.height / 2
                                    radiusX: avatar.width / 2 - 2; radiusY: avatar.height / 2 - 2
                                    startAngle: 0; sweepAngle: 360; moveToStart: true
                                }
                            }
                        }
                        // Agent initial, centred (mono, honest machine label).
                        Text {
                            anchors.centerIn: parent
                            text: (chip.modelData.name || "?").charAt(0).toUpperCase()
                            color: Theme.textPrimary
                            font.family: Theme.fontMono
                            font.pixelSize: 9
                            font.weight: Theme.weightSemibold
                        }
                    }

                    // ---- Agent name — sans, body-strong ----
                    Text {
                        anchors.verticalCenter: parent.verticalCenter
                        text: chip.modelData.name
                        color: Theme.textPrimary
                        font.family: Theme.fontSans
                        font.pixelSize: Theme.fsSmall
                        font.weight: Theme.weightMedium
                    }

                    // ---- Current policy tier (+ lock/pulse when awaiting) ----
                    PolicyTierIndicator {
                        anchors.verticalCenter: parent.verticalCenter
                        tier: chip.modelData.tier
                        awaitingApproval: chip.awaiting
                    }
                }

                // ---- Glass tooltip: activity/project/elapsed on hover (§6/§11) ----
                Rectangle {
                    id: tip
                    visible: opacity > 0.001
                    opacity: hover.hovered ? 1.0 : 0.0
                    anchors.top: parent.bottom
                    anchors.topMargin: Theme.space2
                    anchors.horizontalCenter: parent.horizontalCenter
                    implicitWidth:  Math.max(tipCol.implicitWidth + Theme.space4, 140)
                    implicitHeight: tipCol.implicitHeight + Theme.space3
                    radius: Theme.radiusLg
                    color: Theme.reducedTransparency ? Theme.surface_1 : Theme.glassElevatedBg
                    border.width: 1
                    border.color: Theme.borderSubtle
                    z: 100
                    // Entrance: fade + slight settle (decelerate, §8.3).
                    transform: Translate { y: hover.hovered ? 0 : -4 }
                    Behavior on opacity { NumberAnimation { duration: Theme.durBase
                                          easing.type: Easing.Bezier; easing.bezierCurve: Theme.easeDecelerate } }

                    // specular top edge — the glass "arris"
                    Rectangle {
                        anchors { top: parent.top; left: parent.left; right: parent.right }
                        anchors.margins: 1
                        height: 1
                        color: Theme.glassEdgeTop
                    }

                    Column {
                        id: tipCol
                        anchors.centerIn: parent
                        width: parent.width - Theme.space4
                        spacing: Theme.space1

                        Text {
                            width: parent.width
                            text: chip.modelData.name
                            color: Theme.textPrimary
                            font.family: Theme.fontSans
                            font.pixelSize: Theme.fsSmall
                            font.weight: Theme.weightSemibold
                            elide: Text.ElideRight
                        }
                        Text {
                            width: parent.width
                            text: chip.modelData.activity || "idle"
                            color: chip.awaiting ? Theme.tierColor(chip.modelData.tier) : Theme.textTertiary
                            font.family: Theme.fontMono
                            font.pixelSize: Theme.fsMonoSm
                            wrapMode: Text.WordWrap
                        }
                        Text {
                            width: parent.width
                            visible: (chip.modelData.project || "") !== "" || (chip.modelData.elapsed || "") !== ""
                            text: [chip.modelData.project || "", chip.modelData.elapsed || ""]
                                    .filter(function (s) { return s !== ""; }).join("  ·  ")
                            color: Theme.textMuted
                            font.family: Theme.fontMono
                            font.pixelSize: Theme.fsMonoSm
                            elide: Text.ElideRight
                        }
                    }
                }
            }
        }
    }
}
