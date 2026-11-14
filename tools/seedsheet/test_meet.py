"""seedsheet's meet mode tests: a config with a meet id, its events (listed, or "all"), the
events ath refuses ("not seeded"), and the meet's index page.

They read saved ath output for the made-up meet 900700 in tests/fixtures/meets/ (made-up people
only): testdata/meet-900700/ath-meet.json is `ath meet 900700 --json`, and f-13-800m and m-13-800m
(.json, .csv) are `ath seed --meet 900700 --gender f|m --div 13 --event 800m --json` and the CSV it
wrote, each with its as_of pinned. tests/seedsheet_meet.rs fails when they are no longer what ath
prints. ath's refusals below are its own messages for the meet's other events. No test here calls
ath or Athletic.net, except AgainstAth, which runs the ath binary that ATH_TEST_BIN names (cargo
test sets it) on the fixtures. SEEDSHEET_BLESS=1 rewrites the expected pages in testdata/expected/.
"""
import io
import json
import os
import re
import sys
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from datetime import date
from pathlib import Path
from unittest import mock

import build
import meet_index
import seedsheet

TESTDATA = Path(__file__).parent / "testdata"
SAVED = TESTDATA / "meet-900700"
EXPECTED = TESTDATA / "expected" / "900700"
FIXTURES = Path(__file__).resolve().parents[2] / "tests" / "fixtures"
MEET = json.loads((SAVED / "ath-meet.json").read_text())
PULLED = "2026-11-07"
THEO = 900100002  # the configured Athlete (made up, as every fixture person is)
NICO = 900100012  # runs leg 2 for Pinecrest Academy A in the boys' 4x800m

# What `ath seed --meet` says for the meet's events it does not seed, as ath prints them.
MIXED = ("is a relay whose legs are not all one flat race (a medley or shuttle relay), and ath seed seeds only "
         "relays of four equal legs (such as 4x800m or 4xMile) so far")
REFUSALS = {
    "Shot": "Shot Put (Shot) is a field event; ath seeds running events only",
    "sprintmed2248": f"SMR 1600m (sprintmed2248) {MIXED}; "
                     "`ath meet 900700 --gender m --div 13 --event sprintmed2248` shows its teams and legs",
    "55shuttleh": f"4x55 Shuttle Hurdles (55shuttleh) {MIXED}; "
                  "`ath meet 900700 --gender m --div 13 --event 55shuttleh` shows its teams and legs "
                  "(it is also only run indoors, and ath seeds on outdoor Personal records)",
    "55m": "55 Meters (55m) is only run indoors, and ath seeds on outdoor Personal records, so it has "
           "no marks to seed on; indoor seeding comes later",
}
# The page's words for a medley or shuttle relay's refusal (meet_index.public).
MIXED_PUBLIC = ("is a relay whose legs are not all one flat race (a medley or shuttle relay), and only relays of "
                "four equal legs are seeded (such as 4x800m or 4xMile) so far")


def seeded(gender, event="800m"):
    """The saved `ath seed --meet` report and CSV for the meet's girls' or boys' Varsity 800m, or the
    boys' 4x800m (a relay's sheet: tests/pipeline.rs keeps testdata/ath-seed-relay.* current)."""
    if event == "4x800m":
        return (json.loads((TESTDATA / "ath-seed-relay.json").read_text()),
                (TESTDATA / "ath-seed-relay.csv").read_text())
    return (json.loads((SAVED / f"{gender}-13-{event}.json").read_text()),
            (SAVED / f"{gender}-13-{event}.csv").read_text())


def write_config(d, **over):
    cfg = {"meet": 900700, "athletes": [THEO], "output_dir": "out", "data_dir": "data", "events": "all"}
    cfg.update(over)
    path = Path(d) / "seedsheet.config.json"
    path.write_text(json.dumps(cfg))
    return path


def refused(path, says):
    with unittest.TestCase().assertRaises(SystemExit) as e:
        seedsheet.load_config(path)
    if says not in str(e.exception):
        raise AssertionError(f"{says!r} not in {e.exception}")


# What ath prints when Athletic.net refuses a meet's entries (src/athletic.rs `refused`).
MEET_REFUSED = ("Athletic.net refused meet 900700 (HTTP 403). Reading a meet's entries may need a signed-in session "
                "with a paid Athletic.net subscription (ADR 0001), or the meet does not publish its entries.")

# What ath prints when no browser tab answers the relay (src/relay.rs), a transport failure.
NO_TAB = ("no Athletic.net browser tab connected to the ath relay on 127.0.0.1:8767 within 30s.\n\n"
          "Run `ath relay install` and add a bookmark whose URL is the bookmarklet it prints")


def default_seeds():
    """What ath seeds of meet 900700: both 800m, and the boys' 4x800m."""
    out = {(g, "800m"): (json.dumps(seeded(g)[0]), seeded(g)[1]) for g in ("f", "m")}
    relay, relay_csv = seeded("m", "4x800m")
    out[("m", "4x800m")] = (json.dumps(relay), relay_csv)
    return out


def fake_resolve(meet, gender, div, event):
    """What the stand-in `ath meet --resolve` finds: a row of meet's events by gender, division (id,
    name or short name) and event (code or name), exactly. ath's own matching is tested in Rust
    (tests/meet.rs); seedsheet only reads what it prints."""
    ids = {str(d["id"]): d["id"] for d in meet["divisions"]}
    ids.update({str(n).lower(): d["id"] for d in meet["divisions"] for n in (d["name"], d["short"]) if n})
    div_id = ids.get(str(div).lower())
    for row in meet["events"]:
        if (row["gender"], row["division_id"]) == (gender, div_id) and event.lower() in (
                row["event_short"].lower(), row["event"].lower()):
            return row
    return None


def fake_ath(meet=MEET, seeds=None, calls=None, meet_error=None, resolve_error=None):
    """A stand-in for ath: `ath meet` prints meet (or fails with meet_error), `ath meet --resolve`
    prints the event it finds (fake_resolve; or fails with resolve_error, or as ath does when there
    is none), `ath seed --meet` prints and writes the saved sheets for the 800m and the 4x800m and
    refuses the rest with ath's message on stderr (exit 1). seeds: {(gender, event): (stdout, csv
    or None)} replaces what a seed run gives; ("error", message) fails with message, and
    ("interrupt",) is a Ctrl-C."""
    seeds = seeds if seeds is not None else default_seeds()

    def run(cmd, **kw):
        if calls is not None:
            calls.append(cmd)

        def fail(message):
            os.write(kw["stderr"], f"ath: some warning\nError: {message}\n".encode())
            return mock.Mock(returncode=1, stdout="")
        arg = lambda name: cmd[cmd.index(name) + 1]  # noqa: E731
        if cmd[1] == "meet" and "--resolve" in cmd:
            if resolve_error:
                return fail(resolve_error)
            row = fake_resolve(meet, arg("--gender"), arg("--div"), arg("--event"))
            if row is None:
                return fail(f"meet 900700 has no {arg('--event')} entries in the {arg('--div')} division (yet)")
            return mock.Mock(returncode=0, stdout=json.dumps({**meet, "resolved": row}))
        if cmd[1] == "meet":
            return fail(meet_error) if meet_error else mock.Mock(returncode=0, stdout=json.dumps(meet))
        key = (arg("--gender"), arg("--event"))
        if key not in seeds:
            return fail(REFUSALS.get(key[1], f"meet 900700 has no {key[1]}"))
        if seeds[key][0] == "error":
            return fail(seeds[key][1])
        if seeds[key][0] == "interrupt":
            raise KeyboardInterrupt
        stdout, csv_text = seeds[key]
        if csv_text is not None:
            Path(arg("--out")).write_text(csv_text)
        return mock.Mock(returncode=0, stdout=stdout)
    return run


