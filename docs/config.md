# Config file, backups and synced folders

The library lives in `config.toml` (format: see the quickstart, section 7). `mini-launcher
diagnose` prints the file it uses on its `Config:` line.

## How the app saves it

When the app changes the config (adding a game, binding a tag), it edits the file in place and
keeps your comments, blank lines and key order. Every save:

1. checks the new config with exactly the same rules as loading it, and refuses to write an
   invalid one;
2. writes a temporary file in the same folder, flushes it to disk and renames it over
   `config.toml`, so a crash or power cut leaves either the old or the new file, never half of
   one;
3. keeps the previous version as `config.toml.bak` (one copy, replaced on each save).

To undo the last save, replace `config.toml` with `config.toml.bak`.

## Export and import

An export is a single, human-readable TOML file: your `config.toml` as is, with a header table on
top:

```toml
# Mini Launcher config export. ...
[mini_launcher]
export_version = 1

[[games]]
id = "half-life-2"
name = "Half-Life 2"
steam_appid = 220
```

Importing reads the whole file and validates it first: a damaged file, an unknown
`export_version`, or any entry that would not load is rejected and the current config is left
alone. Only then is it saved as `config.toml` (the old one becomes `config.toml.bak`). A plain
`config.toml` without the header can be imported too. To use an export by hand instead, copy it
to `config.toml` and delete the `[mini_launcher]` table.

Exports contain your exe paths and arguments; importing one makes those programs launchable by
tags, so only import files you made or trust.

## Keeping the config in another folder

To keep the config in a synced folder (e.g. OneDrive), create `location.toml` next to where
`config.toml` would normally be (`%APPDATA%\mini-launcher\config\location.toml` on Windows):

```toml
dir = 'D:\OneDrive\Mini Launcher'
```

Mini Launcher then loads, saves and watches `config.toml` (and `config.toml.bak`) in that folder.
`dir` must be a full path to a folder that already exists. If `location.toml` is invalid, the app
logs a warning and uses the default folder; `diagnose` shows the problem on a
`Config folder override ignored:` line. Restart the tray app after changing `location.toml`.
