"""rankmap's tests, against a saved `ath rank` report (testdata/ath-rank-2099.json).

That file is `ath rank 982389051 --event 400m --season 2099 --in CA,CCS --ahead all --json` on the
anonymized fixtures in tests/fixtures (California and its Central Coast Section, ADR 0010: invented
people, the fictional season 2099); tests/rank.rs fails when it is no longer what ath prints. The
pages take their State Meet standards from testdata/areas/ (`areas_dir`, see report.state_areas):
the anonymizer's (`--areas-out`), mapped as the fixtures' marks are, the seasons moved. Run: python3 -m unittest discover
-s tools/rankmap (cargo test runs it too). RANKMAP_BLESS=1 rewrites the expected pages in
testdata/expected/.
"""
import copy
import io
import json
import os
import re
import shlex
import shutil
import stat
import subprocess
import sys
import tempfile
import textwrap
import unittest
from contextlib import redirect_stderr, redirect_stdout
from datetime import date
from pathlib import Path
from unittest import mock

import build
import rankmap

TESTDATA = Path(__file__).parent / "testdata"
RANK = json.loads((TESTDATA / "ath-rank-2099.json").read_text())
CA, CCS = 985316289, 996953819
SEASON = 2099


# The saved report's State Meet standards: its seasons and marks go with the anonymizer's mapped
# standards, which every test on it passes as `areas_dir` (the rest of each Area file is areas/'s).
AREAS = TESTDATA / "areas"


class AnonymizedAreas(unittest.TestCase):
    """Tests on the saved report, built with areas_dir=AREAS."""


def saved(rank=None, pulled="2099-10-30"):
    return build.season_file(copy.deepcopy(rank or RANK), pulled)


def labels(view, abbr):
    return next(s["label"] for s in view["rows"] if s["abbr"] == abbr)


class Prepare(AnonymizedAreas):
    def test_ranks_match_ath_and_rankmaps_counts(self):
        sd = build.prepare({SEASON: saved()}, areas_dir=AREAS)[str(SEASON)]
        self.assertEqual(sd["pr"], "58.98")
        self.assertEqual([v["label"] for v in sd["views"]], ["9th", "9th-10th", "9th-11th", "All HS"])
        self.assertEqual([labels(v, "CA") for v in sd["views"]], ["42", "109", "T195", "T303"])
        self.assertEqual([next(s["label"] for s in v["areas"] if s["abbr"] == "CCS") for v in sd["views"]],
                         ["6", "12", "20", "36"])
        self.assertEqual((sd["home"], sd["area_file"]["label"]), ("CA", "Section"))

    def test_everyone_ahead_is_listed_past_rank_50(self):
        all_hs = build.prepare({SEASON: saved()}, areas_dir=AREAS)[str(SEASON)]["views"][-1]
        ca = next(s for s in all_hs["rows"] if s["abbr"] == "CA")
        self.assertEqual((ca["rank"], len(ca["ahead"]), len(ca["ties"])), (303, 302, 1))
        first = ca["ahead"][0]
        self.assertEqual(set(first), {"name", "mark", "team", "grade", "url"})
        self.assertTrue(first["url"].startswith("https://www.athletic.net/athlete/"))
        self.assertEqual(ca["url"], f"https://www.athletic.net/TrackAndField/rankings/list/{CA}/f/400m?grades=9,10,11,12")

    def test_states_sort_best_rank_first(self):
        view = build.prepare({SEASON: saved()}, areas_dir=AREAS)[str(SEASON)]["views"][0]
        self.assertEqual([s["abbr"] for s in view["rows"]], ["CA"])
        self.assertEqual([(s["abbr"], s["name"]) for s in view["areas"]], [("CCS", "Central Coast")])


class Check(AnonymizedAreas):
    def refused(self, s, says):
        with self.assertRaises(build.SeasonError) as e:
            build.check(s)
        self.assertIn(says, str(e.exception))

    def test_a_whole_report_passes(self):
        self.assertEqual(build.check(saved())["athlete"]["first_name"], "Jamie")

    def test_a_legacy_collector_file_is_refused(self):
        legacy = {"season": 2026, "athleteId": 1, "pr": {"display": "56.85", "sort": 56850}, "views": []}
        self.refused(legacy, "retired browser collector")

    def test_an_unwrapped_report_is_not_called_a_collector_file(self):
        self.refused(copy.deepcopy(RANK), "is not a saved season (no format)")

    def test_a_list_that_is_not_a_states_is_refused(self):
        r = copy.deepcopy(RANK)
        r["results"][0]["list"]["kind"] = "list"
        self.refused(saved(r), "has lists that are neither a State's nor an Area's: California")

    def test_grade_groups_on_different_lists_are_refused(self):
        r = copy.deepcopy(RANK)
        r["results"] = [x for x in r["results"] if not (x["list"]["kind"] == "state" and x["grades"] == [9])]
        self.refused(saved(r), "does not have the same State lists for every grade group")

    def test_results_outside_the_grade_groups_are_refused(self):
        r = copy.deepcopy(RANK)
        r["groups"] = r["groups"][1:]
        self.refused(saved(r), "has results for grades [9], not one of its grade groups")

    def test_the_expected_season_athlete_event_and_state_count(self):
        want = {"athlete_id": 982389051, "event": "400m", "season": SEASON, "state_lists": 1}
        build.check(saved(), want)
        build.check(saved(), dict(want, event="400 Meters"))  # the full name, as ath also takes it
        with self.assertRaises(build.SeasonError) as e:
            build.check(saved(), dict(want, season=2098))
        self.assertIn("is athlete 982389051 400m 2099, not athlete 982389051 400m 2098", str(e.exception))
        with self.assertRaises(build.SeasonError) as e:
            build.check(saved(), dict(want, state_lists=51))
        self.assertIn("has 1 of the 51 State lists", str(e.exception))

    def test_a_file_of_another_shape_is_refused_not_a_crash(self):
        self.refused([], "is not a saved season")
        r = copy.deepcopy(RANK)
        r["athlete"] = None
        with self.assertRaises(build.SeasonError):
            build.check(saved(r), {"athlete_id": 1, "event": "400m", "season": SEASON, "state_lists": 1})
        self.refused({"format": 2}, "is not a whole ath rank report (KeyError")
        r = copy.deepcopy(RANK)
        r["results"][0]["place"] = None
        self.refused(saved(r), "is not a whole ath rank report (TypeError")

    def test_another_schema_is_refused(self):
        r = copy.deepcopy(RANK)
        r["schema"] = 2
        self.refused(saved(r), "schema 2; this rankmap reads 1")

    def test_no_mark_is_refused(self):
        r = copy.deepcopy(RANK)
        r.update(status="no_mark", mark=None, note="no 2099 400m mark on the California list yet")
        self.refused(saved(r), "no 2099 400m mark")

    def test_a_list_not_read_is_refused(self):
        r = copy.deepcopy(RANK)
        r["results"][5].update(status="lookup_failed", place=None, note="HTTP 429")
        self.refused(saved(r), "1 list(s) not read: Central Coast 9th-10th")

    def test_a_cut_neighborhood_is_refused(self):
        r = copy.deepcopy(RANK)
        r["results"][-1]["place"]["neighborhood"]["skipped"] = 98
        self.refused(saved(r), "--ahead all")


class Pages(AnonymizedAreas):
    def test_pages_match_the_expected_ones(self):
        expected = TESTDATA / "expected"
        with tempfile.TemporaryDirectory() as tmp:
            html, _ = build.build({SEASON: saved()}, [2099, 2100, 2101, 2102], Path(tmp), areas_dir=AREAS)
            self.assertEqual(html.name, "982389051-400m.html")
            for ext in (".html", ".md", ".csv"):
                got = html.with_suffix(ext).read_text()
                want = expected / f"982389051-400m{ext}"
                if os.environ.get("RANKMAP_BLESS"):
                    expected.mkdir(exist_ok=True)
                    want.write_text(got)
                self.assertEqual(got, want.read_text(), f"{ext} differs; RANKMAP_BLESS=1 rewrites it")

    def test_a_rank_past_four_digits_shrinks_into_the_four_digit_spot(self):
        # Ranks of 10000 or worse are past the spotted shapes (one to four digits): they take
        # the four-digit spot, as much smaller as they are wider, with the same middle.
        spots = {"Texas": {"4": [100.0, 200.0, 40.0], "t4": [100.0, 200.0, 36.0]}}
        x, y, size = build.spot(spots, "Texas", "10234")
        k = build.label_width("4") / build.label_width("5")
        self.assertEqual((x, size), (100.0, round(40 * k, 1)))
        drop = build.THEME.metrics.ink_drop_em
        self.assertAlmostEqual(y + drop * size, 200 + drop * 40, delta=0.1)
        self.assertEqual(build.spot(spots, "Texas", "T10234")[2], round(36 * build.label_width("t4") / build.label_width("t5"), 1))
        self.assertEqual(build.spot(spots, "Texas", "1234"), [100.0, 200.0, 40.0])

    def test_a_tie_shows_its_t_as_a_superscript(self):
        self.assertEqual(build.rank_html("T12"), '<span class="sup">t</span>12')
        self.assertEqual(build.rank_html("12"), "12")
        self.assertEqual(build.rank_svg("T12"), '<tspan class="sup" dy="-.318em">t</tspan><tspan dy=".191em">12</tspan>')
        self.assertEqual(build.rank_svg("12"), "12")

    def test_the_page_keeps_out_of_search_and_names_no_source(self):
        # Aggregated lists are not for search engines, and the page names the source only in its links.
        with tempfile.TemporaryDirectory() as tmp:
            html, _ = build.build({SEASON: saved()}, [SEASON], Path(tmp), areas_dir=AREAS)
            page = html.read_text()
        self.assertIn('<meta name="robots" content="noindex, nofollow, noarchive">', page)
        self.assertNotIn("athletic.net", re.sub(r'(?:href="|https?://)[^"\s]*', "", page).lower())

    def test_the_page_takes_its_links_from_ath(self):
        with tempfile.TemporaryDirectory() as tmp:
            html, _ = build.build({SEASON: saved()}, [SEASON], Path(tmp), areas_dir=AREAS)
            page = html.read_text()
        self.assertIn(f'href="https://www.athletic.net/TrackAndField/rankings/list/{CA}/f/400m?grades=9"', page)
        self.assertIn('href="https://www.athletic.net/athlete/982389051/track-and-field"', page)

    def test_a_missing_grade_is_left_out_not_shown_as_none(self):
        r = copy.deepcopy(RANK)
        for x in r["results"]:
            n = x["place"]["neighborhood"]
            for a in n["ahead"] + n["tied"]:
                a["grade"] = None
        with tempfile.TemporaryDirectory() as tmp:
            html, _ = build.build({SEASON: saved(r)}, [SEASON], Path(tmp), areas_dir=AREAS)
            page, table = html.read_text(), html.with_suffix(".csv").read_text()
        self.assertNotIn("None", page)
        self.assertNotIn("(gr ", table)


