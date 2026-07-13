<div align="center">

# 🌀 VibeOS

**Ein unveränderliches Betriebssystem, das leer geboren wird,<br/>in dem die KI ein Bürger des Systems ist – keine installierte Anwendung.**

[![CI](https://github.com/Micka420-collab/vibeos/actions/workflows/ci.yml/badge.svg)](https://github.com/Micka420-collab/vibeos/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/lizenz-Apache--2.0-blue.svg)](LICENSE)
[![Arch](https://img.shields.io/badge/arch-amd64%20%2B%20arm64-8a2be2.svg)](docs/HARDWARE.md)
[![Base](https://img.shields.io/badge/basis-Fedora%20Kinoite%2042-51a2da.svg)](os/Containerfile)
[![Desktop](https://img.shields.io/badge/desktop-KDE%20Plasma%206-1d99f3.svg)](docs/DESKTOP.md)
[![Status](https://img.shields.io/badge/status-pre--alpha%20v0.1-orange.svg)](STATUS.md)

**🌐 [Français](README.md) · [English](README.en.md) · [Español](README.es.md) · Deutsch**

</div>

> ℹ️ Dies ist eine Übersetzung der Haupt-README. VibeOS ist ein französischsprachiges Projekt: Die ausführlichen Dokumente unter `docs/` werden auf Französisch gepflegt (der kanonischen Sprache des Projekts). Diese Seite verlinkt direkt darauf.

VibeOS ist eine **KI-native, unveränderliche, von Grund auf sichere** Linux-Distribution für das *Vibecoding*. Abgeleitet von Fedora Kinoite (KDE Plasma 6) und als Image mit bootc/OSTree gebaut, stellt sie die Systemsteuerung KI-Agenten über einen strikten Vertrag zur Verfügung – einen Systemdaemon (`vibed`), einen MCP-Server, eine Policy-Engine und ein Audit-Log – statt rohen Shell-Zugriff. Das Betriebssystem wird **leer** ausgeliefert: Sein Speicher wird beim ersten Start durch eine *Genesis*-Sequenz erzeugt und gehört seinem Benutzer, und niemandem sonst (die LUKS-Verschlüsselung dieses Speichers kommt in **Phase 3** – siehe [ROADMAP.md](ROADMAP.md)). Ein mehrjähriges Projekt: Das v0.1-Fundament steht – signiertes Multi-Arch-Image, zwei ISOs, `vibed`-Daemon beim Start aktiv, Vibecoding-Desktop ausgeliefert.

> 📊 **Wo steht das Projekt?** Der lebende Status (erledigt / in Arbeit / offen) steht in **[STATUS.md](STATUS.md)**.

---

## Kernfunktionen

### 🌱 Leer geboren — Genesis
Das OS-Image enthält **keinen Werksspeicher**. Beim ersten Start führt `vibeos-genesis.service` (abgesichert durch `ConditionPathExists=!/var/lib/vibeos/memory/.initialized`) `/usr/libexec/vibeos/genesis.sh` aus und baut den Speicher der Maschine von Grund auf unter `/var/lib/vibeos/memory` auf – **im Klartext in v0.1**; die LUKS/TPM2-Verschlüsselung des Volumes ist ein Ergebnis der **Phase 3**. Der **amnestische Modus** (inspiriert von Tails) erzeugt diesen Speicher **bei jedem Start** in tmpfs neu – nichts überlebt das Herunterfahren: Der **systemd-Generator** ist **ausgeliefert** (aktiviert durch den Kernel-Parameter `vibeos.amnesic=1`); seine Validierung in der VM bleibt **Phase 3**. Die vollständige Spezifikation steht in [docs/MEMORY.md](docs/MEMORY.md).

### 🤖 Die KI als Bürger des Systems — vibed + MCP + Policies
> **Status:** Das `vibed`-Binary ist **im Image eingebettet** (mehrstufig in `os/Containerfile` kompiliert, installiert unter `/usr/bin/vibed`). Beim Start startet `vibed.service` (per Preset aktiviert), **lädt und erzwingt die installierte Policy** (`/etc/vibeos/policy.d/`, fail-closed), **bedient den MCP-Server** unter `/run/vibed/mcp.sock` und **auditiert** jeden Aufruf unter `/var/lib/vibeos`. Auf Client-Seite liefert das Image die **MCP-Konfiguration von Claude Code** (`/etc/skel/.claude.json`): Der `vibeos`-Server wird **ohne manuelle Konfiguration** entdeckt (Voraussetzung: die Gruppe `vibeos-agents`). Noch ausstehend: das **Sandboxing pro Werkzeug** (systemd-run, seccomp, landlock – **Phase 3**) und die Härtung (dediziertes SELinux `vibed_t`, `User=vibed`, externe TPM/Rekor-Verankerung der Audit-Kette – **Phase 4**). Das `vibed`-Crate ist getestet (114 grüne Tests, darunter 5 End-to-End-MCP-Integrationstests über Socket + 2 Policy-Tests); das Audit-Log ist **per SHA-256-Hash verkettet** (`vibed --verify-audit`).

Der Systemdaemon `vibed` (Rust, tokio, Unit `vibed.service`) stellt die Systemsteuerung über einen **MCP-Server** (JSON-RPC 2.0) auf dem Unix-Socket `/run/vibed/mcp.sock` bereit. Jede Agentenaktion durchläuft eine **Policy-Engine** (`/etc/vibeos/policy.d/*.toml`, die erste passende Regel gewinnt, standardmäßig Ablehnung), organisiert in Fähigkeitsstufen:

| Stufe | Umfang | Menschliche Freigabe |
|---|---|---|
| **T0** | Beobachten (nur lesen) | Nein |
| **T1** | Benutzeränderung (Dateien, Konfiguration) | Nein (konfigurierbar) |
| **T2** | Systemänderung (Pakete, Dienste) | **Ja, immer** |
| **T3** | Destruktiv (Datenträger, Zugangsdaten, Netzwerkidentität) | **Ja, immer** |

Jeder Werkzeugaufruf wird in einem **anfüge-only JSONL-Audit-Log, per SHA-256-Hash verkettet, mit Rotation pro UTC-Tag** (`/var/lib/vibeos/audit/vibed-<datum>.jsonl`) protokolliert, samt Identität des Aufrufers (uid/gid/pid) – jede Manipulation wird durch `vibed --verify-audit` erkannt. Die externe Verankerung der Kette (TPM/Rekor) bleibt **Phase 4**.

Der **menschliche Freigabe-Ablauf für T2/T3** ist auf der Klempnerei-Seite ausgeliefert: Eine Anfrage erzeugt einen protokollierten Eintrag, der Operator führt `vibectl approve <id>` aus, und ein **Einmal-Grant** (gebunden an `(Werkzeug, Ziel, uid)`, kurzlebig) autorisiert den erneuten Aufruf – ein Agent kann seine eigene Anfrage **niemals** freigeben (nur-root-Speicher), und das Audit hält fest, *wer* freigegeben hat (`ok_approved(by_uid=N)`). Ein **Rate-Limiting pro uid** (Token-Bucket) begrenzt einen entlaufenen oder kompromittierten Agenten (Anti-Flood, auditierte Ablehnung). Der Plasma/HUD-Dialog und die echten T2-Backends kommen in **Phase 4**.

### 🔒 Unveränderlichkeit & überprüfbare Sicherheit
Bereits in v0.1 ausgeliefert: nur lesbares Root, atomare Updates und Werks-Rollback (bootc/OSTree), SELinux `enforcing` (Fedoras targeted-Policy), OS-Images **mit sigstore/cosign in der CI signiert**, Basis-Image per Digest gepinnt und KI-CLIs auf exakte Versionen gepinnt. Geplant: gemessene Boot-Kette **UEFI Secure Boot → UKI → dm-verity/composefs** (**Phase 4**), Sandbox pro Werkzeug – systemd-run, seccomp, landlock (**Phase 3**), dedizierte SELinux-Policy `vibed_t` (**Phase 4**). Image-Referenz: `ghcr.io/micka420-collab/vibeos`.

### 🧰 Vollständiger Vibecoding-Werkzeugkasten — Cloud + lokal
Hybride Agenten-Laufzeit, vorinstalliert und im Image gepinnt: **Claude Code** und das **Claude Agent SDK** (Anthropic-Cloud), **gemini-cli** (Google), **codex** (OpenAI), **opencode** (Multi-Provider-Terminal-Agent, 100 % lokal über ollama) und **ollama** für lokale Modelle – das Image bündelt alles, um offline zu programmieren (die formale Validierung „`ollama run` ohne Netz" ist ein Ausgangskriterium der Phase 1, noch offen). `aider` bleibt bei Bedarf installierbar (`uvx --python 3.12 aider-chat`), ohne das unveränderliche OS zu berühren.

### 🛡️ Regierter Cybersecurity-Werkzeugkasten
VibeOS ist **security-first**: Ein professioneller Pentest/DFIR-Werkzeugkasten ist im Image eingebettet (≈ 60 signierte Fedora/RPM-Fusion-RPMs – `nmap`, `hashcat`, `radare2`, `aircrack-ng`, `impacket`, `sleuthkit`, `suricata`, `lynis`…), im Stil von Kali/Parrot/BlackArch. Der Unterschied: Er wird von der Policy-Engine **regiert**. Ein KI-Agent kann den Werkzeugkasten schreibgeschützt **entdecken** (MCP-Werkzeug `sectools.list`, T0), aber **kein Werkzeug ausführen**, ohne die Stufung zu durchlaufen – alles, was **aktiv gegen ein Ziel ist, ist T2, Destruktives ist T3**, mit **verpflichtender menschlicher Freigabe**. Vollständiger Katalog (Stand der Technik 2025–2026, einschließlich **KI/LLM-Sicherheit**: garak, PyRIT, guardrails) und Rahmen für die autorisierte Nutzung: [docs/SECURITY-TOOLKIT.md](docs/SECURITY-TOOLKIT.md).

### 🧬 Multi-Architektur — amd64 + arm64
VibeOS zielt auf **linux/amd64 und linux/arm64**. Seit dem Release `v0.1.0-dev` baut die CI beide Architekturen auf **nativen Runnern**, veröffentlicht das **cosign-signierte Multi-Arch-Manifest** (keyless, Rekor-Log) auf ghcr.io und erzeugt **eine ISO pro Architektur** als Release-Artefakte. Die **NVIDIA**-Treiberschicht (akmod, RPM Fusion) wird beim Image-Build kompiliert, nur auf amd64; ihre **Validierung auf dem Referenz-PC** (RTX 3070 Ti) ist ein Ausgangskriterium der Phase 1, noch offen – siehe [docs/HARDWARE.md](docs/HARDWARE.md).

### 🎨 Ein für das Vibecoding gestaltetes Desktop-Erlebnis
Ein Plasma-6-Desktop, organisiert um das Triptychon **Agent / Kontext / Vertrauen**. Die Sitzung öffnet sich im **Global Theme „VibeOS Dark"** (Systemstandard, Kvantum-Engine inklusive) mit dem **Agenten-HUD** (Quickshell, ins Image kompiliert, automatisch gestartet – es wird den Agentenstatus, die aktuelle Policy-Stufe und die Anzeigen des lokalen Modells zeigen; **gemockte Daten, bis seine Live-Anbindung an `vibed` programmiert ist**, aus Ehrlichkeit). Das Terminal ist ab dem ersten Start einsatzbereit: Ghostty + fish + Starship + Zellij mit dem charakteristischen Layout „Agent + lazygit + Audit", Neovim-Preset „VibeVim". Diese Auswahl ist das Ergebnis einer **Kuratierung von 113 Open-Source-Projekten**, gefiltert nach weiterverteilbarer Lizenz und Kohärenz – detailliert in [docs/ECOSYSTEM.md](docs/ECOSYSTEM.md) und [docs/DESKTOP.md](docs/DESKTOP.md).

---

## Ausgeliefert in v0.1 / Unterwegs

| Fähigkeit | Status |
|---|---|
| Unveränderliches bootc-Image (Fedora Kinoite, RO-Root, atomares Rollback) | ✅ Ausgeliefert v0.1 |
| **amd64**-Image + ISO (lokaler Build + CI) | ✅ Ausgeliefert v0.1 |
| **arm64**-Manifest + ISO pro Architektur (native Runner, Release `v0.1.0-dev`) | ✅ Ausgeliefert v0.1 |
| ISO-Boot in VM validiert + NVIDIA auf dem Referenz-PC validiert | 🔄 Ausgangskriterien Phase 1 (in Arbeit) |
| KI-CLIs vorinstalliert und gepinnt (claude, agent SDK, gemini, codex, opencode, ollama) | ✅ Ausgeliefert v0.1 |
| cosign-Signierung (keyless) der Images in der CI | ✅ Ausgeliefert v0.1 |
| Policy-Dateien abgelegt in `/etc/vibeos/policy.d/` | ✅ Ausgeliefert v0.1 |
| `vibed`-Binary im Image eingebettet (startet beim Boot) | ✅ Ausgeliefert v0.1 |
| `vibed`-MCP-Server auf `/run/vibed/mcp.sock` | ✅ Ausgeliefert v0.1 |
| Policy-Laden/-Erzwingung durch `vibed` (fail-closed) | ✅ Ausgeliefert v0.1 |
| SHA-256-hashverkettetes JSONL-Audit-Log (`/var/lib/vibeos/audit/`, eine Datei pro Tag) mit Aufrufer-Identität | ✅ Ausgeliefert v0.1 |
| **Menschlicher T2/T3-Freigabe-Ablauf** (Klempnerei: `vibectl approve/deny`, Einmal-Grants, Freigebender auditiert) | ✅ Ausgeliefert (Phase 2) |
| **Rate-Limiting pro uid** (Token-Bucket, Anti-Flood; begrenzter Freigabe-Speicher) | ✅ Ausgeliefert (Phase 2) |
| Genesis beim ersten Boot (Speicher **im Klartext** erzeugt, Unit + `genesis.sh`) | ✅ Ausgeliefert v0.1 |
| Global Theme **VibeOS Dark standardmäßig** (`/etc/xdg/kdeglobals` + Kvantum) | ✅ Ausgeliefert (Phase 2) |
| **Quickshell-HUD** installiert + automatisch gestartet (Runtime ins Image kompiliert) | ✅ Ausgeliefert (Phase 2) — gemockte Daten |
| **Claude-Code-MCP-Konfiguration** ausgeliefert (`/etc/skel/.claude.json` → `vibed`-Socket) | ✅ Ausgeliefert (Phase 2) |
| **Live**-Anbindung des HUD an den `vibed`-Socket (QML `Quickshell.Io`) | 🛣️ Phase 2 |
| **`memory.append`** (T1, additiv: journal + knowledge) · `scope`/`limit` von `memory.query` | ✅ Ausgeliefert (Phase 2) |
| Zusätzliche **echte T1-Werkzeuge** · `user`/`projects`-Scopes von `memory.append` | 🛣️ Phase 2 |
| **Agenten-Supervisor** mit Budgets + **Always-on-Autonomiemodus** (voll T0/T1, T2/T3 asynchron in Warteschlange – Freigabe-Schwelle nie gesenkt) | 🛣️ Phase 2.5 (vorgeschlagen) |
| **Erfassung des Reasonings** der Agenten (Tap auf den `stream-json`-Strom) + T0-Werkzeug `agent.thinking` | 🛣️ Phase 2.5 (vorgeschlagen) |
| **Abo-Authentifizierung** (setup-token) TPM2-versiegelt + Egress-Allowlist pro Unit | 🛣️ Phase 2.5 (vorgeschlagen) |
| LUKS/TPM2-Verschlüsselung des Speichers | 🛣️ Phase 3 |
| Amnestischer Modus (tmpfs bei jedem Boot neu erzeugt, systemd-Generator) | 🛣️ Phase 3 |
| Geburts-Interview (Prototyp: `agent/genesis_interview.py`, in v0.1 nicht verdrahtet) | 🛣️ Phase 3 |
| Sandbox pro Werkzeug (systemd-run, seccomp, landlock) | 🛣️ Phase 3 |
| UKI / gemessener Boot, hashverkettetes Audit, dediziertes SELinux, `User=vibed` | 🛣️ Phase 4 |
| Geführter Installer, Datenträgerverschlüsselung standardmäßig | 🛣️ Phase 5 |

Schreibregel des Projekts: Kein nicht implementierter Mechanismus wird im Präsens beschrieben – jedes Dokument unterscheidet „ausgeliefert in v0.1" von „Phase N (spezifiziert)".

---

## Architektur auf einen Blick

```mermaid
flowchart LR
    subgraph AGENTS["MCP-Clients"]
        CC["Claude Code / Agent SDK (Cloud)<br/>gelieferte Konfig: /etc/skel/.claude.json"]
        OL["Lokale Modelle (ollama)"]
        AD["opencode · gemini · codex"]
        HUD["Quickshell-HUD (T0, nur lesen)<br/>(Live-Anbindung: Phase 2)"]
    end
    subgraph VIBED["vibed — Systemdaemon (Rust)"]
        MCP["MCP-Server · JSON-RPC 2.0<br/>/run/vibed/mcp.sock"]
        POL["Policy-Engine<br/>/etc/vibeos/policy.d/*.toml<br/>T0 → T3"]
        AUD["JSONL-Audit-Log<br/>/var/lib/vibeos/audit/ (pro Tag)"]
    end
    subgraph OS["Unveränderliches VibeOS (bootc/OSTree)"]
        SYS["Dienste · Pakete · Dateien"]
        MEM[("Speicher /var/lib/vibeos/memory<br/>von Genesis erzeugt<br/>(LUKS: Phase 3)")]
    end
    CC --> MCP
    OL --> MCP
    AD --> MCP
    HUD -.-> MCP
    MCP --> POL
    POL -->|"erlaubt"| SYS
    POL --> AUD
    SYS --- MEM
```

---

## Repository-Struktur

| Verzeichnis | Inhalt |
|---|---|
| `docs/` | Dokumentation: Architektur, Build ([docs/BUILD.md](docs/BUILD.md)), Referenz-Hardware ([docs/HARDWARE.md](docs/HARDWARE.md)), Speicher, Sicherheit, Entscheidungen |
| `os/` | bootc/OSTree-Image-Definition (abgeleitet von Fedora Kinoite, KDE Plasma 6, Multi-Arch) |
| `vibed/` | Systemdaemon `vibed` (Rust, tokio): MCP-Server, Policy-Engine, Audit |
| `agent/` | Agenten-Laufzeit: Claude-Code-/Agent-SDK-Integration, ollama, opencode, Genesis-Interview-Prototyp |
| `memory/` | Speicher-Subsystem: Genesis-Sequenz (`memory/genesis.sh`) |
| `security/` | Policies (`policy.d`), Härtung, Signierung |
| `desktop/` | Desktop-Arbeit: VibeOS-Dark-Theme, Stufen-Palette, Quickshell-HUD (QML) — siehe [docs/DESKTOP.md](docs/DESKTOP.md) |
| `installer/` | Installer: kickstart, Branding, Logo — siehe [docs/INSTALLER.md](docs/INSTALLER.md) |
| `.github/` | GitHub-Actions-CI: Tests (`ci.yml`), Multi-Arch-OS-Image-Build, cosign-Signierung, Push zu ghcr.io, ISO-Erzeugung |

---

## Schnellstart

### Das Image ausprobieren (ohne etwas zu bauen)

```bash
# Das Multi-Arch-Image holen (amd64 / arm64):
podman pull ghcr.io/micka420-collab/vibeos:0.1.0-dev

# Die cosign-Signatur prüfen (keyless, GitHub-Actions-CI):
cosign verify ghcr.io/micka420-collab/vibeos:0.1.0-dev \
  --certificate-identity-regexp 'https://github.com/Micka420-collab/vibeos/.*' \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com
```

Die **installierbaren ISOs** (eine pro Architektur) werden bei jedem `v*`-Tag als CI-Artefakte erzeugt – siehe [docs/BUILD.md](docs/BUILD.md), um sie lokal zu erzeugen.

### Mit `vibed` aus einer VibeOS-Sitzung sprechen

Socket-Zugriff: **Administratoren (Gruppe `wheel`) werden bei jedem Boot automatisch** in `vibeos-agents` eingetragen (`vibeos-agents-group.service`) – sie haben bereits `sudo`, es ist also *weniger* als das, was sie ohnehin besitzen. Ein **Nicht-`wheel`**-Konto bleibt opt-in: `sudo usermod -aG vibeos-agents <benutzer>` (dann die Sitzung neu öffnen). Die Mitgliedschaft wird beim nächsten Login wirksam.

```bash
# Claude Code entdeckt den MCP-Server „vibeos" automatisch
# (Konfig in ~/.claude.json geliefert; Anweisungen in ~/.claude/CLAUDE.md).
# Manueller Test ohne MCP-Client:
printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' \
  | socat - UNIX-CONNECT:/run/vibed/mcp.sock
```

### Das Image selbst bauen

Der lokale Build läuft unter **WSL2 Ubuntu + podman** (der Windows-Host braucht weder docker noch podman). Die GitHub-Actions-CI baut das Multi-Arch-OS-Image auf nativen Runnern, signiert es mit cosign, pusht es nach `ghcr.io` und erzeugt die ISOs mit `bootc-image-builder`.

```bash
git clone https://github.com/Micka420-collab/vibeos.git
cd vibeos
podman build -t vibeos:dev -f os/Containerfile .
```

➡️ **Alle detaillierten Anweisungen (Voraussetzungen, ISOs, Veröffentlichung) stehen in [docs/BUILD.md](docs/BUILD.md).**

---

## Projektstatus

| | |
|---|---|
| **Phase** | Pre-Alpha — Phase 1 „Erste ISO" (VM-Validierung ausstehend) · Phase 2 „vibed + MCP" in Arbeit · Phase 2.5 „Regierte Autonomie" vorgeschlagen |
| **Letzte Aktualisierung** | 2026-07-13 |
| **OS-Image** | `ghcr.io/micka420-collab/vibeos:0.1.0-dev` — amd64- + arm64-Manifest, **cosign-signiert** (Rekor) |
| **ISO** | amd64 (7,0 GB) + arm64 (6,3 GB) — CI-Artefakte des Releases `v0.1.0-dev` |
| **Build** | Grüne CI (native Runner, ~15 min/Arch) · `bootc container lint` OK · 114 grüne `vibed`-Tests |
| **Referenzmaschine** | Ryzen 7 3700X + RTX 3070 Ti + 16 GB — [docs/HARDWARE.md](docs/HARDWARE.md) |
| **Erwarten Sie** | Brüche, Umschreibungen, null Stabilitätsgarantie |

VibeOS ist ein **mehrjähriges** Projekt. v0.1 legt ein vollständiges, kohärentes, baubares Repository an – kein fertiges Produkt. Die Tabelle „Ausgeliefert in v0.1 / Unterwegs" oben ist maßgeblich dafür, was tatsächlich existiert.

---

## Weiterführendes

**Vision & Steuerung**
- 📜 [VISION.md](VISION.md) — das Manifest: warum VibeOS existiert, seine fünf Gründungsprinzipien
- 🗺️ [ROADMAP.md](ROADMAP.md) — die mehrjährige Trajektorie, Meilenstein für Meilenstein
- 📊 [STATUS.md](STATUS.md) — der lebende Status (erledigt / in Arbeit / offen)

**Design**
- 🏛️ [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) — die geschichtete Architektur (Diagramme, Sequenzen)
- 🧭 [docs/DECISIONS.md](docs/DECISIONS.md) — die Architekturentscheidungen (ADR)
- 🧠 [docs/MEMORY.md](docs/MEMORY.md) — das Speicher-Subsystem und Genesis
- 🎨 [docs/DESKTOP.md](docs/DESKTOP.md) · 🧩 [docs/ECOSYSTEM.md](docs/ECOSYSTEM.md) — der Vibecoding-Desktop und die OSS-Auswahl

**Sicherheit**
- 🛡️ [SECURITY.md](SECURITY.md) — Sicherheitsrichtlinie und Meldung von Schwachstellen
- 🎯 [docs/THREAT-MODEL.md](docs/THREAT-MODEL.md) · 🔐 [docs/SECURITY-ARCHITECTURE.md](docs/SECURITY-ARCHITECTURE.md)

**Bauen & installieren**
- 🔨 [docs/BUILD.md](docs/BUILD.md) — Image-Build, ISOs, Veröffentlichung
- 💿 [docs/INSTALLER.md](docs/INSTALLER.md) — der Installer und der erste Start
- 🖥️ [docs/HARDWARE.md](docs/HARDWARE.md) — Zielarchitekturen und Referenzmaschine

## Lizenz

Verteilt unter der **Apache-2.0**-Lizenz. Siehe [LICENSE](LICENSE).
