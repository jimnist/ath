"""seedsheet: a meet's seed sheets, one shareable page per event.

    python3 tools/seedsheet/seedsheet.py [--config NAME|PATH] [--no-fetch] [--refresh]

Reads local/seedsheet/seedsheet.config.json (copy an example config), runs `ath seed` for each
event in it, saves each report, then writes the pages. A config is one of two kinds, told apart by
its "meet":
- a meet id (a number): meet mode, seeding the meet's own entries (`ath seed --meet`) for the
  events listed, or "all" of them; writes <output_dir>/<meet id>/index.html, the meet's index
  page, and a page plus .csv per event beside it (<gender>-<division id>-<event code>).
- the meet's name (text): roster mode, one roster CSV per event (`ath seed --roster`); writes
  <output_dir>/<meet>-<label>.html plus .csv.
See README.md.
"""
import argparse
import json
import os
import sys
from datetime import date
from pathlib import Path

import build  # puts tools/ on sys.path for report
import meet_index
from report import (RELAY_HINT, beside, check_moved, config_path, config_theme, find_ath, older_place,
                    pretty_date, run_ath_report, write_json)

HERE = Path(__file__).parent
EXAMPLES = "seedsheet.meet.config.example.json (meet mode) or seedsheet.config.example.json (rosters)"

# The saved meet format this build reads (`ath meet`'s report, with the day it was pulled), and the
# `ath meet` JSON schema inside it.
MEET_FORMAT = 1
ATH_MEET_SCHEMA = 1


_int = build._int  # a number that is not a bool, as a saved sheet's ids are checked


def _text(x):
    return isinstance(x, str) and x.strip() != ""


# ---------------------------------------------------------------- config

def no_config_yet(path):
    """Exit saying how to start a config at path, from either example and when to use each."""
    sys.exit(f"No config at {path}. Make its folder (mkdir -p {path.parent}), copy one of these there as "
             f"{path.name}, and fill it in:\n"
             f"  {HERE / 'seedsheet.meet.config.example.json'}\n"
             f"    meet mode: a meet's id; seeds the meet's own entries, for every event or the ones you list, "
             f"with an index page for the meet.\n"
             f"  {HERE / 'seedsheet.config.example.json'}\n"
             f"    roster mode: the meet's name and a roster CSV of your own per event (no entries on the "
             f"site yet, or a roster that differs from them).")


def load_config(path):
    if not path.exists():
        no_config_yet(path)
    try:
        cfg = json.loads(path.read_text())
    except ValueError as e:
        sys.exit(f"{path}: not valid JSON ({e})")
    if not isinstance(cfg, dict):
        sys.exit(f"{path}: a config is a JSON object")
    if _int(cfg.get("meet")):
        cfg["mode"] = "meet"
    elif _text(cfg.get("meet")):
        cfg["mode"] = "roster"
    else:
        sys.exit(f"{path}: 'meet' is required: the meet's id (a number) to seed its entries, or its "
                 f"name (text) to seed rosters; see {EXAMPLES}")
    if not isinstance(cfg.get("output_dir"), str):
        sys.exit(f"{path}: 'output_dir' is required (str)")
    athletes = cfg.get("athletes", [])
    if not isinstance(athletes, list) or not all(_int(a) for a in athletes):
        sys.exit(f"{path}: 'athletes' must be a list of athlete ids (numbers)")
    for key in ("data_dir", "fixtures_dir"):
        if key in cfg and not _text(cfg[key]):
            sys.exit(f"{path}: '{key}' must be a folder (text)")
    (load_meet_events if cfg["mode"] == "meet" else load_roster_events)(path, cfg)
    cfg["athletes"] = athletes
    cfg["output_dir"] = beside(path, cfg["output_dir"])
    cfg["data_dir"] = beside(path, cfg.get("data_dir", "data"))
    if cfg.get("fixtures_dir"):
        cfg["fixtures_dir"] = beside(path, cfg["fixtures_dir"])
    # The pages' look: the house theme, or a theme file's tokens over it (report.load_theme).
    cfg["theme"] = config_theme(path, cfg)
    return cfg