def run_main(d, args=(), run=None, config=None):
    """seedsheet.main on a config in d (written from config, or "all" of meet 900700), with ath
    stubbed (run). Returns what it printed, and its exit message."""
    out = io.StringIO()
    path = write_config(d, **(config or {}))
    with mock.patch("subprocess.run", run or fake_ath()), mock.patch.object(seedsheet, "find_ath", lambda: "ath"), \
            mock.patch.object(seedsheet, "older_place", lambda *parts: Path(d) / "older"), \
            redirect_stdout(out), redirect_stderr(io.StringIO()):
        try:
            seedsheet.main(["--config", str(path), *args])
        except SystemExit as e:
            out.write(f"\nexit: {e}")
    return out.getvalue()


def index_rows(html):
    """The index's rows: (group, event, entries, yours, what it links to or why not seeded)."""
    rows = []
    for group, body in re.findall(r'<th colspan="\d" scope="rowgroup">(.*?)</th></tr>(.*?)</tbody>', html,
                                  flags=re.S):
        for tr in re.findall(r"<tr[^>]*>(.*?)</tr>", body):
            ev = re.sub(r"<div.*", "", re.sub(r"<a[^>]*>([^<]*)</a>", r"\1", tr.split("</td>")[0]))
            ev = re.sub(r"<[^>]+>", "", ev)
            n = re.search(r'<td class="n">([^<]*)</td>', tr).group(1)
            mine = re.search(r'<td class="mine">(?:<span class="hl">(\d+)</span>)?</td>', tr)
            page = re.search(r'<a href="([^"]+\.html)">', tr)
            why = re.search(r'<span class="none">Not seeded</span> (.*?)</div>', tr)
            rows.append((group, ev, n, mine.group(1) if mine else None,
                         page.group(1) if page else f"not seeded: {why.group(1)}"))
    return rows


class Config(unittest.TestCase):
    def test_a_meet_id_is_meet_mode_and_a_name_is_roster_mode(self):
        with tempfile.TemporaryDirectory() as d:
            cfg = seedsheet.load_config(write_config(d))
            self.assertEqual((cfg["mode"], cfg["events"]), ("meet", "all"))
            cfg = seedsheet.load_config(write_config(d, meet="Spring Invitational", events=[
                {"label": "Girls 400m", "event": "400m", "roster": "girls.csv"}]))
            self.assertEqual(cfg["mode"], "roster")

    def test_listed_events_load(self):
        events = [{"gender": "m", "div": "V", "event": "800m", "label": "Boys 800"},
                  {"gender": "f", "div": 13, "event": "800 Meters"}]
        with tempfile.TemporaryDirectory() as d:
            cfg = seedsheet.load_config(write_config(d, events=events, fixtures_dir="fx"))
            self.assertEqual(cfg["events"], events)
            self.assertEqual(cfg["fixtures_dir"], (Path(d) / "fx").resolve())

    def test_the_meet_example_config_loads(self):
        cfg = seedsheet.load_config(Path(__file__).parent / "seedsheet.meet.config.example.json")
        self.assertEqual(cfg["mode"], "meet")
        self.assertEqual(cfg["meet"], 900700)

    def test_a_bad_meet_config_says_what_is_wrong(self):
        ev = {"gender": "m", "div": "V", "event": "800m"}
        with tempfile.TemporaryDirectory() as d:
            for over, says in (
                    ({"meet": None}, "'meet' is required: the meet's id (a number) to seed its entries, or its name"),
                    ({"meet": True}, "'meet' is required"),
                    ({"meet": 0}, "'meet' must be a meet id (a positive number)"),
                    ({"meet_date": "2099-01-09"}, "'meet_date' is not taken with a meet id"),
                    ({"events": "some"}, "'events' is required: \"all\", or a list of events"),
                    ({"events": []}, "'events' lists no events (\"all\" takes every event with entries)"),
                    ({"events": [{**ev, "roster": "r.csv"}]}, "event 1 has a roster, but 'meet' is a meet id"),
                    ({"events": [{**ev, "gender": "boys"}]}, "event 1 needs 'gender': \"f\" (girls) or \"m\" (boys)"),
                    ({"events": [{"gender": "m", "event": "800m"}]}, "event 1 needs 'div'"),
                    ({"events": [{**ev, "div": True}]}, "event 1 needs 'div'"),
                    ({"events": [ev, {"gender": "m", "div": "V"}]}, "event 2 needs 'event'"),
                    ({"events": [{**ev, "label": ""}]}, "event 1's 'label' must be text"),
                    ({"events": ["800m"]}, "event 1 is not an object"),
                    ({"athletes": ["x"]}, "'athletes' must be a list of athlete ids"),
                    ({"fixtures_dir": 5}, "'fixtures_dir' must be a folder")):
                refused(write_config(d, **over), says)

    def test_a_roster_config_cannot_take_all_or_a_meet_id_as_text(self):
        with tempfile.TemporaryDirectory() as d:
            refused(write_config(d, meet="Spring Invitational", events="all"),
                    "'events': \"all\" takes the meet's entries, so 'meet' must be its id (a number)")
            refused(write_config(d, meet="900700", events=[{"gender": "m", "div": "V", "event": "800m"}]),
                    "write its id as a number (\"meet\": 900700)")

    def test_a_config_that_is_not_an_object_is_refused(self):
        with tempfile.TemporaryDirectory() as d:
            path = Path(d) / "x.config.json"
            path.write_text("[1]")
            refused(path, "a config is a JSON object")
            path.write_text("{nope")
            refused(path, "not valid JSON")


