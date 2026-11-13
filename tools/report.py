"""Pieces shared by the reports under tools/ (rankmap, seedsheet, and the ones after it).

A report renders its own page; this module holds what every report does the same way:
finding and running the ath binary it gets its data from, where its config lives, links, the US map (states as SVG paths with label
spots, and callout chips for the small states), the design tokens (THEME) and the light-only theme,
the segmented toggles, the highlight and map panel styling, and CSV export.
CSS here is plain CSS (single braces, tokens as [[name]]); styles(theme) fills it in for a page template.
Standard library only.
"""
import ast
import codecs
import csv
import functools
import hashlib
import heapq
import inspect
import html
import json
import math
import os
import re
import shlex
import shutil
import subprocess
import sys
import threading
import typing
from dataclasses import dataclass, field, fields, is_dataclass, replace
from datetime import date
from pathlib import Path

HERE = Path(__file__).parent
REPO = HERE.parent
TOPO = HERE / "us-states-albers.topo.json"  # us-atlas@3.0.1 states-albers-10m (pre-projected)

# The US map's right edge (its viewBox, see map_viewbox, is the lower 48 and DC, Alaska and Hawaii
# inset, and room on the right for the callout chips), unless a theme's callouts need more.
MAP_RIGHT = 1050

# States too small to hold a label get a chip off the coast instead.
CALLOUTS = ["VT", "NH", "MA", "RI", "CT", "NJ", "DE", "MD", "DC", "HI"]
CALLOUT_X = 1000


# ---------------------------------------------------------------- ath

def find_ath():
    """The ath binary: $ATH, else this checkout's own build (the JSON contract is versioned with
    tools/), else ath on PATH."""
    if os.environ.get("ATH"):
        return os.environ["ATH"]
    built = [p for p in (REPO / "target" / "release" / "ath", REPO / "target" / "debug" / "ath") if p.exists()]
    if built:
        return str(max(built, key=lambda p: p.stat().st_mtime))
    on_path = shutil.which("ath")
    if on_path:
        return on_path
    sys.exit(f"No ath binary: run `cargo build --release` in {REPO}, or set ATH to its path.")


# ---------------------------------------------------------------- config

# Environment variables that point git somewhere other than the folder it runs in (a git hook
# sets some of them); cleared for every git run here.
GIT_REDIRECTS = ("GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_COMMON_DIR", "GIT_OBJECT_DIRECTORY",
                 "GIT_ALTERNATE_OBJECT_DIRECTORIES", "GIT_NAMESPACE", "GIT_PREFIX")


def git_env():
    return {k: v for k, v in os.environ.items() if k not in GIT_REDIRECTS}


@functools.lru_cache(maxsize=None)
def main_checkout(repo=REPO):
    """The main working tree of the git checkout at repo: the repository's core.worktree when it
    sets one (a submodule), else the first non-bare entry of `git worktree list`, so linked
    worktrees and a bare repo's worktrees find theirs. repo itself when it is not a git checkout,
    or (with a warning) when git cannot say."""
    repo = Path(repo).resolve()
    if not (repo / ".git").exists():
        return repo

    def git(*args):
        try:
            run = subprocess.run(["git", *args], cwd=repo, env=git_env(), capture_output=True, text=True)
        except OSError as e:
            return None, f"cannot run git ({e})"
        if run.returncode != 0:
            return None, run.stderr.strip() or f"git {args[0]} exited {run.returncode}"
        return run.stdout.strip(), None

    common, why = git("rev-parse", "--path-format=absolute", "--git-common-dir")
    if common:
        worktree, _ = git("--git-dir", common, "config", "--local", "--get", "core.worktree")
        if worktree:
            return (Path(common) / worktree).resolve()
        listed, why = git("worktree", "list", "--porcelain")
        for entry in (listed or "").split("\n\n"):
            lines = entry.strip().splitlines()
            if lines and lines[0].startswith("worktree ") and "bare" not in lines[1:]:
                return Path(lines[0][len("worktree "):]).resolve()
        why = why or "git lists no working tree"
    print(f"warning: local/ and the older tools/ places fall back to {repo}, since git could not "
          f"find the main checkout: {why}. Set ATH_LOCAL to choose local/.", file=sys.stderr)
    return repo


def local_dir(repo=REPO):
    """Your local/ folder: $ATH_LOCAL, else local/ in the main checkout, so every git worktree of it
    shares one (see main_checkout)."""
    if os.environ.get("ATH_LOCAL"):
        return Path(os.environ["ATH_LOCAL"]).expanduser().resolve()
    return main_checkout(repo) / "local"


def older_place(report, *parts, repo=REPO):
    """Where the main checkout kept a report's things before local/: tools/<report>/<parts>."""
    return main_checkout(repo).joinpath("tools", report, *parts)


def config_path(report, given, local=None, older=None):
    """The config to run (given: --config's text, or None): a path when it has a slash in it (./x
    for one in the current folder), else a name in <local>/<report>/ (.config.json added when
    left off), else default_config."""
    if given is None:
        return default_config(report, local, older)
    if "/" in given or os.sep in given or given.startswith("~"):
        return Path(given).expanduser().resolve()
    name = given if given.endswith(".json") else f"{given}.config.json"
    return (local or local_dir()) / report / name


def default_config(report, local=None, older=None):
    """A report's config when --config is not given: <local>/<report>/<report>.config.json, or the
    older tools/<report>/<report>.config.json (in the main checkout) while only that one exists.
    Warns (stderr) when both exist, since the older one is then ignored."""
    home = (local or local_dir()) / report / f"{report}.config.json"
    older = older or older_place(report, f"{report}.config.json")
    if older.exists() and home.exists():
        print(f"warning: using {home}; {older} is ignored now (move anything you still need from it)",
              file=sys.stderr)
    return older if older.exists() and not home.exists() else home


def check_moved(data_dir, old_dir, names):
    """Stop when none of a config's saved files (names) are in its data_dir but some are in the
    older place (old_dir), printing the move, rather than collect them all again."""
    data_dir, old_dir = data_dir.resolve(), old_dir.resolve()
    if old_dir == data_dir or any((data_dir / n).exists() for n in names):
        return
    found = [old_dir / n for n in names if (old_dir / n).exists()]
    if not found:
        return
    sys.exit(f"{data_dir} has none of this config's saved files, but {old_dir} has: they are kept "
             f"beside the config now. Move them, then run again:\n"
             f"  mkdir -p {shlex.quote(str(data_dir))} && mv {' '.join(shlex.quote(str(p)) for p in found)} "
             f"{shlex.quote(str(data_dir))}/")


def beside(config, p):
    """A path from a config file, resolved: ~ expanded, and relative paths taken from the config's
    own folder, so a config keeps its data and output next to it wherever it lives."""
    p = Path(p).expanduser()
    return (p if p.is_absolute() else config.parent / p).resolve()


def no_config(path, example):
    """Exit saying how to start a config at path from the report's example."""
    sys.exit(f"No config at {path}. Make its folder (mkdir -p {path.parent}), copy {example} there "
             f"as {path.name}, and fill it in.")


RELAY_HINT = ("ath fetches through the browser relay: keep an Athletic.net tab connected "
              "(`ath relay install` sets it up), or set ATH_COOKIE.")


class StderrTee:
    """A pipe to give a child process as its stderr: what the child writes is passed on to this
    process's stderr as it comes (so ath's relay instructions show while it waits), and kept, as
    text, once close() has been called."""

    def __init__(self):
        read, self.fd = os.pipe()
        self.text = ""
        self._chunks = []
        self._thread = threading.Thread(target=self._pump, args=(read,), daemon=True)
        self._thread.start()

    def _pump(self, read):
        decoder = codecs.getincrementaldecoder("utf-8")(errors="replace")
        with os.fdopen(read, "rb", buffering=0) as f:
            for chunk in iter(lambda: f.read(4096), b""):
                text = decoder.decode(chunk)
                self._chunks.append(text)
                sys.stderr.write(text)
                sys.stderr.flush()
        self._chunks.append(decoder.decode(b"", final=True))

    def close(self):
        """Close this end of the pipe (the child has exited, or never started) and wait for the
        rest of its text. Safe to call more than once."""
        if self.fd is not None:
            os.close(self.fd)
            self.fd = None
        self._thread.join()
        self.text = "".join(self._chunks)


