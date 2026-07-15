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
//   { sessionId, startedAt: "14 juil. 03:42", durationLabel: "3 h 42",
//     sizeLabel: "128 Kio" }
// Ni provider/model ni compteur de tours : voir la note DATA SOURCE ci-dessous.
//
// ---------------------------------------------------------------------------
// DATA SOURCE — live-wired, honesty rule (identique à AgentStatus.qml)
// ---------------------------------------------------------------------------
// Ce fichier ne ships avec AUCUNE donnée : `live`/`history` valent [] par
// défaut, le panneau affiche son placeholder "aucune session". Rien n'est
// mocké. `agent.sessions` + `agent.thinking` existent dans vibed/src/mcp.rs et
// shell.qml câble `live` ET `history` en direct : agent.sessions rend la liste
// des sessions avec leurs métadonnées (la plus récente d'abord) et suit `latest`,
// agent.thinking tail le raisonnement (mappé par reasoningToLive). `live` et
// `history` restent [] tant qu'aucune session n'existe ou que vibed est
// hors-ligne. Sélectionner une session passée déclenche `historySelected` →
// shell.qml va chercher son raisonnement (`selected`). Un ÉCHEC de lecture
// (`selectedError`) se dit échec — jamais « rien de capté », qui serait une
// affirmation inventée sur l'historique de l'agent.
// Rendu visuel jamais validé sur un Plasma booté (machine-gated).
// ---------------------------------------------------------------------------

import QtQuick
import QtQuick.Layouts
import QtQuick.Effects
// ListModel lives in QtQml.Models since Qt6. QtQuick re-exports it, but the
// import is explicit here rather than inherited by luck — this is the only file
// in the HUD that uses one, and there is no QML linter to catch it if it were
// missing.
import QtQml.Models
import Quickshell