def first_in_ccs():
    """The saved report with her first on the Central Coast's 9th-grade list (made up: the
    anonymized subject is 6th there), for what a page says about a first."""
    r = copy.deepcopy(RANK)
    x = next(x for x in r["results"] if x["list"]["id"] == CCS and x["grades"] == [9])
    x["place"].update(rank=1, label="1", tied=False, ahead=0, ties=0)
    x["place"]["neighborhood"].update(ahead=[], skipped=0, tied=[])
    return r


def without_areas():
    r = copy.deepcopy(RANK)
    r["results"] = [x for x in r["results"] if x["list"]["kind"] == "state"]
    return r


def build_page(rank=None):
    with tempfile.TemporaryDirectory() as tmp:
        html, _ = build.build({SEASON: saved(rank)}, [SEASON], Path(tmp), areas_dir=AREAS)
        return html.read_text(), html.with_suffix(".csv").read_text(), html.with_suffix(".md").read_text()


ME, ME_A = 954056988, 900601


def maine():
    """The saved report moved to Maine and made up: California becomes Maine, Central Coast its
    Class A, with Classes B and C copies of it ranked 15th and 30th, and the Athlete a made-up
    Gwen Garcia whose school is in Class A."""
    r = copy.deepcopy(RANK)
    r["athlete"].update(id=4001, name="Gwen Garcia", first_name="Gwen",
                        url="https://www.athletic.net/athlete/4001/track-and-field")
    r["mark"].update(team="Willow Creek High", team_id=4100)
    r["mark"]["list"].update(id=ME, name="Maine")
    r["home_area"] = {"id": ME_A, "name": "A", "level": "Class", "state": "ME", "season": SEASON,
                      "url": f"https://www.athletic.net/TrackAndField/rankings/list/{ME_A}/f/400m"}
    results = []
    for x in r["results"]:
        if x["list"]["kind"] == "state":
            x["list"].update(id=ME, name="Maine", state="ME")
            x["url"] = x["url"].replace(str(CA), str(ME))
            results.append(x)
            continue
        for i, (name, rank) in enumerate((("A", None), ("B", 15), ("C", 30))):
            y = copy.deepcopy(x)
            y["list"].update(id=ME_A + i, name=name, state="ME", level="Class")
            y["url"] = x["url"].replace(str(CCS), str(ME_A + i))
            if rank:
                y["place"].update(rank=rank, label=str(rank), tied=False)
            results.append(y)
    r["results"] = results
    return r


class Classes(AnonymizedAreas):
    """A home State whose Areas are not places (Maine's Classes, areas/ME.json): cards, not a map."""

    def test_the_state_meet_is_the_own_classs_standards(self):
        # Gwen's 58.98 against the 2099 Class A girls' 400m standards of the anonymized copy
        # (1:03.80 / 1:05.80; Class B's are 1:04.80 / 1:06.80).
        sd = build.prepare({SEASON: saved(maine())}, areas_dir=AREAS)[str(SEASON)]
        sm = sd["state_meet"]
        self.assertEqual((sm["kind"], sm["area_name"], sm["automatic"], sm["provisional"], sm["meets"]),
                         ("standards", "Class A", "1:03.80", "1:05.80", "automatic"))
        self.assertEqual(sm["seeds"]["9"], (6, "6"))
        page, _, md = build_page(maine())
        self.assertIn("<h3>MPA Class A State Meet, 2099</h3>", page)
        self.assertIn("Automatic <b>1:03.80</b> &middot; provisional <b>1:05.80</b>. "
                      "58.98 meets the automatic standard.", page)
        self.assertIn('<p class="smseed" data-season="2099" data-view="9">Against 9th graders, Gwen would be '
                      'seeded about <b>#6</b> in Class A.</p>', page)
        self.assertIn("Maine sends 6 per event to the New England Championship", page)
        # Only the own Class: no other Class's standards on the page.
        self.assertNotIn("1:04.80", page)
        self.assertIn("### MPA Class A State Meet", md)
        self.assertIn("- Automatic 1:03.80, provisional 1:05.80: 58.98 meets the automatic standard.", md)
        self.assertIn("- Against all high schoolers, Gwen is #36 on the Class A list, past the field of 32.", md)
        # Where the standards come from, from the file's notes.
        self.assertIn("<summary>About these standards</summary>", page)
        self.assertIn("printed the 2025 standards for some events", page)
        self.assertIn("Only regular-season and league championship marks count", page)

    def test_the_standards_line_says_which_standard_the_mark_meets(self):
        sm = {"meets": "provisional", "field": 32}
        self.assertEqual(build.standards_line(sm, "1:05.00"),
                         "1:05.00 meets the provisional standard: in if the field of 32 has room for it after the "
                         "automatic qualifiers.")
        self.assertEqual(build.standards_line({"meets": None}, "1:09.00"),
                         "1:09.00 does not meet the provisional standard.")
        r = maine()
        r["mark"].update(mark="1:05.00", sort_int=65000)
        self.assertEqual(build.prepare({SEASON: saved(r)}, areas_dir=AREAS)[str(SEASON)]["state_meet"]["meets"], "provisional")

    def test_no_visible_athletic_net_text(self):
        # Links to its lists and profiles stay; the words never show (robots noindex pages).
        for page in (build_page()[0], build_page(maine())[0]):
            text = re.sub(r"<script>.*?</script>|<style>.*?</style>|<[^>]+>", " ", page, flags=re.S)
            self.assertNotIn("Athletic.net", text)

    def test_no_seed_past_the_field_or_short_of_the_provisional_standard(self):
        sm = {"meets": "automatic", "field": 32, "area_name": "Class B"}
        self.assertEqual(build.seed_text(sm, (40, "40"), "all high schoolers", "Gwen", build.rank_text),
                         "Against all high schoolers, Gwen is #40 on the Class B list, past the field of 32.")
        self.assertEqual(build.seed_text(sm, (32, "T32"), "9th graders", "Gwen", build.rank_text),
                         "Against 9th graders, Gwen would be seeded about T32 in Class B.")
        self.assertEqual(build.seed_text(dict(sm, meets=None), (3, "3"), "9th graders", "Gwen", build.rank_text),
                         "Gwen would not be in the Class B field.")
        r = maine()
        r["mark"].update(mark="1:10.00", sort_int=70000)
        page, _, md = build_page(r)
        self.assertIn("1:10.00 does not meet the provisional standard.", page)
        self.assertIn("Gwen would not be in the Class A field.", page)
        self.assertNotIn("would be seeded", page + md)

    def test_a_field_events_standards_run_the_other_way(self):
        # Girls' Class A high jump 2099: automatic 4-10, provisional 4-08 (as published: the
        # fixtures have no high jump to move them with). ath's field SortInt is
        # 20_000_000 - inches x 1000 (src/mark.rs), so a higher jump has the smaller SortInt.
        r = maine()
        r["event"], r["event_name"] = "HJ", "High Jump"
        for height, meets in (("4-11", "automatic"), ("4-09", "provisional"), ("4-06", None)):
            feet, inches = height.split("-")
            r["mark"].update(mark=height, sort_int=20_000_000 - (int(feet) * 12 + int(inches)) * 1000)
            sm = build.prepare({SEASON: saved(r)}, areas_dir=AREAS)[str(SEASON)]["state_meet"]
            self.assertEqual((sm["automatic"], sm["provisional"], sm["meets"]), ("4-10", "4-08", meets), height)

    def test_a_home_area_of_another_state_or_season_is_not_the_athletes_class(self):
        for change in ({"state": "NH"}, {"season": 2098}):
            r = maine()
            r["home_area"].update(change)
            sd = build.prepare({SEASON: saved(r)}, areas_dir=AREAS)[str(SEASON)]
            self.assertIsNone(sd["home_area"], change)
            self.assertIsNone(sd["state_meet"], change)

    def test_no_state_meet_without_standards_or_a_home_class(self):
        r = maine()
        r["season"] = 2100  # no 2100 standards yet
        self.assertIsNone(build.prepare({2100: saved(r)}, areas_dir=AREAS)["2100"]["state_meet"])
        r = maine()
        r["home_area"] = None
        sd = build.prepare({SEASON: saved(r)}, areas_dir=AREAS)[str(SEASON)]
        self.assertIsNone(sd["state_meet"])
        page, _, _ = build_page(r)
        self.assertNotIn('class="smeet"', page)

    def test_an_area_the_file_does_not_name_is_refused(self):
        r = maine()
        for x in r["results"]:
            if x["list"]["name"] == "C":
                x["list"]["name"] = "D"
        with self.assertRaises(build.SeasonError) as e:
            build.check(saved(r))
        self.assertIn("has ME Area lists that areas/ME.json does not name: D", str(e.exception))

    def test_another_states_area_lists_are_not_checked_against_the_home_file(self):
        r = maine()
        for x in r["results"]:
            if x["list"]["name"] == "C":
                x["list"].update(name="Central Coast", state="CA", level="Section")
        build.check(saved(r))

    def test_classes_read_as_their_label_and_name(self):
        sd = build.prepare({SEASON: saved(maine())}, areas_dir=AREAS)[str(SEASON)]
        self.assertEqual((sd["home"], sd["home_area"]), ("ME", "A"))
        self.assertEqual([(s["abbr"], s["name"]) for s in sd["views"][0]["areas"]],
                         [("A", "Class A"), ("B", "Class B"), ("C", "Class C")])

    def test_the_page_has_class_cards_and_no_map(self):
        page, csv, md = build_page(maine())
        self.assertIn('aria-pressed="false">ME Classes</button>', page)
        self.assertIn('id="areacards"', page)
        self.assertNotIn('id="areamap"', page)
        cards = re.findall(r'<button type="button" class="acard (\w+)" data-area="(\w)".*?'
                           r'<span class="cname">([^<]+)</span><span class="crank">([^<]+)</span>(.*?)</button>', page)
        self.assertEqual([c[:4] for c in cards], [("b5", "A", "Class A", "6"), ("b11", "B", "Class B", "15"),
                                                  ("b21", "C", "Class C", "30")])
        self.assertIn("Gwen&rsquo;s class", cards[0][4])
        self.assertEqual([c[4] for c in cards[1:]], ["", ""])
        # No stats or map spots for the cards: three cards say it all.
        self.assertNotIn('class="stats" data-season="2099" data-view="9" data-map="areas"', page)
        self.assertIn(f'<p class="mapnote" data-map="areas" hidden>Classes group schools by enrollment', page)
        self.assertIn('<td class="sn"><a href="https://www.athletic.net/TrackAndField/rankings/list/900602/f/400m?grades=9"', page)
        self.assertIn("#### Maine Classes", md)
        self.assertIn("| 15 | Class B |", md)
        self.assertIn("Class,30,Class C,C,", csv)


