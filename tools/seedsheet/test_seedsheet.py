"""seedsheet's tests, against a saved `ath seed` report (testdata/ath-seed.json).

That file and testdata/ath-seed.csv are `ath seed --roster tests/roster_seedsheet.csv --event 400m`
on the hand-built fixture profiles in tests/fixtures (made-up people only), as of 2026-06-10 for a meet on 2026-06-13, from one run:
the JSON as `ath seed --json` prints it and the seed sheet CSV it writes. tests/pipeline.rs fails when
they are no longer what ath produces. No test calls ath or Athletic.net. Run: python3 -m unittest
discover -s tools/seedsheet (cargo test runs it too). SEEDSHEET_BLESS=1 rewrites the expected pages
in testdata/expected/.
"""
import copy
import io
import json
import os
import re
import sys
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from datetime import date
from html import unescape
from pathlib import Path
from unittest import mock
from urllib.parse import unquote

import build
import seedsheet
from report import pretty_date

TESTDATA = Path(__file__).parent / "testdata"
SEED = json.loads((TESTDATA / "ath-seed.json").read_text())
SEED_CSV = (TESTDATA / "ath-seed.csv").read_text()
GWEN = 4001  # the configured Athlete (a made-up fixture person, as all of them are)
META = {"meet": "Spring Invitational", "label": "Girls 400m", "athletes": [GWEN]}


CSV_LINES = {line.split(",")[2]: line for line in SEED_CSV.splitlines()[1:]}


def csv_for(seed):
    """ath's CSV lines for a (possibly edited) report's entrants, in its order: what ath would have
    written beside it. Tests only pick lines out of ath's CSV; they never write one."""
    entrants = seed.get("entrants")
    if not isinstance(entrants, list) or not all(isinstance(e, dict) for e in entrants):
        return SEED_CSV  # a malformed report: check refuses it before reading the CSV
    lines = [SEED_CSV.splitlines()[0]]
    for e in entrants:
        lines.append(CSV_LINES.get(str(e["athlete_id"]), f"{e['rank']},x,{e['athlete_id']}"))
    return "\n".join(lines) + "\n"


def saved(seed=None, pulled="2026-06-10", csv_text=None):
    seed = copy.deepcopy(seed or SEED)
    return build.sheet_file(seed, csv_for(seed) if csv_text is None else csv_text, pulled)


def page(seed=None, meta=META):
    sheet = saved(seed)
    return build.html(build.check(sheet), {**meta, "pulled": "2026-06-10", "csv": sheet["csv"], "csv_name": "x.csv"})


def page_order(html):
    return [int(i) for i in re.findall(r'<tr[^>]* data-athlete="(\d+)"', html)]


def csv_order(text):
    return [int(line.split(",")[2]) for line in text.splitlines()[1:]]


class Order(unittest.TestCase):
    def test_the_page_lists_entrants_in_the_seed_sheet_csvs_order(self):
        self.assertEqual(page_order(page()), csv_order(SEED_CSV))
        self.assertEqual(len(csv_order(SEED_CSV)), 8)

    def test_the_csv_export_is_aths_own_csv_byte_for_byte(self):
        self.assertEqual(saved()["csv"], SEED_CSV)
        with tempfile.TemporaryDirectory() as d:
            path, _ = build.build(saved(), META, Path(d))
            self.assertEqual(path.with_suffix(".csv").read_bytes(), (TESTDATA / "ath-seed.csv").read_bytes())
        self.assertFalse(hasattr(build, "csv_text"), "the page carries ath's CSV; it writes none")

    def test_a_csv_that_is_not_the_reports_sheet_is_refused(self):
        lines = SEED_CSV.splitlines(keepends=True)
        swapped = lines[0] + lines[2] + lines[1] + "".join(lines[3:])
        with self.assertRaises(build.SheetError) as e:
            build.check(saved(csv_text=swapped))
        self.assertIn("not the report's sheet", str(e.exception))
        for bad in ("", "rank,athlete\n1,x\n"):
            with self.assertRaises(build.SheetError):
                build.check(saved(csv_text=bad))
        with self.assertRaises(build.SheetError):
            build.check({**saved(), "csv": None})

    def test_the_page_carries_the_same_csv_to_download(self):
        href = re.search(r'class="dl" href="data:text/csv;charset=utf-8,([^"]*)" download="x.csv"', page())
        self.assertIsNotNone(href)
        self.assertEqual(unquote(href.group(1)), SEED_CSV)

    def test_the_page_keeps_aths_order_and_never_resorts(self):
        # Order is ath's job (it sorts on SortInt). Give the page entrants whose display strings
        # would sort differently: it still shows them as ath ranked them.
        s = copy.deepcopy(SEED)
        s["entrants"][0]["pr"], s["entrants"][1]["pr"] = "9:99.99", "1.00"
        self.assertEqual(page_order(page(s)), csv_order(SEED_CSV))

    def test_no_mark_rows_sit_at_the_bottom_marked_as_such(self):
        html = page()
        marks, _, nomarks = html.partition('<tbody class="nomark">')
        self.assertEqual(page_order(nomarks), [1003, 4002, 3003])
        self.assertNotIn("No mark</span>", marks)
        self.assertEqual(nomarks.count('>No mark</span>'), 3)
        self.assertIn("No mark in the 400 Meters &middot; 3", nomarks)
        self.assertIn("3 with no mark are at the bottom.", html)

    def test_a_sheet_where_everyone_has_a_mark_has_no_no_mark_group(self):
        s = copy.deepcopy(SEED)
        s["entrants"] = [e for e in s["entrants"] if e["sort_int"] is not None]
        html = page(s)
        self.assertNotIn("nomark", html)
        self.assertNotIn("at the bottom", html)


