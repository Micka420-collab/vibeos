// OllamaGauge — local inference at a glance: an elegant circular VRAM ring
// (gradient-stroked, rounded cap), the loaded model name and used/total VRAM.
// Sized for the 8 GiB budget of an RTX 3070 Ti (docs/ECOSYSTEM.md), where
// "how full is the card" is the daily question. Honest "—" when unreachable.
//
// The ring reads in Sky by default (palette.md: ollama/VRAM gauges), escalating
// to Peach then Red as it fills — Red here is a genuine over-budget ALERT state
// (llama-swap time), not decoration, so it stays inside the tier semantics (§10).
//
// Model shape (property `ollama`, from vibed_client.js):
//   { available: true,               // ollama reachable at all?
//     model: "qwen2.5-coder:7b",     // loaded model name ("" if none)
//     generating: false,             // a request is currently running
//     vram_used_mb: 5200,
//     vram_total_mb: 8192 }
//
// ---------------------------------------------------------------------------
// DATA SOURCE — v0.1 vs Phase 2
// ---------------------------------------------------------------------------
// v0.1: MOCK reads from vibed_client.js (mockOllama() -> available:false).
// TODO(Phase 2): two local, read-only sources (no vibed dependency — this gauge
// must keep working when vibed is offline, and show "—" when ollama itself is
// unreachable):
//   1. model + activity: GET http://127.0.0.1:11434/api/ps (behind `ollama ps`);
//   2. VRAM: `nvidia-smi --query-gpu=memory.used,memory.total
//      --format=csv,noheader,nounits` via Quickshell.Io.Process — graceful
//      fallback to "—" on non-NVIDIA (AMD: amdgpu sysfs mem_info_vram_used/total).
// ---------------------------------------------------------------------------

import QtQuick

Item {
    id: gauge

    // See header comment for the expected shape.
    property var ollama: ({ available: false, model: "", generating: false,
                            vram_used_mb: 0, vram_total_mb: 0 })

    readonly property bool available: ollama && ollama.available === true
    readonly property bool generating: available && ollama.generating === true
    readonly property real vramRatio: (available && ollama.vram_total_mb > 0)
        ? Math.max(0, Math.min(1, ollama.vram_used_mb / ollama.vram_total_mb))
        : 0

    // Threshold colour: nominal Sky -> Peach (>75%) -> Red (>90%, over-budget).
    readonly property color arcColor: !available ? Theme.tierOffline
                                     : vramRatio > 0.90 ? Theme.red
                                     : vramRatio > 0.75 ? Theme.peach
                                     : Theme.sky
    // Gradient companion (start of the arc gradient).
    readonly property color arcColorSoft: !available ? Theme.tierOffline
                                        : vramRatio > 0.90 ? Theme.maroon
                                        : vramRatio > 0.75 ? Theme.yellow
                                        : Theme.sapphire

    implicitWidth:  layout.implicitWidth
    implicitHeight: 26

    // Animated ratio so the ring eases to a new value (§8, standard curve).
    property real animRatio: gauge.available ? gauge.vramRatio : 0
    Behavior on animRatio { NumberAnimation { duration: Theme.durSlow
                            easing.type: Easing.Bezier; easing.bezierCurve: Theme.easeStandard } }
    onAnimRatioChanged: ringCanvas.requestPaint()
    onArcColorChanged:  ringCanvas.requestPaint()
    onAvailableChanged: ringCanvas.requestPaint()

    // Slow pulse for the "generating" activity dot (static if reduced-motion).
    property real pulse: 1.0
    SequentialAnimation on pulse {
        running: gauge.generating && !Theme.reducedMotion
        loops: Animation.Infinite
        NumberAnimation { from: 1.0; to: 0.35; duration: 640
                          easing.type: Easing.Bezier; easing.bezierCurve: Theme.easeStandard }
        NumberAnimation { from: 0.35; to: 1.0; duration: 640
                          easing.type: Easing.Bezier; easing.bezierCurve: Theme.easeStandard }
    }

    Row {
        id: layout
        anchors.verticalCenter: parent.verticalCenter
        spacing: Theme.space2

        // ---- Circular VRAM ring: track + gradient value arc, rounded cap ----
        Item {
            id: ringWrap
            anchors.verticalCenter: parent.verticalCenter
            width: 24; height: 24

            Canvas {
                id: ringCanvas
                anchors.fill: parent
                antialiasing: true
                readonly property real stroke: 3

                onPaint: {
                    var ctx = getContext("2d");
                    ctx.reset();
                    var cx = width / 2, cy = height / 2;
                    var r = Math.min(width, height) / 2 - stroke / 2;

                    // Track — full circle, recessed surface hue.
                    ctx.beginPath();
                    ctx.arc(cx, cy, r, 0, 2 * Math.PI);
                    ctx.lineWidth = stroke;
                    ctx.strokeStyle = gauge.available ? Theme.surface_3 : Theme.surface_1r;
                    ctx.stroke();

                    // Value arc — from 12 o'clock, clockwise, gradient + round cap.
                    if (gauge.available && gauge.animRatio > 0.001) {
                        var start = -Math.PI / 2;
                        var end = start + gauge.animRatio * 2 * Math.PI;
                        var grad = ctx.createLinearGradient(0, 0, width, height);
                        grad.addColorStop(0.0, gauge.arcColorSoft);
                        grad.addColorStop(1.0, gauge.arcColor);
                        ctx.beginPath();
                        ctx.arc(cx, cy, r, start, end);
                        ctx.lineWidth = stroke;
                        ctx.lineCap = "round";
                        ctx.strokeStyle = grad;
                        ctx.stroke();
                    }
                }
            }

            // Centre readout: VRAM % (mono) when available, else an honest dash.
            Text {
                anchors.centerIn: parent
                text: gauge.available ? Math.round(gauge.vramRatio * 100) : "—"
                color: gauge.available ? Theme.textSecondary : Theme.textMuted
                font.family: Theme.fontMono
                font.pixelSize: 9
                font.weight: Theme.weightMedium
            }
        }

        // ---- Model + VRAM figures, stacked (mono, tabular) ----
        Column {
            anchors.verticalCenter: parent.verticalCenter
            spacing: 1

            Row {
                spacing: Theme.space1
                // Generating dot — pulses while a request runs.
                Rectangle {
                    anchors.verticalCenter: parent.verticalCenter
                    width: 6; height: 6; radius: 3
                    visible: gauge.generating
                    color: Theme.sky
                    opacity: Theme.reducedMotion ? 1.0 : gauge.pulse
                }
                Text {
                    anchors.verticalCenter: parent.verticalCenter
                    text: gauge.available
                        ? (gauge.ollama.model !== "" ? gauge.ollama.model : "ollama (idle)")
                        : "ollama —"
                    color: gauge.available ? Theme.textPrimary : Theme.textMuted
                    font.family: Theme.fontMono
                    font.pixelSize: Theme.fsMonoSm
                    font.weight: gauge.available ? Theme.weightMedium : Theme.weightRegular
                }
            }

            Text {
                text: gauge.available
                    ? ((gauge.ollama.vram_used_mb / 1024).toFixed(1) + " / "
                       + (gauge.ollama.vram_total_mb / 1024).toFixed(0) + " Gio VRAM")
                    : "VRAM —"
                color: Theme.textMuted
                font.family: Theme.fontMono
                font.pixelSize: Theme.fsMonoSm
            }
        }
    }

    Component.onCompleted: ringCanvas.requestPaint()
}
