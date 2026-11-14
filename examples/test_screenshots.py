"""The docs' screenshot script (examples/screenshots.py): its pages build from the committed
examples, and, where Chrome is found, it shoots every screenshot at the docs' sizes.

Run: python3 -m unittest discover -s examples (cargo test runs it too, tests/tools.rs). The
shooting test skips without Chrome, and when the web fonts cannot load (no network); it writes into
a temporary folder, never docs/images/.
"""
import os
import struct
import tempfile
import unittest
from pathlib import Path
from unittest import mock

import build_site
import screenshots

# Each screenshot's width: the docs' content column with 30px each side, or the whole window.
WIDTHS = {"rankmap-map": 1108, "rankmap-table": 1108, "example-rankmap-ca": 1280,
          "example-rankmap-me": 1280, "example-rankmap-or": 1280}


def chrome():
    """The Chrome the script would use, or None (a $CHROME that is not a program included)."""
    try:
        return screenshots.find_chrome()
    except SystemExit:
        return None


def png_size(data):
    """(width, height) from a PNG's header."""
    assert data[:8] == b"\x89PNG\r\n\x1a\n" and data[12:16] == b"IHDR", "not a PNG"
    return struct.unpack(">II", data[16:24])


class Pages(unittest.TestCase):
    def test_every_screenshot_is_of_an_example_page_that_builds(self):
        self.assertEqual(set(screenshots.SHOTS), set(WIDTHS))
        keys = {key for key, _, _ in build_site.RANKMAPS}
        with tempfile.TemporaryDirectory() as tmp:
            pages = screenshots.build(Path(tmp))
            self.assertEqual(set(pages), keys)
            for name, (key, fragment, _) in screenshots.SHOTS.items():
                page = pages[key]
                self.assertTrue(page.is_file(), page)
                if fragment:
                    # A fragment the page's script opens on: #[areas-]<season>-<grades>, of a season
                    # and grade group the page has.
                    self.assertRegex(fragment, r"^#(areas-)?\d{4}-[\w-]+$", name)
                    season, grades = fragment[1:].removeprefix("areas-").split("-", 1)
                    self.assertIn(f'data-season="{season}" data-view="{grades}"', page.read_text(), name)

    def test_the_screenshots_are_the_ones_the_docs_and_the_site_show(self):
        for key, _, _ in build_site.RANKMAPS:
            self.assertIn(build_site.SHOT.format(key=key)[:-4], screenshots.SHOTS)
        for name in screenshots.SHOTS:
            self.assertTrue((screenshots.IMAGES / f"{name}.png").is_file(), name)

    def test_a_chrome_that_is_not_there_is_named(self):
        with mock.patch.dict(os.environ, {"CHROME": "no-such-chrome-here"}):
            with self.assertRaises(SystemExit) as e:
                screenshots.find_chrome()
        self.assertIn("CHROME=no-such-chrome-here", str(e.exception))


@unittest.skipUnless(chrome(), "no Chrome found (CHROME, or the usual macOS and Linux places)")
class Shoot(unittest.TestCase):
    def test_every_screenshot_is_shot_at_its_size(self):
        with tempfile.TemporaryDirectory() as tmp:
            pages = screenshots.build(Path(tmp))
            browser = screenshots.Chrome(chrome())
            try:
                for name, (key, fragment, crop) in screenshots.SHOTS.items():
                    try:
                        browser.open(pages[key].as_uri() + fragment)
                    except RuntimeError as e:
                        if "web fonts" in str(e):
                            self.skipTest(str(e))
                        raise
                    width, height = png_size(browser.shoot(crop))
                    self.assertEqual(width, WIDTHS[name] * screenshots.SCALE, name)
                    self.assertTrue(800 <= height <= 1300, f"{name}: {height}px high")
            finally:
                browser.close()


if __name__ == "__main__":
    unittest.main()
