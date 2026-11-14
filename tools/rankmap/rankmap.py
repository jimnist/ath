"""rankmap: where an athlete's season best would rank in every state, as a map page.

    python3 tools/rankmap/rankmap.py [--config NAME|PATH] [--no-fetch] [--refresh SEASON ...]

Reads local/rankmap/rankmap.config.json (copy rankmap.config.example.json), collects any seasons
that need data with `ath rank` (all states, every grade group), then writes
<output_dir>/<athlete_id>-<event>.html plus .md and .csv. See README.md.
"""
import argparse
import json
import sys
from datetime import date
from pathlib import Path

import build  # puts tools/ on sys.path for report
from report import (RELAY_HINT, beside, check_moved, config_path, config_theme, find_ath, no_config,
                    older_place, run_ath_report, write_json)

HERE = Path(__file__).parent
LISTS = "all,areas"  # every State, and every Area of the home State (`ath rank --in`)
STATE_LISTS = 51  # the 50 states and DC


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
    cfg["example"] = cfg.get("example", False)
    if not isinstance(cfg["example"], bool):
        sys.exit(f"{path}: 'example' is true or false")
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

def render(cfg, have):
    """Build this config's page from its saved seasons ({season: saved}) into its output_dir.
    Returns (html path, build.Page)."""
    return build.build(have, high_school_seasons(cfg), cfg["output_dir"], cfg["theme"], example=cfg["example"],
                       areas_dir=cfg["areas_dir"])


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
    for season in todo:
        got = collect(cfg, season, wants_fresh(season, have, args, short), today)
        if got:
            have[season] = got
    for season in sorted(set(stale) - set(have)):
        print(f"{season}: left out, {data_path(cfg, season)} {stale[season]}")

    if not have:
        sys.exit("No collected seasons yet, so nothing to build.")
    html_path, page = render(cfg, have)
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
    print(f"\nWrote {html_path} (+ .md, .csv)")
    if cfg.get("artifact_url"):
        print(f"This config's shared page: {cfg['artifact_url']} (republish it there only if you share this page)")


if __name__ == "__main__":
    main()
