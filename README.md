# nfc-game-launcher

*(working name)*

Put an NFC tag in a 3D-printed game cartridge or disc, slot it into a 3D-printed console-style
reader, and the game launches on your PC (Steam or any executable). Windows and Linux.

**Status:** early development. See [AGENTS.md](AGENTS.md) for the design and roadmap.
Try the Windows test build: [docs/windows-quickstart.md](docs/windows-quickstart.md).

- `app/` — desktop launcher (Rust, Windows + Linux)
- `web/` — box-art generator for printed case inserts and labels
- `hardware/` — printable parts, BOM, wiring
- `docs/` — Windows quickstart, tag format, assembly, compliance
