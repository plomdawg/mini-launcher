# ADR 0001: Desktop stack — tray, settings window, process model, installer

- Status: accepted (maintainer decision on the PR: egui instead of Slint)
- Date: 2026-11-16
- Issue: #41

## Context

M3 turns the CLI into a per-user Windows tray app (status, pause, settings, quit; no console
window; start at login; per-user installer). M4 adds a settings window: game library list,
add-game wizard (Steam detection, tap a tag, write/bind it), launch-option forms,
export/import. Windows ships first; Linux follows later. Constraints: small binary, keyboard
use, licenses compatible with MIT OR Apache-2.0 with no attribution duty, no extra runtime to install, and every tool buildable in GitHub Actions with pinned versions.

Since this was filed, `run` already gained a named-mutex single instance
(`Local\mini-launcher`), a per-user `HKCU\…\Run` value named `Mini Launcher`
(`autostart enable|disable|status`), reconnect, and a rolling log file. The decisions below
build on those.

## Decision

1. **Tray: `tray-icon` + `muda`** (tauri-apps, MIT OR Apache-2.0).
2. **Settings window / wizard: egui/eframe 0.37 with the wgpu backend**, with a custom
   retro theme (see [Visual direction](#visual-direction)).
3. **Process model: two executables from one workspace.** The existing console-subsystem
   `mini-launcher.exe` stays the CLI; a new GUI-subsystem `mini-launcher-tray.exe` (new crate
   `app/tray`) hosts the tray icon and the settings window (amended in #76: on two UI
   threads, see [Process model](#process-model)).
4. **Installer: Inno Setup 6**, per-user (`PrivilegesRequired=lowest`), with a pinned
   ISCC download in CI.

## Measurements

Throwaway prototypes (not committed) built on `windows-latest` with the pinned toolchain
(1.100.0). Each GUI prototype is the same mock: 20-item library list, name/type/payload form
(two text fields, one combo box), three buttons. "small" = `lto = "fat"`,
`codegen-units = 1`, `strip = true`, `opt-level = "s"`; "release" = Cargo defaults.

| Binary | release | small | Start → 2nd frame (warm) | Idle working set / private |
|---|---|---|---|---|
| current CLI `mini-launcher.exe` (reference) | 2.10 MiB | 1.45 MiB | — | — |
| `tray-icon` + `muda` only (Win32 message pump) | 0.38 MiB | 0.30 MiB | ~8 ms to icon | 9.2 / 1.6 MiB |
| egui/eframe, glow (OpenGL) | 6.58 MiB | 5.01 MiB | **failed to start** (no OpenGL 2+ on the runner) | — |
| egui/eframe, wgpu | 12.81 MiB | 9.05 MiB | ~170 ms (500 ms first run) | 49 / 277 MiB¹ |
| egui/eframe wgpu + tray | 13.01 MiB | 9.18 MiB | ~175 ms | 49 / 276 MiB¹ |
| Slint, default features (femtovg + software) | 14.51 MiB | 10.61 MiB | ~6 ms² (212 ms first run) | 23 / 5.2 MiB |
| Slint, software renderer only | — | 9.79 MiB | ~5 ms (205 ms first run) | 22 / 4.8 MiB |
| Slint software + tray (the M3/M4 shape) | — | 9.82 MiB | ~10 ms | 23 / 4.9 MiB |

¹ The runner has no GPU, so wgpu ran on D3D12 WARP (software). Real GPUs will use far less
private memory, but this is exactly what users on VMs, Remote Desktop or broken drivers get.
² Slint's timer fires once the event loop runs; wall-clock including process exit was
~140 ms (default) / ~75 ms (software only) vs ~460 ms for egui-wgpu.

**Screen-reader tree** (UI Automation, enumerated from PowerShell on the runner; not a
priority for this app, recorded for completeness): both toolkits expose a real tree through
AccessKit — window, `Edit` ×2, `ComboBox`, labelled `Text`, buttons. Slint exposes the library
as `List` + `ListItem`; egui exposes each selectable row as a `Button`. egui 0.37 already
handles UIA `SetValue` on text fields (emilk/egui#8699, fixed by #8701 before the 0.37.0
tag); the prototype did not exercise it.

**Installer** (Inno Setup 6.7.1 preinstalled on the image; payload = the egui-wgpu tray
prototype 9.2 MiB + CLI 1.45 MiB, i.e. the chosen stack): **5.24 MiB setup.exe**, silent
per-user install in 2.2 s to `%LOCALAPPDATA%\Programs\Mini Launcher`; created the Start-menu
shortcut, the `HKCU\…\Run` `Mini Launcher` value (optional task) and an HKCU uninstall
entry; silent uninstall removed the directory and the Run value. (The runner account is an
admin, so "no UAC prompt for a standard user" still needs one manual check in M3.)

Not measured locally on Linux: Slint needs `libfontconfig1-dev` and `tray-icon` needs GTK 3 +
libayatana-appindicator headers to build there; the dev box had neither, so all numbers are
from the Windows runner. egui/glow built on Linux without extra headers (16.6 MiB release,
7.7 MiB small).

## Options considered

### Tray icon

| Option | Notes |
|---|---|
| **`tray-icon` 0.26 + `muda` 0.21** ✔ | Native `Shell_NotifyIcon` + native Win32 popup menu (Narrator/NVDA read native menus with no work from us). Check items, separators, tooltips, icon swap for status. 0.30 MiB. Maintained by the Tauri org; also what Tauri itself uses. Needs a Win32 message loop on the creating thread — the winit loop provides one. Linux: GTK 3 + appindicator (build-time dep, its own GTK loop thread). |
| `tray-item` | Smaller API, less active; no menu check items; would still need its own loop. |
| raw `windows` crate | Smallest, full control, but we'd own the menu/hidpi/taskbar-restart code; no Linux path. |
| `ksni` (Linux only) | Pure-Rust StatusNotifierItem over D-Bus, no GTK. Worth revisiting for the Linux tray instead of `tray-icon`'s GTK path. |

### Settings window / wizard

| Option | Size (small) | License | Verdict |
|---|---|---|---|
| **egui/eframe 0.37, wgpu** | 9.18 MiB with tray | MIT OR Apache-2.0 | **Chosen** |
| egui/eframe 0.37, glow | 5.15 MiB with tray | MIT OR Apache-2.0 | Rejected: needs OpenGL 2+, failed to start on the runner (as it would on VMs, Remote Desktop, missing drivers) |
| Slint 1.19 | 9.82 MiB with tray | GPL-3.0 OR Royalty-free 2.0 OR commercial | Rejected: the royalty-free license requires in-app (AboutSlint) or download-page attribution |
| iced 0.14 | not measured | MIT | Rejected: fewer ready-made form/list widgets, no upstream accessibility yet (iced-rs/iced#552) |
| Tauri 2 (WebView2) | not measured; ~1–3 MiB exe reported | MIT OR Apache-2.0 | Rejected: second stack (HTML/JS + IPC), WebView2 process tree (100+ MB) while open |
| Native Win32 (`windows` / native-windows-gui) | smallest | MIT/Apache | Rejected: list + wizard by hand is a lot of code, no Linux path, nwg unmaintained |

Why egui + wgpu: permissive license with no attribution duty, pure Rust (no DSL or build
step), quickest path to a list + form + wizard, and wgpu falls back to D3D12 WARP so the window
opens without GPU drivers. Costs: ~4 MiB over glow, ~170 ms to first frame, and while the
window is open a large private footprint on software rendering (277 MiB on WARP; much less
on a real GPU).

The tray must not pay that cost while idle: the wgpu device/surface exists only while the
settings window is open and is dropped on close (eframe hidden root viewport or our own
winit loop with `egui-winit` + `egui-wgpu` — decided in M3, verified by checking that private
memory returns to tray level after closing the window).

### Visual direction

Retro / low-tech, deliberately not generic-modern: no rounded corners, hard 1 px borders and
bevels, a pixel/bitmap font under OFL or a similar permissive license (no in-app credit), and a
small palette taken from the app icon (blue ≈ `#0b6aa2`, black, white). egui suits this well:
it draws every pixel itself, so `Visuals`/`Style` (corner radius 0, strokes, spacing, colours)
and a custom font cover the whole look with no native-widget fights, and the theme applies
identically on Linux later. The theme itself is designed in M4 with a mockup for sign-off.

### Process model

| Option | Verdict |
|---|---|
| **Two exes: `mini-launcher.exe` (console) + `mini-launcher-tray.exe` (`#![windows_subsystem = "windows"]`)** | **Chosen.** CLI behaviour is unchanged: `diagnose`/`scan` print, Ctrl+C works, `run --simulate` reads stdin, pipes work. The tray exe never flashes a console at login. Both are thin over `mini-launcher-core`. Cost: +1.45 MiB uncompressed, ~0.6 MiB in the installer. |
| One GUI-subsystem exe + `AttachConsole(ATTACH_PARENT_PROCESS)` for subcommands | Rejected. cmd/PowerShell don't wait for GUI-subsystem processes: the prompt returns immediately, output interleaves with it, Ctrl+C and stdin (`scan`, `run --simulate`) break. |
| One console exe that calls `FreeConsole()` in tray mode | Rejected: a console window flashes at every login. |

Consequences: the tray exe owns the reader and takes the existing `Local\mini-launcher`
mutex, so `run` and the tray exclude each other and `scan` keeps warning when the port is
held. `autostart enable` must point the `Mini Launcher` Run value at
`mini-launcher-tray.exe` (sibling of the CLI) instead of `mini-launcher.exe run`, otherwise
login opens a console. The reader loop stays on its worker thread.

Amended in #76: the tray keeps its own Win32 message loop on the main thread, and the settings
window runs a winit event loop on a dedicated `settings-window` thread (winit's
`with_any_thread`). Running the window with `run_and_return` on the tray's thread blocked the
tray's loop while the window was open (menu clicks, Quit and worker updates queued until it
closed). A shared winit loop driving both would mean rewriting the tray around winit; a second
thread leaves the tray untouched. winit allows one event loop per process, so the thread lives
as long as the app and reuses eframe's kept loop for every opening; each opening's window,
surface and wgpu device are still dropped on close. The tray talks to the window through the
window's `egui::Context` (focus, close on Quit) and gets opened/closed events over a channel.
CI checks the two windows live on different threads and that the tray answers messages while
the window is open.

### Installer

| Option | Verdict |
|---|---|
| **Inno Setup 6** | **Chosen.** Per-user without admin (`PrivilegesRequired=lowest`, `{autopf}` → `%LOCALAPPDATA%\Programs`), Start-menu entry, uninstaller + HKCU uninstall entry, optional "Start at login" task writing the same `Mini Launcher` Run value, `AppMutex=Local\mini-launcher` so setup/uninstall ask to close a running app, `CloseApplications` via Restart Manager. License permits commercial use (keep notices); jrsoftware.org *asks* for-profit users with revenue over US$5k/yr to buy a commercial license but states it is not strictly required — maintainer's call. Verified end-to-end on the runner (above). Don't pass `/CURRENTUSER`: with `PrivilegesRequired=lowest` the install is already per-user (the flag only matters when overrides are allowed), so leave `PrivilegesRequiredOverridesAllowed` out of the real `.iss`. |
| WiX v6/v7 (MSI) | Rejected: per-user MSI is awkward (HKCU key-path rules, no `%LOCALAPPDATA%\Programs` default), `cargo-wix` targets legacy WiX 3, and the official binaries carry the Open Source Maintenance Fee EULA: v6's EULA has no revenue floor; v7 (OSMF EULA v1.1) exempts revenue under US$10k/yr — kits are sold, so the fee may apply. |
| NSIS (directly or via `cargo-packager`) | Viable (zlib license, per-user mode). Not chosen: not on the runner image, weaker script language; `cargo-packager` is young and adds another config format. |
| MSIX | Rejected for now: requires signing and a package identity; revisit with SignPath. |

Pinning: the runner image's Inno Setup version floats with image updates, so the workflow
downloads a fixed `innosetup-6.x.y.exe`, verifies its SHA-256, installs it silently to a
temp dir, and calls that `ISCC.exe` with `/DAppVersion=<cargo version>`. Uninstall keeps the
user's config/log directories (documented).

## Changes this implies (later PRs, not this one)

- `app/tray` crate (bin `mini-launcher-tray`), Windows-only tray code behind `cfg(windows)`;
  CI keeps building on ubuntu (stub `main` there until the Linux milestone).
- `app/Cargo.toml`: a `[profile.dist]` (`inherits = "release"`, fat LTO, `codegen-units = 1`,
  `strip = true`, `opt-level = "s"`) used by `release.yml` only — CLI 2.10 → 1.45 MiB.
- `autostart` targets `mini-launcher-tray.exe`; the tray settings get the same toggle.
- `installer/mini-launcher.iss` + app icon (#51).
- `ci.yml`: a Windows job builds the installer when `installer/**` changes (ubuntu needs GTK
  headers only once the Linux tray lands, or keep tray code `cfg(windows)` until then).
- `release.yml`: build with `--profile dist`; install pinned Inno Setup (hash-checked); run
  ISCC; publish `mini-launcher-<v>-windows-x86_64-setup.exe` as the primary asset, plus a
  portable zip of both exes; extend `SHA256SUMS`. When SignPath lands: sign both exes before
  ISCC, then the setup exe (Inno `SignTool=` can call it).
- README: SmartScreen note now covers the setup exe; bundled font's license file ships with
  the app.

## Risks / open questions

- Numbers come from a GPU-less CI VM; re-check first-frame time and open-window memory on
  real hardware during M3, and that idle memory drops back after the window closes.
- "No UAC prompt for a standard user" still needs one manual check (runner account is admin).
- Linux tray: `tray-icon` drags in GTK; prefer `ksni` there and keep the tray optional
  (GNOME has none by default).
