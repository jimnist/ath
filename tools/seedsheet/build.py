"""Render a seed sheet page (HTML) and its CSV from a saved `ath seed` run.

A saved sheet is one `ath seed --json` report, the seed sheet CSV that same run wrote, and the
day it was pulled (see sheet_file). The page lists the entrants in the report's order, which is
the CSV's order: marks fastest first (ath sorts on SortInt), then everyone with no mark. The CSV
is ath's own, carried byte for byte: this build reads it, never writes one. Used by
seedsheet.py; see README.md. Standard library only.
"""
import csv
import io
import re
import sys
from datetime import date
from pathlib import Path
from urllib.parse import quote

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from report import THEME, esc, link, pretty_date, styles  # noqa: E402

# The saved-sheet format this build reads, and the `ath seed` JSON schemas inside it.
# Format 2 (from the seed sheet page's review) added ath's own CSV; format 1 sheets are collected
# again.
# A relay's sheet is schema 1 too, with `kind: "relay"` and `teams` where an individual
# sheet (no `kind`) has `entrants`.
SHEET_FORMAT = 2
ATH_SEED_SCHEMA = 1
RELAY_KIND = "relay"
RELAY_LEGS = 4


def _int(x):
    return isinstance(x, int) and not isinstance(x, bool)


def _number(x):
    return isinstance(x, (int, float)) and not isinstance(x, bool)


def _str(x):
    return isinstance(x, str)


def _maybe(kind):
    return lambda x: x is None or kind(x)


# Each entrant field the page reads, and the type `ath seed` gives it.
ENTRANT_TYPES = {
    "rank": _int, "athlete": _str, "athlete_id": _int, "athlete_url": _str, "pr": _str,
    "pr_numeric": _maybe(_number), "sort_int": _maybe(_int), "pr_season": _maybe(_int),
    "school": _str, "school_url": _str, "club": _str, "club_url": _str,
}
LIST_TYPES = {
    "skipped": {"name": _str, "athlete_url": _str},
    "unresolved": {"name": _str, "reason": _str},
}
# A relay sheet's teams and each team's legs (runners), as `ath seed` gives them.
TEAM_TYPES = {
    "rank": _int, "team": _str, "letter": _str, "team_url": _str, "aggregate": _str,
    "aggregate_seconds": _maybe(_number), "sort_int": _maybe(_int),
    "missing_legs": lambda x: isinstance(x, list) and all(_int(n) for n in x),
    "same_runner_legs": lambda x: isinstance(x, list) and all(
        isinstance(g, list) and all(_int(n) for n in g) for g in x),
    "no_aggregate_reason": _maybe(_str),
    "estimated": lambda x: isinstance(x, bool),
    "legs": lambda x: isinstance(x, list) and all(isinstance(leg, dict) for leg in x),
}
LEG_TYPES = {
    "leg": _int, "athlete": _str, "athlete_id": _int, "athlete_url": _str, "grade": _maybe(_int),
    "pr": _str, "pr_numeric": _maybe(_number), "pr_season": _maybe(_int), "sort_int": _maybe(_int),
    "estimated": lambda x: isinstance(x, bool), "estimated_from": _maybe(_str), "estimated_from_pr": _maybe(_str),
}


def is_mile(name):
    """Whether an event name is the mile, however a Profile spells it: `1 Mile`, `Mile`, `1 Mile Run`,
    `Mile Run`, in any case and spacing."""
    return bool(name) and re.fullmatch(r"(1\s*)?mile(\s*run)?", name.strip().lower()) is not None


def event_words(name):
    """An event as the page says it in a sentence: `mile` for the mile, else its name."""
    return "mile" if is_mile(name) else (name or "another event")


def estimate_source(leg):
    """What an Estimated mark was made from, in words: `the mile (5:02.10)`."""
    return f'the {esc(event_words(leg["estimated_from"]))} ({esc(leg["estimated_from_pr"] or "")})'


def is_relay(s):
    """Whether an `ath seed` report is a relay's: its `kind` says so (an individual sheet has none)."""
    return s.get("kind") == RELAY_KIND


class SheetError(ValueError):
    """A sheet that cannot be built (or saved): the message says why."""


def sheet_file(seed, csv_text, pulled):
    """What seedsheet saves for an event: the `ath seed` report, the CSV that run wrote, and the
    day it was pulled."""
    return {"format": SHEET_FORMAT, "pulled": pulled, "seed": seed, "csv": csv_text}


def check(saved):
    """The `ath seed` report inside a saved sheet, if it is one this build can render; raises
    SheetError saying why not, for any shape of file (never TypeError, KeyError and the like)."""
    try:
        return _check(saved)
    except SheetError:
        raise
    except (TypeError, KeyError, AttributeError, IndexError, ValueError, csv.Error) as e:
        raise SheetError(f"is not a seed sheet this build can read ({type(e).__name__}: {e})") from e


def _iso(x):
    """Whether x is a YYYY-MM-DD date."""
    if not _str(x):
        return False
    try:
        date.fromisoformat(x)
    except ValueError:
        return False
    return True


