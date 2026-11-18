#!/usr/bin/env python3
"""Small same-origin web server and SteamGridDB / LaunchBox Games DB proxy for Plomcase."""

from __future__ import annotations

import json
import os
import re
from html.parser import HTMLParser
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.error import HTTPError, URLError
from urllib.parse import parse_qs, quote, unquote, urlencode, urljoin, urlsplit
from urllib.request import HTTPRedirectHandler, Request, build_opener


ROOT = Path(__file__).resolve().parent
API_BASE = "https://www.steamgriddb.com/api/v2"
MAX_IMAGE_BYTES = 40 * 1024 * 1024
# Raster formats the editor can place. SVG and anything else are refused by both image proxies.
ALLOWED_IMAGE_MIMES = frozenset({"image/jpeg", "image/png", "image/webp"})
ALLOWED_API_QUERY = {"dimensions", "styles", "mimes", "types", "nsfw", "humor", "epilepsy", "limit", "page"}


def allowed_image_url(value: str) -> bool:
    try:
        parsed = urlsplit(value)
        if parsed.scheme != "https" or parsed.username or parsed.password or parsed.port not in (None, 443):
            return False
        host = (parsed.hostname or "").lower()
        if host == "steamgriddb.com" or host.endswith(".steamgriddb.com"):
            return True
        if host == "s3.amazonaws.com" and parsed.path.startswith("/steamgriddb/"):
            return True
        if host == "steamgriddb.s3.amazonaws.com" or host.startswith("steamgriddb.s3."):
            return True
        return False
    except ValueError:
        return False


class SafeImageRedirect(HTTPRedirectHandler):
    def redirect_request(self, request, file_pointer, code, message, headers, new_url):
        if not allowed_image_url(new_url):
            return None
        return super().redirect_request(request, file_pointer, code, message, headers, new_url)


# --- LaunchBox Games DB -----------------------------------------------------
# No API key: Plomcase reads the public game images page on demand and only
# relays images from the LaunchBox image host. Nothing is cached or polled.

LAUNCHBOX_HOST = "gamesdb.launchbox-app.com"
LAUNCHBOX_BASE = f"https://{LAUNCHBOX_HOST}"
LAUNCHBOX_IMAGE_HOST = "images.launchbox-app.com"
LAUNCHBOX_USER_AGENT = "Plomcase/0.1"
LAUNCHBOX_MAX_PAGE_BYTES = 4 * 1024 * 1024
LAUNCHBOX_MAX_QUERY = 100
# A search term is percent-encoded into the results path: up to 4 UTF-8 bytes per character, "%XX" each.
LAUNCHBOX_MAX_QUERY_PATH = LAUNCHBOX_MAX_QUERY * 4 * 3
LAUNCHBOX_MAX_RESULTS = 50
# Image types offered in the picker, in display order. Add a type here to offer it.
LAUNCHBOX_IMAGE_TYPES = ("Box - Front", "Box - Back", "Box - Spine", "Disc", "Clear Logo")
_LAUNCHBOX_GUID = r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}"
_LAUNCHBOX_IMAGE_PATH = re.compile(rf"/(?:r2_)?{_LAUNCHBOX_GUID}\.(?:jpg|jpeg|png)", re.IGNORECASE)
_LAUNCHBOX_PAGE_PATH = re.compile(r"/games/(?:images|details)/\d{1,10}(?:-[a-z0-9-]{0,200})?|/games/results/[^/?#]{1,%d}" % LAUNCHBOX_MAX_QUERY_PATH, re.IGNORECASE)
_LAUNCHBOX_DETAILS_HREF = re.compile(r"/games/details/(\d{1,10})(?:-[a-z0-9-]{0,200})?", re.IGNORECASE)
_LAUNCHBOX_CAPTION = re.compile(r"^(?P<type>.+?)(?: \((?P<region>[^()]{1,80})\))? - (?P<width>\d{1,6}) x (?P<height>\d{1,6})$")
_LAUNCHBOX_DIMENSIONS = re.compile(r"(\d{1,6}) x (\d{1,6})")


def _secure_url_on_host(value: str, host: str):
    try:
        parsed = urlsplit(value)
        if parsed.scheme != "https" or parsed.username or parsed.password or parsed.port not in (None, 443):
            return None
        if (parsed.hostname or "").lower() != host or parsed.fragment:
            return None
        return parsed
    except ValueError:
        return None