class Events(unittest.TestCase):
    def cfg(self, d, **over):
        cfg = seedsheet.load_config(write_config(d, **over))
        cfg["config"] = Path(d) / "seedsheet.config.json"
        return cfg

    def row(self, gender, event, div=13):
        return next(r for r in MEET["events"] if (r["gender"], r["event_short"], r["division_id"]) == (gender, event, div))

    def resolved(self, *pairs):
        """A saved resolution for each (spec, row or reason)."""
        return {seedsheet.spec_key(spec): ({"row": found} if isinstance(found, dict) else {"reason": found})
                for spec, found in pairs}

    def test_all_is_every_event_ath_meet_lists_in_its_order(self):
        with tempfile.TemporaryDirectory() as d:
            events = seedsheet.meet_events(self.cfg(d), MEET, {})
        self.assertEqual([(e["gender"], e["div"], e["event"], e["name"], e["label"], e["row_label"]) for e in events], [
            ("f", "13", "800m", "girls-13-800m", "Girls Varsity 800 Meters", "800 Meters"),
            ("m", "13", "800m", "boys-13-800m", "Boys Varsity 800 Meters", "800 Meters"),
            ("m", "13", "4x800m", "boys-13-4x800m", "Boys Varsity 4x800 Relay", "4x800 Relay"),
            ("m", "13", "Shot", "boys-13-shot", "Boys Varsity Shot Put", "Shot Put"),
            ("m", "13", "sprintmed2248", "boys-13-sprintmed2248", "Boys Varsity SMR 1600m", "SMR 1600m"),
            ("m", "13", "55shuttleh", "boys-13-55shuttleh", "Boys Varsity 4x55 Shuttle Hurdles",
             "4x55 Shuttle Hurdles"),
            ("m", "14", "55m", "boys-14-55m", "Boys Junior Varsity 55 Meters", "55 Meters"),
        ])
        self.assertEqual(events[0]["stem"], "900700-girls-13-800m")
        self.assertEqual(events[0]["url"], "https://www.athletic.net/TrackAndField/meet/900700/entries/f/13/800m")

    def test_file_names_are_ids_so_a_renamed_division_keeps_its_files(self):
        # Gender, division id and event code: never the division's or the event's name.
        renamed = {**MEET, "events": [{**r, "division": "Varsity Gold", "event": "800 Meter Run"} for r in MEET["events"]]}
        with tempfile.TemporaryDirectory() as d:
            before = seedsheet.meet_events(self.cfg(d), MEET, {})
            after = seedsheet.meet_events(self.cfg(d), renamed, {})
        self.assertEqual([e["stem"] for e in before], [e["stem"] for e in after])
        self.assertEqual(after[1]["label"], "Boys Varsity Gold 800 Meter Run")

    def test_a_meet_with_no_events_with_entries_has_none(self):
        with tempfile.TemporaryDirectory() as d:
            self.assertEqual(seedsheet.meet_events(self.cfg(d), {**MEET, "events": []}, {}), [])

    def test_listed_events_are_what_ath_meet_resolve_found_in_the_index_order(self):
        listed = [{"gender": "m", "div": "jv", "event": "55 meters"},
                  {"gender": "m", "div": "13", "event": "4X800M"},
                  {"gender": "f", "div": "Varsity", "event": "800m", "label": "Girls 800"},
                  {"gender": "m", "div": 13, "event": "800 Meters"}]
        resolved = self.resolved((listed[0], self.row("m", "55m", 14)), (listed[1], self.row("m", "4x800m")),
                                 (listed[2], self.row("f", "800m")), (listed[3], self.row("m", "800m")))
        with tempfile.TemporaryDirectory() as d:
            events = seedsheet.meet_events(self.cfg(d, events=listed), MEET, resolved)
        # Girls first, then each division in the meet's order, then as listed.
        self.assertEqual([(e["gender"], e["div"], e["event"], e["name"], e["label"], e["row_label"]) for e in events], [
            ("f", "13", "800m", "girls-13-800m", "Girls 800", "Girls 800"),
            ("m", "13", "4x800m", "boys-13-4x800m", "Boys Varsity 4x800 Relay", "4x800 Relay"),
            ("m", "13", "800m", "boys-13-800m", "Boys Varsity 800 Meters", "800 Meters"),
            ("m", "14", "55m", "boys-14-55m", "Boys Junior Varsity 55 Meters", "55 Meters"),
        ])

    def test_an_event_ath_did_not_find_is_listed_with_why_and_has_no_files(self):
        listed = [{"gender": "m", "div": "V", "event": "1mile"}, {"gender": "f", "div": "Frosh", "event": "400m"}]
        why = "meet 900700 has no 1mile entries in the Varsity division (yet)"
        with tempfile.TemporaryDirectory() as d:
            events = seedsheet.meet_events(self.cfg(d, events=listed), MEET, self.resolved((listed[0], why)))
        self.assertEqual([(e["row_label"], e["name"], e["url"], e["unfound"]) for e in events], [
            ("400m", None, None, "not looked up yet: run seedsheet without --no-fetch"),
            ("1mile", None, None, why),
        ])

    def test_the_same_resolved_event_twice_is_refused_whatever_its_labels(self):
        listed = [{"gender": "m", "div": "Varsity", "event": "800m", "label": "Boys 800"},
                  {"gender": "m", "div": "v", "event": "800 Meters", "label": "Varsity 800"}]
        resolved = self.resolved((listed[0], self.row("m", "800m")), (listed[1], self.row("m", "800m")))
        with tempfile.TemporaryDirectory() as d:
            with self.assertRaises(SystemExit) as e:
                seedsheet.meet_events(self.cfg(d, events=listed), MEET, resolved)
        self.assertIn("two listed events are the same event of meet 900700 (boys-13-800m); list each once",
                      str(e.exception))

    def test_a_bare_number_is_refused_in_the_config(self):
        with tempfile.TemporaryDirectory() as d:
            refused(write_config(d, events=[{"gender": "m", "div": "V", "event": " 800 "}]),
                    "event 1's 'event' ' 800 ' is a bare number; did you mean \"800m\"?")

    def test_listed_events_are_resolved_by_ath_once_each_and_saved(self):
        listed = [{"gender": "m", "div": "V", "event": "800m"}, {"gender": "m", "div": "V", "event": "1mile"}]
        calls = []
        with tempfile.TemporaryDirectory() as d:
            run_main(d, run=fake_ath(calls=calls), config={"events": listed, "fixtures_dir": "fx"})
            fx = ["--fixtures-dir", str((Path(d) / "fx").resolve())]
            self.assertEqual([c for c in calls if "--resolve" in c], [
                ["ath", "meet", "900700", "--gender", "m", "--div", "V", "--event", "800m", "--resolve", "--json", *fx],
                ["ath", "meet", "900700", "--gender", "m", "--div", "V", "--event", "1mile", "--resolve", "--json",
                 *fx]])
            # ath seed runs only for the event ath found; the other is listed with ath's reason.
            self.assertEqual([c[c.index("--event") + 1] for c in calls if c[1] == "seed"], ["800m"])
            rows = index_rows((Path(d) / "out" / "900700" / "index.html").read_text())
            self.assertEqual([r[4] for r in rows], [
                "boys-13-800m.html", "not seeded: meet 900700 has no 1mile entries in the V division (yet)"])
            # --no-fetch reads the saved resolutions: no ath at all.
            calls.clear()
            run_main(d, args=["--no-fetch"], run=fake_ath(calls=calls), config={"events": listed, "fixtures_dir": "fx"})
            self.assertEqual(calls, [])
            self.assertEqual([r[4] for r in index_rows((Path(d) / "out" / "900700" / "index.html").read_text())],
                             [r[4] for r in rows])


