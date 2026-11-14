"""Render the state-rank map page (HTML) plus Markdown and CSV lists from saved seasons.

A saved season is one `ath rank --in all --ahead all --json` report with the day it was pulled
(see season_file). Used by rankmap.py; see README.md. Standard library only.
"""
import functools
import json
import re
import sys
from dataclasses import dataclass
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from report import (  # noqa: E402
    CALLOUTS, callout_spots, chips, esc, label_shape, label_spots, label_width, map_viewbox, num, outline_extent,
    map_viewbox_tight, pretty_date, styles, LABEL_DIGITS, THEME, WIDEST_CHIP,
    state_areas, state_map, us_map, write_csv,
    DEMO_PAGE, FOOTER_CSS, ROBOTS, example_footer, meet_kind, page_link,
)

ALL_HS = [9, 10, 11, 12]


# The saved-season format this build reads, and the `ath rank` JSON schema inside it.
SEASON_FORMAT = 2
ATH_RANK_SCHEMA = 1

# Rank buckets: (legend label, min rank, max rank, css class), each about twice as wide as the
# last; buckets() adds a theme's colors.
RANK_BUCKETS = [
    ("#1", 1, 1, "b1"),
    ("#2-4", 2, 4, "b2"),
    ("#5-10", 5, 10, "b5"),
    ("#11-20", 11, 20, "b11"),
    ("#21+", 21, 10**6, "b21"),
]


def buckets(theme=THEME):
    """The rank buckets with their colors from theme.ranks: (legend label, min rank, max rank, css
    class, fill, ink)."""
    if len(theme.ranks) != len(RANK_BUCKETS):
        raise ValueError(f"the theme's ranks has {len(theme.ranks)} colors; the rank buckets need {len(RANK_BUCKETS)}")
    return [b + (sw.fill, sw.ink) for b, sw in zip(RANK_BUCKETS, theme.ranks)]


SHOW_ALL = 6  # longer "ahead" lists collapse behind a disclosure
FIT_MIN = 20  # an Area whose fitted size for a two-digit tie (t12) is under this gets a pill
CALLOUT_GAP = 28  # a callout's distance from the State's edge at its height, in map units
CALLOUT_PAD = 36  # room between the farthest callout name and the panel's edge
BLANK_PX = 12  # the size of a label with no rank in it; nothing shows


@dataclass(frozen=True)
class Callouts:
    """A State map's callouts in a theme, in map units: an Area name's width per character, the
    least spacing of stacked callouts (a name, or a pill when it is taller), how far above and
    below a name's middle the State's edge must keep clear, and a callout's room for its pill (the
    widest rank) before its name."""
    name_em: float
    step: int
    clear: int
    slot: int


@functools.lru_cache(maxsize=None)
def callouts(theme=THEME):
    """Callouts for theme's name_px and chip_px (the numbers here are at 22px)."""
    ch = chips(theme)
    step, clear = (round(n / 22 * theme.name_px) for n in (30, 12))
    return Callouts(name_em=theme.metrics.name_char_em * theme.name_px, step=max(step, ch.step), clear=clear,
                    slot=ch.width(WIDEST_CHIP) + ch.gap)


# ---------------------------------------------------------------- data

class SeasonError(ValueError):
    """A season that cannot be built (or saved): the message says why."""


def season_file(rank, pulled):
    """What rankmap saves for a season: the `ath rank` report and the day it was pulled."""
    return {"format": SEASON_FORMAT, "pulled": pulled, "rank": rank}


def lacks_areas(saved):
    """Whether a saved season has none of its home State's Areas although the State has an Area
    file: saved before rankmap asked for them, or Athletic.net had none (a partial tree, an
    older season). Collecting it again reads ath's cache, so trying again costs little."""
    r = saved["rank"]
    if any(x["list"]["kind"] == "area" for x in r["results"]):
        return False
    home = home_state(r)
    return bool(home and state_areas(home))


def ordinal(n):
    """1st, 2nd, 3rd, 4th ... 11th, 12th, 13th ... 21st: a grade as text."""
    suffix = "th" if n % 100 in (11, 12, 13) else {1: "st", 2: "nd", 3: "rd"}.get(n % 10, "th")
    return f"{n}{suffix}"


def view_meta(grades):
    """(key, toggle label, phrase) for a grade group."""
    if grades == ALL_HS:
        return "all", "All HS", "all high schoolers"
    if len(grades) == 1:
        return str(grades[0]), ordinal(grades[0]), f"{ordinal(grades[0])} graders"
    names = [ordinal(g) for g in grades]
    phrase = " and ".join(names) if len(names) == 2 else ", ".join(names[:-1]) + " and " + names[-1]
    return f"{grades[0]}-{grades[-1]}", f"{names[0]}-{names[-1]}", phrase + " graders"


def mark_sort(mark):
    """A published standard as Athletic.net's sort key (`CONTEXT.md`, SortInt): a time (55.60, 2:11.70)
    in milliseconds; a field mark in feet-inches (5-06, 18-02.75) as 20_000_000 - inches x 1000."""
    if "-" in mark:
        feet, inches = mark.split("-")
        return round(20_000_000 - (int(feet) * 12 + float(inches)) * 1000)
    seconds = 0.0
    for part in mark.split(":"):
        seconds = seconds * 60 + float(part)
    return round(seconds * 1000)


def state_meet(areas, r, views, home_area=None):
    """A season's State Meet picture from the home State's Area file (`state_meet`), or None.

    Two kinds, by the file's shape. By places (California): each Area's places (by abbreviation),
    the at-large standard for this event and gender (when published for the season), whether the
    Athlete's mark meets it, and their All HS place in each Area, which is what counts (the State
    Meet is open to every grade). By standards (Maine, a `standards` key): see standards_meet."""
    meet = areas.get("state_meet") if areas else None
    if not meet:
        return None
    if meet_kind(meet) == "standards":
        return standards_meet(meet, areas, r, views, home_area)
    abbr = {a["name"]: a["abbr"] for a in areas["areas"]}
    standard = meet["at_large"].get(str(r["season"]), {}).get(r["gender"], {}).get(r["event"])
    all_hs = next((v for v in views if v["grades"] == ALL_HS), None)
    return {
        "kind": "places", "name": meet["name"], "short": meet["short"],
        "places": {abbr[name]: n for name, n in meet["places"].items() if name in abbr},
        "standard": standard,
        "meets": standard is not None and r["mark"]["sort_int"] <= mark_sort(standard),
        "all_hs": {s["abbr"]: (s["rank"], s["label"]) for s in all_hs["areas"]} if all_hs else {},
        "notes": meet.get("notes", []),
    }


def standards_meet(meet, areas, r, views, home_area):
    """A State Meet entered by standards (Maine's MPA Class meets): the Athlete's own Area (their
    school's, `home_area`) and its automatic and provisional standards for this season, event and
    gender, which of them the mark meets, and the Athlete's place on that Area's list in each grade
    group, the seed it suggests. None without a home Area or a published standard: each Area's
    meet is its own, so another Area's standards say nothing about the Athlete."""
    a = next((a for a in areas["areas"] if a["abbr"] == home_area), None)
    standard = a and meet["standards"].get(str(r["season"]), {}).get(a["name"], {}).get(
        r["gender"], {}).get(r["event"])
    if not standard:
        return None
    automatic, provisional = standard
    sort = r["mark"]["sort_int"]
    meets = ("automatic" if sort <= mark_sort(automatic) else
             "provisional" if sort <= mark_sort(provisional) else None)
    return {
        "kind": "standards", "short": meet["short"], "field": meet["field"],
        "new_england": meet.get("new_england"), "area_name": area_name(a, areas),
        "automatic": automatic, "provisional": provisional, "meets": meets,
        "seeds": {v["key"]: next(((s["rank"], s["label"]) for s in v["areas"] if s["abbr"] == home_area), None)
                  for v in views},
        "notes": meet.get("notes", []), "sources": meet.get("sources", []), "checked": meet.get("checked"),
    }


def standards_line(sm, pr):
    """What the Athlete's mark meets, for the page and the Markdown."""
    if sm["meets"] == "automatic":
        return f"{pr} meets the automatic standard."
    if sm["meets"] == "provisional":
        return (f"{pr} meets the provisional standard: in if the field of {sm['field']} has room for it "
                "after the automatic qualifiers.")
    return f"{pr} does not meet the provisional standard."


def seed_text(sm, seed, phrase, first, rank):
    """The seed a place on the Area's list suggests, against `phrase`; `rank` formats a rank label.
    No seed for a mark short of the provisional standard, or a place past the field."""
    area = sm["area_name"]
    if sm["meets"] is None:
        return f"{first} would not be in the {area} field."
    if seed is None:
        return f"No place for {first} on the {area} list against {phrase}."
    if seed[0] > sm["field"]:
        return f"Against {phrase}, {first} is {rank(seed[1])} on the {area} list, past the field of {sm['field']}."
    return f"Against {phrase}, {first} would be seeded about {rank(seed[1])} in {area}."


def source_label(url):
    """A source link's text: its site (mpa.cc, me.milesplit.com)."""
    host = re.sub(r"^https?://", "", url).split("/")[0]
    return host[4:] if host.startswith("www.") else host


def rank_text(label):
    """A rank label in plain text: #12, or T12 for a tie."""
    return label if label.startswith("T") else f"#{label}"


def bucket(rank):
    return next(b for b in RANK_BUCKETS if b[1] <= rank <= b[2])


def person(row):
    """A Neighborhood row as the page shows it."""
    return {"name": row["name"], "mark": row["mark"], "team": row["team"] or "",
            "grade": row["grade"], "url": row["url"]}


def grade_note(a):
    """`gr 10, `, or nothing when Athletic.net gives no grade."""
    return "" if a["grade"] is None else f"gr {a['grade']}, "


def check(saved, expect=None):
    """The `ath rank` report in a saved season, once it is whole: a mark, every list read, every
    grade group on the same State lists, and everyone ahead listed. Raises SeasonError otherwise,
    also for a file of another shape.

    expect: what the season must be, when the caller knows: {"athlete_id", "event", "season",
    "state_lists"} (the event as configured: the code or the full name)."""
    if not isinstance(saved, dict):
        raise SeasonError("is not a saved season")
    if "format" not in saved:
        # Files written before rankmap moved onto `ath` have no format; the browser collector's had
        # views.
        raise SeasonError("is from the retired browser collector (fetch.js)" if "views" in saved
                          else "is not a saved season (no format)")
    if saved["format"] != SEASON_FORMAT:
        raise SeasonError(f"saved season format {saved['format']}; this rankmap reads {SEASON_FORMAT}")
    try:
        return _whole(saved["rank"], expect)
    except (KeyError, TypeError, AttributeError) as e:
        raise SeasonError(f"is not a whole ath rank report ({type(e).__name__}: {e})") from None


