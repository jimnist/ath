"""rankmap: where an athlete's season best would rank in every state, as a map page.

    python3 tools/rankmap/rankmap.py [--config NAME|PATH] [--no-fetch] [--refresh SEASON ...]

Reads local/rankmap/rankmap.config.json (copy rankmap.config.example.json), collects any seasons
that need data with `ath rank` (all states, every grade group), then writes
<output_dir>/<athlete_id>-<event>.html plus .md and .csv. See README.md.
"""
import argparse
import json
import re
import sys
from datetime import date
from pathlib import Path

import build  # puts tools/ on sys.path for report
from report import (RELAY_HINT, beside, check_moved, config_path, config_theme, find_ath, no_config,
                    older_place, run_ath_report, write_json)

HERE = Path(__file__).parent
LISTS = "all,areas"  # every State, and every Area of the home State (`ath rank --in`)
STATE_LISTS = 51  # the 50 states and DC
# The college teams a page ranks the athlete against when its config names none (`colleges`).
DEFAULT_COLLEGES = [{"name": "UC Berkeley", "team": 21484}, {"name": "Colorado", "team": 20765},
                    {"name": "Virginia", "team": 21507}]
# The folder, beside the config, of hand-entered college marks: <team id>/<season>-<gender>-<event>.json.
COLLEGE_MARKS = "college-marks"


# ---------------------------------------------------------------- config

def load_config(path):
    if not path.exists():
        no_config(path, HERE / "rankmap.config.example.json")
    cfg = json.loads(path.read_text())
    for key, kind in (("athlete_id", int), ("event", str), ("graduation_year", int), ("output_dir", str)):
        if not isinstance(cfg.get(key), kind):
            sys.exit(f"{path}: '{key}' is required ({kind.__name__})")
    if cfg["athlete_id"] <= 0:
        sys.exit(f"{path}: put your athlete's Athletic.net id as 'athlete_id' (the number in their Profile URL)")

    cfg["output_dir"] = beside(path, cfg["output_dir"])
    cfg["data_dir"] = beside(path, cfg.get("data_dir", "data")) / f"{cfg['athlete_id']}-{cfg['event']}"
    # The page's look: the house theme, or a theme file's tokens over it (report.load_theme).
    cfg["theme"] = config_theme(path, cfg)
    month, day = map(int, cfg.get("season_end", "07-31").split("-"))
    cfg["season_end"] = (month, day)
    cfg["colleges"] = config_colleges(path, cfg)
    cfg["example"] = cfg.get("example", False)
    if not isinstance(cfg["example"], bool):
        sys.exit(f"{path}: 'example' is true or false")
    # Hand-entered college marks (college_marks.py) sit beside the config, in the user's private
    # local/ folder; an example page never reads any.
    cfg["college_marks_dir"] = None if cfg["example"] else path.parent / COLLEGE_MARKS
    # An example's State Meet standards are its own (mapped as its anonymized marks are), never the
    # published ones in areas/; a real page's are always those (report.state_areas).
    if cfg["example"]:
        if not isinstance(cfg.get("areas_dir"), str) or not cfg["areas_dir"].strip():
            sys.exit(f"{path}: an example needs 'areas_dir', a folder (path string) with its own State Meet "
                     "standards (an empty folder for a State without an Area file)")
        cfg["areas_dir"] = beside(path, cfg["areas_dir"])
        if not cfg["areas_dir"].is_dir():
            sys.exit(f"{path}: 'areas_dir' {cfg['areas_dir']} is not a folder")
    elif "areas_dir" in cfg:
        sys.exit(f"{path}: 'areas_dir' is only for an example (\"example\": true); a real page reads areas/")
    else:
        cfg["areas_dir"] = None
    return cfg


