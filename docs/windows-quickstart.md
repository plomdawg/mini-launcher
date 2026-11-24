# Windows quickstart

From a box of parts to "insert a cart, the game starts" on Windows. No programming needed, but
you'll type a few commands into PowerShell.

You need:

- the PN532 reader board and the USB-UART adapter that comes with it, plus 4 jumper wires
- NTAG213 NFC stickers (tags)
- an Android or iPhone with the free **NFC Tools** app (to write tags)
- Steam, if you want to launch Steam games
- for emulator games: the emulator, plus your own legally dumped games and any keys or firmware
  the emulator needs (see [Emulator games](#emulator-games))

## 1. Install the launcher

Download it from the repository's
[**Releases** page](https://github.com/plomdawg/mini-launcher/releases) on GitHub. Open the
newest release and, under **Assets**, download
`mini-launcher-<version>-windows-x86_64-setup.exe` (for example
`mini-launcher-0.6.0-windows-x86_64-setup.exe`) and `SHA256SUMS`.

Optional but recommended: check that the download isn't damaged or tampered with. In
PowerShell, in the folder where you downloaded both files, run:

```powershell
Get-FileHash .\mini-launcher-0.6.0-windows-x86_64-setup.exe -Algorithm SHA256
Get-Content .\SHA256SUMS
```

The `Hash` from the first command must match the long hex string next to the same file name
in `SHA256SUMS` (upper/lower case doesn't matter). If they differ, delete the file and
download it again.

Run the setup. It installs for your user only and never asks for admin rights:

- Files go to `%LOCALAPPDATA%\Programs\Mini Launcher` (`mini-launcher-tray.exe`, the tray
  app, and `mini-launcher.exe`, the command-line tool used below).
- It adds **Mini Launcher** to the Start menu.
- **Start Mini Launcher when I sign in** (ticked by default) starts the tray app at login.
  You can change this later with **Start at login** in the tray menu, or on the
  **Settings** screen of the settings window.
- **Launch Mini Launcher** on the last page starts the tray app right away.

The setup isn't code-signed yet, so Windows SmartScreen may say "Windows protected your PC":
click **More info**, then **Run anyway**. If Windows still blocks it, right-click the file →
**Properties** → tick **Unblock** → **OK**, or run `Unblock-File` on it in PowerShell.

The tray app runs in the background without a console window: use its notification-area icon
to open **Settings…**, pause, open the config or logs, turn **Start at login** on or off, or quit. It works like
`mini-launcher run`, and only one of the two can use the reader at a time. Pausing switches the
reader's antenna off until you resume; a cart left in the reader does not launch on resume.
The settings window has the same **Pause** / **Resume** button in its library toolbar.

The settings window's **Settings** button (top right of the library) opens the app preferences:

- **Start Mini Launcher when I sign in**: the same switch as the tray's **Start at login**.
- **When a cart is already in the reader at startup**: **Wait until it's re-inserted** (the
  default) records the cart without launching it, so signing in with a cart left in the reader
  doesn't start a game; take it out and put it back to play. **Launch it** starts it right away.
  Either way, a cart that stays in while the reader is unplugged and plugged back in is not
  launched again.
- **Text size**: 90 % to 150 % for the settings window (it grows to fit).
- **Sounds**: short retro sound effects. **Play sounds** turns them all on or off, **Volume**
  sets their level (**-** / **+** or click the meter; **Test** plays the "saved" jingle), and one
  box per event turns that sound off: **Waiting for tag**, **Writing**, **Success**, **Failure**
  (while writing a tag in **Add game** or **Write tag...**) and **Game launch** (when a cart
  starts a game).
- **Library file**: **Import...**, **Export...** and **Folder...** (see `docs/config.md`).

These are saved in `[settings]` in `config.toml`, except start at login (see `docs/config.md`).

When you insert a cart that has no game yet, the tray shows a **New cart — click to add a game**
notification. Click it to open **Settings** on **Add game**: pick the game, and if the cart is
still in the reader it is written (or bound) right away.

The library's **LAUNCH** column shows what starts each game (`steam://440`, `Program: game.exe`,
`Eden: game.nsp`, ...), and its **TAG** column shows how the cart is recognised: `UID 04:A1:...`
when it's bound by its serial number, what the tag was written with (`steam://<appid>` or
`id:<name>`), or *no tag yet* (also shown for carts written by an older version, until they're
written again with **Edit → Write tag...**). In a game's **Edit** form, **Launch** starts the game with
what's in the form, without saving it, so you can try out launch options first.

**Updating:** run the newer setup; it replaces the installed version and keeps your settings.
If the tray app is running, setup closes it and starts the new version when it's done. If
**Settings** is open, setup leaves it alone so nothing you haven't saved is lost: the window comes
to the front, and setup continues once you close it (if you take longer, setup asks you to quit
Mini Launcher and click **OK**). A `mini-launcher run` in a console still has to be stopped by hand.
`mini-launcher-tray.exe --quit` asks a running tray app to quit the same way (for scripts).
The **Start Mini Launcher when I sign in** box follows your current setting: it's ticked if
start at login is on and unticked if you turned it off (for example in the tray), so updating
never turns it back on by itself. Unticking it turns start at login off. For silent installs,
`/MERGETASKS=autostart` or `/MERGETASKS=!autostart` overrides this.

**Uninstalling:** Windows **Settings → Apps → Installed apps → Mini Launcher → Uninstall**
(it closes a running tray app the same way, unless Settings is open). This removes the program, the Start menu entry and the start-at-login
entry. Your config (`%APPDATA%\mini-launcher`) and logs (`%LOCALAPPDATA%\mini-launcher`) are
kept; delete those folders by hand if you want them gone.

**Portable alternative:** the release also has `mini-launcher-<version>-windows-x86_64.zip`
with the same two programs and no installer. Unzip it somewhere permanent (for example
`C:\Tools\mini-launcher\`); nothing is added to the Start menu or the uninstall list. SmartScreen
applies the same way. Testers who want an unreleased build can download the
`mini-launcher-windows-x86_64` artifact from any green **CI** run in the repository's
**Actions** tab (signed-in GitHub users only); it holds the same two programs.

`mini-launcher.exe` is a **console program**: double-clicking it just flashes a window. Run it
from PowerShell or Windows Terminal instead:

1. Open the folder with `mini-launcher.exe` in File Explorer. For an installed copy, paste
   `%LOCALAPPDATA%\Programs\Mini Launcher` into File Explorer's address bar.
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
   the number in the game's Steam store URL: `store.steampowered.com/app/440/...`, or run
   `.\mini-launcher.exe steam list` to see the app IDs of the games you have installed.
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

### Start at login

If you installed with the setup, the tray app already handles this (the setup task or the
tray's **Start at login**, or the settings window's **Settings** screen). The commands below are for the console `run` instead; they use the
same single start-at-login entry, so enabling one replaces the other.

To start `run` automatically when you sign in (per user, no admin rights needed):

```powershell
.\mini-launcher.exe autostart enable            # or: autostart enable --port COM3
.\mini-launcher.exe autostart status
.\mini-launcher.exe autostart disable
```

This registers the `.exe` at its current location, so move it to where it will stay first (and
run `enable` again if you move it). Only one `run` can use the reader at a time; a second one
exits with `mini-launcher is already running`.

## 7. The config file

The config file lets you launch games that aren't on Steam, and use tags you can't or don't want
to rewrite. With the tray app you rarely need to edit it by hand: **Settings…** in the tray
menu lists your games and adds, edits and removes them (including unbinding a tag's UID), and
saves keep your comments. The tray picks up the change right away. On Windows it lives at:

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

- Each game is a `[[games]]` block with an `id` and a `name`, plus **exactly one** of
  `steam_appid`, `steam_shortcut` (see [Non-Steam games added to Steam](#non-steam-games-added-to-steam)),
  `battlenet` (see [Battle.net games](#battlenet-games)), `exe`, or `emulator` + `rom` (see
  [Emulator games](#emulator-games)).
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
- `args` and `cwd` are only allowed with `exe`. Unknown keys (e.g. from a newer version, or a
  typo) are ignored with a warning in the log; see [config.md](config.md#newer-and-older-versions).

### Non-Steam games added to Steam

Games you added to Steam yourself (**Games → Add a Non-Steam Game to My Library...**) show up in
**Add game** too, marked `Steam shortcut` in the **Store** column. Pick one and write the tag as
usual; the tag gets `id:<id>`, and the config entry holds the shortcut's app ID:

```toml
[[games]]
id = "doom"
name = "Doom"
steam_shortcut = 3237998081
```

The game is started **through Steam**, the same way Steam starts it, so the Steam overlay, your
Steam Input controller layout and (on Linux) Proton all still apply. Steam must be running or
able to start, and the shortcut must still exist in Steam: if you delete it and add it again,
or change its name or target in Steam, it can get a new ID, so add the game again (or fix the
number with **Edit → Launch with: Steam shortcut**).

Mini Launcher reads the shortcuts of every Steam account that has signed in on this PC
(`<Steam folder>\userdata\<account>\config\shortcuts.vdf`) and skips hidden ones.
`mini-launcher steam list` prints them with `(shortcut)` after the name.

### Battle.net games

**Add game** lists installed Battle.net games next to your Steam games, marked
`Battle.net <code>` in the **Store** column. Pick one and write the tag as usual; the tag gets
`id:<id>`, and the config entry holds the game's Battle.net *launch code*:

```toml
[[games]]
id = "world-of-warcraft-forever"
name = "World of Warcraft: Forever"
battlenet = "WoWF"
```

A tag for it runs `Battle.net.exe --exec="launch WoWF"`. If Battle.net is already running, the
game starts right away. If it isn't, Battle.net drops that request while it starts up, so Mini
Launcher opens Battle.net, waits (up to 90 seconds) for its window, gives it a few more seconds
to log in, and sends the request again. If you have to type a password, the game won't start
on its own; insert the cart again once you're logged in. The main codes Mini Launcher detects:

| Game | Code | | Game | Code |
|---|---|---|---|---|
| World of Warcraft | `WoW` | | Hearthstone | `WTCG` |
| World of Warcraft Classic | `WoWC` | | Heroes of the Storm | `Hero` |
| World of Warcraft: Forever | `WoWF` | | StarCraft II | `S2` |
| Overwatch | `Pro` | | StarCraft: Remastered | `S1` |
| Diablo IV | `Fen` | | Warcraft III: Reforged | `W3` |
| Diablo III | `D3` | | Diablo II: Resurrected | `OSI` |
| Diablo Immortal | `ANBS` | | Diablo | `D1` |

The game list and launch codes are in `app/core/src/battlenet.rs`. Detection reads Battle.net's
own install list (`C:\ProgramData\Battle.net\Agent\product.db`), Blizzard's uninstall entries and
the usual install folders. WoW versions are found by their subfolders (`_retail_`, `_classic_`,
`_forever_`); a version counts only when its subfolder has the game in it. Click a column header in
**Add game** to sort by name or store. `mini-launcher battlenet list` shows what it found, and
`mini-launcher battlenet list -v` also shows everything it looked at. A game it misses can still be added by hand: **Edit → Launch with: Battle.net**, then
type its code. The code is letters, digits and `_` only (case matters: `WoWF`, not `wowf`).

WoW Classic, Classic Era and Season of Discovery all use `WoWC`: Battle.net starts whichever
Classic version was last selected in its **Game Version** menu, so Mini Launcher only lists
Classic itself.

### Emulator games

Mini Launcher can start games in an emulator: the tag picks the game, and the emulator and the
game file come from your config. You need your own legally dumped games and, where the emulator
needs them, your own keys and firmware. Mini Launcher doesn't include, download or help you find
any of these.

The walkthrough below uses **Eden**, a Switch emulator. **PCSX2** (PS2) is built in too — see
[PCSX2](#pcsx2) — and any other emulator that takes the game file on its command line works the
same way. You can add the emulator and its games in the settings
window (step 3) or in `config.toml` by hand.

**1. Set up the emulator on its own.** Install Eden, give it your keys and firmware, and check
that your game starts from inside Eden. If it doesn't start there, it won't start from a tag
either.

**2. Find `eden.exe`.** The usual places are:

- `%LOCALAPPDATA%\Programs\eden\eden.exe`
- `%LOCALAPPDATA%\eden\eden.exe`

`%LOCALAPPDATA%` is usually `C:\Users\<you>\AppData\Local`; paste the path into File Explorer's
address bar to check it. If you unzipped Eden somewhere else, use that path. In the config, write
the full path (`%LOCALAPPDATA%` isn't expanded there). Point `exe` at the emulator's real `.exe`,
not a `.bat` or `.cmd` that starts it: those run through `cmd.exe`, which reads the arguments
again and can break or refuse game paths with characters like `&`, `%` or `^`.

**3. Add the emulator and a game.** In the settings window: **Add game → Emulator game...**. If
Eden isn't set up yet, click **Add Eden...** (each built-in emulator you haven't added has its
own **Add ...** button, here and on the settings window's Emulators screen): the form fills in Eden's arguments and finds
`eden.exe` in the usual places above (otherwise **Browse** for it); **Save** brings you back to
the wizard. Then choose the game file with **Browse**, or click **Scan folder...** to list the
Eden game files in a folder (and in the folders up to three levels inside it) and click one. See
[Updates and DLC](#updates-and-dlc) for what the scan leaves out. **Scan
folder...** is greyed out when the app doesn't recognise the emulator as Eden (its name isn't
"Eden" and its exe isn't `eden.exe`); use **Browse** then. **Next** shows a name and tag ID made
from the file name; change them if you like. If the name has nothing an ID can be made from (for
example only Japanese characters), type an ID yourself. **Next** again takes you to the same tag
step as for any other game (step 4 is done for you). If the file is already in your library on
that emulator, the first **Next** skips the name and goes straight to the tag step for that
game. The **Emulators** button on the library screen lists, edits and removes emulators, and a
game's **Edit** form can change its game file or move it onto another emulator.

By hand: describe each emulator once in an
`[[emulators]]` block, then point games at it with `emulator` (its `id`) and `rom` (the full path
to the game file):

```toml
[[emulators]]
id = "eden"
name = "Eden"
exe = 'C:\Users\you\AppData\Local\Programs\eden\eden.exe'
args = ["-f", "-g", "{rom}"]
# cwd = 'C:\Users\you\AppData\Local\Programs\eden'   (optional)

[[games]]
id = "kart-racer"
name = "Kart Racer"
emulator = "eden"
rom = 'D:\Games\Switch\kart-racer.nsp'

[[games]]
id = "my-puzzle-game"
name = "My Puzzle Game"
emulator = "eden"
rom = 'D:\Games\Switch\My Puzzle Game [v0].xci'
```

For Eden, `-f` starts in fullscreen and `-g` is followed by the game to run. Eden's game files
usually end in `.nsp`, `.xci`, `.nca`, `.nro` or `.nso`. The config doesn't check that the
`rom` file exists: if the path is wrong, Eden opens and reports that it can't load the game.

**4. Write the tag** (if you added the game by hand). Write `id:` followed by the game's `id`,
for example `id:kart-racer`, with NFC Tools ([step 4](#4-write-a-tag)) or with
`.\mini-launcher.exe write id:kart-racer` (quit the tray app first). Or leave the tag as it is
and map its UID with `uid` on the game, as for any other game.

**5. Try it.** The tray app reloads `config.toml` when you save it (restart `run` if you use
that). `run --dry-run` shows the exact command without starting Eden:

```
launch 04A1B2C3D4E5F6: emulator eden: run "C:\\Users\\you\\AppData\\Local\\Programs\\eden\\eden.exe" with args ["-f", "-g", "D:\\Games\\Switch\\kart-racer.nsp"] (tag id:kart-racer) [dry run]
```

(The backslashes are doubled only in this printout.)

Rules for emulators and emulator games:

- An emulator has an `id` (same rules as a game's), a `name`, the full path to its `exe`, its
  `args`, and optionally a `cwd`. Every emulator `id` must be different; games and emulators
  have separate ids, so `eden` can be both.
- `args` must contain `"{rom}"` exactly once, as an item of its own. It is replaced by the
  game's `rom` path as one argument, spaces included. `{rom}` inside a longer item (e.g.
  `"--rom={rom}"`) is an error; nothing else is substituted.
- `{rom}` must be written exactly like that: lowercase, no spaces. `{ROM}` or `{ rom }` aren't
  recognised; they count as plain text, so `args = ["-g", "{ROM}"]` is rejected with an error
  ending in `args must contain "{rom}" exactly once`.
- `args` and `cwd` belong to the emulator; a game with `emulator` can't set them, and `rom` is
  only allowed together with `emulator`.
- The tag still only says `id:<slug>` (or is mapped by `uid`); nothing on the tag reaches the
  command line.

#### PCSX2

Install PCSX2 2.x, set up your BIOS in it and check that your game boots there first. **Add
PCSX2...** finds `pcsx2-qt.exe` in `C:\Program Files\PCSX2\` (the installer's default) or a
Scoop install (`%USERPROFILE%\scoop\apps\pcsx2\current\`); for the portable build, **Browse**
to wherever you unzipped it. The arguments it fills in are:

```toml
args = ["-nogui", "-fullscreen", "--", "{rom}"]
```

`-nogui` hides PCSX2's game list and quits PCSX2 when you close the game, `-fullscreen` starts in
fullscreen, and `--` ends the options so the game path is never read as one. **Scan folder...**
lists `.iso`, `.chd`, `.cso`, `.zso`, `.gz`, `.cue` and `.elf` files; a CD game ripped as `.bin` +
`.cue` is listed by its `.cue` (pick a lone `.bin` with **Browse**). PS2 games have no separate
update files, so a PCSX2 scan leaves nothing out.

#### Updates and DLC

A tag launches a base game; updates and DLC aren't games of their own, so **Scan folder...**
leaves them out of the list and says how many it skipped. A file counts as an update or DLC when:

- its name has an `[UPD]`, `[Update]` or `[DLC]` tag (any case);
- its name has a title ID in brackets (16 hex characters starting with `01`) that isn't a base
  game's (base game IDs end in `000`, updates in `800`, and anything else is DLC), e.g.
  `[0100152000022800]`;
- it is anywhere under a folder named `Update`, `Updates`, `DLC` or `Add-On` (any case) inside
  the scanned folder, e.g. `Update\v1.0.1\Game.nsp`.

So a library laid out like this lists one game, *Super Mario Party*, and skips one update:

```
Games\
  Super Mario Party\
    Title\Super Mario Party.nsp
    Update\Super Mario Party v1.0.1 [UPD].nsp
```

Install updates and DLC in Eden once (**File > Install Files to NAND**); after that they load
with the base game whenever it starts, including from a tag.

Backups, export/import and keeping the config in a synced folder are covered in
[config.md](config.md).

Restart `run` after you edit the config. If the file has a mistake, `run` stops with
`error: cannot load ...config.toml: ...` and the message names the problem, e.g.
`games[1]: "my-game": set one of steam_appid, steam_shortcut, battlenet, exe or emulator`. `games[1]` is the second block (counting
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
| `mini-launcher is already running` | The tray app or another `run` (maybe the one started at login) has the reader. Quit it (tray icon → **Quit**), or use `autostart disable`. |
| `lost the reader on COM3` | The adapter was unplugged or dropped off USB. Plug it back in: `run` keeps going and prints `reader reconnected on COM3` (`scan` exits; start it again). |
| Tag isn't detected | Center it on the reader's antenna. Keep metal (foil, real CDs/DVDs) away from it. Check with `scan`. |
| Tag is detected but nothing launches | Run `.\mini-launcher.exe run -v` (or `-vv` for more detail) and read the line printed for the tag. `unknown tag` means map it in the config or rewrite it. For an `exe` game, check the path. |
| `launch ...: failed to start: ...` | The `exe` path or `cwd` is wrong, or the file has moved. |
| An emulator game opens the emulator but not the game | The `rom` path is wrong or the file has moved, or the emulator is missing keys or firmware. Check that the game starts from inside the emulator, and compare the path in `run --dry-run` with the real file. |
| `emulator "..." is not defined in [[emulators]]` | The game's `emulator` doesn't match any emulator's `id` (ids are lowercase). |
| A Steam game doesn't start | Make sure Steam is installed and signed in. Try opening `steam://rungameid/440` (with your app ID) from the Run dialog (Win+R) to check. |
| `error: cannot load ...config.toml` | The config has a mistake; the error names the block and the problem. |
| Need to see what happened earlier | `run` also writes a log file per day to `%LOCALAPPDATA%\mini-launcher\logs\` (the last 7 are kept); `diagnose` prints the folder. |
