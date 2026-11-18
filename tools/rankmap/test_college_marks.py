"""Hand-entered college marks: reading a pasted season list (college_marks.py), the private files,
and their merge into the Colleges section (build.prepare_colleges). Every name is made up, and every
file goes in a temporary folder, never your local/. Run: python3 -m unittest discover -s tools/rankmap
"""
import copy
import io
import json
import os
import re
import shutil
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from datetime import date
from pathlib import Path
from unittest import mock

import build
import college_marks as cm
import rankmap
from test_rankmap import AREAS, COLLEGES, SEASON, TEAMS, college_files, saved, season_rank

EVENT = "400 Meters"

# A season list as copying it from a results page gives it: the event heading and its "Top" link,
# the column names one per line, then five lines per athlete.
ONE_PER_LINE = """400 Meters
Top
Athlete
Year
Time
Meet
Meet Date
Larkspur, Odile
JR-3
53.76
Fernhollow Invitational
Apr 17, 2099
Quenby, Marisol
SO-2
1:00.04
Lakemarsh Open
May 2, 2099
"""


def parse(text, event=EVENT, gender="f", season=2099):
    return cm.parse_paste(text, event, gender, season)


def brief(rows):
    return [(r["name"], r["year"], r["mark"], r["sort_int"], r["meet"], r["date"]) for r in rows]


class Parse(unittest.TestCase):
    def test_one_field_per_line_with_its_heading_and_column_names(self):
        self.assertEqual(brief(parse(ONE_PER_LINE)), [
            ("Odile Larkspur", "JR-3", "53.76", 53760, "Fernhollow Invitational", "2099-04-17"),
            ("Marisol Quenby", "SO-2", "1:00.04", 60040, "Lakemarsh Open", "2099-05-02")])

    def test_one_field_per_line_without_column_names_or_optional_fields(self):
        text = "Larkspur, Odile\n53.76\nQuenby, Marisol\nSo.\n55.00\nLakemarsh Open\nTamsin Oakhurst\n2:01.30\nApr 3\n"
        self.assertEqual(brief(parse(text)), [
            ("Odile Larkspur", None, "53.76", 53760, None, None),
            ("Marisol Quenby", "So.", "55.00", 55000, "Lakemarsh Open", None),
            ("Tamsin Oakhurst", None, "2:01.30", 121300, None, "2099-04-03")])

    def test_tab_separated_rows_from_a_copied_table(self):
        text = ("Pl\tName\tYr\tTime\tMeet\tDate\n"
                "1\tOdile Larkspur\tJr.\t53.76a\tFernhollow Invitational\t4/17/2099\n"
                "2\tQuenby, Marisol\tSO-2\t1:00.04\tLakemarsh Open\t2099-05-02\n")
        self.assertEqual(brief(parse(text)), [
            ("Odile Larkspur", "Jr.", "53.76", 53760, "Fernhollow Invitational", "2099-04-17"),
            ("Marisol Quenby", "SO-2", "1:00.04", 60040, "Lakemarsh Open", "2099-05-02")])
        # Without the column names, by what each cell looks like.
        self.assertEqual(brief(parse("\n".join(text.splitlines()[1:]))), brief(parse(text)))

    def test_typed_rows(self):
        text = ("Odile Larkspur, 53.76, Fernhollow Invitational, Apr 17, 2099\n"
                "Quenby, Marisol, SO-2, 1:00.04\n"
                "Tamsin Oakhurst, 56.20, Lakemarsh Open\n")
        self.assertEqual(brief(parse(text)), [
            ("Odile Larkspur", None, "53.76", 53760, "Fernhollow Invitational", "2099-04-17"),
            ("Marisol Quenby", "SO-2", "1:00.04", 60040, None, None),
            ("Tamsin Oakhurst", None, "56.20", 56200, "Lakemarsh Open", None)])

    def test_a_first_athlete_is_never_taken_for_a_heading(self):
        for first in ("Jo Hammer", "Hammer, Jo", "Rowan Walker", "Mile, Ada"):
            self.assertEqual(parse(f"400 Meters\n{first}\n55.00\n")[0]["mark"], "55.00", first)
        self.assertEqual(len(parse("1,500 Meters\nTop\nJo Hammer\n4:30.10\n", event="1500 Meters")), 1)

    def test_an_empty_list_is_its_heading_and_column_names_alone(self):
        self.assertEqual(parse("400 Meters\nTop\nAthlete\nYear\nTime\nMeet\nMeet Date\n"), [])

    def test_a_paste_it_cannot_read_is_refused(self):
        cases = [
            ("", "nothing was pasted"),
            ("   \n\n", "nothing was pasted"),
            ("some words\nmore words\n", "'more words' is not a time"),
            ("Larkspur, Odile\n53.7\n", "'53.7' is not a time"),
            ("Larkspur, Odile\n53.76\nFernhollow Invitational\nApr 17, 2099\nJR-3\n",
             "is not where an athlete's row starts"),
            ("Larkspur, Odile\n5.60m\n", "'5.60m' is a field event's mark"),
            ("Larkspur, Odile\n18-04.25\n", "'18-04.25' is a field event's mark"),
            ("Larkspur, Odile\n53.7h\n", "'53.7h' is a hand time"),
            ("800 Meters\nLarkspur, Odile\n2:10.00\n", "headed '800 Meters', not the 400 Meters"),
            ("Men's 400 Meters\nLarkspur, Odile\n53.76\n", "not the women's list"),
            ("Larkspur, Odile\n53.76\nFernhollow Invitational\nApr 17, 2098\n", "is not in the 2099 season"),
            ("Name\tTime\nOdile Larkspur\tfast\n", "'fast' is not a time"),
        ]
        for text, says in cases:
            with self.assertRaises(cm.PasteError, msg=text) as e:
                parse(text)
            self.assertIn(says, str(e.exception), text)

    def test_a_field_event_is_refused(self):
        with self.assertRaisesRegex(cm.PasteError, "Long Jump is a field event"):
            parse("Larkspur, Odile\n53.76\n", event="Long Jump")

    def test_times_and_names_as_compared(self):
        self.assertEqual(build.time_hundredths("1:00.04"), 6004)
        self.assertEqual(build.time_hundredths("12:03.40"), 72340)
        for bad in ("1:60.00", "53.7", "53.765", "5.60m", "fast", ""):
            self.assertIsNone(build.time_hundredths(bad), bad)
        self.assertEqual(build.name_key("Larkspur, Odile"), build.name_key("odile  LARKSPUR"))
        self.assertEqual(build.name_key("O'Rourke-Vale, Ines"), build.name_key("Ines ORourke Vale"))
        self.assertEqual(build.name_key("Zoë Ambler"), build.name_key("Zoe Ambler"))
        self.assertNotEqual(build.name_key("Odile Larkspur"), build.name_key("Odile Larkspurr"))