def config_colleges(path, cfg):
    """The config's `colleges`, checked: a list of {"name": <shown name>, "team": <Athletic.net team
    id>}, in display order, no team or name twice; an empty list for no Colleges section.
    DEFAULT_COLLEGES without the key."""
    if "colleges" not in cfg:
        return [dict(c) for c in DEFAULT_COLLEGES]
    shape = (f'{path}: \'colleges\' is a list of {{"name": <shown name>, "team": <Athletic.net team id>}}, '
             "in the order to show them")
    colleges = cfg["colleges"]
    if not isinstance(colleges, list):
        sys.exit(shape)
    out = []
    for i, c in enumerate(colleges):
        if not isinstance(c, dict) or set(c) != {"name", "team"}:
            sys.exit(f"{shape}; item {i + 1} is {json.dumps(c)}")
        if not isinstance(c["name"], str) or not c["name"].strip():
            sys.exit(f"{path}: colleges item {i + 1}: 'name' is the name to show (text)")
        if not isinstance(c["team"], int) or isinstance(c["team"], bool) or c["team"] <= 0:
            sys.exit(f"{path}: colleges item {i + 1}: 'team' is the team's Athletic.net id (the number in its URL)")
        out.append({"name": c["name"].strip(), "team": c["team"]})
    for key in ("team", "name"):
        seen = [c[key] for c in out]
        twice = sorted({str(v) for v in seen if seen.count(v) > 1})
        if twice:
            sys.exit(f"{path}: colleges lists the {key} {', '.join(twice)} twice")
    return out


def check_moved_data(cfg, old=None):
    """Stop when this config's data_dir has none of its seasons but the old default place (the main
    checkout's tools/rankmap/data/<athlete_id>-<event>) does, printing the move, rather than
    collect them all again."""
    old = old or older_place("rankmap", "data", cfg["data_dir"].name)
    check_moved(cfg["data_dir"], old, [data_path(cfg, s).name for s in high_school_seasons(cfg)])


def high_school_seasons(cfg):
    grad = cfg["graduation_year"]
    return list(range(grad - 3, grad + 1))


def grade_in(cfg, season):
    return 12 - (cfg["graduation_year"] - season)


def season_status(cfg, season, today):
    if today < date(season, 1, 1):
        return "upcoming"
    if today <= date(season, *cfg["season_end"]):
        return "in progress"
    return "finished"


def data_path(cfg, season):
    return cfg["data_dir"] / f"{season}.json"


def check_season(cfg, season, saved):
    """build.check against what this config's season must be: its athlete, event and season, on
    every State's list. Returns the report; raises build.SeasonError."""
    return build.check(saved, {"athlete_id": cfg["athlete_id"], "event": cfg["event"],
                               "season": season, "state_lists": STATE_LISTS})


def load_data(cfg):
    """Saved seasons that build ({season: saved}), and the ones that need collecting again
    ({season: why}): files from the retired browser collector, or ones this build cannot read."""
    have, stale = {}, {}
    for season in high_school_seasons(cfg):
        p = data_path(cfg, season)
        if not p.exists():
            continue
        try:
            saved = json.loads(p.read_text())
        except ValueError as e:
            stale[season] = f"is not valid JSON ({e})"
            continue
        try:
            check_season(cfg, season, saved)
            have[season] = saved
        except build.SeasonError as e:
            stale[season] = str(e)
    return have, stale


# ---------------------------------------------------------------- colleges

def college_seasons(have):
    """The college seasons a page shows, newest first: the years of its State seasons (have, the
    saved high school seasons), since each season's Colleges rank that season's best."""
    return sorted(have, key=int, reverse=True)


def college_path(cfg, season):
    return cfg["data_dir"] / "colleges" / f"{season}.json"


def college_event(cfg, have):
    """The event as `ath colleges` is asked for it: the full name the newest saved State season
    has (so an event code such as 100mH, which a Profile cannot be read by, still matches), else
    the configured one."""
    for season in sorted(have, reverse=True):
        name = have[season]["rank"].get("event_name")
        if name:
            return name
    return cfg["event"]


def check_college_season(cfg, season, saved, have):
    """build.check_colleges against what this config's college season must be. Raises
    build.SeasonError."""
    return build.check_colleges(saved, {"athlete_id": cfg["athlete_id"], "season": season,
                                        "teams": [c["team"] for c in cfg["colleges"]],
                                        "events": [cfg["event"], college_event(cfg, have)]})