def load_roster_events(path, cfg):
    """Roster mode: each event has its label, event and roster; the meet's date is optional."""
    if cfg.get("events") == "all":
        sys.exit(f"{path}: 'events': \"all\" takes the meet's entries, so 'meet' must be its id (a number), "
                 f"not its name")
    if not isinstance(cfg.get("events"), list):
        sys.exit(f"{path}: 'events' is required (list)")
    if not cfg["events"]:
        sys.exit(f"{path}: 'events' lists no events")
    meet_date = cfg.get("meet_date")
    if meet_date is not None:
        try:
            date.fromisoformat(meet_date)
        except (TypeError, ValueError):
            sys.exit(f"{path}: 'meet_date' must be YYYY-MM-DD, not {meet_date!r}")
    for i, ev in enumerate(cfg["events"], 1):
        if not isinstance(ev, dict):
            sys.exit(f"{path}: event {i} is not an object")
        if "roster" not in ev and "gender" in ev and cfg["meet"].strip().isdigit():
            sys.exit(f"{path}: event {i} has no roster; to seed meet {cfg['meet']}'s entries, write its "
                     f"id as a number (\"meet\": {cfg['meet'].strip()})")
        for key in ("label", "event", "roster"):
            if not isinstance(ev.get(key), str) or not ev[key]:
                sys.exit(f"{path}: event {i} needs '{key}' (text)")
        ev["roster"] = beside(path, ev["roster"])
    names = [build.slug(cfg["meet"], ev["label"]) for ev in cfg["events"]]
    dupes = sorted({n for n in names if names.count(n) > 1})
    if dupes:
        sys.exit(f"{path}: two events would write the same page ({', '.join(dupes)}); give them different labels")


def load_meet_events(path, cfg):
    """Meet mode: "all", or events by gender, division and event, each with an optional label.
    The meet gives the date, so there is no meet_date."""
    if cfg["meet"] <= 0:
        sys.exit(f"{path}: 'meet' must be a meet id (a positive number), not {cfg['meet']}")
    if "meet_date" in cfg:
        sys.exit(f"{path}: 'meet_date' is not taken with a meet id: the date comes from the meet")
    events = cfg.get("events")
    if events == "all":
        return
    if not isinstance(events, list):
        sys.exit(f"{path}: 'events' is required: \"all\", or a list of events "
                 f"({{\"gender\": \"m\", \"div\": \"V\", \"event\": \"800m\"}})")
    if not events:
        sys.exit(f"{path}: 'events' lists no events (\"all\" takes every event with entries)")
    for i, ev in enumerate(events, 1):
        if not isinstance(ev, dict):
            sys.exit(f"{path}: event {i} is not an object")
        if "roster" in ev:
            sys.exit(f"{path}: event {i} has a roster, but 'meet' is a meet id, which seeds the meet's own "
                     f"entries; drop 'roster', or give 'meet' as the meet's name to seed rosters")
        if ev.get("gender") not in meet_index.GENDERS:
            sys.exit(f"{path}: event {i} needs 'gender': \"f\" (girls) or \"m\" (boys), not {ev.get('gender')!r}")
        if not (_int(ev.get("div")) or _text(ev.get("div"))):
            sys.exit(f"{path}: event {i} needs 'div': the division's id, name or short name (13, \"Varsity\" or \"V\")")
        if not _text(ev.get("event")):
            sys.exit(f"{path}: event {i} needs 'event': the event's code or name (\"800m\" or \"800 Meters\")")
        if ev["event"].strip().isdigit():  # names no event: ath seed refuses it too
            sys.exit(f"{path}: event {i}'s 'event' {ev['event']!r} is a bare number; did you mean "
                     f"\"{ev['event'].strip()}m\"?")
        if "label" in ev and not _text(ev["label"]):
            sys.exit(f"{path}: event {i}'s 'label' must be text")


# ---------------------------------------------------------------- meet mode: the meet and its events

def event_entry(cfg, row, label=None):
    """An event of the meet (an `ath meet` events row, or one `ath meet --resolve` found) as
    seedsheet goes through it: what ath seed is given (gender, div, event), its division and name,
    its entries page, its labels (label: its page's and the run's, `Boys Varsity 800 Meters` unless
    the config gives one; row_label: its row on the index, the event's name unless the config gives
    a label), and its file name (name, the pages') and its saved files' (stem), both made of its
    gender, division id and code, never its names, so a renamed division keeps its sheets."""
    word = meet_index.gender_word(row["gender"])
    name = build.slug(word, str(row["division_id"]), row["event_short"])
    return {
        "gender": row["gender"], "div": str(row["division_id"]), "event": row["event_short"],
        "division": row["division"], "event_name": row["event"], "url": row["url"],
        "label": label or f"{word} {row['division']} {row['event']}", "row_label": label or row["event"],
        "name": name, "stem": f"{cfg['meet']}-{name}",
        "key": (row["gender"], row["division_id"], row["event_short"]),
    }