# ---------------------------------------------------------------- the files and the command

class Files(unittest.TestCase):
    """college_marks.py add and list, on a config in a temporary local/ ($ATH_LOCAL)."""

    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp()).resolve()
        self.addCleanup(shutil.rmtree, self.tmp, ignore_errors=True)
        env = mock.patch.dict(os.environ, {"ATH_LOCAL": str(self.tmp / "local")})
        env.start()
        self.addCleanup(env.stop)
        folder = self.tmp / "local" / "rankmap"
        folder.mkdir(parents=True)
        cfg = {"athlete_id": 982389051, "event": "400m", "graduation_year": 2102, "output_dir": "out",
               "colleges": TEAMS}
        (folder / "c.config.json").write_text(json.dumps(cfg))
        data = folder / "data" / "982389051-400m"
        data.mkdir(parents=True)
        (data / f"{SEASON}.json").write_text(json.dumps(saved()))
        self.marks = folder / "college-marks"

    def run_cm(self, *argv, paste=ONE_PER_LINE):
        with redirect_stdout(io.StringIO()) as out, mock.patch("sys.stdin", io.StringIO(paste)), \
                mock.patch.object(rankmap, "STATE_LISTS", 1):
            got = cm.main([argv[0], "--config", "c", *argv[1:]], today=date(2099, 10, 30))
        return got, out.getvalue()

    def add(self, *more, paste=ONE_PER_LINE):
        return self.run_cm("add", "--team", "Harrowgate", "--season", "2099", *more, paste=paste)

    def test_a_preview_writes_nothing(self):
        got, out = self.add("--source", "the team's season list")
        self.assertIsNone(got)
        self.assertIn("Harrowgate 2099 women's 400 Meters: 2 rows", out)
        self.assertIn("Odile Larkspur", out)
        self.assertIn("1:00.04", out)
        self.assertIn("Preview only: nothing written", out)
        self.assertFalse(self.marks.exists())

    def test_yes_writes_the_list_with_how_it_was_gathered(self):
        path, out = self.add("--source", "the team's season list", "--url", "https://example.org/list", "--yes")
        self.assertEqual(path, self.marks / "960000001" / "2099-f-400-meters.json")
        h = build.check_hand_marks(json.loads(path.read_text()))
        self.assertEqual({k: h[k] for k in ("format", "team", "college", "season", "gender", "event", "source",
                                            "source_url", "entered", "how")},
                         {"format": 1, "team": 960000001, "college": "Harrowgate", "season": 2099, "gender": "f",
                          "event": "400 Meters", "source": "the team's season list",
                          "source_url": "https://example.org/list", "entered": "2099-10-30",
                          "how": "copied by hand from a season list"})
        self.assertEqual(h["pasted"], ONE_PER_LINE)
        self.assertEqual(brief(h["marks"]), brief(parse(ONE_PER_LINE)))
        self.assertIn("Wrote", out)

    def test_entering_it_again_replaces_it_and_keeps_the_source(self):
        self.add("--source", "the team's season list", "--url", "https://example.org/list", "--yes")
        path, out = self.add("--yes", paste="Larkspur, Odile\n52.90\n")
        self.assertIn("Replaced", out)
        h = json.loads(path.read_text())
        self.assertEqual([r["mark"] for r in h["marks"]], ["52.90"])
        self.assertEqual((h["source"], h["source_url"]), ("the team's season list", "https://example.org/list"))
        self.assertEqual(len(list(self.marks.glob("*/*.json"))), 1)

    def test_the_source_is_needed_the_first_time(self):
        with self.assertRaisesRegex(SystemExit, "Say where this list was copied from"):
            self.add("--yes")
        self.assertFalse(self.marks.exists())

    def test_gender_event_and_team_by_id_can_be_given(self):
        path, _ = self.run_cm("add", "--team", "960000002", "--season", "2098", "--gender", "m", "--event",
                              "800 Meters", "--source", "x", "--yes", paste="Brannock, Ivo\n1:52.10\n")
        self.assertEqual(path, self.marks / "960000002" / "2098-m-800-meters.json")

    def test_a_college_not_in_the_config_or_an_unreadable_paste_is_refused(self):
        with self.assertRaisesRegex(SystemExit, "not one of this config's colleges"):
            self.run_cm("add", "--team", "Elsewhere", "--season", "2099", "--source", "x", "--yes")
        with self.assertRaisesRegex(SystemExit, "Could not read the paste: nothing was pasted"):
            self.add("--source", "x", "--yes", paste="")
        self.assertFalse(self.marks.exists())

    def test_list_shows_what_is_entered_per_college_and_season(self):
        self.add("--source", "the team's season list", "--yes")
        self.run_cm("add", "--team", "960000002", "--season", "2098", "--gender", "m", "--source", "a note",
                    "--yes", paste="Brannock, Ivo\n49.10\n")
        _, out = self.run_cm("list")
        self.assertIn("for women's 400 Meters:", out)
        self.assertRegex(out, r"Harrowgate +2099 +2 +2099-10-30 +the team's season list")
        # The seasons the page shows (her saved 2099), and any with a list.
        self.assertNotRegex(out, r"Harrowgate +2098")
        self.assertRegex(out, r"Wrenmoor State +2099 +- +none entered")
        self.assertIn("Other lists (another college, gender or event):", out)
        self.assertIn("team 960000002 (Wrenmoor State) 2098 m 400 Meters: 1 rows", out)