@dataclass(frozen=True)
class AthFailure:
    """Why an ath run gave no report: its reason (ath's own `Error:` message when it has one),
    whether ath could not reach Athletic.net at all (transport: no relay tab, a tab that stopped
    answering, the network, a challenge or expired cookie, the relay's port taken), and whether
    Athletic.net refused the whole meet (meet_wide: a 401/403 on its event list or entries, which
    need a paid session or are not published). Every run after either fails the same way."""
    reason: str
    transport: bool = False
    meet_wide: bool = False


# How ath words a failure to reach Athletic.net, or to start fetching at all (src/relay.rs,
# src/http.rs, src/athletic.rs), as opposed to a refusal of what it was asked: lowercase pieces of
# its error text.
TRANSPORT_SIGNS = (
    "starting the browser relay",  # its port is taken: another ath is running
    "address already in use",
    "ath relay",  # no tab connected, or the tab stopped answering or did not answer
    "browser relay dropped",
    "browser tab could not fetch",
    "browser tab answered",
    "cloudflare challenge",
    "session cookie",
    "attempts.",  # throttled or challenged on every retry
    "error sending request",
    "connection refused",
    "connection reset",
    "dns error",
)

# How ath words Athletic.net refusing a meet's event list or entries (src/athletic.rs `refused`).
MEET_REFUSED_SIGNS = ("athletic.net refused",)


def ath_error(stderr, name, returncode):
    """What ath said went wrong, from its stderr, as an AthFailure: its `Error: ...` message (the
    first line of the last one) and whether its error (that message and what follows it, the
    causes) is a transport failure or a refusal of the whole meet; else that it failed with its
    exit code."""
    lines = stderr.splitlines()
    errors = [i for i, line in enumerate(lines) if line.startswith("Error: ")]
    if not errors:
        return AthFailure(f"{name} failed (exit {returncode})")
    tail = "\n".join(lines[errors[-1]:]).lower()
    return AthFailure(lines[errors[-1]][len("Error: "):].strip(), any(s in tail for s in TRANSPORT_SIGNS),
                      any(s in tail for s in MEET_REFUSED_SIGNS))


def run_ath_report(cmd, label, errors=None):
    """Run an ath command that prints a JSON report (cmd: [ath, subcommand, ...]) and return the
    report, or None after saying why (`  <label>: ...`): interrupted, ath failed, or it printed
    no JSON. ath's warnings and relay instructions go straight to the terminal (stderr). errors:
    a list to add an AthFailure to when there is no report; with it, an interrupt (Ctrl-C) is
    raised again rather than counted as a failure, so it never stands as a reason, and the caller
    says what it kept (this says only that ath failed)."""
    name = f"ath {cmd[1]}"
    tee = StderrTee() if errors is not None else None
    try:
        proc = subprocess.run(cmd, stdout=subprocess.PIPE, text=True, **({"stderr": tee.fd} if tee else {}))
    except KeyboardInterrupt:
        if tee:
            raise
        print(f"\n  {label}: skipped")
        return None
    finally:
        if tee:
            tee.close()
    if proc.returncode != 0:
        if tee:
            print(f"  {label}: {name} failed (exit {proc.returncode})")
            errors.append(ath_error(tee.text, name, proc.returncode))
        else:
            print(f"  {label}: {name} failed (exit {proc.returncode}); kept what was saved")
        return None
    try:
        return json.loads(proc.stdout)
    except ValueError as e:
        print(f"  {label}: not saved, {name} printed no JSON report ({e})")
        if errors is not None:
            errors.append(AthFailure(f"{name} printed no report"))
        return None


# ---------------------------------------------------------------- text

def esc(t):
    return html.escape(str(t))


def link(text, url, cls=None):
    """A link that opens in a new tab, text and URL escaped, with an optional class."""
    c = f' class="{cls}"' if cls else ""
    return f'<a{c} href="{esc(url)}" target="_blank" rel="noopener">{esc(text)}</a>'


# ---------------------------------------------------------------- examples

# Real pages aggregate Athletic.net's lists for the people they are shared with: they ask search
# engines to stay away (this line, in their <head>) and never name Athletic.net in visible text.
ROBOTS = '<meta name="robots" content="noindex, nofollow, noarchive">\n'

# Example pages (a report's `example` mode, built from anonymized data for the published examples,
# ADR 0010) are the other way round: indexable, every Athletic.net link sent to one demo page beside
# them, and a footer saying what they are.
DEMO_PAGE = "demo.html"
EXAMPLE_NOTE = ("Example with anonymized data: names, schools, meets, seasons and marks are invented or "
                "shifted, and ranks are realistic, not real. Built with ath from Athletic.net data; not "
                "affiliated with or endorsed by Athletic.net. Real use needs a paid Athletic.net subscription.")

# The footer every example page ends with (<footer class="example">, inside the page's column).
FOOTER_CSS = """footer.example { margin-top: 8px; padding-top: 14px; border-top: 1px solid var(--line); color: var(--muted); font-size: 13px; max-width: 72ch; }
footer.example p { margin: 0; }
footer.example a { color: inherit; }"""


def example_footer():
    """An example page's footer (styled by FOOTER_CSS)."""
    return f'<footer class="example"><p>{esc(EXAMPLE_NOTE)}</p></footer>'


def page_link(text, url, cls=None, example=False):
    """A report page's link: link() on a real page; on an example page, a link to DEMO_PAGE in the
    same tab, whatever url is (an example has no real page to link to)."""
    if not example:
        return link(text, url, cls)
    c = f' class="{cls}"' if cls else ""
    return f'<a{c} href="{DEMO_PAGE}">{esc(text)}</a>'


def pretty_date(iso):
    d = date.fromisoformat(iso)
    return f"{d:%B} {d.day}, {d.year}"


def write_json(path, data):
    """Save data as one line of JSON, atomically: written beside the file, then moved over it, so
    an interrupted save leaves the file as it was."""
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_name(f".{path.name}.tmp")
    tmp.write_text(json.dumps(data, ensure_ascii=False, separators=(",", ":")) + "\n")
    os.replace(tmp, path)


def write_csv(path, header, rows):
    with open(path, "w", newline="") as f:
        w = csv.writer(f)
        w.writerow(header)
        w.writerows(rows)


# ---------------------------------------------------------------- US map

def decode_topology(topo):
    sx, sy = topo["transform"]["scale"]
    tx, ty = topo["transform"]["translate"]
    arcs = []
    for arc in topo["arcs"]:
        x = y = 0
        pts = []
        for dx, dy in arc:
            x += dx
            y += dy
            pts.append((x * sx + tx, y * sy + ty))
        arcs.append(pts)

    states = {}
    for g in topo["objects"]["states"]["geometries"]:
        polys = g["arcs"] if g["type"] == "MultiPolygon" else [g["arcs"]]
        states[g["properties"]["name"]] = [[_ring(arcs, r) for r in poly] for poly in polys]
    return states


def ring_area(pts):
    return abs(sum(x0 * y1 - x1 * y0 for (x0, y0), (x1, y1) in zip(pts, pts[1:] + pts[:1]))) / 2


def seg_dist2(px, py, ax, ay, bx, by):
    dx, dy = bx - ax, by - ay
    t = 0.0
    if dx or dy:
        t = max(0.0, min(1.0, ((px - ax) * dx + (py - ay) * dy) / (dx * dx + dy * dy)))
    ex, ey = ax + t * dx - px, ay + t * dy - py
    return ex * ex + ey * ey


def inside(px, py, rings):
    """Even-odd: inside one of rings (polygons that do not overlap, holes as rings)."""
    hit = False
    for r in rings:
        for (ax, ay), (bx, by) in zip(r, r[1:] + r[:1]):
            if (ay > py) != (by > py) and px < (bx - ax) * (py - ay) / (by - ay) + ax:
                hit = not hit
    return hit