def unfound_entry(spec, reason):
    """A listed event ath did not find in the meet (`ath meet --resolve` said why): it is not
    seeded, and has no file of its own."""
    word = meet_index.gender_word(spec["gender"])
    return {
        "gender": spec["gender"], "div": str(spec["div"]), "event": spec["event"], "division": str(spec["div"]),
        "event_name": spec["event"], "url": None, "label": spec.get("label") or f"{word} {spec['div']} {spec['event']}",
        "row_label": spec.get("label") or spec["event"], "name": None, "stem": None, "key": None,
        "unfound": reason,
    }


def spec_key(spec):
    """A listed event as the config gives it, as a key for its saved resolution."""
    return json.dumps([spec["gender"], str(spec["div"]), spec["event"]])


def meet_events(cfg, meet, resolved):
    """The events seedsheet goes through for a meet config, in the index's order. "all" is every
    event `ath meet` lists (those with entries), in its order. A listed event is the one
    `ath meet --resolve` found for it (resolved: {spec_key: {"row": ...} or {"reason": ...}}),
    ordered by gender (girls first), then division (in the meet's order), then as listed; one it
    did not find, or that was not resolved yet, is listed with why."""
    if cfg["events"] == "all":
        return [event_entry(cfg, row) for row in meet["events"]]
    events = []
    for spec in cfg["events"]:
        found = resolved.get(spec_key(spec)) or {"reason": "not looked up yet: run seedsheet without --no-fetch"}
        events.append(event_entry(cfg, found["row"], spec.get("label")) if "row" in found
                      else unfound_entry(spec, found["reason"]))
    divisions = [str(d["id"]) for d in meet.get("divisions") or []]
    order = list(meet_index.GENDERS)

    def group(ev):
        div = divisions.index(ev["div"]) if ev["key"] and ev["div"] in divisions else len(divisions)
        return order.index(ev["gender"]), div
    events.sort(key=group)
    keys = [ev["key"] for ev in events if ev["key"]]
    dupes = sorted({ev["name"] for ev in events if ev["key"] and keys.count(ev["key"]) > 1})
    if dupes:
        sys.exit(f"{cfg['config']}: two listed events are the same event of meet {cfg['meet']} "
                 f"({', '.join(dupes)}); list each once")
    return events


def resolve_events(cfg, saved_meet):
    """Run `ath meet <id> --gender --div --event --resolve` for each listed event (ath finds it as
    `ath seed --meet` would, from the meet's cached event list, reading no entries) and save what
    it found, or why not, with the saved meet. Returns the resolutions. Raises Unreachable or
    MeetRefused as collect does; what was found before that is saved."""
    resolved = dict(saved_meet.get("resolved") or {})
    try:
        for spec in cfg["events"]:
            cmd = [find_ath(), "meet", str(cfg["meet"]), "--gender", spec["gender"], "--div", str(spec["div"]),
                   "--event", spec["event"], "--resolve", "--json"] + ath_options(cfg, False)
            label = f"{spec['gender']} {spec['div']} {spec['event']}"
            print(f"\n{label}: {' '.join(cmd[1:])}", flush=True)
            errors = []
            report = run_ath_report(cmd, label, errors)
            stop_if_run_wide(errors)
            row = (report or {}).get("resolved") if isinstance(report, dict) else None
            if row_ok(row):
                resolved[spec_key(spec)] = {"row": row}
                print(f"  {label}: {row['event']} ({row['event_short']}), {row['division']} (division "
                      f"{row['division_id']})")
            else:
                why = errors[0].reason if errors else "ath meet --resolve gave no event"
                resolved[spec_key(spec)] = {"reason": why}
                print(f"  {label}: not found ({why})")
    finally:
        saved_meet["resolved"] = resolved
        write_json(meet_path(cfg), saved_meet)
    return resolved


ROW_TYPES = {"gender": str, "division_id": int, "division": str, "event": str, "event_short": str, "url": str}