class Transport(unittest.TestCase):
    """ath's failures: a transport failure stops the run, anything else is the event's reason."""

    def test_aths_transport_errors_are_told_from_refusals(self):
        import report
        transport = [
            NO_TAB,
            "the Athletic.net tab running the ath relay on 127.0.0.1:8767 stopped answering (nothing from it for 30s).",
            "the Athletic.net tab running the ath relay took GET https://www.athletic.net/x but did not answer within "
            "30s (was it closed or reloaded?)",
            "the browser relay dropped GET https://www.athletic.net/x",
            "Athletic.net returned a Cloudflare challenge (HTTP 403) after 3 attempts. Your session cookie is likely "
            "expired",
            "reading bio JSON for athlete 1\n\nCaused by:\n    error sending request for url (https://x): dns error",
            # Another ath holds the relay's port (src/relay.rs): nothing can be fetched this run.
            "starting the browser relay on 127.0.0.1:8767 (is another ath already running? Wait for it, or pass a "
            "cookie to use the cookie transport)\n\nCaused by:\n    Address already in use (os error 48)",
        ]
        for message in transport:
            failure = report.ath_error(f"ath: note\nError: {message}\n", "ath seed", 1)
            self.assertTrue(failure.transport, message)
            self.assertEqual(failure.reason, message.splitlines()[0])
        for message in [*REFUSALS.values(), "meet 900700 has no 1mile entries in the Varsity division (yet)",
                        MEET_REFUSED]:
            self.assertFalse(report.ath_error(f"Error: {message}\n", "ath seed", 1).transport, message)
        # Athletic.net refusing the meet is not a transport failure: it is the whole meet's answer.
        failure = report.ath_error(f"Error: {MEET_REFUSED}\n", "ath seed", 1)
        self.assertEqual((failure.transport, failure.meet_wide), (False, True))
        self.assertFalse(report.ath_error(f"Error: {REFUSALS['Shot']}\n", "ath seed", 1).meet_wide)
        self.assertEqual(report.ath_error("panic\n", "ath seed", 101),
                         report.AthFailure("ath seed failed (exit 101)", False, False))

    def test_a_refused_meet_runs_no_more_ath_and_every_event_says_why(self):
        calls = []
        with tempfile.TemporaryDirectory() as d:
            out = run_main(d, run=fake_ath(seeds={("f", "800m"): ("error", MEET_REFUSED)}, calls=calls))
            self.assertEqual([c[1] for c in calls], ["meet", "seed"])
            self.assertNotIn("Reconnect", out)
            self.assertNotIn("exit:", out)
            rows = index_rows((Path(d) / "out" / "900700" / "index.html").read_text())
            self.assertEqual({r[4] for r in rows}, {
                "not seeded: The results site refused meet 900700 (HTTP 403). Reading a meet&#x27;s entries may "
                "need a signed-in session with a paid subscription, or the meet does not publish its entries."})

    def test_a_transport_failure_stops_the_run_and_leaves_what_was_saved(self):
        with tempfile.TemporaryDirectory() as d:
            run_main(d)
            data = Path(d) / "data"
            # The saved meet, sheets and reasons (an .ath.csv is ath's scratch copy of a run's CSV).
            before = {p.name: p.read_bytes() for p in data.iterdir() if not p.name.endswith(".ath.csv")}
            calls = []
            seeds = {**default_seeds(), ("m", "800m"): ("error", NO_TAB)}
            out = run_main(d, run=fake_ath(seeds=seeds, calls=calls))
            self.assertIn("exit: \nStopped: ath could not reach Athletic.net (no Athletic.net browser tab connected "
                          "to the ath relay on 127.0.0.1:8767 within 30s.)", out)
            self.assertIn("The sheets and reasons saved before stay as they were.", out)
            # ath meet, the girls' 800m, the boys' 800m: then nothing more.
            self.assertEqual([c[1] for c in calls], ["meet", "seed", "seed"])
            after = {p.name: p.read_bytes() for p in data.iterdir() if p.name in before}
            del before["900700-girls-13-800m.json"], after["900700-girls-13-800m.json"]  # pulled again
            self.assertEqual(after, before)
            self.assertFalse((data / "900700-boys-13-800m.not-seeded.json").exists())

    def test_a_transport_failure_reading_the_meet_stops_at_once(self):
        with tempfile.TemporaryDirectory() as d:
            run_main(d)
            calls = []
            out = run_main(d, run=fake_ath(meet_error=NO_TAB, calls=calls))
            self.assertIn("Stopped: ath could not reach Athletic.net", out)
            self.assertEqual([c[1] for c in calls], ["meet"])

    def test_an_interrupt_stops_the_run_and_never_becomes_a_reason(self):
        with tempfile.TemporaryDirectory() as d:
            run_main(d)
            reason = Path(d) / "data" / "900700-boys-13-shot.not-seeded.json"
            before = reason.read_bytes()
            out = run_main(d, run=fake_ath(seeds={**default_seeds(), ("m", "Shot"): ("interrupt",)}))
            self.assertIn("exit: \nInterrupted. The sheets and reasons saved before stay as they were.", out)
            self.assertEqual(reason.read_bytes(), before)
        with tempfile.TemporaryDirectory() as d:
            run_main(d, run=fake_ath(seeds={("f", "800m"): ("interrupt",)}))
            self.assertEqual(list((Path(d) / "data").glob("*.not-seeded.json")), [])

    def test_a_stopped_run_still_rebuilds_the_index_from_what_is_saved(self):
        # Stopped at the boys' 800m: its sheet from before stays, its page and the index agree with
        # what is saved, and a page the index no longer links is gone.
        for stop in (("error", NO_TAB), ("interrupt",)):
            with tempfile.TemporaryDirectory() as d:
                run_main(d)
                folder = Path(d) / "out" / "900700"
                (folder / "index.html").unlink()
                # A page this config wrote once (in its manifest) that the index no longer links.
                (folder / "boys-13-1600m.html").write_text("x")
                manifest = json.loads((folder / ".seedsheet.json").read_text())
                (folder / ".seedsheet.json").write_text(json.dumps({**manifest, "files": manifest["files"] + [
                    "boys-13-1600m.html"]}))
                out = run_main(d, run=fake_ath(seeds={**default_seeds(), ("m", "800m"): stop}))
                self.assertIn("The index and pages are built from what is saved.", out)
                rows = index_rows((folder / "index.html").read_text())
                self.assertEqual([r[4] for r in rows[:4]], [
                    "girls-13-800m.html", "boys-13-800m.html", "boys-13-4x800m.html",
                    "not seeded: Shot Put (Shot) is a field event; only running events are seeded"])
                self.assertFalse((folder / "boys-13-1600m.html").exists())


class Rosters(unittest.TestCase):
    """Roster mode keeps going past an interrupted event, and builds from saved sheets once ath
    cannot reach the site."""

    EVENTS = [{"label": "Girls 800", "event": "800m", "roster": "g.csv"},
              {"label": "Boys 800", "event": "800m", "roster": "b.csv"}]

    def fake(self, outcomes, calls):
        """ath seed --roster: per roster file, ("ok", gender) prints that saved sheet, ("error", message)
        fails, ("interrupt",) is a Ctrl-C."""
        def run(cmd, **kw):
            calls.append(cmd)
            what = outcomes[Path(cmd[cmd.index("--roster") + 1]).name]
            if what[0] == "interrupt":
                raise KeyboardInterrupt
            if what[0] == "error":
                os.write(kw["stderr"], f"Error: {what[1]}\n".encode())
                return mock.Mock(returncode=1, stdout="")
            seed, csv_text = seeded(what[1])
            Path(cmd[cmd.index("--out") + 1]).write_text(csv_text)
            return mock.Mock(returncode=0, stdout=json.dumps(seed))
        return run

    def run_rosters(self, d, outcomes):
        calls = []
        out = run_main(d, run=self.fake(outcomes, calls), config={"meet": "Spring", "events": self.EVENTS})
        return calls, out

    def test_ctrl_c_skips_that_event_and_goes_on(self):
        with tempfile.TemporaryDirectory() as d:
            calls, out = self.run_rosters(d, {"g.csv": ("interrupt",), "b.csv": ("ok", "m")})
            self.assertEqual(len(calls), 2)
            self.assertIn("  Girls 800: skipped (Ctrl-C)\n  Girls 800: no saved sheet yet, so no page\n", out)
            self.assertNotIn("kept", out)
            self.assertTrue((Path(d) / "out" / "spring-boys-800.html").exists())
            self.assertNotIn("exit:", out)

    def test_unreachable_stops_fetching_and_builds_from_saved_sheets(self):
        with tempfile.TemporaryDirectory() as d:
            self.run_rosters(d, {"g.csv": ("ok", "f"), "b.csv": ("ok", "m")})
            calls, out = self.run_rosters(d, {"g.csv": ("error", NO_TAB), "b.csv": ("ok", "m")})
            self.assertEqual(len(calls), 1)
            self.assertIn("The rest are built from their saved sheets.", out)
            self.assertIn(f"Wrote {(Path(d) / 'out').resolve()}/spring-boys-800.html", out)
            self.assertIn("exit: \nStopped: ath could not reach Athletic.net", out)

    def test_the_stderr_pipe_is_closed_whatever_happens(self):
        import report
        tees = []
        real = report.StderrTee

        def tee():
            tees.append(real())
            return tees[-1]

        def broken(cmd, **kw):
            raise OSError("no such file")
        with mock.patch.object(report, "StderrTee", tee), mock.patch("subprocess.run", broken):
            with self.assertRaises(OSError):
                report.run_ath_report(["ath", "seed"], "x", [])
        self.assertIsNone(tees[0].fd)
        self.assertFalse(tees[0]._thread.is_alive())


