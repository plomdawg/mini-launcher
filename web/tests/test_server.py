"""Tests for the Plomcase server's LaunchBox allowlists and HTML parsers (no network)."""

from __future__ import annotations

import json
import sys
import threading
import unittest
from http.server import ThreadingHTTPServer
from pathlib import Path
from urllib.error import HTTPError
from urllib.request import urlopen

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

import server  # noqa: E402

GUID = "92d3999b-e6d3-4aaf-a717-eb848233fece"

# Hand-written fixture modeled on a Games DB images page. URLs are made-up GUIDs, no artwork.
IMAGES_PAGE = """<!doctype html><html><head><title>Example Quest &amp; Co Images - LaunchBox Games Database</title></head>
<body><div class="games-images-hero heading"><h1>Example Quest &amp; Co</h1></div>
<div class="imageContainerGrid image-list"><div class="imageContainerColumn">
  <a href="https://images.launchbox-app.com/00000000-0000-0000-0000-000000000001.jpg" data-gameimageid="1"
     data-toggle="lightbox" data-gallery="gallery" data-title="Example Quest &amp; Co - Box - Front Image (North America)"
     data-footer="780 x 1118 JPEG, 0.21 MB</br><a data-gameimageid='1' class='edit-image' href='javascript:'>Edit Image</a>">
    <div class="img-container">
      <img class="imageCard" src="https://images.launchbox-app.com/00000000-0000-0000-0000-0000000000a1.jpg" width="196" height="281" alt="x" />
      <div class="edit-image"><img src="/Theme/Images/WhiteEditIcon.svg" /></div>
      <div class="imgCardText"><p>Box - Front (North America) - 780 x 1118</p></div>
    </div>
  </a>
  <a href="https://images.launchbox-app.com/00000000-0000-0000-0000-000000000002.png" data-toggle="lightbox"
     data-title="Example Quest &amp; Co - Box - Front Image" data-footer="1531 x 2175 PNG">
    <img class="imageCard" src="https://images.launchbox-app.com/00000000-0000-0000-0000-0000000000a2.png" />
    <div class="imgCardText"><p>Box - Front - 1531 x 2175</p></div>
  </a>
  <a href="https://images.launchbox-app.com/00000000-0000-0000-0000-000000000003.jpg" data-toggle="lightbox"
     data-title="Example Quest &amp; Co - Box - Back Image (Europe)" data-footer="800 x 1098 JPEG">
    <img class="imageCard" src="https://images.launchbox-app.com/00000000-0000-0000-0000-0000000000a3.jpg" />
    <div class="imgCardText"><p>Box - Back (Europe) - 800 x 1098</p></div>
  </a>
  <a href="https://images.launchbox-app.com/00000000-0000-0000-0000-000000000004.png" data-toggle="lightbox"
     data-title="Example Quest &amp; Co - Disc Image (North America)" data-footer="720 x 720 PNG">
    <img class="imageCard" src="https://images.launchbox-app.com/00000000-0000-0000-0000-0000000000a4.png" />
    <div class="imgCardText"><p>Disc (North America) - 720 x 720</p></div>
  </a>
  <a href="https://images.launchbox-app.com/00000000-0000-0000-0000-000000000005.jpg" data-toggle="lightbox"
     data-title="Example Quest &amp; Co - Fanart - Box - Front Image" data-footer="1920 x 1080 JPEG">
    <img class="imageCard" src="https://images.launchbox-app.com/00000000-0000-0000-0000-0000000000a5.jpg" />
    <div class="imgCardText"><p>Fanart - Box - Front - 1920 x 1080</p></div>
  </a>
  <a href="https://images.launchbox-app.com/00000000-0000-0000-0000-000000000006.png" data-toggle="lightbox"
     data-title="Example Quest &amp; Co - Clear Logo Image" data-footer="400 x 200 PNG">
    <img class="imageCard" src="https://images.launchbox-app.com/00000000-0000-0000-0000-0000000000a6.png" />
    <div class="imgCardText"><p>Clear Logo - 400 x 200</p></div>
  </a>
  <a href="https://images.launchbox-app.com/00000000-0000-0000-0000-000000000007.jpg" data-toggle="lightbox"
     data-title="Example Quest &amp; Co - Box - Spine Image (Japan)" data-footer="60 x 1100 JPEG">
    <img class="imageCard" src="https://images.launchbox-app.com/00000000-0000-0000-0000-0000000000a7.jpg" />
  </a>
  <a href="https://evil.example.com/00000000-0000-0000-0000-000000000008.jpg" data-toggle="lightbox">
    <div class="imgCardText"><p>Box - Front (Evil) - 1 x 1</p></div>
  </a>
  <a href="https://images.launchbox-app.com/00000000-0000-0000-0000-000000000001.jpg" data-toggle="lightbox">
    <div class="imgCardText"><p>Box - Front (North America) - 780 x 1118</p></div>
  </a>
  <a href="/games/details/1-example">Details</a>
</div></div></body></html>"""

