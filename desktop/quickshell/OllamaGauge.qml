// OllamaGauge — local inference at a glance: loaded model, VRAM bar,
// activity dot. Sized for the 8 GiB VRAM budget of an RTX 3070 Ti
// (docs/ECOSYSTEM.md), where "how full is the card" is a daily question.
//
// Model shape (property `ollama`):
//   { available: true,               // ollama reachable at all?
//     model: "qwen2.5-coder:7b",     // loaded model name ("" if none)
//     generating: false,             // a request is currently running
//     vram_used_mb: 5200,
//     vram_total_mb: 8192 }
//
// ---------------------------------------------------------------------------
// DATA SOURCE — v0.1 vs Phase 2
// ---------------------------------------------------------------------------
// v0.1: MOCK reads from vibed_client.js (mockOllama()). Nothing is polled.
//
// TODO(Phase 2): two local, read-only sources (no vibed dependency — this
// gauge must keep working when vibed is offline, and show "—" when ollama
// itself is unreachable):
//   1. model + activity: ollama's local API, GET http://127.0.0.1:11434/api/ps
//      (the JSON behind `ollama ps`), polled every few seconds;
//   2. VRAM: `nvidia-smi --query-gpu=memory.used,memory.total
//      --format=csv,noheader,nounits` via Quickshell.Io Process — with a
//      graceful fallback to "—" on non-NVIDIA machines (AMD: amdgpu sysfs
//      `mem_info_vram_used/total` as the alternate reader).
// ---------------------------------------------------------------------------

import QtQuick

Item {
    id: gauge

    // See header comment for the expected shape.
    property var ollama: ({ available: false, model: "", generating: false,
                            vram_used_mb: 0, vram_total_mb: 0 })

    readonly property bool available: ollama && ollama.available === true
    readonly property real vramRatio: (available && ollama.vram_total_mb > 0)
        ? Math.max(0, Math.min(1, ollama.vram_used_mb / ollama.vram_total_mb))
        : 0

    readonly property color colText: "#cdd6f4"
    readonly property color colMuted: "#6c7086"
    readonly property color colTrack: "#313244"
    readonly property color colOk: "#a6e3a1"      // < 75 % VRAM
    readonly property color colWarn: "#fab387"    // 75–90 %
    readonly property color colHot: "#f38ba8"     // > 90 % — llama-swap time
    readonly property color barColor: vramRatio > 0.9 ? colHot
                                    : vramRatio > 0.75 ? colWarn : colOk

    implicitWidth: layout.implicitWidth
    implicitHeight: 22

    Row {
        id: layout
        anchors.verticalCenter: parent.verticalCenter
        spacing: 8

        // Activity dot: pulses while a generation is running.
        Rectangle {
            anchors.verticalCenter: parent.verticalCenter
            width: 8; height: 8; radius: 4
            color: gauge.available
                ? (gauge.ollama.generating ? gauge.colOk : gauge.colMuted)
                : gauge.colTrack
            SequentialAnimation on opacity {
                running: gauge.available && gauge.ollama.generating === true
                loops: Animation.Infinite
                NumberAnimation { from: 1.0; to: 0.3; duration: 500 }
                NumberAnimation { from: 0.3; to: 1.0; duration: 500 }
            }
        }

        // Model name — "ollama —" when unreachable (graceful degradation).
        Text {
            anchors.verticalCenter: parent.verticalCenter
            text: gauge.available
                ? (gauge.ollama.model !== "" ? gauge.ollama.model : "ollama (idle)")
                : "ollama —"
            color: gauge.available ? gauge.colText : gauge.colMuted
            font.pixelSize: 11
        }

        // VRAM bar: track + fill, with a compact MB label.
        Item {
            anchors.verticalCenter: parent.verticalCenter
            width: 64
            height: 6
            visible: gauge.available

            Rectangle { // track
                anchors.fill: parent
                radius: 3
                color: gauge.colTrack
            }
            Rectangle { // fill
                width: parent.width * gauge.vramRatio
                height: parent.height
                radius: 3
                color: gauge.barColor
                Behavior on width { NumberAnimation { duration: 300 } }
            }
        }

        Text {
            anchors.verticalCenter: parent.verticalCenter
            visible: gauge.available
            text: (gauge.ollama.vram_used_mb / 1024).toFixed(1) + " / "
                + (gauge.ollama.vram_total_mb / 1024).toFixed(0) + " Gio"
            color: gauge.colMuted
            font.pixelSize: 10
        }
    }
}
