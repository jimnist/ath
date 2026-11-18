"""Hand-entered college marks: a college team's season list for one event, copied by hand from
another results source and pasted in, kept in your private local/ folder for rankmap's Colleges
section.

    python3 tools/rankmap/college_marks.py add --config NAME --team ID|NAME --season YEAR
        [--source NOTE] [--url URL] [--gender f|m] [--event NAME] [--file PATH] [--yes] < paste
    python3 tools/rankmap/college_marks.py list [--config NAME]

`add` reads the paste (stdin, or --file), shows the rows it read, and writes them only with --yes;
without it nothing is written. Entering the same college, season, gender and event again replaces
the list. Nothing here fetches anything: the list is whatever was pasted. See docs/rankmap.md.
"""
import argparse
import re
import sys
from datetime import date
from pathlib import Path

import build  # puts tools/ on sys.path for report
import rankmap
from report import config_path, write_json


class PasteError(ValueError):
    """A paste that cannot be read as a season list: the message says where and why."""


# Column names a copied list's header may have, by what the column holds. Others (a place, wind,
# team) are known header words too, and their cells are left out.
COLUMNS = {
    "name": {"athlete", "name", "athlete name", "student-athlete", "student athlete", "competitor"},
    "year": {"year", "yr", "class", "cl", "grade", "eligibility", "elig"},
    "mark": {"time", "mark", "result", "results", "performance", "perf", "season best", "best", "sb"},
    "meet": {"meet", "meet name", "competition"},
    "date": {"meet date", "date"},
}
OTHER_COLUMNS = {"rank", "place", "pl", "#", "no", "no.", "wind", "w", "team", "school", "points", "pts", "event"}
# A class year as lists give it: JR-3, Jr., SO-2, FR, R-Fr, Gr., 5th.
YEAR = re.compile(r"(?:r-?|rs-?)?(?:fr|so|jr|sr|gr|fy|freshman|sophomore|junior|senior|grad(?:uate)?|5th|6th)\.?"
                  r"(?:-\d)?", re.I)
# A field event's mark, which this never reads: 18-04.25, 5.60m, 18' 4".
FIELD_MARK = re.compile(r"\d{1,3}-\d{1,2}(?:\.\d+)?|\d+(?:\.\d+)?\s*m|\d+'\s*\d*(?:\.\d+)?\"?")
FIELD_EVENTS = re.compile(r"jump|vault|put|throw|discus|javelin|hammer|weight|thlon", re.I)
MONTHS = {m: i + 1 for i, m in enumerate(("jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct",
                                           "nov", "dec"))}


def header_word(cell):
    """A cell as a column name: lower case, without a trailing colon or a sort arrow."""
    return re.sub(r"[\s:\u2191\u2193\u25b2\u25bc]+$", "", cell.strip().lower())


def known_column(cell):
    w = header_word(cell)
    return any(w in names for names in COLUMNS.values()) or w in OTHER_COLUMNS


def is_header(cells):
    """Whether cells are a list's column names, naming at least the athlete and the mark."""
    cells = [c for c in cells if c.strip()]
    kinds = {column_kind(c) for c in cells}
    return bool(cells) and all(known_column(c) for c in cells) and {"name", "mark"} <= kinds


def column_kind(cell):
    w = header_word(cell)
    return next((k for k, names in COLUMNS.items() if w in names), None)


def read_time(text):
    """A time cell as (mark, hundredths), or None. A FAT time may carry its `a`; a hand time (`h`)
    is refused, as it never counts."""
    t = text.strip()
    if re.fullmatch(r"\d[\d:.]*\s*h", t, re.I):
        raise PasteError(f"{t!r} is a hand time; only FAT times count")
    t = re.sub(r"(?<=\d)\s*a$", "", t, flags=re.I)
    hund = build.time_hundredths(t)
    return None if hund is None else (t, hund)


def read_date(text, season):
    """A meet date cell as an ISO date (Apr 17, 2026; April 17 2026; 4/17/2026; 4/17/26;
    2026-04-17; Apr 17, in the season), or None."""
    t = " ".join(text.replace(",", " ").split()).lower()
    m = re.fullmatch(r"([a-z]{3})[a-z]*\.? (\d{1,2})(?: (\d{4}))?", t)
    try:
        if m and m.group(1) in MONTHS:
            return date(int(m.group(3) or season), MONTHS[m.group(1)], int(m.group(2))).isoformat()
        m = re.fullmatch(r"(\d{1,2})/(\d{1,2})/(\d{2}|\d{4})", t)
        if m:
            year = int(m.group(3)) + (2000 if len(m.group(3)) == 2 else 0)
            return date(year, int(m.group(1)), int(m.group(2))).isoformat()
        m = re.fullmatch(r"(\d{4})-(\d{2})-(\d{2})", t)
        if m:
            return date(int(m.group(1)), int(m.group(2)), int(m.group(3))).isoformat()
    except ValueError:
        return None
    return None


