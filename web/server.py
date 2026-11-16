#!/usr/bin/env python3
"""Small same-origin web server and SteamGridDB proxy for Plomcase."""

from __future__ import annotations

import json
import os
import re
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.error import HTTPError, URLError
from urllib.parse import parse_qs, quote, unquote, urlencode, urlsplit
from urllib.request import HTTPRedirectHandler, Request, build_opener


ROOT = Path(__file__).resolve().parent
API_BASE = "https://www.steamgriddb.com/api/v2"
MAX_IMAGE_BYTES = 40 * 1024 * 1024
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


class PlomcaseHandler(SimpleHTTPRequestHandler):
    server_version = "Plomcase/0.1"

    def __init__(self, *args, **kwargs):
        super().__init__(*args, directory=str(ROOT), **kwargs)

    def end_headers(self):
        self.send_header("X-Content-Type-Options", "nosniff")
        super().end_headers()

    def do_GET(self):
        path = urlsplit(self.path).path
        if path == "/healthz":
            self._send_json(200, {"ok": True})
        elif path == "/api/steamgriddb":
            self._proxy_api()
        elif path == "/api/steamgriddb/image":
            self._proxy_image()
        else:
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
                if not mime.startswith("image/"):
                    self._send_json(415, {"error": "SteamGridDB returned a non-image asset."})
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