class Athletes(unittest.TestCase):
    def test_configured_athletes_are_highlighted(self):
        html = page()
        self.assertEqual(re.findall(r'<tr class="me[^"]*" data-athlete="(\d+)"', html), [str(GWEN)])
        self.assertIn('<span class="hl">56.85</span> <b>Gwen Garcia</b> seeded 3rd of 8', html)

    def test_an_athlete_with_no_mark_is_highlighted_at_the_bottom(self):
        html = page(meta={**META, "athletes": [4002]})
        self.assertIn('<tr class="me nm" data-athlete="4002">', html)
        self.assertIn("<b>Hana Hollis</b> no mark in the 400 Meters yet, so seeded at the bottom (7th of 8)", html)

    def test_no_athletes_no_highlight(self):
        html = page(meta={**META, "athletes": []})
        self.assertNotIn('class="me', html)
        self.assertNotIn('class="spot"', html)

    def test_an_athlete_not_on_the_sheet_is_left_out(self):
        self.assertNotIn('class="spot"', page(meta={**META, "athletes": [999]}))

    def test_ordinals(self):
        self.assertEqual([build.ordinal(n) for n in (1, 2, 3, 4, 11, 12, 13, 21, 22, 101, 111)],
                         ["1st", "2nd", "3rd", "4th", "11th", "12th", "13th", "21st", "22nd", "101st", "111th"])


class Pages(unittest.TestCase):
    def test_pages_match_the_expected_ones(self):
        with tempfile.TemporaryDirectory() as d:
            path, _ = build.build(saved(), META, Path(d))
            self.assertEqual(path.name, "spring-invitational-girls-400m.html")
            for ext in (".html", ".csv"):
                got = path.with_suffix(ext).read_text()
                want = TESTDATA / "expected" / path.with_suffix(ext).name
                if os.environ.get("SEEDSHEET_BLESS"):
                    want.parent.mkdir(exist_ok=True)
                    want.write_text(got)
                self.assertEqual(got, want.read_text(), f"{ext} differs; SEEDSHEET_BLESS=1 rewrites it")

    def test_the_page_keeps_out_of_search_and_names_no_source(self):
        html = page()
        self.assertIn('<meta name="robots" content="noindex, nofollow, noarchive">', html)
        # The page's own text names no source. The CSV's data (in the collapsed CSV box) holds
        # profile URLs, as the download does.
        page_text = re.sub(r'<details class="csv".*?</details>', "", html, flags=re.S)
        visible = re.sub(r"<[^>]+>", " ", re.sub(r"<(style|script)[^>]*>.*?</\1>", "", page_text, flags=re.S))
        self.assertNotIn("athletic.net", visible.lower())
        self.assertNotIn(chr(0x2014), html)  # no em dashes

    def test_the_page_is_light_only_in_the_shared_type(self):
        from report import THEME, attr_url
        html = page()
        self.assertIn('<meta name="color-scheme" content="light only">', html)
        self.assertIn(f"--bg: {THEME.bg};", html)
        self.assertIn(f"--display: {THEME.display};", html)
        self.assertIn(f"--body: {THEME.body};", html)
        self.assertIn(f'<link rel="stylesheet" href="{attr_url(THEME.fonts_url)}">', html)
        self.assertNotIn("prefers-color-scheme", html)

    def test_every_color_is_a_token(self):
        import report
        for path in (build.__file__, seedsheet.__file__):
            self.assertEqual(report.color_literals(path), [],
                             f"{Path(path).name}: a color outside report.THEME; make it a token")
        self.assertIn(f"tr.me td {{ background: {report.THEME.highlight_row}; }}", page())

    def test_the_page_renders_in_another_theme(self):
        import dataclasses
        from report import THEME
        theme = dataclasses.replace(THEME, bg="ivory", highlight_row="khaki", copy_px=17)
        html = build.html(build.check(saved()), {**META, "pulled": "2026-06-10", "csv": saved()["csv"],
                                                 "csv_name": "x.csv"}, theme)
        self.assertIn("--bg: ivory;", html)
        self.assertIn("tr.me td { background: khaki; }", html)
        self.assertIn("font-size: 17px; line-height: 1.5;", html)
        self.assertNotIn(THEME.highlight_row, html)
        self.assertIn(f"--bg: {THEME.bg};", page())  # THEME's page is still THEME's

    def test_entrants_link_to_their_profiles_from_ath(self):
        html = page()
        for e in SEED["entrants"]:
            self.assertIn(f'<a href="{e["athlete_url"]}" target="_blank" rel="noopener">{e["athlete"]}</a>', html)

    def test_names_are_escaped(self):
        s = copy.deepcopy(SEED)
        s["entrants"][0]["athlete"] = "<b>Erin</b> & Co"
        html = page(s)
        self.assertIn("&lt;b&gt;Erin&lt;/b&gt; &amp; Co", html)
        self.assertNotIn("<b>Erin</b>", html)

    def test_everyone_left_off_the_sheet_is_named_with_why(self):
        # The saved report's unresolved row (Ivy Ironwood matches no profile), plus a skipped
        # profile and an ambiguous name: the entrant count never hides them.
        s = copy.deepcopy(SEED)
        s["skipped"] = [{"name": "Kai Kestrel", "athlete_id": 7, "athlete_url": "https://example.test/7", "error": "x"}]
        s["unresolved"].append({"name": "Lou Larkspur", "reason": "ambiguous", "note": "3 possible Profiles"})
        html = page(s)
        self.assertIn('<a class="miss-link" href="#missing">3 names on the roster are not on the sheet.</a>', html)
        missing = html.split('<section class="missing" id="missing">')[1].split("</section>")[0]
        self.assertIn("<h2>Not on the sheet &middot; 3</h2>", missing)
        self.assertEqual(re.findall(r"<li><b>(.*?)</b> <span>(.*?)</span></li>", missing), [
            ('<a href="https://example.test/7" target="_blank" rel="noopener">Kai Kestrel</a>',
             "profile could not be read"),
            ("Ivy Ironwood", "no profile found for this name"),
            ("Lou Larkspur", "more than one possible profile; not yet picked"),
        ])

    def test_one_name_left_off_reads_in_the_singular(self):
        self.assertIn("1 name on the roster is not on the sheet.", page())

    def test_nobody_left_off_no_section(self):
        s = copy.deepcopy(SEED)
        s["unresolved"] = []
        html = page(s)
        self.assertNotIn('id="missing"', html)
        self.assertNotIn("not on the sheet", html)

    def test_counts_read_in_the_singular(self):
        s = copy.deepcopy(SEED)
        s["entrants"] = s["entrants"][:1]
        self.assertIn("1 entrant, seeded", page(s))
        self.assertEqual(build.plural(1, "entrant"), "1 entrant")
        self.assertEqual(build.plural(0, "entrant"), "0 entrants")

    def test_an_empty_sheet_says_so(self):
        s = copy.deepcopy(SEED)
        s["entrants"] = []
        html = page(s)
        self.assertIn("Nobody on the roster could be seeded yet.", html)
        self.assertNotIn("<table>", html)

    def test_the_csv_is_on_the_page_to_copy(self):
        html = page()
        pre = re.search(r'<pre id="csv">(.*?)</pre>', html, flags=re.S).group(1)
        self.assertEqual(unescape(pre), SEED_CSV)
        # The Copy CSV button is shown by the script, so it never shows without one.
        self.assertIn('<button class="dl" type="button" id="copy" hidden>', html)
        self.assertIn("btn.hidden = false;", html)

    def test_slug(self):
        self.assertEqual(build.slug("Spring Invitational", "Girls 400m"), "spring-invitational-girls-400m")
        self.assertEqual(build.slug("CCS Finals!", "Boys 4x400 (A)"), "ccs-finals-boys-4x400-a")