def is_year(text):
    return bool(YEAR.fullmatch(text.strip()))


def is_name(text):
    """Whether a line can be a person's name: letters, and no digits."""
    return bool(re.search(r"[^\W\d_]", text)) and not re.search(r"\d", text)


# An event's name in a heading, once its gender words are dropped: a distance in meters (with what
# follows it: 100 Meters Hurdles), a mile, or a field event's name. Never a word a surname can be
# alone (Hammer, Walker), so a list's first athlete is never taken for its heading.
HEADING = re.compile(r"\d[\d,]* meters(?: [a-z ]+)?|(?:\d+ )?mile(?: run)?|[a-z0-9 ]*\b(?:hurdles|steeplechase|"
                     r"(?:long|triple|high) jump|pole vault|shot put|discus|javelin|(?:hammer|weight) throw)\b[a-z0-9 ]*")


def heading_event(line):
    """The event a heading line names, as build.event_key has it, with its gender words and a
    trailing "Top" dropped; None for a line that names no event (or has a comma, as a "Last,
    First" name does)."""
    line = re.sub(r"(?<=\d),(?=\d{3})", "", line)
    if "," in line:
        return None
    t = re.sub(r"\b(?:women'?s|men'?s|women|men|girls|boys|outdoor|top)\b", " ", line.lower())
    t = " ".join(re.sub(r"[-:|()]", " ", t).split())
    t = re.sub(r"\b(?:metres?|meter)\b", "meters", t)
    t = re.sub(r"^(\d[\d,]*)\s*m\b", r"\1 meters", t)
    return build.event_key(t) if HEADING.fullmatch(t) else None


def heading_gender(line):
    t = line.lower()
    if re.search(r"\b(?:women'?s|women|girls)\b", t):
        return "f"
    if re.search(r"\b(?:men'?s|men|boys)\b", t):
        return "m"
    return None


def check_heading(line, event, gender):
    """A line before the list: fine when it is a heading of this event (or "Top"); refused when it
    names another event or gender."""
    named = heading_event(line)
    if named is not None and named != build.event_key(event):
        raise PasteError(f"the paste is headed {line.strip()!r}, not the {event}")
    g = heading_gender(line)
    if named is not None and g and g != gender:
        raise PasteError(f"the paste is headed {line.strip()!r}, not the {'women' if gender == 'f' else 'men'}'s list")


def field_or_unreadable(text, where):
    t = text.strip()
    if FIELD_MARK.fullmatch(t):
        return PasteError(f"{where}: {t!r} is a field event's mark; only running times are read")
    return PasteError(f"{where}: {t!r} is not a time (such as 53.76 or 1:00.04)")


def row(name, year, mark, meet, when, where):
    if not is_name(name):
        raise PasteError(f"{where}: {name.strip()!r} is not an athlete's name")
    return {"name": build.shown_name(name), "year": year.strip() or None if year else None, "mark": mark[0],
            "sort_int": mark[1] * 10, "meet": " ".join(meet.split()) or None if meet else None, "date": when}


