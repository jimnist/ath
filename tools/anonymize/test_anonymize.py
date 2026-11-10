"""The anonymizer's tests (ADR 0010, docs/anonymize.md), on the made-up Recording in
testdata/recording/ (testdata/make_recording.py writes it). cargo test runs them too
(tests/tools.rs), with ATH_TEST_BIN set to the ath it built: then the tests also replay the
anonymized copy through ath and check that every rank, tie and seed comes out as recorded.
"""

import datetime
import io
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import unittest
from contextlib import redirect_stdout
from unittest import mock
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import anonymize  # noqa: E402

RECORDING = HERE / "testdata" / "recording"
SEED = 20261108


def load(path):
    return json.loads(Path(path).read_text())


def files(root):
    return sorted(p.relative_to(root) for p in Path(root).rglob("*.json"))


def walk(value, key=None):
    """(key, value) for every scalar in a JSON value."""
    if isinstance(value, dict):
        for k, v in value.items():
            yield from walk(v, k)
    elif isinstance(value, list):
        for v in value:
            yield from walk(v, key)
    else:
        yield key, value


def competition_ranks(sorts):
    ranks, last = [], None
    for i, s in enumerate(sorts, start=1):
        if s != last:
            rank, last = i, s
        ranks.append(rank)
    return ranks