class Check(unittest.TestCase):
    def refused(self, s, says):
        with self.assertRaises(build.SheetError) as e:
            build.check(s)
        self.assertIn(says, str(e.exception))

    def test_a_whole_report_passes(self):
        self.assertEqual(len(build.check(saved())["entrants"]), 8)

    def test_an_unwrapped_report_is_refused(self):
        self.refused(copy.deepcopy(SEED), "is not a saved seed sheet")

    def test_another_schema_is_refused(self):
        s = copy.deepcopy(SEED)
        s["schema"] = 2
        self.refused(saved(s), "schema 2; this build reads schema 1")

    def test_another_kind_is_refused_and_no_kind_is_an_individual_sheet(self):
        self.refused(saved({**SEED, "kind": "medley"}), "of kind 'medley'")
        # Dispatch is on kind, never on keys: a sheet with teams but no kind is an individual one.
        self.refused(saved({**{k: v for k, v in RELAY.items() if k != "kind"}}), "report has no entrants")

    def test_a_missing_field_is_refused(self):
        s = copy.deepcopy(SEED)
        del s["entrants"][2]["sort_int"]
        self.refused(saved(s), "entrants item 3 has no sort_int")

    def test_an_older_saved_format_is_refused_saying_what_to_do(self):
        old = {"format": 1, "pulled": "2026-06-10", "seed": copy.deepcopy(SEED)}
        self.refused(old, "older seedsheet (format 1, without ath's CSV); run seedsheet without --no-fetch")

    def test_a_bad_meet_date_is_refused(self):
        for bad in ("June 13", "2026-13-40", 20260613, ""):
            self.refused(saved({**SEED, "meet_date": bad}), "meet_date is not a YYYY-MM-DD date")
        self.assertIsNone(build.check(saved({**SEED, "meet_date": None}))["meet_date"])

    def test_wrong_field_types_are_refused(self):
        for field, bad in (("athlete_id", '1"><script>alert(1)</script>'), ("rank", "1"), ("rank", True),
                           ("sort_int", 54.5), ("pr_season", "2025"), ("athlete", None), ("pr_numeric", "54.5"),
                           ("school_url", 5)):
            s = copy.deepcopy(SEED)
            s["entrants"][0][field] = bad
            self.refused(saved(s), f"entrants item 1 has the wrong type for {field}")
        s = copy.deepcopy(SEED)
        s["unresolved"][0]["name"] = ["Ivy"]
        self.refused(saved(s), "unresolved item 1 has the wrong type for name")
        self.refused(saved({**SEED, "event": 400}), "event is not text")

    def test_every_value_put_in_the_page_is_escaped(self):
        # Even past check (defense in depth): a hostile id, rank or text is escaped where it lands.
        s = copy.deepcopy(SEED)
        e = s["entrants"][0]
        e["athlete_id"], e["rank"] = '1"><script>x()</script>', '<i>1</i>'
        row = build.row(e, set(), "400 Meters")
        self.assertIn('data-athlete="1&quot;&gt;&lt;script&gt;x()&lt;/script&gt;"', row)
        self.assertIn('<td class="rk">&lt;i&gt;1&lt;/i&gt;</td>', row)
        self.assertNotIn("<script>", row)
        self.assertNotIn("<i>", row)

    def test_a_mark_after_a_no_mark_is_refused(self):
        s = copy.deepcopy(SEED)
        s["entrants"][4], s["entrants"][5] = s["entrants"][5], s["entrants"][4]
        s["entrants"][4]["rank"], s["entrants"][5]["rank"] = 5, 6
        self.refused(saved(s), "has a mark but comes after an entrant with none")

    def test_ranks_out_of_order_are_refused(self):
        s = copy.deepcopy(SEED)
        s["entrants"][0]["rank"] = 2
        self.refused(saved(s), "entrant 1 is ranked 2")

    def test_a_mark_without_its_sortint_is_refused(self):
        s = copy.deepcopy(SEED)
        s["entrants"][0]["sort_int"] = None
        self.refused(saved(s), "without its SortInt")

    def test_any_malformed_shape_is_a_sheet_error(self):
        for bad in (None, [], "x", {"format": 1}, {"format": 1, "seed": {"schema": 1}},
                    saved({**SEED, "entrants": None}), saved({**SEED, "as_of": None}),
                    saved({**SEED, "skipped": [{"name": "x"}]}), saved({**SEED, "unresolved": "x"})):
            with self.assertRaises(build.SheetError, msg=repr(bad)[:60]):
                build.check(bad)