def _check(saved):
    if not isinstance(saved, dict):
        raise SheetError("is not a saved seed sheet (not an object)")
    if saved.get("format") == 1:
        raise SheetError("was saved by an older seedsheet (format 1, without ath's CSV); "
                         "run seedsheet without --no-fetch to collect it again")
    if saved.get("format") != SHEET_FORMAT:
        raise SheetError(f"is not a saved seed sheet (format {saved.get('format')})")
    if not _iso(saved.get("pulled")):
        raise SheetError(f"has no pulled date (got {saved.get('pulled')!r})")
    s = saved.get("seed")
    if not isinstance(s, dict) or s.get("schema") != ATH_SEED_SCHEMA:
        got = s.get("schema") if isinstance(s, dict) else None
        raise SheetError(f"holds `ath seed` JSON schema {got}; this build reads schema {ATH_SEED_SCHEMA}")
    if s.get("kind", RELAY_KIND) != RELAY_KIND:
        raise SheetError(f"holds an `ath seed` sheet of kind {s['kind']!r}; this build reads individual sheets "
                         f"(no kind) and {RELAY_KIND!r}")
    relay = is_relay(s)
    for key in ("event", "as_of", "teams" if relay else "entrants") + (("seeding_event",) if relay else ()):
        if key not in s:
            raise SheetError(f"`ath seed` report has no {key}")
    if not _str(s["event"]) or not _maybe(_str)(s.get("event_name")):
        raise SheetError("`ath seed` report's event is not text")
    if relay and not (_str(s["seeding_event"]) and _maybe(_str)(s.get("seeding_event_name"))
                      and _maybe(_str)(s.get("estimate_from"))):
        raise SheetError("`ath seed` report's seeding event is not text")
    if not _iso(s["as_of"]):
        raise SheetError(f"`ath seed` report's as_of is not a YYYY-MM-DD date (got {s['as_of']!r})")
    if not (s.get("meet_date") is None or _iso(s["meet_date"])):
        raise SheetError(f"`ath seed` report's meet_date is not a YYYY-MM-DD date (got {s['meet_date']!r})")
    lists = (("teams", TEAM_TYPES),) if relay else (("entrants", ENTRANT_TYPES),)
    for key, types in (*lists, *LIST_TYPES.items()):
        _check_items(s.get(key, []), key, types)
    if relay:
        for i, t in enumerate(s["teams"], 1):
            _check_items(t["legs"], f"team {i}'s legs", LEG_TYPES)
        _check_teams(s["teams"])
        _check_relay_csv(saved.get("csv"), s["teams"])
        return s
    marks_done = False
    for i, e in enumerate(s["entrants"], 1):
        if e["rank"] != i:
            raise SheetError(f"entrant {i} is ranked {e['rank']}; ath ranks entrants 1 to n in order")
        if (e["sort_int"] is None) != (e["pr"] == ""):
            raise SheetError(f"entrant {i} ({e['athlete']}) has a mark without its SortInt, or the other way round")
        if e["sort_int"] is None:
            marks_done = True
        elif marks_done:
            raise SheetError(f"entrant {i} ({e['athlete']}) has a mark but comes after an entrant with none")
    _check_csv(saved.get("csv"), s["entrants"])
    return s


def _check_items(items, key, types):
    """Every item of a report's list is an object with each field in types, of its type."""
    if not isinstance(items, list) or not all(isinstance(x, dict) for x in items):
        raise SheetError(f"`ath seed` report's {key} is not a list of objects")
    for i, x in enumerate(items, 1):
        missing = set(types) - set(x)
        if missing:
            raise SheetError(f"`ath seed` report's {key} item {i} has no {', '.join(sorted(missing))}")
        wrong = sorted(k for k, ok in types.items() if not ok(x[k]))
        if wrong:
            raise SheetError(f"`ath seed` report's {key} item {i} has the wrong type for {', '.join(wrong)}")


def _check_teams(teams):
    """A relay sheet in seed order: ranks 1 to n, every aggregate before the teams without one, each
    aggregate the sum of its four runners' SortInts, and legs 1 to 4 each either run (in leg order) or
    missing, once."""
    aggregates_done = False
    for i, t in enumerate(teams, 1):
        name = f"team {i} ({t['team']} {t['letter']})"
        if t["rank"] != i:
            raise SheetError(f"team {i} is ranked {t['rank']}; ath ranks teams 1 to n in order")
        run = [leg["leg"] for leg in t["legs"]]
        if run != sorted(run) or sorted(run + t["missing_legs"]) != list(range(1, RELAY_LEGS + 1)):
            raise SheetError(f"{name} has legs {run} and missing legs {t['missing_legs']}, "
                             f"not legs 1 to {RELAY_LEGS} once each")
        for leg in t["legs"]:
            if (leg["sort_int"] is None) != (leg["pr"] == ""):
                raise SheetError(f"{name}'s runner {leg['athlete']} has a mark without its SortInt, or the other way round")
        if (t["sort_int"] is None) != (t["aggregate"] == ""):
            raise SheetError(f"{name} has an aggregate without its SortInt, or the other way round")
        if (t["sort_int"] is None) != (t["no_aggregate_reason"] is not None):
            raise SheetError(f"{name} has no aggregate without saying why, or says why with one")
        if t["sort_int"] is None:
            aggregates_done = True
            continue
        if aggregates_done:
            raise SheetError(f"{name} has an aggregate but comes after a team with none")
        legs = [leg["sort_int"] for leg in t["legs"]]
        if (t["missing_legs"] or t["same_runner_legs"] or None in legs
                or len({leg["athlete_id"] for leg in t["legs"]}) != len(legs) or sum(legs) != t["sort_int"]):
            raise SheetError(f"{name}'s aggregate is not the sum of its four runners' marks")


