# Tag format

This is the spec for what goes on a tag and how the launcher interprets it. If the launcher's
behavior differs from this document, treat that as a bug.

## Hardware

- **Tag type:** NXP NTAG213 (NFC Forum Type 2). 45 pages of 4 bytes; the user area is pages
  4–39 (`0x04`–`0x27`), **144 bytes**. The capability container in page 3 is `E1 10 12 00`
  (NDEF v1.0, data area 0x12 × 8 = 144 bytes, read/write).
- **Layout:** the NDEF message lives in an NDEF Message TLV (`0x03`, length, message) starting at
  page 4, followed by a terminator TLV (`0xFE`). Phone apps write this layout for you. The
  launcher also accepts NULL TLVs (`0x00`) and Lock/Memory Control TLVs (`0x01`, `0x02`) before
  the NDEF Message TLV, which it skips, and both the 1-byte and 3-byte (`0xFF xx xx`) length
  forms.
- **Record:** one NDEF **Text** record (TNF `0x01`, type `T`) or **URI** record (TNF `0x01`,
  type `U`).
- **Capacity:** every payload defined here is under 80 bytes, so it always fits. After TLV and
  record overhead an NTAG213 holds roughly 130 bytes of text, so there is room to spare.

Keep metal (foil, real CD/DVD) away from the tag: it detunes the antenna and the tag will not
read reliably.

## Reading the record

1. The launcher reads the NDEF message and uses the **first usable Text or URI record**. Records
   of other types are ignored.
2. A record is usable if it decodes to a payload string:
   - **Text record:** the text after the status byte and language code. The text must be UTF-8;
     the language code is ignored. A UTF-16 Text record or one with invalid UTF-8 is
     **skipped**, and the launcher moves on to the next record.
   - **URI record:** the URI identifier prefix (the full NFC Forum table, `0x00`–`0x23`) is
     expanded and joined with the rest of the field, e.g. code `0x00` + `steam://440` →
     `steam://440`. Invalid UTF-8 is skipped the same way.
3. Surrounding ASCII whitespace (spaces, tabs, CR, LF) is trimmed from the payload string.
   Everything else must match the grammar below exactly: prefixes are lowercase, and nothing may
   follow the value, including whitespace inside the string.

An empty tag, a tag with no NDEF message, a message with no usable Text or URI record, or a
payload that fails validation counts as **nothing usable**.

## Payload grammar

| Payload | Meaning |
|---|---|
| `steam://<appid>` | Launch Steam game `<appid>`. Works with zero configuration. |
| `steam:<appid>` | Same as above (short form). |
| `id:<slug>` | Launch the entry `<slug>` from the local library in the user's `config.toml` (executable, args, working directory). |
| anything else | Nothing usable — the tag is identified by its UID only. |

### `<appid>`

- ASCII digits `0`–`9` only — no sign, spaces, separators or other characters.
- Parsed as a decimal unsigned 32-bit integer (`u32`): the value must be between `1` and
  `4294967295`. Zero and values that overflow are rejected.
- Nothing may follow the digits (no `/`, `?`, `;`, query, fragment or embedded whitespace).
  Surrounding whitespace has already been trimmed (see above).

Leading zeros are tolerated because only the parsed number is used, but tag writers should not
emit them.

### `<slug>`

Must match:

```
^[a-z0-9][a-z0-9-]{0,63}$
```

That is 1–64 characters, lowercase ASCII letters, digits and `-`, starting with a letter or
digit.

## Resolution order

When a tag is inserted:

1. **Local UID mapping.** If the tag's UID (7 bytes on an NTAG213) is mapped in the local config,
   that mapping is used. **A local UID mapping always overrides what the tag says.**
2. **`steam://<appid>` / `steam:<appid>`** → launch that Steam app.
3. **`id:<slug>`** → launch the library entry with that slug. If the config has no such entry,
   nothing is launched and the launcher reports the unknown slug.
4. **Nothing usable** and no UID mapping → nothing is launched; the launcher reports the UID so
   it can be mapped.

The UID mapping lets you use blank or read-only tags, and lets you override a tag you can't or
don't want to rewrite.