class Run(unittest.TestCase):
    """seedsheet.py in meet mode end to end, with ath stubbed out (no ath, no network)."""

    def test_all_seeds_what_it_can_and_lists_the_rest_on_the_index(self):
        calls = []
        with tempfile.TemporaryDirectory() as d:
            out = run_main(d, run=fake_ath(calls=calls), config={"fixtures_dir": "fx"})
            here = Path(d).resolve()
            fx = ["--fixtures-dir", str(here / "fx")]
            self.assertEqual(calls[0], ["ath", "meet", "900700", "--json", *fx])
            self.assertEqual(calls[2], ["ath", "seed", "--meet", "900700", "--gender", "m", "--div", "13",
                                        "--event", "800m", "--out",
                                        str(here / "data" / "900700-boys-13-800m.ath.csv"), "--json", *fx])
            self.assertEqual(len(calls), 8)
            folder = Path(d) / "out" / "900700"
            self.assertEqual(sorted(p.name for p in folder.iterdir()), [
                ".seedsheet.json", "boys-13-4x800m.csv", "boys-13-4x800m.html", "boys-13-800m.csv",
                "boys-13-800m.html", "girls-13-800m.csv", "girls-13-800m.html", "index.html"])
            self.assertEqual(json.loads((folder / ".seedsheet.json").read_text()), {
                "config": str((Path(d) / "seedsheet.config.json").resolve()),
                "files": sorted(p.name for p in folder.iterdir() if p.name != ".seedsheet.json")})
            self.assertEqual((folder / "boys-13-800m.csv").read_text(), seeded("m")[1])
            self.assertEqual((folder / "boys-13-4x800m.csv").read_text(), seeded("m", "4x800m")[1])
            self.assertEqual(index_rows((folder / "index.html").read_text()), [
                ("Girls &middot; Varsity", "800 Meters", "1 athlete", None, "girls-13-800m.html"),
                ("Boys &middot; Varsity", "800 Meters", "4 athletes", "1", "boys-13-800m.html"),
                ("Boys &middot; Varsity", "4x800 Relay", "4 teams", None, "boys-13-4x800m.html"),
                ("Boys &middot; Varsity", "Shot Put", "", None,
                 "not seeded: Shot Put (Shot) is a field event; only running events are seeded"),
                ("Boys &middot; Varsity", "SMR 1600m", "", None, f"not seeded: SMR 1600m (sprintmed2248) {MIXED_PUBLIC}"),
                ("Boys &middot; Varsity", "4x55 Shuttle Hurdles", "", None,
                 f"not seeded: 4x55 Shuttle Hurdles (55shuttleh) {MIXED_PUBLIC} (it is also only run indoors, and "
                 f"seeding reads outdoor Personal records)"),
                ("Boys &middot; Junior Varsity", "55 Meters", "", None,
                 "not seeded: 55 Meters (55m) is only run indoors, and seeding reads outdoor Personal records, "
                 "so it has no marks to seed on; indoor seeding comes later"),
            ])
            # The terminal gets ath's own reason, hint and all; the run does not fail.
            self.assertIn(f"Boys Varsity SMR 1600m: not seeded ({REFUSALS['sprintmed2248']})", out)
            self.assertIn("Theo Quill 2:00.45 (seed 3)", out)
            self.assertIn("Boys Varsity 4x800 Relay: saved", out)
            self.assertIn("(4 teams, 2 with an aggregate)", out)
            self.assertIn(f"Wrote {folder.resolve()}/index.html: 3 of 7 events seeded", out)
            self.assertNotIn("exit:", out)
            saved = json.loads((Path(d) / "data" / "900700-boys-13-sprintmed2248.not-seeded.json").read_text())
            self.assertEqual(saved["reason"], REFUSALS["sprintmed2248"])

    def test_event_pages_link_back_to_the_index(self):
        with tempfile.TemporaryDirectory() as d:
            run_main(d)
            folder = Path(d) / "out" / "900700"
            page = (folder / "boys-13-800m.html").read_text()
            self.assertIn('<p class="kicker"><a class="back" href="index.html">&larr; All events</a> &middot; '
                          'Riverton Winter Classic &middot; January 9, 2099</p>', page)
            self.assertIn("<h1>Boys Varsity 800 Meters seed sheet</h1>", page)
            index = (folder / "index.html").read_text()
            for name in ("boys-13-800m", "girls-13-800m"):
                self.assertIn(f'<a href="{name}.html">', index)
                self.assertIn(f'<a class="x" href="{name}.csv" download>CSV</a>', index)
                self.assertTrue((folder / f"{name}.html").exists())

    def test_no_fetch_rebuilds_the_index_and_pages_from_what_was_saved(self):
        with tempfile.TemporaryDirectory() as d:
            run_main(d)
            index = Path(d) / "out" / "900700" / "index.html"
            before = index.read_text()
            index.unlink()
            calls = []
            out = run_main(d, args=["--no-fetch"], run=fake_ath(calls=calls))
            self.assertEqual(calls, [])
            self.assertEqual(index.read_text(), before)
            self.assertIn("not seeded (Shot Put (Shot) is a field event", out)

    def test_no_fetch_without_a_saved_meet_says_so(self):
        with tempfile.TemporaryDirectory() as d:
            out = run_main(d, args=["--no-fetch"])
            self.assertIn("exit: No saved meet 900700 yet, so nothing to build: run seedsheet without --no-fetch.", out)

    def test_a_failed_meet_fetch_uses_the_saved_meet(self):
        def down(cmd, **kw):  # the site answers, with an error: not a transport failure
            os.write(kw["stderr"], b"Error: Athletic.net answered meet 900700 with HTTP 500: Server Error\n")
            return mock.Mock(returncode=1, stdout="")
        with tempfile.TemporaryDirectory() as d:
            run_main(d)
            out = run_main(d, run=down)
            self.assertIn("meet 900700: ath meet failed (exit 1)\n", out)
            # One status line per event: ath failed, then the sheet kept.
            self.assertIn("  Boys Varsity 800 Meters: ath seed failed (exit 1)\n  Boys Varsity 800 Meters: kept the "
                          "sheet saved on", out)
            self.assertNotIn("kept what was saved", out)
            # Every sheet saved before stays; the events with none say why this run could not seed them.
            rows = index_rows((Path(d) / "out" / "900700" / "index.html").read_text())
            self.assertEqual([r[4] for r in rows[:3]],
                             ["girls-13-800m.html", "boys-13-800m.html", "boys-13-4x800m.html"])
            self.assertEqual(rows[3][4], "not seeded: The results site answered meet 900700 with HTTP 500: Server Error")

    def test_a_meet_with_no_events_with_entries_writes_an_index_saying_so(self):
        with tempfile.TemporaryDirectory() as d:
            out = run_main(d, run=fake_ath(meet={**MEET, "events": []}))
            self.assertIn("Meet 900700 (Riverton Winter Classic) has no events with entries yet (`ath meet 900700` "
                          "lists none), so there is nothing to seed", out)
            html = (Path(d) / "out" / "900700" / "index.html").read_text()
            self.assertIn("No event of this meet has entries yet, so there is nothing to seed.", html)
            self.assertNotIn("events seeded", html)
            self.assertNotIn('class="status"', html)
            self.assertNotIn('class="notes"', html)
            self.assertNotIn("<table>", html)

    def test_a_relay_sheet_is_seeded_by_its_kind(self):
        # A relay's sheet (kind "relay", teams, no entrants) goes through the real check and the
        # relay page; the index counts its teams and the configured Athletes running on them.
        relay, relay_csv = seeded("m", "4x800m")
        with tempfile.TemporaryDirectory() as d:
            out = run_main(d, config={"athletes": [THEO, NICO]})
            self.assertIn("Boys Varsity 4x800 Relay: 4 teams; Nico Vance 2:00.40 (leg 2, Pinecrest Academy A, seed 2)",
                          out)
            folder = Path(d) / "out" / "900700"
            index = (folder / "index.html").read_text()
            self.assertIn(("Boys &middot; Varsity", "4x800 Relay", "4 teams", "1", "boys-13-4x800m.html"),
                          index_rows(index))
            self.assertIn('<td class="n">4 teams</td>', index)
            page = (folder / "boys-13-4x800m.html").read_text()
            self.assertEqual(page, build.html(build.check(build.sheet_file(relay, relay_csv, date.today().isoformat())),
                                              {"meet": "Riverton Winter Classic", "label": "Boys Varsity 4x800 Relay",
                                               "athletes": [THEO, NICO], "index": "index.html",
                                               "pulled": date.today().isoformat(), "csv": relay_csv,
                                               "csv_name": "boys-13-4x800m.csv"}))
            self.assertIn('<a class="back" href="index.html">&larr; All events</a>', page)
            self.assertEqual((folder / "boys-13-4x800m.csv").read_text(), relay_csv)
        # A relay with no teams is not seeded, saying so; the roster check never applies.
        no_teams = {**relay, "teams": []}
        with tempfile.TemporaryDirectory() as d:
            out = run_main(d, run=fake_ath(seeds={("m", "4x800m"): (json.dumps(no_teams), None)}),
                           config={"events": [{"gender": "m", "div": "V", "event": "4x800m"}]})
            self.assertIn("not seeded (no accepted teams)", out)
            self.assertNotIn("roster", out)

    def test_pages_the_index_no_longer_links_are_removed(self):
        with tempfile.TemporaryDirectory() as d:
            run_main(d)
            folder = Path(d) / "out" / "900700"
            # This config's manifest names the files it wrote; others in the folder are not its own.
            for name in ("boys-13-1600m.html", "boys-13-1600m.csv", "girls-13-400m.html", "notes.html"):
                (folder / name).write_text("x")
            manifest = json.loads((folder / ".seedsheet.json").read_text())
            (folder / ".seedsheet.json").write_text(json.dumps({**manifest, "files": manifest["files"] + [
                "boys-13-1600m.html", "boys-13-1600m.csv"]}))
            (Path(d) / "data" / "900700-girls-13-800m.json").write_text("{not json")
            out = run_main(d, args=["--no-fetch"])
            self.assertEqual(sorted(p.name for p in folder.iterdir()), [
                ".seedsheet.json", "boys-13-4x800m.csv", "boys-13-4x800m.html", "boys-13-800m.csv",
                "boys-13-800m.html", "girls-13-400m.html", "index.html", "notes.html"])
            self.assertIn(f"removed {folder.resolve()}/boys-13-1600m.html (no longer on the index)", out)
            self.assertIn(f"removed {folder.resolve()}/girls-13-800m.html (no longer on the index)", out)
            rows = index_rows((folder / "index.html").read_text())
            self.assertEqual(rows[0][4], "not seeded: its saved seed sheet could not be read")
            # The unreadable sheet is moved aside, and its reason saved, so the next run says the same.
            data = Path(d) / "data"
            self.assertEqual((data / "900700-girls-13-800m.json.unreadable").read_text(), "{not json")
            self.assertFalse((data / "900700-girls-13-800m.json").exists())
            self.assertEqual(json.loads((data / "900700-girls-13-800m.not-seeded.json").read_text())["reason"],
                             "its saved seed sheet could not be read")

    def test_no_accepted_entrants_now_replaces_a_sheet_saved_before(self):
        # ath's answer now (everyone scratched) is the event's status: the old sheet goes aside,
        # its page goes, and the index says why.
        listed = {"events": [{"gender": "m", "div": "V", "event": "800m"}]}
        empty = {**seeded("m")[0], "entrants": []}
        with tempfile.TemporaryDirectory() as d:
            run_main(d, config=listed)
            folder = Path(d) / "out" / "900700"
            self.assertTrue((folder / "boys-13-800m.html").exists())
            run_main(d, run=fake_ath(seeds={("m", "800m"): (json.dumps(empty), None)}), config=listed)
            data = Path(d) / "data"
            self.assertFalse((data / "900700-boys-13-800m.json").exists())
            self.assertTrue((data / "900700-boys-13-800m.json.old").exists())
            self.assertFalse((folder / "boys-13-800m.html").exists())
            rows = index_rows((folder / "index.html").read_text())
            self.assertEqual(rows[0][4], "not seeded: no accepted entrants (2 entries scratched or not accepted)")
            out = run_main(d, args=["--no-fetch"], config=listed)
            self.assertIn("not seeded (no accepted entrants (2 entries scratched or not accepted))", out)

    def test_a_renamed_division_finds_its_saved_sheets(self):
        # Saved sheets and pages are named by gender, division id and event code.
        with tempfile.TemporaryDirectory() as d:
            run_main(d)
            renamed = {**MEET, "divisions": [{**v, "name": "Varsity Gold"} if v["id"] == 13 else v
                                             for v in MEET["divisions"]],
                       "events": [{**r, "division": "Varsity Gold"} if r["division_id"] == 13 else r
                                  for r in MEET["events"]]}
            calls = []
            run_main(d, run=fake_ath(meet=renamed, seeds={}, calls=calls))
            rows = index_rows((Path(d) / "out" / "900700" / "index.html").read_text())
            self.assertEqual([(r[0], r[4]) for r in rows[:3]], [
                ("Girls &middot; Varsity Gold", "girls-13-800m.html"), ("Boys &middot; Varsity Gold", "boys-13-800m.html"),
                ("Boys &middot; Varsity Gold", "boys-13-4x800m.html")])

    def test_another_configs_meet_folder_is_left_alone(self):
        with tempfile.TemporaryDirectory() as d:
            folder = Path(d) / "out" / "900700"
            folder.mkdir(parents=True)
            (folder / ".seedsheet.json").write_text(json.dumps({"config": "/elsewhere/other.config.json",
                                                                "files": ["index.html"]}))
            (folder / "index.html").write_text("theirs")
            out = run_main(d)
            self.assertIn("holds the pages of another config (/elsewhere/other.config.json)", out)
            self.assertIn("its own 'output_dir'", out)
            self.assertEqual((folder / "index.html").read_text(), "theirs")

    def test_a_past_meet_runs_no_ath_seed(self):
        # Checked once from the saved meet: every event without a sheet gets the one reason.
        calls = []
        with tempfile.TemporaryDirectory() as d:
            out = run_main(d, run=fake_ath(meet={**MEET, "date": "2020-01-04", "end_date": "2020-01-05"},
                                           calls=calls))
            self.assertEqual([c[1] for c in calls], ["meet"])
            self.assertIn("ath seed is not run", out)
            rows = index_rows((Path(d) / "out" / "900700" / "index.html").read_text())
            self.assertEqual({r[4] for r in rows}, {"not seeded: the meet is over (it ended on January 5, 2020), and "
                                                    "seed sheets are only for upcoming meets"})

    def test_a_meet_refused_as_past_stops_running_the_rest(self):
        past = ("meet 900700 was on 2026-11-06, in the past (today is 2026-11-07). Seed sheets are only for "
                "upcoming meets; ath does not reconstruct a past seeding.")
        calls = []
        with tempfile.TemporaryDirectory() as d:
            run_main(d, run=fake_ath(seeds={("f", "800m"): ("error", past)}, calls=calls))
            self.assertEqual([c[1] for c in calls], ["meet", "seed"])
            rows = index_rows((Path(d) / "out" / "900700" / "index.html").read_text())
            self.assertEqual({r[4] for r in rows}, {
                "not seeded: meet 900700 was on 2026-11-06, in the past (today is 2026-11-07). Seed sheets are only "
                "for upcoming meets; a past seeding is not reconstructed."})

    def test_an_event_with_no_accepted_entrants_is_not_seeded_saying_so(self):
        empty = {**seeded("m")[0], "entrants": []}
        empty["meet"] = {**empty["meet"], "left_out": empty["meet"]["left_out"][:1]}
        with tempfile.TemporaryDirectory() as d:
            out = run_main(d, run=fake_ath(seeds={("m", "800m"): (json.dumps(empty), None)}),
                           config={"events": [{"gender": "m", "div": "V", "event": "800m"}]})
            self.assertIn("not seeded (no accepted entrants (1 entry scratched or not accepted))", out)

    def test_a_failed_run_keeps_the_saved_sheet_and_records_no_reason(self):
        listed = {"events": [{"gender": "m", "div": "V", "event": "800m"}]}
        with tempfile.TemporaryDirectory() as d:
            run_main(d, config=listed)
            run_main(d, run=fake_ath(seeds={}), config=listed)
            self.assertFalse((Path(d) / "data" / "900700-boys-13-800m.not-seeded.json").exists())
            rows = index_rows((Path(d) / "out" / "900700" / "index.html").read_text())
            self.assertEqual(rows[0][4], "boys-13-800m.html")

    def test_a_reason_is_cleared_once_the_event_is_seeded(self):
        listed = {"events": [{"gender": "m", "div": "V", "event": "800m"}]}
        with tempfile.TemporaryDirectory() as d:
            run_main(d, run=fake_ath(seeds={}), config=listed)
            reason = Path(d) / "data" / "900700-boys-13-800m.not-seeded.json"
            self.assertTrue(reason.exists())
            run_main(d, config=listed)
            self.assertFalse(reason.exists())

    def test_refresh_goes_to_every_ath_run(self):
        calls = []
        with tempfile.TemporaryDirectory() as d:
            run_main(d, args=["--refresh"], run=fake_ath(calls=calls))
        self.assertTrue(calls)
        self.assertTrue(all(c[-1] == "--refresh" for c in calls))