SEARCH_PAGE = """<html><body><div class="row" id="cardsContainer">
  <div class="games-grid-card"><a class="list-item link-no-underline" href="/games/details/1992-example-quest-co">
    <div class="gameCard"><div class="cardImgPart">
      <img src="https://images.launchbox-app.com/00000000-0000-0000-0000-0000000000b1.jpg" alt="Example Quest &amp; Co" />
      <div class="imgOver"><img src="https://images.launchbox-app.com/r2_00000000-0000-0000-0000-0000000000b2.jpg" /></div>
    </div><div class="cardContent"><div class="cardHeading"><div class="cardTitle">
      <h3>Example Quest &amp; Co</h3><p>Windows</p>
    </div><div class="addToCollectionDropdown hide"><h6>Add to collection</h6></div></div>
    <div class="contentHeading"><div class="releaseDate"><span>Release Date</span><p>2003</p></div></div></div>
  </div></a></div>
  <div class="games-grid-card"><a class="list-item link-no-underline" href="/games/details/7495-example-quest-co">
    <div class="cardTitle"><h3> Example Quest &amp; Co </h3><p>Microsoft Xbox</p></div></a></div>
  <div class="games-grid-card"><a class="list-item link-no-underline" href="/games/details/1992-example-quest-co">
    <div class="cardTitle"><h3>Duplicate</h3><p>Windows</p></div></a></div>
  <a class="list-item" href="https://evil.example.com/games/details/5">x</a>
  <a class="nav-link" href="/games/details/42-not-a-card"><h3>Nav</h3></a>
</div></body></html>"""


class LaunchBoxImageAllowlistTest(unittest.TestCase):
    def test_accepts_guid_images(self):
        for url in (
            f"https://images.launchbox-app.com/{GUID}.jpg",
            f"https://images.launchbox-app.com/{GUID}.png",
            f"https://images.launchbox-app.com/{GUID}.jpeg",
            f"https://IMAGES.launchbox-app.com/{GUID.upper()}.PNG",
            f"https://images.launchbox-app.com:443/{GUID}.jpg",
            f"https://images.launchbox-app.com/r2_{GUID}.jpg",
        ):
            with self.subTest(url=url):
                self.assertTrue(server.allowed_launchbox_image_url(url))

    def test_rejects_everything_else(self):
        for url in (
            "",
            f"http://images.launchbox-app.com/{GUID}.jpg",
            f"https://images.launchbox-app.com.evil.com/{GUID}.jpg",
            f"https://evil.com/{GUID}.jpg",
            f"https://launchbox-app.com/{GUID}.jpg",
            f"https://gamesdb.launchbox-app.com/{GUID}.jpg",
            f"https://user@images.launchbox-app.com/{GUID}.jpg",
            f"https://images.launchbox-app.com:8443/{GUID}.jpg",
            f"https://images.launchbox-app.com/{GUID}.gif",
            f"https://images.launchbox-app.com/{GUID}.svg",
            f"https://images.launchbox-app.com/{GUID}.jpg?x=1",
            f"https://images.launchbox-app.com/{GUID}.jpg#frag",
            f"https://images.launchbox-app.com/sub/{GUID}.jpg",
            f"https://images.launchbox-app.com/{GUID}.jpg/../../etc",
            "https://images.launchbox-app.com/not-a-guid.jpg",
            f"https://images.launchbox-app.com/x{GUID}.jpg",
            f"https://images.launchbox-app.com:notaport/{GUID}.jpg",
            f"//images.launchbox-app.com/{GUID}.jpg",
            f"javascript:alert(1)//images.launchbox-app.com/{GUID}.jpg",
        ):
            with self.subTest(url=url):
                self.assertFalse(server.allowed_launchbox_image_url(url))


class LaunchBoxPageAllowlistTest(unittest.TestCase):
    def test_accepts_game_pages(self):
        for url in (
            "https://gamesdb.launchbox-app.com/games/images/1992",
            "https://gamesdb.launchbox-app.com/games/images/1992-the-example-game",
            "https://gamesdb.launchbox-app.com/games/details/1992-the-example-game",
            "https://gamesdb.launchbox-app.com/games/results/example%20quest?searchText=example+quest",
        ):
            with self.subTest(url=url):
                self.assertTrue(server.allowed_launchbox_page_url(url))

    def test_rejects_other_pages(self):
        for url in (
            "http://gamesdb.launchbox-app.com/games/images/1992",
            "https://gamesdb.launchbox-app.com/",
            "https://gamesdb.launchbox-app.com/#s",
            "https://gamesdb.launchbox-app.com/account/sign-in",
            "https://gamesdb.launchbox-app.com/games/images/abc",
            "https://gamesdb.launchbox-app.com/games/images/1992/extra",
            "https://evil.com/games/images/1992",
            "https://images.launchbox-app.com/games/images/1992",
        ):
            with self.subTest(url=url):
                self.assertFalse(server.allowed_launchbox_page_url(url))


