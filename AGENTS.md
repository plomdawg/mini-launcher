# Mini Launcher — Agent Context

> The product name is **Mini Launcher**. In names, copy and listings, avoid "cartridge(s)"
> (collides with GNOME *Cartridges* and bpbwaite/Cartridge) and any console trademark (PS2,
> Switch, GameCube, -station, -cube, amiibo).

Open-source product: NTAG213 NFC stickers inside 3D-printed game carts / CDs / cases. Inserting a
cart into a 3D-printed, console-styled reader shell (PN532 inside) launches the matching game —
usually via Steam, or any exe + args. It ships as an installable Windows tray app (the CLI is kept
for diagnostics); Linux support is planned later. Kits are sold (shop.plom.one).
## Layout

```
app/        Rust workspace — core (reader, NDEF, payload, config, launch, debounce) + cli
web/        Box-art generator ("Plomcase" prototype): vanilla JS + Python stdlib server
hardware/   STL/3MF/STEP, BOM.md, wiring diagrams
docs/       tag-format.md, reader protocol notes, assembly.md, compliance.md
.github/    ci.yml (windows + ubuntu matrix), release.yml (tag → GitHub Releases)
```

## Hardware

- Reader: Teyleten "PN532 V2.0" red board + the **USB-UART adapter it ships with**
  (CH340/CP2102-class). No MCU, no firmware: the host speaks the PN532 **HSU** frame protocol over
  a serial port at 115200. DIP switches must be set to HSU (both OFF).
- Tags: NTAG213 stickers (144 B user memory).
- An RP2040-Zero bridge is a possible later revision (LEDs/buttons/stable USB IDs) — not now.
- Printed carts must not contain metal (foil, real CD/DVD) near the tag — it detunes it.

## Software design rules

- **One Rust codebase** for Windows + Linux; OS-specific code only in thin `cfg` modules
  (launching, autostart, port discovery).
- `Reader` abstraction with `Pn532Hsu` and `Simulated` implementations. Everything above the
  reader must be testable without hardware (`--simulate`).
- Host owns everything: PN532 framing, NTAG page reads, NDEF parsing, insert/remove debounce.
  Removal = several consecutive missed polls (~1 s). Same UID re-seated within a few seconds is
  not a new launch. Close-game-on-remove is opt-in only (killing games loses saves).

### Tag format (see `docs/tag-format.md`)

NDEF Text or URI record containing one of:
- `steam://<appid>` (also accept `steam:<appid>`) — self-contained, works with zero config
- `id:<slug>` — resolved via the local `config.toml` library entry (exe/args/cwd)
- nothing usable → fall back to looking up the tag **UID** in config

A local UID mapping always overrides what the tag says.

### Security (non-negotiable)

Tag contents are untrusted input.
- Parse the Steam appid to `u32` and **rebuild** `steam://rungameid/{n}` — never pass tag text
  to ShellExecute / `xdg-open` / a shell.
- `id:` slugs are map keys only — validate `^[a-z0-9][a-z0-9-]{0,63}$`, never use in paths.
- Executables and args come only from the user's local config, spawned without a shell.

### Linux specifics

- Launch Steam via the `steam://` URI through the desktop opener, never the `steam` binary
  (Flatpak Steam has none on PATH).
- Ship a udev rule (`TAG+="uaccess"`, `ENV{ID_MM_DEVICE_IGNORE}="1"` so ModemManager leaves the
  port alone) and a `systemd --user` unit. Tray icon is optional (GNOME has none by default; Steam
  Deck gaming mode has none at all).

## CI / releases

- Every PR: `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test` on windows-latest +
  ubuntu-latest; CI uploads the built binaries as artifacts.
- The Rust toolchain is pinned in `app/rust-toolchain.toml` (rustup and CI both read it; MSRV =
  `rust-version` in `app/Cargo.toml`). To bump: update both to the new stable version in one PR
  and fix any new fmt/clippy findings there.
- Releases are Windows-only for now: tags `app-v*` → GitHub Release with the `.exe` +
  `SHA256SUMS`. Windows code signing via SignPath (free for OSS) is planned later; until then
  document SmartScreen "More info → Run anyway". Keep ubuntu-latest in the CI matrix.

## Web (box-art generator)

- `web/` is the imported Plomcase prototype (see `web/README.md`). Users supply their own art or
  their own SteamGridDB API key; the server only proxies SteamGridDB and stores nothing.
- The server port is set with `PORT` (default 8784).
- Planned rework: templates per printed part (case wrap, cart label, CD label/insert), split
  `app.js` into modules, tests for layout math.

## Selling guardrails

- Kits sold from day 1 as **unassembled** kits; document radio-compliance status (FCC §15.225 /
  CE RED) in `docs/compliance.md`.
- Never sell items printed with commercial game art or publisher logos; never ship art presets.
- No console names or logos in product names, SKUs, or listings. Sold shells are "inspired-by"
  designs.
- Software is dual-licensed MIT OR Apache-2.0 (`LICENSE-MIT`, `LICENSE-APACHE`). Hardware
  license still TBD (leaning CERN-OHL-W or CC BY-SA 4.0 — no NC licenses).

## Milestones

1. **M1 Windows test build** — done.
2. **M2 Foundation + release pipeline** — rename, license, toolchain pin, release workflow
   (Windows `.exe` + `SHA256SUMS`), M1 hardening/doc leftovers.
3. **M3 Tray app + installer** — background tray app (status, pause, settings, quit), no console
   window, single instance, start at login, log file, reader auto-reconnect, per-user installer
   (no admin, Start menu, uninstaller). CLI stays for `diagnose`/`scan`.
4. **M4 Add a game** — settings window + add-game wizard: Steam library detection, manual exe
   entries, tag binding (write payload with read-back verify, fall back to UID binding when the
   tag can't be written; never lock tags), launch options editable in the GUI, export/import
   backup and optional synced config folder.
5. **M5 Emulators** — generic emulator + ROM launch type (tag only names a configured entry),
   starting with a Switch emulator.
6. **M6 Box-art generator** — rework `web/`, deploy on a `*.plom.one` subdomain.
7. **M7 Shop** — BOM, assembly guide, name/trademark check, listings, code signing.

- Later: Linux parity (udev rule, `systemd --user` unit, Linux release asset).

## Contributing

One issue per change, small PRs, CI must be green, and every PR is reviewed before merge.
