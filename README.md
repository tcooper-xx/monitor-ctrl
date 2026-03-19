# Monitor Ctrl

A lightweight Windows system-tray application for switching monitor inputs via DDC/CI. Control your displays from the tray, set up global hotkeys, and never touch the monitor's physical buttons again.

## Features

- **Tray menu input switching** — right-click the tray icon to switch any monitor to any input instantly
- **Live input tracking** — the tray menu checkmark always reflects the actual active input, polling DDC every 3 seconds to stay in sync with manual switches
- **Cycle hotkeys** — assign a single key combo to toggle a monitor between two inputs (e.g. HDMI-2 ↔ DP-1)
- **Per-input hotkeys** — bind a key combo to switch a specific monitor to one specific input
- **Apply Default Inputs** — one click (or one hotkey) to send all monitors back to their configured defaults
- **Custom input names** — rename DDC-reported inputs to meaningful labels; hide inputs you never use
- **Start at Login** — optional auto-launch via the Windows registry (HKCU, no admin required)
- **Dark mode aware** — tray menu follows the system light/dark theme automatically

## Requirements

- Windows 10 or Windows 11
- Monitors connected via a cable that carries DDC/CI (DisplayPort and HDMI both work; passive adapters and KVM switches sometimes break DDC)
- [Rust toolchain](https://rustup.rs/) 1.70 or later (for building from source)

> **Note:** Laptop internal screens (LVDS/eDP panels) are automatically ignored — they don't support DDC/CI input switching.

## Building from Source

### 1. Install Rust

If you don't have Rust installed, get it from [rustup.rs](https://rustup.rs/). The installer will set up `rustc` and `cargo`.

### 2. Clone or download the repository

```
git clone <repository-url>
cd MonitorCtrl
```

### 3. Build

**Debug build** (faster compile, larger binary, console log output):

```
cargo build
```

**Release build** (optimised, smaller binary, recommended for daily use):

```
cargo build --release
```

The compiled binary is placed at:

- Debug: `target\debug\monitor-ctrl.exe`
- Release: `target\release\monitor-ctrl.exe`

The build script (`build.rs`) automatically generates the tray icon (`assets\icon.ico`) at compile time — no manual asset preparation is needed.

### 4. Run

```
cargo run
```

Or run the compiled binary directly. The application appears in the system tray with no taskbar window.

## Usage

### Switching inputs

Right-click the tray icon. The menu lists each detected monitor with its available inputs. The currently active input is shown with a checkmark. Click any input to switch immediately.

### Settings

Open **Settings...** from the tray menu to configure:

| Section | What you can do |
|---|---|
| **General** | Toggle Start at Login; set the Apply Default Inputs hotkey |
| **Monitors** | Rename a monitor, set its default input, rename/hide individual inputs |
| **Cycle Hotkeys** | Assign a key combo that flips one monitor between two inputs |
| **Per-Input Hotkeys** | Assign a key combo that sends one monitor to one specific input |

Changes take effect immediately after clicking **Save**.

### Override inputs (advanced)

If a monitor reports inputs with generic names like `Input 0x1B`, open the **Override inputs** section inside that monitor's settings panel. You can:

- **Rename** any input to a meaningful label (e.g. rename `Input 0x1B` to `USB-C`)
- **Hide** inputs from the tray menu using the **Show** checkbox — useful for ports you never use
- **Edit the VCP hex code** if a monitor uses a non-standard code
- **Add** entries manually for monitors that don't report their inputs via DDC

### Hotkey syntax

Hotkeys are entered as `Modifier+Modifier+Key`, for example:

```
Ctrl+Alt+1
Shift+F3
Ctrl+Alt+Shift+F12
```

Supported modifiers: `Ctrl`, `Alt`, `Shift`, `Win`
Supported keys: `0–9`, `A–Z`, `F1–F12`

### Apply Default Inputs

Set a **Default input** for each monitor in Settings. Then use **Apply Default Inputs** from the tray menu (or the configured hotkey) to send all monitors to their defaults at once — useful when returning to your desk after connecting a laptop.

## Configuration

Settings are stored in `config.toml` next to the executable. The file is managed by the app — you don't need to edit it manually, but it's plain TOML if you want to inspect or back it up.

Example:

```toml
start_at_login = true

[[cycle_hotkeys]]
monitor_id = "DISPLAY\\DELA0BA\\4&1a2b3c4d&0&UID256"
input_a = 18
input_b = 15
hotkey = "Shift+F3"

[monitors."DISPLAY\\DELA0BA\\4&1a2b3c4d&0&UID256"]
name = "Dell S2725QC"
default_input = 27

[monitors."DISPLAY\\DELA0BA\\4&1a2b3c4d&0&UID256".inputs]
"HDMI-2" = 18
"DP-1" = 15
"USB-C" = 27
```

## Troubleshooting

**Monitor doesn't appear in the tray menu**
DDC/CI may be disabled in the monitor's OSD settings. Look for a "DDC/CI" or "Smart Control" option and enable it. Some monitors also require a brief delay after powering on before DDC becomes available.

**Input switch does nothing / wrong input switches**
The VCP input codes (hex values) are manufacturer-defined and occasionally non-standard. Use **Override inputs** in Settings to set the correct code for each input. Tools like [ControlMyMonitor](https://www.nirsoft.net/utils/control_my_monitor.html) can help identify the correct values.

**Inputs show as `Input 0x??`**
The monitor's DDC capabilities string doesn't include standard input names. Open Settings → the affected monitor → **Override inputs** (the section will auto-open with a warning) and rename each entry.

**Hotkey doesn't register**
Another application may have already registered the same key combo system-wide. Try a different combination.

**"Access denied" when building**
The previous instance of `monitor-ctrl.exe` is still running. Kill it via Task Manager or `taskkill /F /IM monitor-ctrl.exe`, then rebuild.

## Architecture

| File | Purpose |
|---|---|
| `src/main.rs` | Win32 message pump, event loop, wires all modules together |
| `src/config.rs` | Load/save `config.toml` via serde + toml |
| `src/ddc.rs` | DDC/CI enumeration, VCP 0x60 read/write, capabilities string parser |
| `src/tray.rs` | System tray icon and context menu (tray-icon + muda) |
| `src/hotkeys.rs` | Global hotkey registration and dispatch (global-hotkey) |
| `src/settings_ui.rs` | Settings window (egui/eframe) |
| `src/startup.rs` | Start-at-login toggle via Windows registry (auto-launch) |
| `build.rs` | Compile-time icon generation — produces `assets/icon.ico` |

## Known Limitations

- DDC/CI is not supported on virtual displays, displays behind passive adapters, or some KVM switches
- NVIDIA Surround and AMD Eyefinity merge multiple monitors into one virtual display, preventing individual DDC control
- The VCP 0x60 input codes for codes above `0x1D` are vendor-specific with no reliable cross-vendor mapping; manual override is required