def signed_dist(px, py, rings):
    """Distance to the nearest edge, negative outside. One pass (it is polylabel's hot loop),
    with the same even-odd test as inside()."""
    hit = False
    best = math.inf
    for r in rings:
        for (ax, ay), (bx, by) in zip(r, r[1:] + r[:1]):
            if (ay > py) != (by > py) and px < (bx - ax) * (py - ay) / (by - ay) + ax:
                hit = not hit
            best = min(best, seg_dist2(px, py, ax, ay, bx, by))
    return math.sqrt(best) * (1 if hit else -1)


def polylabel(rings, precision=0.5):
    """Pole of inaccessibility: the interior point farthest from any edge, and that distance."""
    xs = [p[0] for p in rings[0]]
    ys = [p[1] for p in rings[0]]
    x0, y0, x1, y1 = min(xs), min(ys), max(xs), max(ys)
    size = min(x1 - x0, y1 - y0)
    h = size / 2
    heap = []

    def push(cx, cy, h):
        d = signed_dist(cx, cy, rings)
        heapq.heappush(heap, (-(d + h * math.sqrt(2)), cx, cy, h, d))

    x = x0
    while x < x1:
        y = y0
        while y < y1:
            push(x + h, y + h, h)
            y += size
        x += size
    best = (signed_dist((x0 + x1) / 2, (y0 + y1) / 2, rings), (x0 + x1) / 2, (y0 + y1) / 2)
    while heap:
        neg_max, cx, cy, ch, d = heapq.heappop(heap)
        if d > best[0]:
            best = (d, cx, cy)
        if -neg_max - best[0] <= precision:
            continue
        ch /= 2
        for ox in (-ch, ch):
            for oy in (-ch, ch):
                push(cx + ox, cy + oy, ch)
    return best[1], best[2], best[0]


def path_d(polys):
    return "".join(
        "M" + "L".join(f"{x:.1f},{y:.1f}" for x, y in r) + "Z" for poly in polys for r in poly)


def us_map(abbr_of):
    """The states named in abbr_of ({state name: two-letter code}) as (places, shapes).

    places: {abbr: (x, y, radius)}, the label spot in the state's biggest polygon and the room
    around it. shapes: [(abbr, SVG path d)] in map order."""
    geo = decode_topology(json.loads(TOPO.read_text()))
    places, shapes = {}, []
    for name, polys in geo.items():
        abbr = abbr_of.get(name)
        if not abbr:
            continue
        biggest = max(polys, key=lambda p: ring_area(p[0]))
        places[abbr] = polylabel(biggest)
        shapes.append((abbr, path_d(polys)))
    missing = set(abbr_of.values()) - set(places)
    assert not missing, missing
    return places, shapes


# ---------------------------------------------------------------- rank labels

# A rank label's glyphs and a callout's text are measured in the display font: a theme's metrics
# (Metrics, with the design tokens below).
LABEL_DIGITS = 4  # the longest rank spotted; longer ones shrink into its spot (rankmap's spot())
LABEL_SHAPES = [f"{t}{n}" for t in ("", "t") for n in range(1, LABEL_DIGITS + 1)]
LABEL_FILL = 0.85  # of the biggest box that fits, so a number keeps clear of the borders
LABEL_MIN, LABEL_MAX = 10, 80  # font sizes, in map units
CENTER_PULL = 0.05  # room given up per unit of distance from the shape's center of area
SPOTS = HERE / "maps" / "label-spots.json"  # the house theme's (THEME's) spots


def label_shape(label):
    """A rank label's shape for spotting: its digit count, with a leading t for a tie (T12: t2)."""
    return ("t" if label.startswith("T") else "") + str(len(label.lstrip("T")))


def label_width(shape, metrics=None):
    """Width of a label shape's ink, in em, in metrics (THEME's by default)."""
    m = metrics or THEME.metrics
    return m.digit_em * int(shape.lstrip("t")) + (m.tie_em if shape.startswith("t") else 0) - m.bearing_em


def label_aspect(shape, metrics=None):
    """Width over height of a label shape's ink, in metrics (THEME's by default)."""
    return label_width(shape, metrics) / (metrics or THEME.metrics).ink_em


def box_room(px, py, aspect, segs):
    """Half-height of the biggest box of this aspect centered at (px, py) that no boundary
    segment enters: the L-infinity distance to the boundary with x shrunk by the aspect."""
    best = math.inf
    for ax, ay, bx, by, x0, x1, y0, y1 in segs:
        # The segment's box is a lower bound; most segments stop here.
        gx = max(x0 - px, px - x1, 0.0) / aspect
        gy = max(y0 - py, py - y1, 0.0)
        if max(gx, gy) >= best:
            continue
        p0, pd = (ax - px) / aspect, (bx - ax) / aspect
        q0, qd = ay - py, by - ay
        us = [0.0, 1.0]
        if pd:
            us.append(-p0 / pd)
        if qd:
            us.append(-q0 / qd)
        if qd != pd:
            us.append((p0 - q0) / (qd - pd))
        if qd != -pd:
            us.append(-(p0 + q0) / (pd + qd))
        for u in us:
            if 0.0 <= u <= 1.0:
                best = min(best, max(abs(p0 + u * pd), abs(q0 + u * qd)))
    return best


def centroid(rings):
    """Center of area of rings (holes wound the other way subtract)."""
    a = cx = cy = 0.0
    for r in rings:
        for (x0, y0), (x1, y1) in zip(r, r[1:] + r[:1]):
            c = x0 * y1 - x1 * y0
            a += c
            cx += (x0 + x1) * c
            cy += (y0 + y1) * c
    return cx / (3 * a), cy / (3 * a)


def edges(lines):
    """Boundary segments, with their boxes, from point lists."""
    return [(ax, ay, bx, by, min(ax, bx), max(ax, bx), min(ay, by), max(ay, by))
            for pts in lines for (ax, ay), (bx, by) in zip(pts, pts[1:])]


def fit_label(rings, segs, aspect, cap, precision=0.25):
    """The biggest box of this aspect inside rings, up to a half-height of cap, pulled a little
    toward their center of area: (center x, center y, half-height). Branch and bound as in
    polylabel. Room past cap counts for nothing, so a label at its biggest goes to the middle."""
    cx, cy = centroid(rings)
    xs = [p[0] for r in rings for p in r]
    ys = [p[1] for r in rings for p in r]
    x0, y0, x1, y1 = min(xs), min(ys), max(xs), max(ys)
    slope = max(1 / aspect, 1.0) + CENTER_PULL * math.sqrt(2)  # how fast a score can change

    def score(x, y):
        room = min(cap, box_room(x, y, aspect, segs)) if inside(x, y, rings) else 0.0
        return room - CENTER_PULL * math.hypot(x - cx, y - cy), room

    size = min(x1 - x0, y1 - y0)
    heap = []

    def push(x, y, h):
        sc, room = score(x, y)
        heapq.heappush(heap, (-(sc + h * slope), x, y, h, sc, room))

    y = y0
    while y < y1:
        x = x0
        while x < x1:
            push(x + size / 2, y + size / 2, size / 2)
            x += size
        y += size
    # Start from the better of the center of area and the pole of inaccessibility.
    best = max(score(x, y) + (x, y) for x, y in ((cx, cy), polylabel(rings)[:2]))
    while heap:
        bound, x, y, h, sc, room = heapq.heappop(heap)
        if sc > best[0]:
            best = (sc, room, x, y)
        if -bound - best[0] <= precision:
            continue
        h /= 2
        for ox in (-h, h):
            for oy in (-h, h):
                push(x + ox, y + oy, h)
    return best[2], best[3], best[1]