RELAY = json.loads((TESTDATA / "ath-seed-relay.json").read_text())
RELAY_CSV = (TESTDATA / "ath-seed-relay.csv").read_text()
NICO, FINCH = 900100012, 900100018  # made-up runners on Pinecrest's A and B teams
RELAY_META = {"meet": "Riverton Winter Classic", "label": "Boys 4x800m", "athletes": [NICO, FINCH]}


def relay_saved(seed=None, csv_text=RELAY_CSV):
    return build.sheet_file(copy.deepcopy(seed or RELAY), csv_text, "2026-11-06")


def relay_page(seed=None, meta=RELAY_META):
    sheet = relay_saved(seed)
    return build.html(build.check(sheet), {**meta, "pulled": "2026-11-06", "csv": sheet["csv"], "csv_name": "x.csv"})


def team_groups(html):
    """Each team's group on a relay page: its seed, then its runners' ids, in page order."""
    return [(int(re.search(r'<td class="rk">(\d+)</td>', g).group(1)),
             [int(i) for i in re.findall(r'data-athlete="(\d+)"', g)])
            for g in re.findall(r'<tbody class="team[^"]*">(.*?)</tbody>', html)]


class Relay(unittest.TestCase):
    """A relay's page, against testdata/ath-seed-relay.json and .csv: `ath seed --meet` on the made-up
    meet 900700's boys' Varsity 4x800m (tests/pipeline.rs keeps them current)."""

    def refused(self, s, says):
        with self.assertRaises(build.SheetError) as e:
            build.check(s)
        self.assertIn(says, str(e.exception))

    def test_the_relay_page_matches_the_expected_one(self):
        with tempfile.TemporaryDirectory() as d:
            path, _ = build.build(relay_saved(), RELAY_META, Path(d))
            self.assertEqual(path.name, "riverton-winter-classic-boys-4x800m.html")
            for ext in (".html", ".csv"):
                got = path.with_suffix(ext).read_text()
                want = TESTDATA / "expected" / path.with_suffix(ext).name
                if os.environ.get("SEEDSHEET_BLESS"):
                    want.write_text(got)
                self.assertEqual(got, want.read_text(), f"{ext} differs; SEEDSHEET_BLESS=1 rewrites it")
            self.assertEqual(path.with_suffix(".csv").read_text(), RELAY_CSV, "ath's CSV, byte for byte")

    def test_the_4xmile_page_matches_the_expected_one_and_flags_estimated_legs(self):
        mile = json.loads((TESTDATA / "ath-seed-4xmile.json").read_text())
        mile_csv = (TESTDATA / "ath-seed-4xmile.csv").read_text()
        iris = 900100052  # made-up: her mile, converted, beats her 1600m
        meta = {"meet": "Lakeshore Distance Carnival", "label": "Girls 4xMile", "athletes": [iris]}
        with tempfile.TemporaryDirectory() as d:
            path, s = build.build(build.sheet_file(mile, mile_csv, "2026-11-06"), meta, Path(d))
            got = path.read_text()
            want = TESTDATA / "expected" / path.name
            if os.environ.get("SEEDSHEET_BLESS"):
                want.write_text(got)
            self.assertEqual(got, want.read_text(), "differs; SEEDSHEET_BLESS=1 rewrites it")
            self.assertEqual(path.with_suffix(".csv").read_text(), mile_csv, "ath's CSV, byte for byte")
        self.assertIn('<div class="est">estimated from the mile (5:02.10)</div>', got)
        self.assertIn('<div class="est">estimated from the mile (5:10.00)</div>', got)
        self.assertEqual(got.count('<div class="est">'), 2, "only the legs whose mile won")
        self.assertIn('<div class="why">Includes estimated legs</div>', got)
        self.assertEqual(got.count("Includes estimated legs"), 1, "not the team without one")
        self.assertIn("A runner’s mile, converted to the 1600 Meters at even pace, stands in when it is "
                      "faster, marked as an estimate.", got)
        self.assertIn('<li><span class="hl">5:00.35</span> '
                      '<b>Iris Kettering</b> runs leg 2 for Cedar Hollow High A, seeded 1st of 2 (20:18.56); '
                      'the mark is estimated from the mile (5:02.10)</li>', got)
        body = re.sub(r'href="[^"]*"', "", relay_page().split("<body>")[1].split("<details")[0])
        self.assertNotIn("estimate", body.lower(), "a 4x800m has none")

    def test_teams_are_in_aths_order_with_their_runners_in_leg_order(self):
        groups = team_groups(relay_page())
        self.assertEqual([seed for seed, _ in groups], [1, 2, 3, 4])
        want = [[leg["athlete_id"] for leg in t["legs"]] for t in RELAY["teams"]]
        self.assertEqual([ids for _, ids in groups], want)
        csv_teams = [line.split(",")[1:3] for line in RELAY_CSV.splitlines()[1:]]
        self.assertEqual(csv_teams, [[t["team"], t["letter"]] for t in RELAY["teams"]])

    def test_the_aggregate_is_the_seed_mark(self):
        html = relay_page()
        self.assertIn('<td class="pr"><span class="mk">7:59.05</span></td>', html)
        self.assertIn("4 teams, seeded on the sum of their four runners’ Personal records in the 800 Meters "
                      "as of November 6, 2026, fastest first.", html)

    def test_teams_without_an_aggregate_sit_at_the_bottom_saying_why(self):
        html = relay_page()
        divider = html.index("No aggregate &middot; 2</th>")
        self.assertLess(html.index("7:59.05"), divider)
        self.assertLess(divider, html.index(">Pinecrest Academy</a> <span class=\"ltr\">B</span>"))
        self.assertIn('<div class="why">Leg 4 has no 800 Meters mark</div>', html)
        self.assertIn('<div class="why">Leg 4 has no runner</div>', html)
        self.assertIn("No runner entered", html)
        self.assertIn(" 2 missing a runner, or with a runner who has no mark, are at the bottom.", html)

    def test_athletes_are_highlighted_on_the_teams_they_run_on(self):
        html = relay_page()
        me = re.findall(r'<tr class="lg me[^"]*" data-athlete="(\d+)"', html)
        self.assertEqual(sorted(map(int, me)), [NICO, FINCH])
        self.assertIn('<li><span class="hl">2:00.40</span> <b>Nico Vance</b> runs leg 2 for Pinecrest Academy A, '
                      'seeded 2nd of 4 (8:02.55)</li>', html)
        self.assertIn('<li><b>Finch Dorsey</b> runs leg 4 for Pinecrest Academy B with no mark in the 800 Meters '
                      'yet, so the team has no aggregate and is seeded at the bottom (3rd of 4)</li>', html)
        self.assertEqual(build.athlete_lines(RELAY, {NICO}), ["Nico Vance 2:00.40 (leg 2, Pinecrest Academy A, seed 2)"])
        self.assertNotIn(' class="lg me', relay_page(meta={**RELAY_META, "athletes": []}))

    def test_runners_link_to_their_profiles_and_show_grades_and_seasons(self):
        html = relay_page()
        for t in RELAY["teams"]:
            for leg in t["legs"]:
                self.assertIn(f'<a href="{leg["athlete_url"]}" target="_blank" rel="noopener">{leg["athlete"]}</a>', html)
        self.assertIn('<td class="c-grade">12</td><td class="pr"><span class="mk">1:57.40</span>'
                      '<span class="ps">2026</span></td><td class="c-season">2026</td>', html)

    def test_the_relay_page_is_light_only_and_names_no_source(self):
        html = relay_page()
        self.assertIn('<meta name="color-scheme" content="light only">', html)
        self.assertNotIn("prefers-color-scheme", html)
        page_text = re.sub(r'<details class="csv".*?</details>', "", html, flags=re.S)
        visible = re.sub(r"<[^>]+>", " ", re.sub(r"<(style|script)[^>]*>.*?</\1>", "", page_text, flags=re.S))
        self.assertNotIn("athletic.net", visible.lower())
        self.assertNotIn(chr(0x2014), html)

    def test_counts(self):
        self.assertEqual(build.counts(RELAY), "4 teams, 2 with an aggregate")
        self.assertEqual(build.counts(SEED), "8 entrants, 5 with a mark")
        self.assertEqual(build.size(RELAY), "4 teams")

    def test_a_runner_whose_profile_was_not_read_is_listed(self):
        s = copy.deepcopy(RELAY)
        s["skipped"] = [{"name": "Finch Dorsey", "athlete_id": FINCH, "athlete_url": "u", "error": "x"}]
        # ath says why in words (it tells a profile not read from no mark); the page shows it as is.
        s["teams"][2]["no_aggregate_reason"] = "Leg 4 has a profile that could not be read"
        html = relay_page(s)
        self.assertIn('<a class="miss-link" href="#missing">1 runner’s profile could not be read.</a>', html)
        self.assertIn("<h2>Profiles not read &middot; 1</h2>", html)
        self.assertIn('<div class="why">Leg 4 has a profile that could not be read</div>', html)

    def test_a_missing_leg_is_shown_in_its_own_place(self):
        # Entered A1, A2, A4: leg 3 has no runner, and leg 4 is still leg 4.
        t = copy.deepcopy(RELAY["teams"][3])
        t["legs"][2]["leg"], t["missing_legs"] = 4, [3]
        t["no_aggregate_reason"] = "Leg 3 has no runner"
        group = build.team_group(t, set(), "800 Meters")
        self.assertIn('<div class="why">Leg 3 has no runner</div>', group)
        numbers = re.findall(r'<span class="ln"[^>]*>(\d)</span>', group)
        self.assertEqual(numbers, ["1", "2", "3", "4"])
        self.assertLess(group.index("No runner entered"), group.index("Wade Pruitt"))

    def test_the_reason_is_escaped_and_one_runner_on_two_legs_is_no_aggregate(self):
        t = copy.deepcopy(RELAY["teams"][3])
        t["no_aggregate_reason"] = "Same runner on legs 1 and 3 <b>"
        self.assertIn("Same runner on legs 1 and 3 &lt;b&gt;", build.team_group(t, set(), "800 Meters"))
        s = copy.deepcopy(RELAY)
        s["teams"][0]["legs"][2]["athlete_id"] = s["teams"][0]["legs"][0]["athlete_id"]
        self.refused(relay_saved(s), "aggregate is not the sum of its four runners")
        s = copy.deepcopy(RELAY)
        s["teams"][2]["no_aggregate_reason"] = None
        self.refused(relay_saved(s), "has no aggregate without saying why")

    def test_the_mile_is_the_mile_however_it_is_spelled(self):
        for name in ("1 Mile", "Mile", "1 Mile Run", "Mile Run", "1 mile", "  MILE "):
            self.assertTrue(build.is_mile(name), name)
            leg = {"estimated_from": name, "estimated_from_pr": "5:02.10"}
            self.assertEqual(build.estimate_source(leg), "the mile (5:02.10)")
        for name in ("1600 Meters", "2 Mile", "Miles", "", None):
            self.assertFalse(build.is_mile(name), name)

    def test_an_empty_relay_sheet_says_so(self):
        s = {**copy.deepcopy(RELAY), "teams": []}
        sheet = relay_saved(s, RELAY_CSV.splitlines()[0] + "\n")
        html = build.html(build.check(sheet), {**RELAY_META, "pulled": "2026-11-06", "csv": sheet["csv"],
                                               "csv_name": "x.csv"})
        self.assertIn("No team could be seeded yet.", html)

    def test_a_whole_relay_report_passes(self):
        self.assertEqual(len(build.check(relay_saved())["teams"]), 4)

    def test_a_relay_in_another_schema_is_refused(self):
        self.assertEqual(RELAY["kind"], "relay")
        self.refused(relay_saved({**RELAY, "schema": 2}), "schema 2; this build reads schema 1")

    def test_an_aggregate_that_is_not_the_sum_of_its_runners_is_refused(self):
        s = copy.deepcopy(RELAY)
        s["teams"][0]["sort_int"] += 10
        self.refused(relay_saved(s), "team 1 (Harbor View High A)'s aggregate is not the sum")

    def test_a_team_with_an_aggregate_after_one_without_is_refused(self):
        s = copy.deepcopy(RELAY)
        s["teams"][1], s["teams"][2] = s["teams"][2], s["teams"][1]
        s["teams"][1]["rank"], s["teams"][2]["rank"] = 2, 3
        self.refused(relay_saved(s), "has an aggregate but comes after a team with none")

    def test_teams_out_of_order_are_refused(self):
        s = copy.deepcopy(RELAY)
        s["teams"][0]["rank"] = 2
        self.refused(relay_saved(s), "team 1 is ranked 2")

    def test_a_team_not_of_four_legs_is_refused(self):
        s = copy.deepcopy(RELAY)
        s["teams"][3]["missing_legs"] = []
        self.refused(relay_saved(s), "has legs [1, 2, 3] and missing legs [], not legs 1 to 4 once each")
        s["teams"][3]["missing_legs"] = [3]
        self.refused(relay_saved(s), "has legs [1, 2, 3] and missing legs [3]")

    def test_a_relay_csv_that_is_not_the_reports_sheet_is_refused(self):
        lines = RELAY_CSV.splitlines()
        swapped = "\n".join([lines[0], lines[2], lines[1], *lines[3:]]) + "\n"
        self.refused(relay_saved(csv_text=swapped), "teams, runners or order differ")
        self.refused(relay_saved(csv_text=SEED_CSV), "no team, letter and runner id columns")

    def test_wrong_leg_types_are_refused(self):
        for field, bad in (("athlete_id", "1"), ("grade", "12"), ("sort_int", 1.5), ("pr", None)):
            s = copy.deepcopy(RELAY)
            s["teams"][0]["legs"][0][field] = bad
            self.refused(relay_saved(s), f"team 1's legs item 1 has the wrong type for {field}")
        s = copy.deepcopy(RELAY)
        s["teams"][0]["legs"] = "x"
        self.refused(relay_saved(s), "teams item 1 has the wrong type for legs")
        self.refused(relay_saved({**RELAY, "seeding_event": 800}), "seeding event is not text")

    def test_every_value_put_in_a_relay_page_is_escaped(self):
        s = copy.deepcopy(RELAY)
        t = s["teams"][0]
        t["rank"], t["letter"] = "<i>1</i>", "<b>A</b>"
        t["legs"][0]["athlete_id"], t["legs"][0]["grade"] = '1"><script>x()</script>', "<u>12</u>"
        group = build.team_group(t, set(), "800 Meters")
        self.assertNotIn("<script>", group)
        for raw in ("<i>", "<b>A", "<u>"):
            self.assertNotIn(raw, group)


