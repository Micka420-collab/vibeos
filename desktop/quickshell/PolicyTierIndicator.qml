// PolicyTierIndicator — T0..T3 badge with the VibeOS tier colors, plus a
// lock when a T2/T3 action awaits human approval.
//
// Tier semantics (docs/ARCHITECTURE.md §4.2 — the tier is a floor):
//   T0 observe        blue   — read-only, allow
//   T1 modify-user    green  — user files/config, allow (audited)
//   T2 modify-system  amber  — packages/services/system config, ASK (human)
//   T3 destructive    red    — disks/credentials/network identity, ASK+
//
// The lock is only meaningful for T2/T3: by design a T2/T3 allow rule always
// produces a human-approval request, so `awaitingApproval` can only be true
// at those tiers. The binding below enforces that defensively.
//
// Colors are the "VibeOS Dark" palette (fork of Catppuccin Mocha, MIT).

import QtQuick

Rectangle {
    id: indicator

    // Policy tier 0..3 (values outside the range are clamped, never crash).
    property int tier: 0
    // True when a call at this tier is pending human approval (T2+ only).
    property bool awaitingApproval: false

    readonly property int safeTier: Math.max(0, Math.min(3, tier))
    readonly property bool showLock: awaitingApproval && safeTier >= 2

    // T0 blue / T1 green / T2 amber / T3 red
    readonly property var tierColors: ["#89b4fa", "#a6e3a1", "#fab387", "#f38ba8"]
    readonly property var tierLabels: ["T0", "T1", "T2", "T3"]
    // Ink dark enough to sit on the pastel tier colors.
    readonly property color colInk: "#11111b"

    implicitWidth: content.implicitWidth + 10
    implicitHeight: 16
    radius: 8
    color: tierColors[safeTier]

    // Pending approval must catch the eye: gentle pulse, no motion sickness.
    SequentialAnimation on opacity {
        running: indicator.showLock
        loops: Animation.Infinite
        NumberAnimation { from: 1.0; to: 0.55; duration: 700 }
        NumberAnimation { from: 0.55; to: 1.0; duration: 700 }
    }
    // Reset cleanly when the approval is resolved.
    onShowLockChanged: if (!showLock) opacity = 1.0

    Row {
        id: content
        anchors.centerIn: parent
        spacing: 3

        Text {
            anchors.verticalCenter: parent.verticalCenter
            text: indicator.tierLabels[indicator.safeTier]
            color: indicator.colInk
            font.pixelSize: 10
            font.bold: true
        }

        // Lock glyph — drawn, not an emoji, so it renders identically with
        // any font stack (shackle arc + body).
        Item {
            visible: indicator.showLock
            anchors.verticalCenter: parent.verticalCenter
            width: 8
            height: 10

            // shackle
            Rectangle {
                x: 1; y: 0
                width: 6; height: 6
                radius: 3
                color: "transparent"
                border.color: indicator.colInk
                border.width: 1.4
            }
            // body
            Rectangle {
                x: 0; y: 4
                width: 8; height: 6
                radius: 1.5
                color: indicator.colInk
            }
        }
    }
}