def label_spot(rings, segs, shape, metrics=None):
    """Where a label of this shape goes and how big: [anchor x, anchor y, font size], the anchor
    being the SVG text's (centered, dominant-baseline central), in metrics (THEME's by default)."""
    m = metrics or THEME.metrics
    x, y, room = fit_label(rings, segs, label_aspect(shape, m), LABEL_MAX * m.ink_em / 2 / LABEL_FILL)
    size = max(LABEL_MIN, min(LABEL_MAX, 2 * room / m.ink_em * LABEL_FILL))
    return [round(x, 1), round(y - m.ink_drop_em * size, 1), round(size, 1)]


def us_regions():
    """{state name: (rings, boundary segments)} of each state's biggest polygon."""
    out = {}
    for name, polys in decode_topology(json.loads(TOPO.read_text())).items():
        biggest = max(polys, key=lambda p: ring_area(p[0]))
        out[name] = (biggest, edges(r + r[:1] for r in biggest))
    return out


def spots_key(theme):
    """What a theme's label spots depend on: the metrics the fitting reads (the Metrics fields
    marked spots) and the display font they were measured in (its first family)."""
    m = theme.metrics
    return (tuple(getattr(m, f.name) for f in fields(m) if f.metadata.get("spots")), primary_family(theme.display))


def spots_inputs(theme=None):
    """A hash of everything a theme's label spots (THEME's: label-spots.json) are computed from: the
    constants, the display font's metrics and the font they were measured in (spots_key), the
    fitting code's own source, the US map and each mapped Area file's map and counties."""
    metrics, family = spots_key(theme or THEME)
    h = hashlib.sha256(f"{' '.join(map(str, metrics))} {family} "
                       f"{LABEL_SHAPES} {LABEL_FILL} {LABEL_MIN} {LABEL_MAX} {CENTER_PULL}".encode())
    # The functions the spots come from, listed by hand: hashing the whole module would ask for a
    # rerun after every CSS change. A new helper the fitting calls belongs here.
    for f in (_ring, decode_topology, ring_area, seg_dist2, inside, signed_dist, polylabel, label_width,
              label_aspect, box_room, centroid, edges, fit_label, label_spot, us_regions, area_regions,
              compute_spots):
        h.update(inspect.getsource(f).encode())
    h.update(TOPO.read_bytes())
    for path in sorted(AREAS_DIR.glob("*.json")):
        areas = json.loads(path.read_text())
        if areas.get("map"):
            h.update(path.stem.encode())
            h.update((REPO / areas["map"]).read_bytes())
            h.update(json.dumps([[a["name"], a["counties"]] for a in areas["areas"]]).encode())
    return h.hexdigest()


def compute_spots(theme=None, progress=None):
    """A theme's label spots (THEME's by default), as label_spots gives them: every label shape's
    spot (label_spot) in every state on the US map and every Area with counties in an Area file
    that has a map. progress: called with a line of text as each region is done, or None. Takes
    about ten seconds."""
    theme = theme or THEME
    say = progress or (lambda line: None)

    def spots_for(regions):
        out = {}
        for name, (rings, segs) in sorted(regions.items()):
            out[name] = {shape: label_spot(rings, segs, shape, theme.metrics) for shape in LABEL_SHAPES}
            say(f"  {name}")
        return out

    say("US states")
    data = {"inputs": spots_inputs(theme), "us": spots_for(us_regions()), "areas": {}}
    for path in sorted(AREAS_DIR.glob("*.json")):
        areas = json.loads(path.read_text())
        if areas.get("map"):
            say(f"{path.stem} {areas['labels']}")
            data["areas"][path.stem] = spots_for(area_regions(areas))
    return data


def spots_cache():
    """Where the label spots of a theme other than the house's are kept: local/cache/label-spots/
    (see local_dir), one file per inputs hash."""
    return local_dir() / "cache" / "label-spots"


@functools.lru_cache(maxsize=None)
def label_spots(theme=None, cache_dir=None):
    """A theme's label spots (THEME's by default): {"inputs", "us": {state name: {shape: spot}},
    "areas": {ST: {Area name: {shape: spot}}}} (see label_spot).

    THEME's, and any theme's with THEME's spots_key (its display font and fitted metrics), are
    tools/maps/label-spots.json, made by tools/maps/label_spots.py (a test says when to rerun it).
    Another theme's are computed once (compute_spots, about ten seconds) and kept in cache_dir
    (spots_cache() by default) as <inputs hash>.json, so they are computed again when the maps or
    the fitting change."""
    theme = theme or THEME
    if spots_key(theme) == spots_key(THEME):
        return json.loads(SPOTS.read_text())
    inputs = spots_inputs(theme)
    path = Path(cache_dir or spots_cache()) / f"{inputs}.json"
    try:
        data = json.loads(path.read_text())
        if isinstance(data, dict) and data.get("inputs") == inputs:
            return data
    except (OSError, ValueError):
        pass
    print(f"Fitting the rank labels to {primary_family(theme.display)}'s metrics (about ten seconds, once; "
          f"kept in {path.parent})", file=sys.stderr, flush=True)
    data = compute_spots(theme)
    write_json(path, data)
    return data


def callout_spots(places, theme=None):
    """(abbr, chip x, chip y) for each small state on the map: the East Coast ones stacked
    north to south off the coast, Hawaii beside itself. theme: THEME by default."""
    ch = chips(theme or THEME)
    east = sorted((a for a in CALLOUTS if a != "HI" and a in places), key=lambda a: places[a][1])
    spots = [(a, CALLOUT_X, 190 + i * ch.step) for i, a in enumerate(east)]
    if "HI" in places:
        hx, hy, _ = places["HI"]
        spots.append(("HI", hx + ch.hawaii, hy - 4))
    return spots


# ---------------------------------------------------------------- a State's Areas

AREAS_DIR = REPO / "areas"

# What an `areas_dir` file sets in a State Meet (`state_meet`), by the meet's kind (state_meet()
# in tools/rankmap/build.py): its standards (CIF's at-large marks for a meet by places, the MPA's
# automatic and provisional ones for a meet by standards; required) and what is said about them.
OVERRIDE_KEYS = {"places": ("at_large",), "standards": ("standards",)}
ABOUT_KEYS = ("notes", "sources", "checked")


def meet_kind(meet):
    """A State Meet's kind: entered by standards (Maine's, a `standards` key) or by places."""
    return "standards" if "standards" in meet else "places"


def _stamp(path):
    """What a file's cached reading depends on: its size and change time (None when missing)."""
    try:
        st = path.stat()
    except FileNotFoundError:
        return None
    return st.st_size, st.st_mtime_ns


def state_areas(state, areas_dir=None):
    """A State's Area file (areas/<ST>.json: its Areas, the label pages use, the county map), or
    None when the State has none. Read once per State and areas_dir, again when a file it reads
    changes; callers must not change it.

    areas_dir (a Path; an example's, ADR 0010): the State Meet's standards and what is said about
    them come from areas_dir/<ST>.json only, never from areas/: there, the anonymizer's standards,
    mapped as the example's marks are, as `{"state_meet": {...}}` with the keys of the meet's kind
    (OVERRIDE_KEYS, required, and ABOUT_KEYS). Everything else (the Areas, the map, the State
    Meet's rules) is areas/'s, so it stays in step with it. Every State whose Area file has a State
    Meet needs its file there, named exactly `<ST>.json`; any other file there (but dotfiles) is
    an error, as is a file of another shape (ValueError, naming the file)."""
    path = AREAS_DIR / f"{state}.json"
    if areas_dir is None:
        return _read_areas(path, _stamp(path), None, ())
    areas_dir = Path(areas_dir)
    listing = tuple(sorted((p.name, _stamp(p)) for p in areas_dir.iterdir() if not p.name.startswith(".")))
    return _read_areas(path, _stamp(path), areas_dir, listing)