def allowed_launchbox_image_url(value: str) -> bool:
    """Only https://images.launchbox-app.com/<guid>.(jpg|jpeg|png), no query string."""
    parsed = _secure_url_on_host(value, LAUNCHBOX_IMAGE_HOST)
    return bool(parsed and not parsed.query and _LAUNCHBOX_IMAGE_PATH.fullmatch(parsed.path))


def allowed_launchbox_page_url(value: str) -> bool:
    """Only game images/details pages and search results on the Games DB host."""
    parsed = _secure_url_on_host(value, LAUNCHBOX_HOST)
    return bool(parsed and _LAUNCHBOX_PAGE_PATH.fullmatch(parsed.path))


def parse_launchbox_caption(text: str):
    """Split a tile caption like "Box - Front (North America) - 800 x 1126"."""
    match = _LAUNCHBOX_CAPTION.match(" ".join(text.split()))
    if not match:
        return None
    return {
        "type": match["type"],
        "region": match["region"] or "",
        "width": int(match["width"]),
        "height": int(match["height"]),
    }


class LaunchBoxImagesParser(HTMLParser):
    """Collects lightbox image tiles and the game title from a Games DB images page."""

    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.title = ""
        self.page_title = ""
        self.tiles = []
        self._tile = None
        self._anchor_depth = 0
        self._in_h1 = False
        self._in_title = False

    def handle_starttag(self, tag, attrs):
        attributes = dict(attrs)
        if tag == "a":
            if self._tile is not None:
                self._anchor_depth += 1
            elif attributes.get("data-toggle") == "lightbox":
                self._tile = {"href": attributes.get("href") or "", "title": attributes.get("data-title") or "",
                              "footer": attributes.get("data-footer") or "", "thumb": "", "text": []}
                self._anchor_depth = 1
        elif tag == "img" and self._tile is not None and not self._tile["thumb"]:
            self._tile["thumb"] = attributes.get("src") or ""
        elif tag == "h1" and not self.title:
            self._in_h1 = True
        elif tag == "title":
            self._in_title = True

    def handle_endtag(self, tag):
        if tag == "a" and self._tile is not None:
            self._anchor_depth -= 1
            if self._anchor_depth <= 0:
                self.tiles.append(self._tile)
                self._tile = None
        elif tag == "h1":
            self._in_h1 = False
        elif tag == "title":
            self._in_title = False

    def handle_data(self, data):
        if self._tile is not None:
            self._tile["text"].append(data)
        elif self._in_h1:
            self.title += data
        elif self._in_title:
            self.page_title += data


def _launchbox_tile_info(tile: dict, game_title: str):
    info = parse_launchbox_caption(" ".join("".join(tile["text"]).split()))
    if info:
        return info
    # Fallback: "<game> - <type> Image (<region>)" plus "<w> x <h> JPEG" in the footer.
    label = " ".join(tile["title"].split())
    if game_title and label.startswith(game_title + " - "):
        label = label[len(game_title) + 3:]
    match = re.fullmatch(r"(?P<type>.+?) Image(?: \((?P<region>[^()]{1,80})\))?", label)
    dims = _LAUNCHBOX_DIMENSIONS.search(tile["footer"])
    if not match:
        return None
    return {"type": match["type"], "region": match["region"] or "",
            "width": int(dims[1]) if dims else 0, "height": int(dims[2]) if dims else 0}


def parse_launchbox_images(html: str, types=LAUNCHBOX_IMAGE_TYPES) -> dict:
    parser = LaunchBoxImagesParser()
    parser.feed(html)
    parser.close()
    title = " ".join(parser.title.split())
    if not title:
        title = " ".join(parser.page_title.split()).removesuffix(" - LaunchBox Games Database").removesuffix(" Images")
    wanted = {name.lower(): index for index, name in enumerate(types)}
    seen = set()
    images = []
    for tile in parser.tiles:
        url = tile["href"]
        if url in seen or not allowed_launchbox_image_url(url):
            continue
        info = _launchbox_tile_info(tile, title)
        if not info or info["type"].lower() not in wanted:
            continue
        seen.add(url)
        thumb = tile["thumb"] if allowed_launchbox_image_url(tile["thumb"]) else url
        images.append({"url": url, "thumb": thumb, **info, "type": types[wanted[info["type"].lower()]]})
    images.sort(key=lambda image: wanted[image["type"].lower()])
    return {"title": title, "images": images}