def row_ok(row):
    """Whether an `ath meet` events row (or resolved event) has what seedsheet reads."""
    return isinstance(row, dict) and all(isinstance(row.get(k), t) for k, t in ROW_TYPES.items())


def meet_path(cfg):
    return cfg["data_dir"] / f"{cfg['meet']}.meet.json"


def meet_folder(cfg):
    """The meet's output folder, named by its id alone: its index page and event pages."""
    return cfg["output_dir"] / str(cfg["meet"])


def check_meet(saved, meet_id):
    """`ath meet`'s report inside a saved meet, or None and why not."""
    if not isinstance(saved, dict) or saved.get("format") != MEET_FORMAT:
        return None, "is not a saved meet"
    m = saved.get("meet")
    if not isinstance(m, dict) or m.get("schema") != ATH_MEET_SCHEMA:
        got = m.get("schema") if isinstance(m, dict) else None
        return None, f"holds `ath meet` JSON schema {got}; this build reads schema {ATH_MEET_SCHEMA}"
    if m.get("id") != meet_id:
        return None, f"is meet {m.get('id')}, not {meet_id}"
    if not _text(m.get("name")) or not isinstance(m.get("events"), list) or not isinstance(m.get("divisions"), list):
        return None, "`ath meet` report has no name, events or divisions"
    for i, row in enumerate(m["events"], 1):
        if not row_ok(row):
            return None, f"`ath meet` report's event {i} is not an event row"
    resolved = saved.get("resolved", {})
    if not isinstance(resolved, dict) or not all(
            isinstance(v, dict) and (row_ok(v.get("row")) or _text(v.get("reason"))) for v in resolved.values()):
        return None, "has resolved events this build cannot read"
    return m, None


def collect_meet(cfg, refresh, today):
    """Run `ath meet <id>` and save its report (keeping the listed events resolved before).
    Returns the saved meet, or None (the meet already saved, if any, stays as it was). Raises
    Unreachable or MeetRefused as collect does."""
    cmd = [find_ath(), "meet", str(cfg["meet"]), "--json"] + ath_options(cfg, refresh)
    label = f"meet {cfg['meet']}"
    print(f"\n{label}: {' '.join(cmd[1:])}", flush=True)
    errors = []
    report = run_ath_report(cmd, label, errors)
    if report is None:
        stop_if_run_wide(errors)
        return None
    saved = {"format": MEET_FORMAT, "pulled": today.isoformat(), "meet": report}
    _, why = check_meet(saved, cfg["meet"])
    if why:
        print(f"  {label}: not saved, the report {why}")
        return None
    before = load_meet(cfg, quiet=True)
    if before and before.get("resolved"):
        saved["resolved"] = before["resolved"]
    write_json(meet_path(cfg), saved)
    n = len(report["events"])
    print(f"  {label}: {report['name']}, {build.plural(n, 'event')} with entries")
    return saved


def load_meet(cfg, quiet=False):
    """The saved meet, or None (saying why, unless quiet)."""
    p = meet_path(cfg)
    if not p.exists():
        return None
    try:
        saved = json.loads(p.read_text())
    except ValueError as e:
        if not quiet:
            print(f"  {p} is not valid JSON ({e})")
        return None
    _, why = check_meet(saved, cfg["meet"])
    if why:
        if not quiet:
            print(f"  {p} {why}")
        return None
    return saved


def not_seeded_path(cfg, ev):
    return cfg["data_dir"] / f"{name_of(cfg, ev)}.not-seeded.json"


def save_not_seeded(cfg, ev, why, today):
    write_json(not_seeded_path(cfg, ev), {"reason": why, "pulled": today.isoformat()})


def saved_reason(cfg, ev):
    """Why an event was not seeded the last time seedsheet ran it, or None."""
    try:
        reason = json.loads(not_seeded_path(cfg, ev).read_text()).get("reason")
    except (OSError, ValueError, AttributeError):
        return None
    return reason if _text(reason) else None


# ---------------------------------------------------------------- saved sheets

def name_of(cfg, ev):
    """An event's saved files' name: meet mode's start with the meet id."""
    return ev.get("stem") or build.slug(cfg["meet"], ev["label"])


def data_path(cfg, ev):
    return cfg["data_dir"] / f"{name_of(cfg, ev)}.json"