def write_config(d, **over):
    cfg = {"meet": "Spring Invitational", "meet_date": "2026-06-13", "athletes": [GWEN],
           "output_dir": "out", "data_dir": "data",
           "events": [{"label": "Girls 400m", "event": "400m", "roster": "girls.csv"}]}
    cfg.update(over)
    path = Path(d) / "seedsheet.config.json"
    path.write_text(json.dumps(cfg))
    return path


class Run(unittest.TestCase):
    """seedsheet.py end to end with `ath seed` stubbed out (no ath, no network)."""

    def run_main(self, d, stdout, returncode=0, args=(), csv_out=SEED_CSV, **config):
        """Run seedsheet with a stand-in for ath that prints stdout, exits returncode, and (as ath
        seed does when it has a sheet) writes csv_out to its --out file."""
        calls = []

        def fake_run(cmd, **kw):
            calls.append(cmd)
            if returncode == 0 and csv_out is not None:
                Path(cmd[cmd.index("--out") + 1]).write_bytes(csv_out.encode("utf-8"))
            return mock.Mock(returncode=returncode, stdout=stdout)

        out = io.StringIO()
        with mock.patch("subprocess.run", fake_run), \
                mock.patch.object(seedsheet, "older_place", lambda *parts: Path(d) / "older"), \
                mock.patch.object(seedsheet, "find_ath", lambda: "ath"), redirect_stdout(out), \
                redirect_stderr(io.StringIO()):
            try:
                seedsheet.main(["--config", str(write_config(d, **config)), *args])
            except SystemExit as e:
                out.write(f"\nexit: {e}")
        return calls, out.getvalue()

    def test_runs_ath_seed_and_builds_the_page(self):
        with tempfile.TemporaryDirectory() as d:
            calls, out = self.run_main(d, json.dumps(SEED))
            self.assertEqual(calls, [["ath", "seed", "--roster", str(Path(d).resolve() / "girls.csv"),
                                      "--event", "400m", "--out",
                                      str(Path(d).resolve() / "data" / "spring-invitational-girls-400m.ath.csv"),
                                      "--json", "--meet-date", "2026-06-13"]])
            page_path = Path(d) / "out" / "spring-invitational-girls-400m.html"
            self.assertTrue(page_path.exists(), out)
            self.assertEqual((Path(d) / "out" / "spring-invitational-girls-400m.csv").read_text(), SEED_CSV)
            kept = json.loads((Path(d) / "data" / "spring-invitational-girls-400m.json").read_text())
            self.assertEqual(kept, build.sheet_file(SEED, SEED_CSV, date.today().isoformat()))
            self.assertIn("Gwen Garcia 56.85 (seed 3)", out)

    def test_a_shared_page_is_named_not_republished(self):
        url = "https://example.com/shared/girls-400m"
        events = [{"label": "Girls 400m", "event": "400m", "roster": "girls.csv", "artifact_url": url}]
        with tempfile.TemporaryDirectory() as d:
            _, out = self.run_main(d, json.dumps(SEED), events=events)
            self.assertIn(f"This config's shared page: {url} (republish it there only if you share this page)", out)
            self.assertNotIn("Publish it to", out)

    def test_a_failed_run_keeps_the_saved_sheet(self):
        with tempfile.TemporaryDirectory() as d:
            self.run_main(d, json.dumps(SEED))
            before = (Path(d) / "data" / "spring-invitational-girls-400m.json").read_text()
            _, out = self.run_main(d, "", returncode=1)
            self.assertIn("Girls 400m: ath seed failed (exit 1)\n", out)
            self.assertIn(f"Girls 400m: kept the sheet saved on {pretty_date(date.today().isoformat())}", out)
            self.assertEqual((Path(d) / "data" / "spring-invitational-girls-400m.json").read_text(), before)
            self.assertIn("Wrote", out)

    def test_output_that_is_not_a_report_is_not_saved(self):
        with tempfile.TemporaryDirectory() as d:
            _, out = self.run_main(d, "not json")
            self.assertIn("printed no JSON report", out)
            self.assertIn("exit: No saved sheets yet", out)
            _, out = self.run_main(d, json.dumps({**SEED, "schema": 9}))
            self.assertIn("not saved, the sheet holds `ath seed` JSON schema 9", out)

    def test_no_csv_from_ath_is_not_saved_and_a_stale_one_is_never_used(self):
        with tempfile.TemporaryDirectory() as d:
            data = Path(d) / "data"
            data.mkdir()
            stale = data / "spring-invitational-girls-400m.ath.csv"
            stale.write_text(SEED_CSV)
            _, out = self.run_main(d, json.dumps(SEED), csv_out=None)
            self.assertIn("not saved, ath seed left no readable CSV", out)
            self.assertFalse(stale.exists())
            self.assertFalse((data / "spring-invitational-girls-400m.json").exists())

    def test_nobody_resolved_is_not_saved_and_says_why(self):
        # ath seed prints an empty sheet when no roster row resolves; seedsheet keeps what it had
        # and names the rows and why.
        empty = {**copy.deepcopy(SEED), "entrants": [], "event_name": None}
        with tempfile.TemporaryDirectory() as d:
            _, out = self.run_main(d, json.dumps(empty))
            self.assertIn("not saved, nobody on the roster could be seeded "
                          "(Ivy Ironwood: no Profile matches the name; pin an athlete_id)", out)
            self.assertFalse((Path(d) / "data" / "spring-invitational-girls-400m.json").exists())
            self.assertIn("exit: No saved sheets yet", out)

    def test_the_run_counts_who_is_not_on_the_sheet(self):
        with tempfile.TemporaryDirectory() as d:
            _, out = self.run_main(d, json.dumps(SEED))
            self.assertIn("(8 entrants, 5 with a mark, 1 not on the sheet)", out)

    def test_a_malformed_saved_sheet_is_left_out_not_a_crash(self):
        with tempfile.TemporaryDirectory() as d:
            data = Path(d) / "data"
            data.mkdir()
            sheet = data / "spring-invitational-girls-400m.json"
            for bad, says in (("{not json", "is not valid JSON"),
                              (json.dumps(saved({**SEED, "entrants": 5})), "entrants is not a list of objects"),
                              (json.dumps(saved({**SEED, "entrants": [1, 2]})), "entrants is not a list of objects"),
                              (json.dumps(saved({**SEED, "as_of": 20260610})), "as_of is not a YYYY-MM-DD date"),
                              (json.dumps(saved({**SEED, "meet_date": "June 13"})), "meet_date is not a YYYY-MM-DD date"),
                              (json.dumps({"format": 2, "pulled": "2026-06-10", "seed": []}), "holds `ath seed` JSON schema None"),
                              (json.dumps({"format": 1, "seed": SEED}), "older seedsheet (format 1")):
                sheet.write_text(bad)
                _, out = self.run_main(d, "", args=["--no-fetch"])
                self.assertIn(says, out)
                self.assertIn("left out", out)
                self.assertIn("exit: No saved sheets yet", out)

    def test_no_fetch_builds_from_saved_sheets_only(self):
        with tempfile.TemporaryDirectory() as d:
            self.run_main(d, json.dumps(SEED))
            calls, out = self.run_main(d, "", args=["--no-fetch"])
            self.assertEqual(calls, [])
            self.assertIn("Wrote", out)

    def test_refresh_is_passed_to_ath(self):
        with tempfile.TemporaryDirectory() as d:
            calls, _ = self.run_main(d, json.dumps(SEED), args=["--refresh"])
            self.assertEqual(calls[0][-1], "--refresh")

    def test_a_bad_config_says_what_is_wrong(self):
        with tempfile.TemporaryDirectory() as d:
            for over, says in (({"events": []}, "'events' lists no events"),
                               ({"athletes": ["x"]}, "'athletes' must be a list of athlete ids"),
                               ({"meet_date": "June 13"}, "'meet_date' must be YYYY-MM-DD"),
                               ({"events": [{"label": "A", "event": "400m"}]}, "event 1 needs 'roster'"),
                               ({"events": [{"label": "A", "event": "400m", "roster": "r"},
                                            {"label": "a", "event": "800m", "roster": "r"}]},
                                "two events would write the same page")):
                with self.assertRaises(SystemExit) as e:
                    seedsheet.load_config(write_config(d, **over))
                self.assertIn(says, str(e.exception))

    def test_the_example_config_loads(self):
        cfg = seedsheet.load_config(Path(__file__).parent / "seedsheet.config.example.json")
        self.assertEqual([seedsheet.name_of(cfg, ev) for ev in cfg["events"]], ["spring-invitational-girls-400m"])

    def test_a_missing_config_says_to_make_its_folder(self):
        with tempfile.TemporaryDirectory() as d:
            path = Path(d) / "local" / "seedsheet" / "seedsheet.config.json"
            with self.assertRaises(SystemExit) as e:
                seedsheet.load_config(path)
            self.assertIn(f"mkdir -p {path.parent}", str(e.exception))
            # Both examples, and when to use each.
            self.assertIn("seedsheet.meet.config.example.json\n    meet mode: a meet's id", str(e.exception))
            self.assertIn("seedsheet.config.example.json\n    roster mode: the meet's name and a roster CSV",
                          str(e.exception))

    def test_sheets_left_in_the_old_place_stop_the_run_with_what_to_move(self):
        with tempfile.TemporaryDirectory() as d:
            old = Path(d) / "old"
            cfg = seedsheet.load_config(write_config(d))
            seedsheet.check_moved_data(cfg, old)  # nothing anywhere: a first run
            old.mkdir()
            (old / "other-meet-boys-800m.json").write_text("{}")
            seedsheet.check_moved_data(cfg, old)  # another meet's sheet: not this config's
            for name in ("spring-invitational-girls-400m.json", "spring-invitational-girls-400m.ath.csv"):
                (old / name).write_text("x")
            with self.assertRaises(SystemExit) as e:
                seedsheet.check_moved_data(cfg, old)
            here = old.resolve()
            self.assertIn(f"mkdir -p {cfg['data_dir']} && mv {here}/spring-invitational-girls-400m.json "
                          f"{here}/spring-invitational-girls-400m.ath.csv {cfg['data_dir']}/", str(e.exception))
            cfg["data_dir"].mkdir()
            (cfg["data_dir"] / "spring-invitational-girls-400m.json").write_text("{}")
            seedsheet.check_moved_data(cfg, old)  # moved: fine

    def test_rosters_data_and_output_sit_beside_the_config(self):
        with tempfile.TemporaryDirectory() as d:
            config = Path(d) / "local" / "seedsheet"
            config.mkdir(parents=True)
            path = write_config(config)
            cfg = json.loads(path.read_text())
            del cfg["data_dir"]  # the default
            path.write_text(json.dumps(cfg))
            cfg = seedsheet.load_config(path)
            here = config.resolve()
            self.assertEqual((cfg["output_dir"], cfg["data_dir"], cfg["events"][0]["roster"]),
                             (here / "out", here / "data", here / "girls.csv"))


if __name__ == "__main__":
    sys.exit(unittest.main())