class Anonymized(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.mkdtemp()
        cls.out = Path(cls.tmp) / "out"
        cls.report = anonymize.anonymize(RECORDING, cls.out, SEED)
        cls.ids = cls.report.id_map

    @classmethod
    def tearDownClass(cls):
        shutil.rmtree(cls.tmp)

    def out_file(self, rel):
        """The anonymized file for the recording's `rel`."""
        return self.out / out_rel(rel, self.report)

    def test_same_files_by_their_new_names(self):
        self.assertEqual(len(files(RECORDING)), len(files(self.out)))
        for rel in files(RECORDING):
            if rel.parts[0] != "search":
                self.assertTrue(self.out_file(rel).is_file(), rel)

    def test_structure_is_byte_for_byte_but_for_values(self):
        """Every file keeps its text but for values; rankings pages too, but for their rows
        (reshuffled) and searches (a dropped athlete's docs go)."""
        token = re.compile(r'"(?:[^"\\]|\\.)*"|-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?')
        rows = re.compile(r'"groupedRankings": \[.*\]\n \]', re.S)
        for rel in files(RECORDING):
            if rel.parts[0] == "search":
                continue
            before = token.sub("V", (RECORDING / rel).read_text())
            after = token.sub("V", self.out_file(rel).read_text())
            if rel.parts[0] == "rankings":
                before, after = rows.sub("ROWS", before), rows.sub("ROWS", after)
            self.assertEqual(before, after, rel)

    def test_deterministic_and_seeded(self):
        again = Path(self.tmp) / "again"
        anonymize.anonymize(RECORDING, again, SEED)
        for rel in files(self.out):
            self.assertEqual((self.out / rel).read_bytes(), (again / rel).read_bytes(), rel)
        other = Path(self.tmp) / "other"
        anonymize.anonymize(RECORDING, other, SEED + 1)
        self.assertNotEqual(files(self.out), files(other))

    def test_ids_move_into_the_reserved_range_consistently(self):
        for real, new in self.ids.items():
            self.assertTrue(900_000_000 <= new <= 999_999_999, (real, new))
        self.assertEqual(len(set(self.ids.values())), len(self.ids))
        subject = self.ids[1203001]
        school = self.ids[4101]
        bio = load(self.out_file("1203001.json"))
        self.assertEqual(bio["athlete"]["IDAthlete"], subject)
        self.assertEqual(bio["athlete"]["SchoolID"], school)
        self.assertIn(str(school), bio["allTeams"])
        self.assertIn("%d_2099" % school, bio["grades"])
        self.assertIn("%d_12099" % school, bio["grades"])
        team = load(self.out_file("teamnav/4101-2026.json"))
        self.assertEqual(team["team"]["ID"], school)
        page = load(self.out_file("rankings/900500-f-400m-9_10-p1.json"))
        self.assertEqual(page["division"]["ID"], self.ids[900500])
        self.assertEqual(page["division"]["ParentID"], self.ids[900020])
        row = next(r for r in page["groupedRankings"][0] if r["AthleteID"] == subject)
        self.assertEqual(row["TeamID"], school)
        meet = load(self.out_file("meets/770001/meet.json"))
        self.assertEqual(meet["meet"]["ID"], self.ids[770001])
        self.assertEqual(meet["tfDivisions"][0]["MeetID"], self.ids[770001])
        entries = load(self.out_file("meets/770001/entries/m-13-4x800m.json"))
        for e in entries["entries"]:
            self.assertTrue(self.out_file("%d.json" % next(r for r, n in self.ids.items() if n == e["IDAthlete"])).is_file())

    def test_names_are_invented_consistently(self):
        bio = load(self.out_file("1203001.json"))
        full = bio["athlete"]["FirstName"] + " " + bio["athlete"]["LastName"]
        school = bio["allTeams"][str(self.ids[4101])]["SchoolName"]
        team = load(self.out_file("teamnav/4101-2026.json"))["team"]
        self.assertEqual(team["Name"], school)
        self.assertNotEqual(team["City"], "Port Ellery")
        seen = 0
        for rel in files(self.out):
            if rel.parts[0] != "rankings":
                continue
            for row in load(self.out / rel)["groupedRankings"][0]:
                if row["AthleteID"] == self.ids[1203001]:
                    self.assertEqual(row["AthleteName"], full)
                    self.assertEqual(row["TeamName"], school)
                    seen += 1
        self.assertGreater(seen, 5)
        # The masked tail stays masked.
        tail = load(self.out_file("rankings/900500-f-400m-all-p1.json"))["groupedRankings"][0][-1]
        self.assertEqual((tail["AthleteID"], tail["TeamID"], tail["MeetID"]), (0, 0, 0))
        self.assertEqual(tail["AthleteName"], "Xxxxxx Xxxxxx")
        # The search doc and its file name.
        search = files(self.out / "search")
        self.assertEqual(len(search), 1)
        doc = load(self.out / "search" / search[0])["response"]["docs"][0]
        self.assertEqual(search[0].stem, anonymize.slug(doc["textsuggest"]))
        self.assertEqual(doc["id_db"], str(self.ids[1204002]))
        entrant = load(self.out_file("1204002.json"))["athlete"]
        self.assertEqual(doc["textsuggest"], entrant["FirstName"] + " " + entrant["LastName"])
        # Free text is blanked, tokens too.
        entries = load(self.out_file("meets/770001/entries/m-13-4x800m.json"))
        self.assertTrue(all(not e["Comment"] for e in entries["entries"]))

    def test_no_real_name_or_id_survives(self):
        names, ids = set(), set()
        for rel in files(RECORDING):
            if rel.parts[0] == "navinfo":
                continue
            for key, value in walk(load(RECORDING / rel)):
                if key in ("FirstName", "LastName", "AthleteName", "SchoolName", "TeamName", "MeetName",
                           "City", "textsuggest", "TeamCode") and isinstance(value, str) and value.strip("Xx "):
                    names.add(value)
                if key in anonymize.ID_KEYS and isinstance(value, int) and value:
                    ids.add(value)
        everything = "\n".join((self.out / rel).read_text() for rel in files(self.out))
        for name in names:
            if " " in name or len(name) > 5:
                self.assertNotIn(name, everything)
        found = {int(n) for n in re.findall(r"(?<!\d)\d{4,}(?!\d)", everything)}
        self.assertFalse(ids & found & set(self.ids))
        # List ids too: no real one anywhere, file names included.
        names = "\n".join(str(p) for p in files(self.out))
        for real in (900500, 900502, 900020, 900010):
            self.assertNotIn(str(real), everything + names)

    def test_seasons_end_at_2099_and_dates_follow(self):
        self.assertEqual(self.report.year_shift, 2099 - 2026)
        self.assertTrue((self.out / "navinfo" / "2099.json").is_file())
        nav = load(self.out / "navinfo" / "2099.json")
        self.assertEqual(nav["seasons"], {"2099": self.ids[900500]})
        self.assertEqual(load(self.out_file("rankings/900500-f-400m-9-p1.json"))["division"]["SeasonID"], 2099)
        meet = load(self.out_file("meets/770001/meet.json"))["meet"]
        self.assertEqual(meet["SeasonID"], 12099 + 73)
        bio = load(self.out_file("1203001.json"))
        self.assertEqual([s["ID"] for s in bio["allSeasons"]], [2099, 12099])
        self.assertEqual(bio["allSeasons"][0]["Display"], "2099 Outdoor")
        before = load(RECORDING / "1203001.json")["resultsTF"]
        for a, b in zip(before, bio["resultsTF"]):
            d = datetime.datetime.fromisoformat(a["ResultDate"])
            moved = d.replace(year=d.year + 73) + datetime.timedelta(days=self.report.date_shift)
            self.assertEqual(datetime.datetime.fromisoformat(b["ResultDate"]), moved)
            self.assertEqual(moved.year, d.year + 73, "a date stays in its season")
            self.assertEqual(b["SeasonID"], a["SeasonID"] + 73)

    def kept_rows(self, rel):
        """(recorded row, its anonymized row) for every athlete kept on the list's pages."""
        after = {}
        for page in sorted(self.out.glob(str(out_rel(rel, self.report)).replace("-p1.json", "-p*.json"))):
            for r in load(page)["groupedRankings"][0]:
                after.setdefault(r["AthleteID"], r)
        pairs = []
        for page in sorted(RECORDING.glob(str(rel).replace("-p1.json", "-p*.json"))):
            for r in load(page)["groupedRankings"][0]:
                new = self.ids.get(r["AthleteID"], 0)
                if r["AthleteID"] and new in after and r["AthleteID"] not in self.report.dropped:
                    pairs.append((r, after[new]))
        return pairs

    def first_pages(self):
        return [rel for rel in files(RECORDING) if rel.parts[0] == "rankings" and rel.stem.endswith("-p1")]

    def test_marks_map_per_event_keeping_order_ties_and_displays(self):
        for rel in self.first_pages():
            pairs = self.kept_rows(rel)  # empty when every athlete on it was dropped
            moved = {}
            for a, b in pairs:
                moved.setdefault(a["SortIntCalc"], set()).add(b["SortIntCalc"])
                self.assertLess(b["SortIntCalc"], a["SortIntCalc"], "marks get better")
                self.assertEqual(b["SortIntRaw"] - a["SortIntRaw"], b["SortIntCalc"] - a["SortIntCalc"])
                self.assertNotEqual(a["display"], b["display"])
                self.assertEqual(self.display_value(b["display"]) - self.display_value(a["display"]),
                                 b["SortIntCalc"] - a["SortIntCalc"], (rel, a["display"], b["display"]))
            self.assertTrue(all(len(v) == 1 for v in moved.values()), rel)
            for (a1, b1), (a2, b2) in zip(pairs, pairs[1:]):
                self.assertEqual((a1["SortIntCalc"] > a2["SortIntCalc"]) - (a1["SortIntCalc"] < a2["SortIntCalc"]),
                                 (b1["SortIntCalc"] > b2["SortIntCalc"]) - (b1["SortIntCalc"] < b2["SortIntCalc"]),
                                 "order and ties among kept athletes stay")

    def test_gaps_are_jittered_not_kept(self):
        """A constant shift would keep the list's gap pattern, which can be matched back to the
        real list; the jitter changes it while keeping every rank and tie."""
        pairs = self.kept_rows("rankings/900500-f-400m-9_10_11_12-p1.json")
        gaps = lambda rows: [b["SortIntCalc"] - a["SortIntCalc"] for a, b in zip(rows, rows[1:])]
        before, after = [a for a, _ in pairs], [b for _, b in pairs]
        self.assertNotEqual(gaps(before), gaps(after))
        self.assertGreater(len({b["SortIntCalc"] - a["SortIntCalc"] for a, b in pairs}), 2)
        for g_before, g_after in zip(gaps(before), gaps(after)):
            self.assertEqual(g_before == 0, g_after == 0, "ties stay ties, nobody new ties")

    def test_no_mark_crosses_a_state_meet_standard(self):
        standards = [s for s in self.report.standards if s["code"] == "400m" and s["gender"] == "f"]
        self.assertTrue(standards, "the CA and ME 400m standards are mapped")
        pairs = self.kept_rows("rankings/900500-f-400m-9_10_11_12-p1.json")
        for std in standards:
            real, mapped = self.display_value(std["display"]), self.display_value(std["new"])
            self.assertLess(mapped, real)
            for a, b in pairs:
                side = lambda v, s: (v > s) - (v < s)
                self.assertEqual(side(self.display_value(a["display"]), real),
                                 side(self.display_value(b["display"]), mapped), (std["display"], a["display"]))

    def test_rows_are_reshuffled(self):
        """Some athletes leave every list; invented ones take places elsewhere; every list keeps
        its length, its pages and its own order; protected athletes all stay."""
        dropped = {self.ids[a] for a in self.report.dropped}
        inserted = set(self.report.inserted)
        self.assertTrue(dropped and len(dropped) == len(inserted))
        protected = {self.ids[a] for a in (1203001, 1205001, 1205005)}
        seen_inserted = set()
        for rel in files(RECORDING):
            if rel.parts[0] != "rankings":
                continue
            before = load(RECORDING / rel)
            after = load(self.out_file(rel))
            rows = after["groupedRankings"][0]
            self.assertEqual(len(rows), len(before["groupedRankings"][0]), rel)
            self.assertEqual(after["minCount"], before["minCount"])
            self.assertEqual([r["rowNum"] for r in rows], [r["rowNum"] for r in before["groupedRankings"][0]], rel)
            ids = [r["AthleteID"] for r in rows]
            self.assertFalse(set(ids) & dropped, rel)
            seen_inserted |= set(ids) & inserted
            real = [r for r in rows if r["AthleteID"]]
            self.assertEqual([r["SortIntCalc"] for r in real], sorted(r["SortIntCalc"] for r in real), rel)
            for a, b in zip(real, real[1:]):
                if a["SortIntCalc"] == b["SortIntCalc"]:
                    self.assertEqual(a["rank"], b["rank"])
                else:
                    self.assertEqual(b["rank"], b["rowNum"], rel)
            for x in protected:
                if self.ids_on(before, x):
                    self.assertIn(x, ids, rel)
        self.assertEqual(seen_inserted, inserted)
        # Two pages: rows 1 to 101, then 100 to 150, the two overlapping rows the same people.
        p1 = load(self.out_file("rankings/900502-m-800m-all-p1.json"))["groupedRankings"][0]
        p2 = load(self.out_file("rankings/900502-m-800m-all-p2.json"))["groupedRankings"][0]
        self.assertEqual((p1[0]["rowNum"], p1[-1]["rowNum"], p2[0]["rowNum"], p2[-1]["rowNum"]), (1, 101, 100, 150))
        self.assertEqual([r["AthleteID"] for r in p1[-2:]], [r["AthleteID"] for r in p2[:2]])
        # No list's kept athletes stand rank for rank as recorded.
        for rel in self.first_pages():
            before = [(r["rank"], self.ids[r["AthleteID"]]) for r in load(RECORDING / rel)["groupedRankings"][0]
                      if r["AthleteID"] and r["AthleteID"] not in self.report.dropped]
            after = [(r["rank"], r["AthleteID"]) for r in load(self.out_file(rel))["groupedRankings"][0]
                     if r["AthleteID"] and r["AthleteID"] not in inserted]
            # Unless every athlete left on it is protected (or none is left).
            if any(a not in protected for _, a in before):
                self.assertNotEqual(before, after, rel)

    def test_grades_are_shuffled_keeping_every_lists_counts(self):
        """Grades swap in pairs among athletes who may go: every list keeps its per-grade counts,
        most of them stand in another grade, protected athletes keep theirs, and every Grade
        group's list holds exactly its grades' people of the whole list, in mark order."""
        count = lambda rows: sorted((g, sum(1 for r in rows if r["GradeID"] == g)) for g in {r["GradeID"] for r in rows})
        real = lambda page: [r for r in page["groupedRankings"][0] if r["AthleteID"]]
        def family(root, rels):
            rows = {}
            for rel in rels:
                for r in real(load(root / rel)):
                    rows.setdefault(r["rowNum"], r)
            return list(rows.values())

        for rel in self.first_pages():
            pages = sorted(RECORDING.glob(str(rel).replace("-p1.json", "-p*.json")))
            recorded = family(RECORDING, [p.relative_to(RECORDING) for p in pages])
            moved = family(self.out, [out_rel(p.relative_to(RECORDING), self.report) for p in pages])
            self.assertEqual(count(moved), count(recorded), rel)
        # Most athletes who may go now stand in another grade.
        before, after = {}, {}
        for rel in files(RECORDING):
            if rel.parts[0] != "rankings":
                continue
            for r in real(load(RECORDING / rel)):
                before[self.ids[r["AthleteID"]]] = r["GradeID"]
            for r in real(load(self.out_file(rel))):
                after[r["AthleteID"]] = r["GradeID"]
        protected = {self.ids[a] for a in (1203001, 1205001, 1205005)}
        for who in protected:
            self.assertEqual(after[who], before[who], "protected grades stay")
        others = [w for w in before if w in after and w not in protected]
        changed = [w for w in others if after[w] != before[w]]
        self.assertGreater(len(changed), len(others) * 0.6, (len(changed), len(others)))
        # A Grade group's list is the whole list's people of those grades (both complete here).
        whole = real(load(self.out_file("rankings/900502-f-400m-9_10_11_12-p1.json")))
        for grades in ("9", "10", "11", "12", "9_10", "9_10_11"):
            want = [r["AthleteID"] for r in whole if str(r["GradeID"]) in grades.split("_")]
            got = [r["AthleteID"] for r in real(load(self.out_file("rankings/900502-f-400m-%s-p1.json" % grades)))]
            self.assertEqual(got, want, grades)
        # The same athlete stands in one grade on every list.
        seen = {}
        for rel in files(self.out):
            if rel.parts[0] == "rankings":
                for r in real(load(self.out / rel)):
                    self.assertEqual(seen.setdefault((r["AthleteID"], rel.stem.split("-")[2]), r["GradeID"]), r["GradeID"])

    def ids_on(self, page, mapped):
        return any(self.ids.get(r["AthleteID"]) == mapped for r in page["groupedRankings"][0])

    def test_the_shifted_areas_copy(self):
        out = Path(self.tmp) / "areas"
        anonymize.write_areas(anonymize.AREAS, out, self.report.standards, self.report.year_shift)
        for std in self.report.standards:
            text = (out / std["file"]).read_text()
            self.assertIn(json.dumps(std["new"]), text)
        # The standards' seasons move with the seasons.
        self.assertIn("2098", load(out / "CA.json")["state_meet"]["at_large"])
        self.assertEqual(sorted(p.name for p in out.iterdir()), sorted(p.name for p in anonymize.AREAS.glob("*.json")))

    def display_value(self, display):
        running = anonymize.parse_running(display)
        if running:
            return running[0]
        total, decimals, _ = anonymize.parse_field(display)
        return -total * 1000 // 10**decimals

    @staticmethod
    def display_delta(after, before):
        return after["SortIntCalc"] - before["SortIntCalc"]

    def test_bio_marks_keep_non_marks_and_suffixes(self):
        before = load(RECORDING / "1203001.json")["resultsTF"]
        after = load(self.out_file("1203001.json"))["resultsTF"]
        for a, b in zip(before, after):
            if a["Result"] == "DNS":
                self.assertEqual((b["Result"], b["SortInt"]), ("DNS", 20000001))
                continue
            self.assertNotEqual(a["Result"], b["Result"])
            self.assertEqual(a["Result"][-1].isalpha(), b["Result"][-1].isalpha())
            if a["Result"][-1].isalpha():
                self.assertEqual(a["Result"][-1], b["Result"][-1])
            self.assertLess(b["SortInt"], a["SortInt"])
            self.assertEqual(self.display_value(b["Result"].rstrip("ah")) - self.display_value(a["Result"].rstrip("ah")),
                             b["SortInt"] - a["SortInt"], a["Result"])

    def test_a_mile_moves_through_its_1600m_estimate(self):
        maps = self.report.event_maps
        self.assertIn("1600 meters", maps["1 mile"].label)
        # Every 4xMile runner's mile-vs-1600m decision is as recorded (also checked by the run).
        for rel in files(RECORDING):
            if len(rel.parts) != 1:
                continue
            results = load(RECORDING / rel)["resultsTF"]
            new = load(self.out_file(rel))["resultsTF"]
            miles = [(a["SortInt"], b["SortInt"]) for a, b in zip(results, new) if a["EventID"] == 59]
            runs = [(a["SortInt"], b["SortInt"]) for a, b in zip(results, new) if a["EventID"] == 23]
            for (m, m2) in miles:
                for (q, q2) in runs:
                    factor = (1600.0, 1609.344)
                    self.assertEqual(anonymize.estimate(m, factor) < q, anonymize.estimate(m2, factor) < q2, rel)

    def test_an_unreviewed_field_stops_the_run(self):
        leaky = Path(self.tmp) / "leaky"
        shutil.copytree(RECORDING, leaky)
        bio = leaky / "1203001.json"
        bio.write_text(bio.read_text().replace('"Gender": "F"', '"Gender": "F", "Mystery": 1203001', 1))
        with self.assertRaises(anonymize.AnonymizeError) as e:
            anonymize.anonymize(leaky, Path(self.tmp) / "leaky-out", SEED)
        # Named by its kind and path, every key it does not know masked: no real id or key.
        self.assertIn("bio: athlete/<key>", str(e.exception))
        self.assertNotIn("Mystery", str(e.exception))
        self.assertNotIn("1203001", str(e.exception))
        self.assertFalse((Path(self.tmp) / "leaky-out").exists(), "nothing is written")

    def test_self_check_fails_on_a_reviewed_field_with_a_real_id(self):
        leaky = Path(self.tmp) / "leaky2"
        shutil.copytree(RECORDING, leaky)
        bio = leaky / "1203001.json"
        bio.write_text(bio.read_text().replace('"Gender": "F"', '"Gender": "F", "Mystery": 1203001', 1))
        anonymize.ALLOWED_KEYS["bio"].add("Mystery")
        try:
            with self.assertRaises(anonymize.AnonymizeError) as e:
                anonymize.anonymize(leaky, Path(self.tmp) / "leaky2-out", SEED)
        finally:
            anonymize.ALLOWED_KEYS["bio"].discard("Mystery")
        self.assertIn("real id", str(e.exception))
        self.assertFalse((Path(self.tmp) / "leaky2-out").exists(), "nothing is written")

    def test_links_and_locations(self):
        noisy = Path(self.tmp) / "noisy"
        shutil.copytree(RECORDING, noisy)
        bio = noisy / "1203001.json"
        bio.write_text(bio.read_text().replace(
            '"Gender": "F"', '"Gender": "F", "Url": "https://www.athletic.net/athlete/1203001/linnea-penhallow", '
            '"Website": "https://linnea.example", "GoogleData": {"Lat": 37.7, "Place": "Port Ellery"}', 1))
        anonymize.anonymize(noisy, Path(self.tmp) / "noisy-out", SEED)
        athlete = load(Path(self.tmp) / "noisy-out" / ("%d.json" % self.ids[1203001]))["athlete"]
        link = athlete["Url"]
        self.assertNotIn("penhallow", link)
        self.assertTrue(link.endswith(anonymize.slug(athlete["FirstName"] + " " + athlete["LastName"])), link)
        self.assertIn(str(self.ids[1203001]), link)
        self.assertEqual(athlete["Website"], "")
        self.assertEqual(athlete["GoogleData"], {"Lat": 0.0, "Place": ""})

    def test_refuses_a_non_empty_output_and_stray_files(self):
        with self.assertRaises(anonymize.AnonymizeError):
            anonymize.anonymize(RECORDING, self.out, SEED)
        stray = Path(self.tmp) / "stray"
        shutil.copytree(RECORDING, stray)
        (stray / "notes.txt").write_text("hi")
        with self.assertRaises(anonymize.AnonymizeError):
            anonymize.anonymize(stray, Path(self.tmp) / "stray-out", SEED)

    def test_the_map_file_stays_out_of_tracked_paths(self):
        inside = subprocess.run(["git", "-C", str(HERE), "rev-parse", "--is-inside-work-tree"],
                                capture_output=True, text=True)
        if inside.returncode != 0 or inside.stdout.strip() != "true":
            self.skipTest("the source tree is not a git work tree, so its guard cannot be asked")
        with self.assertRaises(anonymize.AnonymizeError):
            anonymize.check_private(HERE / "map.json")
        anonymize.check_private(HERE.parents[1] / "local" / "maps" / "map.json")


def variant(tmp, name, replace=(), edit=None):
    """A copy of the made-up Recording under `tmp/name`, with each (old, new) text replaced in
    every file and `edit(root)` applied."""
    root = Path(tmp) / name
    shutil.copytree(RECORDING, root)
    for path in root.rglob("*.json"):
        text = path.read_text()
        for old, new in replace:
            text = text.replace(old, new)
        path.write_text(text)
    if edit:
        edit(root)
    return root


def out_rel(rel, report):
    """Where the recording's `rel` lands in the anonymized copy."""
    ids, years = report.id_map, report.year_shift
    rel = Path(rel)
    parts = list(rel.parts)
    if len(parts) == 1:
        parts[0] = "%d.json" % ids[int(rel.stem)]
    elif parts[0] == "teamnav":
        school, season = rel.stem.split("-")
        parts[1] = "%d-%d.json" % (ids[int(school)], int(season) + years)
    elif parts[0] == "meets":
        parts[1] = str(ids[int(parts[1])])
    elif parts[0] == "navinfo":
        bits = rel.stem.split("-")
        parts[1] = str(int(bits[0]) + years) + ("-%d" % ids[int(bits[1])] if len(bits) > 1 else "") + ".json"
    elif parts[0] == "rankings":
        list_id, rest = rel.stem.split("-", 1)
        parts[1] = "%d-%s.json" % (ids[int(list_id)], rest)
    return Path(*parts)


def built(recording, seed=SEED):
    """An Anonymizer with `recording` read and its maps built (nothing written)."""
    a = anonymize.Anonymizer(anonymize.seed_key(seed), anonymize.AREAS)
    files = a.read(Path(recording))
    a.collect(files)
    a.build()
    return a


class Edges(unittest.TestCase):
    """The review's edge cases."""

    def setUp(self):
        self.tmp = tempfile.mkdtemp()

    def tearDown(self):
        shutil.rmtree(self.tmp)

    def test_the_output_never_holds_the_recording_and_replace_needs_the_marker(self):
        rec = variant(self.tmp, "parent/rec")
        with self.assertRaises(anonymize.AnonymizeError):
            anonymize.anonymize(rec, Path(self.tmp) / "parent", SEED, replace=True)
        with self.assertRaises(anonymize.AnonymizeError):
            anonymize.anonymize(rec, rec / "out", SEED)
        self.assertTrue((rec / "1203001.json").is_file(), "the recording is untouched")
        other = Path(self.tmp) / "other"
        other.mkdir()
        (other / "keep.txt").write_text("mine")
        with self.assertRaises(anonymize.AnonymizeError) as e:
            anonymize.anonymize(rec, other, SEED, replace=True)
        self.assertIn("not an earlier anonymizer output", str(e.exception))
        self.assertTrue((other / "keep.txt").is_file())
        out = Path(self.tmp) / "out"
        anonymize.anonymize(rec, out, SEED)
        self.assertTrue((out / anonymize.MARKER).is_file())
        anonymize.anonymize(rec, out, SEED + 1, replace=True)

    def test_a_kept_kind_that_is_also_a_real_name(self):
        rec = variant(self.tmp, "rec", replace=[
            ("Saoirse", "Christian"), ("Saltmarsh Christian", "Fernbrook Christian"),
            ("Lisle", "Park"), ("Harrowgate Fieldhouse", "Harrowgate Park")])
        out = Path(self.tmp) / "out"
        report = anonymize.anonymize(rec, out, SEED)
        self.assertTrue(report.value_map["Fernbrook Christian"].endswith(" Christian"))
        self.assertNotEqual(report.value_map["Fernbrook Christian"], "Fernbrook Christian")
        self.assertTrue(report.value_map["Harrowgate Park"].endswith(" Park"))

    def test_invented_first_names_avoid_real_cities(self):
        rec = variant(self.tmp, "rec", replace=[("Callowmere", "Dallas"), ("Sunder Flats", "Avery")])
        a = built(rec)
        names = {a.first("Firstname%d" % i).lower() for i in range(400)}
        self.assertTrue(names)
        self.assertFalse(names & {"dallas", "avery"})
        # A stand-in's first name too.
        stand_ins = {a.invented_person(n).split()[0].lower() for n in range(400)}
        self.assertFalse(stand_ins & {"dallas", "avery"})

    def test_a_whole_second_mile(self):
        def whole(root):
            for path in root.glob("*.json"):
                bio = json.loads(path.read_text())
                changed = False
                for r in bio["resultsTF"]:
                    if r["EventID"] == 59:
                        r["SortInt"] = r["SortInt"] // 1000 * 1000
                        r["Result"] = "%d:%02d" % divmod(r["SortInt"] // 1000, 60)
                        changed = True
                if changed:
                    path.write_text(json.dumps(bio, indent=1))
        rec = variant(self.tmp, "rec", edit=whole)
        out = Path(self.tmp) / "out"
        report = anonymize.anonymize(rec, out, SEED)
        self.assertEqual(report.event_maps["1 mile"].unit, 1000)
        miles = [r for p in out.glob("*.json") for r in load(p)["resultsTF"] if r["EventID"] == 59]
        self.assertTrue(miles)
        for r in miles:
            self.assertRegex(r["Result"], r"^\d+:\d\d$")
            self.assertEqual(r["SortInt"] % 1000, 0)

    def test_search_names_are_unique_and_follow_the_recorded_query(self):
        self.assertEqual(anonymize.search_fixture_name("José Núñez"), "jos-n-ez-43884fd3")
        self.assertEqual(anonymize.search_fixture_name("Jos N ez"), "jos-n-ez")
        # Pinned the same in src/source.rs: an inexact spelling always gets the hash.
        self.assertEqual(anonymize.search_fixture_name("MARY KATE DOE"), "mary-kate-doe")
        self.assertEqual(anonymize.search_fixture_name("Mary-Kate Doe"), "mary-kate-doe-74813ee1")
        self.assertEqual(anonymize.search_fixture_name("O'Neil"), "o-neil-eb1aed4f")
        self.assertEqual(anonymize.search_fixture_name("O Neil"), "o-neil")
        self.assertEqual(anonymize.search_fixture_name("Jane  Doe"), "jane-doe-760946a8")
        self.assertEqual(anonymize.search_fixture_name("山田 太郎"), "ed6d53c9")

        def searches(root):
            for query in ("Mary Ann Smith", "José Núñez"):
                body = {"athQuery": query, "response": {"numFound": 0, "docs": []}}
                (root / "search" / (anonymize.search_fixture_name(query) + ".json")).write_text(json.dumps(body))
        rec = variant(self.tmp, "rec", edit=searches)
        out = Path(self.tmp) / "out"
        report = anonymize.anonymize(rec, out, SEED)
        for query in ("Mary Ann Smith", "José Núñez"):
            mapped = report.value_map[query]
            path = out / "search" / (anonymize.search_fixture_name(mapped) + ".json")
            self.assertEqual(load(path)["athQuery"], mapped)
        mary = report.value_map["Mary Ann Smith"].split(" ")
        self.assertEqual(len(mary), 2, "first name, then the rest as the last name")

    def test_a_points_event_is_left_alone(self):
        def pentathlon(root):
            path = root / "1203001.json"
            bio = json.loads(path.read_text())
            bio["eventsTF"].append({"Event": "Pentathlon", "Type": "F", "IDEvent": 70, "IDEventType": 0})
            bio["resultsTF"].append({"AthleteID": 1203001, "Result": "3456", "SortInt": 3456, "EventID": 70,
                                     "EventTypeID": 0, "SeasonID": 2026, "Exhibition": False,
                                     "ResultDate": "2026-04-11T00:00:00", "MeetID": 660101})
            path.write_text(json.dumps(bio, indent=1))
        rec = variant(self.tmp, "rec", edit=pentathlon)
        out = Path(self.tmp) / "out"
        report = anonymize.anonymize(rec, out, SEED)
        bio = load(out / ("%d.json" % report.id_map[1203001]))
        points = [r for r in bio["resultsTF"] if r["EventID"] == 70]
        self.assertEqual((points[0]["Result"], points[0]["SortInt"]), ("3456", 3456))
        self.assertNotIn("pentathlon", report.event_maps)

    def test_metric_and_feet_inches_field_marks_together(self):
        def metric(root):
            path = root / "rankings/900502-f-Shot-all-p1.json"
            page = json.loads(path.read_text())
            for row in page["groupedRankings"][0][::2]:
                inches = (20_000_000 - row["SortIntCalc"]) / 1000
                row["display"] = "%.2fm" % (int(inches * 2.54) / 100)
            path.write_text(json.dumps(page, indent=1))
        rec = variant(self.tmp, "rec", edit=metric)
        out = Path(self.tmp) / "out"
        report = anonymize.anonymize(rec, out, SEED)
        rows = load(out / out_rel("rankings/900502-f-Shot-all-p1.json", report))["groupedRankings"][0]
        by_id = {r["AthleteID"]: r for r in rows}
        before, after = [], []
        for r in load(rec / "rankings/900502-f-Shot-all-p1.json")["groupedRankings"][0]:
            new = by_id.get(report.id_map[r["AthleteID"]])
            if new is not None:
                before.append(r)
                after.append(new)
        self.assertGreater(len(before), 2)

        def inches(display):
            if display.endswith("m"):
                return float(display[:-1]) / 0.0254
            total, decimals, _ = anonymize.parse_field(display)
            return total / 10 ** decimals

        self.assertTrue(any(r["display"].endswith("m") for r in after))
        self.assertEqual([anonymize.display_value(r["display"], "field") for r in rows],
                         sorted(anonymize.display_value(r["display"], "field") for r in rows), "one scale")
        for (a, b), (c, d) in zip(zip(before, after), list(zip(before, after))[1:]):
            self.assertEqual(a["display"].endswith("m"), b["display"].endswith("m"))
            self.assertGreater(inches(b["display"]), inches(a["display"]), "farther")
            if a["SortIntCalc"] < c["SortIntCalc"]:
                self.assertLess(b["SortIntCalc"], d["SortIntCalc"])
                self.assertGreater(inches(b["display"]), inches(d["display"]))

    def test_a_mile_standard_is_mapped_and_never_crossed(self):
        areas = Path(self.tmp) / "areas"
        areas.mkdir()
        (areas / "ZZ.json").write_text(json.dumps({"state": "ZZ", "state_meet": {"standards": {"2026": {
            "A": {"f": {"1mile": ["5:06.00", "5:12.00"], "1600m": ["5:02.00"]}}}}}}, indent=1))
        out = Path(self.tmp) / "out"
        report = anonymize.anonymize(RECORDING, out, SEED, areas_dir=areas)
        miles = [s for s in report.standards if s["code"] == "1mile"]
        self.assertEqual(len(miles), 2)
        marks = []
        for path in RECORDING.glob("*.json"):
            for r, n in zip(load(path)["resultsTF"], load(out / ("%d.json" % report.id_map[int(path.stem)]))["resultsTF"]):
                if r["EventID"] == 59:
                    marks.append((anonymize.parse_running(r["Result"])[0], anonymize.parse_running(n["Result"])[0]))
        self.assertTrue(marks)
        for std in miles:
            real, mapped = anonymize.parse_running(std["display"])[0], anonymize.parse_running(std["new"])[0]
            self.assertLess(mapped, real)
            for a, b in marks:
                self.assertEqual((a > real) - (a < real), (b > mapped) - (b < mapped), (std["display"], a))

    def test_dotfiles_are_skipped_and_other_strays_refused(self):
        def temp(root):
            (root / ".1203001.json.123.0.tmp").write_text("{half")
            (root / ".anonymized").write_text("x")
        rec = variant(self.tmp, "rec", edit=temp)
        anonymize.anonymize(rec, Path(self.tmp) / "out", SEED)
        (rec / "notes.txt").write_text("hi")
        with self.assertRaises(anonymize.AnonymizeError):
            anonymize.anonymize(rec, Path(self.tmp) / "out2", SEED)

    def test_the_cli_key_file_and_destinations(self):
        repo = Path(self.tmp) / "repo"
        subprocess.run(["git", "init", "-q", str(repo)], check=True)
        (repo / ".gitignore").write_text("/local/\n")
        out = Path(self.tmp) / "out"
        key = repo / "local" / "anonymize.key"
        run = lambda *args: subprocess.run([sys.executable, str(HERE / "anonymize.py"), str(RECORDING)] + list(args),
                                           capture_output=True, text=True)
        # A key file git would track is refused before anything is written.
        done = run(str(out), "--key-file", str(repo / "anonymize.key"))
        self.assertNotEqual(done.returncode, 0)
        self.assertFalse(out.exists() or (repo / "anonymize.key").exists())
        # A map file git would track: refused, and nothing written either.
        done = run(str(out), "--key-file", str(key), "--map-out", str(repo / "map.json"))
        self.assertNotEqual(done.returncode, 0)
        self.assertFalse(out.exists() or key.exists())
        # Made when missing, 64 hex digits, never printed; the same key gives the same output.
        done = run(str(out), "--key-file", str(key), "--map-out", str(repo / "local" / "map.json"))
        self.assertEqual(done.returncode, 0, done.stderr)
        secret = key.read_text().strip()
        self.assertRegex(secret, r"^[0-9a-f]{64}$")
        self.assertNotIn(secret, done.stdout + done.stderr)
        self.assertIn(str(key), done.stdout)
        self.assertNotIn(secret, (repo / "local" / "map.json").read_text())
        first = {p: (out / p).read_bytes() for p in files(out)}
        done = run(str(out), "--key-file", str(key), "--replace")
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertEqual(first, {p: (out / p).read_bytes() for p in files(out)})
        # Neither a key nor a seed: refused.
        self.assertNotEqual(run(str(Path(self.tmp) / "out3")).returncode, 0)

    def test_destinations_are_checked_before_writing(self):
        blocker = Path(self.tmp) / "file"
        blocker.write_text("not a folder")
        done = subprocess.run([sys.executable, str(HERE / "anonymize.py"), str(RECORDING), str(Path(self.tmp) / "out"),
                               "--seed", "3", "--areas-out", str(blocker / "areas")], capture_output=True, text=True)
        self.assertNotEqual(done.returncode, 0)
        self.assertFalse((Path(self.tmp) / "out").exists(), "checked before writing")

    def test_a_list_mixing_hand_and_fat_times_sorts_on_sortint(self):
        """57.9h sorts after 58.10 (its SortInt adds the hand-time conversion): the map keys on
        SortInt, so nothing flips, and each display keeps its own relation to its SortInt."""
        def mixed(root):
            template = json.loads((root / "rankings/900502-f-Shot-all-p1.json").read_text())
            base = template["groupedRankings"][0][0]
            marks = [("57.40", 57400), ("57.81", 57810), ("58.10", 58100), ("57.9h", 58140), ("58.33", 58330),
                     ("58.60", 58600), ("58.9h", 59140), ("59.20", 59200), ("59.71", 59710), ("60.05", 60050)]
            rows = []
            for n, (display, sort) in enumerate(marks, start=1):
                rows.append(dict(base, rank=n, rowNum=n, AthleteID=1208000 + n, AthleteName="Ione Quarrington%d" % n,
                                 display=display, SortIntRaw=sort, SortIntCalc=sort, SortIntOrig=sort,
                                 Event="400 Meters", EventShort="400m", GradeID=9 + n % 4,
                                 TeamID=4999, TeamName="Fenwold Academy"))
            page = dict(template, eventShort="400m", groupedRankings=[rows], minCount=len(rows))
            page["division"] = dict(template["division"], ID=900999, Name="Made-up League")
            (root / "rankings/900999-f-400m-all-p1.json").write_text(json.dumps(page, indent=1))
        rec = variant(self.tmp, "rec", edit=mixed)
        out = Path(self.tmp) / "out"
        report = anonymize.anonymize(rec, out, SEED)
        before = {report.id_map[r["AthleteID"]]: r for r in load(rec / "rankings/900999-f-400m-all-p1.json")["groupedRankings"][0]}
        after = load(out / out_rel("rankings/900999-f-400m-all-p1.json", report))["groupedRankings"][0]
        sorts = [r["SortIntCalc"] for r in after]
        self.assertEqual(sorts, sorted(sorts))
        kept = [(before[r["AthleteID"]], r) for r in after if r["AthleteID"] in before]
        self.assertTrue(kept)
        for a, b in kept:
            relation = lambda r: anonymize.parse_running(r["display"])[0] - r["SortIntCalc"]
            self.assertEqual(relation(a), relation(b), (a["display"], b["display"]))
            self.assertEqual(a["display"].endswith("h"), b["display"].endswith("h"))
        for (a1, b1), (a2, b2) in zip(kept, kept[1:]):
            self.assertLess(a1["SortIntCalc"], a2["SortIntCalc"])
            self.assertLess(b1["SortIntCalc"], b2["SortIntCalc"])

    def test_two_seasons_of_one_list(self):
        """2025's and 2026's lists share athletes: each season's marks and grades are its own,
        an athlete's grade moves by one amount in both, and a stand-in takes each season's mark."""
        def earlier(root):
            page = json.loads((root / "rankings/900502-f-400m-9_10_11_12-p1.json").read_text())
            rows = [dict(r, GradeID=r["GradeID"] - 1, SortIntRaw=r["SortIntRaw"] + 500, SortIntCalc=r["SortIntCalc"] + 500,
                         SortIntOrig=r["SortIntOrig"] + 500, display=anonymize.format_running(r["SortIntCalc"] + 500, 2, ""),
                         ResultDate=r["ResultDate"].replace("2026", "2025"), SeasonID=2025)
                    for r in page["groupedRankings"][0] if r["GradeID"] > 9]
            for n, r in enumerate(rows, start=1):
                r["rowNum"] = n
                r["rank"] = n if n == 1 or rows[n - 2]["SortIntCalc"] != r["SortIntCalc"] else rows[n - 2]["rank"]
            page = dict(page, groupedRankings=[rows], minCount=len(rows))
            page["division"] = dict(page["division"], ID=900499, SeasonID=2025)
            page["settings"] = dict(page["settings"], grades=[9, 10, 11, 12])
            (root / "rankings/900499-f-400m-9_10_11_12-p1.json").write_text(json.dumps(page, indent=1))
        rec = variant(self.tmp, "rec", edit=earlier)
        out = Path(self.tmp) / "out"
        report = anonymize.anonymize(rec, out, SEED)
        old = {r["AthleteID"]: r for r in load(out / out_rel("rankings/900499-f-400m-9_10_11_12-p1.json", report))["groupedRankings"][0]}
        new = {r["AthleteID"]: r for r in load(out / out_rel("rankings/900502-f-400m-9_10_11_12-p1.json", report))["groupedRankings"][0]}
        both = set(old) & set(new)
        self.assertTrue(both)
        for who in both:
            self.assertEqual(new[who]["GradeID"] - old[who]["GradeID"], 1, who)
            self.assertEqual(old[who]["SeasonID"], 2098)
            self.assertNotEqual(old[who]["SortIntCalc"], new[who]["SortIntCalc"])
        invented = set(report.inserted)
        self.assertTrue(invented & set(old) or invented & set(new))

    def test_empty_and_masked_only_pages(self):
        def pages(root):
            template = json.loads((root / "rankings/900500-f-400m-all-p1.json").read_text())
            masked = template["groupedRankings"][0][-1]
            empty = dict(template, eventShort="100m", groupedRankings=[[]])
            (root / "rankings/900500-f-100m-all-p1.json").write_text(json.dumps(empty, indent=1))
            only = dict(template, eventShort="100m", groupedRankings=[[dict(masked, rank=1, rowNum=1)]])
            (root / "rankings/900502-f-100m-all-p1.json").write_text(json.dumps(only, indent=1))
        rec = variant(self.tmp, "rec", edit=pages)
        out = Path(self.tmp) / "out"
        report = anonymize.anonymize(rec, out, SEED)
        self.assertEqual(load(out / out_rel("rankings/900500-f-100m-all-p1.json", report))["groupedRankings"], [[]])
        rows = load(out / out_rel("rankings/900502-f-100m-all-p1.json", report))["groupedRankings"][0]
        self.assertEqual([(r["AthleteID"], r["rowNum"]) for r in rows], [(0, 1)])
        # The capped list keeps its Masked tail last, where it was.
        capped = load(out / out_rel("rankings/900500-f-400m-all-p1.json", report))["groupedRankings"][0]
        self.assertEqual((capped[-1]["AthleteID"], capped[-1]["rowNum"], len(capped)), (0, 26, 26))

    def test_a_masked_row_in_the_middle_is_refused(self):
        def middle(root):
            path = root / "rankings/900500-f-400m-all-p1.json"
            page = json.loads(path.read_text())
            rows = page["groupedRankings"][0]
            rows.insert(3, rows.pop())
            path.write_text(json.dumps(page, indent=1))
        rec = variant(self.tmp, "rec", edit=middle)
        with self.assertRaises(anonymize.AnonymizeError) as e:
            anonymize.anonymize(rec, Path(self.tmp) / "out", SEED)
        self.assertIn("masked row with people after it", str(e.exception))

    def test_a_dangling_symlink_out_of_local_is_refused(self):
        repo = Path(self.tmp) / "repo"
        subprocess.run(["git", "init", "-q", str(repo)], check=True)
        (repo / ".gitignore").write_text("/local/\n")
        (repo / "local").mkdir()
        (repo / "tests").mkdir()
        os.symlink(str(repo / "tests" / "map.json"), str(repo / "local" / "map.json"))
        with self.assertRaises(anonymize.AnonymizeError):
            anonymize.check_private(repo / "local" / "map.json")
        os.symlink(str(repo / "tests"), str(repo / "local" / "dir"))
        with self.assertRaises(anonymize.AnonymizeError):
            anonymize.check_private(repo / "local" / "dir" / "map.json")
        # A symlink inside local/ to local/ passes the guard but is never written through.
        os.symlink(str(repo / "local" / "real.json"), str(repo / "local" / "link.json"))
        anonymize.check_private(repo / "local" / "link.json")
        with self.assertRaises(anonymize.AnonymizeError):
            anonymize.no_symlink(repo / "local" / "link.json")
        self.assertFalse((repo / "tests" / "map.json").exists())

    def test_areas_out_never_writes_over_the_area_files(self):
        run = lambda *args: subprocess.run([sys.executable, str(HERE / "anonymize.py"), str(RECORDING),
                                            str(Path(self.tmp) / "out"), "--seed", "1"] + list(args),
                                           capture_output=True, text=True)
        for target in (anonymize.AREAS, anonymize.AREAS.parent):
            done = run("--areas-out", str(target))
            self.assertNotEqual(done.returncode, 0, target)
            self.assertIn("apart from the Area files", done.stderr)
        mine = Path(self.tmp) / "myareas"
        shutil.copytree(anonymize.AREAS, mine)
        done = run("--areas", str(mine), "--areas-out", str(mine))
        self.assertNotEqual(done.returncode, 0)
        self.assertFalse((Path(self.tmp) / "out").exists())
        self.assertEqual(subprocess.run(["git", "-C", str(HERE), "status", "--porcelain", "--", str(anonymize.AREAS)],
                                        capture_output=True, text=True).stdout, "")

    def test_a_public_number_equal_to_a_real_id_is_only_noted(self):
        rec = variant(self.tmp, "rec", replace=[("4105", "22")])
        report = anonymize.anonymize(rec, Path(self.tmp) / "out", SEED)
        self.assertGreater(report.collisions, 0, "girls' 800m is event 22, and so is a school now")

    def test_the_privacy_guard_asks_git(self):
        repo = Path(self.tmp) / "repo"
        subprocess.run(["git", "init", "-q", str(repo)], check=True)
        (repo / ".gitignore").write_text("/local/\n/scratch/\n")
        for bad in ("map.json", "tests/map.json", "src/local/map.json", "local/../map.json"):
            with self.assertRaises(anonymize.AnonymizeError, msg=bad):
                anonymize.check_private(repo / bad)
        for good in ("local/maps/map.json", "scratch/map.json"):
            anonymize.check_private(repo / good)
        anonymize.check_private(Path(self.tmp) / "outside" / "map.json")



class RealShapes(unittest.TestCase):
    """Shapes seen in real responses (recorded 2026-11-09), made up here: the fields reviewed
    then, and the lists' own quirks (ties ranked apart, rows whose grade their list does not
    explain, pages read at different moments, every State's list at once)."""

    def setUp(self):
        self.tmp = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, self.tmp)

    def test_list_unreviewed_names_paths_never_values(self):
        def plant(root):
            path = root / "1203001.json"
            bio = json.loads(path.read_text())
            bio["athlete"]["Mystery"] = 1203001
            bio["athlete"]["Nickname"] = "Linnea Penhallow"
            bio["Odd"] = {"77123": 5}
            bio["Pals"] = {"Linnea Penhallow": {"Event": "400 Meters"}}  # keyed by a name
            bio["byTeam"] = {"4101_2026": 9}  # keyed by `<id>_<season>`
            path.write_text(json.dumps(bio, indent=1))
        rec = variant(self.tmp, "rec", edit=plant)
        out = io.StringIO()
        with redirect_stdout(out):
            code = anonymize.main([str(rec), "--list-unreviewed"])
        text = out.getvalue()
        self.assertEqual(code, 0)
        # Every key no one knows is masked; a known one (`athlete`, `Event`) shows.
        self.assertRegex(text, r"bio\s+athlete/<key>\s+2\s+2 keys\s+int, string\n")
        self.assertRegex(text, r"bio\s+<key>\s+3\s+3 keys\s+object\n")
        self.assertRegex(text, r"bio\s+<key>/<key>\s+3\s+3 keys\s+int, object\n")
        self.assertIn("3 unreviewed key paths", text)
        for value in ("1203001", "Linnea", "Penhallow", "77123", "Mystery", "Nickname", "Pals", "4101", "2026"):
            self.assertNotIn(value, text)

    def test_fields_seen_in_real_responses(self):
        def real(root):
            path = root / "1203001.json"
            bio = json.loads(path.read_text())
            bio["athlete"].update(Handle="LinneaP", age=16, UsatfId="1234567890", PhotoUrl=None, NoSearch=False,
                                  isClaimed=True, rsMugshot=None)
            for s in bio["allSeasons"]:
                s.update(IDSeason=s["ID"], Selected=True, age=16)
            bio["allTeams"]["4101"].update(MascotUrl="//cdn.example/wren.png", PrefMetric=0, PrefConvert=1, TeamCode="WRNF")
            bio["relayTeamMembers"] = [{"Name": "Wren Halvorsen-Halvorsen", "SortID": 1, "TeamID": 7700123}]
            bio["synonyms"] = [{"RealFirstName": "Linnea"}]
            first = bio["resultsTF"][0]
            first.update(Place="3", shortCode="abcDEF123ghiJKL45", MediaCount={"photos": 2}, SortIntRaw=first["SortInt"],
                         Blurred=False, Division="Varsity", DivisionShort="V", ignoreDay=None, ignoreMonth=None)
            path.write_text(json.dumps(bio, indent=1))

            # Every row of every list has them, as on Athletic.net.
            for path in root.glob("rankings/*.json"):
                page = json.loads(path.read_text())
                if path.name == "900500-f-400m-all-p1.json":
                    page["division"].update(BaseDiv={"ID": 903, "ParentID": 902, "Name": "California",
                                                     "Country": "USA", "Note": None, "Level": 4, "Type": "State"},
                                            ImageUrl="//cdn.example/ca.png", EventSelectionID=3)
                    page["settings"]["disabledFilters"] = ["altitude"]
                for row in page["groupedRankings"][0]:
                    if row["AthleteID"]:
                        n = row["AthleteID"]
                        row.update(Handle="Handle%d" % n, Age=16, Country="CAN", PhotoUrl="//cdn.example/p%d.jpg" % n,
                                   TeamMascot="//cdn.example/m.png", shortCode="code%d" % n, mediaCount={"photos": 1},
                                   MediaCountJson='{"photos":1}', IDResult=98000000 + n)
                path.write_text(json.dumps(page, indent=1))

            path = root / "teamnav/4101-2026.json"
            nav = json.loads(path.read_text())
            nav["team"].update(Mascot="Wrens", MascotUrl="//cdn.example/wren.png", colors=["#123456"],
                               coverPhoto="//cdn.example/c.jpg", Country="USA", ReplacementID=None)
            nav["customLists"] = {"31337": {"IDCustomList": 31337, "Name": "Linnea's list", "Description": "Our best",
                                            "PublicList": 1, "Type": "S"}}
            nav["customDivisions"] = [{"IDDivision": 712345, "DivName": "Wren League"}]
            nav["grades"] = [{"IDGrade": 9, "GradeDesc": "9th Grade"}]
            nav["linkedTeams"] = [{"IDSchool": 4102, "SchoolName": "Tidewater High School", "Level": 4}]
            for d in nav["divisions"]:
                d["gender"] = "x"
            path.write_text(json.dumps(nav, indent=1))

            path = root / "navinfo/2026.json"
            tree = json.loads(path.read_text())
            tree["levels"] = [{"id": 900020, "name": "High School", "state": None, "divType": "Level",
                               "subDivType": "State", "depth": 0, "customDivision": None}]
            tree["countries"] = [{"id": 900010, "name": "United States", "state": None, "divType": None,
                                  "subDivType": None, "depth": None, "customDivision": None}]
            tree["events"] = [{"id": 21, "name": "400 Meters", "short": "400m", "fmt": None, "h": False, "m": "E",
                               "r": False, "so": 400, "t": "T", "w": False}]
            tree["events_main"] = [21]
            path.write_text(json.dumps(tree, indent=1))

        rec = variant(self.tmp, "rec", edit=real)
        out = Path(self.tmp) / "out"
        report = anonymize.anonymize(rec, out, SEED)
        ids = report.id_map

        bio = load(out / ("%d.json" % ids[1203001]))
        self.assertEqual((bio["athlete"]["Handle"], bio["athlete"]["age"], bio["athlete"]["UsatfId"]), ("", 0, ""))
        self.assertEqual(sorted(s["IDSeason"] for s in bio["allSeasons"]), [2099, 12099])
        self.assertEqual({s["age"] for s in bio["allSeasons"]}, {0})
        team = bio["allTeams"][str(ids[4101])]
        self.assertEqual(team["MascotUrl"], "")
        self.assertNotEqual(team["TeamCode"], "WRNF")
        self.assertEqual(bio["synonyms"], [{"RealFirstName": ""}])
        first = bio["resultsTF"][0]
        self.assertEqual((first["Place"], first["shortCode"], first["MediaCount"]), ("", "", {"photos": 0}))
        self.assertEqual(first["Division"], "Varsity")
        # A relay teammate is invented as on the lists, and the relay team's id moves.
        member = bio["relayTeamMembers"][0]
        self.assertEqual(member["Name"], report.value_map["Wren Halvorsen-Halvorsen"])
        self.assertEqual(member["TeamID"], ids[7700123])

        page = load(out / out_rel("rankings/900500-f-400m-all-p1.json", report))
        self.assertEqual((page["division"]["BaseDiv"]["ID"], page["division"]["BaseDiv"]["ParentID"]), (ids[903], ids[902]))
        self.assertEqual(page["division"]["BaseDivID"], ids[903])
        self.assertEqual(page["division"]["ImageUrl"], "")
        self.assertEqual(page["settings"]["disabledFilters"], ["altitude"])
        for row in page["groupedRankings"][0]:
            if row["AthleteID"]:
                self.assertEqual((row["Handle"], row["Age"], row["Country"], row["PhotoUrl"], row["TeamMascot"],
                                  row["shortCode"], row["mediaCount"], row["MediaCountJson"]),
                                 ("", 0, "", "", "", "", {"photos": 0}, ""))
                self.assertGreaterEqual(row["IDResult"], anonymize.RESERVED_BASE)

        nav = load(out / out_rel("teamnav/4101-2026.json", report))
        self.assertEqual((nav["team"]["Mascot"], nav["team"]["MascotUrl"], nav["team"]["colors"], nav["team"]["coverPhoto"]),
                         ("", "", [""], ""))
        self.assertEqual(nav["customLists"], {str(ids[31337]): {"IDCustomList": ids[31337], "Name": "", "Description": "",
                                                                "PublicList": 1, "Type": "S"}})
        self.assertEqual(nav["customDivisions"], [{"IDDivision": ids[712345], "DivName": ""}])
        self.assertEqual(nav["linkedTeams"][0]["IDSchool"], ids[4102])
        self.assertEqual(nav["linkedTeams"][0]["SchoolName"], report.value_map["Tidewater High School"])

        tree = load(out / "navinfo/2099.json")
        self.assertEqual((tree["levels"][0]["id"], tree["countries"][0]["id"]), (ids[900020], ids[900010]))
        self.assertEqual((tree["events"][0]["id"], tree["events_main"]), (21, [21]))

    def test_a_kept_kind_that_is_a_real_name_after_reshuffling(self):
        # A real city named like a kept kind: the rebuilt rows' invented schools keep the kind.
        rec = variant(self.tmp, "rec", replace=[("Sunder Flats", "Christian")])
        report = anonymize.anonymize(rec, Path(self.tmp) / "out", SEED)
        self.assertTrue(report.value_map["Saltmarsh Christian"].endswith(" Christian"))
        self.assertTrue(report.dropped)

    def test_the_latest_season_with_data_is_2099_and_a_tree_may_run_ahead(self):
        def ahead(root):
            path = root / "navinfo/2026.json"
            tree = json.loads(path.read_text())
            tree["seasons"] = {"2026": 900500, "2027": 900600}
            path.write_text(json.dumps(tree, indent=1))
        rec = variant(self.tmp, "rec", edit=ahead)
        report = anonymize.anonymize(rec, Path(self.tmp) / "out", SEED)
        self.assertEqual(report.year_shift, 73, "2026 has the data; the tree's 2027 does not count")
        seasons = load(Path(self.tmp) / "out" / "navinfo/2099.json")["seasons"]
        self.assertEqual(sorted(seasons), ["2099", "2100"])
        # Pinned: the same real season becomes 2099 in every Recording used together.
        pinned = anonymize.anonymize(rec, Path(self.tmp) / "pinned", SEED, latest=2027)
        self.assertEqual(pinned.year_shift, 72)
        self.assertTrue((Path(self.tmp) / "pinned" / "navinfo/2098.json").exists())

    def test_no_team_stays_team_zero(self):
        def unattached(root):
            path = root / "1203001.json"
            bio = json.loads(path.read_text())
            bio["allTeams"]["0"] = {"IDSchool": 0, "SchoolName": "Unattached", "Level": 16, "Year": 2026}
            bio["grades"]["0_2026"] = 9
            path.write_text(json.dumps(bio, indent=1))
        rec = variant(self.tmp, "rec", edit=unattached)
        report = anonymize.anonymize(rec, Path(self.tmp) / "out", SEED)
        bio = load(Path(self.tmp) / "out" / ("%d.json" % report.id_map[1203001]))
        self.assertEqual(bio["allTeams"]["0"]["IDSchool"], 0)
        self.assertEqual(bio["grades"]["0_2099"], 9)
        self.assertNotIn(0, report.id_map)

    def test_a_list_with_no_bio_knows_its_rows_event(self):
        rec = Path(self.tmp) / "rec"
        (rec / "rankings").mkdir(parents=True)
        page = load(RECORDING / "rankings/900502-f-400m-9_10_11_12-p1.json")
        for row in page["groupedRankings"][0]:
            row["EventID"] = 21
        (rec / "rankings/900502-f-400m-9_10_11_12-p1.json").write_text(json.dumps(page, indent=1))
        report = anonymize.anonymize(rec, Path(self.tmp) / "out", SEED)
        self.assertIn("400 meters", report.event_maps)

    def set_400m(self, root, athlete, **fields):
        """`fields` on `athlete`'s row in every 2026 girls' 400m list."""
        for path in root.glob("rankings/*-f-400m-*.json"):
            page = json.loads(path.read_text())
            for row in page["groupedRankings"][0]:
                if row["AthleteID"] == athlete:
                    for k, v in fields.items():
                        if v is None:
                            row.pop(k, None)
                        else:
                            row[k] = v
            path.write_text(json.dumps(page, indent=1))

    def test_rows_their_grade_does_not_explain_stay_as_recorded(self):
        # As Athletic.net lists a few: an 11th grader on 9th-grade lists, and a row with no grade.
        def misfits(root):
            self.set_400m(root, 1203124, GradeID=11)
            self.set_400m(root, 1203119, GradeID=None)
        rec = variant(self.tmp, "rec", edit=misfits)
        out = Path(self.tmp) / "out"
        report = anonymize.anonymize(rec, out, SEED)
        for who, grade in ((1203124, 11), (1203119, None)):
            self.assertNotIn(who, report.dropped)
            new = report.id_map[who]
            for path in sorted(rec.glob("rankings/*-f-400m-*.json")):
                before = [r for r in load(path)["groupedRankings"][0] if r["AthleteID"] == who]
                after = [r for r in load(out / out_rel(path.relative_to(rec), report))["groupedRankings"][0]
                         if r["AthleteID"] == new]
                self.assertEqual(len(after), len(before), (who, path.name))
                self.assertTrue(all(r.get("GradeID") == grade for r in after), (who, path.name))

    def test_equal_marks_ranked_apart_stay_apart(self):
        # Athletic.net often ranks equal marks one after another (3 and 4 for two 56.09s).
        def apart(root):
            for path in root.glob("rankings/900500-f-400m-*.json"):
                page = json.loads(path.read_text())
                rows = page["groupedRankings"][0]
                for a, b in zip(rows, rows[1:]):
                    if (a["AthleteID"], b["AthleteID"]) == (1203102, 1203103):
                        b["rank"] = b["rowNum"]
                path.write_text(json.dumps(page, indent=1))
        rec = variant(self.tmp, "rec", edit=apart)
        out = Path(self.tmp) / "out"
        report = anonymize.anonymize(rec, out, SEED, keep=[1203102, 1203103])
        pair = (report.id_map[1203102], report.id_map[1203103])
        seen = 0
        for path in rec.glob("rankings/900500-f-400m-*.json"):
            rows = load(out / out_rel(path.relative_to(rec), report))["groupedRankings"][0]
            for a, b in zip(rows, rows[1:]):
                if (a["AthleteID"], b["AthleteID"]) == pair:
                    seen += 1
                    self.assertEqual(a["SortIntCalc"], b["SortIntCalc"])
                    self.assertEqual(b["rank"], a["rank"] + 1, path.name)
        self.assertGreater(seen, 0)

    def check_list(self, out, report, rels):
        """The anonymized pages of one list: rowNums one after another, each person once."""
        rows = {}
        for rel in rels:
            for r in load(out / out_rel(rel, report))["groupedRankings"][0]:
                self.assertEqual(rows.setdefault(r["rowNum"], r["AthleteID"]), r["AthleteID"], r["rowNum"])
        self.assertEqual(sorted(rows), list(range(1, len(rows) + 1)))
        self.assertEqual(len(set(rows.values())), len(rows), "each person once")
        return rows

    def test_pages_read_at_different_moments(self):
        rels = ["rankings/900502-m-800m-all-p1.json", "rankings/900502-m-800m-all-p2.json"]

        def tied_at_the_seam(root):
            # Rows 101 and 102 tied, and page 2 lists them the other way round.
            p1, p2 = (root / rels[0], root / rels[1])
            one, two = json.loads(p1.read_text()), json.loads(p2.read_text())
            rows = two["groupedRankings"][0]
            x, y = rows[1], rows[2]
            assert (x["rowNum"], y["rowNum"]) == (101, 102)
            for k in ("SortIntRaw", "SortIntCalc", "SortIntOrig", "display", "rank"):
                y[k] = x[k]
            x_id, y_id = x["AthleteID"], y["AthleteID"]
            x["AthleteID"], x["AthleteName"], y["AthleteID"], y["AthleteName"] = y_id, y["AthleteName"], x_id, x["AthleteName"]
            p2.write_text(json.dumps(two, indent=1))
            self.assertEqual(one["groupedRankings"][0][100]["AthleteID"], x_id)

        def changed_between_pages(root):
            # Someone new at row 100 of page 2, who was not on page 1.
            p2 = root / rels[1]
            two = json.loads(p2.read_text())
            row = two["groupedRankings"][0][0]
            assert row["rowNum"] == 100
            row["AthleteID"], row["AthleteName"] = 1207900, "Torin Quell"
            for k in ("SortIntRaw", "SortIntCalc", "SortIntOrig"):
                row[k] -= 50
            row["display"] = anonymize.format_running(row["SortIntCalc"], 2, "")
            p2.write_text(json.dumps(two, indent=1))

        for name, edit, people in (("tied", tied_at_the_seam, 150), ("changed", changed_between_pages, 151)):
            rec = variant(self.tmp, name, edit=edit)
            out = Path(self.tmp) / (name + "-out")
            report = anonymize.anonymize(rec, out, SEED)
            rows = self.check_list(out, report, rels)
            self.assertEqual(len(rows), people, name)
            kept = {report.id_map[r["AthleteID"]] for rel in rels for r in load(rec / rel)["groupedRankings"][0]
                    if r["AthleteID"] not in report.dropped}
            self.assertTrue(kept <= set(rows.values()), name)

    def test_a_read_cut_short_holds_the_people_it_needs(self):
        # ath reads a list recorded in part down to 3 past the subject's mark. When the lists
        # after would leave fewer behind it on the recorded pages, the people tied with the
        # subject and the 3 just behind, as recorded, are held there and the lists made again.
        a = anonymize.Anonymizer(anonymize.seed_key(SEED), anonymize.AREAS)
        files = a.read(RECORDING)
        a.collect(files)
        a.public_divisions(files)
        a.build()
        outputs = {}
        for f in files:
            edits, f["checks"] = a.rewrite(f)
            outputs[f["rel"]] = (a.rename(f), anonymize.splice(f["text"], edits))
        subject = a.new_id(1203001)
        sh = anonymize.Reshuffler(a, files, outputs, {subject})
        fam = next(f for f in sh.families.values() if f.capped and f.event == "400m")
        mark = next(r.sort for r in fam.rows if r.athlete == subject)
        self.assertEqual(sh.short_reads({subject}), set(), "as recorded: enough")
        whole = sh.rebuilt
        sh.rebuilt = lambda f: [x for x in whole(f) if f is not fam or x["sort"] <= mark]
        tied = {r.athlete for r in fam.rows if r.sort == mark and r.athlete != subject}
        behind = [r.athlete for r in fam.rows if r.sort > mark][:3]
        self.assertTrue(tied)
        self.assertEqual(sh.short_reads({subject}), tied | set(behind))

    def test_a_read_still_short_is_refused(self):
        # Hold as it may, a list that would still read short on the copy stops the run.
        subject = []
        whole = anonymize.Reshuffler.rebuilt

        def short(self, fam):
            rows = whole(self, fam)
            me = [x["sort"] for x in rows if x["athlete"] in subject]
            if fam.capped and fam.event == "400m" and me:
                return [x for x in rows if x["sort"] <= me[0]]
            return rows

        def protected(self, files, outputs):
            subject.append(self.new_id(1203001))
            return original(self, files, outputs)

        original = anonymize.Anonymizer.reshuffle
        with mock.patch.object(anonymize.Reshuffler, "rebuilt", short), \
                mock.patch.object(anonymize.Anonymizer, "reshuffle", protected):
            with self.assertRaises(anonymize.AnonymizeError) as e:
                anonymize.anonymize(RECORDING, Path(self.tmp) / "out", SEED)
        self.assertIn("fewer than 3 people behind a subject's mark", str(e.exception))
        self.assertFalse((Path(self.tmp) / "out").exists())

    def test_the_latest_season_is_checked(self):
        for latest, says in ((12026, "not an outdoor season"), (2031, "not a season of this Recording"),
                             (2025, "past 2099")):
            def has_2025(root):
                path = root / "navinfo/2026.json"
                tree = json.loads(path.read_text())
                tree["seasons"] = {"2025": 900499, "2026": 900500}
                path.write_text(json.dumps(tree, indent=1))
            rec = variant(self.tmp, "rec-%d" % latest, edit=has_2025)
            with self.assertRaises(anonymize.AnonymizeError) as e:
                anonymize.anonymize(rec, Path(self.tmp) / ("out-%d" % latest), SEED, latest=latest)
            self.assertIn(says, str(e.exception))

    def test_divisions_below_the_area_lose_their_names(self):
        # A school's league or conference (below its Area) would narrow it down past its Area:
        # blanked in its chain and in the tree; the nation, level, State and Area stay.
        def leagues(root):
            path = root / "teamnav/4101-2026.json"
            nav = json.loads(path.read_text())
            nav["divisions"].append({"id": 900513, "b": 908, "name": " Wren League"})
            path.write_text(json.dumps(nav, indent=1))
            path = root / "navinfo/2026.json"
            tree = json.loads(path.read_text())
            tree["regions"].append({"id": 900513, "name": " Wren League", "state": "CA", "divType": "League",
                                    "subDivType": None, "depth": 3, "customDivision": False})
            path.write_text(json.dumps(tree, indent=1))
        rec = variant(self.tmp, "rec", edit=leagues)
        out = Path(self.tmp) / "out"
        report = anonymize.anonymize(rec, out, SEED)
        chain = load(out / out_rel("teamnav/4101-2026.json", report))["divisions"]
        self.assertEqual([d["name"] for d in chain],
                         ["   United States", "High School", "California", "Central Coast", "", ""])
        regions = {r["id"]: r["name"] for r in load(out / "navinfo/2099.json")["regions"]}
        self.assertEqual(regions[report.id_map[900513]], "")
        self.assertEqual(regions[report.id_map[900502]], " Central Coast")
        self.assertEqual(regions[report.id_map[900301]], "", "Texas' league: Texas has no Areas")
        self.assertEqual(regions[report.id_map[900300]], "Texas")

    def test_a_planted_min_count_below_the_rows_is_refused(self):
        # A complete list's minCount below its rows (as recorded it covered them) stops the run.
        write = anonymize.Reshuffler.write

        def planted(self):
            out = write(self)
            for rel, (new_rel, text) in list(out.items()):
                if str(rel).endswith("900502-f-400m-9-p1.json"):
                    out[rel] = (new_rel, re.sub(r'"minCount": \d+', '"minCount": 1', text))
            return out

        with mock.patch.object(anonymize.Reshuffler, "write", planted):
            with self.assertRaises(anonymize.AnonymizeError) as e:
                anonymize.anonymize(RECORDING, Path(self.tmp) / "out", SEED)
        self.assertRegex(str(e.exception), r"minCount 1 below its \d+ rows")

    def test_a_meet_only_recording_keeps_state_and_area_names(self):
        # No tree in the Recording: the Area files say which division names are public.
        rec = Path(self.tmp) / "rec"
        shutil.copytree(RECORDING / "meets", rec / "meets")
        shutil.copy(RECORDING / "1203001.json", rec)  # a bio for its seasons; no tree
        entries = sorted((rec / "meets").glob("*/entries/*.json"))[0]
        page = json.loads(entries.read_text())
        page["teams"][0]["DivName"] = "Wren Valley League"
        entries.write_text(json.dumps(page, indent=1))
        report = anonymize.anonymize(rec, Path(self.tmp) / "out", SEED)
        out = Path(self.tmp) / "out" / out_rel(entries.relative_to(rec), report)
        teams = load(out)["teams"]
        self.assertEqual(teams[0]["DivName"], "", "a league: below the Area")
        self.assertEqual({(t["ParentDivName"], t["GrandparentDivName"]) for t in teams}, {("California", "High School")})
        kept = {t["DivName"] for f in (Path(self.tmp) / "out").glob("meets/*/entries/*.json") for t in load(f)["teams"]}
        self.assertEqual(kept, {"", "Central Coast"})

    def test_a_first_and_last_name_together_are_a_person_in_the_map(self):
        report = anonymize.anonymize(RECORDING, Path(self.tmp) / "out", SEED)
        self.assertIn("person", report.value_kinds["Linnea Penhallow"])

    def test_another_states_list_leaves_room_for_stand_ins(self):
        # A Recording of every State's list: Texas' first page spans California's marks, and no
        # Californian is on it, so stand-ins still find places on their own lists.
        def texas(root):
            template = json.loads((root / "rankings/900500-f-400m-all-p1.json").read_text())
            base = template["groupedRankings"][0][0]
            rows = []
            for n in range(1, 102):
                sort = 54000 + 100 * n
                rows.append(dict(base, rank=n, rowNum=n, AthleteID=1209000 + n, AthleteName="Tex Rider%d" % n,
                                 display=anonymize.format_running(sort, 2, ""), SortIntRaw=sort, SortIntCalc=sort,
                                 SortIntOrig=sort, GradeID=9 + n % 4, TeamID=4999, TeamName="Fenwold Academy",
                                 State="TX"))
            page = dict(template, groupedRankings=[rows], minCount=900)
            page["division"] = dict(template["division"], ID=900300, Name="Texas", BaseDivID=909)
            (root / "rankings/900300-f-400m-all-p1.json").write_text(json.dumps(page, indent=1))
        rec = variant(self.tmp, "rec", edit=texas)
        report = anonymize.anonymize(rec, Path(self.tmp) / "out", SEED)
        californians = {r["AthleteID"] for r in load(rec / "rankings/900500-f-400m-all-p1.json")["groupedRankings"][0]}
        self.assertTrue(set(report.dropped) & californians)


ATH = os.environ.get("ATH_TEST_BIN")

# The pinned clock (ATH_TODAY) the recording is replayed with; the copy's is moved with its dates.
TODAY = "2026-11-08"

# Every command the Recording answers, with the recording's ids and seasons; `{id}` is mapped and
# `{2026}` moved with the seasons. These print what they printed, but for the anonymized values.
SAME = [
    ["profile", "{1203001}", "--event", "400m"],
    ["areas", "CA", "--season", "{2026}"],
    ["meet", "{770001}"],
    ["seed", "--meet", "{770001}", "--gender", "f", "--div", "13", "--event", "800m"],
    ["seed", "--meet", "{770001}", "--gender", "m", "--div", "13", "--event", "4x800m"],
    ["seed", "--meet", "{770001}", "--gender", "f", "--div", "13", "--event", "4xmile"],
]
# These read reshuffled lists: the same shape, every list consistent in itself.
LISTS = [
    ["list", "--state", "CA", "--event", "400m", "--gender", "f", "--season", "{2026}", "--limit", "10"],
    ["list", "--area", "CCS", "--event", "400m", "--gender", "f", "--season", "{2026}", "--grades", "9-11"],
    ["list", "--area", "CCS", "--event", "Shot", "--gender", "f", "--season", "{2026}"],
    ["list", "--area", "CCS", "--event", "800m", "--gender", "m", "--season", "{2026}"],
]
RANK = ["rank", "{1203001}", "--event", "400m", "--season", "{2026}", "--in", "CA,CCS", "--ahead", "all"]

# Output keys whose values the anonymization changes (checked separately below).
MARK_KEYS = {"mark", "sort_int", "pr", "pr_numeric", "aggregate", "aggregate_seconds", "entered_sort_int",
             "estimated_from_pr", "estimated_from_sort_int", "season_best_mark"}
DATE_KEYS = {"date", "meet_date", "end_date", "as_of"}
SEASON_KEYS = {"season", "year", "season_id", "pr_season"}


@unittest.skipUnless(ATH, "set ATH_TEST_BIN to an ath binary (cargo test does)")
class Replayed(unittest.TestCase):
    """`ath` on the anonymized copy, with its clock moved as the dates were: what it says on the
    recording but for the anonymized values (Profiles, meets, seed sheets, Areas), and lists that
    are consistent in themselves (reshuffled: realistic, not real, ranks)."""

    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.mkdtemp()
        cls.out = Path(cls.tmp) / "out"
        cls.report = anonymize.anonymize(RECORDING, cls.out, SEED)

    @classmethod
    def tearDownClass(cls):
        shutil.rmtree(cls.tmp)

    def moved(self, day):
        d = datetime.date.fromisoformat(day)
        return (d.replace(year=d.year + self.report.year_shift)
                + datetime.timedelta(days=self.report.date_shift)).isoformat()

    def args(self, command, anonymized):
        def one(m):
            value = int(m.group(1))
            if not anonymized:
                return str(value)
            return str(value + self.report.year_shift) if value < 10000 and value > 1900 else str(self.report.id_map[value])
        return [re.sub(r"\{(\d+)\}", one, a) for a in command]

    def ath(self, command, anonymized, name):
        args = self.args(command, anonymized)
        csv = Path(self.tmp) / (name + ".csv")
        extra = ["--out", str(csv)] if args[0] == "seed" else []
        env = {k: v for k, v in os.environ.items() if k != "ATH_COOKIE"}
        env["ATH_TODAY"] = self.moved(TODAY) if anonymized else TODAY
        done = subprocess.run([ATH] + args + extra + ["--json", "--fixtures-dir", str(self.out if anonymized else RECORDING)],
                              capture_output=True, text=True, env=env)
        self.assertEqual(done.returncode, 0, "%s: %s" % (args, done.stderr))
        return json.loads(done.stdout)

    def test_ath_says_the_same_on_the_anonymized_copy(self):
        for i, command in enumerate(SAME):
            before = self.ath(command, False, "real-%d" % i)
            after = self.ath(command, True, "anon-%d" % i)
            self.compare(before, after, command, ())

    def test_reshuffled_lists_are_consistent(self):
        for i, command in enumerate(LISTS):
            before = self.ath(command, False, "list-real-%d" % i)
            after = self.ath(command, True, "list-anon-%d" % i)
            self.assertEqual((after["count"], after["total"], after["complete"]),
                             (before["count"], before["total"], before["complete"]), command)
            rows = after["rows"]
            for a, b in zip(rows, rows[1:]):
                self.assertLessEqual(a["sort_int"], b["sort_int"], command)
                self.assertEqual(a["sort_int"] == b["sort_int"], a["rank"] == b["rank"], command)
                self.assertTrue(b["tied"] or b["rank"] == b["place"], command)
            dropped = {self.report.id_map[a] for a in self.report.dropped}
            self.assertFalse({r["athlete_id"] for r in rows} & dropped, command)

    def test_grade_lists_agree_with_the_whole_list(self):
        whole = ["list", "--area", "CCS", "--event", "400m", "--gender", "f", "--season", "{2026}", "--grades", "9-12"]
        everyone = self.ath(whole, True, "grades-whole")["rows"]
        for grades in ("9-10", "11", "9-11"):
            lo, hi = (int(g) for g in (grades.split("-") * 2)[:2])
            got = self.ath(whole[:-1] + [grades], True, "grades-%s" % grades)["rows"]
            want = [r["athlete_id"] for r in everyone if lo <= r["grade"] <= hi]
            self.assertEqual([r["athlete_id"] for r in got], want, grades)
            self.assertEqual([r["place"] for r in got], list(range(1, len(got) + 1)), grades)

    def test_the_subject_ranks_plausibly(self):
        before = self.ath(RANK, False, "rank-real")
        after = self.ath(RANK, True, "rank-anon")
        self.assertEqual(after["athlete"]["id"], self.report.id_map[1203001])
        self.assertEqual(len(after["results"]), len(before["results"]))
        drops = len(self.report.dropped)
        for a, b in zip(before["results"], after["results"]):
            self.assertEqual((a["status"], a["group"]), (b["status"], b["group"]))
            if a.get("place"):
                self.assertLessEqual(abs(a["place"]["rank"] - b["place"]["rank"]), drops)
                tied = b["place"]["neighborhood"]["tied"]
                self.assertIn(after["athlete"]["id"], [t["athlete_id"] for t in tied] + [after["athlete"]["id"]])

    def expected_string(self, value):
        mapped = self.report.value_map.get(value)
        if mapped is not None:
            return mapped
        # A name ath puts together from a first and a last name.
        first, _, last = value.partition(" ")
        if first in self.report.value_map and last in self.report.value_map:
            return self.report.value_map[first] + " " + self.report.value_map[last]
        def number(m):
            n = int(m.group(0))
            if n in self.report.id_map:
                return str(self.report.id_map[n])
            return str(n + self.report.year_shift) if 1900 <= n <= 2200 else m.group(0)
        return re.sub(r"(?<!\d)\d{4,}(?!\d)", number, value)

    def compare(self, a, b, command, path):
        where = "%s at %s" % (" ".join(command), "/".join(map(str, path)))
        self.assertEqual(type(a), type(b), where)
        if isinstance(a, dict):
            self.assertEqual(list(a), list(b), where)
            for k in a:
                self.compare(a[k], b[k], command, path + (k,))
        elif isinstance(a, list):
            self.assertEqual(len(a), len(b), where)
            for i, (x, y) in enumerate(zip(a, b)):
                self.compare(x, y, command, path + (i,))
        else:
            key = next((p for p in reversed(path) if isinstance(p, str)), None)
            if a is None or isinstance(a, bool):
                self.assertEqual(a, b, where)
            elif key in MARK_KEYS:
                self.assertNotEqual(a, b, where)
            elif key in DATE_KEYS and isinstance(a, str):
                self.assertEqual(b[:10], self.moved(a[:10]), where)
            elif key in SEASON_KEYS and isinstance(a, int):
                self.assertEqual(b, a + self.report.year_shift, where)
            elif isinstance(a, int) and (key == "id" or key.endswith("_id")):
                self.assertEqual(b, self.report.id_map.get(a, a), where)
            elif isinstance(a, str):
                self.assertEqual(b, self.expected_string(a), where)
            else:
                self.assertEqual(a, b, where)


if __name__ == "__main__":
    unittest.main()