@functools.lru_cache(maxsize=None)
def _read_areas(path, stamp, areas_dir, listing):
    """state_areas, cached on what it reads: the Area file's stamp and areas_dir's files."""
    if stamp is None:
        return None
    areas = json.loads(path.read_text())
    if areas_dir is None:
        return areas
    with_meet = {p.stem for p in AREAS_DIR.glob("*.json") if json.loads(p.read_text()).get("state_meet")}
    stray = [name for name, _ in listing if not (name.endswith(".json") and name[:-5] in with_meet)]
    if stray:
        raise ValueError(f"{areas_dir}: {', '.join(stray)}: an areas_dir holds only <ST>.json, for a State whose "
                         f"Area file has a State Meet ({', '.join(sorted(with_meet))})")
    meet = areas.get("state_meet")
    if not meet:
        return areas
    own_path = areas_dir / path.name
    if path.name not in {name for name, _ in listing}:
        raise ValueError(f"{own_path}: missing; an example needs its own {path.stem} State Meet standards")
    try:
        own = json.loads(own_path.read_text())
    except ValueError as e:
        raise ValueError(f"{own_path}: not valid JSON ({e})") from None
    if not isinstance(own, dict) or set(own) != {"state_meet"} or not isinstance(own["state_meet"], dict):
        raise ValueError(f"{own_path}: must be an object with only a \"state_meet\" object in it")
    own_meet, kind = own["state_meet"], meet_kind(meet)
    allowed = OVERRIDE_KEYS[kind] + ABOUT_KEYS
    wrong = sorted(set(own_meet) - set(allowed))
    missing = [k for k in OVERRIDE_KEYS[kind] if not isinstance(own_meet.get(k), dict)]
    if wrong or missing:
        raise ValueError(f"{own_path}: a State Meet by {kind} takes {', '.join(allowed)} "
                         f"({', '.join(OVERRIDE_KEYS[kind])} required, an object)"
                         + (f"; not {', '.join(wrong)}" if wrong else "")
                         + (f"; {', '.join(missing)} missing" if missing else ""))
    for key in OVERRIDE_KEYS[kind] + ABOUT_KEYS:
        meet.pop(key, None)
    meet.update(own_meet)
    return areas


def _ring(arcs, idx):
    pts = []
    for i in idx:
        a = arcs[i] if i >= 0 else arcs[~i][::-1]
        pts.extend(a if not pts else a[1:])
    return [tuple(p) for p in pts]


def _line(pts):
    return "M" + "L".join(f"{x},{y}" for x, y in pts)


def state_map(areas):
    """A State's county map drawn by Area, from its Area file (`map`, `areas[].counties`).

    Returns {"viewBox", "counties": [(county, Area name, path d)], "borders": d (Area borders and
    the State's outline), "lines": d (county lines inside an Area), "places": {Area name:
    (x, y, radius)} (the label spot in its roomiest county), "anchors": {Area name: (x, y)} for
    Areas with no counties of their own (a city's schools: the spot in the county they are in)}."""
    m = json.loads((REPO / areas["map"]).read_text())
    arcs = m["arcs"]
    area_of = {c: a["name"] for a in areas["areas"] for c in a["counties"]}
    missing = set(m["counties"]) - set(area_of)
    assert not missing, f"counties with no Area in areas/{areas['state']}.json: {sorted(missing)}"

    users = {}
    counties, places, county_spot = [], {}, {}
    for county, polys in m["counties"].items():
        rings = [[_ring(arcs, r) for r in poly] for poly in polys]
        counties.append((county, area_of[county], path_d(rings)))
        biggest = max(rings, key=lambda p: ring_area(p[0]))
        county_spot[county] = polylabel(biggest)
        for poly in polys:
            for r in poly:
                for i in r:
                    users.setdefault(i if i >= 0 else ~i, []).append(area_of[county])
    for a in areas["areas"]:
        spots = [county_spot[c] for c in a["counties"]]
        if spots:
            places[a["name"]] = max(spots, key=lambda s: s[2])
    anchors = {a["name"]: county_spot[a["county"]][:2] for a in areas["areas"]
               if not a["counties"] and a.get("county") in county_spot}
    # Areas that are one district's schools, outlined over their county (`outlines`).
    districts = {}
    if areas.get("outlines"):
        for name, polys in json.loads((REPO / areas["outlines"]).read_text())["districts"].items():
            rings = [[[tuple(p) for p in r] for r in poly] for poly in polys]
            biggest = max(rings, key=lambda p: ring_area(p[0]))
            districts[name] = path_d(rings)
            anchors[name] = polylabel(biggest)[:2]
    borders = "".join(_line(arcs[i]) for i, u in sorted(users.items()) if len(u) == 1 or len(set(u)) > 1)
    lines = "".join(_line(arcs[i]) for i, u in sorted(users.items()) if len(u) > 1 and len(set(u)) == 1)
    outline = [arcs[i] for i, u in users.items() if len(u) == 1]

    def side(x, y):
        """West or east: the nearer edge of the State's outline at height y."""
        xs = [a + (b - a) * (y - ay) / (by - ay)
              for arc in outline for (a, ay), (b, by) in zip(arc, arc[1:]) if (ay - y) * (by - y) < 0]
        return "west" if not xs or x - min(xs) <= max(xs) - x else "east"

    spots = {name: p[:2] for name, p in places.items()} | anchors
    return {"viewBox": m["viewBox"], "counties": counties, "borders": borders, "lines": lines,
            "places": places, "anchors": anchors, "districts": districts, "outline": outline,
            "sides": {name: side(*xy) for name, xy in spots.items()}, "spots": spots}


def outline_extent(outline, y0, y1):
    """The State's westmost and eastmost x between heights y0 and y1 (islands count), or None
    when the band misses it: where a callout beside the State at that height has to clear."""
    xs = []
    for arc in outline:
        for (a, ay), (b, by) in zip(arc, arc[1:]):
            if max(ay, by) < y0 or min(ay, by) > y1:
                continue
            # The part of the segment inside the band.
            for x, y in ((a, ay), (b, by)):
                if y0 <= y <= y1:
                    xs.append(x)
            for yb in (y0, y1):
                if (ay - yb) * (by - yb) < 0:
                    xs.append(a + (b - a) * (yb - ay) / (by - ay))
    return (min(xs), max(xs)) if xs else None


def area_regions(areas):
    """{Area name: (rings, boundary segments)} for each Area with counties of its own: all its
    counties' rings, and the arcs that border another Area or the State's edge."""
    m = json.loads((REPO / areas["map"]).read_text())
    arcs = m["arcs"]
    area_of = {c: a["name"] for a in areas["areas"] for c in a["counties"]}
    rings, users = {}, {}
    for county, polys in m["counties"].items():
        name = area_of[county]
        for poly in polys:
            for r in poly:
                rings.setdefault(name, []).append(_ring(arcs, r))
                for i in r:
                    users.setdefault(i if i >= 0 else ~i, []).append(name)
    border = {}
    for i, u in users.items():
        for name in set(u):
            if u.count(name) == 1:  # an arc two of its own counties share is inside it
                border.setdefault(name, []).append([tuple(p) for p in arcs[i]])
    return {name: (rings[name], edges(border[name])) for name in rings}


# ---------------------------------------------------------------- design tokens

@dataclass(frozen=True)
class Swatch:
    """A data color: the fill, and the ink a number on it is drawn in."""
    fill: str
    ink: str


def css_var():
    """A Theme field that theme_css writes as a CSS custom property."""
    return field(metadata={"css_var": True})


def metric(spots=False, positive=True):
    """A Metrics field: in em, positive unless said otherwise; spots: the label fitting reads it,
    so the label spots depend on it (spots_key)."""
    return field(metadata={"spots": spots, "positive": positive})


@dataclass(frozen=True)
class Metrics:
    """The display font's metrics the maps are laid out with, in em of the text's size (house:
    Roboto Condensed, measured in Chrome). A rank label's glyphs (weight 900, tabular digits): a
    digit's advance, the tie's small raised t's advance (at .6 the size), the ink's height (the t's
    top is level with the digits'), the side bearings the ink leaves out, how far the ink's middle
    sits below an SVG `dominant-baseline: central` anchor (negative: above it), and how far the t
    rises from its own central anchor to the digits' top. A callout's text, in em of its own size: a
    chip's width per rank character (a digit and the room around it), an Area name's width per
    character (700), a state abbreviation's capital (700) and its letter-spacing (the CSS's).
    tools/maps/measure_font.html measures them for another font (docs/rankmap.md, Restyling)."""
    digit_em: float = metric(spots=True)
    tie_em: float = metric(spots=True)
    ink_em: float = metric(spots=True)
    bearing_em: float = metric(spots=True, positive=False)
    ink_drop_em: float = metric(spots=True, positive=False)
    tie_rise_em: float = metric(positive=False)
    chip_char_em: float = metric()
    name_char_em: float = metric()
    abbr_char_em: float = metric()
    abbr_tracking_em: float = metric(positive=False)


