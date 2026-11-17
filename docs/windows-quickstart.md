# Windows quickstart

From a box of parts to "insert a cart, the game starts" on Windows. No programming needed, but
you'll type a few commands into PowerShell.

You need:

- the PN532 reader board and the USB-UART adapter that comes with it, plus 4 jumper wires
- NTAG213 NFC stickers (tags)
- an Android or iPhone with the free **NFC Tools** app (to write tags)
- Steam, if you want to launch Steam games

## 1. Download the launcher

Download the launcher from the repository's
[**Releases** page](https://github.com/plomdawg/mini-launcher/releases) on GitHub (the first
release is `app-v0.1.0`):

1. Open the newest release and, under **Assets**, download
   `mini-launcher-<version>-windows-x86_64.exe` (for example
   `mini-launcher-0.1.0-windows-x86_64.exe`) and `SHA256SUMS`.
2. Move the `.exe` somewhere permanent, for example `C:\Tools\mini-launcher\`, and rename it to
   `mini-launcher.exe` so the commands in this guide work as written.
3. Optional but recommended: check that the download isn't damaged or tampered with. In
   PowerShell, in the folder where you downloaded both files (before renaming the `.exe`), run:

   ```powershell
   Get-FileHash .\mini-launcher-0.1.0-windows-x86_64.exe -Algorithm SHA256
   Get-Content .\SHA256SUMS
   ```

   The `Hash` from the first command must match the long hex string next to the same file name
   in `SHA256SUMS` (upper/lower case doesn't matter). If they differ, delete the file and
   download it again.

Testers who want an unreleased build can also download the `mini-launcher-windows-x86_64`
artifact from any green **CI** run in the repository's **Actions** tab (signed-in GitHub users
only). It's a zip with `mini-launcher.exe` inside.

The `.exe` isn't code-signed yet. If you start it from File Explorer, Windows SmartScreen may
say "Windows protected your PC": click **More info**, then **Run anyway**. Starting it from
PowerShell (below) usually skips that prompt. If Windows still blocks it, right-click the
`.exe` → **Properties** → tick **Unblock** → **OK**, or run `Unblock-File .\mini-launcher.exe`
in PowerShell.

`mini-launcher.exe` is a **console program**: double-clicking it just flashes a window. Run it
from PowerShell or Windows Terminal instead:

1. Open the folder with `mini-launcher.exe` in File Explorer.
2. Right-click an empty spot in the folder and choose **Open in Terminal** (on older Windows:
   hold Shift, right-click, **Open PowerShell window here**).
3. Check that it works:

   ```powershell
   .\mini-launcher.exe --help
   ```

In PowerShell you have to type `.\` in front of the program name. The rest of this guide
writes the commands as `.\mini-launcher.exe ...`.

## 2. Set up the reader

1. **DIP switches.** The PN532 board has two small switches. Set **both to OFF**. That selects
   HSU (high-speed UART) mode, which is how the launcher talks to it. If the switches are in
   another position, the reader won't answer.
2. **Wiring.** Connect the board to the USB-UART adapter with 4 wires:

   | USB-UART adapter | PN532 board |
   |---|---|
   | TX | RX |
   | RX | TX |
   | **5V** | VCC |
   | GND | GND |

   TX and RX are **crossed**: the adapter's TX goes to the board's RX, and the adapter's RX goes to
   the board's TX.

   Power the board from the adapter's **5V** pin, **not 3V3**. The board has its own 3.3 V
   regulator, so 5V is safe. The adapter's 3V3 pin can't supply enough current for the reader,
   which then doesn't answer. If the adapter has a jumper for the logic level (3.3V / 5V), set it
   to **3.3V**. If it has no jumper, that's fine too.
3. **Plug the adapter into a USB port.** Windows normally installs the driver by itself.
4. **Find the COM port.** Open **Device Manager** (right-click the Start button) and expand
   **Ports (COM & LPT)**. The adapter shows up as **USB-SERIAL CH340 (COMx)**, for example
   `USB-SERIAL CH340 (COM3)`. If it isn't there, or has a yellow warning sign, install the CH340
   driver from the adapter maker (WCH) and plug it in again.

The launcher finds a CH340 adapter on its own when only one is plugged in. With any other
adapter (for example a CP2102), or with more than one CH340, add `--port` with the COM number
to every command, e.g. `--port COM3`.

## 3. Check the reader: `diagnose`

```powershell
.\mini-launcher.exe diagnose
```

When everything is right, the output looks like this (port names, numbers and descriptions will
differ):

```
Serial ports:
  COM3  USB 1A86:7523  wch.cn / USB-SERIAL CH340  <- likely reader (CH340)