def parse_paste(text, event, gender, season):
    """The rows of a pasted season list for event (as build.event_key has it), as
    [{"name", "year", "mark", "sort_int", "meet", "date"}], in the paste's order. Reads the layouts
    a copied list comes in: one field per line, tab-separated rows (a copied table), or typed
    `name, mark[, meet, date]` rows; an event heading (and a "Top" link) and a header of column
    names before the rows are skipped. Raises PasteError rather than guess."""
    if FIELD_EVENTS.search(event):
        raise PasteError(f"the {event} is a field event; only running times are read")
    lines = [ln for ln in (raw.strip() for raw in text.splitlines()) if ln]
    if not lines:
        raise PasteError("nothing was pasted")
    # The header: a tab-separated row of column names, or column names one per line.
    header, start = None, 0
    for i, line in enumerate(lines[:8]):
        if "\t" in line and is_header(line.split("\t")):
            header, start = [column_kind(c) for c in line.split("\t")], i + 1
            break
        run = 0
        while i + run < len(lines) and "\t" not in lines[i + run] and known_column(lines[i + run]):
            run += 1
        if run >= 2 and is_header(lines[i:i + run]):
            header, start = [column_kind(c) for c in lines[i:i + run]], i + run
            break
    if header is None:
        while start < len(lines) and (heading_event(lines[start]) is not None or header_word(lines[start]) == "top"):
            start += 1
    for line in lines[:start]:
        check_heading(line, event, gender)
    data = lines[start:]
    if not data:
        return []
    if any("\t" in ln for ln in data):
        rows = [tab_row(ln.split("\t"), header, season, f"row {n + 1}") for n, ln in enumerate(data)]
    elif all("," in ln and any(read_time_quiet(t) for t in ln.split(",")) for ln in data):
        rows = [typed_row(ln, season, f"row {n + 1}") for n, ln in enumerate(data)]
    else:
        rows = line_rows(data, season, start)
    for r in rows:
        if r["date"] and int(r["date"][:4]) != season:
            raise PasteError(f"{r['name']}'s meet date {r['date']} is not in the {season} season")
    return rows


def read_time_quiet(text):
    try:
        return read_time(text)
    except PasteError:
        return None


def tab_row(cells, header, season, where):
    """One tab-separated row: by the header's columns when there is one, else by what each cell
    looks like."""
    cells = [c.strip() for c in cells]
    if header:
        got = {k: cells[i] for i, k in enumerate(header) if k and i < len(cells)}
        mark = read_time(got.get("mark", ""))
        if mark is None:
            raise field_or_unreadable(got.get("mark", ""), where)
        when = got.get("date") or ""
        iso = read_date(when, season) if when else None
        if when and iso is None:
            raise PasteError(f"{where}: {when!r} is not a date")
        return row(got.get("name", ""), got.get("year"), mark, got.get("meet"), iso, where)
    k = next((i for i, c in enumerate(cells) if read_time(c)), None)
    if k is None:
        cell = next((c for c in cells if FIELD_MARK.fullmatch(c)), " ".join(cells))
        raise field_or_unreadable(cell, where)
    name = next((c for c in cells[:k] if is_name(c) and not is_year(c)), "")
    year = next((c for c in cells if is_year(c) and c != name), None)
    when = next((read_date(c, season) for c in cells[k + 1:] if read_date(c, season)), None)
    meet = next((c for c in cells[k + 1:] if is_name_or_meet(c) and not read_date(c, season) and not is_year(c)), None)
    return row(name, year, read_time(cells[k]), meet, when, where)


def is_name_or_meet(text):
    return bool(re.search(r"[^\W\d_]", text))


def typed_row(line, season, where):
    """One typed row: `name, mark[, meet, date]`, the name as "First Last" or "Last, First", a
    class year after it when given."""
    parts = [p.strip() for p in line.split(",")]
    k = next(i for i, p in enumerate(parts) if read_time_quiet(p))
    names, rest = parts[:k], parts[k + 1:]
    year = None
    if len(names) > 1 and is_year(names[-1]):
        year, names = names[-1], names[:-1]
    when = None
    for n in (2, 1):
        if len(rest) >= n and read_date(", ".join(rest[-n:]), season):
            when, rest = read_date(", ".join(rest[-n:]), season), rest[:-n]
            break
    return row(", ".join(names), year, read_time(parts[k]), ", ".join(rest), when, where)


def line_rows(lines, season, offset):
    """Rows from one field per line: a name, its class year when given, the time, then the meet and
    its date when given. A line after a time is the next athlete when a time (or a class year and
    a time) follows it, else the meet."""
    def time_at(i):
        return i < len(lines) and read_time(lines[i]) is not None

    def starts_row(i):
        return i < len(lines) and is_name(lines[i]) and not is_year(lines[i]) and (
            time_at(i + 1) or (i + 1 < len(lines) and is_year(lines[i + 1]) and time_at(i + 2)))

    rows, i = [], 0
    while i < len(lines):
        where = f"line {offset + i + 1} ({lines[i]!r})"
        if re.fullmatch(r"#?\d+\.?", lines[i]):  # a place number before the name
            i += 1
            continue
        if not starts_row(i):
            nxt = lines[i + 2] if i + 1 < len(lines) and is_year(lines[i + 1]) and i + 2 < len(lines) else (
                lines[i + 1] if i + 1 < len(lines) else None)
            if is_name(lines[i]) and nxt is not None:
                raise field_or_unreadable(nxt, f"the mark after {lines[i]!r}")
            raise PasteError(f"{where} is not where an athlete's row starts (a name, then the time)")
        name, i = lines[i], i + 1
        year = None
        if is_year(lines[i]):
            year, i = lines[i], i + 1
        mark, i = read_time(lines[i]), i + 1
        meet = when = None
        if i < len(lines) and not starts_row(i) and not re.fullmatch(r"#?\d+\.?", lines[i]):
            if read_date(lines[i], season):
                when, i = read_date(lines[i], season), i + 1
            else:
                meet, i = lines[i], i + 1
                if i < len(lines) and read_date(lines[i], season):
                    when, i = read_date(lines[i], season), i + 1
        rows.append(row(name, year, mark, meet, when, where))
    return rows