@dataclass(frozen=True)
class Theme:
    """Every color and font the report pages use, and their type sizes for copy, toggles and map
    text, as named tokens: colors are any CSS color, sizes CSS pixels. THEME holds the house values
    (see house_theme); a fork restyles every page there, and a config's theme file (load_theme)
    restyles its own pages.

    Pages read the fields made with css_var as CSS custom properties (see theme_css: --bg, --fg,
    --display, ...). The rest go into a page as values: `[[name]]` in the CSS here (see css),
    `{theme.name}` in a page template, and the rank colors through rankmap's buckets(). Other sizes
    (headings, tables, cards, paddings) are each page's layout, in its template. Everything made
    from a theme takes it as an argument (THEME by default), so a page renders in any theme."""
    # Page colors, the CSS custom properties.
    bg: str = css_var()  # page background
    surface: str = css_var()  # tables, cards and panels on the page
    fg: str = css_var()  # text, and the maps' State, Area and district outlines
    muted: str = css_var()  # secondary text, the maps' leader lines
    line: str = css_var()  # rules and borders
    accent: str = css_var()  # hovered links, focus rings, ties, standards met
    seg: str = css_var()  # a toggle group's track, the seed sheet's buttons
    seg_on: str = css_var()  # a pressed toggle, a hovered button
    seg_on_fg: str = css_var()  # text on seg_on
    highlight: str = css_var()  # the highlighter behind a mark
    map_bg: str = css_var()  # the US map's water
    # Other colors.
    map_edge: str = field()  # the map panel's border
    county_line: str = field()  # county lines inside an Area, over its fill
    highlight_row: str = field()  # the configured Athlete's row in a list, a picked Area's
    tag: str = field()  # the tag on the athlete's own Class card and on their home State's row
    tag_fg: str = field()  # text on tag
    ranks: tuple[Swatch, ...] = field()  # the rank buckets' colors, best first (rankmap's buckets())
    no_rank: Swatch = field()  # an Area with no rank in the view
    # Fonts, the CSS custom properties, the display font's metrics, and the stylesheet that loads
    # them (an https URL), or None for fonts the viewer has (system font stacks).
    display: str = css_var()  # headings, numbers and the maps' labels (measured: see metrics)
    body: str = css_var()  # copy
    metrics: Metrics = field()  # the display font's, for the maps' layout
    fonts_url: typing.Optional[str] = field(metadata={"url": True})
    # Type sizes, in px.
    copy_px: int = field()  # copy, the legend's keys, the pick line under a map
    toggle_px: int = field()  # a toggle's label
    chip_px: int = field()  # a rank in a map's callout chip (the chip is sized for it: see chips)
    abbr_px: int = field()  # a state's abbreviation beside its chip on the US map, the tags (tag)
    name_px: int = field()  # an Area's name in a State map's callout or on a Class card


def house_theme():
    """The pages' own look: THEME's values. Two inks are page colors by design, so they are
    written once: #1's rank ink is the highlighter, and an Area with no rank is inked muted."""
    highlight, muted = "#fff200", "#5d6a62"
    return Theme(
        bg="#fffcf0", surface="#ffffff", fg="#161c18", muted=muted, line="#d9e2dc",
        accent="#17793f", seg="#e6e6e6", seg_on="#3a3a3a", seg_on_fg="#ffffcc",
        highlight=highlight, map_bg="#e5f1fb",
        map_edge="#d4d4d4", county_line="rgba(255,255,255,.4)", highlight_row="#fff8c4",
        tag="#3a3a3a", tag_fg="#ffffcc",
        # Five steps of a yellow-to-green scale (ColorBrewer YlGn without its darkest green), darkest
        # at #1. Inks: the highlighter yellow on #1 and a softer yellow (#fef98b, halfway in OKLCH to
        # the palest) on #2-4 stand out; past that each number is a darker shade of its own fill
        # (same OKLCH hue), close to it (2.5:1) so it reads without competing.
        ranks=(
            Swatch("#31a354", highlight),
            Swatch("#78c679", "#fef98b"),
            Swatch("#addd8e", "#618d42"),
            Swatch("#d9f0a3", "#85994f"),
            Swatch("#ffffcc", "#a5a474"),
        ),
        no_rank=Swatch("#dfe6e1", muted),
        display='"Roboto Condensed", "Arial Narrow", "Helvetica Neue", sans-serif',
        body='"Atkinson Hyperlegible", "Helvetica Neue", Arial, sans-serif',
        # An Area name's width per character: Roboto Condensed 700 measures 10 to 11.1 map units at
        # 22px; the abbreviations' capitals about half an em.
        metrics=Metrics(digit_em=0.511, tie_em=0.188, ink_em=0.731, bearing_em=0.05, ink_drop_em=-0.019,
                        tie_rise_em=0.191, chip_char_em=0.6, name_char_em=11.2 / 22, abbr_char_em=0.5,
                        abbr_tracking_em=0.04),
        fonts_url="https://fonts.googleapis.com/css2?family=Atkinson+Hyperlegible:ital,wght@0,400;0,700;1,400"
                  "&family=Roboto+Condensed:wght@600;700;800;900&display=swap",
        copy_px=15, toggle_px=17, chip_px=15, abbr_px=13, name_px=22,
    )


THEME = house_theme()


def primary_family(stack):
    """The first family in a CSS font stack: `"Roboto Condensed", sans-serif` -> Roboto Condensed."""
    return stack.split(",")[0].strip().strip("\"'")


# ---------------------------------------------------------------- theme files

class ThemeError(ValueError):
    """A theme file that cannot be used: the message says why."""


# What a CSS value from a theme file may not hold: it would end the declaration or the <style>.
CSS_UNSAFE = re.compile(r"[;{}<>\\\x00-\x1f\x7f]")


def load_theme(path, base=None):
    """The theme a theme file makes: base (THEME by default) with the tokens the file names in
    place of base's. The file is a JSON object of Theme's fields (`_`-prefixed keys are comments),
    each as its type in JSON: a color, a font stack or a URL as text, a size as a whole number of
    px, a Swatch as {"fill", "ink"}, `ranks` as a list of as many of them as base has (rankmap's
    buckets), `metrics` as an object with every Metrics field, and `fonts_url` an https URL or null
    (no web fonts: system font stacks). A file that changes `display`'s first family must give
    `metrics` too. Raises ThemeError, naming the token."""
    base = base or THEME
    try:
        data = json.loads(Path(path).read_text())
    except FileNotFoundError:
        raise ThemeError("no such file") from None
    except ValueError as e:
        raise ThemeError(f"not valid JSON ({e})") from None
    if not isinstance(data, dict):
        raise ThemeError("a theme file is a JSON object of design tokens")
    tokens = {f.name: f for f in fields(Theme)}
    given = {k: v for k, v in data.items() if not k.startswith("_")}
    unknown = [k for k in given if k not in tokens]
    if unknown:
        raise ThemeError(f"no design token {', '.join(repr(k) for k in unknown)}; the tokens are "
                         f"{', '.join(tokens)}")
    hints = typing.get_type_hints(Theme)
    theme = replace(base, **{name: _token_value(name, hints[name], value, getattr(base, name), tokens[name].metadata)
                             for name, value in given.items()})
    if "metrics" not in given and primary_family(theme.display) != primary_family(base.display):
        raise ThemeError(
            f"'display' is {primary_family(theme.display)}, not {primary_family(base.display)}, so it needs its "
            f"'metrics' too: the maps are laid out with the display font's measurements ("
            f"{', '.join(f.name for f in fields(Metrics))}). Measure them: open tools/maps/measure_font.html in a "
            f"browser, give it the family and its stylesheet, and paste the \"metrics\" it prints into the theme "
            f"file (docs/rankmap.md, Restyling)")
    return theme


