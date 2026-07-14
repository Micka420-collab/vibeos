// ReasoningPanel — troisième pilier du triptyque HUD, axe "visibilité" : ce
// qu'un agent pense réellement, en direct et dans l'historique. Compagnon
// d'AgentStatus (qui) et PolicyTierIndicator (quel tier) — celui-ci répond
// à "pourquoi il fait ça". Phase 2.5 (docs/ROADMAP.md).
//
// La capture vient du superviseur d'agent qui tape le flux du CLI, JAMAIS du
// transcript sur disque du CLI lui-même (docs/DECISIONS.md ADR-012) : les
// modèles actuels vident le champ thinking de leur JSONL sauvegardé — il n'y
// a rien à relire après coup si VibeOS n'a pas capté au moment du streaming.
//
// Model shape — LIVE (tour en cours, une entrée par agent actif) :
//   { sessionId: "auto-abc123",
//     provider: "claude-code",   // claude-code | codex | gemini-cli | opencode | ollama
//     model: "claude-sonnet-5",
//     raw: false,                // true UNIQUEMENT pour un modèle local (ollama) — voir note
//     redacted: false,           // true si le fournisseur a chiffré ce tour
//     streaming: true,
//     text: "Let me check whether..." }   // grandit à mesure que les deltas arrivent
//
// Model shape — HISTORY (une entrée par session passée, plus récent d'abord) :
//   { sessionId, provider, model, startedAt, durationLabel: "3h42",
//     turnCount, byteSize }
//
// ---------------------------------------------------------------------------
// DATA SOURCE — live-wired, honesty rule (identique à AgentStatus.qml)
// ---------------------------------------------------------------------------
// Ce fichier ne ships avec AUCUNE donnée : `live`/`history` valent [] par
// défaut, le panneau affiche son placeholder "aucune session". Rien n'est
// mocké. `agent.sessions` + `agent.thinking` existent dans vibed/src/mcp.rs et
// shell.qml câble `live` en direct : agent.sessions suit la session la plus
// récente, agent.thinking tail son raisonnement (mappé par reasoningToLive).
// `live` reste [] tant qu'aucune session n'existe ou que vibed est hors-ligne.
// TODO(Phase 2.5): brancher `history` (sessions passées) — pas encore lié dans
// shell.qml. Rendu visuel jamais validé sur un Plasma booté (machine-gated).
// ---------------------------------------------------------------------------

import QtQuick
import QtQuick.Layouts
import QtQuick.Effects
import Quickshell