Config: C:\Users\you\AppData\Roaming\mini-launcher\config\config.toml (not created yet; steam:// tags work without it)

Reader on COM3:
  PN532 (IC 0x32), firmware 1.6
  supports: ISO/IEC 14443 Type A, ISO/IEC 14443 Type B, ISO 18092
OK: the reader is answering.
```

Note the **`Config:`** line: that's where your config file goes (step 7).

If it fails, it prints a message starting with `error:`, often followed by a hint:

| Error says | What to do |
|---|---|
| `no USB serial ports found` | The adapter isn't detected. Plug it in again, try another USB port, check Device Manager, install the CH340 driver. |
| `no CH340 reader adapter found among these ports` | Your adapter isn't a CH340. Pick its port from the list and run `diagnose --port COM3` (with your number). |
| `more than one CH340 adapter found` | Unplug the other one, or pick one with `--port`. |
| `COM port not found or in use by another program` | Wrong port number, or another program has the port open. Close serial monitors, the Arduino IDE, and any other `mini-launcher` window. |
| `no reply from the PN532` or `cannot configure the PN532 on COM3` | The adapter works but the board doesn't answer properly. Check that both DIP switches are OFF, that TX/RX are crossed, that the board's VCC is on the adapter's **5V** pin (not 3V3), and that GND is connected. |

`diagnose --all` also lists non-USB ports (like a motherboard `COM1`), which are hidden by
default.

## 4. Write a tag

Write one tag with your phone using **NFC Tools**:

1. Open **Write** → **Add a record** → **Text**.
2. Enter `steam://` followed by the game's Steam app ID, for example `steam://440`. The app ID is
   the number in the game's Steam store URL: `store.steampowered.com/app/440/...`.
3. Tap **Write** and hold the phone on the tag until it confirms.

Use the **Text** record. Don't use the plain **URL** record: it adds `https://` in front, so the
tag ends up as `https://steam://440`, which the launcher rejects.

A tag can also hold `id:<name>` to launch a game from your config file, or nothing at all (then
the launcher goes by the tag's serial number, its UID). The full rules are in
[tag-format.md](tag-format.md).

## 5. Read the tag: `scan`

```powershell
.\mini-launcher.exe scan
```

It prints `Scanning on COM3. Place a tag on the reader (Ctrl-C to stop).` Put the tag on the
reader. You'll see its UID, the raw contents, the NDEF record, and how the launcher reads it,
ending with a line like:

```
  payload: Steam app 440 (steam://rungameid/440)
```

If it says `payload: unusable`, the tag has nothing the launcher understands. Rewrite it (step 4)
or map its UID in the config (step 7). Press **Ctrl-C** to stop.

If the reader stopped before the last page, `scan` prints a line like
`(pages after 7 not read: nothing past here can change the result)`. That's normal: the launcher
stops reading as soon as the rest of the tag can't change what it does.

## 6. Launch games: `run`

Try it first with `--dry-run`, which shows what would launch without starting anything:

```powershell
.\mini-launcher.exe run --dry-run
```

Insert the tag:

```
config: C:\Users\you\AppData\Roaming\mini-launcher\config\config.toml (0 games)
watching COM3 for tags (Ctrl-C to stop)
launch 04A1B2C3D4E5F6: open steam://rungameid/440 (tag steam:) [dry run]
```

If that looks right, run it for real:

```powershell
.\mini-launcher.exe run
```

Keep the window open while you play; the launcher only works while `run` is running. Press
**Ctrl-C** to stop it.

- Insert a cart → the game launches.
- Pull it out and put the same one back within a few seconds → no second launch (`reseated`).
- Removing a cart does **not** close the game.

`steam://` tags work with no config file at all. For anything else, read on.

## 7. The config file

The config file lets you launch games that aren't on Steam, and use tags you can't or don't want
to rewrite. On Windows it lives at:

```
%APPDATA%\mini-launcher\config\config.toml
```

which is usually `C:\Users\<you>\AppData\Roaming\mini-launcher\config\config.toml`. Note the extra
`config` folder. `diagnose` prints the exact path on its `Config:` line. The file isn't created
for you: make the folders and create the file with Notepad (in the Save dialog, choose **All
files** so Notepad doesn't add `.txt`). `run --config <PATH>` loads that file instead of the
default one, which is handy for trying out changes.

Example with one Steam game, one game started from an `.exe`, and one tag mapped by its UID:

```toml
# Launched by a tag that says id:half-life-2
[[games]]
id = "half-life-2"
name = "Half-Life 2"
steam_appid = 220

# A non-Steam game, launched by a tag that says id:my-game
[[games]]
id = "my-game"
name = "My Game"
exe = 'C:\Games\MyGame\game.exe'
args = ["--fullscreen", "--skip-intro"]
cwd = 'C:\Games\MyGame'

# A blank tag: its UID always launches this game, whatever the tag says
[[games]]
id = "portal"
name = "Portal"
uid = "04C1D2E3F40A1B"
steam_appid = 400
```

Rules:

- Each game is a `[[games]]` block with an `id` and a `name`, plus **either** `steam_appid` **or**
  `exe`, not both.
- `id`: lowercase letters, digits and `-`, starting with a letter or digit (up to 64 characters).
  Every `id` must be different.
- `exe`: use the **full path**. Put Windows paths in single quotes (`'C:\...'`) so the
  backslashes are kept as they are. In double quotes you'd have to write every `\` as `\\`.
- `args` (optional): a list, one item per argument. Nothing is passed through a shell, so
  quoting and `&&` tricks don't apply. Each item is passed exactly as written.
- `cwd` (optional): the folder the game starts in. Some games need this to find their files.
  Usually it's the folder that contains the `.exe`.
- `.bat` and `.cmd` files work as `exe` too. Windows runs them through `cmd.exe`.
- `uid` (optional): a tag UID in hex, with or without `:` between the bytes. A UID mapping
  **always wins** over what the tag says.
- `args` and `cwd` are only allowed with `exe`. Unknown keys are an error, so typos show up right
  away.

Restart `run` after you edit the config. If the file has a mistake, `run` stops with
`error: cannot load ...config.toml: ...` and the message names the problem, e.g.
`games[1]: "my-game": set one of steam_appid or exe`. `games[1]` is the second block (counting
starts at 0).

### Mapping an unknown tag

When `run` sees a tag it can't use, it doesn't launch anything. Instead it prints a block you can
paste into the config:

```
unknown tag 04112233445566: nothing usable on it and its UID is not mapped; not launching. To map it, add to C:\Users\you\AppData\Roaming\mini-launcher\config\config.toml:
    [[games]]
    id = "my-game"
    name = "My Game"
    uid = "04112233445566"
    steam_appid = 440   # or: exe = 'C:\Games\MyGame\game.exe'
```

Copy it into the config, change `id`, `name` and `steam_appid` (or replace `steam_appid` with an
`exe` line), save, and restart `run`. A tag that says `id:something` that isn't in your config
gets a similar message with that `id` filled in.

## Troubleshooting

| Problem | What to do |
|---|---|
| Double-clicking the `.exe` just flashes a window | It's a console program. Run it from PowerShell (step 1). |
| "Windows protected your PC" | **More info** → **Run anyway**. |
| `no reply from the PN532` or `cannot configure the PN532` | DIP switches both OFF, TX/RX crossed, board VCC on the adapter's **5V** pin (not 3V3), GND connected. |
| `COM port not found or in use by another program` | Check the COM number in Device Manager. Close any other program using the port, including a second `mini-launcher` window. |
| `no CH340 reader adapter found` | Your adapter is a different chip. Pass `--port COM3` (your number). |
| `lost the reader on COM3` | The adapter was unplugged or dropped off USB. Plug it back in and start again. |
| Tag isn't detected | Center it on the reader's antenna. Keep metal (foil, real CDs/DVDs) away from it. Check with `scan`. |
| Tag is detected but nothing launches | Run `.\mini-launcher.exe run -v` (or `-vv` for more detail) and read the line printed for the tag. `unknown tag` means map it in the config or rewrite it. For an `exe` game, check the path. |
| `launch ...: failed to start: ...` | The `exe` path or `cwd` is wrong, or the file has moved. |
| A Steam game doesn't start | Make sure Steam is installed and signed in. Try opening `steam://rungameid/440` (with your app ID) from the Run dialog (Win+R) to check. |
| `error: cannot load ...config.toml` | The config has a mistake; the error names the block and the problem. |