The launcher reads user memory four pages at a time and stops as soon as the NDEF data is complete
(a whole NDEF Message TLV or a terminator), so smaller Type 2 tags (MIFARE Ultralight, NTAG210)
work as long as the message fits. A tag that isn't Type 2 at all (e.g. MIFARE Classic), or whose
pages keep failing to read for a few seconds (e.g. a read-protected NTAG), counts as having
nothing usable: it's reported once with its UID, so it can be mapped.

If a tag you wrote shows up as unusable (for example because it was slid in slowly or sat at the
edge of the antenna), lift it off the reader for more than five seconds and put it back. A tag
that's only briefly lifted counts as re-seated and isn't launched again.

## Security model

Tag contents are untrusted input: anyone can write anything to a tag and put it in a cart.

- The appid is parsed to a `u32`, then the launcher **builds the URI itself** as
  `steam://rungameid/{n}`. Tag text is never passed to ShellExecute, `xdg-open`, a shell, or any
  other opener.
- Slugs are only used as keys to look up entries in the local config. They are never used to
  build file paths, commands or URLs.
- Executables, arguments and working directories come **only from the user's local config**, and
  are started directly, without a shell. A tag can choose which configured entry to launch; it
  cannot add a new program or change arguments.
- Anything that fails validation is treated as nothing usable — it is never "partly" used.

The worst a malicious tag can do is launch a Steam app by ID or launch an entry the user has
already configured.

## Writing a tag with a phone

Any NDEF-capable writer works. With **NFC Tools** (Android or iOS):

1. Open **Write** → **Add a record**.
2. Choose **Text** (recommended) and enter the payload, e.g. `steam://440` or `id:my-game`.
   **Custom URL / URI** with the same string also works. Do **not** use the plain **URL** record:
   it adds an `https://` prefix, so the tag ends up as `https://steam://440` and is rejected.
3. Tap **Write** and hold the phone over the tag until it confirms.
4. Optional: read the tag back in the **Read** tab to check the record.

To find a Steam appid, open the game's Steam store page: the number in the URL
(`store.steampowered.com/app/<appid>/...`) is the appid.

Don't lock the tag (the "Lock tag" / make read-only option) unless you are sure — locking is
permanent. A locked tag can still be remapped through a local UID mapping.

## Examples

### Valid

| Payload | Result |
|---|---|
| `steam://440` | Steam appid 440 → launches `steam://rungameid/440` |
| `steam:440` | Same as above |
| `steam://4294967295` | Largest allowed appid |
| `id:my-game` | Library entry `my-game` |
| `id:0` | Library entry `0` (a single digit is a valid slug) |
| `id:half-life-2` | Library entry `half-life-2` |
| `steam://440 ` / `steam://440` + newline | Surrounding whitespace is trimmed → appid 440 |

### Rejected (treated as nothing usable → UID lookup)

| Payload | Why |
|---|---|
| `steam://0` | Appid must be non-zero |
| `steam://` | No appid |
| `steam://4294967296` | Overflows `u32` |
| `steam://+440` | Sign not allowed — digits only |
| `steam://-1` | Sign not allowed — digits only |
| `steam://0x1b8` | Hex not allowed — decimal digits only |
| `steam://٤٤٠` | Non-ASCII (Arabic-Indic) digits not allowed |
| `steam://440 && calc` | Internal whitespace and trailing text |
| `steam://440/` | Trailing text |
| `steam://440?x=1` | Trailing text |
| `steam://rungameid/440` | Not `steam://<appid>`; write `steam://440` instead |
| `steam://run/440` | Not `steam://<appid>`; write `steam://440` instead |
| `steam://440;calc.exe` | Trailing text |
| `STEAM://440` | Prefix is case-sensitive (lowercase only) |
| `https://store.steampowered.com/app/440/` | Not a supported payload |
| `https://steam://440` | What the plain NFC Tools URL record writes — not a supported payload |
| `id:` | Empty slug |
| `id:My-Game` | Uppercase not allowed |
| `id:-game` | Must start with a letter or digit |
| `id:my_game` | `_` not allowed |
| `id:../x` | `.` and `/` not allowed |
| `id:../games/x` | `.` and `/` not allowed |
| `id:` + 65 characters | Longer than 64 characters |
| `C:\Games\game.exe` | Executables can't be put on a tag; configure them locally |