# ---------------------------------------------------------------- the files

def hand_file(team, college, season, gender, event, source, url, entered, pasted, rows):
    """A hand-entered list as saved (build.check_hand_marks reads it)."""
    return {"format": build.HAND_FORMAT, "team": team, "college": college, "season": season, "gender": gender,
            "event": event, "source": source, "source_url": url, "entered": entered, "how": build.HAND_HOW,
            "pasted": pasted, "marks": rows}


def earlier_source(folder, team, season):
    """The source note and link of the newest list already entered for this college and season (any
    event), so they are asked for only the first time: (source, url), or (None, None)."""
    got, _ = rankmap.read_college_marks(folder, {team}, {season})
    lists = sorted(got.get((team, season), []), key=lambda h: h.get("entered") or "")
    return (lists[-1].get("source"), lists[-1].get("source_url")) if lists else (None, None)


def athlete_side(cfg):
    """The athlete's gender and event name from the newest saved season (the event as `ath
    colleges` is asked for it), or (None, the config's event) before any season is saved."""
    have, _ = rankmap.load_data(cfg)
    if not have:
        return None, cfg["event"]
    return have[max(have)]["rank"]["gender"], rankmap.college_event(cfg, have)


def pick_team(cfg, given):
    """The config's college named by --team: its id, or its name in any case."""
    for c in cfg["colleges"]:
        if given.strip() == str(c["team"]) or given.strip().lower() == c["name"].lower():
            return c
    names = ", ".join(f"{c['name']} ({c['team']})" for c in cfg["colleges"]) or "none"
    sys.exit(f"--team {given}: not one of this config's colleges ({names}); add it to the config's `colleges` first")


def gender_of(text):
    g = {"f": "f", "w": "f", "women": "f", "woman": "f", "girls": "f", "m": "m", "men": "m", "man": "m",
         "boys": "m"}.get(text.strip().lower())
    if g is None:
        sys.exit(f"--gender {text}: f (women) or m (men)")
    return g


def print_rows(rows):
    if not rows:
        print("  (no rows: an empty list)")
        return
    width = max(len(r["name"]) for r in rows)
    for n, r in enumerate(rows, 1):
        print(f"  {n:>3}  {r['name']:<{width}}  {r['year'] or '':<6} {r['mark']:>8}  {r['meet'] or ''}"
              + (f"  {r['date']}" if r["date"] else ""))
    keys = [build.name_key(r["name"]) for r in rows]
    twice = sorted({r["name"] for r, k in zip(rows, keys) if keys.count(k) > 1})
    if twice:
        print(f"  Listed more than once (their best counts): {', '.join(twice)}")


def add(cfg, args, today):
    folder = cfg["college_marks_dir"]
    if folder is None:
        sys.exit("An example config takes no hand-entered marks.")
    college = pick_team(cfg, args.team)
    gender, event = athlete_side(cfg)
    if args.gender:
        gender = gender_of(args.gender)
    if gender is None:
        sys.exit("No season of this athlete is saved yet, so say whose list it is: --gender f or m")
    event = args.event or event
    if args.file:
        pasted = Path(args.file).expanduser().read_text()
    else:
        if sys.stdin.isatty():
            print("Paste the list, then press Ctrl-D on a line of its own:", file=sys.stderr)
        pasted = sys.stdin.read()
    try:
        rows = parse_paste(pasted, event, gender, args.season)
    except PasteError as e:
        sys.exit(f"Could not read the paste: {e}. Nothing was written.")
    source, url = args.source, args.url
    before_source, before_url = earlier_source(folder, college["team"], args.season)
    source = source or before_source
    url = url or before_url
    path = rankmap.college_marks_path(folder, college["team"], args.season, gender, event)
    who = "women" if gender == "f" else "men"
    print(f"{college['name']} {args.season} {who}'s {event}: {len(rows)} row{'s' if len(rows) != 1 else ''}")
    print_rows(rows)
    print(f"Source: {source or '(none yet)'}" + (f" ({url})" if url else ""))
    if not source:
        msg = (f"Say where this list was copied from the first time for {college['name']} {args.season}: "
               '--source "<a note in your own words>" (and --url <the page>, optional).')
        if args.yes:
            sys.exit(msg + " Nothing was written.")
        print(msg)
    if not args.yes:
        print(f"Preview only: nothing written. Rerun with --yes to write {path}"
              + (" (replacing the list there)" if path.exists() else ""))
        return None
    replacing = path.exists()
    write_json(path, hand_file(college["team"], college["name"], args.season, gender, event, source, url,
                               today.isoformat(), pasted, rows))
    print(f"{'Replaced' if replacing else 'Wrote'} {path}")
    print("Rebuild the page to count it: python3 tools/rankmap/rankmap.py --config <config> --no-fetch")
    return path