def _token_value(path, kind, value, base, meta):
    """A theme file's value for one token (path: its name, as an error names it) as kind, base's
    type annotation for it; base is the value it replaces (a tuple's length comes from it)."""
    if typing.get_origin(kind) is typing.Union:  # Optional[X]
        if value is None:
            return None
        kind = next(k for k in typing.get_args(kind) if k is not type(None))
    elif value is None:
        raise ThemeError(f"{path}: cannot be null")
    if kind is str:
        if not isinstance(value, str) or not value.strip():
            raise ThemeError(f"{path}: must be text, not {json.dumps(value)}")
        if meta.get("url"):
            if not re.fullmatch(r"https://[^\s\"'<>]+", value):
                raise ThemeError(f"{path}: must be an https:// URL (a stylesheet), or null for system fonts only")
        elif CSS_UNSAFE.search(value):
            raise ThemeError(f"{path}: {json.dumps(value)} is not a CSS value (no ; {{ }} < > or backslash)")
        return value
    if kind is int:
        if isinstance(value, bool) or not isinstance(value, int) or value <= 0:
            raise ThemeError(f"{path}: must be a whole number of px above 0, not {json.dumps(value)}")
        return value
    if kind is float:
        if isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(value):
            raise ThemeError(f"{path}: must be a number (in em), not {json.dumps(value)}")
        if meta.get("positive") and value <= 0:
            raise ThemeError(f"{path}: must be above 0, not {json.dumps(value)}")
        return float(value)
    if typing.get_origin(kind) is tuple:
        item = typing.get_args(kind)[0]
        if not isinstance(value, list) or len(value) != len(base):
            got = f"{len(value)} items" if isinstance(value, list) else json.dumps(value)
            raise ThemeError(f"{path}: must be a list of exactly {len(base)} {_describe(item)}, not {got}")
        return tuple(_token_value(f"{path}[{i}]", item, v, b, {}) for i, (v, b) in enumerate(zip(value, base)))
    if is_dataclass(kind):
        names = [f.name for f in fields(kind)]
        if not isinstance(value, dict):
            raise ThemeError(f"{path}: must be {_describe(kind)}, not {json.dumps(value)}")
        wrong = [k for k in value if k not in names]
        missing = [k for k in names if k not in value]
        if wrong or missing:
            raise ThemeError(f"{path}: must be {_describe(kind)}"
                             + (f"; no {', '.join(repr(k) for k in wrong)}" if wrong else "")
                             + (f"; {', '.join(repr(k) for k in missing)} missing" if missing else ""))
        hints = typing.get_type_hints(kind)
        return kind(**{f.name: _token_value(f"{path}.{f.name}", hints[f.name], value[f.name], getattr(base, f.name),
                                            f.metadata) for f in fields(kind)})
    raise TypeError(f"{path}: a theme file cannot set a token of type {kind}")


def _describe(kind):
    """How a theme file writes a value of kind, for an error."""
    if is_dataclass(kind):
        return "an object of " + ", ".join(f'"{f.name}"' for f in fields(kind))
    return kind.__name__


def config_theme(config, cfg):
    """The theme a report config names: its `theme`, a theme file's path (from the config's folder,
    see beside), loaded with load_theme; THEME when it names none. Exits naming the problem."""
    if "theme" not in cfg:
        return THEME
    if not isinstance(cfg["theme"], str) or not cfg["theme"].strip():
        sys.exit(f"{config}: 'theme' is a theme file's path (text), from the config's folder")
    path = beside(config, cfg["theme"])
    try:
        return load_theme(path)
    except ThemeError as e:
        sys.exit(f"{config}: theme {path}: {e}")


def theme_css(theme=THEME):
    """The light-only theme's :root block: theme's css_var fields as CSS custom properties, one
    per line in field order, each a token's name with dashes (seg_on: --seg-on). Always light: the
    page ignores the viewer's dark-mode setting."""
    props = "".join(f"  --{f.name.replace('_', '-')}: {getattr(theme, f.name)};\n"
                    for f in fields(theme) if f.metadata.get("css_var"))
    return ":root {\n" + props + "  color-scheme: light only;\n}"


# A token in report CSS: [[name]], or [[name.field]] for a field of one (a metric), which CSS and
# JS never write.
TOKEN = re.compile(r"\[\[(\w+(?:\.\w+)*)\]\]")


def num(x):
    """A number for CSS or SVG, as the pages write them: up to three decimals, no trailing zeros,
    no leading zero (0.04: .04, -0.3183: -.318)."""
    text = f"{x:.3f}".rstrip("0").rstrip(".")
    return re.sub(r"^(-?)0\.", r"\1.", text) if text not in ("0", "-0") else "0"


def css(text, theme=THEME):
    """Plain CSS with theme's tokens filled in for [[name]] (a number as num writes it); an
    unknown name raises KeyError."""
    def token(m):
        value = theme
        for name in m.group(1).split("."):
            if not is_dataclass(value) or name not in {f.name for f in fields(value)}:
                raise KeyError(f"no design token {m.group(1)!r}")
            value = getattr(value, name)
        return num(value) if isinstance(value, float) else str(value)
    return TOKEN.sub(token, text)


def attr_url(url):
    """A URL for an HTML attribute: quotes and angle brackets escaped. Its query's & stay as
    typed, which HTML reads as themselves (an & is only a character reference before name;)."""
    return url.replace('"', "&quot;").replace("<", "&lt;").replace(">", "&gt;")


# CSS's named colors (CSS Color 4), to catch one written out as a value.
CSS_COLOR_NAMES = """aliceblue antiquewhite aqua aquamarine azure beige bisque black blanchedalmond blue
blueviolet brown burlywood cadetblue chartreuse chocolate coral cornflowerblue cornsilk crimson cyan
darkblue darkcyan darkgoldenrod darkgray darkgreen darkgrey darkkhaki darkmagenta darkolivegreen
darkorange darkorchid darkred darksalmon darkseagreen darkslateblue darkslategray darkslategrey
darkturquoise darkviolet deeppink deepskyblue dimgray dimgrey dodgerblue firebrick floralwhite
forestgreen fuchsia gainsboro ghostwhite gold goldenrod gray green greenyellow grey honeydew hotpink
indianred indigo ivory khaki lavender lavenderblush lawngreen lemonchiffon lightblue lightcoral
lightcyan lightgoldenrodyellow lightgray lightgreen lightgrey lightpink lightsalmon lightseagreen
lightskyblue lightslategray lightslategrey lightsteelblue lightyellow lime limegreen linen magenta
maroon mediumaquamarine mediumblue mediumorchid mediumpurple mediumseagreen mediumslateblue
mediumspringgreen mediumturquoise mediumvioletred midnightblue mintcream mistyrose moccasin
navajowhite navy oldlace olive olivedrab orange orangered orchid palegoldenrod palegreen
paleturquoise palevioletred papayawhip peachpuff peru pink plum powderblue purple rebeccapurple red
rosybrown royalblue saddlebrown salmon sandybrown seagreen seashell sienna silver skyblue slateblue
slategray slategrey snow springgreen steelblue tan teal thistle tomato turquoise violet wheat white
whitesmoke yellow yellowgreen""".split()

