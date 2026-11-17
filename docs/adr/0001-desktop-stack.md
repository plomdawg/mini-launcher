# ADR 0001: Desktop stack — tray, settings window, process model, installer

- Status: proposed (needs maintainer sign-off before M3 tray/installer work starts)
- Date: 2026-11-16
- Issue: #41

## Context

M3 turns the CLI into a per-user Windows tray app (status, pause, settings, quit; no console
window; start at login; per-user installer). M4 adds a settings window: game library list,
add-game wizard (Steam detection, tap a tag, write/bind it), launch-option forms,
export/import. Windows ships first; Linux follows later. Constraints: small binary, real
accessibility (screen readers, keyboard), licenses compatible with MIT OR Apache-2.0, no
extra runtime to install, and every tool buildable in GitHub Actions with pinned versions.

Since this was filed, `run` already gained a named-mutex single instance
(`Local\mini-launcher`), a per-user `HKCU\…\Run` value named `Mini Launcher`
(`autostart enable|disable|status`), reconnect, and a rolling log file. The decisions below
build on those.

## Decision

1. **Tray: `tray-icon` + `muda`** (tauri-apps, MIT OR Apache-2.0).
2. **Settings window / wizard: Slint**, software renderer only, used under the Slint
   Royalty-free License 2.0. Runner-up: egui/eframe with the wgpu backend.
3. **Process model: two executables from one workspace.** The existing console-subsystem
   `mini-launcher.exe` stays the CLI; a new GUI-subsystem `mini-launcher-tray.exe` (new crate
   `app/tray`) hosts the tray icon and the settings window on one UI thread.
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