class Sections(AnonymizedAreas):
    """The home State's Areas (California's CIF Sections, areas/CA.json) as a second map."""

    def test_a_season_saved_without_its_sections_is_short(self):
        self.assertFalse(build.lacks_areas(saved()))
        self.assertTrue(build.lacks_areas(saved(without_areas())))
        # A short season still builds, its Sections map saying they are not in yet.
        page, _, _ = build_page(without_areas())
        self.assertIn('id="usmap"', page)
        self.assertIn('data-map="areas" aria-pressed="false">CA Sections</button>', page)
        self.assertIn('<p class="nodata" data-season="2099" data-map="areas" hidden>No CA Sections in the 2099 '
                      'data yet: rankmap collects them the next time it collects 2099.</p>', page)

    def test_only_finished_seasons_are_short(self):
        cfg = {"graduation_year": 2102, "season_end": (7, 31)}
        have = {SEASON: saved(without_areas())}
        self.assertEqual(rankmap.short_seasons(cfg, have, date(2099, 11, 2)), {SEASON})
        # In progress, it is collected (and pulled fresh) only when asked, as always.
        self.assertEqual(rankmap.short_seasons(cfg, have, date(2099, 5, 2)), set())

    def test_a_short_season_is_collected_again_from_the_cache(self):
        cfg = {"graduation_year": 2102, "season_end": (7, 31), "data_dir": Path("/nowhere")}
        args = type("Args", (), {"no_fetch": False, "refresh": []})()
        with redirect_stdout(io.StringIO()) as out, mock.patch.object(rankmap, "ask", lambda q, default: default):
            todo = rankmap.plan(cfg, {SEASON: saved()}, {}, args, date(2099, 11, 2), short={SEASON})
        self.assertEqual(todo, [SEASON])
        self.assertIn("2099: saved without its home State's Area lists; collecting it again", out.getvalue())
        self.assertFalse(rankmap.wants_fresh(SEASON, {SEASON}, args, short={SEASON}))
        self.assertTrue(rankmap.wants_fresh(SEASON, {SEASON}, args))

    def test_every_areas_file_is_complete(self):
        from report import REPO, state_areas
        # The real files, and each with the standards of every areas_dir (the tests', the
        # examples'), as the pages read them.
        files = sorted((REPO / "areas").glob("*.json"))
        self.assertTrue(files)
        dirs = [None, AREAS] + sorted((REPO / "examples" / "rankmap").glob("*/areas"))
        for path in files:
            for d in dirs:
                if d is not None and not (d / path.name).exists():
                    continue  # an example's areas_dir covers its own State only
                with self.subTest(path.name, areas_dir=d):
                    self.complete(state_areas(path.stem, d))

    def areas_dir_error(self, files, state="CA"):
        """The ValueError state_areas(state, d) raises for an areas_dir d of files {name: content}."""
        from report import state_areas
        with tempfile.TemporaryDirectory() as tmp:
            for name, content in files.items():
                (Path(tmp) / name).write_text(content if isinstance(content, str) else json.dumps(content))
            with self.assertRaises(ValueError) as caught:
                state_areas(state, Path(tmp))
            return str(caught.exception)

    def test_an_areas_dir_holds_only_its_meets_standards(self):
        ca = {"at_large": {}, "notes": ["moved"]}
        # Another kind's keys, or the meet's rules.
        self.assertIn("not standards", self.areas_dir_error({"CA.json": {"state_meet": dict(ca, standards={})}}))
        self.assertIn("not places", self.areas_dir_error({"CA.json": {"state_meet": dict(ca, places={})}}))
        self.assertIn("not at_large", self.areas_dir_error(
            {"ME.json": {"state_meet": {"standards": {}, "at_large": {}}}}, "ME"))
        self.assertIn("at_large missing", self.areas_dir_error({"CA.json": {"state_meet": {"notes": []}}}))

    def test_an_areas_dir_file_has_its_shape(self):
        for bad in ("[]", "{", {"state_meet": []}, {"state_meet": {}, "areas": []}, {}):
            with self.subTest(bad):
                self.assertIn("CA.json", self.areas_dir_error({"CA.json": bad}))

    def test_an_areas_dir_needs_each_states_file_by_its_exact_name(self):
        ca = {"state_meet": {"at_large": {}}}
        self.assertIn("missing", self.areas_dir_error({}))
        self.assertIn("me.json", self.areas_dir_error({"CA.json": ca, "me.json": ca}))
        self.assertIn("ME.json.bak", self.areas_dir_error({"CA.json": ca, "ME.json.bak": ca}))
        self.assertIn("OR.json", self.areas_dir_error({"CA.json": ca, "OR.json": ca}))  # no Area file

    def test_an_empty_areas_dir_is_fine_for_a_state_without_an_area_file(self):
        from report import state_areas
        with tempfile.TemporaryDirectory() as tmp:
            (Path(tmp) / ".gitkeep").write_text("")
            self.assertIsNone(state_areas("OR", Path(tmp)))

    def test_an_areas_dir_replaces_the_published_standards(self):
        from report import state_areas
        with tempfile.TemporaryDirectory() as tmp:
            (Path(tmp) / "CA.json").write_text(json.dumps({"state_meet": {"at_large": {"2099": {}}}}))
            meet = state_areas("CA", Path(tmp))["state_meet"]
            self.assertEqual(meet["at_large"], {"2099": {}})
            for gone in ("notes", "sources", "checked"):
                self.assertNotIn(gone, meet)
            self.assertTrue(meet["places"])  # the rules stay areas/'s
            self.assertNotEqual(state_areas("CA")["state_meet"]["at_large"], {"2099": {}})  # areas/ keeps its own

    def test_an_edited_areas_dir_file_is_read_again(self):
        from report import state_areas
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "CA.json"
            path.write_text(json.dumps({"state_meet": {"at_large": {}, "notes": ["one"]}}))
            self.assertEqual(state_areas("CA", Path(tmp))["state_meet"]["notes"], ["one"])
            path.write_text(json.dumps({"state_meet": {"at_large": {}, "notes": ["two, longer"]}}))
            self.assertEqual(state_areas("CA", Path(tmp))["state_meet"]["notes"], ["two, longer"])

    def complete(self, a):
        from report import REPO
        for key in ("state", "level", "label", "labels", "note", "areas"):
            self.assertIn(key, a)
        for x in a["areas"]:
            self.assertTrue(x.get("abbr"), x["name"])
        meet = a.get("state_meet")
        if meet and "standards" in meet:
            # Entered by standards (Maine): per season, every Area, both genders, each event's
            # [automatic, provisional], the automatic one at least as hard.
            self.assertIsInstance(meet["field"], int)
            for season, by_area in meet["standards"].items():
                self.assertEqual(set(by_area), {x["name"] for x in a["areas"]}, f"{season}: every Area")
                for genders in by_area.values():
                    self.assertEqual(set(genders), {"f", "m"})
                    for events in genders.values():
                        for event, (automatic, provisional) in events.items():
                            self.assertLessEqual(build.mark_sort(automatic), build.mark_sort(provisional),
                                                 f"{season} {event}")
        elif meet:
            self.assertEqual(set(meet["places"]), {x["name"] for x in a["areas"]}, "places for every Area")
            for season, genders in meet["at_large"].items():
                for marks in genders.values():
                    for event, mark in marks.items():
                        build.mark_sort(mark)  # parses
        if not a.get("map"):
            # Areas that are not places (Maine's Classes): nothing to draw.
            for key in ("outlines", "split_counties"):
                self.assertNotIn(key, a, "only with a map")
            for x in a["areas"]:
                self.assertNotIn("counties", x, "only with a map")
                self.assertNotIn("county", x, "only with a map")
            return
        counties = json.loads((REPO / a["map"]).read_text())["counties"]
        placed = [c for x in a["areas"] for c in x["counties"]]
        self.assertEqual(sorted(placed), sorted(counties), "every map county in exactly one Area")
        for x in a["areas"]:
            if not x["counties"]:
                self.assertIn(x.get("county"), counties, f"{x['name']} is drawn inside a county")
        for county in a.get("split_counties", {}):
            self.assertIn(county, counties)
        if a.get("outlines"):
            districts = json.loads((REPO / a["outlines"]).read_text())["districts"]
            self.assertTrue(set(districts) <= {x["name"] for x in a["areas"] if not x["counties"]})

    def test_the_page_has_a_section_map(self):
        page, _, _ = build_page()
        self.assertIn('<div class="seg maps" role="group" aria-label="Map"><button type="button" data-map="us" '
                      'aria-pressed="true">US states</button><button type="button" data-map="areas" '
                      'aria-pressed="false">CA Sections</button></div>', page)
        counties = re.findall(r'<path class="ct (\w+)" data-area="(\w+)"', page)
        self.assertEqual(len(counties), 58)
        # Central Coast, the one Section in the data, is colored by its place (#6 against 9th graders);
        # the others have no data in this report.
        colored = {area: cls for cls, area in counties if cls != "none"}
        self.assertEqual(colored, {"CCS": "b5"})
        # Every Section has a callout off the State's edge, named in full ("Los Angeles City", not
        # Athletic.net's "Los Angeles"): the coastal ones west, the inland ones east.
        callouts = re.findall(r'<g class="co" data-area="(\w+)" data-side="(\w+)"', page)
        self.assertEqual(sorted(callouts), sorted([
            ("CCS", "west"), ("LACS", "west"), ("NCS", "west"), ("OS", "west"), ("SDS", "west"), ("SFS", "west"),
            ("CS", "east"), ("NS", "east"), ("SJS", "east"), ("SS", "east")]))
        named = re.findall(r'<text class="ab" x="-?[\d.]+" y="[\d.]+" text-anchor="(?:end|start)">([^<]+)</text>', page)
        self.assertEqual(sorted(named), ["Central", "Central Coast", "Los Angeles City", "North Coast", "Northern",
                                         "Oakland", "Sac-Joaquin", "San Diego", "San Francisco", "Southern"])
        # A rank that fits in its Section is shown there, so only the small Sections' callouts have a pill.
        self.assertEqual(sorted(re.findall(r'<rect class="achip \w+" data-area="(\w+)"', page)), ["LACS", "OS", "SFS"])
        # The Los Angeles City and Oakland Sections (one district's schools each) are outlined.
        self.assertEqual(sorted(re.findall(r'<path class="dist \w+" data-area="(\w+)"', page)), ["LACS", "OS"])
        self.assertNotIn('class="eyebrow"', page)  # one title line: name, event, season best
        self.assertIn(' 400m <span class="pr">', page)
        self.assertIn(f'<tr data-pick="CCS"><td class="rk"><span class="pill b11">12</span></td><td class="sn">'
                      f'<a href="https://www.athletic.net/TrackAndField/rankings/list/{CCS}/f/400m?grades=9,10" ', page)

    def test_a_home_state_without_an_areas_file_has_no_switch(self):
        # Her newest season's home State is one with no areas/<ST>.json (she moved, say).
        r = without_areas()
        home = next(x for x in r["results"] if x["list"]["id"] == r["mark"]["list"]["id"])
        for x in r["results"]:
            if x["list"]["id"] == home["list"]["id"]:
                x["list"]["state"] = "TX"
        page, _, _ = build_page(r)
        self.assertNotIn('class="seg maps"', page)
        self.assertNotIn('id="areamap"', page)
        self.assertFalse(build.lacks_areas(saved(r)))

    def test_a_tie_for_first_highlights_when_any_of_its_sections_is_picked(self):
        page, _, _ = build_page(first_in_ccs())
        self.assertIn('tr.dataset.pick.split("|").includes(pick)', page)
        self.assertIn('<tr class="firsts" data-pick="CCS">', page)

    def test_csv_and_markdown_list_the_sections(self):
        _, table, md = build_page()
        self.assertIn(",9th graders,Section,6,Central Coast,CCS,5,0,", table)
        self.assertIn("#### California Sections", md)

    def test_standards_read_as_sort_keys(self):
        self.assertEqual(build.mark_sort("55.60"), 55600)
        self.assertEqual(build.mark_sort("2:11.70"), 131700)
        self.assertEqual(build.mark_sort("10:29.42"), 629420)
        self.assertEqual(build.mark_sort("18-02.75"), 20_000_000 - 218750)
        self.assertEqual(build.mark_sort("5-06"), 20_000_000 - 66000)

    def test_the_state_meet_by_section(self):
        sm = build.prepare({SEASON: saved()}, areas_dir=AREAS)[str(SEASON)]["state_meet"]
        # The 2099 girls' 400m at-large mark of the anonymized copy: 55.20; her 58.98 does not meet it.
        self.assertEqual((sm["standard"], sm["meets"]), ("55.20", False))
        self.assertEqual((sm["places"]["SS"], sm["places"]["CCS"], sm["places"]["SFS"]), (6, 3, 1))
        self.assertEqual(sm["all_hs"]["CCS"], (36, "36"))
        page, _, md = build_page()
        self.assertIn("<h3>CIF State Track &amp; Field Championships, 2099</h3>", page)
        # The cards follow the grade group: 6th against 9th graders, 36th All HS, both past the top 3.
        lists = dict(re.findall(r'<ul class="smlist" data-season="2099" data-view="([\w-]+)"[^>]*>(.*?)</ul>', page))
        self.assertIn('<li class="out" data-pick="CCS"><b>Central Coast</b> <span>top 3 go &middot; Jamie #6</span></li>',
                      lists["9"])
        self.assertIn('<li class="out" data-pick="CCS"><b>Central Coast</b> <span>top 3 go &middot; Jamie #36</span></li>',
                      lists["all"])
        self.assertIn("<p>The at-large mark for 2099 was <b>55.20</b></p>", page)
        self.assertNotIn("does not meet it", page)  # the page states the mark; the Markdown compares
        self.assertIn("- At-large mark 55.20: 58.98 does not meet it", md)
        self.assertIn("- Central Coast: top 3 go, Jamie #36 All HS", md)
        # The pick line and tooltip carry it too.
        self.assertIn("Central Coast: #36 against all high schoolers in 2099 (35 faster); State Meet: its top 3 go", page)
        # Inside the top 3, a card says so.
        page, _, _ = build_page(first_in_ccs())
        lists = dict(re.findall(r'<ul class="smlist" data-season="2099" data-view="([\w-]+)"[^>]*>(.*?)</ul>', page))
        self.assertIn('<li class="in" data-pick="CCS"><b>Central Coast</b> <span>top 3 go &middot; Jamie #1</span></li>',
                      lists["9"])

    def test_every_county_is_in_a_section(self):
        from report import state_areas, state_map
        smap = state_map(state_areas("CA"))
        self.assertEqual(len(smap["counties"]), 58)
        self.assertEqual(set(smap["places"]) | set(smap["anchors"]),
                         {a["name"] for a in state_areas("CA")["areas"]})


