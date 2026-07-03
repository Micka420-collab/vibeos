/*
 * VibeOS Dark — Plasma 6 desktop layout script
 * =============================================
 * Runs once, when the "VibeOS Dark" Global Theme is first applied (or on a
 * "reset to defaults" of the desktop layout). It builds the two permanent
 * surfaces of the VibeOS desktop:
 *
 *   1. a SLIM FLOATING GLASS TOP PANEL (chrome) — launcher · activity pager ·
 *      centered clock · system tray                       (DESKTOP.md §2.2)
 *   2. a CENTERED AUTO-HIDE DOCK (icons only) — the scene belongs to the
 *      terminal, so the dock stays out of the way          (DESKTOP.md §2.3)
 *
 * Install target (image build, read-only /usr):
 *   /usr/share/plasma/look-and-feel/org.vibeos.dark/contents/layouts/
 *       org.kde.plasma.desktop-layout.js
 *
 * DESIGN NOTES
 *   - Glass look (glass-chrome: Mantle 72%, blur-base 32px, §6.1/§11.3) is NOT
 *     set from a layout script — Plasma has no scripting API for panel opacity/
 *     blur. It is delivered by the Panel Colorizer preset (Phase 2). Here we set
 *     `floating = true` so both panels detach from the screen edge and read as
 *     floating glass slabs (radius-lg container corners, §4.1).
 *   - The Quickshell HUD (agents · tiers · gauges) is a SEPARATE additive layer
 *     (Quickshell PanelWindow, DESKTOP §2.4) launched via autostart — it is NOT
 *     a Plasma panel and is intentionally absent from this script. Phase 2.
 *   - Every dimension maps to a design token; see the inline comments.
 *
 * HONESTY / PHASE — this whole Global Theme is Phase 2 (DESKTOP §2.6). The pins,
 * activities and shortcuts referenced below are Phase-2 desktop defaults; the
 * apps themselves (Ghostty…) already ship in v0.1.
 */

// ---------------------------------------------------------------------------
// 0. Desktop containments (one per screen). Keep the default wallpaper plugin;
//    the VibeOS "Genesis"/"Void" wallpapers (grad-mesh-genesis / grad-void, §7)
//    are a Phase 5 branding deliverable — we do not force an image here.
// ---------------------------------------------------------------------------
var allDesktops = desktops();
for (var i = 0; i < allDesktops.length; i++) {
    var desktop = allDesktops[i];
    desktop.wallpaperPlugin = "org.kde.image"; // Phase 5: swap for VibeOSDark wallpapers
    desktop.currentConfigGroup = ["Wallpaper", "org.kde.image", "General"];
    // Left intentionally at the packaged default in v0.1 — no third-party asset.
}

// ---------------------------------------------------------------------------
// 1. TOP PANEL — slim floating chrome (surface-1 / glass-chrome)
// ---------------------------------------------------------------------------
var topPanel = new Panel;
topPanel.location = "top";
// 30px slim chrome (DESKTOP §2.2 / design system §11.3). The panel is a calm
// frame, not a feature — height is deliberately minimal.
topPanel.height = 30;
// Floating detaches it from the screen edge → floating glass slab, radius-lg
// container corners once the Panel Colorizer preset lands (§4.1 / §11.3).
topPanel.floating = true;
// Permanent: this is the single always-present Plasma panel (DESKTOP §2.2).
topPanel.hiding = "none";
// Full width (minus floating margins).
topPanel.lengthMode = "fill";

// --- Left: application launcher (Kickoff), VibeOS glyph in v0.1 (§2.2) ---
var launcher = topPanel.addWidget("org.kde.plasma.kickoff");
launcher.currentConfigGroup = ["General"];
// Icon-only, compact launcher; the definitive VibeOS logo is Phase 5 branding.
launcher.writeConfig("icon", "start-here-kde-symbolic");
launcher.writeConfig("favoritesPortedToKAstats", true);