def _check_relay_csv(text, teams):
    """ath's relay CSV must be the same sheet as its report: the same teams and runners in the same
    order."""
    if not _str(text):
        raise SheetError("has no seed sheet CSV from ath")
    rows = list(csv.reader(io.StringIO(text)))
    ids = [f"runner{n}_id" for n in range(1, RELAY_LEGS + 1)]
    if not rows or not all(c in rows[0] for c in ("team", "letter", *ids)):
        raise SheetError("holds a CSV with no team, letter and runner id columns")
    cols = [rows[0].index(c) for c in ("team", "letter", *ids)]
    got = [[r[c] if c < len(r) else "" for c in cols] for r in rows[1:]]
    want = [[t["team"], t["letter"], *[str(leg["athlete_id"]) if leg else "" for _, leg in slots(t)]]
            for t in teams]
    if got != want:
        raise SheetError("holds a CSV that is not the report's sheet (teams, runners or order differ)")


def _check_csv(text, entrants):
    """ath's CSV must be the same sheet as its report: the same athletes in the same order."""
    if not _str(text):
        raise SheetError("has no seed sheet CSV from ath")
    rows = list(csv.reader(io.StringIO(text)))
    if not rows or "athlete_id" not in rows[0]:
        raise SheetError("holds a CSV with no athlete_id column")
    col = rows[0].index("athlete_id")
    ids = [r[col] for r in rows[1:]]
    if ids != [str(e["athlete_id"]) for e in entrants]:
        raise SheetError("holds a CSV that is not the report's sheet (athletes or order differ)")


# ---------------------------------------------------------------- page

def slug(*parts):
    """Output file name from words: `Spring Invitational`, `Girls 400m` -> spring-invitational-girls-400m."""
    return re.sub(r"[^a-z0-9]+", "-", " ".join(parts).lower()).strip("-")


def ordinal(n):
    suffix = "th" if 10 <= n % 100 <= 20 else {1: "st", 2: "nd", 3: "rd"}.get(n % 10, "th")
    return f"{n}{suffix}"


def plural(n, word):
    """`1 entrant`, `2 entrants`."""
    return f"{n} {word}{'' if n == 1 else 's'}"


def size(s):
    """How big a checked sheet is: `8 entrants`, or a relay's `4 teams`."""
    return plural(len(s["teams"]), "team") if is_relay(s) else plural(len(s["entrants"]), "entrant")


def counts(s):
    """`8 entrants, 5 with a mark`, or a relay's `4 teams, 2 with an aggregate`."""
    rows = s["teams"] if is_relay(s) else s["entrants"]
    seeded = sum(r["sort_int"] is not None for r in rows)
    return f"{size(s)}, {seeded} with {'an aggregate' if is_relay(s) else 'a mark'}"


def athlete_lines(s, athletes):
    """Each configured Athlete on a sheet in words, for the run's summary: `Gwen Garcia 56.85 (seed
    3)`, or on a relay `Nico Vance 2:00.40 (leg 2, Pinecrest Academy A, seed 2)`."""
    if not is_relay(s):
        return [f"{e['athlete']} {e['pr'] or 'no mark'} (seed {e['rank']})"
                for e in s["entrants"] if e["athlete_id"] in athletes]
    return [f"{leg['athlete']} {leg['pr'] or 'no mark'} (leg {leg['leg']}, {t['team']} {t['letter']}, "
            f"seed {t['rank']})"
            for t in s["teams"] for leg in t["legs"] if leg["athlete_id"] in athletes]


def team(name, url):
    if not name:
        return ""
    return link(name, url) if url else esc(name)


def teams_line(e):
    """School and club under the name, for narrow screens; each wraps whole."""
    parts = [team(e["school"], e["school_url"]), team(e["club"], e["club_url"])]
    return " &middot; ".join(f'<span class="tw">{p}</span>' for p in parts if p)


def row(e, athletes, event_name):
    me = e["athlete_id"] in athletes
    has = e["sort_int"] is not None
    classes = " ".join(c for c in ("me" if me else "", "" if has else "nm") if c)
    cls = f' class="{classes}"' if classes else ""
    pr = (f'<span class="mk">{esc(e["pr"])}</span>' if has
          else f'<span class="none" title="No mark in the {esc(event_name)}">No mark</span>')
    season = esc(e["pr_season"]) if e["pr_season"] is not None else ""
    sub_season = f'<span class="ps">{season}</span>' if season else ""
    return (f'<tr{cls} data-athlete="{esc(e["athlete_id"])}">'
            f'<td class="rk">{esc(e["rank"])}</td>'
            f'<td class="ath">{link(e["athlete"], e["athlete_url"])}'
            f'<div class="sub">{teams_line(e)}</div></td>'
            f'<td class="pr">{pr}{sub_season}</td>'
            f'<td class="c-season">{season}</td>'
            f'<td class="c-team">{team(e["school"], e["school_url"])}</td>'
            f'<td class="c-team">{team(e["club"], e["club_url"])}</td></tr>')


def spotlight(s, athletes, event_name):
    """One line per configured Athlete on the sheet: their seed and mark."""
    n = len(s["entrants"])
    out = []
    for e in s["entrants"]:
        if e["athlete_id"] not in athletes:
            continue
        if e["sort_int"] is None:
            what = f'no mark in the {esc(event_name)} yet, so seeded at the bottom ({ordinal(e["rank"])} of {n})'
            out.append(f'<li><b>{esc(e["athlete"])}</b> {what}</li>')
        else:
            out.append(f'<li><span class="hl">{esc(e["pr"])}</span> <b>{esc(e["athlete"])}</b> '
                       f'seeded {ordinal(e["rank"])} of {n}</li>')
    return f'<ul class="spot">{"".join(out)}</ul>' if out else ""