class LaunchBoxSearchParser(HTMLParser):
    """Collects game cards (id, title, platform) from a Games DB search results page."""

    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.results = []
        self._card = None
        self._anchor_depth = 0
        self._field = None

    def handle_starttag(self, tag, attrs):
        attributes = dict(attrs)
        if tag == "a":
            if self._card is not None:
                self._anchor_depth += 1
                return
            match = _LAUNCHBOX_DETAILS_HREF.fullmatch(attributes.get("href") or "")
            if match and "list-item" in (attributes.get("class") or "").split():
                self._card = {"id": int(match[1]), "title": "", "platform": "", "thumb": ""}
                self._anchor_depth = 1
        elif self._card is not None:
            if tag == "img" and not self._card["thumb"]:
                src = attributes.get("src") or ""
                self._card["thumb"] = src if allowed_launchbox_image_url(src) else ""
            elif tag == "h3" and not self._card["title"]:
                self._field = "title"
            elif tag == "p" and self._card["title"] and not self._card["platform"]:
                self._field = "platform"

    def handle_endtag(self, tag):
        if tag in ("h3", "p"):
            self._field = None
        elif tag == "a" and self._card is not None:
            self._anchor_depth -= 1
            if self._anchor_depth <= 0:
                card = self._card
                self._card = None
                card["title"] = " ".join(card["title"].split())
                card["platform"] = " ".join(card["platform"].split())
                if card["title"]:
                    self.results.append(card)

    def handle_data(self, data):
        if self._card is not None and self._field:
            self._card[self._field] += data


def parse_launchbox_search(html: str, limit: int = LAUNCHBOX_MAX_RESULTS) -> list:
    parser = LaunchBoxSearchParser()
    parser.feed(html)
    parser.close()
    seen = set()
    results = []
    for card in parser.results:
        if card["id"] in seen:
            continue
        seen.add(card["id"])
        results.append(card)
        if len(results) >= limit:
            break
    return results


class LaunchBoxError(Exception):
    def __init__(self, status: int, message: str):
        super().__init__(message)
        self.status = status
        self.message = message


class LaunchBoxPageRedirect(HTTPRedirectHandler):
    def redirect_request(self, request, file_pointer, code, message, headers, new_url):
        if not allowed_launchbox_page_url(urljoin(request.full_url, new_url)):
            return None
        return super().redirect_request(request, file_pointer, code, message, headers, new_url)


class LaunchBoxImageRedirect(HTTPRedirectHandler):
    def redirect_request(self, request, file_pointer, code, message, headers, new_url):
        if not allowed_launchbox_image_url(urljoin(request.full_url, new_url)):
            return None
        return super().redirect_request(request, file_pointer, code, message, headers, new_url)


def fetch_launchbox_page(url: str) -> str:
    if not allowed_launchbox_page_url(url):
        raise LaunchBoxError(400, "Only LaunchBox Games DB game pages can be read.")
    request = Request(url, headers={"Accept": "text/html", "User-Agent": LAUNCHBOX_USER_AGENT})
    try:
        with build_opener(LaunchBoxPageRedirect()).open(request, timeout=15) as response:
            if not allowed_launchbox_page_url(response.geturl()):
                raise LaunchBoxError(502, "LaunchBox redirected somewhere unexpected.")
            if response.headers.get_content_type() != "text/html":
                raise LaunchBoxError(502, "LaunchBox returned an unexpected page type.")
            body = response.read(LAUNCHBOX_MAX_PAGE_BYTES + 1)
            if len(body) > LAUNCHBOX_MAX_PAGE_BYTES:
                raise LaunchBoxError(502, "LaunchBox returned an unexpectedly large page.")
            charset = response.headers.get_content_charset() or "utf-8"
            try:
                return body.decode(charset, errors="replace")
            except LookupError:  # unknown charset label
                return body.decode("utf-8", errors="replace")
    except HTTPError as error:
        error.close()
        if error.code == 404:
            raise LaunchBoxError(404, "LaunchBox has no game with that ID.") from None
        if 300 <= error.code < 400:
            raise LaunchBoxError(502, "LaunchBox redirected somewhere unexpected.") from None
        raise LaunchBoxError(502, f"LaunchBox returned HTTP {error.code}.") from None
    except (URLError, TimeoutError, OSError):
        raise LaunchBoxError(502, "Plomcase could not connect to LaunchBox.") from None


