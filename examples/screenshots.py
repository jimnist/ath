"""Shoot the docs' rankmap screenshots from the examples, with headless Chrome.

    python3 examples/screenshots.py [NAME ...] [--out DIR]

Builds the examples site (examples/build_site.py: anonymized data only, never a Recording or
local/) into a temporary folder, opens its pages in headless Chrome at a fixed window size and
device scale factor, waits for the web fonts, and writes each screenshot below into docs/images/
(--out DIR for another folder), cropped to the page's own elements so a crop never ends mid-element.
NAME picks some of them (default: all).

Chrome: $CHROME when set, else Google Chrome or Chromium in their usual macOS and Linux places.
The pages' fonts come from Google Fonts, so shooting needs the network. When oxipng or optipng is on
PATH, each PNG goes through it (lossless). Python 3.9+, standard library only; macOS or Linux (Chrome
is driven over --remote-debugging-pipe). See docs/anonymize.md, "The examples".
"""
import argparse
import base64
import json
import os
import select
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parent
IMAGES = REPO / "docs" / "images"
sys.path.insert(0, str(HERE))
import build_site  # noqa: E402

# The window every page is laid out in, in CSS pixels, and the device scale factor. The Sections map
# is min(78vh, 820px) tall: at 1100 high it is its full 820.
WIDTH, HEIGHT, SCALE = 1280, 1100, 1

# The page's measures, for the crops (evaluated in the page; every value in CSS pixels).
# visible(sel): the elements matching sel that are shown. content(): the .wrap's content box.
# below(el, pad): pad under el's bottom, but never past the top of the next shown block of the
# page (a .wrap child below el), so the crop never takes a sliver of what follows.
MEASURE = """
const visible = sel => [...document.querySelectorAll(sel)].filter(e => e.getClientRects().length);
const one = sel => { const v = visible(sel); if (!v.length) throw new Error("nothing shown matches " + sel); return v[0]; };
const box = e => e.getBoundingClientRect();
const content = () => { const w = one(".wrap"), r = box(w), s = getComputedStyle(w);
  return {left: r.left + parseFloat(s.paddingLeft), right: r.right - parseFloat(s.paddingRight)}; };
const below = (e, pad) => { const b = box(e).bottom;
  return Math.min(b + pad, ...visible(".wrap > *").map(c => box(c).top).filter(t => t >= b)); };
const lastOf = sel => { const v = visible(sel); if (!v.length) throw new Error("nothing shown matches " + sel); return v.reduce((a, e) => box(e).bottom > box(a).bottom ? e : a); };
"""

# Each screenshot: the file in docs/images/, the example (examples/rankmap/<key>/) and the link's
# fragment it opens on (#areas-<season>-<grades> opens on the State's Areas), and the crop, a JS
# expression for {x, y, width, height} with MEASURE's helpers.
# Docs crops are the content column with 30px each side (1108 px wide at WIDTH); the examples' are
# the whole window wide (the landing page's cards show their top left, 16:10).
DOCS_X = "(() => { const c = content(); return {x: c.left - 30, width: c.right - c.left + 60}; })()"
SHOTS = {
    # docs/rankmap.md's and the README's first image: the title, toggles, headline numbers, US map
    # with the home State outlined, and its legend with the home State key.
    "rankmap-map": ("ca", "", "({...%s, y: 0, height: below(lastOf('.legend'), 16)})" % DOCS_X),
    # docs/rankmap.md's table: its heading and the rows that fit in 990 px, ending on a row's bottom
    # line (half of it below the row's box: the table's borders collapse).
    "rankmap-table": ("ca", "", """(() => {
        const top = Math.floor(box(one('h2')).top - 12), rows = visible('.list tbody tr').map(box).filter(r => r.bottom - top < 990);
        return {...%s, y: top, height: Math.ceil(rows[rows.length - 1].bottom + .5) - top}; })()""" % DOCS_X),
    # The README's and the landing page's example cards.
    "example-rankmap-ca": ("ca", "#areas-2099-9-11", "({x: 0, width: innerWidth, y: 0, height: below(lastOf('.legend'), 16)})"),
    "example-rankmap-me": ("me", "#areas-2099-9-11", "({x: 0, width: innerWidth, y: 0, height: below(lastOf('.tbl-wrap'), 16)})"),
    "example-rankmap-or": ("or", "", "({x: 0, width: innerWidth, y: 0, height: below(lastOf('.legend'), 16)})"),
}

# The pages' fonts (report.THEME): shooting waits for them, and fails when one did not load.
FONTS = ("Roboto Condensed", "Atkinson Hyperlegible")