def html(s, meta, theme=THEME):
    """The page, in theme. meta: {"meet", "label", "athletes" (ids to highlight), "pulled", "csv" (ath's
    CSV text), "csv_name", and "index" (the meet's index page, linked back to) when it has one}."""
    if is_relay(s):
        return relay_html(s, meta, theme)
    event_name = s.get("event_name") or s["event"]
    athletes = set(meta.get("athletes") or [])
    marks = [e for e in s["entrants"] if e["sort_int"] is not None]
    nomarks = [e for e in s["entrants"] if e["sort_int"] is None]
    as_of = pretty_date(s["as_of"])
    meet_date = s.get("meet_date")
    kicker = kicker_line(meta, meet_date)
    body = "".join(row(e, athletes, event_name) for e in marks)
    tail = ""
    if nomarks:
        tail = (f'<tbody class="nomark"><tr class="divider"><th colspan="6" scope="rowgroup">'
                f'No mark in the {esc(event_name)} &middot; {len(nomarks)}</th></tr>'
                + "".join(row(e, athletes, event_name) for e in nomarks) + "</tbody>")
    lede = (f'{plural(len(s["entrants"]), "entrant")}, seeded on their Personal record in the '
            f'{esc(event_name)} as of {as_of}, fastest first.')
    if nomarks:
        lede += f' {len(nomarks)} with no mark {"is" if len(nomarks) == 1 else "are"} at the bottom.'
    missing = missing_items(s)
    if missing:
        lede += (f' <a class="miss-link" href="#missing">{plural(len(missing), "name")} on the roster '
                 f'{"is" if len(missing) == 1 else "are"} not on the sheet.</a>')
    notes = (f"    <p>Each entrant is seeded on their Personal record in the {esc(event_name)}: their fastest "
             f"outdoor mark in any season, set on or before {as_of}. Indoor marks, relay splits, exhibition and "
             f"hand-timed marks never count. Set is the season the mark was run in.</p>\n"
             f"    <p>Entrants with no mark in the {esc(event_name)} are listed last. Names link to their profiles.</p>")
    return page_html(
        meta, theme, kicker=kicker, lede=lede, spotlight=spotlight(s, athletes, event_name),
        table=table(body, tail) if s["entrants"] else '<p class="empty">Nobody on the roster could be seeded yet.</p>',
        missing=missing_section(missing), notes=notes, relay_css="")


def kicker_line(meta, meet_date):
    """The line above the title: the meet and its date, after a link back to the meet's index page
    when the page has one (meta["index"])."""
    kicker = esc(meta["meet"]) + (f" &middot; {pretty_date(meet_date)}" if meet_date else "")
    if meta.get("index"):
        kicker = f'<a class="back" href="{esc(meta["index"])}">&larr; All events</a> &middot; ' + kicker
    return kicker


def page_html(meta, theme, **parts):
    """TEMPLATE filled in: the page's own parts (kicker, lede, spotlight, table, missing, notes,
    relay_css) with the title, styles and the CSV both kinds of page share."""
    csv_body = meta["csv"]
    csv_href = "data:text/csv;charset=utf-8," + quote(csv_body)
    return TEMPLATE.format(
        title=esc(f"{meta['label']} seed sheet"),
        **vars(styles(theme)), **SHARED_CSS, theme=theme, label=esc(meta["label"]),
        csv_href=esc(csv_href), csv_name=esc(meta["csv_name"]), csv_text=esc(csv_body),
        pulled=pretty_date(meta["pulled"]), **parts,
    )


# ---------------------------------------------------------------- relay page

def relay_html(s, meta, theme=THEME):
    """A relay's page, in theme: one group per team in seed order, the team and its aggregate (the
    seed mark), then its four runners in leg order with their grades and Personal records in the
    Seeding event. Teams without an aggregate sit under a divider at the bottom. A configured
    Athlete's runner row is highlighted on every team they run on."""
    seed_on = esc(s.get("seeding_event_name") or s["seeding_event"])
    athletes = set(meta.get("athletes") or [])
    teams = s["teams"]
    with_agg = [t for t in teams if t["sort_int"] is not None]
    without = [t for t in teams if t["sort_int"] is None]
    as_of = pretty_date(s["as_of"])
    meet_date = s.get("meet_date")
    kicker = kicker_line(meta, meet_date)
    body = "".join(team_group(t, athletes, seed_on) for t in with_agg)
    if without:
        body += (f'<tbody class="nomark"><tr class="divider"><th colspan="5" scope="rowgroup">'
                 f'No aggregate &middot; {len(without)}</th></tr></tbody>'
                 + "".join(team_group(t, athletes, seed_on) for t in without))
    lede = (f'{plural(len(teams), "team")}, seeded on the sum of their four runners’ Personal records in '
            f'the {seed_on} as of {as_of}, fastest first.')
    if without:
        lede += (f' {len(without)} missing a runner, or with a runner who has no mark, '
                 f'{"is" if len(without) == 1 else "are"} at the bottom.')
    skipped = s.get("skipped") or []
    if skipped:
        lede += (f' <a class="miss-link" href="#missing">{plural(len(skipped), "runner")}’s profile '
                 f'could not be read.</a>' if len(skipped) == 1 else
                 f' <a class="miss-link" href="#missing">{len(skipped)} runners’ profiles could not be read.</a>')
    estimate = ""
    if s.get("estimate_from"):
        source = esc(event_words(s["estimate_from"]))
        lede += (f" A runner’s {source}, converted to the {seed_on} at even pace, stands in when it is faster, "
                 f"marked as an estimate.")
        estimate = (f" Where a runner’s {source} converted to the {seed_on} at even pace ({source} time "
                    f"&times; 1600 / 1609.344) is faster than their {seed_on}, the converted mark stands in and is "
                    f"marked as estimated; a team with one is marked as including estimated legs.")
    notes = (f"    <p>Each team is seeded on its aggregate: the sum of its four runners’ Personal records in the "
             f"{seed_on}, each their fastest outdoor mark in any season, set on or before {as_of}. Indoor marks, "
             f"relay splits, exhibition and hand-timed marks never count.{estimate} Set is the season a mark was "
             f"run in.</p>\n"
             f"    <p>A team missing a runner, or with a runner who has no mark in the {seed_on}, has no aggregate "
             f"and is listed last. Runners are in leg order; names link to their profiles.</p>")
    return page_html(
        meta, theme, kicker=kicker, lede=lede, spotlight=relay_spotlight(s, athletes, seed_on),
        table=relay_table(body) if teams else '<p class="empty">No team could be seeded yet.</p>',
        missing=skipped_section(skipped), notes=notes, relay_css="\n" + RELAY_CSS)