Item {
    id: reasoningPanel

    // [{ sessionId, provider, model, raw, redacted, streaming, text }]
    property var live: []
    // [{ sessionId, provider, model, startedAt, durationLabel, turnCount, byteSize }]
    property var history: []
    property bool online: false           // vibed / superviseur d'agent joignable
    property bool captureEnabled: true    // reflète le toggle par session (ROADMAP Phase 2.5)

    readonly property bool anyStreaming: {
        for (var i = 0; i < live.length; ++i) if (live[i].streaming) return true
        return false
    }

    property int selectedHistoryIndex: -1   // -1 = vue live, sinon index dans `history`

    implicitHeight: 26
    implicitWidth: chip.implicitWidth

    // ===========================================================================
    // DÉCLENCHEUR — chip de bar, même langage visuel que les chips d'AgentStatus.
    // ===========================================================================
    Rectangle {
        id: chip
        anchors.verticalCenter: parent.verticalCenter
        implicitWidth: chipRow.implicitWidth + Theme.space3
        implicitHeight: 26
        radius: Theme.radiusFull
        color: hover.hovered || panel.visible ? Theme.surface_4 : Theme.surface_3
        border.width: 1
        border.color: panel.visible ? Theme.borderAccent
                       : (hover.hovered ? Theme.borderSubtle : Theme.hairline)

        Behavior on color { ColorAnimation { duration: Theme.durFast
                             easing.type: Easing.Bezier; easing.bezierCurve: Theme.easeStandard } }

        HoverHandler { id: hover }
        TapHandler { onTapped: panel.visible = !panel.visible }

        Row {
            id: chipRow
            anchors.centerIn: parent
            spacing: Theme.space2

            // ---- pastille "pensée" : pulse quand ça streame, statique sinon ----
            Item {
                anchors.verticalCenter: parent.verticalCenter
                width: 14; height: 14

                Rectangle {
                    anchors.centerIn: parent
                    width: 8; height: 8; radius: 4
                    color: reasoningPanel.online
                        ? (reasoningPanel.anyStreaming ? Theme.mauve : Theme.textMuted)
                        : Theme.tierOffline

                    // Pulse lent — même idiome que PolicyTierIndicator (reducedMotion honoré).
                    SequentialAnimation on scale {
                        running: reasoningPanel.anyStreaming && !Theme.reducedMotion
                        loops: Animation.Infinite
                        NumberAnimation { to: 1.6; duration: Theme.durPulse / 2
                            easing.type: Easing.Bezier; easing.bezierCurve: Theme.easeEmphasized }
                        NumberAnimation { to: 1.0; duration: Theme.durPulse / 2
                            easing.type: Easing.Bezier; easing.bezierCurve: Theme.easeEmphasized }
                    }
                }
            }

            Text {
                anchors.verticalCenter: parent.verticalCenter
                text: reasoningPanel.online
                    ? (reasoningPanel.anyStreaming ? "raisonnement…" : "raisonnement")
                    : "raisonnement — hors ligne"
                color: reasoningPanel.online ? Theme.textPrimary : Theme.textMuted
                font.family: Theme.fontSans
                font.pixelSize: Theme.fsSmall
                font.weight: Theme.weightMedium
                font.italic: !reasoningPanel.online
            }
        }
    }

    // ===========================================================================
    // PANNEAU — premier vrai second niveau de cette shell (pas de précédent :
    // composé directement à partir des tokens d'élévation de Theme).
    // À vérifier en intégrant : ancrer un PopupWindow depuis un Item non-fenêtre
    // (plutôt que directement sous le PanelWindow `bar` de shell.qml) — le
    // comportement d'`anchor.item` à cette profondeur d'imbrication est à
    // confirmer contre la version de Quickshell packagée dans l'image.
    // ===========================================================================
    PopupWindow {
        id: panel
        visible: false
        implicitWidth: 420
        implicitHeight: 480
        color: "transparent"

        anchor.item: chip
        anchor.edges: Edges.Bottom | Edges.Left
        // Réglages fins (marge, gravité) existent peut-être selon la version —
        // à vérifier contre quickshell.org/docs/.../PopupAnchor avant de
        // peaufiner l'alignement pixel-perfect.

        Rectangle {
            id: surface
            anchors.fill: parent
            radius: Theme.radiusLg
            border.width: 0   // QTBUG-137166 : transparent + border != 0 rend tout invisible en dessous
            color: Theme.reducedTransparency ? Theme.surface_2 : Theme.glassElevatedBg

            layer.enabled: true
            layer.effect: MultiEffect {
                shadowEnabled: true
                shadowColor: Theme.shadow3
                shadowBlur: Theme.elevBlur3
                shadowVerticalOffset: Theme.elevY3
                autoPaddingEnabled: true
            }

            // bordure dessinée à part (cf. note QTBUG ci-dessus)
            Rectangle {
                anchors.fill: parent
                radius: parent.radius
                color: "transparent"
                border.width: 1
                border.color: Theme.borderSubtle
            }
            Rectangle {  // arris spéculaire du haut
                anchors { top: parent.top; left: parent.left; right: parent.right }
                anchors.margins: 1
                height: 1
                color: Theme.glassEdgeTop
            }

            ColumnLayout {
                anchors.fill: parent
                anchors.margins: Theme.space4
                spacing: Theme.space3

                // ---- En-tête : titre + toggle de capture ----
                RowLayout {
                    Layout.fillWidth: true
                    Text {
                        Layout.fillWidth: true
                        text: "Raisonnement des agents"
                        color: Theme.textPrimary
                        font.family: Theme.fontSans
                        font.pixelSize: Theme.fsH3
                        font.weight: Theme.weightSemibold
                    }
                    // Capture on/off — le raisonnement est facturé comme de
                    // l'output (ROADMAP Phase 2.5) : garder ça visible et
                    // rapide à couper pendant un run de production.
                    Rectangle {
                        implicitWidth: 40; implicitHeight: 20; radius: Theme.radiusFull
                        color: reasoningPanel.captureEnabled ? Theme.withAlpha(Theme.green, 0.25) : Theme.surface_3
                        border.width: 1
                        border.color: reasoningPanel.captureEnabled ? Theme.green : Theme.hairline
                        TapHandler { onTapped: reasoningPanel.captureEnabled = !reasoningPanel.captureEnabled }
                        Rectangle {
                            width: 14; height: 14; radius: 7
                            anchors.verticalCenter: parent.verticalCenter
                            x: reasoningPanel.captureEnabled ? parent.width - width - 3 : 3
                            color: reasoningPanel.captureEnabled ? Theme.green : Theme.overlay1
                            Behavior on x { NumberAnimation { duration: Theme.durFast } }
                        }
                    }
                }

                // ---- Note de transparence (exigence d'honnêteté ADR-012) ----
                Text {
                    Layout.fillWidth: true
                    text: {
                        var anyLocal = false, anyCloud = false
                        for (var i = 0; i < reasoningPanel.live.length; ++i) {
                            if (reasoningPanel.live[i].raw) anyLocal = true; else anyCloud = true
                        }
                        if (anyLocal && !anyCloud)
                            return "Modèle local (ollama) : raisonnement brut, sans résumé."
                        if (anyCloud)
                            return "Résumé fourni par le fournisseur — pas le calcul interne brut (docs/DECISIONS.md ADR-012)."
                        return "Aucune session active."
                    }
                    color: Theme.textTertiary
                    font.family: Theme.fontSans
                    font.pixelSize: Theme.fsCaption
                    wrapMode: Text.WordWrap
                }

                Rectangle { Layout.fillWidth: true; height: 1; color: Theme.hairline }

                // ---- Corps : flux live OU session d'historique sélectionnée ----
                RowLayout {
                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    spacing: Theme.space3

                    // -- Rail gauche : historique des sessions --
                    ColumnLayout {
                        Layout.preferredWidth: 120
                        Layout.fillHeight: true
                        spacing: Theme.space1

                        Text {
                            text: "Historique"
                            color: Theme.textTertiary
                            font.family: Theme.fontSans
                            font.pixelSize: Theme.fsCaption
                            font.weight: Theme.weightSemibold
                        }

                        Rectangle {
                            Layout.fillWidth: true
                            implicitHeight: 28
                            radius: Theme.radiusSm
                            color: reasoningPanel.selectedHistoryIndex === -1 ? Theme.stateSelected : "transparent"
                            Text {
                                anchors.centerIn: parent
                                text: "● en direct"
                                color: reasoningPanel.selectedHistoryIndex === -1 ? Theme.textPrimary : Theme.textMuted
                                font.family: Theme.fontMono
                                font.pixelSize: Theme.fsMonoSm
                            }
                            TapHandler { onTapped: reasoningPanel.selectedHistoryIndex = -1 }
                        }

                        ListView {
                            Layout.fillWidth: true
                            Layout.fillHeight: true
                            clip: true
                            model: reasoningPanel.history
                            delegate: Rectangle {
                                required property var modelData
                                required property int index
                                width: ListView.view.width
                                implicitHeight: 40
                                radius: Theme.radiusSm
                                color: reasoningPanel.selectedHistoryIndex === index ? Theme.stateSelected
                                       : histHover.hovered ? Theme.stateHover : "transparent"
                                HoverHandler { id: histHover }
                                TapHandler { onTapped: reasoningPanel.selectedHistoryIndex = index }
                                Column {
                                    anchors.fill: parent
                                    anchors.margins: Theme.space1
                                    Text {
                                        text: modelData.provider + " · " + modelData.durationLabel
                                        color: Theme.textSecondary
                                        font.family: Theme.fontMono
                                        font.pixelSize: Theme.fsMonoSm
                                        elide: Text.ElideRight
                                        width: parent.width
                                    }
                                    Text {
                                        text: modelData.startedAt
                                        color: Theme.textMuted
                                        font.family: Theme.fontMono
                                        font.pixelSize: Theme.fsCaption
                                        elide: Text.ElideRight
                                        width: parent.width
                                    }
                                }
                            }
                        }
                    }

                    Rectangle { Layout.fillHeight: true; width: 1; color: Theme.hairline }

                    // -- Droite : le texte de raisonnement, auto-scroll en direct --
                    Flickable {
                        id: flick
                        Layout.fillWidth: true
                        Layout.fillHeight: true
                        clip: true
                        contentHeight: body.implicitHeight
                        // Reste collé en bas pendant le streaming — comme un tail -f.
                        onContentHeightChanged: {
                            if (reasoningPanel.selectedHistoryIndex === -1 && reasoningPanel.anyStreaming)
                                contentY = Math.max(0, contentHeight - height)
                        }

                        Text {
                            id: body
                            width: flick.width
                            wrapMode: Text.WordWrap
                            font.family: Theme.fontMono
                            font.pixelSize: Theme.fsMonoSm
                            color: Theme.textSecondary
                            text: {
                                if (reasoningPanel.selectedHistoryIndex === -1) {
                                    if (reasoningPanel.live.length === 0)
                                        return "Aucune session autonome en cours."
                                    // TODO(Phase 2.5): un onglet par entrée de `live` si
                                    // plusieurs agents tournent en parallèle — squelette :
                                    // on affiche la première ici.
                                    var entry = reasoningPanel.live[0]
                                    return entry.redacted
                                        ? "Une partie du raisonnement a été chiffrée par les systèmes de sécurité du fournisseur — cela n'affecte pas la qualité de la réponse."
                                        : (entry.text || "…")
                                }
                                // TODO(Phase 2.5): charger la session sélectionnée via
                                // agent.thinking(session_id) — pas encore câblé ici.
                                return "(chargement de la session sélectionnée — Phase 2.5)"
                            }
                        }
                    }
                }

                // ---- Pied : rappel de la vue historique complète en CLI ----
                Rectangle { Layout.fillWidth: true; height: 1; color: Theme.hairline }
                Text {
                    Layout.fillWidth: true
                    text: "Historique complet par session : vibectl agent thinking --session <id>"
                    color: Theme.textMuted
                    font.family: Theme.fontMono
                    font.pixelSize: Theme.fsCaption
                    elide: Text.ElideRight
                }
            }
        }
    }
}