def load_colleges(cfg, have):
    """Saved college seasons that build ({season: saved}), and the ones to collect again ({season:
    why}): not valid JSON, another shape (an older ath colleges schema too), or for other teams than
    the config's. Only the page's seasons (college_seasons) are read."""
    got, stale = {}, {}
    if not cfg["colleges"]:
        return got, stale
    for season in college_seasons(have):
        p = college_path(cfg, season)
        if not p.exists():
            continue
        try:
            saved = json.loads(p.read_text())
            check_college_season(cfg, season, saved, have)
            got[season] = saved
        except ValueError as e:  # build.SeasonError is one
            stale[season] = str(e)
    return got, stale


def plan_colleges(cfg, saved, have, args, today, collected=()):
    """College seasons to collect now, newest first: none with --no-fetch or no colleges; otherwise,
    of the page's seasons (college_seasons), one not saved (or saved unreadable, for other teams, or
    with an older ath colleges schema: load_colleges leaves those out of saved), one whose State
    season is in progress, one asked for with --refresh, and one whose State season this run
    collected (collected), so its ranks go with the season best just collected. A finished season
    saved whole stays as it is."""
    if args.no_fetch or not cfg["colleges"]:
        return []
    return [s for s in college_seasons(have)
            if s not in saved or s in args.refresh or s in collected
            or season_status(cfg, s, today) == "in progress"]


# ---------------------------------------------------------------- hand-entered college marks

def event_slug(event):
    """An event as a file name part: 400 Meters is 400-meters, as is 400m."""
    return re.sub(r"[^a-z0-9]+", "-", build.event_key(event)).strip("-")


def college_marks_path(folder, team, season, gender, event):
    """Where a hand-entered list is kept: <folder>/<team id>/<season>-<gender>-<event>.json."""
    return Path(folder) / str(team) / f"{season}-{gender}-{event_slug(event)}.json"


def read_college_marks(folder, teams=None, seasons=None):
    """The hand-entered lists in folder ({(team id, season): [saved list, ...]}), of the given teams
    and seasons (all without), and the files left out ({path: why}): not valid JSON, another shape,
    or filed under another team."""
    got, bad = {}, {}
    if folder is None or not Path(folder).is_dir():
        return got, bad
    for path in sorted(Path(folder).glob("*/*.json")):
        try:
            try:
                raw = json.loads(path.read_text())
            except ValueError as e:
                raise build.SeasonError(f"is not valid JSON ({e})") from None
            h = build.check_hand_marks(raw)
            if str(h["team"]) != path.parent.name:
                raise build.SeasonError(f"is team {h['team']}'s, filed under {path.parent.name}")
        except ValueError as e:  # build.SeasonError is one
            bad[path] = str(e)
            continue
        if (teams is None or h["team"] in teams) and (seasons is None or h["season"] in seasons):
            got.setdefault((h["team"], h["season"]), []).append(h)
    return got, bad


def load_college_marks(cfg, have):
    """This config's hand-entered lists: of its colleges, in the college seasons the page shows
    (read_college_marks; build.prepare_colleges keeps the athlete's gender and event). Files that
    cannot be read are left out, saying why."""
    if not cfg["colleges"]:
        return {}
    got, bad = read_college_marks(cfg["college_marks_dir"], {c["team"] for c in cfg["colleges"]},
                                  set(college_seasons(have)))
    for path, why in bad.items():
        print(f"hand-entered marks: left out, {path} {why}")
    return got


def colleges_command(cfg, season, event, refresh):
    cmd = [find_ath(), "colleges", str(cfg["athlete_id"]), "--event", event,
           "--teams", ",".join(str(c["team"]) for c in cfg["colleges"]), "--seasons", str(season), "--json"]
    if refresh:
        cmd.append("--refresh")
    return cmd


