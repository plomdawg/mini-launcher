# Mini Launcher

Put an NFC tag in a 3D-printed game cartridge or disc, slot it into a 3D-printed console-style
reader, and the game launches on your PC (Steam, any executable, or an emulator with your own
legally dumped games). Windows first; Linux support is planned.

**Status:** early development. See [AGENTS.md](AGENTS.md) for the design and roadmap.
Windows builds are on the [Releases page](https://github.com/plomdawg/mini-launcher/releases);
setup guide: [docs/windows-quickstart.md](docs/windows-quickstart.md).

- `app/` — desktop launcher (Rust; Windows tray app planned, CLI for diagnostics)
- `web/` — box-art generator for printed case inserts and labels
- `hardware/` — printable parts, BOM, wiring
- `docs/` — Windows quickstart, tag format, assembly, compliance

## License

Copyright (C) 2026 plomdawg and Mini Launcher contributors.

The software in this repository (the app, installer and web box-art generator) is free software:
you can redistribute it and/or modify it under the terms of the GNU General Public License as
published by the Free Software Foundation, either version 3 of the License, or (at your option)
any later version. See [LICENSE](LICENSE).

In short: you may use, change, share and sell it, but anything you distribute that is based on it
must stay open source under the same license and keep this copyright notice. Builds before
`app-v0.5.0` were MIT OR Apache-2.0; they are no longer published, and every release from
`app-v0.5.0` on is GPL-3.0-or-later.

Bundled third-party parts keep their own licenses: Departure Mono (SIL OFL 1.1,
`app/assets/fonts/DepartureMono-OFL.txt`) and three.js (MIT, `web/vendor/three/LICENSE`).

The hardware license is still to be decided.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in
this project is licensed under GPL-3.0-or-later, without any additional terms or conditions.