def _same_event(configured, r):
    """Whether the configured event is the report's: its code (400m) or full name (400 Meters)."""
    names = {str(n).strip().lower() for n in (r["event"], r["event_name"]) if n}
    return str(configured).strip().lower() in names


def _whole(r, expect):
    if r.get("schema") != ATH_RANK_SCHEMA:
        raise SeasonError(f"ath rank JSON schema {r.get('schema')}; this rankmap reads {ATH_RANK_SCHEMA}")
    if r["status"] != "ok":
        raise SeasonError(r.get("note") or f"ath rank status {r['status']}")
    failed = [f"{x['list']['name']} {x['group']}" for x in r["results"] if x["status"] != "ok"]
    if failed:
        raise SeasonError(f"{len(failed)} list(s) not read: {', '.join(failed[:5])}"
                          + (" ..." if len(failed) > 5 else ""))
    cut = [x for x in r["results"] if x["place"]["neighborhood"]["skipped"]]
    if cut:
        raise SeasonError("ath rank left people ahead out of a Neighborhood; run it with --ahead all")
    other = [x["list"]["name"] for x in r["results"] if x["list"]["kind"] not in ("state", "area")]
    if other:
        raise SeasonError(f"has lists that are neither a State's nor an Area's: {', '.join(sorted(set(other)))}")
    groups = [list(g) for g in r["groups"]]
    stray = [x["grades"] for x in r["results"] if x["grades"] not in groups]
    if stray:
        raise SeasonError(f"has results for grades {stray[0]}, not one of its grade groups")
    for kind in ("state", "area"):
        per_group = [{x["list"]["name"] for x in r["results"] if x["grades"] == g and x["list"]["kind"] == kind}
                     for g in groups]
        if any(lists != per_group[0] for lists in per_group):
            raise SeasonError(f"does not have the same {kind.capitalize()} lists for every grade group")
    states = {x["list"]["state"] for x in r["results"] if x["list"]["kind"] == "state" and x["grades"] == groups[0]}
    # Every Area list must be one the home State's file names: the map, cards and State Meet are
    # drawn from the file, so an Area Athletic.net added would otherwise show in the table only.
    home = home_state(r)
    known = {a["name"].lower() for a in (state_areas(home) or {}).get("areas", [])} if home else set()
    unknown = sorted({x["list"]["name"] for x in r["results"] if x["list"]["kind"] == "area"
                      and x["list"]["state"] == home and x["list"]["name"].lower() not in known})
    if unknown:
        raise SeasonError(f"has {home} Area lists that areas/{home}.json does not name: {', '.join(unknown)}; "
                          "add them to the file")
    if expect:
        got = (r["athlete"]["id"], r["season"])
        if got != (expect["athlete_id"], expect["season"]) or not _same_event(expect["event"], r):
            raise SeasonError(f"is athlete {got[0]} {r['event']} {got[1]}, not athlete "
                              f"{expect['athlete_id']} {expect['event']} {expect['season']}")
        if len(states) != expect["state_lists"]:
            raise SeasonError(f"has {len(states)} of the {expect['state_lists']} State lists")
    return r


def area_name(a, areas=None):
    """What a page calls an Area: its display name, when Athletic.net's is ambiguous (its
    "Los Angeles" is the city's schools); for Areas that are not places (no `map` in the State's
    Area file: Maine's Classes), the label and Athletic.net's name ("Class A"); else
    Athletic.net's."""
    if a.get("display"):
        return a["display"]
    if areas is not None and not areas.get("map"):
        return f'{areas["label"]} {a["name"]}'
    return a["name"]


def home_state(r):
    """The Athlete's home State (code): the State of the list their mark is read from."""
    return next((x["list"]["state"] for x in r["results"]
                 if x["list"]["kind"] == "state" and x["list"]["id"] == r["mark"]["list"]["id"]), None)


def prepare(saved_seasons, areas_dir=None):
    """Rank rows per season and grade group from the saved seasons ({season: saved}).

    areas_dir: where the State Meet standards come from instead of areas/ (report.state_areas)."""
    seasons = {}
    for season in sorted(saved_seasons, key=int):
        saved = saved_seasons[season]
        r = check(saved)
        home = home_state(r)
        areas = state_areas(home, areas_dir) if home else None
        abbr = {a["name"].lower(): a["abbr"] for a in areas["areas"]} if areas else {}
        shown = {a["name"].lower(): area_name(a, areas) for a in areas["areas"]} if areas else {}
        own = r.get("home_area") or {}
        # Only the home State's Area for this season says which of the file's Areas is theirs.
        home_area = own.get("name", "").lower() if (own.get("state"), own.get("season")) == (home, r["season"]) else ""

        def row(x, key):
            p, n = x["place"], x["place"]["neighborhood"]
            return {
                "name": x["list"]["name"], "abbr": key, "url": x["url"], "rank": p["rank"], "label": p["label"],
                "ahead": [person(a) for a in n["ahead"]], "ties": [person(a) for a in n["tied"]],
                "bucket": bucket(p["rank"]),
            }

        views = []
        for grades in r["groups"]:
            key, label, phrase = view_meta(grades)
            mine = [x for x in r["results"] if x["grades"] == grades]
            rows = [row(x, x["list"]["state"]) for x in mine if x["list"]["kind"] == "state"]
            area_rows = [dict(row(x, abbr.get(x["list"]["name"].lower(), x["list"]["name"])),
                              name=shown.get(x["list"]["name"].lower(), x["list"]["name"]))
                         for x in mine if x["list"]["kind"] == "area"]
            for rs in (rows, area_rows):
                rs.sort(key=lambda s: (s["rank"], s["name"]))
            views.append({"key": key, "label": label, "phrase": phrase, "grades": grades, "rows": rows,
                          "areas": area_rows})
        seasons[str(season)] = {"pr": r["mark"]["mark"], "grade": r["mark"].get("grade"),
                                "pulled": saved["pulled"], "views": views,
                                "home": home, "area_file": areas, "short": lacks_areas(saved),
                                "home_area": abbr.get(home_area),
                                "state_meet": state_meet(areas, r, views, abbr.get(home_area))}
    return seasons


def grouped(rows):
    """Rows for the tables: every #1 state (ties included) merged into one alphabetical row first."""
    firsts = sorted((s for s in rows if s["rank"] == 1), key=lambda s: s["name"])
    rest = [s for s in rows if s["rank"] != 1]
    return firsts, rest


# ---------------------------------------------------------------- renderer


def rank_html(label):
    """A rank label for HTML: a tie's T as a small raised t (t12)."""
    if label.startswith("T"):
        return f'<span class="sup">t</span>{esc(label[1:])}'
    return esc(label)


TIE_SCALE = 0.6  # the tie's t, of the rank's size (tspan.sup, span.sup)


def tie_dy(metrics):
    """The dy of a tie's t and of the number after it (rank_svg), in each tspan's own em: the t
    (TIE_SCALE em) rises metrics.tie_rise_em of the text's em and the number comes back down by as
    much."""
    return num(-metrics.tie_rise_em / TIE_SCALE), num(metrics.tie_rise_em)


def rank_svg(label, metrics=None):
    """A rank label inside an SVG <text>: a tie's T as a small raised t, its top level with the
    digits' tops (so the label is no taller than its digits, as report.fit_label assumes). The
    text's central baseline already lifts the smaller t; it rises the rest of the way by tie_dy,
    from the display font's metrics (THEME's by default). rankTspans in the page script builds the
    same."""
    if label.startswith("T"):
        up, down = tie_dy(metrics or THEME.metrics)
        return f'<tspan class="sup" dy="{up}em">t</tspan><tspan dy="{down}em">{esc(label[1:])}</tspan>'
    return esc(label)


def spot(spots, name, label, metrics=None):
    """A label's [x, y, font size] in a shape, from the label spots (report.label_spots) fitted in
    metrics (THEME's by default). A label longer than the longest spotted shape (a rank of 10000 or
    worse) takes that shape's spot, its size scaled down to the extra width."""
    metrics = metrics or THEME.metrics
    shape = label_shape(label)
    longest = ("t" if shape.startswith("t") else "") + str(LABEL_DIGITS)
    try:
        if int(shape.lstrip("t")) <= LABEL_DIGITS:
            return spots[name][shape]
        x, y, size = spots[name][longest]
        k = label_width(longest, metrics) / label_width(shape, metrics)
        # Same middle: the anchor moves with the ink's drop at the smaller size.
        return [x, round(y + metrics.ink_drop_em * size * (1 - k), 1), round(size * k, 1)]
    except KeyError:
        raise SystemExit(f"the label spots have no {shape} spot for {name}: "
                         "run python3 -I tools/maps/label_spots.py") from None


def lead_target(entry, side, metrics=None):
    """Where a callout's lead meets the rank on its Area: just beside the number, on the side of
    the name, in the display font's metrics (THEME's by default). leadTarget in the page script
    does the same when the view changes."""
    metrics = metrics or THEME.metrics
    lab, size, x, y = entry[0], entry[2], entry[5], entry[6]
    half = label_width(label_shape(lab), metrics) * size / 2 + 4
    return (x - half if side == "west" else x + half), y + metrics.ink_drop_em * size