class LaunchBoxParserTest(unittest.TestCase):
    def test_caption(self):
        self.assertEqual(server.parse_launchbox_caption("Box - Front (North America) - 800 x 1126"),
                         {"type": "Box - Front", "region": "North America", "width": 800, "height": 1126})
        self.assertEqual(server.parse_launchbox_caption("Clear Logo - 400 x 200"),
                         {"type": "Clear Logo", "region": "", "width": 400, "height": 200})
        self.assertEqual(server.parse_launchbox_caption("Fanart - Box - Front - 1920 x 1080")["type"], "Fanart - Box - Front")
        self.assertIsNone(server.parse_launchbox_caption("Box - Front"))

    def test_images_page(self):
        result = server.parse_launchbox_images(IMAGES_PAGE)
        self.assertEqual(result["title"], "Example Quest & Co")
        summary = [(image["type"], image["region"], image["width"], image["height"], image["url"][-6:]) for image in result["images"]]
        self.assertEqual(summary, [
            ("Box - Front", "North America", 780, 1118, "01.jpg"),
            ("Box - Front", "", 1531, 2175, "02.png"),
            ("Box - Back", "Europe", 800, 1098, "03.jpg"),
            ("Box - Spine", "Japan", 60, 1100, "07.jpg"),  # no caption: falls back to data-title/footer
            ("Disc", "North America", 720, 720, "04.png"),
            ("Clear Logo", "", 400, 200, "06.png"),
        ])
        self.assertTrue(result["images"][0]["thumb"].endswith("0000000000a1.jpg"))
        for image in result["images"]:
            self.assertTrue(server.allowed_launchbox_image_url(image["url"]))

    def test_images_type_filter_is_configurable(self):
        result = server.parse_launchbox_images(IMAGES_PAGE, types=("Fanart - Box - Front",))
        self.assertEqual([image["type"] for image in result["images"]], ["Fanart - Box - Front"])

    def test_search_page(self):
        results = server.parse_launchbox_search(SEARCH_PAGE)
        self.assertEqual([(r["id"], r["title"], r["platform"]) for r in results], [
            (1992, "Example Quest & Co", "Windows"),
            (7495, "Example Quest & Co", "Microsoft Xbox"),
        ])
        self.assertTrue(results[0]["thumb"].endswith("0000000000b1.jpg"))
        self.assertEqual(results[1]["thumb"], "")

    def test_garbage_input(self):
        self.assertEqual(server.parse_launchbox_images("<a data-toggle='lightbox'><p>unclosed"), {"title": "", "images": []})
        self.assertEqual(server.parse_launchbox_search("<<<>>>"), [])


class QuietHandler(server.PlomcaseHandler):
    def log_message(self, format_string, *args):
        pass


class LaunchBoxEndpointValidationTest(unittest.TestCase):
    """Requests rejected before any outbound fetch, so these never touch the network."""

    @classmethod
    def setUpClass(cls):
        cls.httpd = ThreadingHTTPServer(("127.0.0.1", 0), QuietHandler)
        cls.thread = threading.Thread(target=cls.httpd.serve_forever, daemon=True)
        cls.thread.start()
        cls.base = f"http://127.0.0.1:{cls.httpd.server_address[1]}"

    @classmethod
    def tearDownClass(cls):
        cls.httpd.shutdown()
        cls.httpd.server_close()

    def assert_rejected(self, path):
        with self.assertRaises(HTTPError) as caught:
            urlopen(self.base + path, timeout=5)
        self.assertEqual(caught.exception.code, 400)
        self.assertIn("error", json.loads(caught.exception.read()))
        caught.exception.close()

    def test_rejects_bad_requests(self):
        for path in (
            "/api/launchbox/game",
            "/api/launchbox/game?id=abc",
            "/api/launchbox/game?id=0",
            "/api/launchbox/game?id=12345678901",
            "/api/launchbox/game?id=1992%2F..%2F..",
            "/api/launchbox/search",
            "/api/launchbox/search?q=%20%20",
            "/api/launchbox/search?q=" + "a" * 101,
            "/api/launchbox/image",
            "/api/launchbox/image?url=https%3A%2F%2Fevil.com%2Fx.jpg",
            "/api/launchbox/image?url=http%3A%2F%2Fimages.launchbox-app.com%2F" + GUID + ".jpg",
        ):
            with self.subTest(path=path):
                self.assert_rejected(path)


if __name__ == "__main__":
    unittest.main()
