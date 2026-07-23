// DangerBanner — the ADR-027 danger panel: a full-width alarm strip shown ONLY
// while the human operator has turned the out-of-band key (`vibectl mode open`)
// and the machine is running AUTONOMOUS / OPEN mode — T2/T3 approvals are
// auto-granted for a bounded window. This is the one state the design system
// spends the reserved destructive red on a whole surface for: the user must not
// be able to glance at the screen without knowing the floor is lifted.
//
// Model shape (from vibed_client.js `modeBanner(osStatus)`):
//   { active: true,                       // open mode is in effect
//     danger: true,                       // wire's danger flag, mirrored
//     remainingLabel: "23 min restantes", // auto-expiry countdown ("" never shown)
//     reason: "démo salon",               // operator's stated reason ("" = none)
//     setByUid: "uid 0" }                 // who turned the key ("" = unknown)
//
// ---------------------------------------------------------------------------
// DATA SOURCE — fail-safe by construction (honesty rule)
// ---------------------------------------------------------------------------
// os.status (T0) carries a `mode` block computed by vibed's mode::status —
// governed on ANY uncertainty (file absent, malformed, expired). shell.qml maps
// it through vibed_client.js `modeBanner()`, which is inactive for governed /
// absent / malformed / offline. So this window stays invisible (no strut, no
// pixels) except during a real, unexpired open window — and conversely, once
// vibed HAS said "open", a junk secondary field never hides the alarm; the
// unknown reads as unknown instead. Same edge as the HUD bar, declared AFTER it
// in shell.qml so the layer shell stacks this strip BELOW the bar's exclusive
// zone. Like the rest of the HUD, not yet validated on a booted Plasma.
// ---------------------------------------------------------------------------

import QtQuick
import QtQuick.Layouts
import Quickshell

PanelWindow {
    id: dangerBanner

    // Mapped shape from vibed_client.js modeBanner() — see header.
    property var banner: ({ active: false, danger: false,
                            remainingLabel: "", reason: "", setByUid: "" })

    // Guarded accessors: a null/partial binding must degrade to the inactive
    // strip, never to a binding error that kills the shell.
    // Ternary, not `banner && ...`: a null banner must yield a real `false`,
    // never a null/undefined the bool property would refuse to accept.
    readonly property bool activeNow: banner ? banner.active === true : false
    readonly property string remainingLabel:
        (banner && typeof banner.remainingLabel === "string") ? banner.remainingLabel : ""
    readonly property string reasonText:
        (banner && typeof banner.reason === "string") ? banner.reason : ""
    readonly property string operatorLabel:
        (banner && typeof banner.setByUid === "string") ? banner.setByUid : ""

    anchors { top: true; left: true; right: true }
    visible: activeNow
    implicitHeight: Theme.barHeight
    // No strut while inactive: governed mode must leave the desktop untouched.
    exclusiveZone: activeNow ? Theme.barHeight : 0
    color: "transparent"

    Rectangle {
        anchors.fill: parent
        // Reserved destructive red as a full field — this IS the destructive
        // state (DESIGN-SYSTEM: red = destructive/error only).
        color: Theme.red

        // Slow attention pulse: a maroon wash breathing over the red field
        // (§8.3 grammar, durPulse band). Reduced motion -> static wash, the
        // banner itself stays fully visible either way. The animation drives a
        // plain property and opacity is BOUND to it (the statePill pattern in
        // shell.qml) — animating opacity directly would sever its binding.
        Rectangle {
            id: pulseWash
            anchors.fill: parent
            color: Theme.maroon
            property real pulse: 0.0
            opacity: Theme.reducedMotion ? 0.2 : pulse
            SequentialAnimation on pulse {
                running: dangerBanner.activeNow && !Theme.reducedMotion
                loops: Animation.Infinite
                NumberAnimation { from: 0.0; to: 0.35; duration: Theme.durPulse / 2
                    easing.type: Easing.Bezier; easing.bezierCurve: Theme.easeStandard }
                NumberAnimation { from: 0.35; to: 0.0; duration: Theme.durPulse / 2
                    easing.type: Easing.Bezier; easing.bezierCurve: Theme.easeStandard }
            }
        }

        // Crust-tinted arris top and bottom so the strip reads as its own
        // plane, not a bleed of the glass bar above it.
        Rectangle { anchors { top: parent.top; left: parent.left; right: parent.right }
                    height: 1; color: Theme.withAlpha(Theme.crust, 0.35) }
        Rectangle { anchors { bottom: parent.bottom; left: parent.left; right: parent.right }
                    height: 1; color: Theme.withAlpha(Theme.crust, 0.35) }

        RowLayout {
            anchors.fill: parent
            anchors.leftMargin: Theme.space4
            anchors.rightMargin: Theme.space4
            spacing: Theme.space3

            // Alarm dot — the statePill dot grammar, in ink-on-accent.
            Rectangle {
                Layout.alignment: Qt.AlignVCenter
                width: 8; height: 8; radius: 4
                color: Theme.textOnAccent
            }

            Text {
                Layout.alignment: Qt.AlignVCenter
                text: "MODE AUTONOME / OUVERT ACTIF"
                color: Theme.textOnAccent
                font.family: Theme.fontSans
                font.pixelSize: Theme.fsBody
                font.weight: Theme.weightSemibold
                font.letterSpacing: 0.8
            }

            Rectangle { Layout.alignment: Qt.AlignVCenter; width: 1; height: 16
                        color: Theme.withAlpha(Theme.crust, 0.35) }

            // Auto-expiry countdown (mono = data). modeBanner() guarantees a
            // label whenever active, but degrade honestly if it is missing.
            Text {
                Layout.alignment: Qt.AlignVCenter
                text: dangerBanner.remainingLabel !== ""
                    ? dangerBanner.remainingLabel : "durée restante inconnue"
                color: Theme.textOnAccent
                font.family: Theme.fontMono
                font.pixelSize: Theme.fsMonoSm
                font.weight: Theme.weightSemibold
            }

            Rectangle { Layout.alignment: Qt.AlignVCenter; width: 1; height: 16
                        color: Theme.withAlpha(Theme.crust, 0.35) }

            // Who turned the key. An unknown operator on an open window is
            // notable — say "inconnu", never render a plausible blank.
            Text {
                Layout.alignment: Qt.AlignVCenter
                text: dangerBanner.operatorLabel !== ""
                    ? ("déverrouillé par " + dangerBanner.operatorLabel)
                    : "opérateur inconnu"
                color: Theme.textOnAccent
                font.family: Theme.fontMono
                font.pixelSize: Theme.fsMonoSm
            }

            // Operator's stated reason. Fills the middle so the kill-switch
            // reminder stays pinned right; empty when none was given.
            Text {
                Layout.fillWidth: true
                Layout.alignment: Qt.AlignVCenter
                text: dangerBanner.reasonText !== ""
                    ? ("motif : " + dangerBanner.reasonText) : ""
                color: Theme.textOnAccent
                font.family: Theme.fontSans
                font.pixelSize: Theme.fsSmall
                font.italic: true
                elide: Text.ElideRight
            }

            // The kill-switch is the operator's half of the ADR-027 contract:
            // the banner must always say how to revert, not just alarm.
            Text {
                Layout.alignment: Qt.AlignVCenter
                text: "kill-switch : vibectl mode governed"
                color: Theme.textOnAccent
                font.family: Theme.fontMono
                font.pixelSize: Theme.fsMonoSm
                font.weight: Theme.weightSemibold
            }
        }
    }
}
