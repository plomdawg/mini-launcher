# Plomcase

A lightweight web app for designing and exporting wraparound game-case art. It has no package dependencies; the included Python standard-library server serves the app and proxies SteamGridDB and LaunchBox Games DB requests to avoid browser CORS restrictions. Projects and artwork are stored in the browser's IndexedDB. Use **Export JSON** to move an editable project to another browser; **Download SVG** and **Export print PNG** create artwork files.

Run locally with Python 3.10 or newer:

```sh
python server.py
```

Then open <http://127.0.0.1:8784>. Opening `index.html` directly still allows local artwork editing, but the SteamGridDB and LaunchBox pickers need the server.

## Prototype features

- Back, spine, and front cover in one spread, with front and back faces kept at a 2:3 ratio.
- Drag and drop or browse for separate cover art, including 600 × 900 SteamGridDB images. The project name is printed vertically on the spine, with an optional rotated logo near its lower end.
- Optional spine art upload for using a scan of the original spine: it replaces the spine fill, title, and logo. Choose stretch (default), fill, or fit; landscape images are rotated to match the spine direction.
- Optional disc art for a round disc label: upload square art laid out like a full 120 mm CD, choose fill or fit, and set the printed diameter (31.75 mm / 1.25" default) and center hole (4 mm default). The art is scaled to the printed diameter and clipped to the disc; the center hole and the area outside the circle stay transparent. A small disc preview sits under the wrap preview, with **Download disc SVG** and **Export disc PNG** (selected DPI, bleed ring, and cut circles when cut guides are on).
- LaunchBox Games DB picker for region box fronts, backs, spines, discs, and clear logos (no key needed).
- Embedded SteamGridDB picker for cover grids and logos. Search is manual; grid results can be limited to 600 × 900 and are paged in batches of up to 50.
- Editable case face height, spine width (13 mm default), title size (8 mm default), bleed (0 mm default), cut guides (off by default), typeface, spine direction, and spine/text colors. Spine fills can use sampled back/front cover-edge colors in either gradient direction, dark/light blends, edge solids, a mixed color, or a custom solid; an adaptive title keyline helps keep text readable. Title size auto-fits to the spine width and length. Rubik Bold and Impact are available.
- Shared settings defaults are stored in this browser. Each project follows those defaults until you edit a setting; edited settings become project overrides. Use **Save as default** or **Use default** within a section to manage that section independently. Project values and override markers are included in exported project JSON; another browser will apply its own shared defaults to inherited settings.
- SVG or DPI-selected PNG export. The PNG includes bleed, selected-DPI pixel dimensions, and physical-resolution metadata for the entered millimeter dimensions.
- Local project library plus portable `.plomcase.json` project files.

## Notes

This is an early layout prototype. Confirm the case's measured print dimensions and test a paper proof before printing finished inserts. The app does not send images to a service. Rubik is fetched from Google Fonts when online; Impact uses a locally installed font.

The case art provided separately by the user is deliberately not bundled into this source folder, so the public source can stay free of third-party cover artwork.

## SteamGridDB picker

The picker uses SteamGridDB's v2 API. Each user supplies their own API key. Plomcase relays requests through its same-origin server to avoid CORS; the server does not store or log the key. If **Remember on this device** is selected, the key is stored in this browser's local storage; clear it by unchecking that option and searching, or by clearing this site's browser storage. The API documentation caps each result page at 50 items; it does not publish a numeric request quota. The picker only searches when asked and does not poll.

The proxy only forwards SteamGridDB game-search, grid, logo, and approved artwork-download requests. For remote deployment, use HTTPS and put the service behind your normal authentication proxy; the API key passes through the Plomcase server to SteamGridDB. An API key can be generated from the [SteamGridDB API preferences page](https://www.steamgriddb.com/profile/preferences/api).

## LaunchBox Games DB picker

**Choose art from LaunchBox** opens a picker backed by the community [LaunchBox Games Database](https://gamesdb.launchbox-app.com/). No account or API key is needed. Search by title (one result per platform), or paste a game link such as `https://gamesdb.launchbox-app.com/games/details/1992-…` or just the numeric game ID. Images are grouped by type (Box - Front, Box - Back, Box - Spine, Disc, Clear Logo) with their region and pixel size; pick an **Apply to** target or leave it on *Match image type*.

Requests happen only when you search, open a game, or apply an image; nothing is cached, polled, or stored on the server. The server reads only the public game images/details pages and search results on `gamesdb.launchbox-app.com`, and only downloads `https://images.launchbox-app.com/<id>.jpg|png` files (40 MB cap). Preview thumbnails load directly from the LaunchBox image host in your browser. The parser reads the site's HTML, so a LaunchBox redesign can break it; run `python -m unittest discover -s web/tests` after changes. To offer more image types, add them to `LAUNCHBOX_IMAGE_TYPES` in `server.py`.

Artwork on LaunchBox is contributed by its community and belongs to its respective owners; use it for personal prints only.