RI = 4300  # a made-up list id for Rhode Island


def with_rhode_island(season, home_ri):
    """The saved report for season with a Rhode Island list beside California's (copies of its
    places, made up), and the Athlete's mark read from Rhode Island's when home_ri: Rhode Island
    has a callout chip on the US map, as the smallest states do."""
    r = copy.deepcopy(RANK)
    r["season"] = season
    for x in [x for x in r["results"] if x["list"]["kind"] == "state"]:
        y = copy.deepcopy(x)
        y["list"].update(id=RI, name="Rhode Island", state="RI")
        y["url"] = x["url"].replace(str(CA), str(RI))
        r["results"].append(y)
    if home_ri:  # and no Areas: Rhode Island has no Area file
        r["mark"]["list"].update(id=RI, name="Rhode Island", url=r["mark"]["list"]["url"].replace(str(CA), str(RI)))
        r["results"] = [x for x in r["results"] if x["list"]["kind"] == "state"]
        r["home_area"] = None
    return r


def two_seasons(newest_home_ri=False):
    """A page of 2098 and 2099, the Athlete at a school in Rhode Island one season and California
    the other (Rhode Island in 2098, unless newest_home_ri)."""
    with tempfile.TemporaryDirectory() as tmp:
        html, page = build.build({2098: saved(with_rhode_island(2098, not newest_home_ri)),
                                  2099: saved(with_rhode_island(2099, newest_home_ri))},
                                 [2098, 2099], Path(tmp), areas_dir=AREAS)
        return html.read_text(), html.with_suffix(".md").read_text()


class HomeState(AnonymizedAreas):
    """The Athlete's home State, outlined on the US map and tagged in the states table."""

    def test_the_home_state_is_outlined_over_its_neighbors(self):
        page, _, _ = build_page()
        shape = re.search(r'<path class="st \w+" data-abbr="CA" d="([^"]+)">', page).group(1)
        # A copy of its shape, after every state's (so over their borders) and before the numbers.
        self.assertIn(f'</g><g><path class="home" d="{shape}"/></g><g><text class="lbl', page)
        self.assertIn("#usmap path.home { fill: none; stroke: var(--fg);", page)
        # Its fill stays its rank's color.
        self.assertRegex(page, r'<path class="st b21" data-abbr="CA"')
        self.assertIn("California: #42 against 9th graders in 2099 (41 faster); Jamie&#x27;s home state", page)

    def test_the_home_states_row_is_tagged_in_the_states_table_only(self):
        page, _, md = build_page()
        tagged = re.findall(r'<td class="sn"><a [^>]+>([^<]+)</a> <span class="htag">home</span></td>', page)
        self.assertEqual(tagged, ["California"] * 4)  # one per grade group; none in the Sections tables
        self.assertEqual(page.count('class="htag"'), 4)
        self.assertIn("| 42 | California (home) |", md)
        self.assertNotIn("Central Coast (home)", md)
        # The legend's key names it, on the US map only.
        self.assertIn('<li class="hk" data-season="2099" data-map="us"><span class="sw hsw"></span>'
                      '<span class="lk">CA</span><span class="lc">&middot; home state</span></li>', page)

    def test_a_home_state_at_first_is_tagged_in_the_firsts_row(self):
        r = copy.deepcopy(RANK)
        x = next(x for x in r["results"] if x["list"]["id"] == CA and x["grades"] == [9])
        x["place"].update(rank=1, label="1", tied=False, ahead=0, ties=0)
        x["place"]["neighborhood"].update(ahead=[], skipped=0, tied=[])
        page, _, md = build_page(r)
        self.assertRegex(page, r'<tr class="firsts" data-pick="CA">.*?<div class="names"><a [^>]+>California</a> '
                               r'<span class="htag">home</span></div>')
        self.assertIn("| 1 | California (home) | - |", md)

    def test_each_season_marks_its_own_home_state(self):
        page, md = two_seasons()
        self.assertIn('const HOME = {"2098":"RI","2099":"CA"};', page)
        # The page opens on 2099: California outlined, Rhode Island's callout plain.
        ca = re.search(r'data-abbr="CA" d="([^"]+)"', page).group(1)
        self.assertIn(f'<path class="home" d="{ca}"/>', page)
        self.assertIn('<g class="co" data-abbr="RI">', page)
        # Switching seasons moves the outline and the ring to that season's home State.
        self.assertIn('outline.setAttribute("d", home.getAttribute("d"));', page)
        self.assertIn('g.classList.toggle("home", g.dataset.abbr === HOME[s])', page)
        # Each season's table tags its own, and the legend's key is the season's.
        for season, name in (("2098", "Rhode Island"), ("2099", "California")):
            for body in re.findall(rf'<tbody data-season="{season}" data-view="[\w-]+" data-map="us"[^>]*>(.*?)</tbody>', page):
                self.assertEqual(re.findall(r'<a [^>]+>([^<]+)</a> <span class="htag">', body), [name])
        self.assertIn('<li class="hk" data-season="2098" data-map="us" hidden>', page)
        self.assertIn('<span class="lk">RI</span>', page)
        self.assertIn("| 42 | Rhode Island (home) |", md)

    def test_a_callout_home_states_chip_is_ringed(self):
        page, _ = two_seasons(newest_home_ri=True)
        self.assertIn('<g class="co home" data-abbr="RI">', page)
        self.assertIn("#usmap .co.home .chip { stroke: var(--fg);", page)
        self.assertEqual(page.count('<g class="co home"'), 1)

    def test_a_phone_lists_the_callouts_under_the_map(self):
        page, _ = two_seasons(newest_home_ri=True)
        # Each callout State's rank in a chip of its rank's color, with its abbreviation and tooltip, the
        # home State's ringed; on the US map only.
        self.assertIn('</svg>\n    <ul class="calls" data-map="us" aria-label="Jamie Uddumcombe\'s rank in the '
                      'states too small to label on the map"><li class="home" data-abbr="RI" title="Rhode Island: '
                      '#42 against 9th graders in 2099 (41 faster); Jamie&#x27;s home state"><span class="cchip b21">42'
                      '</span><span class="cab">RI</span></li></ul>', page)
        self.assertIn(".calls .home .cchip { box-shadow: inset 0 0 0 1.5px var(--fg); }", page)
        # Shown at phone width only, in place of the map's callouts, the map fitted to the States.
        self.assertIn(".calls { display: none; ", page)
        phone = re.search(r"@media \(max-width: 560px\) \{(.*)\}\n", page).group(1)
        self.assertIn("#usmap .co { display: none; } .calls { display: grid; }", phone)
        self.assertIn('viewBox="-10 0 1060 620" data-wide="-10 0 1060 620" data-tight="-10 0 978 620"', page)
        self.assertIn("usmap.setAttribute(\"viewBox\", narrow.matches ? usmap.dataset.tight : usmap.dataset.wide)", page)
        # The page script keeps it in step with the season and grade group, the ring with the season's home.
        self.assertIn('document.querySelectorAll(".calls li").forEach(li => {', page)
        self.assertIn('li.classList.toggle("home", li.dataset.abbr === HOME[s]);', page)
        # The 2098 page opens on California at home: Rhode Island's chip is plain.
        self.assertIn('<li data-abbr="RI" title=', two_seasons()[0])

    def test_no_callout_list_without_a_callout_state(self):
        page, _, _ = build_page()  # California only
        self.assertNotIn('<ul class="calls"', page)
        self.assertIn('data-tight="-10 0 978 620"', page)


