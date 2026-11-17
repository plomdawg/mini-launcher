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

## License

The software in this repository is licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or
  <http://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or <http://opensource.org/licenses/MIT>)

at your option. The hardware license is still to be decided.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion
in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above,
without any additional terms or conditions.
