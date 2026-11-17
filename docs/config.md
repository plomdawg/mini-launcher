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

In the settings window, **Export...** saves one (suggested name
`mini-launcher-backup-<date>.toml`) and **Import...** reads one, shows how many games and tag
bindings it has and asks before replacing your library. The current config is first copied to
`config.toml.import-<date>-<time>.bak` (UTC), which, unlike `config.toml.bak`, later saves don't
replace.

Importing reads the whole file and validates it first: a damaged file, an unknown
`export_version`, or any entry that would not load is rejected and the current config is left
alone. Only then is it saved as `config.toml` (the old one becomes `config.toml.bak`). A plain
`config.toml` without the header can be imported too. To use an export by hand instead, copy it
to `config.toml` and delete the `[mini_launcher]` table.

Exports contain your exe paths and arguments; importing one makes those programs launchable by
tags, so only import files you made or trust.

## Keeping the config in another folder

The easiest way is the settings window: **Folder...** shows where the config lives now (default
folder, a custom folder, or why the custom folder is unavailable). **Move to folder...** picks a
folder and writes `location.toml` for you; if that folder already has a `config.toml` you choose
between using the one there and copying yours over it (the one there is then kept as
`config.toml.bak`), otherwise your library is copied there. **Reset to default** removes
`location.toml` the same way. The tray switches to the new folder at once.

To set it up by hand instead, create `location.toml` next to where
`config.toml` would normally be (`%APPDATA%\mini-launcher\config\location.toml` on Windows):

```toml
dir = 'D:\OneDrive\Mini Launcher'
```

Mini Launcher then loads, saves and watches `config.toml` (and `config.toml.bak`) in that folder.
`dir` must be a full path to a folder that already exists. Restart the tray app after editing
`location.toml` by hand.

If `location.toml` can't be used (it is invalid, or the folder is missing, e.g. a drive or
synced folder that isn't mounted yet at login), Mini Launcher does **not** switch to the default
folder, since that would be a different library and saves there would be lost when the folder
comes back. Instead:

- the tray shows a "Config folder unavailable" notification and the reason in its tooltip;
- `steam://` tags still launch, but `id:` tags and UID mappings don't;
- every save and import fails with an error instead of writing anywhere;
- the tray checks again every 30 seconds and when `location.toml` changes, and loads the config
  as soon as the folder is there. `mini-launcher run` prints a warning and needs a restart.

`mini-launcher diagnose` explains the problem on its `Config:` line. To go back to the default
folder, delete `location.toml`.

A network share (`\\server\share\...`) works, but while it is unreachable each check can hang
until Windows gives up on it (often 20 seconds or more). That delays startup and, while the tray
is following an available share, tag reads; the 30-second rechecks of an unavailable folder run
in the background. A local synced folder such as OneDrive doesn't have this problem.

On Linux and macOS, saving keeps the file's permissions, and a symlinked `config.toml` stays a
symlink (the file it points to is replaced, and its backup is `<target>.bak` next to that file).
Imports larger than 1 MiB are rejected.

On Linux, an unmounted mount point is still an (empty) folder, so it counts as available; point
`location.toml` at a subfolder of the mounted drive instead, which only exists while it is
mounted.