// --- Left: activity pager — the three named contexts Vibe / Focus / Review ---
// Embodies the "Contexte" pillar of the triptych (DESKTOP §3). The activities
// themselves are pre-configured in /etc/skel in Phase 2; this widget surfaces
// them. Accent (Mauve) on the current activity comes from the color scheme.
topPanel.addWidget("org.kde.plasma.activitypager");

// --- Elastic spacer → pushes the clock to true center ---
var spacerLeft = topPanel.addWidget("org.kde.plasma.panelspacer");
spacerLeft.currentConfigGroup = ["General"];
spacerLeft.writeConfig("expanding", true);

// --- Center: digital clock, 24h (DESKTOP §2.2), mono reads as "machine time" ---
var clock = topPanel.addWidget("org.kde.plasma.digitalclock");
clock.currentConfigGroup = ["Appearance"];
clock.writeConfig("use24hFormat", 2);   // 2 = force 24-hour
clock.writeConfig("showDate", false);    // slim chrome: time only, calm (§1)
clock.writeConfig("dateDisplayFormat", "BesideTime");

// --- Elastic spacer → balances the right side, keeps the clock centered ---
var spacerRight = topPanel.addWidget("org.kde.plasma.panelspacer");
spacerRight.currentConfigGroup = ["General"];
spacerRight.writeConfig("expanding", true);

// --- Right: system tray (network, sound, notifications). The memory-mode
//     indicator (persistent/amnesiac) lands here in Phase 3 (MEMORY.md §5). ---
var tray = topPanel.addWidget("org.kde.plasma.systemtray");
tray.currentConfigGroup = ["General"];
// Keep the tray quiet and predictable — scroll actions off (calm, §1).
tray.writeConfig("scaleIconsToFit", true);

// ---------------------------------------------------------------------------
// 2. DOCK — centered, auto-hiding, icons only (the terminal is the scene)
// ---------------------------------------------------------------------------
var dock = new Panel;
dock.location = "bottom";
// 48px = space-12 (§4.2): comfortable icon target (≥ 32px rule, §15) with room
// for the active-app dot indicator.
dock.height = 48;
// Floating glass slab, radius-lg corners (§11.3), consistent with the top panel.
dock.floating = true;
// Auto-hide: the dock yields the stage to Ghostty/Zellij (DESKTOP §1 / §2.3).
dock.hiding = "autohide";
// Centered, hugging its icons (not stretched across the screen).
dock.alignment = "center";
dock.lengthMode = "fit";

// --- Icons-only task manager with the default pinned tools (DESKTOP §2.3) ---
var tasks = dock.addWidget("org.kde.plasma.icontasks");
tasks.currentConfigGroup = ["General"];
// Default pins (order per DESKTOP §2.3). These are user-editable defaults, not
// a lock-in. Ghostty ships in v0.1; the rest are the curated stack (ECOSYSTEM).
// Icons-only manager → active app is marked with a Mauve dot (accent, §11.3).
tasks.writeConfig("launchers", [
    "applications:com.mitchellh.ghostty.desktop",       // terminal — the scene
    "applications:codium.desktop",                       // VSCodium
    "applications:firefox.desktop",                      // Firefox
    "applications:org.kde.dolphin.desktop",              // Dolphin (files)
    "applications:org.kde.plasma-systemmonitor.desktop"  // system monitor
]);
// Group by app, do not show only the current desktop's tasks (single scene).
tasks.writeConfig("groupingStrategy", 1);
tasks.writeConfig("showOnlyCurrentDesktop", false);
tasks.writeConfig("showOnlyCurrentActivity", true); // scope tasks to the activity

// ---------------------------------------------------------------------------
// Done. Two calm, floating, glass-ready surfaces. Everything expressive (agent
// state, tiers, gauges) lives in the additive Quickshell HUD — Phase 2.
// ---------------------------------------------------------------------------