class Page:
    def __init__(self, meta, seasons, all_seasons, theme=THEME, example=False, spots_dir=None):
        self.meta = meta  # name, first, gender, event, event_name, athlete_id, url
        self.theme, self.chips, self.callouts = theme, chips(theme), callouts(theme)
        self.metrics = theme.metrics
        # Where each rank goes on the maps, fitted to the theme's display font (report.label_spots).
        self.spots = label_spots(theme, spots_dir)
        self.example = example  # an example page (see build)
        self.seasons = seasons
        self.all_seasons = all_seasons
        self.default_season = max(seasons, key=int)
        self.default_view = seasons[self.default_season]["views"][0]["key"]
        # The Areas map is the newest season's home State's, when it has an Area file; another
        # season shows its Areas on it only when its home State is the same (see area_rows).
        self.areas = seasons[self.default_season]["area_file"]
        # Areas that are places get a county map; others (Maine's Classes) a row of cards.
        self.mapped = bool(self.areas and self.areas.get("map"))
        # This State's Areas in the label spots (where each rank goes on its map).
        self.area_spots = self.spots["areas"].get(self.areas["state"], {}) if self.areas else {}
        self.state_name = self.areas and next(
            (r["name"] for sd in seasons.values() for r in sd["views"][0]["rows"] if r["abbr"] == self.areas["state"]),
            self.areas["state"])

    def link(self, text, url, cls=None):
        """A link on this page: to url, or on an example page to the demo page (report.page_link)."""
        return page_link(text, url, cls, self.example)

    def href(self, url):
        """Where a link to url goes on this page: url, or on an example page the demo page."""
        return DEMO_PAGE if self.example else url

    def body_prefix(self):
        """Whose Areas they are, before their name in page text ("MPA "), or nothing."""
        return f'{self.areas["body"]} ' if self.areas and self.areas.get("body") else ""

    def area_rows(self, season, v):
        """A grade group's Area rows for the Areas map: none when that season's home State is not
        the map's."""
        same = self.areas and self.seasons[season]["home"] == self.areas["state"]
        return v["areas"] if same else []

    def area_note(self, season):
        """Why a season shows no Areas on the map, or None when it shows some."""
        sd, st, labels = self.seasons[season], self.areas["state"], self.areas["labels"]
        if sd["home"] != st:
            return f"{self.meta['first']} was at a school in {sd['home']} in {season}, so {season} has no {st} {labels}."
        if any(v["areas"] for v in sd["views"]):
            return None
        if sd["short"]:
            return f"No {st} {labels} in the {season} data yet: rankmap collects them the next time it collects {season}."
        return f"There are no {st} {labels} lists for {season}."

    def attrs(self, season, view=None, map_=None):
        shown = (season == self.default_season and view in (None, self.default_view)
                 and map_ in (None, "us"))
        return (f' data-season="{season}"' + (f' data-view="{view}"' if view else "")
                + (f' data-map="{map_}"' if map_ else "") + ("" if shown else " hidden"))

    def athlete_li(self, a, multi, tie=False):
        gr = f' <span class="gr">gr {a["grade"]}</span>' if multi and a["grade"] is not None else ""
        t = " &middot; tied" if tie else ""
        cls = ' class="tie"' if tie else ""
        return (f'<li{cls}><span class="mk">{esc(a["mark"])}</span> '
                f'<span class="nm">{self.link(a["name"], a["url"])}</span>{gr} '
                f'<span class="tm">{esc(a["team"])}{t}</span></li>')

    def fits(self, name):
        """Whether an Area's rank goes on the map (else in its callout's pill): a two-digit tie
        fits there at FIT_MIN or bigger (tools/maps/label-spots.json)."""
        if not next(a for a in self.areas["areas"] if a["name"] == name)["counties"]:
            return False  # a city's schools, drawn over its county: always a pill
        return spot(self.area_spots, name, "T12", self.metrics)[2] >= FIT_MIN

    def noun(self, map_):
        """(singular, plural) for the lists a map shows: state(s), or the home State's Area word."""
        if map_ == "us":
            return "state", "states"
        return self.areas["label"].lower(), self.areas["labels"].lower()

    def table(self, season, v, map_):
        rows = v["rows"] if map_ == "us" else self.area_rows(season, v)
        first = esc(self.meta["first"])
        multi = len(v["grades"]) > 1
        firsts, rest = grouped(rows)
        # The season's home State, tagged in the states table as it is outlined on the map.
        home = self.seasons[season]["home"] if map_ == "us" else None
        tag = lambda s: ' <span class="htag">home</span>' if s["abbr"] == home else ""
        out = []
        if firsts:
            names = ", ".join(
                self.link(s["name"], s["url"]) + tag(s)
                + (f' <span class="tm">(tied with {self.link(s["ties"][0]["name"], s["ties"][0]["url"])})</span>'
                   if s["ties"] else "")
                for s in firsts)
            picks = "|".join(esc(s["abbr"]) for s in firsts)  # "|": names (an abbr fallback) have spaces
            out.append(
                f'<tr class="firsts" data-pick="{picks}"><td class="rk"><span class="pill b1">1</span></td>'
                f'<td class="sn" colspan="2"><div class="cnt">{len(firsts)} {self.noun(map_)[len(firsts) > 1]} where {first} '
                f'would be #1</div><div class="names">{names}</div></td></tr>')
        for s in rest:
            items = "".join(self.athlete_li(a, multi) for a in s["ahead"]) + \
                "".join(self.athlete_li(a, multi, True) for a in s["ties"])
            if len(s["ahead"]) + len(s["ties"]) <= SHOW_ALL:
                who = f'<ol class="who">{items}</ol>'
            else:
                who = (f'<details><summary>{len(s["ahead"])} faster'
                       f'{", " + str(len(s["ties"])) + " tied" if s["ties"] else ""}'
                       f' <span class="tm">&middot; fastest {esc(s["ahead"][0]["mark"])}</span></summary>'
                       f'<ol class="who">{items}</ol></details>')
            out.append(
                f'<tr data-pick="{esc(s["abbr"])}"><td class="rk"><span class="pill {s["bucket"][3]}">{rank_html(s["label"])}</span></td>'
                f'<td class="sn">{self.link(s["name"], s["url"])}{tag(s)}</td>'
                f'<td class="ah">{who}</td></tr>')
        return f'<tbody{self.attrs(season, v["key"], map_)}>' + "".join(out) + "</tbody>"

    def stats(self, season, v, map_):
        rows = v["rows"] if map_ == "us" else self.area_rows(season, v)
        worst = max(rows, key=lambda s: s["rank"])
        ones = sum(s["rank"] == 1 for s in rows)
        return (
            f'<ul class="stats"{self.attrs(season, v["key"], map_)}>'
            f'<li><b>{ones}</b><span>{self.noun(map_)[ones != 1]} at #1</span></li>'
            f'<li><b>{sum(s["rank"] <= 3 for s in rows)}</b><span>top 3 of {len(rows)}</span></li>'
            f'<li><b>{sum(s["rank"] <= 5 for s in rows)}</b><span>top 5</span></li>'
            f'<li><b>{"" if worst["ties"] else "#"}{rank_html(worst["label"])}</b><span>toughest: {esc(worst["name"])}</span></li></ul>')

    def map_data(self, rows_of, spot_of, note=lambda season, s: ""):
        """Per season and view, what the toggles apply to each list on a map:
        {abbr: [label, bucket class, font size, tooltip, Athletic.net list, x, y]}, x and y the
        label's spot for its shape (None for a callout's pill, which stays put); spot_of gives
        [x, y, font size]; note adds to a tooltip."""
        data = {}
        for season, sd in self.seasons.items():
            data[season] = {}
            for v in sd["views"]:
                d = {}
                for s in rows_of(season, v):
                    n = len(s["ahead"])
                    tip = (f'{s["name"]}: {"" if s["ties"] else "#"}{s["label"]} against {v["phrase"]} in {season}'
                           + (f" ({n} faster)" if n else " (nobody faster)"))
                    x, y, size = spot_of(s)
                    d[s["abbr"]] = [s["label"], s["bucket"][3], size, tip + note(season, s), self.href(s["url"]), x, y]
                data[season][v["key"]] = d
        return data

    def meet_note(self, season, s):
        """The State Meet part of an Area's tooltip and pick line (which already give the place)."""
        sm = self.seasons[season]["state_meet"]
        if not sm or sm["kind"] != "places" or s["abbr"] not in sm["places"]:
            return ""
        return f'; {sm["short"]}: its top {sm["places"][s["abbr"]]} go'

    def meet_panel(self, season):
        """The State Meet for a season, under the Areas map: the season's at-large mark, and each
        Area's places against the Athlete's place there in the grade group picked (one list per
        group; 9th-11th, this season's field without its seniors, is a look at next season's)."""
        sm, sd = self.seasons[season]["state_meet"], self.seasons[season]
        if not sm or sd["home"] != self.areas["state"]:
            return ""
        return (self.standards_panel if sm["kind"] == "standards" else self.places_panel)(season, sm, sd)

    def places_panel(self, season, sm, sd):
        """The State Meet by places (California): see meet_panel."""
        first, by_abbr = esc(self.meta["first"]), {a["abbr"]: a for a in self.areas["areas"]}
        lists = []
        for v in sd["views"]:
            row = {s["abbr"]: s for s in self.area_rows(season, v)}
            items = []
            for abbr, n in sorted(sm["places"].items(), key=lambda kv: area_name(by_abbr[kv[0]], self.areas)):
                s = row.get(abbr)
                inside = s is not None and s["rank"] <= n
                where = f'{first} {"" if s["ties"] else "#"}{rank_html(s["label"])}' if s else "no place"
                items.append(f'<li class="{"in" if inside else "out"}" data-pick="{esc(abbr)}"><b>{esc(area_name(by_abbr[abbr], self.areas))}</b>'
                             f' <span>top {n} go &middot; {where}</span></li>')
            lists.append(f'<ul class="smlist"{self.attrs(season, v["key"])}>{"".join(items)}</ul>')
        return (f'<section class="smeet" data-season="{season}" data-map="areas" hidden>'
                f'<h3>{esc(sm["name"])}, {season}</h3>'
                + (f'<p>The at-large mark for {season} was <b>{esc(sm["standard"])}</b></p>' if sm["standard"] else "")
                + "".join(lists) +
                f'<p class="mapnote">An estimate from the season lists against the grades picked (9th-11th is '
                f'roughly next season&rsquo;s field): the {esc(self.noun("areas")[1])} pick their qualifiers by place '
                f'at their own State qualifying meets.</p>'
                + self.about_standards(sm["notes"]) +
                f'</section>')

    def mark_ch(self):
        """The mark column's width in ch: the page's longest mark ahead of or tied with the Athlete
        (tabular digits are 1ch; a colon and a point are narrower), so names line up after it."""
        return max((len(a["mark"]) for sd in self.seasons.values() for v in sd["views"]
                    for s in v["rows"] + v["areas"] for a in s["ahead"] + s["ties"]), default=5)

    @staticmethod
    def about_standards(notes, more=""):
        """The State Meet's notes on its standards, folded away, or nothing."""
        items = "".join(f"<li>{esc(n)}</li>" for n in notes) + more
        return (f'<details class="smnotes"><summary>About these standards</summary><ul>{items}</ul></details>'
                if items else "")

    def standards_panel(self, season, sm, sd):
        """The State Meet of the Athlete's own Area, entered by standards (Maine's Class meets):
        the standards and which the season best meets, then the seed the Athlete's place on that
        Area's list suggests in the grade group picked (9th-11th is roughly next season's field),
        and the file's notes on where the standards come from."""
        first = esc(self.meta["first"])
        bold = lambda label: f'<b>{"" if label.startswith("T") else "#"}{rank_html(label)}</b>'
        seeds = "".join(
            f'<p class="smseed"{self.attrs(season, v["key"])}>'
            f'{seed_text(sm, sm["seeds"].get(v["key"]), esc(v["phrase"]), first, bold)}</p>'
            for v in sd["views"])
        checked = (f'<li>Checked {esc(pretty_date(sm["checked"]))}: '
                   + ", ".join(self.link(source_label(u), u) for u in sm["sources"]) + "</li>"
                   if sm["checked"] else "")
        return (f'<section class="smeet" data-season="{season}" data-map="areas" hidden>'
                f'<h3>{esc(self.body_prefix())}{esc(sm["area_name"])} {esc(sm["short"])}, {season}</h3>'
                f'<p>Automatic <b>{esc(sm["automatic"])}</b> &middot; provisional <b>{esc(sm["provisional"])}</b>. '
                f'{esc(standards_line(sm, sd["pr"]))}</p>'
                + seeds +
                f'<p class="mapnote">An estimate from the season list against the grades picked: everyone at the '
                f'automatic standard is in, and the field is filled up to {sm["field"]} from the provisional one. '
                f'Only regular-season and league championship marks count, so a season best from a later meet may '
                f'not.' + (f' {esc(sm["new_england"])}' if sm.get("new_england") else "") + '</p>'
                + self.about_standards(sm["notes"], checked)
                + '</section>')

    def us_svg(self, start, places, shapes):
        """The US map in the default season and view: each state filled with its rank's color and
        its number on it (a callout's in a chip at the map's edge), and the season's home State
        outlined over its neighbors' borders, its callout too. The page script moves the outline
        when a season's home State is another (home)."""
        home = self.seasons[self.default_season]["home"]
        svg_shapes, svg_labels, svg_callouts = [], [], []
        outline = '<path class="home" hidden/>'  # a season without a home State on the map
        for abbr, d in shapes:
            if abbr == home:
                outline = f'<path class="home" d="{d}"/>'
            lab, cls, size, tip, _, x, y = start[abbr]
            svg_shapes.append(f'<path class="st {cls}" data-abbr="{abbr}" d="{d}"><title>{esc(tip)}</title></path>')
            if abbr not in CALLOUTS:
                svg_labels.append(
                    f'<text class="lbl {cls}" data-abbr="{abbr}" x="{x:.1f}" y="{y:.1f}" '
                    f'style="font-size:{size}px">{rank_svg(lab, self.metrics)}</text>')
        ch = self.chips
        for abbr, cx, cy in callout_spots(places, self.theme):
            x, y, _ = places[abbr]
            lab, cls = start[abbr][:2]
            w = ch.width(lab)
            svg_callouts.append(
                f'<g class="co{" home" if abbr == home else ""}" data-abbr="{abbr}">'
                f'<path class="lead" d="M{x:.1f},{y:.1f}L{cx - ch.left},{cy}"/>'
                f'<circle class="dot" cx="{x:.1f}" cy="{y:.1f}" r="1.8"/>'
                f'<rect class="chip {cls}" data-abbr="{abbr}" data-cx="{cx}" x="{cx - ch.left}" y="{cy - ch.half}" width="{w}" height="{2 * ch.half}" rx="4"/>'
                f'<text class="lbl {cls}" data-abbr="{abbr}" x="{cx - ch.left + w / 2}" y="{cy}" style="font-size:{self.theme.chip_px}px">{rank_svg(lab, self.metrics)}</text>'
                f'<text class="ab" x="{cx - ch.left + w + ch.gap}" y="{cy + 1}">{abbr}</text></g>')
        # A phone fits the map to the States alone (data-tight), its callouts listed under it (us_calls).
        wide = map_viewbox(self.theme)
        return (f'<svg class="map" id="usmap" data-map="us" viewBox="{wide}" data-wide="{wide}" '
                f'data-tight="{map_viewbox_tight()}" role="img" '
                f'aria-label="US map with {esc(self.meta["name"])}\'s rank in each state">'
                f'<g>{"".join(svg_shapes)}</g><g>{outline}</g><g>{"".join(svg_labels)}</g>'
                f'<g>{"".join(svg_callouts)}</g></svg>')

    def calls_col(self):
        """The least width of a column of us_calls, in px: a chip for a three-character rank (T12,
        123), the gap and a State's abbreviation, measured as the map's callout column is."""
        m = self.metrics
        abbr = 2 * (m.abbr_char_em + m.abbr_tracking_em) * self.theme.abbr_px
        return round(self.chips.width("T12") + self.chips.gap + abbr)

    def us_calls(self, start, places):
        """The US map's callouts as a list under it, for a phone, where the whole map is too small
        for its chips to read: each callout State's rank in a chip of its rank's color at chip_px,
        its abbreviation beside it, in the callouts' order, the home State's chip ringed. Hidden at
        wider widths (the page's CSS); the page script keeps it in step with the map."""
        home = self.seasons[self.default_season]["home"]
        items = []
        for abbr, _, _ in callout_spots(places, self.theme):
            lab, cls, _, tip = start[abbr][:4]
            mark = ' class="home"' if abbr == home else ""
            items.append(f'<li{mark} data-abbr="{abbr}" title="{esc(tip)}"><span class="cchip {cls}">'
                         f'{rank_html(lab)}</span><span class="cab">{abbr}</span></li>')
        if not items:
            return ""  # no callout State on the map
        return (f'\n    <ul class="calls" data-map="us" aria-label="{esc(self.meta["name"])}\'s rank in the states '
                f'too small to label on the map">{"".join(items)}</ul>')

    def area_svg(self, start, smap):
        """The home State's county map, colored by Area, with a rank in each roomy Area, district
        Areas (a city's schools) drawn over their county, and every Area named in a callout off
        the State's west or east edge, with its rank in a pill when it has no room for it."""
        by_name = {a["name"]: a for a in self.areas["areas"]}
        empty = ["", "none", BLANK_PX, "", "", None, None]
        counties = "".join(
            f'<path class="ct {start.get(by_name[area]["abbr"], empty)[1]}" data-area="{by_name[area]["abbr"]}" '
            f'd="{d}"><title>{esc(county)} County: {esc(area_name(by_name[area]))}</title></path>'
            for county, area, d in smap["counties"])
        districts = "".join(
            f'<path class="dist {start.get(by_name[name]["abbr"], empty)[1]}" data-area="{by_name[name]["abbr"]}" '
            f'd="{d}"><title>{esc(area_name(by_name[name]))}: {esc(by_name[name].get("note", ""))}</title></path>'
            for name, d in smap["districts"].items())
        labels, fits = [], set()
        for a in self.areas["areas"]:
            if self.fits(a["name"]):
                fits.add(a["abbr"])
                lab, cls, size, _, _, x, y = start.get(a["abbr"], empty)
                if x is None:  # no rank in this view: where a one-digit rank would go
                    x, y, _ = self.area_spots[a["name"]]["1"]
                labels.append(f'<text class="albl {cls}" data-area="{a["abbr"]}" x="{x:.1f}" y="{y:.1f}" '
                              f'style="font-size:{size}px">{rank_svg(lab, self.metrics)}</text>')
        _, _, width, height = (float(n) for n in smap["viewBox"].split())
        sides = {side: sorted((a for a in self.areas["areas"] if smap["sides"].get(a["name"]) == side),
                              key=lambda a: smap["spots"][a["name"]][1])
                 for side in ("west", "east")}
        ch, co = self.chips, self.callouts
        callouts, xs = [], [0.0, width]
        for side in ("west", "east"):
            prev = None
            for a in sides[side]:
                x, y = smap["spots"][a["name"]]
                # Stacked down the side in north-to-south order, at least a name apart.
                cy = y if prev is None else max(y, prev + co.step)
                prev = cy
                abbr, name = a["abbr"], esc(area_name(a))
                lab, cls = start.get(abbr, empty)[:2]
                w = ch.width(lab)
                pill = abbr not in fits  # a rank that fits is on the map: the callout only names it
                if not pill:  # the middle of where a one-digit rank goes, for a view without one
                    x, y, size1 = self.area_spots[a["name"]]["1"]
                    y += self.metrics.ink_drop_em * size1
                sx, sy = x, y
                e = start.get(abbr)
                if not pill and e and e[0] and e[5] is not None:
                    x, y = lead_target(e, side, self.metrics)
                # Each callout follows the State's edge at its own height, CALLOUT_GAP off it (the edge
                # over a name's height, so a bulge cannot touch it): its pill (if any) nearest the
                # State, room for the widest pill, then its name.
                west_x, east_x = outline_extent(smap["outline"], cy - co.clear, cy + co.clear) or (x, x)
                text_w = len(area_name(a)) * co.name_em
                if side == "west":
                    x0 = west_x - CALLOUT_GAP - co.slot  # the pill's left edge; it grows toward the State
                    name_x, anchor = (x0 - ch.gap if pill else west_x - CALLOUT_GAP), "end"
                    lead_end = x0 + w + ch.lead if pill else name_x + 4
                    xs.append(name_x - text_w)
                else:
                    x0 = east_x + CALLOUT_GAP
                    name_x, anchor = (x0 + co.slot if pill else x0), "start"
                    lead_end = x0 - ch.lead if pill else name_x - 4
                    xs.append(name_x + text_w)
                chip = (f'<rect class="achip {cls}" data-area="{abbr}" x="{x0:.1f}" y="{cy - ch.half:.1f}" width="{w}" '
                        f'height="{2 * ch.half}" rx="4"/>'
                        f'<text class="albl {cls}" data-area="{abbr}" data-chip="1" x="{x0 + w / 2:.1f}" y="{cy:.1f}" '
                        f'style="font-size:{self.theme.chip_px}px">{rank_svg(lab, self.metrics)}</text>') if pill else ""
                callouts.append(
                    f'<g class="co" data-area="{abbr}" data-side="{side}" data-x0="{x0:.1f}" data-x="{x:.1f}" '
                    f'data-y="{y:.1f}" data-cy="{cy:.1f}" data-end="{lead_end:.1f}" data-sx="{sx:.1f}" '
                    f'data-sy="{sy:.1f}">'
                    f'<path class="lead" d="M{x:.1f},{y:.1f}L{lead_end:.1f},{cy:.1f}"/>'
                    f'<circle class="dot" cx="{x:.1f}" cy="{y:.1f}" r="1.8"/>{chip}'
                    f'<text class="ab" x="{name_x:.1f}" y="{cy + 1:.1f}" text-anchor="{anchor}">{name}</text></g>')
        # The panel reaches the farthest name on each side, with room to spare.
        left, right = min(xs) - CALLOUT_PAD, max(xs) + CALLOUT_PAD
        view = f"{left:.0f} 0 {right - left:.0f} {height:.0f}"
        state = esc(self.areas["state"])
        # A phone fits the map to the State alone (data-tight), its callouts hidden (see the script).
        return (f'<svg class="map amap" id="areamap" data-map="areas" hidden viewBox="{view}" '
                f'data-wide="{view}" data-tight="{smap["viewBox"]}" role="img" '
                f'aria-label="{state} map with {esc(self.meta["name"])}\'s rank in each {esc(self.noun("areas")[0])}">'
                f'<g>{counties}</g><path class="cl" d="{smap["lines"]}"/><path class="bd" d="{smap["borders"]}"/>'
                f'<g>{districts}</g><g>{"".join(labels)}</g><g>{"".join(callouts)}</g></svg>')

    def area_cards(self, start):
        """The home State's Areas as cards, for Areas that are not places (Maine's Classes): each
        with its rank in dark numbers on its bucket's fill (a card's number is its whole message, so
        not the map's quiet inks), the Athlete's own marked. The page script recolors them
        as the season and grade group change, as it does the map."""
        empty = ["", "none", BLANK_PX, "", "", None, None]
        own = f"{esc(self.meta['first'])}&rsquo;s {esc(self.noun('areas')[0])}"
        cards = []
        for a in self.areas["areas"]:
            abbr = a["abbr"]
            lab, cls, _, tip = start.get(abbr, empty)[:4]
            tag = "".join(f'<span class="ctag" data-season="{s}"{"" if s == self.default_season else " hidden"}>{own}</span>'
                          for s, sd in self.seasons.items() if sd["home_area"] == abbr)
            cards.append(
                f'<button type="button" class="acard {cls}" data-area="{esc(abbr)}" aria-pressed="false" title="{esc(tip)}">'
                f'<span class="cname">{esc(area_name(a, self.areas))}</span>'
                f'<span class="crank">{rank_html(lab)}</span>{tag}</button>')
        state = esc(self.areas["state"])
        return (f'<div class="acards" id="areacards" data-map="areas" hidden role="group" '
                f'aria-label="{esc(self.meta["name"])}\'s rank in each {state} {esc(self.noun("areas")[0])}">'
                f'{"".join(cards)}</div>')

    def html(self):
        meta, seasons = self.meta, self.seasons
        abbr_of = {s["name"]: s["abbr"] for sd in seasons.values() for s in sd["views"][0]["rows"]}
        places, shapes = us_map(abbr_of)
        us = self.map_data(
            lambda season, v: v["rows"],
            lambda s: [None, None, self.theme.chip_px] if s["abbr"] in CALLOUTS
            else spot(self.spots["us"], s["name"], s["label"], self.metrics),
            lambda season, s: f"; {meta['first']}'s home state" if s["abbr"] == seasons[season]["home"] else "")
        us_start = us[self.default_season][self.default_view]
        us_svg = self.us_svg(us_start, places, shapes) + self.us_calls(us_start, places)

        maps = [("us", "US states")]
        areas_data, area_figure, area_notes, map_lede = {}, "", "", ""
        if self.areas:
            name_of = {a["abbr"]: a["name"] for a in self.areas["areas"]}
            areas_data = self.map_data(
                self.area_rows,
                lambda s: spot(self.area_spots, name_of[s["abbr"]], s["label"], self.metrics)
                if self.mapped and self.fits(name_of[s["abbr"]]) else [None, None, self.theme.chip_px],
                self.meet_note)
            start = areas_data[self.default_season][self.default_view]
            st, labels = self.areas["state"], self.areas["labels"]
            maps.append(("areas", f"{st} {labels}"))
            # The pick line goes under the map and shows only once a Section is picked, so it takes
            # no room until then and never moves the map under the pointer.
            area_figure = (
                (self.area_svg(start, state_map(self.areas)) if self.mapped else self.area_cards(start))
                + f'<p class="pickline" data-map="areas" hidden aria-live="polite"></p>'
                f'<p class="mapnote" data-map="areas" hidden>{esc(self.areas["note"])}</p>')
            area_notes = "".join(
                f'<p class="nodata" data-season="{s}" data-map="areas" hidden>{esc(self.area_note(s))}</p>'
                for s in seasons if self.area_note(s))
            map_lede = f'<span data-map="areas" hidden>{esc(self.body_prefix())}{esc(self.noun("areas")[0])}</span>'

        def seg(name, label, choices):
            return (f'<div class="seg {name}" role="group" aria-label="{label}">' + "".join(
                f'<button type="button" data-{name[:-1]}="{key}" aria-pressed="{str(i == 0).lower()}">{esc(text)}</button>'
                for i, (key, text) in enumerate(choices)) + "</div>")

        season_buttons = "".join(
            f'<button type="button" id="s-{s}" data-season="{s}" aria-pressed="{str(s == self.default_season).lower()}">{s}</button>'
            if s in seasons else
            f'<button type="button" id="s-{s}" data-season="{s}" disabled title="No {s} data yet">{s}</button>'
            for s in self.all_seasons)
        view_groups = "".join(
            f'<div class="seg views" role="group" aria-label="Rank against"{self.attrs(s)}>' + "".join(
                f'<button type="button" id="v-{s}-{v["key"]}" data-view="{v["key"]}" '
                f'aria-pressed="{str(s == self.default_season and v["key"] == self.default_view).lower()}">{v["label"]}</button>'
                for v in sd["views"]) + "</div>"
            for s, sd in seasons.items())
        per_season = lambda f: "".join(f'<span{self.attrs(s)}>{f(s, sd)}</span>' for s, sd in seasons.items())
        against = "".join(f'<span{self.attrs(s, v["key"])}>{v["phrase"]}</span>'
                          for s, sd in seasons.items() for v in sd["views"])
        # The home State's key: the outline on a fill of no rank, named per season.
        home_key = "".join(
            f'<li class="hk" data-season="{s}" data-map="us"{"" if s == self.default_season else " hidden"}>'
            f'<span class="sw hsw"></span><span class="lk">{esc(sd["home"])}</span>'
            f'<span class="lc">&middot; home state</span></li>'
            for s, sd in seasons.items() if sd["home"])
        legend = '<ul class="legend"><li class="grp">Rank</li>' + "".join(
            f'<li data-b="{b[3]}"><span class="sw {b[3]}"></span><span class="lk">{b[0]}</span>'
            f'<span class="lc"></span></li>' for b in RANK_BUCKETS) + home_key + "</ul>"
        pulled = "".join(
            f'<li>{s} season data pulled {pretty_date(sd["pulled"])}</li>'
            for s, sd in sorted(seasons.items(), key=lambda kv: -int(kv[0])))
        map_ids = [m for m, _ in maps]
        stats = "".join(self.stats(s, v, m) for s, sd in seasons.items() for v in sd["views"]
                        for m in map_ids if m == "us" or (self.mapped and self.area_rows(s, v)))
        tables = "".join(self.table(s, v, m) for s, sd in seasons.items() for v in sd["views"]
                         for m in map_ids if m == "us" or self.area_rows(s, v))
        headings = "".join(
            f'<h2 data-map="{m}"{"" if m == "us" else " hidden"}>{esc(self.noun(m)[0].capitalize())} by '
            f'{esc(self.noun(m)[0])}</h2>' for m in map_ids)

        return TEMPLATE.format(
            robots="" if self.example else ROBOTS,
            example_css="\n" + FOOTER_CSS if self.example else "",
            footer="\n  " + example_footer() if self.example else "",
            links=("Names link to a page about the links: an example has no real lists or profiles."
                   if self.example else "Names link to the list used, and athlete names to their profiles."),
            mark_ch=self.mark_ch(),
            pick_new_tab="" if self.example else '\n  link.target = "_blank";\n  link.rel = "noopener";',
            title=esc(f"{meta['first']}'s {meta['event']} State Ranks"),
            **vars(styles(self.theme)), theme=self.theme,
            athlete=self.link(meta["name"], meta["url"]),
            first=esc(meta["first"]),
            # A race reads best as its code (400m, 100mH); a field event code (LJ, Shot) by its name.
            event_short=esc(meta["event"] if re.fullmatch(r"\d+m(H)?", meta["event"]) else meta["event_name"]),
            event_name=esc(meta["event_name"]),
            pr=per_season(lambda s, sd: esc(sd["pr"])),
            maps=seg("maps", "Map", maps) if len(maps) > 1 else "",
            season_buttons=season_buttons, view_groups=view_groups, against=against,
            map_lede=map_lede, stats=stats, headings=headings, tables=tables, legend=legend, pulled=pulled,
            bucket_css="\n".join(f".{b[3]} {{ --f: {b[4]}; --i: {b[5]}; }}" for b in buckets(self.theme)),
            us_svg=us_svg, area_figure=area_figure, area_notes=area_notes,
            meet_panels="".join(self.meet_panel(s) for s in seasons) if self.areas else "",
            name_head="".join(f'<span data-map="{m}"{"" if m == "us" else " hidden"}>{esc(self.noun(m)[0].capitalize())}</span>'
                              for m in map_ids),
            coverage=("State lists cover 50 states plus Washington, DC."
                      + (f" {esc(self.areas['state'])} {esc(self.areas['labels'])} are the "
                         f"{esc(self.areas.get('body', ''))} {esc(self.noun('areas')[1])} of {esc(self.state_name)}; "
                         f"a school&rsquo;s {esc(self.noun('areas')[0])} decides whose list "
                         f"its athletes are on." if self.areas else "")),
            area_noun=esc(self.noun("areas")[0]) if self.areas else "",
            area_nouns=esc(self.noun("areas")[1]) if self.areas else "",
            data=json.dumps(us, separators=(",", ":")), areas=json.dumps(areas_data, separators=(",", ":")),
            home=json.dumps({s: sd["home"] for s, sd in seasons.items()}, separators=(",", ":")),
            label_em=json.dumps([self.metrics.digit_em, self.metrics.tie_em, self.metrics.bearing_em,
                                 self.metrics.ink_drop_em]),
            tie_dy=tie_dy(self.metrics),
            chip=self.chips, chip_height=2 * self.chips.half, chip_inset=num(self.chips.pad / 2),
            calls_col=self.calls_col(), blank_px=BLANK_PX,
            default_season=self.default_season, default_view=self.default_view)

    def csv(self, path):
        rows = []
        for season, sd in self.seasons.items():
            for v in sd["views"]:
                kind = sd["area_file"]["label"] if sd["area_file"] else "Area"
                for kind, rs in (("State", v["rows"]), (kind, v["areas"])):
                    for s in rs:
                        # An example has no real profiles or lists to link to: it leaves them out.
                        who = "; ".join(f"{a['name']} {a['mark']} ({grade_note(a)}{a['team']}"
                                        + ("" if self.example else f", {a['url']}") + ")"
                                        for a in s["ahead"] + s["ties"])
                        rows.append([season, sd["pr"], v["phrase"], kind, s["label"], s["name"], s["abbr"],
                                     len(s["ahead"]), len(s["ties"]), who, "" if self.example else s["url"]])
        write_csv(path, ["Season", "Mark", "Against", "List", "Rank", "Name", "Abbr", "Faster", "Tied",
                         "Athletes ahead (mark, grade, team, profile)", "Athletic.net list"], rows)

    def md_table(self, rows, noun, home=None):
        firsts, rest = grouped(rows)
        mine = lambda s: " (home)" if s["abbr"] == home else ""  # the season's home State
        lines = [f"- #1 in {len(firsts)} of {len(rows)} {noun}",
                 f"- Top 3 in {sum(s['rank'] <= 3 for s in rows)}, top 5 in {sum(s['rank'] <= 5 for s in rows)}",
                 "", "| Rank | Name | Faster |", "| ---: | --- | --- |"]
        if firsts:
            names = ", ".join(
                s["name"] + mine(s) + (f" (tied with {s['ties'][0]['name']})" if s["ties"] else "") for s in firsts)
            lines.append(f"| 1 | {names} | - |")
        for s in rest:
            if len(s["ahead"]) + len(s["ties"]) <= SHOW_ALL:
                parts = [f"{a['name']} {a['mark']}" for a in s["ahead"]]
                if s["ties"]:
                    parts.append("tied: " + ", ".join(f"{a['name']} {a['mark']}" for a in s["ties"]))
                who = ", ".join(parts)
            else:
                who = f"{len(s['ahead'])} faster, led by {s['ahead'][0]['name']} {s['ahead'][0]['mark']}"
            lines.append(f"| {s['label']} | {s['name']}{mine(s)} | {who} |")
        return lines

    def meet_lines(self, sd):
        """A season's State Meet in the Markdown (the seed against all high schoolers), or none."""
        sm = sd["state_meet"]
        if not sm or sd["home"] != (self.areas or {}).get("state"):
            return []
        first = self.meta["first"]
        if sm["kind"] == "standards":
            lines = ["", f"### {self.body_prefix()}{sm['area_name']} {sm['short']}", "",
                     f"- Automatic {sm['automatic']}, provisional {sm['provisional']}: {standards_line(sm, sd['pr'])}",
                     "- " + seed_text(sm, sm["seeds"].get(view_meta(ALL_HS)[0]), "all high schoolers", first, rank_text)]
            return lines + ([f"- {sm['new_england']}"] if sm.get("new_england") else [])
        by_abbr = {a["abbr"]: a for a in sd["area_file"]["areas"]}
        lines = ["", f"### {sm['name']}", ""]
        if sm["standard"]:
            lines.append(f"- At-large mark {sm['standard']}: {sd['pr']} {'meets' if sm['meets'] else 'does not meet'} it")
        for abbr, n in sorted(sm["places"].items(), key=lambda kv: area_name(by_abbr[kv[0]], self.areas)):
            place = sm["all_hs"].get(abbr)
            lines.append(f"- {area_name(by_abbr[abbr], self.areas)}: top {n} go, {first} "
                         f"{rank_text(place[1]) if place else '#-'} All HS")
        return lines

    def markdown(self):
        meta = self.meta
        lines = [
            f"# {meta['name']} - {meta['event']} rankings",
            "",
            f"Where {meta['first']}'s season best would rank on each state's outdoor {meta['event_name']} list (Athletic.net, FAT, official marks), against several grade groups"
            + (f", and on each {self.noun('areas')[0]} of {self.state_name}." if self.areas else "."),
            "Rank is 1 plus the number of athletes in that group with a faster mark.",
            "An athlete with the same mark shares the rank (shown as T).",
        ]
        for season, sd in sorted(self.seasons.items(), key=lambda kv: -int(kv[0])):
            lines += ["", f"## {season} season - {sd['pr']} (data pulled {pretty_date(sd['pulled'])})"]
            for v in sd["views"]:
                lines += ["", f"### Against {v['phrase']}", ""] + self.md_table(v["rows"], "(50 states + DC)", sd["home"])
                if v["areas"]:
                    home = next((r["name"] for r in v["rows"] if r["abbr"] == sd["home"]), sd["home"])
                    labels = sd["area_file"]["labels"]
                    lines += ["", f"#### {home} {labels}", ""]
                    lines += self.md_table(v["areas"], labels.lower())
            lines += self.meet_lines(sd)
        return "\n".join(lines) + "\n"