# ---------------------------------------------------------------- the merge and the page

def hand(rows, team=960000001, season=2099, gender="f", event=EVENT):
    """A hand-entered list of (name, mark[, meet]) rows."""
    marks = [{"name": r[0], "year": None, "mark": r[1], "sort_int": build.time_hundredths(r[1]) * 10,
              "meet": r[2] if len(r) > 2 else None, "date": None} for r in rows]
    return build.check_hand_marks(cm.hand_file(team, "x", season, gender, event, "private note", None,
                                               "2099-10-30", "pasted", marks))


def page_with(*lists, files=None, both=False):
    """The page, CSV and Markdown built with hand-entered lists, and the prepared Colleges section:
    of 2099, or with both 2099 and 2098 (her 2098 best the same 58.98)."""
    merged = {}
    for h in lists:
        merged.setdefault((h["team"], h["season"]), []).append(h)
    states = {SEASON: saved()}
    if both:
        states[2098] = saved(season_rank(2098, "58.98", 58980))
    with tempfile.TemporaryDirectory() as tmp:
        html, page = build.build(states, sorted(states), Path(tmp), areas_dir=AREAS, colleges=TEAMS,
                                 college_seasons=college_files() if files is None else files, college_marks=merged)
        return (html.read_text(), html.with_suffix(".csv").read_text(), html.with_suffix(".md").read_text(),
                page.colleges)