def team_name(t):
    """`Pinecrest Academy` (linked to the school) and its letter."""
    return f'{team(t["team"], t["team_url"])} <span class="ltr">{esc(t["letter"])}</span>'


def slots(t):
    """A team's legs 1 to 4 in order, each as (leg number, its runner or None when it has none)."""
    by_leg = {leg["leg"]: leg for leg in t["legs"]}
    return [(n, by_leg.get(n)) for n in range(1, RELAY_LEGS + 1)]


def team_group(t, athletes, seed_on):
    """One team's rows: the team with its aggregate, then a row per leg 1 to 4 in order, a leg with no
    runner in its own place."""
    has = t["sort_int"] is not None
    mine = any(leg["athlete_id"] in athletes for leg in t["legs"])
    agg = (f'<span class="mk">{esc(t["aggregate"])}</span>' if has
           else '<span class="none" title="No aggregate">No aggregate</span>')
    # Why there is no aggregate, leg by leg number, as ath says it.
    why = "" if has else f'<div class="why">{esc(t["no_aggregate_reason"] or "")}</div>'
    if has and t["estimated"]:
        why = '<div class="why">Includes estimated legs</div>'
    cls = " ".join(c for c in ("team", "mine" if mine else "", "" if has else "nm") if c)
    rows = [f'<tr class="tm"><td class="rk">{esc(t["rank"])}</td>'
            f'<th class="tn" scope="rowgroup">{team_name(t)}{why}</th><td class="c-grade"></td>'
            f'<td class="pr">{agg}</td><td class="c-season"></td></tr>']
    for n, leg in slots(t):
        if leg is not None:
            rows.append(leg_row(leg, n, athletes, seed_on))
            continue
        rows.append(f'<tr class="lg nm"><td class="rk"></td><td class="ath"><span class="ln">{esc(n)}</span>'
                    f'<span class="nr">No runner entered</span></td><td class="c-grade"></td>'
                    f'<td class="pr"><span class="none">No runner</span></td><td class="c-season"></td></tr>')
    return f'<tbody class="{cls}">{"".join(rows)}</tbody>'


def leg_row(leg, n, athletes, seed_on):
    me = leg["athlete_id"] in athletes
    has = leg["sort_int"] is not None
    classes = " ".join(c for c in ("lg", "me" if me else "", "" if has else "nm") if c)
    pr = (f'<span class="mk">{esc(leg["pr"])}</span>' if has
          else f'<span class="none" title="No mark in the {seed_on}">No mark</span>')
    season = esc(leg["pr_season"]) if leg["pr_season"] is not None else ""
    sub_season = f'<span class="ps">{season}</span>' if season else ""
    grade = esc(leg["grade"]) if leg["grade"] is not None else ""
    sub_grade = f'<div class="sub">Grade {grade}</div>' if grade else ""
    est = f'<div class="est">estimated from {estimate_source(leg)}</div>' if leg["estimated"] else ""
    return (f'<tr class="{classes}" data-athlete="{esc(leg["athlete_id"])}">'
            f'<td class="rk"></td>'
            f'<td class="ath"><span class="ln" title="Leg {esc(n)}">{esc(n)}</span>'
            f'{link(leg["athlete"], leg["athlete_url"])}{sub_grade}</td>'
            f'<td class="c-grade">{grade}</td>'
            f'<td class="pr">{pr}{sub_season}{est}</td>'
            f'<td class="c-season">{season}</td></tr>')


def relay_spotlight(s, athletes, seed_on):
    """One line per configured Athlete on a relay sheet, for each team they run on: their leg, their
    mark, and the team's seed."""
    n = len(s["teams"])
    out = []
    for t in s["teams"]:
        for leg in t["legs"]:
            if leg["athlete_id"] not in athletes:
                continue
            where = f'leg {esc(leg["leg"])} for {esc(t["team"])} {esc(t["letter"])}'
            place = f'({ordinal(t["rank"])} of {n})'
            if leg["sort_int"] is None:
                out.append(f'<li><b>{esc(leg["athlete"])}</b> runs {where} with no mark in the {seed_on} yet, '
                           f'so the team has no aggregate and is seeded at the bottom {place}</li>')
                continue
            seed = (f'seeded {ordinal(t["rank"])} of {n} ({esc(t["aggregate"])})' if t["sort_int"] is not None
                    else f'which has no aggregate, so is seeded at the bottom {place}')
            est = f'; the mark is estimated from {estimate_source(leg)}' if leg["estimated"] else ""
            out.append(f'<li><span class="hl">{esc(leg["pr"])}</span> <b>{esc(leg["athlete"])}</b> runs '
                       f'{where}, {seed}{est}</li>')
    return f'<ul class="spot">{"".join(out)}</ul>' if out else ""