def slug(meta):
    """Output file name: Athletic.net athlete id plus event code, e.g. <athlete_id>-400m."""
    return re.sub(r"[^A-Za-z0-9]+", "-", f"{meta['athlete_id']}-{meta['event']}").strip("-")


def build(saved_seasons, all_seasons, out_dir, theme=THEME, example=False, areas_dir=None, spots_dir=None):
    """Write <slug>.html/.md/.csv into out_dir, in theme. saved_seasons: {season: saved season}.
    spots_dir: where label spots fitted to a theme's own display font are kept (report.label_spots).

    example: an example page, built from anonymized data (ADR 0010): every link goes to the demo
    page beside it (report.DEMO_PAGE), in the same tab, the CSV has no links, search engines may
    index it, and a footer says what it is. An example needs areas_dir, so its State Meet
    standards are its own (report.state_areas), never the published ones.

    Returns (html path, the Page: its meta and prepared seasons)."""
    if example and areas_dir is None:
        raise ValueError("an example page needs its own areas_dir (its State Meet standards)")
    latest = check(saved_seasons[max(saved_seasons, key=int)])
    a = latest["athlete"]
    meta = {
        "name": a["name"], "first": a["first_name"], "gender": latest["gender"], "url": a["url"],
        "event": latest["event"], "event_name": latest["event_name"] or latest["event"], "athlete_id": a["id"],
    }
    page = Page(meta, prepare(saved_seasons, areas_dir), [str(s) for s in all_seasons], theme, example, spots_dir)
    out_dir.mkdir(parents=True, exist_ok=True)
    base = out_dir / slug(meta)
    base.with_suffix(".html").write_text(page.html())
    base.with_suffix(".md").write_text(page.markdown())
    page.csv(base.with_suffix(".csv"))
    return base.with_suffix(".html"), page