def college(colleges, team=960000001, season="2099"):
    """A college's row in a season's Colleges."""
    return next(r for r in colleges["seasons"][season]["rows"] if r["team"] == team)


def cell(colleges, team=960000001, season="2099"):
    return college(colleges, team, season)["cell"]


def names_cell(page, season="2099"):
    section = page[page.index(f'<div class="cseason" data-season="{season}"'):]
    return re.findall(r'<td class="sn">(.*?)</td>', section[:section.index("</table>")])


class Merge(unittest.TestCase):
    # Harrowgate 2099 on Athletic.net: 53.41, 55.02, 56.10 ahead of her 58.98, 58.98 tied (Della
    # Fenmarsh), 59.80 behind (Liv Ashcombe): T4 of 6.

    def test_without_hand_entered_lists_nothing_changes(self):
        plain = page_with()
        self.assertEqual(plain[:3], page_with(hand([("Odile Larkspur", "57.00")], gender="m"))[:3])
        self.assertNotIn('class="hm"', plain[0])
        self.assertNotIn(".hm {", plain[0])

    def test_the_same_athlete_and_mark_from_both_counts_once(self):
        # As "Last, First" or "First Last", in any case.
        page, table, md, c = page_with(hand([("Fenmarsh, Della", "58.98"), ("LIV ASHCOMBE", "59.80"),
                                             ("Quillfeather, Maren", "53.41", "Another Meet")]))
        x = cell(c)
        self.assertEqual((x["label"], x["marks"], len(x["ahead"]), len(x["ties"])), ("T4", 5, 3, 1))
        self.assertEqual(x["ahead"][0]["url"], "https://www.athletic.net/athlete/960100001/track-and-field")
        self.assertFalse(college(c)["hand"], "every hand-entered mark was already counted")
        self.assertEqual((page, table, md), page_with()[:3])

    def test_a_new_faster_mark_moves_the_rank_and_marks_the_college(self):
        page, table, md, c = page_with(hand([("Larkspur, Odile", "57.40", "Fernhollow Invitational")]))
        x = cell(c)
        self.assertEqual((x["label"], x["marks"]), ("T5", 6))
        self.assertEqual([a["name"] for a in x["ahead"]],
                         ["Maren Quillfeather", "Tessa Brackwood", "Ines Corrowmere", "Odile Larkspur"])
        self.assertIsNone(x["ahead"][3]["url"])
        self.assertTrue(college(c)["hand"])
        # The page: a bare asterisk after the name, outside its link, and nothing else.
        self.assertEqual(names_cell(page)[0], '<a href="https://www.athletic.net/team/960000001/track-and-field-outdoor/2099/event-records"'
                         ' target="_blank" rel="noopener">Harrowgate</a><span class="hm">*</span>')
        self.assertEqual(sum('class="hm"' in n for n in names_cell(page)), 1)
        self.assertIn(".hm {", page)
        self.assertNotIn("private note", page + table + md)
        self.assertNotIn("hand", page.lower().replace("handle", ""))
        self.assertIn('<span class="pill b5"><span class="sup">t</span>5</span><span class="of">of 7</span>', page)
        self.assertIn('<summary>4 faster, 1 tied <span class="tm">&middot; fastest 53.41</span></summary>', page)
        # A hand-entered mark, unlinked, with its meet.
        self.assertIn('<li><span class="mk">57.40</span> <span class="nt"><span class="nm">Odile Larkspur</span> '
                      '<span class="tm">Fernhollow Invitational</span></span></li>', page)
        # The Markdown: the same asterisk; the CSV: the same columns, its ranks merged.
        self.assertIn("| Harrowgate\\* | T5 of 7, led by Maren Quillfeather 53.41 |", md)
        self.assertIn("| Wrenmoor State | #8 of 10", md)
        self.assertEqual(table.splitlines()[0], page_with()[1].splitlines()[0])
        self.assertIn("2099,58.98,college season bests (FAT),College,T5 of 7,Harrowgate,960000001,4,1,", table)
        self.assertIn("Odile Larkspur 57.40 (Fernhollow Invitational)", table)

    def test_a_faster_mark_of_someone_already_counted_replaces_theirs(self):
        _, _, _, c = page_with(hand([("Ashcombe, Liv", "58.10", "Fernhollow Invitational")]))
        x = cell(c)
        self.assertEqual((x["label"], x["marks"]), ("T5", 5))
        liv = x["ahead"][3]
        # Her mark and meet are the hand-entered ones; her name and profile link stay Athletic.net's.
        self.assertEqual((liv["name"], liv["mark"], liv["meet"]), ("Liv Ashcombe", "58.10", "Fernhollow Invitational"))
        self.assertEqual(liv["url"], COLLEGES[2099]["results"][0]["behind"][0]["url"])
        self.assertTrue(college(c)["hand"])

    def test_a_slower_new_mark_counts_in_of_n_only(self):
        page, _, _, c = page_with(hand([("Odile Larkspur", "1:02.00")]))
        self.assertEqual((cell(c)["label"], cell(c)["marks"]), ("T4", 6))
        self.assertIn('<span class="of">of 7</span>', page)
        self.assertTrue(college(c)["hand"], "it changes the count")

    def test_an_equal_mark_to_the_hundredth_is_a_tie(self):
        _, _, _, c = page_with(hand([("Odile Larkspur", "58.98")]))
        self.assertEqual((cell(c)["label"], len(cell(c)["ties"])), ("T4", 2))

    def test_a_team_without_marks_on_athletic_net_takes_them(self):
        # Wrenmoor State has no 2098 marks on Athletic.net.
        _, _, md, c = page_with(hand([("Odile Larkspur", "57.00"), ("Quenby, Marisol", "1:00.04")],
                                     team=960000002, season=2098), both=True)
        self.assertEqual((cell(c, 960000002, "2098")["label"], cell(c, 960000002, "2098")["marks"]), ("2", 2))
        self.assertIn("| Wrenmoor State\\* | #2 of 3, led by Odile Larkspur 57.00 |", md)
        self.assertIn("| Wrenmoor State | #8 of 10, led by Sable Ondrejka 52.80 |", md)

    def test_another_gender_event_or_season_never_counts(self):
        for h in (hand([("Odile Larkspur", "50.00")], gender="m"),
                  hand([("Odile Larkspur", "50.00")], event="800 Meters"),
                  hand([("Odile Larkspur", "50.00")], season=2097)):
            _, _, _, c = page_with(h)
            self.assertEqual(cell(c)["label"], "T4")
            self.assertFalse(any(r["hand"] for r in c["seasons"]["2099"]["rows"]))
        # The event by its code is the same event.
        _, _, _, c = page_with(hand([("Odile Larkspur", "50.00")], event="400m"))
        self.assertEqual(cell(c)["label"], "T5")

    def test_the_athletes_own_row_is_left_out(self):
        _, _, _, c = page_with(hand([("Uddumcombe, Jamie", "58.98")]))
        self.assertEqual(cell(c)["marks"], 5)
        self.assertFalse(college(c)["hand"])

    def test_the_asterisk_is_the_season_picked(self):
        # A list for 2099 marks Harrowgate in 2099 only; one that changes nothing (2098's, a mark
        # already counted) marks nothing.
        page, _, md, c = page_with(hand([("Larkspur, Odile", "57.40")]),
                                   hand([("Holloway-Prynne, Bea", "59.10")], season=2098), both=True)
        self.assertEqual((college(c)["hand"], college(c, season="2098")["hand"]), (True, False))
        self.assertTrue(names_cell(page, "2099")[0].endswith('Harrowgate</a><span class="hm">*</span>'))
        self.assertTrue(names_cell(page, "2098")[0].endswith("Harrowgate</a>"))
        self.assertEqual(md.count("\\*"), 1)
        self.assertLess(md.index("| Harrowgate\\* |"), md.index("## 2098 season"))

    def test_a_hand_entered_mark_ranks_each_seasons_best(self):
        # The same 2098 list against a slower 2098 best: the hand-entered 59.50 is now ahead of her.
        merged = {(960000001, 2098): [hand([("Odile Larkspur", "59.50")], season=2098)]}
        with tempfile.TemporaryDirectory() as tmp:
            _, page = build.build({2098: saved(season_rank(2098, "1:00.10", 60100)), SEASON: saved()}, [2098, SEASON],
                                  Path(tmp), areas_dir=AREAS, colleges=TEAMS, college_seasons=college_files(),
                                  college_marks=merged)
        x = cell(page.colleges, season="2098")
        self.assertEqual((x["label"], x["marks"], [a["mark"] for a in x["ahead"]]), ("4", 3, ["54.20", "59.10", "59.50"]))

    def test_a_bad_file_is_refused_saying_why(self):
        good = hand([("Odile Larkspur", "57.00")])
        cases = [({**good, "format": 2}, "not a hand-entered list"),
                 ({**good, "gender": "x"}, "not a whole hand-entered list"),
                 ({**good, "marks": [{"name": "Odile Larkspur", "mark": "5.60m"}]}, "row 1 is not a name and a time"),
                 ({k: v for k, v in good.items() if k != "event"}, "KeyError")]
        for value, says in cases:
            with self.assertRaisesRegex(build.SeasonError, says):
                build.check_hand_marks(value)