def check_moved_data(cfg, old=None):
    """Stop when this config's data_dir has none of its saved sheets but the old default place (the
    main checkout's tools/seedsheet/data) does, printing the move, rather than fetch them again
    (a sheet saved before the meet cannot be fetched after it)."""
    old = old or older_place("seedsheet", "data")
    names = [p(cfg, ev).name for ev in cfg["events"] for p in (data_path, ath_csv_path)]
    check_moved(cfg["data_dir"], old, names)


def meta(cfg, ev):
    return {"meet": cfg["meet"], "label": ev["label"], "athletes": cfg["athletes"]}


# ---------------------------------------------------------------- collecting

def ath_csv_path(cfg, ev):
    """Where `ath seed --out` writes this event's seed sheet CSV."""
    return cfg["data_dir"] / f"{name_of(cfg, ev)}.ath.csv"


class Unreachable(Exception):
    """ath could not reach Athletic.net (an AthFailure with transport set): the run stops, since
    every event after it would fail the same way."""


class MeetRefused(Exception):
    """Athletic.net refused the meet's event list or entries (an AthFailure with meet_wide set):
    no more ath runs for the meet, and every event with no sheet saved before is not seeded, for
    this one reason."""


def stop_if_run_wide(errors):
    """Raise when an ath run failed in a way every later run would too."""
    for failure in errors:
        if failure.transport:
            raise Unreachable(failure.reason)
        if failure.meet_wide:
            raise MeetRefused(failure.reason)


def ath_options(cfg, refresh):
    """The options every ath run of this config takes: recorded responses (fixtures_dir), and
    --refresh. ath has no narrower refresh, so in meet mode each `ath seed --refresh` also rereads
    the meet and its event list (two small requests per event) along with the event's entries and
    its entrants' profiles, which are what --refresh is for."""
    out = ["--fixtures-dir", str(cfg["fixtures_dir"])] if cfg.get("fixtures_dir") else []
    return out + (["--refresh"] if refresh else [])


def ath_command(cfg, ev, refresh):
    if cfg["mode"] == "meet":
        cmd = [find_ath(), "seed", "--meet", str(cfg["meet"]), "--gender", ev["gender"], "--div", ev["div"],
               "--event", ev["event"]]
    else:
        cmd = [find_ath(), "seed", "--roster", str(ev["roster"]), "--event", ev["event"]]
    cmd += ["--out", str(ath_csv_path(cfg, ev)), "--json"]
    if cfg.get("meet_date"):
        cmd += ["--meet-date", cfg["meet_date"]]
    return cmd + ath_options(cfg, refresh)


def collect(cfg, ev, refresh, today):
    """Run `ath seed` for one event and save its report with the CSV that run wrote. Returns
    (the saved sheet, None, False), or (None, why, current) when ath fails, refuses the event or
    leaves no usable report and CSV. current is whether why is ath's answer about the event as it
    is now (in meet mode, nobody accepted on it), which replaces a sheet saved before; otherwise
    that sheet stays as it was. Raises Unreachable when ath could not reach Athletic.net,
    MeetRefused when Athletic.net refused the meet, and lets an interrupt through."""
    cfg["data_dir"].mkdir(parents=True, exist_ok=True)
    out = ath_csv_path(cfg, ev)
    out.unlink(missing_ok=True)  # so a CSV read below is this run's, never a stale one
    cmd = ath_command(cfg, ev, refresh)
    print(f"\n{ev['label']}: {' '.join(cmd[1:])}", flush=True)
    errors = []
    report = run_ath_report(cmd, ev["label"], errors)
    if report is None:
        stop_if_run_wide(errors)
        return None, errors[0].reason if errors else "ath seed gave no report", False
    empty = nobody_on(report, cfg["mode"])
    if empty:
        print(f"  {ev['label']}: not saved, {empty[1]}")
        return None, empty[0], cfg["mode"] == "meet"
    try:
        csv_text = out.read_bytes().decode("utf-8")
    except (OSError, UnicodeDecodeError) as e:
        print(f"  {ev['label']}: not saved, ath seed left no readable CSV at {out} ({e})")
        return None, "ath seed wrote no seed sheet CSV", False
    saved = build.sheet_file(report, csv_text, today.isoformat())
    try:
        s = build.check(saved)
    except build.SheetError as e:
        print(f"  {ev['label']}: not saved, the sheet {e}")
        return None, f"the sheet {e}", False
    write_json(data_path(cfg, ev), saved)
    left_out = len(s.get("skipped") or []) + len(s.get("unresolved") or [])
    print(f"  {ev['label']}: saved {data_path(cfg, ev)} ({build.counts(s)}"
          + (f", {left_out} not on the sheet" if left_out else "") + ")")
    return saved, None, False


