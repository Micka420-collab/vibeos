// AgentStatus — running agents and their current policy tier, as chips.
//
// Model shape (one entry per agent):
//   { name: "claude-code",       // agent binary / CLI name
//     tier: 1,                   // current policy tier 0..3 (T0 observe ..
//                                //   T3 destructive) — the HIGHEST tier of
//                                //   its in-flight tool calls
//     awaitingApproval: false,   // true when a T2/T3 call is pending human
//                                //   approval (shows the lock)
//     activity: "editing files" }// short free-form status line (tooltip)
//
// ---------------------------------------------------------------------------
// DATA SOURCE — v0.1 vs Phase 2 (D20 honesty)
// ---------------------------------------------------------------------------
// v0.1: this model is MOCK data from vibed_client.js (mockAgents()). vibed
// v0.1 exposes NO tool that lists connected agents — the roster is not yet
// derivable from the daemon, and we say so instead of faking a wire.
//
// TODO(Phase 2): feed this from vibed over /run/vibed/mcp.sock. Candidate
// designs (to be specified with the vibed track):
//   a) a new T0 tool `agents.list` returning connected peers (vibed already
//      captures SO_PEERCRED uid/gid/pid per connection — see mcp.rs), or
//   b) a notification stream derived from audit records (tool, tier,
//      decision) grouped by caller pid.
// Either way the call goes through vibed_client.js, same as os.status.
// ---------------------------------------------------------------------------

import QtQuick
import QtQuick.Layouts

Item {
    id: agentStatus

    // [{ name, tier, awaitingApproval, activity }] — see header for shape.
    property var agents: []
    // false while vibed is unreachable: show a neutral placeholder, no chips.
    property bool online: false

    readonly property color colChip: "#313244"
    readonly property color colText: "#cdd6f4"
    readonly property color colMuted: "#6c7086"

    implicitHeight: 22
    implicitWidth: row.implicitWidth

    RowLayout {
        id: row
        anchors.verticalCenter: parent.verticalCenter
        spacing: 8

        // Offline / empty placeholder — the graceful-degradation path.
        Text {
            visible: !agentStatus.online || agentStatus.agents.length === 0
            text: agentStatus.online
                ? "aucun agent actif"
                : "agents : en attente de vibed (Phase 2)"
            color: agentStatus.colMuted
            font.pixelSize: 11
            font.italic: true
        }

        Repeater {
            model: agentStatus.online ? agentStatus.agents : []

            delegate: Rectangle {
                id: chip
                required property var modelData

                implicitWidth: chipRow.implicitWidth + 16
                implicitHeight: 22
                radius: 11
                color: agentStatus.colChip

                Row {
                    id: chipRow
                    anchors.centerIn: parent
                    spacing: 6

                    Text {
                        anchors.verticalCenter: parent.verticalCenter
                        text: chip.modelData.name
                        color: agentStatus.colText
                        font.pixelSize: 11
                    }

                    PolicyTierIndicator {
                        anchors.verticalCenter: parent.verticalCenter
                        tier: chip.modelData.tier
                        awaitingApproval: chip.modelData.awaitingApproval === true
                    }
                }

                // Lightweight tooltip substitute: activity line on hover.
                MouseArea {
                    anchors.fill: parent
                    hoverEnabled: true
                    onEntered: activityTip.visible = true
                    onExited: activityTip.visible = false
                }
                Rectangle {
                    id: activityTip
                    visible: false
                    anchors.top: parent.bottom
                    anchors.topMargin: 4
                    anchors.horizontalCenter: parent.horizontalCenter
                    implicitWidth: tipText.implicitWidth + 12
                    implicitHeight: tipText.implicitHeight + 8
                    radius: 4
                    color: "#181825"
                    border.color: agentStatus.colChip
                    z: 10
                    Text {
                        id: tipText
                        anchors.centerIn: parent
                        text: chip.modelData.activity || ""
                        color: agentStatus.colText
                        font.pixelSize: 10
                    }
                }
            }
        }
    }
}