Item {
    id: reasoningPanel

    // [{ sessionId, provider, model, raw, redacted, streaming, text }]
    property var live: []
    // Sessions passées, la plus récente d'abord — alimenté par agent.sessions
    // (mappé par Vibed.sessionsToHistory). Pas de `provider`/`model` : le store
    // de raisonnement ne les porte pas, et le panneau n'invente rien. Pas de
    // compteur de tours non plus : vibed devrait relire chaque fichier en entier
    // à chaque poll pour le produire.
    // [{ sessionId, startedAt, durationLabel, sizeLabel }]
    property var history: []
    // Raisonnement de la session d'historique sélectionnée — même forme que
    // `live`, chargé à la demande par shell.qml via agent.thinking(session_id).
    property var selected: []
    property bool selectedLoading: false
    // Pourquoi la lecture a échoué ("" = pas d'échec). SANS cet état, un déni de
    // politique ou un vibed coupé retombait sur « aucun raisonnement capté pour
    // cette session » — une AFFIRMATION FABRIQUÉE sur l'historique de l'agent,
    // exactement le mensonge que ce panneau existe pour empêcher. Un échec doit
    // se dire échec.
    property string selectedError: ""
    // Émis quand l'utilisateur choisit une session passée : shell.qml va chercher
    // son raisonnement. Le panneau reste un pur observateur, il n'ouvre aucun
    // socket lui-même.
    signal historySelected(string sessionId)
    property bool online: false           // vibed / superviseur d'agent joignable
    property bool captureEnabled: true    // reflète le toggle par session (ROADMAP Phase 2.5)

    readonly property bool anyStreaming: {
        for (var i = 0; i < live.length; ++i) if (live[i].streaming) return true
        return false
    }

    // Session d'historique affichée, par IDENTITÉ ("" = vue live). Surtout pas
    // par index : `history` est retriée par activité et republiée à chaque poll,
    // donc un index devient périmé en silence — la ligne surlignée désignerait
    // une session pendant que le corps affiche le raisonnement d'une autre.
    // Attribuer une pensée à la mauvaise session est précisément le mensonge que
    // ce panneau existe pour empêcher.
    property string selectedHistoryId: ""
    // Rendu quand vibed a tronqué la liste ("" sinon) — une vue partielle le dit.
    property string historyNote: ""

    // Le modèle RÉEL de la liste. `history` arrive en NOUVEAU tableau à chaque
    // poll (5 s) ; le lier directement à ListView.model réinitialiserait la vue à
    // chaque tick — délégués détruits, scroll rejeté en haut — parce que la
    // session LIVE est dans la liste et que ses libellés de taille/durée changent
    // réellement à mesure qu'elle écrit. Un garde par « clé de contenu » ne sert à
    // rien ici : la clé change justement quand quelque chose se passe, donc il ne
    // protège que quand il n'y a rien à protéger. On DIFFE : seules les lignes qui
    // ont bougé sont mises à jour EN PLACE, la vue (et le scroll de
    // l'utilisateur) ne bougent pas.
    ListModel { id: historyModel }

    function applyHistory(rows) {
        var n = rows ? rows.length : 0
        for (var i = 0; i < n; ++i) {
            var r = rows[i]
            if (i < historyModel.count) {
                var cur = historyModel.get(i)
                if (cur.sessionId !== r.sessionId || cur.startedAt !== r.startedAt
                        || cur.durationLabel !== r.durationLabel
                        || cur.sizeLabel !== r.sizeLabel)
                    historyModel.set(i, r)   // en place : pas de reset de la vue
            } else {
                historyModel.append(r)
            }
        }
        while (historyModel.count > n)
            historyModel.remove(historyModel.count - 1)
    }
    onHistoryChanged: applyHistory(history)
    Component.onCompleted: applyHistory(history)

    // Une coupure de vibed jette `selected` (shell.qml) mais la sélection vit
    // ICI : sans ça, le corps afficherait « aucun raisonnement capté » pour une
    // session qu'on n'a simplement plus les moyens de lire. Retour à la vue live,
    // qui dit honnêtement qu'on est hors ligne.
    onOnlineChanged: if (!online) selectedHistoryId = ""

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
                            color: reasoningPanel.selectedHistoryId === "" ? Theme.stateSelected : "transparent"
                            Text {
                                anchors.centerIn: parent
                                text: "● en direct"
                                color: reasoningPanel.selectedHistoryId === "" ? Theme.textPrimary : Theme.textMuted
                                font.family: Theme.fontMono
                                font.pixelSize: Theme.fsMonoSm
                            }
                            TapHandler {
                                onTapped: {
                                    reasoningPanel.selectedHistoryId = ""
                                    // Drop the selection so no past session keeps
                                    // being tracked behind the live view.
                                    reasoningPanel.historySelected("")
                                }
                            }
                        }

                        ListView {
                            Layout.fillWidth: true
                            Layout.fillHeight: true
                            clip: true
                            model: historyModel
                            delegate: Rectangle {
                                // Rôles du ListModel (et non `modelData` : le
                                // modèle est diffé en place, pas remplacé).
                                required property string sessionId
                                required property string startedAt
                                required property string durationLabel
                                required property string sizeLabel
                                width: ListView.view.width
                                implicitHeight: 40
                                radius: Theme.radiusSm
                                color: reasoningPanel.selectedHistoryId === sessionId ? Theme.stateSelected
                                       : histHover.hovered ? Theme.stateHover : "transparent"
                                HoverHandler { id: histHover }
                                TapHandler {
                                    onTapped: {
                                        reasoningPanel.selectedHistoryId = sessionId
                                        reasoningPanel.historySelected(sessionId)
                                    }
                                }
                                Column {
                                    anchors.fill: parent
                                    anchors.margins: Theme.space1
                                    // Ce que le store sait réellement d'une session :
                                    // quand elle a commencé, combien de temps elle a
                                    // duré, et le poids du raisonnement capté.
                                    Text {
                                        text: startedAt
                                        color: Theme.textSecondary
                                        font.family: Theme.fontMono
                                        font.pixelSize: Theme.fsMonoSm
                                        elide: Text.ElideRight
                                        width: parent.width
                                    }
                                    Text {
                                        text: durationLabel + " · " + sizeLabel
                                        color: Theme.textMuted
                                        font.family: Theme.fontMono
                                        font.pixelSize: Theme.fsCaption
                                        elide: Text.ElideRight
                                        width: parent.width
                                    }
                                }
                            }
                        }

                        // vibed plafonne la liste : quand elle est tronquée, on
                        // le dit plutôt que de faire passer une fenêtre pour
                        // l'historique complet.
                        Text {
                            Layout.fillWidth: true
                            visible: reasoningPanel.historyNote !== ""
                            text: reasoningPanel.historyNote
                            color: Theme.textMuted
                            font.family: Theme.fontMono
                            font.pixelSize: Theme.fsCaption
                            elide: Text.ElideRight
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
                            if (reasoningPanel.selectedHistoryId === "" && reasoningPanel.anyStreaming)
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
                                var isLive = reasoningPanel.selectedHistoryId === ""
                                var entries = isLive ? reasoningPanel.live : reasoningPanel.selected
                                if (entries.length === 0) {
                                    // Hors ligne : on ne SAIT pas ce qui tourne. Le
                                    // dire, plutôt qu'affirmer qu'il n'y a rien.
                                    if (!reasoningPanel.online)
                                        return "vibed est hors ligne — aucun raisonnement lisible."
                                    if (isLive) return "Aucune session autonome en cours."
                                    // Un échec de lecture (déni de politique, limite
                                    // de débit, erreur protocole) n'est PAS un constat
                                    // sur la session : ne jamais le maquiller en
                                    // « rien de capté ».
                                    if (reasoningPanel.selectedError !== "")
                                        return "Lecture impossible : " + reasoningPanel.selectedError
                                    if (reasoningPanel.selectedLoading)
                                        return "Chargement de la session…"
                                    // Seul cas restant : vibed a bien répondu, et la
                                    // session n'a réellement rien capté (capture
                                    // coupée, ou rien avant la fin).
                                    return "Aucun raisonnement capté pour cette session."
                                }
                                // TODO(Phase 2.5): un onglet par entrée de `live` si
                                // plusieurs agents tournent en parallèle — squelette :
                                // on affiche la première ici.
                                var entry = entries[0]
                                return entry.redacted
                                    ? "Une partie du raisonnement a été chiffrée par les systèmes de sécurité du fournisseur — cela n'affecte pas la qualité de la réponse."
                                    : (entry.text || "…")
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
