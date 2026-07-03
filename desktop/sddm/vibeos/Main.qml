// VibeOS — SDDM login theme (Qt6 / QML)
// =============================================================================
// Target (image build, read-only at runtime):
//   /usr/share/sddm/themes/vibeos/Main.qml
// Source of truth (this repo, COPY-ed into the image by the os track):
//   desktop/sddm/vibeos/Main.qml     (do NOT edit os/Containerfile from here)
//
// STATUS — 🛣️ Phase 5 (branding). In v0.1 the shipped greeter is Breeze
// (docs/DESKTOP.md §4.1). This theme is designed in full now; it only becomes
// the default once Phase 5 sets `[Theme] Current=vibeos` (see desktop/sddm/README.md).
//
// DESIGN — docs/DESIGN-SYSTEM.md §11.2 (Login). Every colour/radius/duration
// below traces to a design token. Tokens are HARDCODED as hex because SDDM has
// no Theme singleton to import — keep them in sync with the design system by hand.
//
// ROBUSTNESS — a login screen must never fail to load, so imports are limited to
// modules that always ship with Qt6 (QtQuick, QtQuick.Controls.Basic, Layouts).
// No optional-effect module is imported: real backdrop blur / drop-shadow would
// need QtQuick.Effects or a compositor, and their absence must not break login.
// "Glass" (§6) is therefore honestly approximated — a translucent Base fill over
// our OWN painted aurora (not live windows), plus a spectral top edge; elevation
// (§5) is faked with layered translucent underlays. This reads as glass while
// staying dependency-free. See desktop/sddm/README.md for the honesty note.
// =============================================================================

import QtQuick
import QtQuick.Layouts
import QtQuick.Controls.Basic

