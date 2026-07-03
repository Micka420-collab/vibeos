// PolicyTierIndicator — the tier code made visible: a gradient tier pill whose
// leading motif is the VibeOS signature ANNEAU (open ring, DESIGN-SYSTEM §9/§10).
//
// A tier is always colour + form + label (never colour alone — colour-blind
// safe, §15): a conic gradient ring in the tier hue, the "T0".."T3" mono label,
// and — for T2/T3 awaiting approval — a drawn lock glyph plus a slow attention
// pulse and soft glow (glow-attention-t2 / glow-alert-t3, §5).
//
// Tier semantics (docs/ARCHITECTURE.md §4.2 — the tier is a floor):
//   T0 observe      Blue   read-only, allow
//   T1 modify-user  Green  user files/config, allow (audited)
//   T2 modify-system Peach  packages/services, ASK (human)   -> lock
//   T3 destructive  Red    disks/credentials/net identity, ASK+ -> lock (reinforced)
//
// By design a T2/T3 allow rule always produces a human-approval request, so
// `awaitingApproval` can only meaningfully be true at T2+; the binding enforces it.
// All colours/tokens come from Theme (no hard-coded values). Drawn with QtQuick
// Shapes (conic gradient annulus) and Rectangles — no emoji, font-independent.

import QtQuick
import QtQuick.Shapes
import QtQuick.Effects

Item {
    id: indicator

    // Policy tier 0..3 (values outside the range are clamped, never crash).
    property int tier: 0
    // True when a call at this tier is pending human approval (T2+ only).
    property bool awaitingApproval: false

    readonly property int  safeTier: Theme.clampTier(tier)
    readonly property bool showLock: awaitingApproval && Theme.tierNeedsApproval(safeTier)
    readonly property color tierC:    Theme.tierColor(safeTier)
    readonly property color tierAltC: Theme.tierColorAlt(safeTier)

    implicitWidth:  pill.implicitWidth
    implicitHeight: 18

    // ---- Slow attention pulse (drives an intermediate property so it never
    // fights the opacity/scale bindings). Static when reduced-motion (§8.3). ----
    property real pulse: 1.0
    SequentialAnimation on pulse {
        running: indicator.showLock && !Theme.reducedMotion
        loops: Animation.Infinite
        NumberAnimation { from: 1.0; to: 0.55; duration: Theme.durPulse / 2
                          easing.type: Easing.Bezier; easing.bezierCurve: Theme.easeStandard }
        NumberAnimation { from: 0.55; to: 1.0; duration: Theme.durPulse / 2
                          easing.type: Easing.Bezier; easing.bezierCurve: Theme.easeStandard }
    }

    // ---- Soft attention glow bloom behind the pill (T2 peach / T3 red) ----
    Rectangle {
        id: glow
        anchors.centerIn: pill
        width:  pill.width  + Theme.space3
        height: pill.height + Theme.space3
        radius: height / 2
        color:  indicator.safeTier >= 3 ? Theme.red : Theme.peach
        // Reduced-motion: hold a steady, clearly-visible glow instead of pulsing.
        opacity: indicator.showLock
                 ? (Theme.reducedMotion ? 0.34 : 0.20 + (indicator.pulse - 0.55) * 0.5)
                 : 0.0
        visible: opacity > 0.001
        layer.enabled: true
        layer.effect: MultiEffect { blurEnabled: true; blur: 1.0; blurMax: 24 }
        Behavior on opacity { NumberAnimation { duration: Theme.durFast
                              easing.type: Easing.Bezier; easing.bezierCurve: Theme.easeStandard } }
    }

    // ---- The tier pill: gradient tier fill (low alpha) + hairline tier border ----
    Rectangle {
        id: pill
        anchors.verticalCenter: parent.verticalCenter
        implicitWidth:  content.implicitWidth + Theme.space2 + Theme.space1
        implicitHeight: indicator.implicitHeight
        radius: height / 2

        // Subtle horizontal tier gradient background (the "gradient pill").
        gradient: Gradient {
            orientation: Gradient.Horizontal
            GradientStop { position: 0.0; color: Theme.withAlpha(indicator.tierC, 0.20) }
            GradientStop { position: 1.0; color: Theme.withAlpha(indicator.tierAltC, 0.12) }
        }
        border.width: 1
        border.color: Theme.withAlpha(indicator.tierC, indicator.showLock ? 0.65 : 0.42)

        Row {
            id: content
            anchors.centerIn: parent
            spacing: Theme.space1

            // ---- Signature ring: conic gradient annulus in the tier hue ----
            // Thickens 2 -> 2.5px while awaiting approval (DESIGN-SYSTEM §4.3).
            Item {
                id: ring
                anchors.verticalCenter: parent.verticalCenter
                width: 12; height: 12
                readonly property real thickness: indicator.showLock ? 2.5 : 2.0
                Behavior on thickness { NumberAnimation { duration: Theme.durFast
                                        easing.type: Easing.Bezier; easing.bezierCurve: Theme.easeStandard } }

                Shape {
                    anchors.fill: parent
                    preferredRendererType: Shape.CurveRenderer
                    antialiasing: true
                    ShapePath {
                        fillRule: ShapePath.OddEvenFill        // outer disc minus inner disc = ring
                        strokeWidth: -1                         // no stroke, gradient fill only
                        fillGradient: ConicalGradient {
                            centerX: ring.width / 2; centerY: ring.height / 2
                            angle: 220                          // grad-tier-tN start (DESIGN-SYSTEM §7)
                            GradientStop { position: 0.0; color: indicator.tierC }
                            GradientStop { position: 0.5; color: indicator.tierAltC }
                            GradientStop { position: 1.0; color: indicator.tierC }
                        }
                        // outer circle
                        PathAngleArc {
                            centerX: ring.width / 2; centerY: ring.height / 2
                            radiusX: ring.width / 2; radiusY: ring.height / 2
                            startAngle: 0; sweepAngle: 360; moveToStart: true
                        }
                        // inner circle (the hole)
                        PathAngleArc {
                            centerX: ring.width / 2; centerY: ring.height / 2
                            radiusX: ring.width / 2 - ring.thickness
                            radiusY: ring.height / 2 - ring.thickness
                            startAngle: 0; sweepAngle: 360; moveToStart: true
                        }
                    }
                }
            }

            // ---- Tier label — mono, tabular, in the (brightened) tier hue ----
            Text {
                anchors.verticalCenter: parent.verticalCenter
                text: Theme.tierLabel(indicator.safeTier)
                color: indicator.tierC
                font.family: Theme.fontMono
                font.pixelSize: Theme.fsMonoSm
                font.weight: Theme.weightSemibold
            }

            // ---- Lock glyph (drawn) — only at T2+ awaiting approval ----
            // Linear padlock: rounded shackle arc + body, per icon spec (§9).
            Item {
                id: lock
                visible: indicator.showLock
                anchors.verticalCenter: parent.verticalCenter
                width: 9; height: 11
                // shackle
                Rectangle {
                    x: 1.5; y: 0
                    width: 6; height: 7; radius: 3
                    color: "transparent"
                    border.color: indicator.tierC
                    border.width: 1.4
                }
                // body
                Rectangle {
                    x: 0; y: 4.5
                    width: 9; height: 6.5; radius: 1.6
                    color: indicator.tierC
                }
            }
        }
    }
}