def collect_colleges(cfg, season, have, refresh, today):
    """Run `ath colleges` for one college season and save it. Returns the saved season, or None
    (and what was saved stays) when ath fails or a team could not be read.

    refresh: refetch instead of reading ath's cache (--refresh SEASON)."""
    cmd = colleges_command(cfg, season, college_event(cfg, have), refresh)
    label = f"colleges {season}"
    print(f"\nCollect {label}: {' '.join(cmd[1:])}", flush=True)
    report = run_ath_report(cmd, label)
    if report is None:
        return None
    saved = build.college_file(report, today.isoformat())
    try:
        check_college_season(cfg, season, saved, have)
    except build.SeasonError as e:
        print(f"  {label}: not saved, {e}")
        return None
    write_json(college_path(cfg, season), saved)
    print(f"  {label}: saved {college_path(cfg, season)}")
    return saved


# ---------------------------------------------------------------- planning

def ask(question, default):
    if not sys.stdin.isatty():
        return default
    hint = "[Y/n]" if default else "[y/N]"
    answer = input(f"{question} {hint} ").strip().lower()
    return default if not answer else answer.startswith("y")


def plan(cfg, have, stale, args, today, short=()):
    """Seasons to collect now, in order. A saved season that no longer builds counts as missing;
    a finished one saved without its home State's Areas (short) is collected again for them."""
    todo = []
    for season in high_school_seasons(cfg):
        status = season_status(cfg, season, today)
        if args.no_fetch:
            continue
        if season in args.refresh:
            todo.append(season)
        elif status == "upcoming":
            continue
        elif status == "in progress":
            if ask(f"{season} season is in progress. Pull fresh rankings?", True):
                todo.append(season)
        elif season not in have:
            if season in stale:
                print(f"{season}: {data_path(cfg, season)} {stale[season]}; collecting it again through ath")
            todo.append(season)
        elif season in short:
            print(f"{season}: saved without its home State's Area lists; collecting it again for them "
                  "(its State lists come from ath's cache)")
            todo.append(season)
        elif ask(f"{season} season is over and already collected. Refresh its data?", False):
            todo.append(season)
    return todo


# ---------------------------------------------------------------- collecting

def ath_command(cfg, season, refresh):
    cmd = [find_ath(), "rank", str(cfg["athlete_id"]), "--event", cfg["event"], "--season", str(season),
           "--in", LISTS, "--ahead", "all", "--grade", str(grade_in(cfg, season)), "--json"]
    if cfg.get("state"):
        cmd += ["--home", cfg["state"]]
    if refresh:
        cmd.append("--refresh")
    return cmd


def short_seasons(cfg, have, today):
    """Finished saved seasons with none of their home State's Areas (build.lacks_areas): collected
    again for them, from ath's cache. A season in progress is not one of them: collecting it asks,
    and pulls fresh, as always."""
    return {season for season, saved in have.items()
            if season_status(cfg, season, today) == "finished" and build.lacks_areas(saved)}


def wants_fresh(season, have, args, short=()):
    """Whether collecting a season should refetch rather than read ath's cache: when it is asked
    for by --refresh, or already saved (collecting it again is a refresh), unless it is collected
    again only for its Area lists (short). Anything else reads the cache, so a retry after a failed
    run only fetches what the failed run did not (in-season lists stay cached 12 hours)."""
    return season in args.refresh or (season in have and season not in short)


def collect(cfg, season, refresh, today):
    """Run `ath rank` for one season and save it. Returns the saved season, or None (and the season
    already saved, if any, stays as it was) when ath fails or its report is not whole.

    refresh: refetch instead of reading ath's cache (see wants_fresh)."""
    cmd = ath_command(cfg, season, refresh)
    print(f"\nCollect {season} (grade {grade_in(cfg, season)}): {' '.join(cmd[1:])}", flush=True)
    rank = run_ath_report(cmd, season)
    if rank is None:
        return None
    saved = build.season_file(rank, today.isoformat())
    try:
        r = check_season(cfg, season, saved)
    except build.SeasonError as e:
        print(f"  {season}: not saved, {e}")
        return None
    write_json(data_path(cfg, season), saved)
    print(f"  {season}: saved {data_path(cfg, season)} ({r['athlete']['name']}, {r['mark']['mark']})")
    return saved


# ---------------------------------------------------------------- main