def skipped_section(skipped):
    """Runners whose profile could not be read: on their team above, without a mark."""
    if not skipped:
        return ""
    items = "".join(f'<li><b>{link(k["name"], k["athlete_url"])}</b> <span>profile could not be read</span></li>'
                    for k in skipped)
    return (f'<section class="missing" id="missing"><h2>Profiles not read &middot; {len(skipped)}</h2>'
            f'<p>These runners are on their teams above, but with no mark, since their profiles could not be read.</p>'
            f'<ul>{items}</ul></section>')


def relay_table(body):
    return f"""<div class="tbl-wrap">
    <table class="relay">
      <thead><tr><th class="rk" scope="col">Seed</th><th scope="col">Team &middot; runners by leg</th><th class="c-grade" scope="col">Grade</th><th class="pr" scope="col">Mark</th><th class="c-season" scope="col">Set</th></tr></thead>
      {body}
    </table>
  </div>"""


# A relay page's own layout, after the shared TEMPLATE's: each team a group of rows, its name and
# aggregate on a ruled first row, its runners indented under it with a leg number.
RELAY_CSS = """.relay .tm td, .relay .tm th { padding-top: 12px; border-bottom: 0; }
.relay .tn { text-align: left; font-family: var(--display); font-weight: 800; font-size: 18px; padding: 12px 12px 4px; }
.relay .tn a { color: inherit; text-decoration: none; border-bottom: 1px solid var(--line); }
.relay .tn a:hover, .relay .tn a:focus-visible { border-color: var(--accent); color: var(--accent); outline: none; }
.relay .ltr { display: inline-block; margin-left: .2em; padding: 0 .35em; font-size: 14px; font-weight: 700; color: var(--muted); border: 1px solid var(--line); border-radius: 3px; vertical-align: 2px; }
.relay .why { font-family: var(--body); font-weight: 400; font-size: 13px; color: var(--muted); margin-top: 2px; }
.relay .est { font-family: var(--body); font-size: 12px; font-style: italic; color: var(--muted); white-space: normal; }
.relay .tm .mk { font-size: 20px; }
.relay tbody.team + tbody.team .tm td, .relay tbody.team + tbody.team .tm th { border-top: 1px solid var(--line); }
.relay .lg td { padding-top: 5px; padding-bottom: 5px; border-bottom: 0; }
.relay tbody.team .lg:last-child td { padding-bottom: 12px; }
.relay .lg .ath { font-weight: 400; padding-left: 12px; }
.relay .lg .mk { font-size: 15px; font-weight: 700; }
.relay .ln { display: inline-block; width: 1.6em; font-family: var(--display); font-weight: 700; font-size: 13px; color: var(--muted); font-variant-numeric: tabular-nums; }
.relay .nr { color: var(--muted); font-style: italic; }
.relay .c-grade { width: 5em; color: var(--muted); font-variant-numeric: tabular-nums; }
.relay .pr { width: 8em; }
.relay .c-season { width: 4.5em; }
.relay tbody.mine .tm .rk { color: var(--fg); }
@media (max-width: 640px) {
  .relay .c-grade { display: none; }
  .relay .lg .ath { padding-left: 8px; }
  .relay .lg .sub { margin-left: 1.6em; }
}"""


# Why a roster row is not on the sheet, as the page says it (`ath seed`'s `unresolved[].reason`).
REASONS = {
    "no_match": "no profile found for this name",
    "ambiguous": "more than one possible profile; not yet picked",
    "search_failed": "the profile search failed",
}


def missing_items(s):
    """Everyone on the roster who is not on the sheet, as (name HTML, why): profiles that could
    not be read, then rows that did not resolve to one athlete."""
    out = [(link(k["name"], k["athlete_url"]), "profile could not be read") for k in s.get("skipped") or []]
    out += [(esc(u["name"]), REASONS.get(u["reason"], "not resolved to one athlete"))
            for u in s.get("unresolved") or []]
    return out


def missing_section(missing):
    if not missing:
        return ""
    items = "".join(f'<li><b>{name}</b> <span>{esc(why)}</span></li>' for name, why in missing)
    return (f'<section class="missing" id="missing"><h2>Not on the sheet &middot; {len(missing)}</h2>'
            f'<p>These names are on the roster but could not be seeded, so the list above leaves them out.</p>'
            f'<ul>{items}</ul></section>')


def table(body, tail):
    return f"""<div class="tbl-wrap">
    <table>
      <thead><tr><th class="rk" scope="col">Seed</th><th scope="col">Athlete</th><th class="pr" scope="col">PR</th><th class="c-season" scope="col">Set</th><th class="c-team" scope="col">School</th><th class="c-team" scope="col">Club</th></tr></thead>
      <tbody>{body}</tbody>
      {tail}
    </table>
  </div>"""