def left_out_note(report):
    left = (report.get("meet") or {}).get("left_out") or []
    entries = "1 entry" if len(left) == 1 else f"{len(left)} entries"
    return f" ({entries} scratched or not accepted)" if left else ""


def nobody_on(report, mode):
    """Why an `ath seed` report has nobody to put on a page, as (the reason kept, what the run
    says), or None. By the report's kind: a relay's sheet has teams, any other (no kind) entrants;
    only a roster's sheet can have nobody because no roster name resolved."""
    if not isinstance(report, dict):
        return None
    if build.is_relay(report):
        if report.get("teams") != []:
            return None
        why = "no accepted teams" + left_out_note(report)
        return why, why
    if report.get("entrants") != []:
        return None
    if mode == "meet":
        why = "no accepted entrants" + left_out_note(report)
        return why, why
    names = "; ".join(f"{u.get('name')}: {u.get('note') or u.get('reason')}" for u in report.get("unresolved") or [])
    return ("nobody on the roster could be seeded",
            "nobody on the roster could be seeded" + (f" ({names})" if names else " (the roster is empty)"))


# An event's reason on the index when its saved sheet cannot be read (the run says why).
UNREADABLE = "its saved seed sheet could not be read"


def read_saved(cfg, ev):
    """The saved sheet for an event and None, or None and why it cannot be read (saying so; None
    when there is no saved sheet)."""
    p = data_path(cfg, ev)
    if not p.exists():
        return None, None
    try:
        saved = json.loads(p.read_text())
    except ValueError as e:
        print(f"  {ev['label']}: {p} is not valid JSON ({e}); left out")
        return None, UNREADABLE
    try:
        build.check(saved)  # raises only SheetError, whatever the file holds
    except build.SheetError as e:
        print(f"  {ev['label']}: {p} {e}; left out")
        return None, UNREADABLE
    return saved, None


def load_saved(cfg, ev):
    """The saved sheet for an event, or None (saying why) when there is none this build reads."""
    return read_saved(cfg, ev)[0]


def built_line(cfg, ev, s):
    """The run's line for a page built: its size and the configured Athletes on it."""
    mine = build.athlete_lines(s, cfg["athletes"])
    return f"  {ev['label']}: {build.size(s)}" + (f"; {', '.join(mine)}" if mine else "")


# ---------------------------------------------------------------- main

def stop_message(e):
    """What a run says when it stops fetching: ath could not reach Athletic.net, or Ctrl-C."""
    if isinstance(e, Unreachable):
        return (f"Stopped: ath could not reach Athletic.net ({e}). The sheets and reasons saved before stay as "
                f"they were. Reconnect (see ath's message above) and run again, or run with --no-fetch to build "
                f"from what is saved.")
    if isinstance(e, MeetRefused):
        return f"Stopped: {e}"
    return "Interrupted. The sheets and reasons saved before stay as they were."


def kept_line(ev, saved):
    """The run's line for an event whose `ath seed` gave nothing new, when a sheet saved before
    stands."""
    return f"  {ev['label']}: kept the sheet saved on {pretty_date(saved['pulled'])}"


def run_rosters(cfg, args, today):
    """Roster mode: one page per event, from its roster. Ctrl-C during an event's `ath seed` skips
    that event; when ath cannot reach Athletic.net, the rest are built from their saved sheets."""
    check_moved_data(cfg)
    built = []
    fetching, stopped = not args.no_fetch, None
    for ev in cfg["events"]:
        saved, tried = None, False
        if fetching:
            tried = True
            try:
                saved = collect(cfg, ev, args.refresh, today)[0]
            except KeyboardInterrupt:
                print(f"\n  {ev['label']}: skipped (Ctrl-C)")
            except (Unreachable, MeetRefused) as e:
                stopped, fetching = stop_message(e), False
                print(f"\n{stopped}\nThe rest are built from their saved sheets.")
        if not saved:
            saved = load_saved(cfg, ev)
            if saved and tried:
                print(kept_line(ev, saved))
        if not saved:
            print(f"  {ev['label']}: no saved sheet yet, so no page")
            continue
        path, s = build.build(saved, meta(cfg, ev), cfg["output_dir"], cfg["theme"])
        built.append(path)
        print(built_line(cfg, ev, s))
        if ev.get("artifact_url"):
            print(f"  This config's shared page: {ev['artifact_url']} (republish it there only if you share this page)")
    if not built:
        sys.exit("No saved sheets yet, so nothing to build." + (f"\n{stopped}" if stopped else ""))
    print()
    for path in built:
        print(f"Wrote {path} (+ .csv)")
    if stopped:
        sys.exit(f"\n{stopped}")