TEMPLATE = """<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover">
{robots}<title>{title}</title>
{head}
<style>
/* Layout: results-board header with season and grade-group toggles, the map as the hero, legend strip, then the ranked list.
   Always light: the page ignores the viewer's dark-mode setting. */
{theme_css}
/* Rank colors are data colors. --f fill, --i ink (a shade of the fill). */
{bucket_css}
{base_css}{example_css}
header {{ display: grid; gap: 10px; }}
h1 {{ font-family: var(--display); font-weight: 900; font-size: clamp(28px, 4.6vw, 46px); line-height: 1; margin: 0; text-wrap: balance; letter-spacing: -.01em; }}
h1 a {{ color: inherit; text-decoration: none; }}
h1 a:hover, h1 a:focus-visible {{ text-decoration: underline; text-decoration-thickness: 3px; text-underline-offset: 6px; outline: none; }}
{highlight_css}
.lede {{ margin: 0; color: var(--muted); max-width: 64ch; }}
.lede b {{ color: var(--fg); font-weight: 600; }}
{toggle_css}
.stats {{ display: flex; flex-wrap: wrap; gap: 10px 32px; margin: 2px 0 0; padding: 0; list-style: none; }}
.stats li {{ display: grid; }}
.stats b {{ font-family: var(--display); font-size: 34px; font-weight: 900; line-height: 1; font-variant-numeric: tabular-nums; }}
.stats span {{ font-size: 13px; color: var(--muted); }}
{map_css}
.legends {{ display: grid; gap: 6px; }}
.legend {{ display: flex; flex-wrap: wrap; gap: 8px 18px; list-style: none; margin: 0; padding: 0; font-size: 13px; color: var(--muted); }}
.legend li {{ display: flex; align-items: center; gap: 7px; }}
.legend .grp {{ font-family: var(--display); font-weight: 600; text-transform: uppercase; letter-spacing: .08em; font-size: 12px; width: 5.5em; }}
.sw {{ width: 18px; height: 12px; border-radius: 2px; display: inline-block; background: var(--f); box-shadow: inset 0 0 0 1px var(--fg); }}
.lk {{ font-family: var(--display); font-weight: 900; color: var(--fg); font-size: {theme.copy_px}px; }}
/* The home State: outlined over its neighbors' borders in the outline ink, its fill left to its rank (a
   stroke in screen pixels, so it reads at any map size); its callout chip ringed, its abbreviation inked;
   a tag on its row. */
#usmap path.home {{ fill: none; stroke: var(--fg); stroke-width: 2px; stroke-linejoin: round; vector-effect: non-scaling-stroke; pointer-events: none; }}
#usmap .co.home .chip {{ stroke: var(--fg); stroke-width: 1.5px; vector-effect: non-scaling-stroke; }}
#usmap .co.home .ab {{ fill: var(--fg); }}
/* At phone width the map's callouts are a list under it (us_calls), on the map's water in a panel like the
   map's, each rank in a chip at its size on a wide map: at most five columns (ten States in two rows), each
   wide enough for a three-character rank's callout; the home State's chip ringed, its abbreviation inked. */
.calls {{ display: none; grid-template-columns: repeat(auto-fill, minmax(max({calls_col}px, (100% - 4 * 8px) / 5), 1fr)); gap: 8px; list-style: none; margin: 0; padding: 10px 12px; background: var(--map-bg); border: 1px solid {theme.map_edge}; border-radius: 10px; }}
.calls li {{ display: flex; align-items: center; gap: {chip.gap}px; }}
.cchip {{ min-width: {chip.least}px; height: {chip_height}px; line-height: {chip_height}px; padding: 0 {chip_inset}px; border-radius: 4px; text-align: center; background: var(--f); color: var(--i); font-family: var(--display); font-weight: 900; font-size: {theme.chip_px}px; font-variant-numeric: tabular-nums; }}
.cab {{ font-family: var(--display); font-weight: 700; font-size: {theme.abbr_px}px; letter-spacing: {theme.metrics.abbr_tracking_em}em; color: var(--muted); }}
.calls .home .cchip {{ box-shadow: inset 0 0 0 1.5px var(--fg); }}
.calls .home .cab {{ color: var(--fg); }}
.hsw {{ background: transparent; box-shadow: inset 0 0 0 2px var(--fg); }}
.htag {{ display: inline-block; margin-left: 6px; font-family: var(--display); font-weight: 700; font-size: {theme.abbr_px}px; line-height: 1.35; letter-spacing: .04em; padding: 0 6px; border-radius: 4px; background: {theme.tag}; color: {theme.tag_fg}; vertical-align: 1px; }}
h2 {{ font-family: var(--display); font-weight: 800; font-size: 26px; margin: 0; letter-spacing: .01em; }}
.list {{ display: grid; gap: 10px; min-width: 0; }}
.tbl-wrap {{ overflow-x: auto; background: var(--surface); border: 1px solid var(--line); border-radius: 6px; }}
table {{ width: 100%; border-collapse: collapse; font-size: 14px; }}
th {{ text-align: left; font-family: var(--display); font-weight: 600; text-transform: uppercase; letter-spacing: .08em; font-size: 12px; color: var(--muted); padding: 10px 14px; border-bottom: 1px solid var(--line); }}
td {{ padding: 9px 14px; border-bottom: 1px solid var(--line); vertical-align: top; }}
tbody tr:last-child td {{ border-bottom: 0; }}
.rk {{ width: 64px; }}
.pill {{ display: inline-block; min-width: 40px; text-align: center; font-family: var(--display); font-weight: 900; font-size: 16px; padding: 1px 8px; border-radius: 4px; font-variant-numeric: tabular-nums; background: var(--f); color: var(--fg); }}
.sn {{ width: 170px; font-weight: 600; }}
td a {{ color: inherit; text-decoration: none; border-bottom: 1px solid var(--line); }}
td a:hover, td a:focus-visible {{ border-color: var(--accent); color: var(--accent); outline: none; }}
.firsts .cnt {{ font-family: var(--display); font-weight: 600; text-transform: uppercase; letter-spacing: .06em; font-size: 12px; color: var(--muted); }}
.firsts .names {{ line-height: 1.8; }}
.who {{ margin: 0; padding: 0; list-style: none; display: grid; gap: 2px; }}
.mk {{ font-variant-numeric: tabular-nums; font-weight: 600; display: inline-block; min-width: {mark_ch}ch; margin-right: .35em; }}
.gr, .tm {{ color: var(--muted); font-size: 13px; }}
.tie .mk {{ color: var(--accent); }}
details summary {{ cursor: pointer; font-weight: 500; }}
details summary:focus-visible {{ outline: 2px solid var(--accent); outline-offset: 2px; }}
details[open] summary {{ margin-bottom: 6px; }}
.notes {{ color: var(--muted); font-size: 13px; max-width: 72ch; display: grid; gap: 6px; }}
.notes p, .notes ul {{ margin: 0; }}
.notes ul {{ padding: 0; list-style: none; display: grid; gap: 2px; }}
.amap {{ height: min(78vh, 820px); width: auto; max-width: 100%; margin: 0; }}
.ct {{ fill: var(--f); stroke: var(--f); stroke-width: .6; cursor: pointer; transition: fill .25s, opacity .2s; }}
.dist {{ fill: var(--f); stroke: var(--fg); stroke-width: 1.1; stroke-linejoin: round; cursor: pointer; transition: fill .25s, opacity .2s; }}
.amap.picking .ct:not(.on), .amap.picking .dist:not(.on) {{ opacity: .4; }}
.cl {{ fill: none; stroke: {theme.county_line}; stroke-width: .5; pointer-events: none; }}
.bd {{ fill: none; stroke: var(--fg); stroke-width: 1.2; stroke-linejoin: round; pointer-events: none; }}
.albl {{ fill: var(--i); font-family: var(--display); font-weight: 900; text-anchor: middle; dominant-baseline: central; pointer-events: none; font-variant-numeric: tabular-nums; }}
.achip {{ fill: var(--f); cursor: pointer; }}
span.sup {{ font-size: .6em; vertical-align: .3em; line-height: 0; }}
tspan.sup {{ font-size: .6em; }}
.amap .ab {{ font-size: {theme.name_px}px; }}
.amap.picking .achip:not(.on) {{ opacity: .4; }}
.none {{ --f: {theme.no_rank.fill}; --i: {theme.no_rank.ink}; }}
.acards {{ display: grid; grid-template-columns: repeat(auto-fit, minmax(min(100%, 200px), 1fr)); gap: 12px; }}
.acard {{ display: grid; grid-template-rows: auto 1fr 22px; justify-items: start; gap: 4px; min-height: 168px; padding: 14px 16px; border: 0; border-radius: 6px; background: var(--f); box-shadow: inset 0 0 0 1px var(--fg); color: var(--fg); font: inherit; text-align: left; cursor: pointer; transition: background .25s, opacity .2s; }}
.acard:focus-visible {{ outline: 3px solid var(--accent); outline-offset: 2px; }}
.acard.on {{ box-shadow: inset 0 0 0 3px var(--fg); }}
.acards.picking .acard:not(.on) {{ opacity: .4; }}
.cname {{ font-family: var(--display); font-weight: 800; font-size: {theme.name_px}px; line-height: 1; }}
.crank {{ font-family: var(--display); font-weight: 900; font-size: 96px; line-height: .9; color: var(--fg); font-variant-numeric: tabular-nums; align-self: end; }}
.ctag {{ align-self: end; font-family: var(--display); font-weight: 700; font-size: {theme.abbr_px}px; letter-spacing: .04em; padding: 2px 8px; border-radius: 4px; background: {theme.tag}; color: {theme.tag_fg}; }}
.pickline {{ margin: 0; font-size: {theme.copy_px}px; }}
.pickline:empty {{ display: none; }}
.pickline a {{ color: var(--accent); font-weight: 600; text-decoration: none; border-bottom: 1px solid currentColor; }}
.mapnote, .nodata {{ margin: 0; color: var(--muted); font-size: 13px; max-width: 72ch; }}
tbody tr.on td {{ background: {theme.highlight_row}; }}
.smeet {{ display: grid; gap: 8px; margin-top: 6px; }}
.smeet h3 {{ font-family: var(--display); font-weight: 800; font-size: 20px; margin: 0; }}
.smeet p {{ margin: 0; max-width: 72ch; }}
.smlist {{ list-style: none; margin: 0; padding: 0; display: grid; grid-template-columns: repeat(auto-fill, minmax(min(100%, 220px), 1fr)); gap: 6px 14px; }}
.smlist li {{ display: grid; align-content: start; padding: 4px 8px; border-radius: 4px; background: var(--surface); border: 1px solid var(--line); }}
.smlist li span {{ color: var(--muted); font-size: 13px; }}
.smlist li.in {{ border-color: var(--accent); }}
.smlist li.in b::after {{ content: " \\2713"; color: var(--accent); }}
.smlist li.on {{ background: {theme.highlight_row}; }}
.smseed b {{ font-family: var(--display); font-weight: 900; font-size: 18px; }}
.smnotes {{ color: var(--muted); font-size: 13px; max-width: 72ch; }}
.smnotes ul {{ margin: 6px 0 0; padding-left: 18px; display: grid; gap: 4px; }}
.smnotes a {{ color: inherit; }}
@media (max-width: 560px) {{ #usmap path.home {{ stroke-width: 1.25px; }} #usmap .co {{ display: none; }} .calls {{ display: grid; }} .acard {{ min-height: 0; grid-template-columns: 1fr auto; grid-template-rows: auto auto; align-items: center; }} .crank {{ grid-row: 1 / 3; grid-column: 2; font-size: 64px; }} .amap {{ height: auto; width: 100%; }} .amap.tight .co {{ display: none; }} .legend .grp {{ width: 100%; }} td, th {{ padding: 8px 10px; }} .sn {{ width: auto; }} .stats b {{ font-size: 28px; }} .seg button {{ padding: 8px 11px; }} }}
@media (prefers-reduced-motion: reduce) {{ .st {{ transition: none; }} }}
</style>
</head>
<body>
<div class="wrap">
  <header>
    <h1>{athlete} {event_short} <span class="pr">{pr}</span> rankings</h1>
    <div class="controls">
      {maps}
      <div class="seg seasons" role="group" aria-label="Season">{season_buttons}</div>
      {view_groups}
    </div>
    <p class="lede">{pr} against <b>{against}</b> in each <span data-map="us">state</span>{map_lede}.</p>
    {stats}
  </header>
  <figure>
    {area_notes}
    {us_svg}
    {area_figure}
    <div class="legends">{legend}</div>
    {meet_panels}
  </figure>
  <section class="list">
    {headings}
    <div class="tbl-wrap">
      <table>
        <thead><tr><th>Rank</th><th>{name_head}</th><th>Ahead of {first}</th></tr></thead>
        {tables}
      </table>
    </div>
  </section>
  <div class="notes">
    <p>Rank is 1 plus the number of athletes in that grade group with a faster outdoor {event_name} that season on the high school list (FAT and official marks only). An athlete with the exact same mark shares the rank, shown with a small t. Grades are as the lists give them.</p>
    <p>{coverage} {links}</p>
    <ul>{pulled}</ul>
  </div>{footer}
</div>
<script>
const DATA = {data};
const AREAS = {areas};
const HOME = {home};  // each season's home State
let season = "{default_season}", map = "us", pick = null;
const views = {{}};  // last grade group picked in each season
function visible(el) {{
  return (!el.dataset.season || el.dataset.season === season)
    && (!el.dataset.view || el.dataset.view === views[season])
    && (!el.dataset.map || el.dataset.map === map);
}}
const SVG = "http://www.w3.org/2000/svg";
function rankTspans(t, label) {{  // as rank_svg: a tie's T as a small raised t
  t.replaceChildren();
  if (!label.startsWith("T")) {{ t.textContent = label; return; }}
  const sup = document.createElementNS(SVG, "tspan"), rest = document.createElementNS(SVG, "tspan");
  sup.setAttribute("class", "sup");
  sup.setAttribute("dy", "{tie_dy[0]}em");
  sup.textContent = "t";
  rest.setAttribute("dy", "{tie_dy[1]}em");
  rest.textContent = label.slice(1);
  t.append(sup, rest);
}}
const [DIGIT_EM, TIE_EM, BEARING_EM, INK_DROP_EM] = {label_em};
function leadTarget(e, side) {{  // as lead_target: beside the number, on the name's side
  const tie = e[0].startsWith("T"), n = e[0].length - (tie ? 1 : 0);
  const half = (DIGIT_EM * n + (tie ? TIE_EM : 0) - BEARING_EM) * e[2] / 2 + 4;
  return [side === "west" ? e[5] - half : e[5] + half, e[6] + INK_DROP_EM * e[2]];
}}
function chipWidth(label) {{ return Math.max({chip.least}, {chip.char} * label.length + {chip.pad}); }}
function chip(rect, label) {{
  const cx = +rect.dataset.cx, w = chipWidth(label);
  rect.setAttribute("x", cx - {chip.left});
  rect.setAttribute("width", w);
  const g = rect.parentNode;
  g.querySelector(".lbl").setAttribute("x", cx - {chip.left} + w / 2);
  g.querySelector(".ab").setAttribute("x", cx - {chip.left} + w + {chip.gap});
}}
function areaChip(rect, label) {{
  const g = rect.parentNode, x0 = +g.dataset.x0, w = chipWidth(label), west = g.dataset.side === "west";
  rect.setAttribute("width", w);
  g.querySelector(".albl").setAttribute("x", x0 + w / 2);
  g.querySelector(".lead").setAttribute("d", "M" + g.dataset.x + "," + g.dataset.y + "L" + (west ? x0 + w + {chip.lead} : x0 - {chip.lead}) + "," + g.dataset.cy);
}}
function pickLine(a) {{
  const p = document.querySelector(".pickline");
  if (!p) return;
  const e = pick && a[pick];
  p.replaceChildren();
  if (!e) return;
  const link = document.createElement("a");
  link.href = e[4];{pick_new_tab}
  link.textContent = "the list";
  p.append(e[3] + " \\u00b7 ", link);
}}
function rankHtml(el, label) {{  // as rank_html: a tie's T as a small raised t
  el.replaceChildren();
  if (!label.startsWith("T")) {{ el.textContent = label; return; }}
  const sup = document.createElement("span");
  sup.className = "sup";
  sup.textContent = "t";
  el.append(sup, label.slice(1));
}}
function show(s, v, m) {{
  if (!DATA[s]) s = "{default_season}";
  season = s;
  if (!DATA[s][v]) v = views[s] && DATA[s][views[s]] ? views[s] : Object.keys(DATA[s])[0];
  views[s] = v;
  map = m === "areas" && (document.getElementById("areamap") || document.getElementById("areacards")) ? "areas" : "us";
  const d = DATA[s][v], a = (AREAS[s] || {{}})[v] || {{}};
  if (pick && !a[pick]) pick = null;
  document.querySelectorAll(".seasons button").forEach(b => {{ if (!b.disabled) b.setAttribute("aria-pressed", String(b.dataset.season === s)); }});
  document.querySelectorAll(".views button").forEach(b => b.setAttribute("aria-pressed", String(b.dataset.view === v)));
  document.querySelectorAll(".maps button").forEach(b => b.setAttribute("aria-pressed", String(b.dataset.map === map)));
  document.querySelectorAll("[data-season]:not(button), [data-view]:not(button), [data-map]:not(button)").forEach(el => {{ el.toggleAttribute("hidden", !visible(el)); }});  // an attribute: SVG elements have no .hidden
  document.querySelectorAll("#usmap .st").forEach(p => {{
    const [, cls, , tip] = d[p.dataset.abbr];
    p.setAttribute("class", "st " + cls);
    p.querySelector("title").textContent = tip;
  }});
  document.querySelectorAll("#usmap .lbl").forEach(t => {{
    const [lab, cls, size, , , x, y] = d[t.dataset.abbr];
    t.setAttribute("class", "lbl " + cls);
    rankTspans(t, lab);
    t.style.fontSize = size + "px";
    if (x != null) {{ t.setAttribute("x", x); t.setAttribute("y", y); }}  // a pill's number stays put
  }});
  document.querySelectorAll("#usmap .chip").forEach(r => {{
    const [lab, cls] = d[r.dataset.abbr];
    r.setAttribute("class", "chip " + cls);
    chip(r, lab);
  }});
  // The season's home State: its outline (a copy of its shape, over its neighbors') and callout.
  const home = HOME[s] && document.querySelector('#usmap .st[data-abbr="' + HOME[s] + '"]'), outline = document.querySelector("#usmap path.home");
  if (home) outline.setAttribute("d", home.getAttribute("d"));
  outline.toggleAttribute("hidden", !home);
  document.querySelectorAll("#usmap .co").forEach(g => g.classList.toggle("home", g.dataset.abbr === HOME[s]));
  document.querySelectorAll(".calls li").forEach(li => {{  // the callouts listed under the map, for a phone
    const [lab, cls, , tip] = d[li.dataset.abbr], c = li.querySelector(".cchip");
    c.className = "cchip " + cls;
    rankHtml(c, lab);
    li.title = tip;
    li.classList.toggle("home", li.dataset.abbr === HOME[s]);
  }});
  document.querySelectorAll("#areamap .ct, #areamap .dist").forEach(p => {{
    const e = a[p.dataset.area], kind = p.classList.contains("dist") ? "dist" : "ct";
    p.setAttribute("class", kind + " " + (e ? e[1] : "none") + (pick === p.dataset.area ? " on" : ""));
  }});
  document.querySelectorAll("#areamap .albl").forEach(t => {{
    const e = a[t.dataset.area];
    t.setAttribute("class", "albl " + (e ? e[1] : "none"));
    rankTspans(t, e ? e[0] : "");
    if (!t.dataset.chip) t.style.fontSize = (e ? e[2] : {blank_px}) + "px";
    if (e && e[5] != null) {{ t.setAttribute("x", e[5]); t.setAttribute("y", e[6]); }}
  }});
  document.querySelectorAll("#areamap .co").forEach(g => {{
    const e = a[g.dataset.area];
    if (g.querySelector(".achip")) return;  // a pill's lead stays put
    // Beside the number, or at the Section itself when this view has no rank for it.
    const [x, y] = e && e[0] && e[5] != null ? leadTarget(e, g.dataset.side) : [+g.dataset.sx, +g.dataset.sy];
    g.querySelector(".dot").setAttribute("cx", x);
    g.querySelector(".dot").setAttribute("cy", y);
    g.querySelector(".lead").setAttribute("d", "M" + x + "," + y + "L" + g.dataset.end + "," + g.dataset.cy);
  }});
  document.querySelectorAll("#areamap .achip").forEach(r => {{
    const e = a[r.dataset.area];
    r.setAttribute("class", "achip " + (e ? e[1] : "none") + (pick === r.dataset.area ? " on" : ""));
    areaChip(r, e ? e[0] : "");
  }});
  document.querySelectorAll("#areacards .acard").forEach(c => {{
    const e = a[c.dataset.area];
    c.setAttribute("class", "acard " + (e ? e[1] : "none") + (pick === c.dataset.area ? " on" : ""));
    c.setAttribute("aria-pressed", String(pick === c.dataset.area));
    c.title = e ? e[3] : "";
    rankHtml(c.querySelector(".crank"), e ? e[0] : "");
  }});
  const amap = document.getElementById("areamap") || document.getElementById("areacards");
  if (amap) amap.classList.toggle("picking", !!pick);
  document.querySelectorAll("tr[data-pick], .smlist li").forEach(tr => tr.classList.toggle("on", map === "areas" && tr.dataset.pick.split("|").includes(pick)));
  pickLine(a);
  const counts = {{}};
  Object.values(map === "us" ? d : a).forEach(e => {{ counts[e[1]] = (counts[e[1]] || 0) + 1; }});
  document.querySelectorAll(".legend li[data-b]").forEach(li => {{
    const n = counts[li.dataset.b] || 0;
    li.querySelector(".lc").textContent = "\\u00b7 " + n + " " + (map === "us" ? (n === 1 ? "state" : "states") : (n === 1 ? "{area_noun}" : "{area_nouns}"));
    li.hidden = n === 0;
  }});
  document.querySelectorAll(".legend").forEach(ul => {{ ul.hidden = !ul.querySelector("li[data-b]:not([hidden])") || (map === "areas" && !document.getElementById("areamap")); }});
  try {{ history.replaceState(null, "", "#" + (map === "us" ? "" : "areas-") + s + "-" + v); }} catch (err) {{}}
}}
document.querySelector(".seasons").addEventListener("click", e => {{
  const b = e.target.closest("button");
  if (b && !b.disabled) show(b.dataset.season, views[b.dataset.season], map);
}});
document.querySelectorAll(".views").forEach(g => g.addEventListener("click", e => {{
  const b = e.target.closest("button");
  if (b) show(season, b.dataset.view, map);
}}));
const maps = document.querySelector(".maps");
if (maps) maps.addEventListener("click", e => {{
  const b = e.target.closest("button");
  if (b) show(season, views[season], b.dataset.map);
}});
const amap = document.getElementById("areamap"), acards = document.getElementById("areacards");
// On a phone the callouts would be too small to read: fit the map to the State, and let the
// ranks inside it and the pick line say the rest.
const narrow = window.matchMedia("(max-width: 560px)");
function fitAreaMap() {{
  if (!amap) return;
  amap.setAttribute("viewBox", narrow.matches ? amap.dataset.tight : amap.dataset.wide);
  amap.classList.toggle("tight", narrow.matches);
}}
fitAreaMap();
if (narrow.addEventListener) narrow.addEventListener("change", fitAreaMap);
// And the US map to the States, its callouts listed under it instead (the CSS: .calls).
const usmap = document.getElementById("usmap");
function fitUsMap() {{ usmap.setAttribute("viewBox", narrow.matches ? usmap.dataset.tight : usmap.dataset.wide); }}
fitUsMap();
if (narrow.addEventListener) narrow.addEventListener("change", fitUsMap);
[amap, acards].forEach(el => {{ if (el) el.addEventListener("click", pickArea); }});
function pickArea(e) {{
  const el = e.target.closest("[data-area]");
  if (!el) return;
  pick = pick === el.dataset.area ? null : el.dataset.area;
  show(season, views[season], map);
}}
const m = location.hash.match(/^#(?:(areas)-)?(\\d{{4}})-(.+)$/);
show(m ? m[2] : "{default_season}", m ? m[3] : "{default_view}", m && m[1] ? "areas" : "us");
</script>
</body>
</html>
"""
