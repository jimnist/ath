"""The real-value guard (real_values.py, docs/anonymize.md): no real id or name of the Recordings
in a tracked file, and the guard itself finds one where it is planted (made-up values)."""

import io
import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path
from unittest import mock

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import real_values  # noqa: E402

ROOT = HERE.parents[1]
# A made-up guard key for the made-up values below (the real one is secret).
MADE_UP_KEY = "11" * 32


def git_checkout():
    try:
        done = subprocess.run(["git", "rev-parse", "--is-inside-work-tree"], cwd=str(ROOT), capture_output=True)
    except OSError:
        return False
    return done.returncode == 0


def real_key():
    try:
        return real_values.guard_key()
    except real_values.NoKey:
        return None


class TrackedFiles(unittest.TestCase):
    @unittest.skipUnless(git_checkout(), "needs the git work tree (git ls-files)")
    @unittest.skipUnless(real_key(), "no guard key: set ATH_GUARD_KEY (a GitHub Actions secret in CI) or keep "
                                     "local/anonymize.key; see docs/anonymize.md, The real-value guard")
    def test_no_real_id_or_name_in_a_tracked_file(self):
        guard = real_values.Guard.load(key=real_key())
        # A key that is not the list's fails here, never skips: the scan would find nothing.
        self.assertTrue(guard.verifies(), "the guard key is not the list's (a wrong ATH_GUARD_KEY or anonymize.key); "
                                          "derive it with: " + real_values.DERIVE)
        hits = real_values.scan(ROOT, guard)
        self.assertEqual(hits, [], "real values in tracked files (path, line, column, kind); see docs/anonymize.md")

    def test_the_list_holds_no_value(self):
        text = real_values.LIST.read_text()
        for line in text.splitlines():
            if line.startswith("#") or not line.strip():
                continue
            self.assertRegex(line, r"^((check|id|first|name) [0-9a-f]{16}|allow (id|name) [0-9a-f]{16} \S.*  # .+)$")
        guard = real_values.Guard.load(key=b"x")
        self.assertTrue(guard.ids and guard.names and guard.firsts)