# A color written out:
# - #rgb, #rgba, #rrggbb, #rrggbbaa (not an HTML entity's &#123, a legend's #2-4 or an id's #add-on);
# - a call of a CSS color function (rgb, rgba, hsl, hsla, hwb, lab, lch, oklab, oklch, color,
#   color-mix), in any case;
# - a named color anywhere in a CSS declaration's value, in a color attribute of markup, or in a
#   script's style assignment or setAttribute; prose that names one is not one.
_NAMES = "(?:" + "|".join(sorted(CSS_COLOR_NAMES, key=len, reverse=True)) + r")(?![\w-])"
_COLOR_ATTR = r"(?:[\w-]*color|fill|stroke)"
COLOR_LITERAL = re.compile(
    r"(?<![&\w])#(?:[0-9a-f]{8}|[0-9a-f]{6}|[0-9a-f]{3,4})(?![\w-])"
    r"|(?<![\w.$-])(?:rgba?|hsla?|hwb|lab|lch|oklab|oklch|color-mix|color)\("
    rf"|(?<![\w-])[a-z-]+\s*:\s*[^;{{}}\"'\n]*?(?<![\w-]){_NAMES}"
    rf"|(?<![\w-]){_COLOR_ATTR}\s*=\s*[\"']{_NAMES}"
    rf"|\.style\.[\w-]+\s*=\s*[^;\n]*?[\"']{_NAMES}"
    rf"|\.style\.setProperty\([^)\n]*?[\"']{_NAMES}"
    rf"|setAttribute\(\s*[\"'][\w-]+[\"']\s*,\s*[\"']{_NAMES}",
    re.IGNORECASE)


def color_literals(path):
    """Colors written out in a Python source file anywhere but house_theme (THEME's values), as
    (line number, match). The reports' tests keep this empty, so every color stays a token."""
    text = Path(path).read_text()
    theme = [n for n in ast.walk(ast.parse(text)) if isinstance(n, ast.FunctionDef) and n.name == "house_theme"]
    skip = {i for n in theme for i in range(n.lineno, n.end_lineno + 1)}
    return [(i, m.group()) for i, line in enumerate(text.splitlines(), 1) if i not in skip
            for m in COLOR_LITERAL.finditer(line)]


# ---------------------------------------------------------------- callout chips

@dataclass(frozen=True)
class Chips:
    """A map's callout chips at a theme's chip_px, in map units: a width per character of the rank,
    the padding beside it, the least width, half the height, the chip's left edge before its anchor,
    the gap before what sits beside it (an abbreviation, a name), how far a leader line stops short
    of it, the spacing of stacked chips, and Hawaii's chip's distance east of its label spot."""
    char: int
    pad: int
    least: int
    half: int
    left: int
    gap: int
    lead: int
    step: int
    hawaii: int

    def width(self, label):
        return max(self.least, self.char * len(label) + self.pad)


# The widest rank a chip is laid out for (a callout keeps room for it).
WIDEST_CHIP = "T123"


@functools.lru_cache(maxsize=None)
def chips(theme=THEME):
    """Callout chips sized for theme's chip_px (the numbers here are at 15px)."""
    px = theme.chip_px
    pad, least, half, left, gap, lead, hawaii = (round(n * px / 15) for n in (10, 32, 11, 16, 6, 4, 52))
    return Chips(char=round(theme.metrics.chip_char_em * px), pad=pad, least=least, half=half, left=left, gap=gap,
                 lead=lead, step=2 * half + 8, hawaii=hawaii)


def chip_width(label, theme=THEME):
    return chips(theme).width(label)


@functools.lru_cache(maxsize=None)
def map_viewbox(theme=THEME):
    """The US map's SVG viewBox: the lower 48 and DC, Alaska and Hawaii inset, and room on the
    right for the callout column, widened when theme's chips and abbreviations need it."""
    ch = chips(theme)
    reach = (CALLOUT_X - ch.left + ch.width(WIDEST_CHIP) + ch.gap
             + 2 * (theme.metrics.abbr_char_em + theme.metrics.abbr_tracking_em) * theme.abbr_px)
    right = max(MAP_RIGHT, round(reach))
    return f"-10 0 {right + 10} 620"


# ---------------------------------------------------------------- page styling

GOOGLE_FONTS = "https://fonts.googleapis.com/"


def head(theme=THEME):
    """<head> lines after the title: the light-only theme and the stylesheet that loads the display
    and body fonts (none when theme.fonts_url is None: system fonts), with Google Fonts' two hosts
    preconnected when it is theirs."""
    lines = '<meta name="claude:viewport-fit-sides" content="cover">\n<meta name="color-scheme" content="light only">'
    if theme.fonts_url is None:
        return lines
    if theme.fonts_url.startswith(GOOGLE_FONTS):
        lines += ('\n<link rel="preconnect" href="https://fonts.googleapis.com">'
                  '\n<link rel="preconnect" href="https://fonts.gstatic.com" crossorigin>')
    return lines + f'\n<link rel="stylesheet" href="{attr_url(theme.fonts_url)}">'


# Page base: hidden toggling, safe areas, body type and the centered column.
BASE_CSS = """[hidden] { display: none !important; }
*, *::before, *::after { box-sizing: border-box; }
:root { padding-top: env(safe-area-inset-top, 0px); padding-bottom: env(safe-area-inset-bottom, 0px); }
body { margin: 0; background: var(--bg); color: var(--fg); font-family: var(--body); font-size: [[copy_px]]px; line-height: 1.5; -webkit-text-size-adjust: 100%; }
.wrap { max-width: 1080px; margin: 0 auto; padding-block: 28px 56px;
  padding-left: max(16px, env(safe-area-inset-left, 0px)); padding-right: max(16px, env(safe-area-inset-right, 0px));
  display: grid; gap: 18px; }"""

# The highlighted mark in a heading (<span class="pr">).
HIGHLIGHT_CSS = """h1 .pr { display: inline-block; line-height: .8; padding: .04em .12em; color: var(--fg); background: var(--highlight); border-radius: 3px; font-variant-numeric: tabular-nums; }"""

# Segmented toggles: <div class="seg"> of <button aria-pressed>.
TOGGLE_CSS = """.controls { display: flex; flex-wrap: wrap; gap: 10px 14px; align-items: center; }
.seg { display: inline-flex; flex-wrap: wrap; gap: 4px; padding: 4px; background: var(--seg); border-radius: 8px; }
.seg button { font: 700 [[toggle_px]]px/1 var(--display); letter-spacing: .02em; color: var(--fg); background: transparent; border: 0; border-radius: 5px; padding: 9px 16px; cursor: pointer; font-variant-numeric: tabular-nums; }
.seg button:hover:not(:disabled):not([aria-pressed="true"]) { background: var(--surface); }
.seg button[aria-pressed="true"] { background: var(--seg-on); color: var(--seg-on-fg); }
.seg button:disabled { color: var(--muted); opacity: .45; cursor: default; }
.seg button:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }"""

# The map panel: states filled from --f, labels inked with --i (set per data class), callout
# chips with their leader lines.
MAP_CSS = """figure { margin: 0; display: grid; gap: 14px; }
.map { width: 100%; height: auto; display: block; background: var(--map-bg); border: 1px solid [[map_edge]]; border-radius: 10px; }
.st { fill: var(--f); stroke: var(--fg); stroke-width: .6; stroke-linejoin: round; transition: fill .25s; }
.st:hover { opacity: .85; }
.lbl { fill: var(--i); font-family: var(--display); font-weight: 900; text-anchor: middle; dominant-baseline: central; pointer-events: none; font-variant-numeric: tabular-nums; }
.chip { fill: var(--f); }
.lead { stroke: var(--muted); stroke-width: .8; fill: none; }
.dot { fill: var(--muted); }
.ab { font-family: var(--display); font-weight: 700; font-size: [[abbr_px]]px; fill: var(--muted); letter-spacing: [[metrics.abbr_tracking_em]]em; dominant-baseline: central; }"""


@dataclass(frozen=True)
class Styles:
    """What every page shares, in one theme: its <head> lines and CSS, named as the page
    templates' format fields (a page passes them all: format ignores the ones it has no use for)."""
    head: str
    theme_css: str
    base_css: str
    highlight_css: str
    toggle_css: str
    map_css: str


@functools.lru_cache(maxsize=None)
def styles(theme=THEME):
    """The shared <head> lines and CSS in theme."""
    return Styles(head=head(theme), theme_css=theme_css(theme), base_css=css(BASE_CSS, theme),
                  highlight_css=css(HIGHLIGHT_CSS, theme), toggle_css=css(TOGGLE_CSS, theme),
                  map_css=css(MAP_CSS, theme))
