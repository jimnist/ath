"""The published examples (ADR 0010): build the site from the committed data and check it.

Run: python3 -m unittest discover -s examples (cargo test runs it too, tests/tools.rs). It builds
the site into a temporary folder, never examples/site/; with ATH_EXAMPLES_SITE set to a built
site (the Pages workflow's), it checks that one instead of building.
"""
import io
import os
import re
import tempfile
import unittest
from contextlib import redirect_stdout
from html.parser import HTMLParser
from pathlib import Path
from urllib.parse import urlsplit

import build_site
import report  # on sys.path through build_site

# The only outside places an example may link to: the project's own pages and the fonts. The SVG
# namespace the map script writes is an identifier, never fetched.
ALLOWED_HOSTS = {"github.com", "jimnist.github.io", "fonts.googleapis.com", "fonts.gstatic.com"}
NAMESPACE_HOSTS = {"www.w3.org"}
SCHEME = re.compile(r"([a-z][a-z0-9+.-]*):", re.I)
# A web address in any text: http(s)://host or a protocol-relative //host.
WEB = re.compile(r"""(?:\bhttps?:)?//[a-z0-9.-]+[^\s"'<>)\\]*""", re.I)


def kind(target):
    """What an href or src is: "page" (a path in the site, or an in-page #anchor), "web" (http,
    https or //host: outside the site) or "bad" (any other scheme: mailto:, javascript:, data:...)."""
    target = target.strip()
    if target.startswith("//"):
        return "web"
    m = SCHEME.match(target)
    if not m:
        return "page"
    return "web" if m.group(1).lower() in ("http", "https") else "bad"


def host(url):
    return urlsplit(url if not url.startswith("//") else "https:" + url).hostname


class Links(HTMLParser):
    """Every href and src of a page."""

    def __init__(self):
        super().__init__()
        self.targets = []

    def handle_starttag(self, tag, attrs):
        self.targets += [v for k, v in attrs if k in ("href", "src") and v is not None]


def targets(path):
    parser = Links()
    parser.feed(path.read_text())
    return parser.targets


class Kinds(unittest.TestCase):
    def test_links_are_told_apart(self):
        for target, want in (("demo.html", "page"), ("#areas-2099-9-11", "page"), ("images/x.png", "page"),
                             ("https://github.com/jimnist/ath", "web"), ("//www.athletic.net/x", "web"),
                             ("HTTP://x.org", "web"), ("mailto:someone@example.org", "bad"),
                             ("javascript:alert(1)", "bad"), ("data:text/html,x", "bad"), ("tel:5551234", "bad"),
                             ("vbscript:x", "bad"), (" JavaScript:x", "bad")):
            self.assertEqual(kind(target), want, target)
        self.assertEqual(host("//www.athletic.net/x"), "www.athletic.net")
        self.assertTrue(WEB.search('<a href="//www.athletic.net/athlete/1">'))


class Site(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        given = os.environ.get("ATH_EXAMPLES_SITE")
        if given:
            cls.site = Path(given).resolve()
        else:
            cls.tmp = tempfile.TemporaryDirectory()
            cls.site = Path(cls.tmp.name) / "site"
            with redirect_stdout(io.StringIO()):
                build_site.main(cls.site)
        cls.files = sorted(p for p in cls.site.rglob("*") if p.is_file())
        cls.pages = [p for p in cls.files if p.suffix == ".html"]
        cls.rankmaps = [cls.site / t for t in targets(cls.site / "index.html")
                        if kind(t) == "page" and t.endswith(".html") and t not in ("index.html", report.DEMO_PAGE)]

    @classmethod
    def tearDownClass(cls):
        if hasattr(cls, "tmp"):
            cls.tmp.cleanup()

    def test_it_has_every_page(self):
        self.assertEqual(len(self.rankmaps), len(build_site.RANKMAPS))
        self.assertEqual({p.name for p in self.pages},
                         {"index.html", report.DEMO_PAGE} | {p.name for p in self.rankmaps})

    def test_no_athletic_net_link_anywhere(self):
        for path in self.files:
            if path.suffix not in (".html", ".md", ".csv"):
                continue
            text = path.read_text()
            self.assertNotRegex(text, r"(?i)athletic\.net/", f"{path.name}: an Athletic.net link")
            for url in WEB.findall(text):
                self.assertIn(host(url), ALLOWED_HOSTS | NAMESPACE_HOSTS, f"{path.name}: a link outside the examples: {url}")
                if host(url) == "github.com":
                    self.assertTrue(url.startswith(build_site.REPO_URL), f"{path.name}: {url}")

    def test_every_link_is_a_page_of_the_site_or_an_allowed_web_page(self):
        for path in self.pages:
            for target in targets(path):
                k = kind(target)
                self.assertNotEqual(k, "bad", f"{path.name}: {target}")
                if k == "web":
                    self.assertIn(host(target), ALLOWED_HOSTS, f"{path.name}: {target}")
                elif not target.startswith("#"):
                    local = target.split("#")[0]
                    self.assertTrue((path.parent / local).is_file(), f"{path.name}: {target} is missing")

    def test_every_page_says_what_it_is_and_may_be_indexed(self):
        for path in self.pages:
            page = path.read_text()
            self.assertEqual(page.count(report.example_footer()), 1, f"{path.name}: the footer")
            self.assertNotIn('name="robots"', page, f"{path.name}: a robots tag")
            self.assertNotIn("noindex", page, path.name)

    def test_rankmap_links_all_go_to_the_demo_page_in_the_same_tab(self):
        for path in self.rankmaps:
            page = path.read_text()
            self.assertEqual({t for t in targets(path) if kind(t) == "page"}, {report.DEMO_PAGE}, path.name)
            self.assertNotIn('target="_blank"', page, path.name)

    def test_the_state_meets_say_their_standards_are_moved(self):
        for path in self.rankmaps:
            page = path.read_text()
            if 'class="smeet"' in page:
                self.assertIn("These standards are moved as the example&#x27;s marks are", page, path.name)
        self.assertEqual(sum('class="smeet"' in p.read_text() for p in self.rankmaps), 2)  # CA, ME

    def test_the_cards_say_who_with_an_ordinal_grade(self):
        index = (self.site / "index.html").read_text()
        whos = re.findall(r'<span class="who"><span>([^<]*)</span><b>[^<]+</b></span>', index)
        self.assertEqual(len(whos), len(build_site.RANKMAPS))
        for who in whos:
            self.assertRegex(who, r", 11th grade, 2099$")

    def test_back_only_returns_within_the_site(self):
        demo = (self.site / report.DEMO_PAGE).read_text()
        self.assertIn('<a id="back" href="index.html">Back</a>', demo)
        self.assertIn("from.pathname.startsWith(here)", demo)

    def test_every_color_is_a_token(self):
        self.assertEqual(report.color_literals(build_site.__file__), [])


if __name__ == "__main__":
    unittest.main()