class Index(unittest.TestCase):
    def build_saved(self, d, config=None):
        """A data_dir as a run on PULLED left it: the meet, the two 800m sheets, the boys' 4x800m
        and ath's reasons for the rest, built with config's keys added to the meet config."""
        data = Path(d) / "data"
        data.mkdir()
        (data / "900700.meet.json").write_text(json.dumps({"format": 1, "pulled": PULLED, "meet": MEET}))
        for g, word, event in (("f", "girls", "800m"), ("m", "boys", "800m"), ("m", "boys", "4x800m")):
            seed, csv_text = seeded(g, event)
            (data / f"900700-{word}-13-{event}.json").write_text(
                json.dumps(build.sheet_file(seed, csv_text, PULLED)))
        cfg = seedsheet.load_config(write_config(d))
        cfg["config"] = Path(d)
        for ev in seedsheet.meet_events(cfg, MEET, {}):
            if ev["event"] in REFUSALS:
                (data / f"{ev['stem']}.not-seeded.json").write_text(
                    json.dumps({"reason": REFUSALS[ev["event"]], "pulled": PULLED}))
        run_main(d, args=["--no-fetch"], config=config)
        return Path(d) / "out" / "900700"

    def test_pages_match_the_expected_ones(self):
        with tempfile.TemporaryDirectory() as d:
            folder = self.build_saved(d)
            for name in ("index.html", "boys-13-800m.html", "boys-13-4x800m.html"):
                got = (folder / name).read_text()
                want = EXPECTED / name
                if os.environ.get("SEEDSHEET_BLESS"):
                    want.parent.mkdir(parents=True, exist_ok=True)
                    want.write_text(got)
                self.assertEqual(got, want.read_text(), f"{name} differs; SEEDSHEET_BLESS=1 rewrites it")

    def test_a_theme_file_changes_only_its_tokens_on_every_page(self):
        from report import THEME
        with tempfile.TemporaryDirectory() as d:
            (Path(d) / "themes").mkdir()
            (Path(d) / "themes" / "t.json").write_text(json.dumps({"bg": "#fafafa", "highlight_row": "#e8f0ff",
                                                                   "fonts_url": None}))
            folder = self.build_saved(d, {"theme": "themes/t.json"})
            for name in ("index.html", "boys-13-800m.html", "boys-13-4x800m.html"):
                want = (EXPECTED / name).read_text().replace(f"--bg: {THEME.bg};", "--bg: #fafafa;")
                want = want.replace(THEME.highlight_row, "#e8f0ff")
                want = "".join(line for line in want.splitlines(keepends=True) if "fonts.g" not in line)
                self.assertEqual((folder / name).read_text(), want, name)

    def test_the_index_keeps_out_of_search_names_no_source_and_is_light_only(self):
        from report import THEME
        with tempfile.TemporaryDirectory() as d:
            html = (self.build_saved(d) / "index.html").read_text()
        self.assertIn('<meta name="robots" content="noindex, nofollow, noarchive">', html)
        self.assertIn('<meta name="color-scheme" content="light only">', html)
        self.assertIn(f"--bg: {THEME.bg};", html)
        self.assertNotIn("prefers-color-scheme", html)
        visible = re.sub(r"<[^>]+>", " ", re.sub(r"<(style|script)[^>]*>.*?</\1>", "", html, flags=re.S))
        self.assertNotIn("athletic.net", visible.lower())
        self.assertNotIn(chr(0x2014), html)  # no em dashes
        self.assertIn("<h1>Riverton Winter Classic</h1>", html)
        self.assertIn("January 9, 2099 &middot; Riverton Fieldhouse, Riverton, NY", html)
        # The relay's sheet was pulled a day before the 800m's (each saved report keeps its own as_of).
        self.assertIn("3 of 7 events seeded, on Personal records as of November 6, 2026 to November 7, 2026. 4 are not "
                      "seeded, with why.", html)

    def test_every_color_is_a_token(self):
        import report
        self.assertEqual(report.color_literals(meet_index.__file__), [],
                         "meet_index.py: a color outside report.THEME; make it a token")

    def test_reasons_drop_aths_command_hints_and_never_name_the_source(self):
        # ath's real messages (src/meet.rs, src/seed.rs, src/athletic.rs, src/list.rs), as the page
        # says them: no command or option advice, no internal references, no URL, no site name.
        for said, page in (
                # ath's wording for a relay before relays were seeded, as a saved reason may hold it.
                ("4x800 Relay (4x800m) is a relay, and ath seed does not seed relays yet; `ath meet 900700 --gender m "
                 "--div 13 --event 4x800m` shows its teams and legs",
                 "4x800 Relay (4x800m) is a relay, and relays are not seeded yet"),
                (REFUSALS["sprintmed2248"], f"SMR 1600m (sprintmed2248) {MIXED_PUBLIC}"),
                (REFUSALS["55shuttleh"], f"4x55 Shuttle Hurdles (55shuttleh) {MIXED_PUBLIC} (it is also only run "
                                         f"indoors, and seeding reads outdoor Personal records)"),
                (REFUSALS["Shot"], "Shot Put (Shot) is a field event; only running events are seeded"),
                (REFUSALS["55m"], "55 Meters (55m) is only run indoors, and seeding reads outdoor Personal records, "
                                  "so it has no marks to seed on; indoor seeding comes later"),
                ("no accepted entrants (1 entry scratched or not accepted)",
                 "no accepted entrants (1 entry scratched or not accepted)"),
                ("meet 900700 has no 1mile entries in the Varsity division (yet)",
                 "meet 900700 has no 1mile entries in the Varsity division (yet)"),
                ("meet 900700 was on 2026-01-09, in the past (today is 2026-11-07). Seed sheets are only for upcoming "
                 "meets; ath does not reconstruct a past seeding.",
                 "meet 900700 was on 2026-01-09, in the past (today is 2026-11-07). Seed sheets are only for upcoming "
                 "meets; a past seeding is not reconstructed."),
                ("meet 900700 has no date on Athletic.net", "meet 900700 has no date on the results site"),
                ("Athletic.net answered the event list of meet 900700 with HTTP 500: Server Error",
                 "The results site answered the event list of meet 900700 with HTTP 500: Server Error"),
                ("Athletic.net refused meet 900700 (HTTP 403). Reading a meet's entries may need a signed-in "
                 "session with a paid Athletic.net subscription (ADR 0001), or the meet does not publish its entries.",
                 "The results site refused meet 900700 (HTTP 403). Reading a meet's entries may need a signed-in "
                 "session with a paid subscription, or the meet does not publish its entries."),
                ("Athletic.net returned a non-JSON page (HTTP 200) after 3 attempts. Athletic.net is throttling this "
                 "burst of requests: wait a few minutes and re-run, and consider a lower --concurrency or a higher "
                 "--delay-ms.",
                 "The results site returned a non-JSON page (HTTP 200) after 3 attempts."),
                ("rankings list 5 is in season 2025, not 2026 (see --season)",
                 "rankings list 5 is in season 2025, not 2026"),
                ("could not read the rankings list to start from (42, see --from-list)",
                 "could not read the rankings list to start from (42)"),
                ("the bare `ath --roster ... --event ...` form was removed; use `ath seed --roster ... --event ...` "
                 "instead", "it could not be seeded"),
                ("no Athletic.net browser tab connected to the ath relay", "no browser tab connected to the ath relay"),
                ("the Athletic.net tab running the ath relay stopped answering",
                 "the tab running the ath relay stopped answering"),
                ("the browser tab could not fetch GET https://www.athletic.net/api/v1/x: Failed",
                 "the browser tab could not fetch GET: Failed"),
                ("parsing meet 900700 (see https://www.athletic.net/TrackAndField/meet/900700)", "parsing meet 900700"),
                ("parsing meet 900700, see https://www.athletic.net/TrackAndField/meet/900700.", "parsing meet 900700.")):
            got = meet_index.public(said)
            self.assertEqual(got, page)
            for bad in ("athletic.net", "http://", "https://", "the the ", "no the ", "a the ", "adr 0", "--",
                        "`", "ath seed", " see.", " see "):
                self.assertNotIn(bad, got.lower())

    def test_reasons_and_labels_are_escaped(self):
        ev = {"row_label": "<b>X</b>", "url": None, "reason": "<script>x()</script>", "gender": "m", "division": "V"}
        html = meet_index.html({"name": "M & M"}, [ev], [])
        self.assertIn("&lt;b&gt;X&lt;/b&gt;", html)
        self.assertIn("&lt;script&gt;x()&lt;/script&gt;", html)
        self.assertIn("<h1>M &amp; M</h1>", html)
        self.assertNotIn("<script>", html)

    def test_no_athletes_no_yours_column(self):
        seed, _ = seeded("m")
        ev = {"row_label": "800 Meters", "url": None, "page": "p", "sheet": seed, "pulled": PULLED,
              "gender": "m", "division": "Varsity"}
        self.assertNotIn("Yours", meet_index.html({"name": "M"}, [ev], []))
        self.assertIn("Yours", meet_index.html({"name": "M"}, [ev], [THEO]))

    def test_a_relay_sheet_counts_its_teams_and_the_athletes_running_in_them(self):
        # By kind: a relay's sheet counts its teams and the athletes running in them.
        relay = build.check(build.sheet_file(*seeded("m", "4x800m"), PULLED))
        self.assertEqual(meet_index.counts(relay, {NICO, 900100021, 7}), (4, "team", 2))
        self.assertEqual(meet_index.counts(seeded("m")[0], {THEO}), (4, "athlete", 1))
        # No kind is an individual event's sheet, whatever other keys it has.
        self.assertEqual(meet_index.counts({**seeded("m")[0], "teams": relay["teams"]}, {THEO}), (4, "athlete", 1))

    def test_a_meet_over_more_than_a_day_shows_its_span(self):
        html = meet_index.html({"name": "M", "date": "2099-01-09", "end_date": "2099-01-10"}, [], [])
        self.assertIn('<p class="lede">January 9, 2099 to January 10, 2099</p>', html)
        html = meet_index.html({"name": "M", "date": "2099-01-09", "end_date": "2099-01-09"}, [], [])
        self.assertIn('<p class="lede">January 9, 2099</p>', html)

    def test_a_header_without_a_date_or_place_has_no_empty_lede(self):
        html = meet_index.html({"name": "M"}, [], [])
        self.assertNotIn('class="lede"', html)
        self.assertNotIn("<p></p>", html)

    def test_both_pages_share_their_header_and_table_css(self):
        with tempfile.TemporaryDirectory() as d:
            folder = self.build_saved(d)
            for name in ("index.html", "boys-13-800m.html", "boys-13-4x800m.html"):
                html = (folder / name).read_text()
                for css in build.SHARED_CSS.values():
                    self.assertIn(css, html, name)