def render(cfg, have, colleges=None, college_marks=None):
    """Build this config's page from its saved seasons ({season: saved}), college seasons
    ({season: saved}) and hand-entered college marks (load_college_marks) into its output_dir.
    Returns (html path, build.Page)."""
    return build.build(have, high_school_seasons(cfg), cfg["output_dir"], cfg["theme"], example=cfg["example"],
                       areas_dir=cfg["areas_dir"], colleges=cfg["colleges"], college_seasons=colleges,
                       college_marks=college_marks)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--config", help="a config file (with a /, e.g. ./x.config.json), or a config's name "
                                          "in local/rankmap/ (default: rankmap.config.json)")
    parser.add_argument("--no-fetch", action="store_true", help="build from saved data only")
    parser.add_argument("--refresh", type=int, nargs="*", default=[], metavar="SEASON",
                        help="collect these seasons without asking, even finished ones")
    args = parser.parse_args(argv)

    config = config_path("rankmap", args.config).resolve()
    print(f"Config: {config}", file=sys.stderr)
    cfg = load_config(config)
    if cfg["example"] and (not args.no_fetch or args.refresh):
        # An example's seasons are anonymized replays (ADR 0010): nothing to collect for it.
        print("An example config builds from its saved data only (--no-fetch).", file=sys.stderr)
        args.no_fetch, args.refresh = True, []
    check_moved_data(cfg)
    have, stale = load_data(cfg)
    today = date.today()
    short = short_seasons(cfg, have, today)
    todo = plan(cfg, have, stale, args, today, short)
    if todo:
        print(RELAY_HINT)
    collected = set()
    for season in todo:
        got = collect(cfg, season, wants_fresh(season, have, args, short), today)
        if got:
            have[season] = got
            collected.add(season)
    for season in sorted(set(stale) - set(have)):
        print(f"{season}: left out, {data_path(cfg, season)} {stale[season]}")

    if not have:
        sys.exit("No collected seasons yet, so nothing to build.")
    colleges, college_stale = load_colleges(cfg, have)
    college_marks = load_college_marks(cfg, have)
    college_todo = plan_colleges(cfg, colleges, have, args, today, collected)
    if college_todo and not todo:
        print(RELAY_HINT)
    for season in college_todo:
        if season in college_stale:
            print(f"colleges {season}: {college_path(cfg, season)} {college_stale[season]}; collecting it again")
        got = collect_colleges(cfg, season, have, season in args.refresh, today)
        if got:
            colleges[season] = got
            college_stale.pop(season, None)
    for season in sorted(college_stale):
        print(f"colleges {season}: left out, {college_path(cfg, season)} {college_stale[season]}")
    html_path, page = render(cfg, have, colleges, college_marks)
    seasons = page.seasons
    print()
    for season, sd in seasons.items():
        for v in sd["views"]:
            rows = v["rows"]
            worst = max(rows, key=lambda s: s["rank"])
            print(f"{season} {sd['pr']:>8} {v['label']:9} #1 in {sum(s['rank'] == 1 for s in rows):2}  "
                  f"top 5 in {sum(s['rank'] <= 5 for s in rows):2}  toughest {worst['abbr']} {worst['label']}")
            if v["areas"]:
                places = "  ".join(f"{s['abbr']} {s['label']}" for s in v["areas"])
                print(f"{'':>14} {sd['area_file']['labels']}: {places}")
    for season, c in (page.colleges or {"seasons": {}})["seasons"].items():
        if c is None:
            print(f"colleges {season}: no college data yet (a run without --no-fetch collects it)")
            continue
        for r in c["rows"]:
            print(f"colleges {season} {r['name']:16} {c['mark']['mark']:>8} {build.college_rank_text(r['cell'])}"
                  + ("  (counts hand-entered marks)" if r["hand"] else ""))
    print(f"\nWrote {html_path} (+ .md, .csv)")
    if cfg.get("artifact_url"):
        print(f"This config's shared page: {cfg['artifact_url']} (republish it there only if you share this page)")


if __name__ == "__main__":
    main()