class Scan(unittest.TestCase):
    """Made-up values planted in a made-up file, under a made-up key."""

    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.tmp)
        env = mock.patch.dict(os.environ, {real_values.KEY_ENV: MADE_UP_KEY})
        env.start()
        self.addCleanup(env.stop)
        maps = self.tmp / "made-up.map.json"
        maps.write_text(json.dumps({
            "ids": {"1203001": 900000001, "41012": 900000002, "660101": 900000003},
            "value_kinds": {"Linnea Penhallow": ["person"], "Wrenfield Valley Academy": ["school"],
                            "Tidewater Prep": ["school"], "Penhallow": ["last"], "Port Ellery": ["city"],
                            "Gullwing Invitational 2026": ["meet"], "Zoë O'Hallorann": ["person"]},
        }))
        extra = self.tmp / "extra.json"
        extra.write_text(json.dumps({"ids": [7700123], "people": ["Ione Quarrington"], "names": ["Quarry Hill Relays"]}))
        self.list = self.tmp / "real-values.txt"
        self.refresh(maps, "--extra", str(extra), "--allow", "id:660101:docs/meets.md:a meet's public id, as an example")
        self.guard = real_values.Guard.load(self.list)

    def refresh(self, *args):
        with redirect_stdout(io.StringIO()):
            return real_values.main(["refresh"] + [str(a) for a in args] + [
                "--list", str(self.list), "--trees", str(self.tmp / "no-trees" / "*.json")])

    def hits(self, text, path="notes.md"):
        return [(line, kind) for line, _, kind in self.guard.scan_text(text, path)]

    def test_ids_and_whole_names_are_found_by_their_words(self):
        text = "\n".join([
            "Linnea Penhallow ran 56.85",              # a name
            "/athlete/1203001/linnea-penhallow",        # an id, and the name as a URL writes it
            "const ATHLETE: i64 = 1_203_001;",          # an id with underscores
            "Wrenfield  Valley Academy (WRNF)",         # a school, words apart
            "relay team 7700123, IONE QUARRINGTON",     # extra values, any case
            "gullwing invitational 2026 and the Quarry Hill Relays",  # meets, one with its year
            '{"MeetID": 660101, "Note": "meet 660101"}',  # in JSON text too, under any key
        ])
        self.assertEqual(self.hits(text), [
            (1, "a real name"), (2, "a real id"), (2, "a real name"), (3, "a real id"), (4, "a real name"),
            (5, "a real id"), (5, "a real name"), (6, "a real name"), (6, "a real name"), (7, "a real id"),
            (7, "a real id")])

    def test_last_first_and_folded_names(self):
        for text in ("Penhallow, Linnea",               # last name first
                     "Zoe OHallorann",                  # no accent, no apostrophe
                     "zoë-o’hallorann",                 # a slug with another apostrophe
                     "O'Hallorann, Zoë"):               # both
            self.assertEqual(self.hits(text), [(1, "a real name")], text)

    def test_a_first_and_last_name_together_in_json(self):
        page = json.dumps({"entries": [{"IDAthlete": 5, "FirstName": "Linnea", "LastName": "Penhallow"},
                                       {"IDAthlete": 6, "FirstName": "Linnea", "LastName": "Other"}]})
        self.assertEqual(self.hits(page, "entries.json"), [(1, "a real name")])
        # Not in a file that is no JSON, where the two are only words apart.
        self.assertEqual(self.hits('"FirstName": "Linnea", "LastName": "Penhallow"', "notes.md"), [])

    def test_what_is_not_guarded(self):
        text = "\n".join([
            "Penhallow alone, Linnea alone",            # a last or first name alone names nobody
            "Port Ellery",                              # a city
            "Tidewater Prep",                           # a two-word school: as often plain words
            "school 41012",                             # an id under six digits
            "12030011 and 21203001",                    # other numbers holding the digits
            "#1203001f and id1203001",                  # inside a word (a color, a hash)
            "Gullwing Invitational",                    # a meet without its year is another name
        ])
        self.assertEqual(self.hits(text), [])
        # In a JSON file, a number under a key that holds no id (a mark, a count) is no id.
        self.assertEqual(self.hits('{"SortIntCalc": 1203001, "rank": 660101}', "page.json"), [])
        self.assertEqual(self.hits('{"AthleteID": 1203001}', "page.json"), [(1, "a real id")])

    def test_a_name_the_public_sources_have_is_not_guarded(self):
        maps = self.tmp / "public.map.json"
        maps.write_text(json.dumps({"ids": {}, "value_kinds": {"Central Coast Section Finals": ["meet"],
                                                                 "Linnea Penhallow": ["person"]}}))
        _, names = real_values.real_values([maps], [], public={"central coast section finals"})
        self.assertEqual(names, {"linnea penhallow", "penhallow linnea"})
        # Public: the Area files and the real trees' national, level, State and Area names, not a
        # league's (and never the fixtures, which the guard checks).
        trees = self.tmp / "rec" / "navinfo"
        trees.mkdir(parents=True)
        (trees / "2026.json").write_text(json.dumps({"regions": [
            {"id": 1, "name": "California", "state": "CA", "divType": "State", "depth": 1},
            {"id": 2, "name": " Central Coast", "state": "CA", "divType": "Section", "depth": 2},
            {"id": 3, "name": " Wren Valley League", "state": "CA", "divType": "League", "depth": 3}]}))
        public = real_values.public_names(trees=str(self.tmp / "*" / "navinfo" / "*.json"))
        self.assertIn("central coast", public)
        self.assertNotIn("wren valley league", public)
        self.assertIn("san luis obispo", public, "an Area file's county")

    def test_an_allowed_place(self):
        self.assertEqual(self.hits("meet 660101", "docs/meets.md"), [])
        self.assertEqual(self.hits("meet 660101", "README.md"), [(1, "a real id")])

    def test_refresh_adds_and_rebuild_replaces(self):
        text = self.list.read_text()
        self.assertNotRegex(text, r"(?i)Linnea|Penhallow|Wrenfield|1203001|660101|7700123|salt")
        maps = self.tmp / "other.map.json"
        maps.write_text(json.dumps({"ids": {"1209001": 900000009}, "value_kinds": {}}))
        self.refresh(maps)
        added = real_values.Guard.load(self.list)
        self.assertEqual(added.allow, self.guard.allow)
        self.assertEqual([kind for _, _, kind in added.scan_text("1209001 1203001")], ["a real id", "a real id"])
        self.refresh(maps, "--rebuild")
        rebuilt = real_values.Guard.load(self.list)
        self.assertEqual(rebuilt.allow, self.guard.allow)
        self.assertEqual([kind for _, _, kind in rebuilt.scan_text("1209001 1203001")], ["a real id"])

    def test_an_escaped_name_or_id_in_json(self):
        page = '{"AthleteName": "Linnea Penh\\u0061llow", "Note": "id \\u0031203001", "Team": "Zo\\u00eb"}'
        self.assertEqual(self.hits(page, "page.json"), [(1, "a real name"), (1, "a real id")])

    def test_a_wrong_key_fails_the_scan(self):
        self.assertTrue(self.guard.verifies())
        wrong = real_values.Guard.load(self.list, key=bytes.fromhex("22" * 32))
        self.assertFalse(wrong.verifies())
        with mock.patch.dict(os.environ, {real_values.KEY_ENV: "22" * 32}), redirect_stderr(io.StringIO()) as err:
            self.assertEqual(real_values.main(["scan", "--list", str(self.list)]), 2)
        self.assertIn("not the list's", err.getvalue())

    def test_another_key_finds_nothing(self):
        other = real_values.Guard.load(self.list, key=bytes.fromhex("22" * 32))
        self.assertEqual(other.scan_text("Linnea Penhallow 1203001"), [])

    def test_the_key_never_reaches_a_terminal(self):
        terminal = io.StringIO()
        terminal.isatty = lambda: True
        with redirect_stdout(terminal), redirect_stderr(io.StringIO()):
            self.assertEqual(real_values.main(["key"]), 2)
        self.assertNotIn(MADE_UP_KEY, terminal.getvalue())
        with redirect_stdout(io.StringIO()) as out:
            self.assertEqual(real_values.main(["key"]), 0)
        self.assertEqual(out.getvalue(), MADE_UP_KEY)
        derived = real_values.derive("ab" * 32)
        self.assertEqual(len(derived), 32)
        self.assertNotEqual(derived.hex(), "ab" * 32)

    @unittest.skipUnless(shutil.which("git"), "needs git")
    def test_scan_reads_the_tracked_text_files_and_names_places(self):
        repo = self.tmp / "repo"
        repo.mkdir()
        subprocess.run(["git", "init", "-q"], cwd=str(repo), check=True)
        (repo / "a.md").write_text("intro\nby Linnea Penhallow\n")
        (repo / "b.bin").write_bytes(b"\xff\xfe1203001")
        (repo / "untracked.md").write_text("Linnea Penhallow")
        subprocess.run(["git", "add", "a.md", "b.bin"], cwd=str(repo), check=True)
        self.assertEqual(real_values.scan(repo, self.guard), [("a.md", 2, 4, "a real name")])


if __name__ == "__main__":
    unittest.main()