**Screen-reader tree** (UI Automation, enumerated from PowerShell on the runner): both
toolkits expose a real tree through AccessKit — window, `Edit` ×2, `ComboBox`, labelled
`Text`, buttons. The difference is semantics: **Slint exposes the library as `List` +
`ListItem`** (virtualised, 10 visible rows), egui exposes each row as a **`Button`** (26
buttons), so a screen reader hears "button" instead of "list item 3 of 20". egui also has an
open TextEdit gap where UIA `SetValue` is ignored (emilk/egui#8699, fix in review).

**Installer** (Inno Setup 6.7.1 preinstalled on the image, egui-wgpu tray 9.2 MiB + CLI
1.45 MiB as payload): **5.24 MiB setup.exe**, silent per-user install (`/CURRENTUSER`) in 2.2 s to `%LOCALAPPDATA%\Programs\Mini Launcher`; created the Start-menu
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
| **`tray-icon` 0.26 + `muda` 0.21** ✔ | Native `Shell_NotifyIcon` + native Win32 popup menu (Narrator/NVDA read native menus with no work from us). Check items, separators, tooltips, icon swap for status. 0.30 MiB. Maintained by the Tauri org; also what Tauri itself uses. Needs a Win32 message loop on the creating thread — Slint's winit loop provides one. Linux: GTK 3 + appindicator (build-time dep, its own GTK loop thread). |
| `tray-item` | Smaller API, less active; no menu check items; would still need its own loop. |
| raw `windows` crate | Smallest, full control, but we'd own the menu/hidpi/taskbar-restart code; no Linux path. |
| `ksni` (Linux only) | Pure-Rust StatusNotifierItem over D-Bus, no GTK. Worth revisiting for the Linux tray instead of `tray-icon`'s GTK path. |

### Settings window / wizard

| Option | Size (small) | Accessibility | License | Verdict |
|---|---|---|---|---|
| **Slint 1.19** | 9.8 MiB (software renderer + tray), 10.6 MiB default features | AccessKit/UIA; proper list semantics; keyboard focus in std widgets | GPL-3.0 OR Royalty-free 2.0 OR commercial | **Chosen** |
| egui/eframe 0.37 | 5.0 MiB glow (needs OpenGL), 9.1 MiB wgpu | AccessKit/UIA, weaker semantics (rows = buttons), TextEdit SetValue gap | MIT OR Apache-2.0 | Runner-up |
| iced 0.14 | not measured | No upstream accessibility (iced-rs/iced#552 still open; AccessKit only in the COSMIC fork) | MIT | Rejected (a11y) |
| Tauri 2 (WebView2) | not measured; ~1–3 MiB exe reported | Best (browser a11y via UIA) | MIT OR Apache-2.0 | Rejected for now: second stack (HTML/JS + IPC), WebView2 process tree (100+ MB) when open, harder offline/locked-down story |
| Native Win32 (`windows` / native-windows-gui) | smallest | Native | MIT/Apache | Rejected: list + wizard by hand is a lot of code, no Linux path, nwg unmaintained |

Size is a wash rather than a win: the full Slint tray app (9.8 MiB) is about the size of
egui-wgpu + tray (9.2 MiB) and ~4.7 MiB more than egui-glow, which however does not start
without OpenGL. Expect the installer to stay around 5–6 MiB (it compresses ~2:1).

Why Slint over egui: it renders on any machine with the software renderer (egui needs
OpenGL 2+ or wgpu/D3D12), uses ~5 MiB private memory instead of tens to hundreds, starts
faster, has retained-mode widgets (`StandardListView`, `LineEdit`, `ComboBox`, `TabWidget`)
that look closer to native and carry correct a11y roles, and the `.slint` markup keeps the
wizard layout reviewable. What we give up:

- **License.** Slint is not MIT/Apache. We use it under the *Royalty-free Desktop, Mobile,
  and Web Applications License 2.0*: free for desktop apps, no embedded use, no exposing
  Slint's API, and attribution — either the `AboutSlint` widget in an About dialog reachable
  from the menu, or the Slint badge on the download page. We do both (About entry in the tray
  menu; badge in the README). Our own source stays MIT OR Apache-2.0; only the binary carries
  Slint. Forks or distro packagers who dislike the terms can take Slint under GPL-3.0 (our
  code is GPL-compatible). Add the `LicenseRef-Slint-Royalty-free-2.0` id to any future
  license allowlist (cargo-deny). **This is the one point the maintainer must explicitly
  accept;** if not, pick egui + wgpu (same plan otherwise, ~+4 MiB vs egui/glow, weaker
  list semantics).
- A DSL to learn, compiled by the `slint!` macro or a `build.rs` (`slint-build`).
- Linux builds need `libfontconfig1-dev` (CI: one `apt-get` line).

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
login opens a console. The tray process runs everything on the Slint event loop thread; the
reader loop stays on its worker thread and posts events with `slint::invoke_from_event_loop`.
The settings window is created on first open and hidden (not destroyed) on close.

### Installer

| Option | Verdict |
|---|---|
| **Inno Setup 6** | **Chosen.** Per-user without admin (`PrivilegesRequired=lowest`, `{autopf}` → `%LOCALAPPDATA%\Programs`), Start-menu entry, uninstaller + HKCU uninstall entry, optional "Start at login" task writing the same `Mini Launcher` Run value, `AppMutex=Local\mini-launcher` so setup/uninstall ask to close a running app, `CloseApplications` via Restart Manager. Permissive license (free incl. commercial; keep notice). Verified end-to-end on the runner (above). |
| WiX v6/v7 (MSI) | Rejected: per-user MSI is awkward (HKCU key-path rules, no `%LOCALAPPDATA%\Programs` default), `cargo-wix` targets legacy WiX 3, and binary use is under the Open Source Maintenance Fee EULA (fee once revenue ≥ US$10k/yr — kits are sold). |
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
- `installer/mini-launcher.iss` + app icon (#51) + an About dialog with `AboutSlint`.
- `ci.yml`: ubuntu installs `libfontconfig1-dev`; a Windows job builds the installer when
  `installer/**` changes.
- `release.yml`: build with `--profile dist`; install pinned Inno Setup (hash-checked); run
  ISCC; publish `mini-launcher-<v>-windows-x86_64-setup.exe` as the primary asset, plus a
  portable zip of both exes; extend `SHA256SUMS`. When SignPath lands: sign both exes before
  ISCC, then the setup exe (Inno `SignTool=` can call it).
- README: Slint attribution badge; SmartScreen note now covers the setup exe.

## Risks / open questions

- Slint license acceptance (above) — the deciding question.
- egui/Slint numbers come from a GPU-less CI VM; re-check idle memory and first-frame time on
  real hardware during M3, especially with Narrator/NVDA running (AccessKit activates lazily).
- Linux tray: `tray-icon` drags in GTK; prefer `ksni` there and keep the tray optional
  (GNOME has none by default).