class PlomcaseHandler(SimpleHTTPRequestHandler):
    server_version = "Plomcase/0.1"

    def __init__(self, *args, **kwargs):
        super().__init__(*args, directory=str(ROOT), **kwargs)

    def end_headers(self):
        self.send_header("X-Content-Type-Options", "nosniff")
        if getattr(self, "_static", False):
            # Revalidate app files on every load so a deploy never mixes new HTML with cached old JS.
            self.send_header("Cache-Control", "no-cache")
        super().end_headers()

    def do_GET(self):
        path = urlsplit(self.path).path
        if path == "/healthz":
            self._send_json(200, {"ok": True})
        elif path == "/api/steamgriddb":
            self._proxy_api()
        elif path == "/api/steamgriddb/image":
            self._proxy_image()
        elif path == "/api/launchbox/game":
            self._launchbox_game()
        elif path == "/api/launchbox/search":
            self._launchbox_search()
        elif path == "/api/launchbox/image":
            self._launchbox_image()
        else:
            self._static = True
            super().do_GET()

    def _send_json(self, status: int, payload: dict):
        body = json.dumps(payload, separators=(",", ":")).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _query(self) -> dict[str, list[str]]:
        return parse_qs(urlsplit(self.path).query, keep_blank_values=True)

    def _proxy_api(self):
        auth = self.headers.get("Authorization", "")
        if not re.fullmatch(r"Bearer\s+[^\s]{8,512}", auth, flags=re.IGNORECASE):
            self._send_json(401, {"error": "Paste a valid SteamGridDB API key and try again."})
            return

        raw_path = self._query().get("path", [""])[0]
        if not raw_path or len(raw_path) > 512:
            self._send_json(400, {"error": "Missing or invalid SteamGridDB endpoint."})
            return
        parsed = urlsplit(raw_path)
        if parsed.scheme or parsed.netloc or parsed.fragment:
            self._send_json(400, {"error": "Only SteamGridDB search, grid, and logo requests are allowed."})
            return

        segments = parsed.path.strip("/").split("/")
        api_path = ""
        if len(segments) == 3 and segments[:2] == ["search", "autocomplete"]:
            term = unquote(segments[2])
            if not term or len(term) > 128:
                self._send_json(400, {"error": "Enter a game title up to 128 characters long."})
                return
            api_path = "search/autocomplete/" + quote(term, safe="")
        elif len(segments) == 3 and segments[0] in {"grids", "logos"} and segments[1] == "game" and segments[2].isdigit():
            api_path = "/".join(segments)
        else:
            self._send_json(400, {"error": "Only SteamGridDB game search, grids, and logos are allowed."})
            return

        query_items = []
        for key, values in parse_qs(parsed.query, keep_blank_values=True).items():
            if key not in ALLOWED_API_QUERY or len(values) != 1:
                self._send_json(400, {"error": "Unsupported SteamGridDB filter."})
                return
            value = values[0]
            if key == "limit" and (not value.isdigit() or not 1 <= int(value) <= 50):
                self._send_json(400, {"error": "SteamGridDB page size must be between 1 and 50."})
                return
            if key == "page" and (not value.isdigit() or int(value) > 10000):
                self._send_json(400, {"error": "Invalid result page."})
                return
            query_items.append((key, value))

        url = f"{API_BASE}/{api_path}"
        if query_items:
            url += "?" + urlencode(query_items)
        request = Request(url, headers={"Authorization": auth, "Accept": "application/json", "User-Agent": "Plomcase/0.1"})
        try:
            with build_opener().open(request, timeout=15) as response:
                body = response.read(5 * 1024 * 1024 + 1)
                if len(body) > 5 * 1024 * 1024:
                    self._send_json(502, {"error": "SteamGridDB returned an unexpectedly large response."})
                    return
                self.send_response(response.status)
                self.send_header("Content-Type", response.headers.get("Content-Type", "application/json"))
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)
        except HTTPError as error:
            try:
                error_body = error.read(1024 * 1024)
            finally:
                error.close()
            try:
                message = json.loads(error_body).get("error") or f"SteamGridDB returned HTTP {error.code}."
            except (ValueError, AttributeError):
                message = f"SteamGridDB returned HTTP {error.code}."
            self._send_json(error.code, {"error": message})
        except (URLError, TimeoutError, OSError):
            self._send_json(502, {"error": "Plomcase could not connect to SteamGridDB."})

    def _proxy_image(self):
        image_url = self._query().get("url", [""])[0]
        if not image_url or len(image_url) > 2048 or not allowed_image_url(image_url):
            self._send_json(400, {"error": "Only secure SteamGridDB artwork URLs can be downloaded."})
            return
        request = Request(image_url, headers={"Accept": "image/*", "User-Agent": "Plomcase/0.1"})
        try:
            with build_opener(SafeImageRedirect()).open(request, timeout=20) as response:
                mime = response.headers.get_content_type()
                if mime not in ALLOWED_IMAGE_MIMES:
                    self._send_json(415, {"error": "SteamGridDB returned an unsupported image type (JPEG, PNG or WebP only)."})
                    return
                body = response.read(MAX_IMAGE_BYTES + 1)
                if len(body) > MAX_IMAGE_BYTES:
                    self._send_json(413, {"error": "Artwork is larger than the 40 MB import limit."})
                    return
                self.send_response(response.status)
                self.send_header("Content-Type", mime)
                self.send_header("Content-Length", str(len(body)))
                self.send_header("Cache-Control", "private, max-age=300")
                self.end_headers()
                self.wfile.write(body)
        except HTTPError as error:
            self._send_json(error.code, {"error": f"Artwork host returned HTTP {error.code}."})
        except (URLError, TimeoutError, OSError):
            self._send_json(502, {"error": "Plomcase could not download this artwork."})

    def _launchbox_game(self):
        game_id = self._query().get("id", [""])[0]
        if not re.fullmatch(r"[1-9][0-9]{0,9}", game_id):
            self._send_json(400, {"error": "Enter a numeric LaunchBox game ID or a gamesdb.launchbox-app.com game URL."})
            return
        try:
            html = fetch_launchbox_page(f"{LAUNCHBOX_BASE}/games/images/{game_id}")
        except LaunchBoxError as error:
            self._send_json(error.status, {"error": error.message})
            return
        self._send_json(200, {"id": int(game_id), **parse_launchbox_images(html)})

    def _launchbox_search(self):
        term = " ".join(self._query().get("q", [""])[0].split())
        if not term:
            self._send_json(400, {"error": "Enter a game title to search."})
            return
        if len(term) > LAUNCHBOX_MAX_QUERY:
            self._send_json(400, {"error": f"Game title is too long (max {LAUNCHBOX_MAX_QUERY} characters)."})
            return
        url = f"{LAUNCHBOX_BASE}/games/results/{quote(term, safe='')}?{urlencode({'searchText': term})}"
        try:
            html = fetch_launchbox_page(url)
        except LaunchBoxError as error:
            self._send_json(error.status, {"error": error.message})
            return
        self._send_json(200, {"results": parse_launchbox_search(html)})

    def _launchbox_image(self):
        image_url = self._query().get("url", [""])[0]
        if not image_url or len(image_url) > 2048 or not allowed_launchbox_image_url(image_url):
            self._send_json(400, {"error": "Only LaunchBox artwork URLs can be downloaded."})
            return
        request = Request(image_url, headers={"Accept": "image/*", "User-Agent": LAUNCHBOX_USER_AGENT})
        try:
            with build_opener(LaunchBoxImageRedirect()).open(request, timeout=20) as response:
                if not allowed_launchbox_image_url(response.geturl()):
                    self._send_json(502, {"error": "LaunchBox redirected somewhere unexpected."})
                    return
                mime = response.headers.get_content_type()
                if mime not in ALLOWED_IMAGE_MIMES:
                    self._send_json(415, {"error": "LaunchBox returned an unsupported image type (JPEG, PNG or WebP only)."})
                    return
                body = response.read(MAX_IMAGE_BYTES + 1)
                if len(body) > MAX_IMAGE_BYTES:
                    self._send_json(413, {"error": "Artwork is larger than the 40 MB import limit."})
                    return
                self.send_response(response.status)
                self.send_header("Content-Type", mime)
                self.send_header("Content-Length", str(len(body)))
                self.send_header("Cache-Control", "private, max-age=300")
                self.end_headers()
                self.wfile.write(body)
        except HTTPError as error:
            error.close()
            self._send_json(502 if 300 <= error.code < 400 else error.code, {"error": f"LaunchBox returned HTTP {error.code}."})
        except (URLError, TimeoutError, OSError):
            self._send_json(502, {"error": "Plomcase could not download this artwork."})

    def log_message(self, format_string, *args):
        # Request paths contain no API key; avoid logging headers or forwarded secrets.
        super().log_message(format_string, *args)


if __name__ == "__main__":
    host = os.environ.get("PLOMCASE_HOST", "127.0.0.1")
    port = int(os.environ.get("PORT", "8784"))
    server = ThreadingHTTPServer((host, port), PlomcaseHandler)
    server.daemon_threads = True
    print(f"Plomcase listening at http://{host}:{port}")
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()