TIMEOUT = 60  # seconds for any one step in Chrome


def find_chrome():
    """$CHROME, else Chrome or Chromium where macOS and Linux put them; exits saying how when none."""
    if os.environ.get("CHROME"):
        chrome = shutil.which(os.environ["CHROME"])
        if not chrome:
            sys.exit(f"CHROME={os.environ['CHROME']} is not a program")
        return chrome
    apps = [Path("/Applications"), Path.home() / "Applications"]
    for app in ("Google Chrome", "Chromium", "Google Chrome for Testing"):
        for folder in apps:
            path = folder / f"{app}.app" / "Contents" / "MacOS" / app
            if path.exists():
                return str(path)
    for name in ("google-chrome", "google-chrome-stable", "chromium", "chromium-browser", "chrome"):
        if shutil.which(name):
            return shutil.which(name)
    return None


class Chrome:
    """Headless Chrome over the DevTools protocol on a pipe (--remote-debugging-pipe: commands in on
    fd 3, replies out on fd 4, each a JSON message ending in a NUL), on one page."""

    def __init__(self, path):
        self.profile = tempfile.mkdtemp(prefix="ath-shots-")
        to_chrome, self.to_chrome = os.pipe()
        self.from_chrome, from_chrome_w = os.pipe()

        def fds():  # in the child: Chrome's ends at fds 3 and 4 (high copies first, so none clobbers)
            import fcntl
            hi_in, hi_out = fcntl.fcntl(to_chrome, fcntl.F_DUPFD, 10), fcntl.fcntl(from_chrome_w, fcntl.F_DUPFD, 10)
            os.dup2(hi_in, 3)
            os.dup2(hi_out, 4)

        args = [path, "--headless=new", "--remote-debugging-pipe", f"--user-data-dir={self.profile}",
                "--no-first-run", "--no-default-browser-check", "--disable-extensions", "--hide-scrollbars",
                "--force-color-profile=srgb", "--mute-audio", "about:blank"]
        if hasattr(os, "geteuid") and os.geteuid() == 0:
            args.insert(1, "--no-sandbox")  # Chrome refuses root otherwise (a container)
        self.proc = subprocess.Popen(args, preexec_fn=fds, close_fds=False, stdin=subprocess.DEVNULL,
                                     stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        os.close(to_chrome)
        os.close(from_chrome_w)
        self.buf, self.events, self.next_id = b"", [], 0
        target = self.call("Target.createTarget", url="about:blank")["targetId"]
        self.session = self.call("Target.attachToTarget", targetId=target, flatten=True)["sessionId"]
        self.call("Page.enable", session=True)

    def read(self, deadline):
        while b"\0" not in self.buf:
            left = deadline - time.monotonic()
            if left <= 0 or not select.select([self.from_chrome], [], [], left)[0]:
                raise TimeoutError("Chrome did not answer in time")
            chunk = os.read(self.from_chrome, 1 << 20)
            if not chunk:
                raise RuntimeError("Chrome quit")
            self.buf += chunk
        msg, self.buf = self.buf.split(b"\0", 1)
        return json.loads(msg)

    def call(self, method, session=False, **params):
        self.next_id += 1
        msg = {"id": self.next_id, "method": method, "params": params}
        if session:
            msg["sessionId"] = self.session
        os.write(self.to_chrome, json.dumps(msg).encode() + b"\0")
        deadline = time.monotonic() + TIMEOUT
        while True:
            got = self.read(deadline)
            if got.get("id") == self.next_id:
                if "error" in got:
                    raise RuntimeError(f"{method}: {got['error'].get('message')}")
                return got["result"]
            if "method" in got:
                self.events.append(got)

    def wait_for(self, event):
        deadline = time.monotonic() + TIMEOUT
        while True:
            for i, got in enumerate(self.events):
                if got["method"] == event:
                    del self.events[: i + 1]
                    return got
            self.events.append(self.read(deadline))

    def js(self, expression):
        """The value of a JS expression in the page (awaited when a promise); a thrown error exits."""
        got = self.call("Runtime.evaluate", session=True, expression=expression, awaitPromise=True,
                        returnByValue=True)
        if "exceptionDetails" in got:
            d = got["exceptionDetails"]
            raise RuntimeError(d.get("exception", {}).get("description") or d.get("text"))
        return got["result"].get("value")

    def size(self, width, height):
        self.call("Emulation.setDeviceMetricsOverride", session=True, width=width, height=height,
                  deviceScaleFactor=SCALE, mobile=False)

    def open(self, url):
        """Load url and wait until its fonts are in and two frames have drawn."""
        self.size(WIDTH, HEIGHT)
        self.call("Emulation.setEmulatedMedia", session=True, features=[{"name": "prefers-color-scheme", "value": "light"}])
        # By way of a blank page: a link to the page just shot, but for its fragment, would only
        # scroll it, never load it again.
        for go in ("about:blank", url):
            self.events.clear()
            self.call("Page.navigate", session=True, url=go)
            self.wait_for("Page.loadEventFired")
        missing = self.js("""document.fonts.ready.then(() => %s.filter(f =>
            ![...document.fonts].some(ff => ff.family.replace(/"/g, "") === f && ff.status === "loaded")))""" % json.dumps(FONTS))
        if missing:
            raise RuntimeError(f"web fonts did not load ({', '.join(missing)}): shooting needs the network "
                               "for Google Fonts")
        self.frames()

    def frames(self):
        self.js("new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r)))")

    def shoot(self, crop):
        """A PNG of the page's crop (a JS expression for {x, y, width, height}), whole pixels."""
        clip = self.js(f"(() => {{ {MEASURE} return ({crop}); }})()")
        clip = {k: round(v) for k, v in clip.items()}
        if clip["y"] + clip["height"] > HEIGHT:
            # Lay out tall enough for the crop, and check that doing so moved nothing.
            self.size(WIDTH, clip["y"] + clip["height"])
            self.frames()
            again = {k: round(v) for k, v in self.js(f"(() => {{ {MEASURE} return ({crop}); }})()").items()}
            if again != clip:
                raise RuntimeError(f"the crop moved with the window height: {clip} then {again}")
        if clip["x"] < 0 or clip["x"] + clip["width"] > WIDTH or clip["width"] <= 0 or clip["height"] <= 0:
            raise RuntimeError(f"the crop {clip} is not inside the {WIDTH}px window")
        got = self.call("Page.captureScreenshot", session=True, format="png", clip={**clip, "scale": 1})
        return base64.b64decode(got["data"])

    def close(self):
        try:
            self.call("Browser.close")
        except (RuntimeError, TimeoutError, OSError):
            pass
        try:
            self.proc.wait(10)
        except subprocess.TimeoutExpired:
            self.proc.kill()
        os.close(self.to_chrome)
        os.close(self.from_chrome)
        shutil.rmtree(self.profile, ignore_errors=True)


def build(site):
    """Build the rankmap example pages into the folder site; returns {example key: its page}."""
    pages = {}
    for key, _, _ in build_site.RANKMAPS:
        pages[key] = site / build_site.build_rankmap(key, site)["href"]
    return pages


def optimize(png):
    """Shrink a PNG losslessly with oxipng or optipng when one is on PATH; returns the tool used."""
    if shutil.which("oxipng"):
        subprocess.run(["oxipng", "-q", "-o", "4", "--strip", "safe", str(png)], check=True)
        return "oxipng"
    if shutil.which("optipng"):
        subprocess.run(["optipng", "-quiet", "-o5", str(png)], check=True)
        return "optipng"
    return None


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("names", nargs="*", metavar="NAME", help=f"screenshots to take (default: all): {', '.join(SHOTS)}")
    parser.add_argument("--out", type=Path, default=IMAGES, help="the folder to write into (default: docs/images)")
    args = parser.parse_args(argv)
    unknown = [n for n in args.names if n not in SHOTS]
    if unknown:
        parser.error(f"no screenshot {', '.join(unknown)}; there are {', '.join(SHOTS)}")
    names = args.names or list(SHOTS)

    chrome_path = find_chrome()
    if not chrome_path:
        sys.exit("No Chrome found: install Google Chrome or Chromium, or set CHROME to its program")
    with tempfile.TemporaryDirectory(prefix="ath-site-") as tmp:
        pages = build(Path(tmp).resolve())
        args.out.mkdir(parents=True, exist_ok=True)
        try:
            chrome = Chrome(chrome_path)
        except (RuntimeError, TimeoutError) as e:
            sys.exit(f"Chrome ({chrome_path}) did not start: {e}")
        try:
            for name in names:
                key, fragment, crop = SHOTS[name]
                chrome.open(pages[key].as_uri() + fragment)
                png = args.out / f"{name}.png"
                png.write_bytes(chrome.shoot(crop))
                tool = optimize(png)
                print(f"Wrote {png} ({png.stat().st_size // 1024} KB{', ' + tool if tool else ''})")
        except (RuntimeError, TimeoutError) as e:
            sys.exit(f"Chrome: {e}")
        finally:
            chrome.close()


if __name__ == "__main__":
    main()