# How ath says a meet is over (src/seed.rs PAST_MEET): every event of it is refused alike.
PAST_MEET = "Seed sheets are only for upcoming meets"


def past_meet(meet, today):
    """Why the meet cannot be seeded any more, from the saved `ath meet` report (its last day, or
    its date, before today), or None."""
    end = meet.get("end_date") or meet.get("date")
    if isinstance(end, str) and build._iso(end) and end < today.isoformat():
        return f"the meet is over (it ended on {pretty_date(end)}), and seed sheets are only for upcoming meets"
    return None


def set_aside(path, why):
    """Move a saved sheet out of the way (kept beside it, never read again): <name>.<why>."""
    if path.exists():
        os.replace(path, path.with_name(f"{path.name}.{why}"))
        print(f"  moved {path.name} aside as {path.name}.{why}")


def run_meet(cfg, args, today):
    """Meet mode: the meet's index page, and a page per event seeded. When ath cannot reach
    Athletic.net, or on Ctrl-C, it stops running ath, builds the index and pages from what is
    saved, and exits saying so. When Athletic.net refuses the meet, or the meet is over, no more
    ath runs: every event with no sheet saved before is not seeded, for that one reason."""
    folder = meet_folder(cfg)
    manifest = Manifest(folder, cfg["config"])
    fetching, stopped, meet_reason = not args.no_fetch, None, None

    def stop(e):
        nonlocal stopped, fetching, meet_reason
        fetching = False
        if isinstance(e, MeetRefused):
            meet_reason = str(e)
            print(f"\n  meet {cfg['meet']}: refused ({e}); no more ath runs for it, and every event without a "
                  f"saved sheet is listed with that reason.")
        else:
            stopped = stop_message(e)
            print(f"\n{stopped}\nThe rest of the meet is built from what is saved.")

    saved_meet = None
    if fetching:
        try:
            saved_meet = collect_meet(cfg, args.refresh, today)
        except (Unreachable, MeetRefused, KeyboardInterrupt) as e:
            stop(e)
    saved_meet = saved_meet or load_meet(cfg)
    if not saved_meet:
        sys.exit(stopped or (f"Meet {cfg['meet']}: {meet_reason}" if meet_reason else
                             f"No saved meet {cfg['meet']} yet, so nothing to build"
                             + (": run seedsheet without --no-fetch." if args.no_fetch else ".")))
    meet = saved_meet["meet"]
    resolved = saved_meet.get("resolved") or {}
    if fetching and cfg["events"] != "all":
        try:
            resolved = resolve_events(cfg, saved_meet)
        except (Unreachable, MeetRefused, KeyboardInterrupt) as e:
            stop(e)
            resolved = saved_meet.get("resolved") or {}
    events = meet_events(cfg, meet, resolved)
    if not events:
        print(f"\nMeet {cfg['meet']} ({meet['name']}) has no events with entries yet (`ath meet {cfg['meet']}` "
              f"lists none), so there is nothing to seed; the index page says so.")
    past = past_meet(meet, today) if fetching else None
    if past:
        print(f"\nMeet {cfg['meet']}: {past}, so ath seed is not run; pages are built from saved sheets.")
        meet_reason, fetching = past, False
    rows = []
    for ev in events:
        if ev.get("unfound"):
            print(f"  {ev['label']}: not seeded ({ev['unfound']})")
            rows.append({**ev, "reason": ev["unfound"]})
            continue
        saved, why, current, tried = None, meet_reason, False, False
        if fetching:
            tried = True
            try:
                saved, why, current = collect(cfg, ev, args.refresh, today)
            except (Unreachable, MeetRefused, KeyboardInterrupt) as e:
                stop(e)
                why = meet_reason
            if why and PAST_MEET in why:  # the meet ended while the run went on: the rest are refused too
                meet_reason, fetching = why, False
        if saved:
            not_seeded_path(cfg, ev).unlink(missing_ok=True)
        else:
            if current:  # ath's answer now: a sheet saved before no longer stands
                set_aside(data_path(cfg, ev), "old")
            saved, unreadable = read_saved(cfg, ev)
            if unreadable:
                set_aside(data_path(cfg, ev), "unreadable")
            why = why or unreadable
            if not saved and why:
                save_not_seeded(cfg, ev, why, today)
            if saved and tried:
                print(kept_line(ev, saved))
        if not saved:
            reason = why or saved_reason(cfg, ev) or "not collected yet"
            print(f"  {ev['label']}: not seeded ({reason})")
            rows.append({**ev, "reason": reason})
            continue
        page_meta = {"meet": meet["name"], "label": ev["label"], "athletes": cfg["athletes"], "name": ev["name"],
                     "index": meet_index.INDEX}
        _, s = build.build(saved, page_meta, folder, cfg["theme"])
        print(built_line(cfg, ev, s))
        rows.append({**ev, "page": ev["name"], "sheet": s, "pulled": saved["pulled"]})
    index = meet_index.write(meet, rows, cfg["athletes"], folder, cfg["theme"])
    wrote = [meet_index.INDEX] + [f"{r['page']}.{ext}" for r in rows if r.get("sheet") is not None
                                  for ext in ("html", "csv")]
    for gone in manifest.replace(wrote):
        print(f"  removed {gone} (no longer on the index)")
    seeded = sum(r.get("sheet") is not None for r in rows)
    print(f"\nWrote {index}: {seeded} of {build.plural(len(rows), 'event')} seeded, each a page (+ .csv) beside it")
    if stopped:
        sys.exit(f"\n{stopped} The index and pages are built from what is saved.")