@unittest.skipUnless(os.environ.get("ATH_TEST_BIN"), "runs the ath binary ATH_TEST_BIN names (cargo test sets it)")
class AgainstAth(unittest.TestCase):
    """seedsheet with the real ath, on the recorded meets 900700 and 900702 (fixtures_dir): what a
    run does, short of the network."""

    def run_real(self, d, **config):
        path = write_config(d, fixtures_dir=str(FIXTURES), **config)
        out = io.StringIO()
        with mock.patch.dict(os.environ, {"ATH": os.environ["ATH_TEST_BIN"]}), redirect_stdout(out), \
                redirect_stderr(io.StringIO()):
            seedsheet.main(["--config", str(path)])
        return out.getvalue()

    def test_all_of_the_fixture_meet(self):
        with tempfile.TemporaryDirectory() as d:
            self.run_real(d, athletes=[THEO, NICO])
            folder = Path(d) / "out" / "900700"
            rows = index_rows((folder / "index.html").read_text())
            # Both 800m and the 4x800m relay are seeded; the boys' relay counts its 4 teams and Nico
            # running leg 2 for Pinecrest Academy A.
            self.assertEqual([(r[1], r[2], r[3], r[4]) for r in rows[:3]], [
                ("800 Meters", "1 athlete", None, "girls-13-800m.html"),
                ("800 Meters", "4 athletes", "1", "boys-13-800m.html"),
                ("4x800 Relay", "4 teams", "1", "boys-13-4x800m.html")])
            relay = (folder / "boys-13-4x800m.html").read_text()
            self.assertIn('<a class="back" href="index.html">&larr; All events</a>', relay)
            self.assertIn("4 teams, seeded on the sum of their four runners", relay)
            self.assertEqual((folder / "boys-13-4x800m.csv").read_text().splitlines()[0],
                             (TESTDATA / "ath-seed-relay.csv").read_text().splitlines()[0])
            # Medley and shuttle relays, field and indoor-only events are not seeded, with why.
            not_seeded = {r[1]: r[4] for r in rows if r[4].startswith("not seeded")}
            self.assertEqual(set(not_seeded), {"Shot Put", "SMR 1600m", "4x55 Shuttle Hurdles", "55 Meters"})
            self.assertIn("field event", not_seeded["Shot Put"])
            self.assertIn("medley or shuttle relay", not_seeded["SMR 1600m"])
            self.assertIn("only run indoors", not_seeded["55 Meters"])

    def test_the_4xmile_with_estimated_legs(self):
        with tempfile.TemporaryDirectory() as d:
            out = self.run_real(d, meet=900702)
            folder = Path(d) / "out" / "900702"
            rows = index_rows((folder / "index.html").read_text())
            self.assertEqual([(r[0], r[1], r[4]) for r in rows],
                             [("Girls &middot; Varsity", "4xMile Relay", "girls-21-4xmile.html")])
            relay = json.loads((TESTDATA / "ath-seed-4xmile.json").read_text())
            self.assertEqual(rows[0][2], f"{len(relay['teams'])} teams")
            page = (folder / "girls-21-4xmile.html").read_text()
            self.assertIn("marked as an estimate", page)
            self.assertIn("Wrote", out)

    def test_listed_events_resolved_by_the_real_ath(self):
        # ath finds each listed event its own way (a flat race's name, a division in any case); one
        # with no entries in the division is listed with ath's reason and never seeded.
        listed = [{"gender": "m", "div": "V", "event": "800 Meters"},
                  {"gender": "m", "div": "varsity", "event": "4x800m", "label": "Boys 4x8"},
                  {"gender": "m", "div": "13", "event": "1mile"}]
        with tempfile.TemporaryDirectory() as d:
            self.run_real(d, events=listed)
            rows = index_rows((Path(d) / "out" / "900700" / "index.html").read_text())
            self.assertEqual([(r[1], r[2], r[4]) for r in rows], [
                ("800 Meters", "4 athletes", "boys-13-800m.html"),
                ("Boys 4x8", "4 teams", "boys-13-4x800m.html"),
                ("1mile", "", "not seeded: meet 900700 has no 1mile entries in the Varsity division (yet); its "
                            "boys&#x27; events with entries there are 800m, 4x800m, Shot, sprintmed2248, 55shuttleh")])


if __name__ == "__main__":
    sys.exit(unittest.main())