Rectangle {
    id: root
    // surface-0 (Crust) — base backdrop; pure #000 is forbidden (§14).
    color: "#11111b"
    // SDDM uses SizeRootObjectToView, so this Rectangle is resized to the greeter.

    // ---- Design tokens (hardcoded hex — source: DESIGN-SYSTEM.md §12) ----------
    readonly property color cCrust:    "#11111b" // surface-0 / void
    readonly property color cMantle:   "#181825" // surface-1 / chrome
    readonly property color cBase:     "#1e1e2e" // surface-2 / canvas
    readonly property color cSurface0: "#313244" // surface-3 / control
    readonly property color cText:     "#cdd6f4" // text-primary
    readonly property color cSubtext0: "#a6adc8" // text-tertiary
    readonly property color cOverlay2: "#9399b2" // text-placeholder
    readonly property color cMauve:    "#cba6f7" // accent — identity / focus ONLY (§4)
    readonly property color cBlue:     "#89b4fa" // T0 / signature-gradient end
    readonly property color cYellow:   "#f9e2af" // caps-lock notice (attente)
    readonly property color cRed:      "#f38ba8" // error (T3/erreur — honest use, §10.3)

    // border-subtle / state-selected / glow-focus / state overlays (alpha tokens)
    readonly property color cBorderSubtle:  Qt.rgba(0.803, 0.839, 0.956, 0.12)
    readonly property color cGlassEdgeTop:  Qt.rgba(1, 1, 1, 0.10)
    readonly property color cGlassBase78:   Qt.rgba(0.117, 0.117, 0.180, 0.78) // Base @ 78% (glass-elevated)
    readonly property color cGlowFocus:     Qt.rgba(0.796, 0.651, 0.968, 0.16) // mauve @ 16%
    readonly property color cStateHover:    Qt.rgba(0.803, 0.839, 0.956, 0.05)
    readonly property color cStateActive:   Qt.rgba(0.066, 0.066, 0.105, 0.35)

    // Typography — font-sans is aspirational Inter, resolved to Noto Sans in the
    // image until Inter is packaged (§3.1); Qt substitutes the fallback silently.
    readonly property string fontSans: "Inter"
    readonly property string fontMono: "JetBrains Mono"

    // ---- runtime state ----------------------------------------------------------
    property bool   busy: false
    property string errorText: ""
    property var    now: new Date()

    function greeting() {
        var h = now.getHours();
        if (h < 5)  return "Bonne nuit";
        if (h < 12) return "Bonjour";
        if (h < 18) return "Bon après-midi";
        return "Bonsoir";
    }
    function attemptLogin() {
        if (root.busy) return;
        root.errorText = "";
        root.busy = true;
        sddm.login(userField.text, passwordField.text, sessionCombo.currentIndex);
    }

    Timer { interval: 1000; running: true; repeat: true; onTriggered: root.now = new Date() }

    Connections {
        target: sddm
        // Login rejected: honest error, keep the user in place (no gaudy shake — §14).
        function onLoginFailed() {
            root.busy = false;
            root.errorText = "Identifiants invalides. Réessayez.";
            passwordField.clear();
            passwordField.forceActiveFocus();
        }
        // Success: quick ease-accelerate fade-out — never make the user wait (§8.3).
        function onLoginSucceeded() { exitAnim.start(); }
    }

    // =========================================================================
    // Reusable pieces (inline components, Qt 6.3+)
    // =========================================================================

    // Signature ring stroke (mauve→blue) drawn on a Canvas — the brand motif
    // (§9), shared with the HUD agent ring and the Plymouth ring. `sweep < 1`
    // draws an open arc (brand mark); `sweep = 1` a full ring (avatar).
    component GradientRing: Canvas {
        property real thickness: 2
        property real sweep: 1.0
        onPaint: {
            var ctx = getContext("2d");
            ctx.reset();
            ctx.lineWidth = thickness;
            ctx.lineCap = "round";
            var g = ctx.createLinearGradient(0, 0, width, height);
            g.addColorStop(0, "#cba6f7"); // mauve
            g.addColorStop(1, "#89b4fa"); // blue
            ctx.strokeStyle = g;
            var start = -Math.PI * 0.5;   // 12 o'clock
            ctx.beginPath();
            ctx.arc(width / 2, height / 2, width / 2 - thickness,
                    start, start + Math.PI * 2 * sweep);
            ctx.stroke();
        }
        onWidthChanged: requestPaint()
        onHeightChanged: requestPaint()
    }

    // Text input styled per §2.5: surface-3, radius-md, mauve focus ring + glow.
    component VField: Item {
        id: f
        property alias  text: input.text
        property string placeholder: ""
        property bool   password: false
        signal accepted()
        implicitHeight: 46          // ≥ space-10, comfortable target (§15)
        function clear() { input.clear() }
        function selectAll() { input.selectAll() }
        function forceActiveFocus() { input.forceActiveFocus() }

        // glow-focus halo (§5) — soft 4px mauve ring, focus only.
        Rectangle {
            anchors.fill: bg
            anchors.margins: -4
            radius: 14
            color: "transparent"
            border.width: 4
            border.color: root.cGlowFocus
            visible: input.activeFocus
        }
        Rectangle {
            id: bg
            anchors.fill: parent
            radius: 10                                   // radius-md
            color: root.cSurface0                        // surface-3
            border.width: input.activeFocus ? 2 : 1
            border.color: input.activeFocus ? root.cMauve : root.cBorderSubtle
            Behavior on border.color { ColorAnimation { duration: 120 } } // duration-fast
        }
        TextInput {
            id: input
            anchors.fill: bg
            anchors.leftMargin: 14
            anchors.rightMargin: 14
            verticalAlignment: TextInput.AlignVCenter
            color: root.cText
            font.family: root.fontSans
            font.pixelSize: 13
            echoMode: f.password ? TextInput.Password : TextInput.Normal
            passwordCharacter: "•"
            passwordMaskDelay: 0
            selectByMouse: true
            selectionColor: root.cMauve
            selectedTextColor: root.cCrust               // text-on-accent
            clip: true
            onAccepted: f.accepted()
        }
        Text {   // placeholder
            anchors.left: bg.left; anchors.leftMargin: 14
            anchors.verticalCenter: bg.verticalCenter
            text: f.placeholder
            color: root.cOverlay2                        // text-placeholder
            font.family: root.fontSans
            font.pixelSize: 13
            visible: input.text.length === 0 && !input.activeFocus
        }
    }

    // Subtle footer action (suspend / reboot / power) — state-layer overlays (§2.5).
    component PowerButton: Rectangle {
        property string label: ""
        signal activated()
        implicitWidth: pbText.implicitWidth + 20
        implicitHeight: 30
        radius: 8                                        // radius-sm+
        color: pbMouse.pressed  ? root.cStateActive
             : pbMouse.containsMouse ? root.cStateHover
             : "transparent"
        border.width: 1
        border.color: root.cBorderSubtle
        Behavior on color { ColorAnimation { duration: 120 } }
        Text {
            id: pbText
            anchors.centerIn: parent
            text: parent.label
            color: root.cSubtext0
            font.family: root.fontSans
            font.pixelSize: 11
        }
        MouseArea {
            id: pbMouse
            anchors.fill: parent
            hoverEnabled: true
            cursorShape: Qt.PointingHandCursor
            onClicked: parent.activated()
        }
    }

    // =========================================================================
    // Background — aurora "Genesis" (grad-mesh-genesis, §7). Painted procedurally
    // so it needs no wallpaper asset and no blur module.
    // =========================================================================
    Image {
        id: bgImage
        anchors.fill: parent
        // Optional override: theme.conf `background=` (absolute path). Empty by
        // default — VibeOS ships the procedural aurora, not a bitmap.
        source: (typeof config !== "undefined" && config.background) ? config.background : ""
        visible: source !== ""
        fillMode: Image.PreserveAspectCrop
        asynchronous: true
    }
    Canvas {
        id: aurora
        anchors.fill: parent
        opacity: bgImage.visible ? 0.9 : 1.0
        onPaint: {
            var ctx = getContext("2d");
            ctx.reset();
            // linear-gradient(180deg, Base 0%, Crust 100%)
            var lg = ctx.createLinearGradient(0, 0, 0, height);
            lg.addColorStop(0.0, "#1e1e2e");
            lg.addColorStop(1.0, "#11111b");
            ctx.fillStyle = lg;
            ctx.fillRect(0, 0, width, height);
            // radial halo mauve→blue→transparent, centred at 78% / 8% (top-right)
            var gx = width * 0.78, gy = height * 0.08;
            var gr = Math.max(width, height) * 1.1;
            var rg = ctx.createRadialGradient(gx, gy, 0, gx, gy, gr);
            rg.addColorStop(0.0,  "rgba(203,166,247,0.20)"); // mauve 20%
            rg.addColorStop(0.32, "rgba(137,180,250,0.08)"); // blue 8%
            rg.addColorStop(0.60, "rgba(30,30,46,0.0)");     // fade to Base
            ctx.fillStyle = rg;
            ctx.fillRect(0, 0, width, height);
        }
        onWidthChanged: requestPaint()
        onHeightChanged: requestPaint()
    }

    // =========================================================================
    // Login card — glass-elevated (§6.1): translucent Base over the aurora,
    // radius-xl, elevation-4 (faked), spectral top edge. Entrance is the one
    // "moment" that earns duration-entrance / ease-decelerate (§8.1).
    // =========================================================================
    Item {
        id: cardWrap
        width: 380
        anchors.centerIn: parent
        height: card.height
        opacity: 0
        transform: Translate { id: cardT; y: 8 }

        // elevation-4 approximation: two stacked, offset, translucent underlays
        // (no blur module) — soft dark bloom beneath the card.
        Rectangle {
            anchors.fill: card
            anchors.margins: -6
            anchors.topMargin: 12
            radius: 26
            color: "#11111b"
            opacity: 0.50
        }
        Rectangle {
            anchors.fill: card
            anchors.margins: -2
            anchors.topMargin: 5
            radius: 22
            color: "#11111b"
            opacity: 0.40
        }

        Rectangle {
            id: card
            width: parent.width
            height: cardCol.implicitHeight + 48          // + 2 × space-6 padding
            color: root.cGlassBase78                     // glass-elevated bg
            radius: 20                                   // radius-xl
            border.width: 1
            border.color: root.cBorderSubtle

            // glass-edge-top: 1px spectral highlight along the top arête (§2.4)
            Rectangle {
                anchors { top: parent.top; left: parent.left; right: parent.right }
                anchors.leftMargin: 1; anchors.rightMargin: 1; anchors.topMargin: 1
                height: 1
                color: root.cGlassEdgeTop
            }

            ColumnLayout {
                id: cardCol
                anchors.fill: parent
                anchors.margins: 24                      // space-6
                spacing: 16                              // space-4

                // ---- brand row: open gradient ring + wordmark ----
                RowLayout {
                    Layout.alignment: Qt.AlignHCenter
                    spacing: 10
                    GradientRing {
                        width: 22; height: 22
                        thickness: 2
                        sweep: 0.72                      // open arc = brand mark
                    }
                    Text {
                        text: "VibeOS"
                        color: root.cText
                        font.family: root.fontSans
                        font.pixelSize: 15               // type-h3
                        font.weight: Font.DemiBold
                        font.letterSpacing: 0.2
                    }
                }

                // ---- avatar: full gradient ring + user initial ----
                Item {
                    Layout.alignment: Qt.AlignHCenter
                    Layout.topMargin: 4
                    width: 76; height: 76
                    GradientRing {
                        anchors.fill: parent
                        thickness: 2.5                   // agent-ring active weight (§4.3)
                        sweep: 1.0
                    }
                    Rectangle {
                        anchors.centerIn: parent
                        width: 60; height: 60; radius: 30
                        color: root.cSurface0            // surface-3
                        Text {
                            anchors.centerIn: parent
                            text: userField.text.length > 0
                                  ? userField.text.charAt(0).toUpperCase() : "V"
                            color: root.cText
                            font.family: root.fontSans
                            font.pixelSize: 26
                            font.weight: Font.DemiBold
                        }
                    }
                }

                // ---- greeting + clock ----
                Text {
                    Layout.alignment: Qt.AlignHCenter
                    text: root.greeting()
                    color: root.cText
                    font.family: root.fontSans
                    font.pixelSize: 28                   // type-display
                    font.weight: Font.DemiBold
                    font.letterSpacing: -0.5             // sans tightens at size
                }
                Text {
                    Layout.alignment: Qt.AlignHCenter
                    Layout.topMargin: -10
                    // Machine truth is mono (§3): time + date in JetBrains Mono.
                    text: Qt.formatDateTime(root.now, "HH:mm") + "   ·   " +
                          Qt.formatDateTime(root.now, "dddd d MMMM")
                    color: root.cSubtext0                // text-tertiary
                    font.family: root.fontMono
                    font.pixelSize: 13
                }

                Item { Layout.fillWidth: true; implicitHeight: 4 } // small breath

                // ---- credentials ----
                VField {
                    id: userField
                    Layout.fillWidth: true
                    placeholder: "Utilisateur"
                    text: (typeof userModel !== "undefined" && userModel.lastUser)
                          ? userModel.lastUser : ""
                    onAccepted: passwordField.forceActiveFocus()
                }
                VField {
                    id: passwordField
                    Layout.fillWidth: true
                    placeholder: "Mot de passe"
                    password: true
                    onAccepted: root.attemptLogin()
                }

                // ---- caps-lock notice (Yellow = attente, never alarming) ----
                RowLayout {
                    Layout.fillWidth: true
                    spacing: 6
                    visible: (typeof keyboard !== "undefined") && keyboard.capsLock
                    Rectangle { width: 6; height: 6; radius: 3; color: root.cYellow }
                    Text {
                        text: "Verrouillage des majuscules activé"
                        color: root.cYellow
                        font.family: root.fontSans
                        font.pixelSize: 12
                    }
                }

                // ---- error line (Red reserved for error — honest, §10.3) ----
                Text {
                    Layout.fillWidth: true
                    visible: root.errorText !== ""
                    text: root.errorText
                    color: root.cRed
                    font.family: root.fontSans
                    font.pixelSize: 12
                    wrapMode: Text.WordWrap
                    horizontalAlignment: Text.AlignHCenter
                }

                // ---- primary action: solid Mauve, text-on-accent ----
                Button {
                    id: loginButton
                    Layout.fillWidth: true
                    enabled: !root.busy
                    contentItem: Text {
                        text: root.busy ? "Connexion…" : "Se connecter"
                        color: root.cCrust               // text-on-accent
                        font.family: root.fontSans
                        font.pixelSize: 13
                        font.weight: Font.DemiBold
                        horizontalAlignment: Text.AlignHCenter
                        verticalAlignment: Text.AlignVCenter
                    }
                    background: Rectangle {
                        implicitHeight: 46
                        radius: 10                       // radius-md
                        color: root.cMauve
                        // state layers (§2.5): press darkens, hover lightens
                        opacity: loginButton.down ? 0.86
                               : (loginButton.hovered ? 0.94 : 1.0)
                        Behavior on opacity { NumberAnimation { duration: 120 } }
                    }
                    onClicked: root.attemptLogin()
                }

                // ---- footer: session selector + power ----
                RowLayout {
                    Layout.fillWidth: true
                    Layout.topMargin: 4
                    spacing: 8

                    ComboBox {
                        id: sessionCombo
                        model: (typeof sessionModel !== "undefined") ? sessionModel : null
                        textRole: "name"
                        currentIndex: (typeof sessionModel !== "undefined")
                                      ? sessionModel.lastIndex : 0
                        implicitWidth: 190
                        implicitHeight: 34
                        font.family: root.fontSans
                        font.pixelSize: 12
                        contentItem: Text {
                            leftPadding: 12; rightPadding: 26
                            text: sessionCombo.displayText
                            color: root.cSubtext0
                            font: sessionCombo.font
                            verticalAlignment: Text.AlignVCenter
                            elide: Text.ElideRight
                        }
                        background: Rectangle {
                            radius: 8
                            color: "transparent"
                            border.width: 1
                            border.color: root.cBorderSubtle
                        }
                        indicator: Canvas {
                            x: sessionCombo.width - width - 10
                            y: sessionCombo.topPadding + (sessionCombo.availableHeight - height) / 2
                            width: 10; height: 6
                            onPaint: {
                                var ctx = getContext("2d");
                                ctx.reset();
                                ctx.strokeStyle = "#a6adc8";
                                ctx.lineWidth = 1.5;
                                ctx.lineCap = "round";
                                ctx.beginPath();
                                ctx.moveTo(0, 0); ctx.lineTo(width / 2, height);
                                ctx.lineTo(width, 0);
                                ctx.stroke();
                            }
                        }
                        popup: Popup {
                            y: sessionCombo.height + 4
                            width: sessionCombo.width
                            implicitHeight: Math.min(contentItem.implicitHeight + 8, 220)
                            padding: 4
                            background: Rectangle {
                                // glass-elevated with opaque fallback (no blur module)
                                color: root.cMantle
                                radius: 12                 // radius-lg-ish
                                border.width: 1
                                border.color: root.cBorderSubtle
                            }
                            contentItem: ListView {
                                clip: true
                                implicitHeight: contentHeight
                                model: sessionCombo.delegateModel
                                currentIndex: sessionCombo.highlightedIndex
                                boundsBehavior: Flickable.StopAtBounds
                            }
                        }
                        delegate: ItemDelegate {
                            width: sessionCombo.width - 8
                            height: 32
                            contentItem: Text {
                                text: model.name
                                color: root.cText
                                font.family: root.fontSans
                                font.pixelSize: 12
                                verticalAlignment: Text.AlignVCenter
                            }
                            highlighted: sessionCombo.highlightedIndex === index
                            background: Rectangle {
                                radius: 6
                                // state-selected = mauve @ 16% (§2.5)
                                color: highlighted ? Qt.rgba(0.796, 0.651, 0.968, 0.16)
                                                   : "transparent"
                            }
                        }
                    }

                    Item { Layout.fillWidth: true }      // spacer

                    PowerButton {
                        label: "Veille"
                        visible: (typeof sddm !== "undefined") && sddm.canSuspend
                        onActivated: sddm.suspend()
                    }
                    PowerButton {
                        label: "Redémarrer"
                        visible: (typeof sddm !== "undefined") && sddm.canReboot
                        onActivated: sddm.reboot()
                    }
                    PowerButton {
                        label: "Éteindre"
                        visible: (typeof sddm !== "undefined") && sddm.canPowerOff
                        onActivated: sddm.powerOff()
                    }
                }
            }
        }
    }

    // ---- motion: entrance (one brand moment) and quick exit ----
    ParallelAnimation {
        id: introAnim
        running: false
        // fade (0→1) + translate 8px, duration-entrance / ease-decelerate (§8.3)
        NumberAnimation {
            target: cardWrap; property: "opacity"; from: 0; to: 1
            duration: 400
            easing.type: Easing.BezierSpline
            easing.bezierCurve: [0.0, 0.0, 0.2, 1.0, 1.0, 1.0]
        }
        NumberAnimation {
            target: cardT; property: "y"; from: 8; to: 0
            duration: 400
            easing.type: Easing.BezierSpline
            easing.bezierCurve: [0.0, 0.0, 0.2, 1.0, 1.0, 1.0]
        }
    }
    NumberAnimation {
        id: exitAnim
        target: root; property: "opacity"; to: 0
        duration: 200                                    // duration-base
        easing.type: Easing.BezierSpline
        easing.bezierCurve: [0.4, 0.0, 1.0, 1.0, 1.0, 1.0] // ease-accelerate
    }

    Component.onCompleted: {
        passwordField.forceActiveFocus();
        introAnim.start();
    }
}