class ExampleMode(AnonymizedAreas):
    """`example`: a page built from anonymized data for the published examples (ADR 0010)."""

    def example(self, rank=None):
        with tempfile.TemporaryDirectory() as tmp:
            html, _ = build.build({SEASON: saved(rank)}, [SEASON], Path(tmp), example=True, areas_dir=AREAS)
            return html.read_text(), html.with_suffix(".csv").read_text(), html.with_suffix(".md").read_text()

    def test_every_link_goes_to_the_demo_page(self):
        import report
        for rank in (None, maine()):
            for out in self.example(rank):
                self.assertNotRegex(out, r"(?i)athletic\.net/")
                self.assertNotIn("https://", out.replace("https://fonts.g", ""))
            page = self.example(rank)[0]
            hrefs = set(re.findall(r'<a[^>]* href="([^"]*)"', page))
            self.assertEqual(hrefs, {report.DEMO_PAGE})
            self.assertIn(f'"{report.DEMO_PAGE}"', page[page.index("const DATA"):])  # the pick line's links

    def test_it_is_indexable_and_says_what_it_is(self):
        import report
        page = self.example()[0]
        self.assertNotIn('name="robots"', page)
        self.assertEqual(page.count(report.example_footer()), 1)
        self.assertIn("footer.example {", page)

    def test_its_standards_are_its_own_and_say_so(self):
        with tempfile.TemporaryDirectory() as tmp:
            meet = json.loads((AREAS / "ME.json").read_text())["state_meet"]
            note = "These standards are moved as the example's marks are."
            (Path(tmp) / "ME.json").write_text(json.dumps({"state_meet": {"standards": meet["standards"],
                                                                          "notes": [note]}}))
            with tempfile.TemporaryDirectory() as out:
                html, _ = build.build({SEASON: saved(maine())}, [SEASON], Path(out), example=True,
                                      areas_dir=Path(tmp))
                page = html.read_text()
        self.assertIn(build.esc(note), page)
        self.assertNotIn("Checked ", page)
        self.assertNotIn("Running standards", page)  # areas/ME.json's own notes

    def test_a_state_meet_by_places_shows_its_notes(self):
        with tempfile.TemporaryDirectory() as tmp:
            meet = json.loads((AREAS / "CA.json").read_text())["state_meet"]
            note = "These standards are moved as the example's marks are."
            (Path(tmp) / "CA.json").write_text(json.dumps({"state_meet": {"at_large": meet["at_large"],
                                                                          "notes": [note]}}))
            with tempfile.TemporaryDirectory() as out:
                html, _ = build.build({SEASON: saved()}, [SEASON], Path(out), example=True, areas_dir=Path(tmp))
                page = html.read_text()
        self.assertIn(f"About these standards</summary><ul><li>{build.esc(note)}</li>", page)

    def test_the_mark_column_fits_the_longest_mark(self):
        page = self.example()[0]
        self.assertIn("min-width: 5ch; margin-right: .35em;", page)  # 400m marks: 53.78

    def test_an_example_needs_its_own_standards(self):
        with tempfile.TemporaryDirectory() as tmp, self.assertRaises(ValueError):
            build.build({SEASON: saved()}, [SEASON], Path(tmp), example=True)

    def test_its_links_stay_in_the_tab_and_its_csv_has_none(self):
        page, table, _ = self.example()
        self.assertNotIn('target="_blank"', page)
        self.assertNotIn("link.target", page)
        self.assertIn("Names link to a page about the links", page)
        self.assertNotIn("demo.html", table)

    def test_a_real_page_has_none_of_it(self):
        import report
        page = build_page()[0]
        self.assertIn(report.ROBOTS, page)
        self.assertNotIn("footer", page)

    def test_the_config_says_whether_a_page_is_an_example(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "rankmap.config.json"
            base = {"athlete_id": 1, "event": "400m", "graduation_year": 2100, "output_dir": "out"}
            path.write_text(json.dumps(base))
            self.assertEqual(rankmap.load_config(path)["areas_dir"], None)
            (Path(tmp) / "areas").mkdir()
            path.write_text(json.dumps(dict(base, example=True, areas_dir="areas")))
            cfg = rankmap.load_config(path)
            self.assertEqual((cfg["example"], cfg["areas_dir"]), (True, (Path(tmp) / "areas").resolve()))
            for bad in ({"example": "yes"}, {"example": True}, {"example": True, "areas_dir": 3},
                        {"example": True, "areas_dir": "missing"}, {"areas_dir": "areas"}):
                path.write_text(json.dumps(dict(base, **bad)))
                with self.subTest(bad), self.assertRaises(SystemExit):
                    rankmap.load_config(path)

    def test_grades_read_as_ordinals(self):
        self.assertEqual([build.ordinal(n) for n in (1, 2, 3, 4, 9, 11, 12, 13, 21, 22, 101, 111)],
                         ["1st", "2nd", "3rd", "4th", "9th", "11th", "12th", "13th", "21st", "22nd", "101st", "111th"])


class Theme(AnonymizedAreas):
    """The design tokens in report.THEME: every color the pages use is one of them, and a page
    renders in any theme."""

    def test_every_color_is_a_token(self):
        import report
        for path in (report.__file__, build.__file__, rankmap.__file__):
            self.assertEqual(report.color_literals(path), [],
                             f"{Path(path).name}: a color outside report.THEME; make it a token")

    def test_a_color_outside_the_tokens_is_caught(self):
        from report import color_literals
        with tempfile.TemporaryDirectory() as d:
            src = Path(d) / "page.py"
            src.write_text(textwrap.dedent('''\
                def house_theme():
                    return Theme(bg="#fffcf0", county_line="rgba(255,255,255,.4)", fg="black")
                A = ".x { color: #FFF8C4; }"
                B = ".x { border-color: RGBA(0,0,0,.1); }"
                C = ".x { fill: #abc; }"
                D = ".x { background: oklch(70% 0.1 120); }"
                E = ".x { --tag: Lab(50 0 0); }"
                F = ".x { stroke: hwb(0 0% 0%); }"
                G = ".x { outline: 2px solid color-mix(in srgb, var(--fg), white); }"
                H = ".x { box-shadow: inset 0 0 0 1px Black; }"
                I = '<rect fill="tomato"/>'
                J = 'el.style.backgroundColor = "white";'
                K = 'el.setAttribute("stroke", "navy");'
                L = "#1, #2-4, #11-20 &#8217; #usmap #add"
                M = ".x { text-shadow: 0 0 2px black; }"
                N = ".x { text-decoration: underline red; }"
                O = ".x { filter: drop-shadow(0 0 1px navy); }"
                P = 'el.style.color = v || "red";'
                # a yellow-to-green scale, the palest yellow on white, in red ink
                R = ".x { fill: none; stroke: currentColor; color: var(--muted); border: 1px solid var(--line); }"
                S = rank_svg(lab) + bucket_color(x) + self.color(y)
                T = 'a[href$=".csv"] { text-decoration: underline; }'
                '''))
            self.assertEqual(sorted({i for i, _ in color_literals(src)}), list(range(3, 19)))

    def test_the_buckets_are_the_rank_tokens(self):
        import dataclasses
        from report import THEME
        self.assertEqual([(b[4], b[5]) for b in build.buckets()], [(s.fill, s.ink) for s in THEME.ranks])
        self.assertEqual(build.buckets()[0][5], THEME.highlight)  # #1's ink is the highlighter
        self.assertEqual(THEME.no_rank.ink, THEME.muted)
        with self.assertRaisesRegex(ValueError, "4 colors; the rank buckets need 5"):
            build.buckets(dataclasses.replace(THEME, ranks=THEME.ranks[:4]))

    def test_the_root_block_is_every_css_var_field_one_per_line(self):
        import dataclasses
        from report import THEME, Theme, theme_css
        want = [f"  --{f.name.replace('_', '-')}: {getattr(THEME, f.name)};"
                for f in dataclasses.fields(Theme) if f.metadata.get("css_var")]
        self.assertEqual(theme_css(THEME).splitlines(), [":root {", *want, "  color-scheme: light only;", "}"])
        self.assertEqual(len(want), 13)

    def test_the_fonts_url_is_escaped_for_its_attribute(self):
        from report import THEME, attr_url, head
        self.assertEqual(attr_url('https://x.test/css?a=1&b="2"<>'), "https://x.test/css?a=1&b=&quot;2&quot;&lt;&gt;")
        self.assertIn(f'href="{THEME.fonts_url}"', head())  # nothing in it needs escaping

    def test_a_restyled_theme_reaches_the_css(self):
        import dataclasses
        from report import THEME, css, theme_css
        theme = dataclasses.replace(THEME, bg="black", seg_on_fg="white", display="serif",
                                    map_edge="silver", name_px=30)
        root = theme_css(theme)
        for line in ("--bg: black;", "--seg-on-fg: white;", "--display: serif;", "color-scheme: light only;"):
            self.assertIn(line, root)
        self.assertEqual(css(".map { border-color: [[map_edge]]; font-size: [[name_px]]px; }", theme),
                         ".map { border-color: silver; font-size: 30px; }")
        # CSS's own $ and braces pass through.
        self.assertEqual(css('a[href$=".csv"] { color: inherit; }', theme), 'a[href$=".csv"] { color: inherit; }')
        with self.assertRaisesRegex(KeyError, "no_such_token"):
            css(".x { color: [[no_such_token]]; }")

    def test_chips_and_callouts_follow_their_sizes(self):
        import dataclasses
        from report import THEME, chips, map_viewbox
        house = chips(THEME)
        self.assertEqual((house.char, house.pad, house.least, house.half, house.left, house.gap, house.lead,
                          house.step, house.hawaii), (9, 10, 32, 11, 16, 6, 4, 30, 52))
        self.assertEqual(map_viewbox(THEME), "-10 0 1060 620")
        self.assertEqual((build.callouts(THEME).name_em, build.callouts(THEME).step, build.callouts(THEME).slot),
                         (11.2, 30, 52))
        big = dataclasses.replace(THEME, chip_px=30)
        self.assertEqual((chips(big).half, chips(big).width("T12")), (22, 74))
        self.assertGreater(int(map_viewbox(big).split()[2]), 1060)  # the callout column still fits
        self.assertEqual(build.callouts(big).step, chips(big).step)  # stacked pills cannot overlap
        # The map without its callout column, as a phone shows it: the same in every theme.
        from report import map_viewbox_tight
        self.assertEqual(map_viewbox_tight(), "-10 0 978 620")

    def test_the_phone_callout_list_follows_the_chip_and_abbreviation_sizes(self):
        import dataclasses
        from report import THEME
        page = build_page()[0]
        self.assertIn(".cchip { min-width: 32px; height: 22px; line-height: 22px; padding: 0 5px;", page)
        self.assertIn("font-weight: 900; font-size: 15px;", page)
        self.assertIn("minmax(max(57px, (100% - 4 * 8px) / 5), 1fr)", page)
        big = dataclasses.replace(THEME, chip_px=18, abbr_px=15)
        with tempfile.TemporaryDirectory() as tmp:
            html, _ = build.build({SEASON: saved()}, [SEASON], Path(tmp), big, areas_dir=AREAS)
            page = html.read_text()
        self.assertIn(".cchip { min-width: 38px; height: 26px; line-height: 26px; padding: 0 6px;", page)
        self.assertIn(".cab { font-family: var(--display); font-weight: 700; font-size: 15px;", page)
        self.assertIn("minmax(max(68px, (100% - 4 * 8px) / 5), 1fr)", page)

    def test_a_page_renders_in_another_theme(self):
        import dataclasses
        from report import THEME
        theme = dataclasses.replace(THEME, bg="ivory", ranks=tuple(dataclasses.replace(s, fill=f"oklch(.{i}0 .1 140)")
                                                                   for i, s in enumerate(THEME.ranks, 5)),
                                    chip_px=18, map_edge="silver")
        with tempfile.TemporaryDirectory() as tmp:
            html, _ = build.build({SEASON: saved()}, [SEASON], Path(tmp), theme, areas_dir=AREAS)
            page = html.read_text()
        self.assertIn("--bg: ivory;", page)
        self.assertIn(".b1 { --f: oklch(.50 .1 140);", page)
        self.assertIn("border: 1px solid silver;", page)
        self.assertIn('style="font-size:18px"', page)
        self.assertNotIn(THEME.map_edge, page)
        self.assertIn(f"--bg: {THEME.bg};", build_page()[0])  # THEME's page is still THEME's


class ThemeFile(AnonymizedAreas):
    """A theme file (report.load_theme): a config's `theme`, its tokens over the house theme's."""

    def load(self, tokens, raw=None):
        from report import load_theme
        with tempfile.TemporaryDirectory() as d:
            path = Path(d) / "theme.json"
            path.write_text(raw if raw is not None else json.dumps(tokens))
            return load_theme(path)

    def refused(self, tokens, says, raw=None):
        from report import ThemeError
        with self.assertRaises(ThemeError) as e:
            self.load(tokens, raw)
        self.assertIn(says, str(e.exception))
        return str(e.exception)

    def test_the_example_theme_loads(self):
        from report import THEME, load_theme
        theme = load_theme(Path(build.__file__).parent.parent / "theme.example.json")
        self.assertNotEqual(theme.display, THEME.display)
        self.assertNotEqual(theme.metrics, THEME.metrics)
        self.assertEqual(len(theme.ranks), len(build.RANK_BUCKETS))

    def test_an_empty_theme_is_the_house_theme(self):
        from report import THEME
        self.assertEqual(self.load({}), THEME)
        self.assertEqual(self.load({"_note": "a comment"}), THEME)

    def test_only_the_named_tokens_change(self):
        import dataclasses
        from report import THEME, Swatch
        theme = self.load({"bg": "#fafafa", "copy_px": 16, "no_rank": {"fill": "#eeeeee", "ink": "#333333"}})
        self.assertEqual(theme, dataclasses.replace(THEME, bg="#fafafa", copy_px=16,
                                                    no_rank=Swatch("#eeeeee", "#333333")))

    def test_an_unknown_token_is_named_with_the_valid_ones(self):
        from report import Theme
        says = self.refused({"bg": "#fff", "backgrund": "#fff"}, "no design token 'backgrund'")
        self.assertIn("the tokens are " + ", ".join(f.name for f in dataclasses_fields(Theme)), says)

    def test_wrong_types_are_refused(self):
        self.refused({"bg": 5}, "bg: must be text")
        self.refused({"bg": ""}, "bg: must be text")
        self.refused({"bg": "red; } body { display: none"}, "bg: \"red; } body { display: none\" is not a CSS value")
        self.refused({"display": None}, "display: cannot be null")
        self.refused({"copy_px": "15"}, "copy_px: must be a whole number of px")
        self.refused({"copy_px": True}, "copy_px: must be a whole number of px")
        self.refused({"copy_px": 0}, "copy_px: must be a whole number of px above 0")
        self.refused({"copy_px": 15.5}, "copy_px: must be a whole number of px")
        self.refused({"no_rank": "#eee"}, 'no_rank: must be an object of "fill", "ink"')
        self.refused({"no_rank": {"fill": "#eee"}}, "no_rank: must be an object of \"fill\", \"ink\"; 'ink' missing")
        self.refused({"no_rank": {"fill": "#eee", "ink": "#333", "edge": "#000"}}, "no_rank: must be an object of "
                     "\"fill\", \"ink\"; no 'edge'")
        self.refused({"fonts_url": "http://fonts.example.com/x.css"}, "fonts_url: must be an https:// URL")
        self.refused({}, "not valid JSON", raw="{bg: 1}")
        self.refused({}, "a theme file is a JSON object", raw="[]")

    def test_ranks_are_as_many_swatches_as_the_rank_buckets(self):
        ranks = [{"fill": f"#00000{i}", "ink": "#ffffff"} for i in range(len(build.RANK_BUCKETS))]
        self.assertEqual([s.fill for s in self.load({"ranks": ranks}).ranks], [r["fill"] for r in ranks])
        self.refused({"ranks": ranks[:4]}, 'ranks: must be a list of exactly 5 an object of "fill", "ink", not 4 items')
        self.refused({"ranks": ranks + ranks[:1]}, "not 6 items")
        self.refused({"ranks": ranks[:4] + [{"fill": "#000000"}]}, "ranks[4]: must be an object")
        self.refused({"ranks": ranks[:4] + [{"fill": 3, "ink": "#fff"}]}, "ranks[4].fill: must be text")

    def test_a_new_display_font_needs_every_metric(self):
        from report import THEME, Metrics
        says = self.refused({"display": '"Oswald", sans-serif'}, "'display' is Oswald, not Roboto Condensed, so it "
                            "needs its 'metrics' too")
        self.assertIn("tools/maps/measure_font.html", says)
        self.assertIn(", ".join(f.name for f in dataclasses_fields(Metrics)), says)
        metrics = {f.name: getattr(THEME.metrics, f.name) for f in dataclasses_fields(Metrics)}
        theme = self.load({"display": '"Oswald", sans-serif', "metrics": dict(metrics, ink_em=0.84)})
        self.assertEqual(theme.metrics.ink_em, 0.84)
        self.refused({"display": '"Oswald", sans-serif', "metrics": dict(metrics, ink_em=None)},
                     "metrics.ink_em: cannot be null")
        self.refused({"metrics": {k: v for k, v in metrics.items() if k != "tie_em"}}, "metrics: must be an object of "
                     "\"digit_em\"")
        self.refused({"metrics": dict(metrics, ink_em=0)}, "metrics.ink_em: must be above 0")
        self.refused({"metrics": dict(metrics, digit_em="0.5")}, "metrics.digit_em: must be a number")
        self.assertEqual(self.load({"metrics": dict(metrics, ink_drop_em=-0.02)}).metrics.ink_drop_em, -0.02)
        # Its fallbacks are not the font the maps are measured in: they need nothing.
        self.assertEqual(self.load({"display": '"Roboto Condensed", serif'}).metrics, THEME.metrics)

    def test_a_partial_theme_changes_only_its_tokens_in_the_page(self):
        from report import THEME
        theme = self.load({"bg": "#fafafa", "map_edge": "#cccccc"})
        with tempfile.TemporaryDirectory() as tmp:  # as the expected page is built (Pages)
            html, _ = build.build({SEASON: saved()}, [2099, 2100, 2101, 2102], Path(tmp), theme, areas_dir=AREAS)
            got = html.read_text()
        want = (TESTDATA / "expected" / "982389051-400m.html").read_text()
        want = want.replace(f"--bg: {THEME.bg};", "--bg: #fafafa;").replace(THEME.map_edge, "#cccccc")
        self.assertEqual(got, want)

    def test_fonts_are_any_stylesheet_or_none(self):
        from report import THEME, head
        self.assertIn('<link rel="preconnect" href="https://fonts.googleapis.com">', head(THEME))
        none = self.load({"fonts_url": None})
        self.assertIsNone(none.fonts_url)
        self.assertEqual(head(none), '<meta name="claude:viewport-fit-sides" content="cover">\n'
                                     '<meta name="color-scheme" content="light only">')
        other = head(self.load({"fonts_url": "https://fonts.example.com/a.css?x=1&y=2"}))
        self.assertIn('<link rel="stylesheet" href="https://fonts.example.com/a.css?x=1&y=2">', other)
        self.assertNotIn("preconnect", other)
        with tempfile.TemporaryDirectory() as tmp:
            html, _ = build.build({SEASON: saved()}, [SEASON], Path(tmp), none, areas_dir=AREAS)
            page = html.read_text()
        self.assertNotIn("<link", page)
        self.assertNotIn("fonts.g", page)

    def test_the_metrics_reach_the_page(self):
        import dataclasses
        from report import THEME, css, num
        self.assertEqual(build.tie_dy(THEME.metrics), ("-.318", ".191"))  # the house page's
        self.assertEqual([num(x) for x in (0.04, -0.3183, 2.0, 0.0, 12.25)], [".04", "-.318", "2", "0", "12.25"])
        self.assertEqual(css(".ab { letter-spacing: [[metrics.abbr_tracking_em]]em; }"), ".ab { letter-spacing: .04em; }")
        with self.assertRaisesRegex(KeyError, "metrics.nope"):
            css("[[metrics.nope]]")
        metrics = dataclasses.replace(THEME.metrics, tie_rise_em=0.3, abbr_tracking_em=0.1, digit_em=0.6)
        theme = dataclasses.replace(THEME, metrics=metrics)
        with tempfile.TemporaryDirectory() as tmp, mock.patch.object(build, "label_spots", lambda *a: json.loads(
                (Path(build.__file__).parent.parent / "maps" / "label-spots.json").read_text())):
            page = build.build({SEASON: saved()}, [SEASON], Path(tmp), theme, areas_dir=AREAS)[0].read_text()
        self.assertEqual(build.rank_svg("T12", metrics), '<tspan class="sup" dy="-.5em">t</tspan><tspan dy=".3em">12</tspan>')
        self.assertEqual(build.rank_svg("T12"), '<tspan class="sup" dy="-.318em">t</tspan><tspan dy=".191em">12</tspan>')
        self.assertIn('sup.setAttribute("dy", "-.5em");', page)
        self.assertIn("letter-spacing: .1em;", page)
        self.assertIn("const [DIGIT_EM, TIE_EM, BEARING_EM, INK_DROP_EM] = [0.6, ", page)

    def test_spots_are_fitted_and_kept_for_a_theme_with_its_own_metrics(self):
        import dataclasses
        import report
        committed = json.loads(report.SPOTS.read_text())
        # A color-only theme, or the house font with other fallbacks, reads the committed spots.
        for same in (dataclasses.replace(report.THEME, bg="#fafafa"),
                     dataclasses.replace(report.THEME, display='"Roboto Condensed", serif')):
            self.assertEqual(report.label_spots(same, "/nonexistent")["inputs"], committed["inputs"])
        # So does a change to a metric the fitting does not read.
        wider = dataclasses.replace(report.THEME, metrics=dataclasses.replace(report.THEME.metrics, chip_char_em=0.7))
        self.assertEqual(report.label_spots(wider, "/nonexistent")["inputs"], committed["inputs"])

        theme = dataclasses.replace(report.THEME, display='"Oswald", sans-serif',
                                    metrics=dataclasses.replace(report.THEME.metrics, ink_em=0.84))
        fitted = copy.deepcopy(committed)
        fitted["us"]["California"] = {shape: [123.4, 456.7, 33.3] for shape in report.LABEL_SHAPES}
        calls = []

        def compute(t):
            calls.append(t)
            return fitted
        with tempfile.TemporaryDirectory() as tmp, mock.patch.object(report, "compute_spots", compute), \
                redirect_stderr(io.StringIO()) as err:
            # (The inputs hash covers compute_spots' source: here, the stand-in's.)
            inputs = fitted["inputs"] = report.spots_inputs(theme)
            self.assertNotEqual(inputs, committed["inputs"])
            html, _ = build.build({SEASON: saved()}, [SEASON], Path(tmp) / "out", theme, areas_dir=AREAS,
                                  spots_dir=Path(tmp) / "spots")
            self.assertIn('x="123.4" y="456.7" style="font-size:33.3px"', html.read_text())
            self.assertEqual(calls, [theme])
            self.assertIn("Fitting the rank labels to Oswald's metrics", err.getvalue())
            kept = Path(tmp) / "spots" / f"{inputs}.json"
            self.assertEqual(json.loads(kept.read_text()), fitted)
            # Kept: a later build reads the file (a new process: the in-memory cache cleared).
            report.label_spots.cache_clear()
            self.assertEqual(report.label_spots(theme, Path(tmp) / "spots"), fitted)
            self.assertEqual(len(calls), 1)
            # A kept file for other inputs (the maps changed) is fitted again.
            kept.write_text(json.dumps(dict(fitted, inputs="stale")))
            report.label_spots.cache_clear()
            report.label_spots(theme, Path(tmp) / "spots")
            self.assertEqual(len(calls), 2)
        report.label_spots.cache_clear()

    def test_compute_spots_fits_in_the_themes_metrics(self):
        import dataclasses
        import report
        # The fitting itself, on two states (the whole map takes about ten seconds).
        taller = dataclasses.replace(report.THEME, metrics=dataclasses.replace(report.THEME.metrics, ink_em=0.9))
        regions = {k: v for k, v in report.us_regions().items() if k in ("Colorado", "Texas")}
        with mock.patch.object(report, "us_regions", lambda: regions), mock.patch.object(report, "AREAS_DIR", AREAS):
            house, tall = report.compute_spots(report.THEME), report.compute_spots(taller)
            self.assertEqual(tall["inputs"], report.spots_inputs(taller))
        committed = report.label_spots()
        self.assertEqual(house["us"], {k: committed["us"][k] for k in regions})
        # Taller ink in the same box: a smaller font (a four-digit rank: one digit is at its biggest).
        self.assertLess(tall["us"]["Colorado"]["4"][2], house["us"]["Colorado"]["4"][2])


def dataclasses_fields(cls):
    import dataclasses
    return dataclasses.fields(cls)


class LabelSpots(unittest.TestCase):
    """tools/maps/label-spots.json, where each rank label goes on the maps."""

    def test_the_spots_are_current(self):
        from report import label_spots, spots_inputs
        self.assertEqual(label_spots()["inputs"], spots_inputs(),
                         "the maps or the fitting changed: run python3 -I tools/maps/label_spots.py")

    def test_every_state_and_roomy_section_has_every_shape(self):
        from report import LABEL_SHAPES, area_regions, label_spots, state_areas, us_regions
        spots = label_spots()
        self.assertEqual(set(spots["us"]), set(us_regions()))
        self.assertEqual(set(spots["areas"]["CA"]), set(area_regions(state_areas("CA"))))
        for shapes in list(spots["us"].values()) + list(spots["areas"]["CA"].values()):
            self.assertEqual(set(shapes), set(LABEL_SHAPES))

    def test_a_label_fills_a_plain_box_and_sits_in_its_middle(self):
        import dataclasses
        from report import LABEL_FILL, THEME, edges, label_spot
        # A 300 x 100 box: a one-digit label is as tall as the box allows (capped at LABEL_MAX)
        # and centered, its anchor raised by the ink's drop, in the house metrics or any other.
        ring = [(0, 0), (300, 0), (300, 100), (0, 100)]
        taller = dataclasses.replace(THEME.metrics, ink_em=0.9, ink_drop_em=0.05)
        for metrics in (None, THEME.metrics, taller):
            m = metrics or THEME.metrics
            x, y, size = label_spot([ring], edges([ring + ring[:1]]), "1", metrics)
            self.assertAlmostEqual(x, 150, delta=0.5)
            self.assertAlmostEqual(size, min(80, 100 / m.ink_em * LABEL_FILL), delta=0.5)
            self.assertAlmostEqual(y, 50 - m.ink_drop_em * size, delta=0.5)


class Config(unittest.TestCase):
    """Where a config's paths point, and which config runs without --config."""

    def write(self, d, **over):
        cfg = {"athlete_id": 982389051, "event": "400m", "graduation_year": 2029, "output_dir": "out"}
        cfg.update(over)
        path = Path(d) / "local" / "rankmap" / "rankmap.config.json"
        path.parent.mkdir(parents=True)
        path.write_text(json.dumps(cfg))
        return path

    def test_data_and_output_sit_beside_the_config(self):
        with tempfile.TemporaryDirectory() as d:
            path = self.write(d)
            cfg = rankmap.load_config(path)
            here = path.parent.resolve()
            self.assertEqual(cfg["output_dir"], here / "out")
            self.assertEqual(cfg["data_dir"], here / "data" / "982389051-400m")

    def test_data_dir_is_relative_to_the_config_or_absolute(self):
        with tempfile.TemporaryDirectory() as d:
            path = self.write(d, data_dir="../seasons")
            self.assertEqual(rankmap.load_config(path)["data_dir"],
                             path.parent.parent.resolve() / "seasons" / "982389051-400m")
            path.unlink()
            path = self.write(Path(d) / "elsewhere", data_dir=str(Path(d) / "abs"))
            self.assertEqual(rankmap.load_config(path)["data_dir"],
                             (Path(d) / "abs").resolve() / "982389051-400m")

    def test_one_folder_comes_out_as_one_path_however_it_is_written(self):
        import report
        with tempfile.TemporaryDirectory() as d:
            config = Path(d) / "a" / "x.config.json"
            ways = ("../b", str(Path(d) / "b"), str(Path(d) / "a" / ".." / "b"))
            self.assertEqual({report.beside(config, p) for p in ways}, {(Path(d) / "b").resolve()})
            self.assertEqual(report.beside(config, "~/x"), (Path.home() / "x").resolve())

    def test_the_default_config_is_in_local_unless_only_the_older_one_exists(self):
        import report
        with tempfile.TemporaryDirectory() as d:
            local = Path(d) / "local"
            home = local / "rankmap" / "rankmap.config.json"
            older = Path(d) / "tools" / "rankmap" / "rankmap.config.json"
            for make, want, warns in ((None, home, False), (older, older, False), (home, home, True)):
                if make:
                    make.parent.mkdir(parents=True)
                    make.write_text("{}")
                err = io.StringIO()
                with mock.patch.object(sys, "stderr", err):
                    self.assertEqual(report.default_config("rankmap", local, older), want)
                self.assertEqual("is ignored now" in err.getvalue(), warns, err.getvalue())

    def test_config_takes_a_path_or_a_name_in_local(self):
        import report
        local = Path("/l")
        self.assertEqual(report.config_path("rankmap", "rankmap.1-400m", local),
                         local / "rankmap" / "rankmap.1-400m.config.json")
        self.assertEqual(report.config_path("rankmap", "x.config.json", local), local / "rankmap" / "x.config.json")
        self.assertEqual(report.config_path("rankmap", "./x.config.json", local), Path("x.config.json").resolve())
        self.assertEqual(report.config_path("rankmap", "/a/x.config.json", local), Path("/a/x.config.json").resolve())

    def test_a_config_loads(self):
        import report
        cfg = rankmap.load_config(TESTDATA / "rankmap.config.json")
        self.assertEqual(cfg["data_dir"], TESTDATA.resolve() / "data" / "982389051-400m")
        self.assertIs(cfg["theme"], report.THEME)

    def test_a_config_takes_a_theme_file_from_its_folder(self):
        with tempfile.TemporaryDirectory() as d:
            (Path(d) / "local" / "themes").mkdir(parents=True)
            theme = Path(d) / "local" / "themes" / "look.json"
            theme.write_text(json.dumps({"accent": "#0055aa"}))
            cfg = rankmap.load_config(self.write(d, theme="../themes/look.json"))
            self.assertEqual(cfg["theme"].accent, "#0055aa")
            for i, (value, says) in enumerate(((5, "'theme' is a theme file's path"),
                                               ("", "'theme' is a theme file's path"),
                                               ("none.json", "none.json: no such file"))):
                with self.assertRaises(SystemExit) as e:
                    rankmap.load_config(self.write(Path(d) / str(i), theme=value))
                self.assertIn(says, str(e.exception))

    def test_the_example_config_asks_for_an_athlete(self):
        example = Path(__file__).parent / "rankmap.config.example.json"
        self.assertEqual(json.loads(example.read_text())["athlete_id"], 0, "a placeholder, no real or fixture athlete")
        with self.assertRaises(SystemExit) as e:
            rankmap.load_config(example)
        self.assertIn("put your athlete's Athletic.net id as 'athlete_id'", str(e.exception))

    def test_a_missing_config_says_to_make_its_folder(self):
        with tempfile.TemporaryDirectory() as d:
            path = Path(d) / "local" / "rankmap" / "rankmap.config.json"
            with self.assertRaises(SystemExit) as e:
                rankmap.load_config(path)
            self.assertIn(f"mkdir -p {path.parent}", str(e.exception))
            self.assertIn("rankmap.config.example.json", str(e.exception))

    def test_seasons_left_in_the_old_place_stop_the_run_with_what_to_move(self):
        with tempfile.TemporaryDirectory() as d:
            old = Path(d) / "old tools" / "982389051-400m"  # a space, to check the quoting
            cfg = rankmap.load_config(self.write(d))
            rankmap.check_moved_data(cfg, old)  # nothing anywhere: a first run
            old.mkdir(parents=True)
            (old / "2026.json").write_text("{}")
            with self.assertRaises(SystemExit) as e:
                rankmap.check_moved_data(cfg, old)
            moved = shlex.quote(str(old.resolve() / "2026.json"))
            self.assertIn(f"mkdir -p {cfg['data_dir']} && mv {moved} {cfg['data_dir']}/", str(e.exception))
            cfg["data_dir"].mkdir(parents=True)
            (cfg["data_dir"] / "2027.json").write_text("{}")
            rankmap.check_moved_data(cfg, old)  # some seasons beside the config: fine
            cfg["data_dir"] = old.resolve()
            rankmap.check_moved_data(cfg, old)  # a config whose data_dir is the old place


def git(cwd, *args):
    """Run git for a test repo: no GIT_* redirection from the environment, no commit signing."""
    import report
    subprocess.run(["git", "-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false",
                    "-c", "tag.gpgsign=false", "-c", "protocol.file.allow=always", *args],
                   cwd=cwd, env=report.git_env(), check=True, capture_output=True)


@unittest.skipIf(shutil.which("git") is None, "no git")
class LocalDir(unittest.TestCase):
    """Which local/ the reports use: $ATH_LOCAL, else the main checkout's, shared by its worktrees,
    and where the older tools/ places are."""

    def setUp(self):
        import report
        self.report = report
        report.main_checkout.cache_clear()
        self.addCleanup(report.main_checkout.cache_clear)
        env = mock.patch.dict(os.environ)
        env.start()
        self.addCleanup(env.stop)
        os.environ.pop("ATH_LOCAL", None)
        self.tmp = Path(tempfile.mkdtemp()).resolve()
        self.addCleanup(shutil.rmtree, self.tmp)

    def repo(self, name):
        path = self.tmp / name
        path.mkdir()
        git(path, "init", "-q")
        git(path, "commit", "-q", "--allow-empty", "-m", "x")
        return path

    def test_a_linked_worktree_shares_the_main_checkouts_local_and_older_places(self):
        main = self.repo("main")
        git(main, "worktree", "add", "-q", str(self.tmp / "wt"))
        for checkout in (main, self.tmp / "wt"):
            self.assertEqual(self.report.local_dir(checkout), main / "local")
            self.assertEqual(self.report.older_place("rankmap", "data", repo=checkout), main / "tools" / "rankmap" / "data")

    def test_a_bare_repos_worktree_and_a_submodule_are_their_own_main_checkout(self):
        git(self.repo("src"), "clone", "-q", "--bare", ".", str(self.tmp / "bare.git"))
        git(self.tmp / "bare.git", "worktree", "add", "-q", str(self.tmp / "wt"))
        self.assertEqual(self.report.main_checkout(self.tmp / "wt"), self.tmp / "wt")
        sup = self.repo("super")
        git(sup, "submodule", "add", "-q", str(self.tmp / "src"), "sub")
        self.assertEqual(self.report.main_checkout(sup / "sub"), sup / "sub")

    def test_a_checkout_git_cannot_read_falls_back_with_a_warning(self):
        broken = self.tmp / "broken"
        broken.mkdir()
        (broken / ".git").write_text("gitdir: /nowhere\n")
        err = io.StringIO()
        with mock.patch.object(sys, "stderr", err):
            self.assertEqual(self.report.local_dir(broken), broken / "local")
        self.assertIn(f"fall back to {broken}", err.getvalue())
        self.assertIn("ATH_LOCAL", err.getvalue())

    def test_ath_local_wins_and_outside_git_it_is_beside_tools(self):
        plain = self.tmp / "plain"
        plain.mkdir()
        err = io.StringIO()
        with mock.patch.object(sys, "stderr", err):
            self.assertEqual(self.report.local_dir(plain), plain / "local")
        self.assertEqual(err.getvalue(), "")
        os.environ["ATH_LOCAL"] = str(self.tmp / "mine")
        self.assertEqual(self.report.local_dir(plain), self.tmp / "mine")


class Collect(unittest.TestCase):
    """rankmap.collect against a stub ath that records its arguments."""

    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp())
        self.cfg = {"athlete_id": 982389051, "event": "400m", "graduation_year": 2102, "state": "CA",
                    "data_dir": self.tmp / "data", "season_end": (7, 31)}
        self.args = self.tmp / "args.json"
        self.old_ath = os.environ.get("ATH")
        # The saved report has one State list (California) and its Central Coast Section.
        states = mock.patch.object(rankmap, "STATE_LISTS", 1)
        states.start()
        self.addCleanup(states.stop)

    def tearDown(self):
        if self.old_ath is None:
            os.environ.pop("ATH", None)
        else:
            os.environ["ATH"] = self.old_ath

    def stub(self, exit_code=0, report=None, raw=None):
        out = self.tmp / "report.json"
        out.write_text(raw if raw is not None else json.dumps(report if report is not None else RANK))
        ath = self.tmp / "ath"
        ath.write_text(textwrap.dedent(f"""\
            #!{sys.executable}
            import json, sys
            json.dump(sys.argv[1:], open({str(self.args)!r}, "w"))
            sys.stdout.write(open({str(out)!r}).read())
            sys.exit({exit_code})
            """))
        ath.chmod(ath.stat().st_mode | stat.S_IEXEC)
        os.environ["ATH"] = str(ath)

    def collect(self, refresh=False):
        with redirect_stdout(io.StringIO()) as out:
            got = rankmap.collect(self.cfg, SEASON, refresh, date(2099, 10, 30))
        return got, out.getvalue()

    def test_a_season_is_ranked_on_every_state_and_saved(self):
        self.stub()
        got, _ = self.collect()
        self.assertEqual(json.loads(self.args.read_text()), [
            "rank", "982389051", "--event", "400m", "--season", "2099", "--in", "all,areas", "--ahead", "all",
            "--grade", "9", "--json", "--home", "CA"])
        self.assertEqual(got, saved())
        on_disk = json.loads(rankmap.data_path(self.cfg, SEASON).read_text())
        self.assertEqual(on_disk, saved())
        have, stale = rankmap.load_data(self.cfg)
        self.assertEqual((list(have), stale), ([SEASON], {}))

    def test_refreshing_a_saved_season_refetches(self):
        self.stub()
        self.collect(refresh=True)
        self.assertEqual(json.loads(self.args.read_text())[-1], "--refresh")

    def test_a_failed_run_keeps_the_saved_season(self):
        path = rankmap.data_path(self.cfg, SEASON)
        path.parent.mkdir(parents=True)
        path.write_text("kept\n")
        self.stub(exit_code=1)
        got, out = self.collect()
        self.assertIsNone(got)
        self.assertIn("ath rank failed (exit 1); kept what was saved", out)
        self.assertEqual(path.read_text(), "kept\n")

    def test_a_partial_report_is_not_saved(self):
        r = copy.deepcopy(RANK)
        r["results"][0].update(status="lookup_failed", place=None)
        self.stub(report=r)
        got, out = self.collect()
        self.assertIsNone(got)
        self.assertIn("not saved, 1 list(s) not read: California 9th", out)
        self.assertFalse(rankmap.data_path(self.cfg, SEASON).exists())

    def test_a_report_missing_states_is_not_saved(self):
        # The real rule: the saved report's 2 State lists are not all 51.
        self.stub()
        with mock.patch.object(rankmap, "STATE_LISTS", 51):
            got, out = self.collect()
        self.assertIsNone(got)
        self.assertIn("not saved, has 1 of the 51 State lists", out)
        self.assertFalse(rankmap.data_path(self.cfg, SEASON).exists())

    def test_a_report_for_someone_else_is_not_saved(self):
        r = copy.deepcopy(RANK)
        r["athlete"]["id"] = 1
        self.stub(report=r)
        got, out = self.collect()
        self.assertIsNone(got)
        self.assertIn("not saved, is athlete 1 400m 2099, not athlete 982389051 400m 2099", out)

    def test_output_that_is_not_json_is_not_saved(self):
        self.stub(raw="")
        got, out = self.collect()
        self.assertIsNone(got)
        self.assertIn("not saved, ath rank printed no JSON report", out)

    def test_a_broken_saved_file_needs_collecting_again(self):
        path = rankmap.data_path(self.cfg, SEASON)
        path.parent.mkdir(parents=True)
        path.write_text('{"format": 2, "pulled"')
        have, stale = rankmap.load_data(self.cfg)
        self.assertEqual(have, {})
        self.assertIn("is not valid JSON", stale[SEASON])
        path.write_text(json.dumps(saved()))
        with mock.patch.object(rankmap, "STATE_LISTS", 51):
            _, stale = rankmap.load_data(self.cfg)
        self.assertEqual(stale[SEASON], "has 1 of the 51 State lists")


