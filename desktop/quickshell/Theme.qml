// Theme.qml — VibeOS Design System, QML token singleton (the crux of cohesion).
//
// This is the SINGLE SOURCE OF TRUTH for the Quickshell HUD's visual language.
// Every other component references `Theme.*` — never a hard-coded colour, radius,
// duration, easing or spacing. A component that invents its own value is a design
// bug (DESIGN-SYSTEM.md §1.7, §14 "Valeurs en dur").
//
// It transcribes DESIGN-SYSTEM.md §12.2 verbatim and extends it with the tokens
// the live HUD needs (elevation, glows, gradient stops, tier helpers, a11y flags).
// The base palette is Catppuccin Mocha (MIT), forked as "VibeOS Dark" — values are
// invariant (palette.md): do NOT edit hexes here, only add derived tokens.
//
// Install target (image build, read-only at runtime):
//   /usr/share/vibeos/quickshell/Theme.qml
// Quickshell auto-registers config-dir singletons, so sibling components reference
// this type simply as `Theme` (no qmldir hand-authoring needed).

pragma Singleton
import QtQuick
import Quickshell

Singleton {
    id: theme

    // =======================================================================
    // 1. BASE PALETTE — Catppuccin Mocha, verbatim & invariant (palette.md §1)
    // =======================================================================
    readonly property color crust:    "#11111b"
    readonly property color mantle:   "#181825"
    readonly property color base:     "#1e1e2e"
    readonly property color surface0: "#313244"
    readonly property color surface1: "#45475a"
    readonly property color surface2: "#585b70"
    readonly property color overlay0: "#6c7086"
    readonly property color overlay1: "#7f849c"
    readonly property color overlay2: "#9399b2"
    readonly property color subtext0: "#a6adc8"
    readonly property color subtext1: "#bac2de"
    readonly property color text:     "#cdd6f4"
    // Accents
    readonly property color mauve:    "#cba6f7"   // THE signature accent — identity/focus only
    readonly property color blue:     "#89b4fa"   // T0
    readonly property color sapphire: "#74c7ec"   // T0 gradient companion
    readonly property color lavender: "#b4befe"   // secondary focus, accent-line end
    readonly property color green:    "#a6e3a1"   // T1
    readonly property color teal:     "#94e2d5"   // T1 gradient companion
    readonly property color sky:      "#89dceb"   // ollama / VRAM gauges
    readonly property color yellow:   "#f9e2af"   // waiting / T2 gradient companion
    readonly property color peach:    "#fab387"   // T2
    readonly property color maroon:   "#eba0ac"   // T3 gradient companion
    readonly property color red:      "#f38ba8"   // T3 — reserved: destructive/error only

    // =======================================================================
    // 2. SURFACES — elevation ladder s0..s4 (+ derived s1r) (DESIGN-SYSTEM §2.2)
    //    Higher plane = lighter surface (it moves toward the light).
    // =======================================================================
    readonly property color surface_0:  crust       // void / recessed / backdrop
    readonly property color surface_1:  mantle       // chrome: panel, dock, HUD, titlebars
    readonly property color surface_2:  base         // content canvas
    readonly property color surface_1r: "#26263a"    // derived subtle card over canvas (only non-Mocha value)
    readonly property color surface_3:  surface0     // card / control
    readonly property color surface_4:  surface1     // floating high: menus, tooltips, hover

    // =======================================================================
    // 3. TEXT ROLES (DESIGN-SYSTEM §2.3)
    // =======================================================================
    readonly property color textPrimary:     text       // body, titles, active data — AAA
    readonly property color textSecondary:   subtext1    // labels, subtitles
    readonly property color textTertiary:    subtext0    // metadata, inactive headers
    readonly property color textMuted:       overlay1    // disabled/offline/decor — NEVER essential
    readonly property color textPlaceholder: overlay2    // placeholders, help
    readonly property color textOnAccent:    crust       // ink on a Mauve fill
    readonly property color textAccent:      mauve       // brand link, active-agent value

    // =======================================================================
    // 4. BORDERS / HAIRLINES / GLASS EDGES (DESIGN-SYSTEM §2.4)
    //    Alpha applied on the text colour so lines adapt to the surface below.
    // =======================================================================
    readonly property color hairline:        Qt.rgba(0.804, 0.839, 0.957, 0.08)
    readonly property color borderSubtle:     Qt.rgba(0.804, 0.839, 0.957, 0.12)
    readonly property color borderStrong:     Qt.rgba(0.804, 0.839, 0.957, 0.20)
    readonly property color borderAccent:     mauve
    readonly property color glassEdgeTop:     Qt.rgba(1.0, 1.0, 1.0, 0.10)   // specular top arris
    readonly property color glassEdgeBottom:  Qt.rgba(0.0, 0.0, 0.0, 0.28)   // shadow bottom arris

    // =======================================================================
    // 5. INTERACTION STATE LAYERS (DESIGN-SYSTEM §2.5)
    // =======================================================================
    readonly property color stateHover:     Qt.rgba(0.804, 0.839, 0.957, 0.05)
    readonly property color stateActive:    Qt.rgba(0.067, 0.067, 0.106, 0.35)  // crust @ 35% (darkens)
    readonly property color stateSelected:  Qt.rgba(0.796, 0.651, 0.968, 0.16)  // mauve @ 16%
    readonly property color stateFocusRing: Qt.rgba(0.796, 0.651, 0.968, 0.60)  // mauve @ 60%
    readonly property real  disabledOpacity: 0.38

    // =======================================================================
    // 6. GLASS RECIPES — colour + blur radius (DESIGN-SYSTEM §6)
    //    Blur is applied by the compositor (KWin) behind the translucent layer
    //    surface; opaque fallback via `reducedTransparency` (see §15 a11y).
    // =======================================================================
    readonly property color glassChromeBg:   Qt.rgba(0.094, 0.094, 0.145, 0.72)  // mantle 72% — panel/dock
    readonly property color glassPanelBg:     Qt.rgba(0.067, 0.067, 0.106, 0.66)  // crust 66% — HUD bar
    readonly property color glassElevatedBg:  Qt.rgba(0.118, 0.118, 0.180, 0.78)  // base 78% — menus/tooltips
    readonly property color glassOsdBg:       Qt.rgba(0.067, 0.067, 0.106, 0.80)  // crust 80% — OSD/toasts
    readonly property int   blurThin:  24
    readonly property int   blurBase:  32
    readonly property int   blurHeavy: 40

    // =======================================================================
    // 7. RADII (DESIGN-SYSTEM §4.1)
    // =======================================================================
    readonly property int radiusXs:   4
    readonly property int radiusSm:   6
    readonly property int radiusMd:   10
    readonly property int radiusLg:   14
    readonly property int radiusXl:   20
    readonly property int radius2xl:  28
    readonly property int radiusFull: 999

    // =======================================================================
    // 8. SPACING — 4pt scale (DESIGN-SYSTEM §4.2)
    // =======================================================================
    readonly property int space1:  4
    readonly property int space2:  8
    readonly property int space3:  12
    readonly property int space4:  16
    readonly property int space5:  20
    readonly property int space6:  24
    readonly property int space8:  32
    readonly property int space10: 40
    readonly property int space12: 48
    readonly property int space16: 64

    // Layout constants specific to the HUD bar (DESKTOP.md §2.4).
    readonly property int barHeight:   34   // thin, always-visible top strip
    readonly property int barShadowPad: 10  // transparent room below the bar for the drop shadow to bleed
    readonly property int rowControl:  24   // standard in-bar control height

    // =======================================================================
    // 9. ELEVATION — shadow tokens (DESIGN-SYSTEM §5). Never pure #000.
    //    QML MultiEffect uses a normalized shadowBlur (0..1); we expose both the
    //    tint and the per-level blur/offset so components stay token-driven.
    // =======================================================================
    readonly property color shadow1: Qt.rgba(0.0, 0.0, 0.0, 0.30)  // elevation-1: card at rest, chip
    readonly property color shadow2: Qt.rgba(0.0, 0.0, 0.0, 0.35)  // elevation-2: raised card, dock
    readonly property color shadow3: Qt.rgba(0.0, 0.0, 0.0, 0.45)  // elevation-3: popover, menu, HUD panel
    readonly property color shadow4: Qt.rgba(0.0, 0.0, 0.0, 0.55)  // elevation-4: dialog, modal sheet

    // MultiEffect mapping (normalized blur + vertical offset px) per level.
    readonly property real elevBlur1: 0.18;  readonly property int elevY1: 1
    readonly property real elevBlur2: 0.35;  readonly property int elevY2: 3
    readonly property real elevBlur3: 0.55;  readonly property int elevY3: 8

    // Semantic glows (state, not height). Rare — reserved for "needs attention".
    readonly property color glowFocus:       Qt.rgba(0.796, 0.651, 0.968, 0.16)  // mauve — focus halo
    readonly property color glowAgentActive: Qt.rgba(0.796, 0.651, 0.968, 0.22)  // mauve bloom — agent working
    readonly property color glowAttentionT2: Qt.rgba(0.980, 0.702, 0.529, 0.30)  // peach — T2 approval waiting
    readonly property color glowAlertT3:     Qt.rgba(0.953, 0.545, 0.659, 0.32)  // red — T3 destructive waiting

    // =======================================================================
    // 10. POLICY TIERS (DESIGN-SYSTEM §10, palette.md §2) — colour + shape code.
    //     A tier is colour + form + label, everywhere. Helpers keep it uniform.
    // =======================================================================
    readonly property color tierT0: blue      // observe (read-only)
    readonly property color tierT1: green     // modify-user
    readonly property color tierT2: peach     // modify-system (approval)
    readonly property color tierT3: red       // destructive (approval+)
    readonly property color tierWait:    yellow    // indeterminate / paused
    readonly property color tierOffline: overlay1   // vibed offline (Phase 1)

    // Companion tones for the conic tier gradients (DESIGN-SYSTEM §7).
    readonly property var _tierBase: [tierT0, tierT1, tierT2, tierT3]
    readonly property var _tierAlt:  [sapphire, teal, yellow, maroon]
    readonly property var _tierName: ["T0", "T1", "T2", "T3"]

    function clampTier(t) { return Math.max(0, Math.min(3, Math.round(t))); }
    function tierColor(t)    { return theme._tierBase[clampTier(t)]; }  // ring/dot base hue
    function tierColorAlt(t) { return theme._tierAlt[clampTier(t)]; }   // gradient companion
    function tierLabel(t)    { return theme._tierName[clampTier(t)]; }  // "T0".."T3" (colour-blind safe)
    // T2+ always carries a lock: it passes a human-approval gate.
    function tierNeedsApproval(t) { return clampTier(t) >= 2; }

    // Utility: return `c` with a new alpha (for state overlays, tinted fills).
    function withAlpha(c, a) { return Qt.rgba(c.r, c.g, c.b, a); }

    // =======================================================================
    // 11. TYPOGRAPHY (DESIGN-SYSTEM §3). Data is ALWAYS mono; titles are sans.
    //     `fontSans` resolves to Inter when packaged, else Noto Sans via fontconfig.
    // =======================================================================
    readonly property string fontMono:    "JetBrains Mono"
    readonly property string fontMonoAlt: "Fira Code"
    readonly property string fontSans:    "Inter"          // fallback: Noto Sans (image v0.1)

    readonly property int fsDisplay:  28
    readonly property int fsH1:       22
    readonly property int fsH2:       18
    readonly property int fsH3:       15
    readonly property int fsBody:     13
    readonly property int fsSmall:    12
    readonly property int fsCaption:  11
    readonly property int fsMono:     13
    readonly property int fsMonoSm:   11

    readonly property int weightRegular: Font.Normal      // 400
    readonly property int weightMedium:  Font.Medium      // 500 — body-strong ~550
    readonly property int weightSemibold: Font.DemiBold   // 600 — headings, mono-strong

    // =======================================================================
    // 12. MOTION (DESIGN-SYSTEM §8). Discreet, fast, caused.
    //     Easing arrays are full cubic-bezier splines: [c1x,c1y, c2x,c2y, 1,1].
    // =======================================================================
    readonly property int durInstant:  90    // press feedback
    readonly property int durFast:      120   // hover, focus, micro-transitions
    readonly property int durBase:      200   // menus, tab change, content fade
    readonly property int durSlow:      320   // HUD panel in/out, dialogs
    readonly property int durEntrance:  400   // rare: first paint moments
    readonly property int durPulse:     1600  // slow attention pulse cycle (1400–1800ms band)

    readonly property var easeStandard:   [0.4, 0.0, 0.2, 1.0, 1.0, 1.0]  // default in-out
    readonly property var easeDecelerate: [0.0, 0.0, 0.2, 1.0, 1.0, 1.0]  // entrances: arrive & settle
    readonly property var easeAccelerate: [0.4, 0.0, 1.0, 1.0, 1.0, 1.0]  // exits: leave fast
    readonly property var easeEmphasized: [0.2, 0.9, 0.1, 1.0, 1.0, 1.0]  // signature soft spring

    // =======================================================================
    // 13. ACCESSIBILITY FLAGS (DESIGN-SYSTEM §15). Default off; TODO(Phase 2):
    //     bind to the session (KDE "reduce animations" / KWin blur availability).
    //     Components must honour these: no pulse & opaque glass fallbacks.
    // =======================================================================
    property bool reducedMotion:       false  // -> static visible states instead of pulses
    property bool reducedTransparency: false  // -> opaque surfaces instead of glass
}