def build(saved, meta, out_dir, theme=THEME):
    """Write <name>.html and <name>.csv into out_dir for one saved sheet, the page in theme. meta:
    {"meet", "label", "athletes"}, and optionally "name" (the file name, slug(meet, label) by
    default) and "index" (see html). Returns (html path, the `ath seed` report)."""
    s = check(saved)
    name = meta.get("name") or slug(meta["meet"], meta["label"])
    out_dir.mkdir(parents=True, exist_ok=True)
    base = out_dir / name
    page = html(s, {**meta, "pulled": saved["pulled"], "csv": saved["csv"], "csv_name": f"{name}.csv"}, theme)
    base.with_suffix(".html").write_text(page)
    base.with_suffix(".csv").write_bytes(saved["csv"].encode("utf-8"))  # ath's bytes, untranslated
    return base.with_suffix(".html"), s


# CSS both seed sheet pages share (this page and meet_index's index page), so the two stay in step:
# plain CSS, which each template takes as its format fields (SHARED_CSS).

# The header: kicker (with the event pages' back link), title and lede.
HEADER_CSS = """header { display: grid; gap: 10px; }
.kicker { margin: 0; font-family: var(--display); font-weight: 700; text-transform: uppercase; letter-spacing: .08em; font-size: 14px; color: var(--muted); }
h1 { font-family: var(--display); font-weight: 900; font-size: clamp(30px, 5vw, 48px); line-height: 1; margin: 0; text-wrap: balance; letter-spacing: -.01em; }
.lede { margin: 0; color: var(--muted); max-width: 64ch; }
.back { color: var(--fg); text-decoration: none; border-bottom: 2px solid var(--highlight); }
.back:hover, .back:focus-visible { color: var(--accent); border-color: var(--accent); outline: none; }"""

# A table in its card: its head and cells.
TABLE_CSS = """.tbl-wrap { background: var(--surface); border: 1px solid var(--line); border-radius: 6px; overflow: hidden; }
table { width: 100%; border-collapse: collapse; font-size: 15px; }
thead th { text-align: left; font-family: var(--display); font-weight: 600; text-transform: uppercase; letter-spacing: .08em; font-size: 12px; color: var(--muted); padding: 10px 12px; border-bottom: 1px solid var(--line); }
td { padding: 9px 12px; border-bottom: 1px solid var(--line); vertical-align: baseline; }"""

# A divider row heading a group of rows.
DIVIDER_CSS = """.divider th { text-align: left; font-family: var(--display); font-weight: 700; text-transform: uppercase; letter-spacing: .08em; font-size: 13px; color: var(--fg); background: var(--seg); padding: 8px 12px; border-bottom: 1px solid var(--line); }"""

# The small print under the table.
NOTES_CSS = """.notes { color: var(--muted); font-size: 13px; max-width: 72ch; display: grid; gap: 6px; }
.notes p { margin: 0; }"""

# The box that stands in for an empty table.
EMPTY_CSS = """.empty { margin: 0; padding: 18px; background: var(--surface); border: 1px dashed var(--muted); border-radius: 6px; color: var(--muted); }"""

SHARED_CSS = {"header_css": HEADER_CSS, "table_css": TABLE_CSS, "divider_css": DIVIDER_CSS,
              "notes_css": NOTES_CSS, "empty_css": EMPTY_CSS}