class Plan(unittest.TestCase):
    def test_a_legacy_season_is_collected_again(self):
        cfg = {"graduation_year": 2101, "season_end": (7, 31), "data_dir": Path("/nowhere")}
        args = type("Args", (), {"no_fetch": False, "refresh": []})()
        have = {SEASON: saved()}
        stale = {2098: "is from the retired browser collector (fetch.js)"}
        with redirect_stdout(io.StringIO()) as out, mock.patch.object(rankmap, "ask", lambda q, default: default):
            todo = rankmap.plan(cfg, have, stale, args, date(2099, 10, 30))
        # 2098 and 2099 are finished; 2099 is saved, and refreshing it defaults to no.
        self.assertEqual(todo, [2098])
        self.assertIn("2098: /nowhere/2098.json is from the retired browser collector (fetch.js); "
                      "collecting it again through ath", out.getvalue())

    def test_what_refetches_instead_of_reading_aths_cache(self):
        def fresh(season, have=(), refresh=()):
            args = type("Args", (), {"refresh": list(refresh)})()
            return rankmap.wants_fresh(season, set(have), args)

        self.assertFalse(fresh(2026), "a season not saved yet (or a retry after a failed run)")
        self.assertTrue(fresh(2026, have=[2026]), "collecting a saved season again")
        self.assertTrue(fresh(2026, refresh=[2026]), "--refresh, saved or not")


if __name__ == "__main__":
    unittest.main()