class Manifest:
    """The files a config wrote in a meet's folder (.seedsheet.json there): the folder is that
    config's, so its index and pages never mix with another config's, and a page this config wrote
    that its index no longer links (an event dropped from the config, or no longer seeded) is
    removed. Files it did not write are never touched."""

    NAME = ".seedsheet.json"

    def __init__(self, folder, config):
        self.folder, self.path, self.config = folder, folder / self.NAME, str(config)
        self.files = []
        if self.path.exists():
            try:
                data = json.loads(self.path.read_text())
                owner, files = data["config"], data["files"]
            except (OSError, ValueError, KeyError, TypeError):
                sys.exit(f"{self.path} is not a seedsheet manifest this build reads; move it away and run again")
            if owner != self.config:
                sys.exit(f"{folder} holds the pages of another config ({owner}). Give {self.config} its own "
                         f"'output_dir', or remove {self.path} to let it take the folder over.")
            self.files = [f for f in files if isinstance(f, str) and "/" not in f and f != self.NAME]

    def replace(self, wrote):
        """Record wrote as this config's files, and remove the ones it wrote before but not now.
        Returns the paths removed."""
        gone = [self.folder / f for f in self.files if f not in wrote and (self.folder / f).is_file()]
        for p in gone:
            p.unlink()
        self.folder.mkdir(parents=True, exist_ok=True)
        write_json(self.path, {"config": self.config, "files": sorted(set(wrote))})
        return gone


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--config", help="a config file (with a /, e.g. ./x.config.json), or a config's name "
                                          "in local/seedsheet/ (default: seedsheet.config.json)")
    parser.add_argument("--no-fetch", action="store_true", help="build from saved sheets only")
    parser.add_argument("--refresh", action="store_true", help="have ath refetch instead of reading its cache")
    args = parser.parse_args(argv)

    config = config_path("seedsheet", args.config).resolve()
    print(f"Config: {config}", file=sys.stderr)
    cfg = load_config(config)
    cfg["config"] = config
    today = date.today()
    if not args.no_fetch:
        print(RELAY_HINT)
    try:
        (run_meet if cfg["mode"] == "meet" else run_rosters)(cfg, args, today)
    except KeyboardInterrupt:  # outside an ath run (those are handled where they happen)
        sys.exit(f"\n{stop_message(KeyboardInterrupt())}")


if __name__ == "__main__":
    main()