class Rebuild(unittest.TestCase):
    """rankmap --no-fetch picks up newly entered lists from beside its config, collecting nothing."""

    def test_a_no_fetch_rebuild_counts_a_new_list(self):
        with tempfile.TemporaryDirectory() as d:
            folder = Path(d)
            (folder / "c.config.json").write_text(json.dumps({
                "athlete_id": 982389051, "event": "400m", "graduation_year": 2102, "output_dir": "out",
                "colleges": TEAMS}))
            data = folder / "data" / "982389051-400m"
            (data / "colleges").mkdir(parents=True)
            (data / f"{SEASON}.json").write_text(json.dumps(saved()))
            for s, f in college_files().items():
                (data / "colleges" / f"{s}.json").write_text(json.dumps(f))
            path = rankmap.college_marks_path(folder / "college-marks", 960000001, 2099, "f", "400 Meters")
            path.parent.mkdir(parents=True)
            path.write_text(json.dumps(hand([("Odile Larkspur", "57.40")])))
            # A broken file is left out, saying so; the rest build.
            (path.parent / "2098-f-400-meters.json").write_text("{")

            class Today(date):
                @classmethod
                def today(cls):
                    return cls(2099, 10, 30)

            with redirect_stdout(io.StringIO()) as out, redirect_stderr(io.StringIO()), \
                    mock.patch.object(rankmap, "date", Today), \
                    mock.patch.object(rankmap, "STATE_LISTS", 1), \
                    mock.patch.object(rankmap, "find_ath", side_effect=AssertionError("nothing is collected")):
                rankmap.main(["--config", str(folder / "c.config.json"), "--no-fetch"])
            self.assertIn("colleges 2099 Harrowgate          58.98 T5 of 7  (counts hand-entered marks)",
                          out.getvalue())
            self.assertIn("2098-f-400-meters.json is not valid JSON", out.getvalue())
            page = (folder / "out" / "982389051-400m.html").read_text()
            self.assertIn('Harrowgate</a><span class="hm">*</span>', page)


if __name__ == "__main__":
    unittest.main()