TEMPLATE = """<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover">
<meta name="robots" content="noindex, nofollow, noarchive">
<title>{title}</title>
{head}
<style>
/* Layout: a results-board header, then the seed list as one table: marks fastest first, then a
   divided group with no mark. Always light: the page ignores the viewer's dark-mode setting. */
{theme_css}
{base_css}
.wrap {{ max-width: 920px; }}
{header_css}
.spot {{ list-style: none; margin: 2px 0 0; padding: 0; display: grid; gap: 4px; font-size: 16px; }}
.hl {{ display: inline-block; padding: 0 .3em; background: var(--highlight); border-radius: 3px; font-family: var(--display); font-weight: 900; font-variant-numeric: tabular-nums; }}
.actions {{ display: flex; flex-wrap: wrap; gap: 10px; }}
.dl {{ display: inline-flex; align-items: center; gap: 8px; font: 700 16px/1 var(--display); letter-spacing: .02em; color: var(--fg); background: var(--seg); border-radius: 6px; padding: 10px 16px; text-decoration: none; }}
.dl:hover {{ background: var(--seg-on); color: var(--seg-on-fg); }}
.dl:focus-visible {{ outline: 2px solid var(--accent); outline-offset: 2px; }}
.dl svg {{ width: 16px; height: 16px; }}
{table_css}
tbody:last-child tr:last-child td {{ border-bottom: 0; }}
.rk {{ width: 3.2em; text-align: right; font-family: var(--display); font-weight: 800; font-size: 17px; font-variant-numeric: tabular-nums; color: var(--muted); }}
thead .rk {{ font-size: 12px; font-weight: 600; }}
.ath {{ font-weight: 600; }}
.pr {{ white-space: nowrap; }}
.mk {{ font-family: var(--display); font-weight: 800; font-size: 18px; font-variant-numeric: tabular-nums; }}
.c-season {{ color: var(--muted); font-variant-numeric: tabular-nums; }}
.c-team {{ color: var(--muted); font-size: 14px; }}
td a {{ color: inherit; text-decoration: none; border-bottom: 1px solid var(--line); }}
td a:hover, td a:focus-visible {{ border-color: var(--accent); color: var(--accent); outline: none; }}
.sub, .ps {{ display: none; }}
tr.me td {{ background: {theme.highlight_row}; }}
tr.me .rk {{ color: var(--fg); box-shadow: inset 4px 0 0 var(--fg); }}
tr.me .mk {{ padding: 0 .2em; background: var(--highlight); border-radius: 3px; }}
{divider_css}
tr.nm .rk {{ font-weight: 600; }}
.none {{ display: inline-block; font-family: var(--display); font-weight: 700; font-size: 13px; text-transform: uppercase; letter-spacing: .06em; color: var(--muted); border: 1px dashed var(--muted); border-radius: 3px; padding: 0 .4em; }}
{notes_css}
button.dl {{ border: 0; cursor: pointer; }}
.copy-msg {{ align-self: center; font-size: 13px; color: var(--muted); }}
.miss-link {{ color: var(--fg); font-weight: 700; text-decoration: underline; text-decoration-color: var(--highlight); text-decoration-thickness: 3px; text-underline-offset: 3px; }}
.missing {{ display: grid; gap: 6px; padding: 12px 14px; background: var(--surface); border: 1px solid var(--line); border-left: 4px solid var(--fg); border-radius: 6px; }}
.missing h2 {{ margin: 0; font-family: var(--display); font-weight: 800; font-size: 20px; }}
.missing p {{ margin: 0; color: var(--muted); font-size: 14px; }}
.missing ul {{ margin: 0; padding: 0; list-style: none; display: grid; gap: 2px; }}
.missing li span {{ color: var(--muted); font-size: 14px; }}
.missing li span::before {{ content: "\\00b7  "; }}
.missing a {{ color: inherit; text-decoration: none; border-bottom: 1px solid var(--line); }}
{empty_css}
.csv {{ min-width: 0; font-size: 13px; color: var(--muted); }}
.csv summary {{ cursor: pointer; font-family: var(--display); font-weight: 700; text-transform: uppercase; letter-spacing: .06em; }}
.csv summary:focus-visible {{ outline: 2px solid var(--accent); outline-offset: 2px; }}
.csv p {{ margin: 6px 0; }}
.csv pre {{ margin: 0; padding: 10px 12px; max-height: 260px; overflow: auto; background: var(--surface); border: 1px solid var(--line); border-radius: 6px; font-size: 12px; line-height: 1.45; color: var(--fg); white-space: pre; }}
@media (max-width: 640px) {{
  .c-season, .c-team {{ display: none; }}
  .sub {{ display: block; font-weight: 400; font-size: 13px; color: var(--muted); margin-top: 2px; }}
  .tw {{ display: inline-block; }}
  .ps {{ display: block; font-size: 12px; color: var(--muted); text-align: right; font-variant-numeric: tabular-nums; }}
  .pr {{ text-align: right; }}
  td, thead th, .divider th {{ padding: 8px 8px; }}
  .rk {{ width: 2.4em; }}
}}{relay_css}
</style>
</head>
<body>
<div class="wrap">
  <header>
    <p class="kicker">{kicker}</p>
    <h1>{label} seed sheet</h1>
    <p class="lede">{lede}</p>
    {spotlight}
    <div class="actions">
      <a class="dl" href="{csv_href}" download="{csv_name}"><svg viewBox="0 0 16 16" aria-hidden="true"><path d="M8 1v9m-4-4 4 4 4-4M2 14h12" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/></svg>Download CSV</a>
      <button class="dl" type="button" id="copy" hidden><svg viewBox="0 0 16 16" aria-hidden="true"><path d="M5 5V2h9v9h-3M2 5h9v9H2z" fill="none" stroke="currentColor" stroke-width="2" stroke-linejoin="round"/></svg>Copy CSV</button>
      <span class="copy-msg" id="copy-msg" role="status"></span>
    </div>
  </header>
  {table}
  {missing}
  <div class="notes">
{notes}
    <p>Data pulled {pulled}.</p>
  </div>
  <details class="csv" id="csv-box">
    <summary>Seed sheet CSV</summary>
    <p>Where the download is blocked (some embedded viewers block downloads), select this text and copy it, or use Copy CSV.</p>
    <pre id="csv">{csv_text}</pre>
  </details>
</div>
<script>
/* Copy CSV: the clipboard when the browser allows it; otherwise the CSV below is opened and
   selected, ready for Ctrl+C or Cmd+C. Without script the button stays hidden. */
(function () {{
  var btn = document.getElementById("copy"), pre = document.getElementById("csv"),
      box = document.getElementById("csv-box"), msg = document.getElementById("copy-msg");
  btn.hidden = false;
  function selectCsv() {{
    box.open = true;
    var range = document.createRange(), sel = window.getSelection();
    range.selectNodeContents(pre);
    sel.removeAllRanges();
    sel.addRange(range);
  }}
  function fallback() {{
    selectCsv();
    var ok = false;
    try {{ ok = document.execCommand("copy"); }} catch (e) {{}}
    msg.textContent = ok ? "Copied" : "Selected below: press Ctrl+C (Cmd+C on a Mac) to copy";
  }}
  btn.addEventListener("click", function () {{
    /* Whichever comes first: the clipboard says yes, or it says no, throws, is missing, or has
       not answered in a second (an embedded frame can leave it hanging). */
    var done = false;
    function copied() {{ if (!done) {{ done = true; msg.textContent = "Copied"; }} }}
    function failed() {{ if (!done) {{ done = true; fallback(); }} }}
    try {{
      navigator.clipboard.writeText(pre.textContent).then(copied, failed);
    }} catch (e) {{
      failed();
      return;
    }}
    setTimeout(failed, 1000);
  }});
}})();
</script>
</body>
</html>
"""