def show(cfg, today):
    """What hand-entered lists there are: per college of the config and college season, for the
    athlete's gender and event, then any other lists in the folder."""
    folder = cfg["college_marks_dir"]
    if folder is None:
        sys.exit("An example config takes no hand-entered marks.")
    gender, event = athlete_side(cfg)
    got, bad = rankmap.read_college_marks(folder)
    mine, other = [], []
    for (team, season), lists in sorted(got.items(), key=lambda kv: (kv[0][0], -kv[0][1])):
        for h in lists:
            ours = h["gender"] == gender and build.event_key(h["event"]) == build.event_key(event)
            (mine if ours else other).append(h)
    who = {"f": "women's ", "m": "men's "}.get(gender, "")
    print(f"Hand-entered college marks in {folder} for {who}{event}:")
    table = [("College", "Season", "Rows", "Entered", "Source")]
    for c in cfg["colleges"]:
        seasons = sorted({h["season"] for h in mine if h["team"] == c["team"]} | set(rankmap.college_seasons(today)),
                         reverse=True)
        for s in seasons:
            h = next((h for h in mine if (h["team"], h["season"]) == (c["team"], s)), None)
            table.append((c["name"], str(s), str(len(h["marks"])) if h else "-", h and h.get("entered") or "",
                          (h.get("source") or "") if h else "none entered"))
    widths = [max(len(r[i]) for r in table) for i in range(len(table[0]))]
    for r in table:
        print("  " + "  ".join(v.ljust(w) for v, w in zip(r, widths)).rstrip())
    teams = {c["team"] for c in cfg["colleges"]}
    other += [h for h in mine if h["team"] not in teams]
    if other:
        print("Other lists (another college, gender or event):")
        for h in other:
            print(f"  team {h['team']} ({h.get('college') or 'no name'}) {h['season']} {h['gender']} {h['event']}: "
                  f"{len(h['marks'])} rows, entered {h.get('entered')}, source: {h.get('source') or ''}")
    for path, why in bad.items():
        print(f"Unreadable: {path} {why}")


def main(argv=None, today=None):
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    sub = parser.add_subparsers(dest="command", required=True)
    a = sub.add_parser("add", help="read a pasted season list, show its rows, and write them with --yes")
    a.add_argument("--team", required=True, help="the college: its Athletic.net team id or its name in the config")
    a.add_argument("--season", type=int, required=True, help="the college outdoor season (year)")
    a.add_argument("--source", help="where the list was copied from, in your own words (private; needed the "
                                    "first time for a college and season, kept after)")
    a.add_argument("--url", help="the page the list was copied from (private, optional, kept after)")
    a.add_argument("--gender", help="f or m (default: the athlete's)")
    a.add_argument("--event", help='the event\'s full name, such as "400 Meters" (default: the athlete\'s)')
    a.add_argument("--file", help="read the paste from this file instead of stdin")
    a.add_argument("--yes", action="store_true", help="write the list (without it, only a preview)")
    ls = sub.add_parser("list", help="show which hand-entered lists there are")
    for p in (a, ls):
        p.add_argument("--config", help="a config file (with a /), or a config's name in local/rankmap/ "
                                        "(default: rankmap.config.json)")
    args = parser.parse_args(argv)
    cfg = rankmap.load_config(config_path("rankmap", args.config).resolve())
    today = today or date.today()
    if args.command == "add":
        return add(cfg, args, today)
    return show(cfg, today)


if __name__ == "__main__":
    main()
