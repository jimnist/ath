"""Write the made-up Recording the anonymizer's tests read (testdata/recording/).

Everyone and everything in it is invented: athletes, schools, cities, meets, marks and list ids.
Only the public structure is Athletic.net's own: the shape and names of the 2026 high school division
tree (copied from the made-up tests/fixtures/navinfo/2026.json) and event names.

It covers every response kind `ath` records (`ath --record-dir`, the `--fixtures-dir` layout):
bios (outdoor, indoor, hand-timed, DNS, relay splits, a relay result under the relay's team id,
field marks), a school's TeamNav chain, the season's tree and a State's own subtree, rankings
pages (Grade groups, ties, a field event, a list capped by a masked tail), a meet with its event
list and entries (an open event and two relays, one of them a 4xMile whose runners have miles),
and an athlete search. Run it again after changing it:

    python3 tools/anonymize/testdata/make_recording.py
"""

import json
import shutil
from pathlib import Path

HERE = Path(__file__).resolve().parent
OUT = HERE / "recording"
REPO = HERE.parents[2]

SEASON = 2026
CA, CCS, US, HS = 900500, 900502, 900010, 900020

SCHOOLS = {
    4101: ("Wrenfield Academy", "Port Ellery", True, "WRNF"),
    4102: ("Tidewater High School", "Callowmere", True, "TIDE"),
    4103: ("Quarry Hill", "Sunder Flats", False, "QHIL"),
    4104: ("Lantern Bay Prep", "Port Ellery", False, "LBAY"),
    4105: ("Saltmarsh Christian", "Gorsebridge", True, "SALT"),
}

MEETS = {
    660101: "Gullwing Invitational",
    660102: "Driftwood Relays 2026",
    660103: "Wrenfield Indoor Opener",
}

FIRST = [
    "Wren", "Juniper", "Marlo", "Tamsin", "Odette", "Linnea", "Brisa", "Calla", "Delphine",
    "Elowen", "Fenna", "Greer", "Hollis", "Ione", "Jessamy", "Kestrel", "Lark", "Merritt",
    "Noor", "Orla", "Perpetua", "Quinby", "Rosalind", "Saoirse", "Tindra", "Ulla", "Vesper",
    "Winslet", "Xanthe", "Yarrow",
]
LAST = [
    "Halvorsen", "Quillfeather", "Brackenridge", "Tolliver", "Ashgrove", "Penhallow",
    "Marchbank", "Underhill", "Fairweather", "Gallowglass", "Hawthorne-Pike", "Ironwood",
    "Jessop", "Kittredge", "Lisle", "Mountjoy", "Northcote", "Oakshott", "Pendergast",
    "Quarrington", "Rookwood", "Stillwater", "Thistlewood", "Upjohn", "Vantongeren",
    "Whitlock", "Yardley", "Zoller", "Abernathy-Cole", "Blythewood",
]

# 400m, girls: (athlete id, first, last, grade, school, SortInt, meet, date). Ties on purpose.
SUBJECT = 1203001
GIRLS = []
marks = [
    55210, 55800, 56090, 56090, 56400, 56770, 56770, 56770, 57020, 57300, 57310, 57640, 57990,
    58100, 58100, 58450, 58800, 59020, 59330, 59600, 60030, 60480, 60480, 61120, 61900, 62240,
    63010, 63500,
]
for i, mark in enumerate(marks):
    athlete = SUBJECT if i == 5 else 1203100 + i
    school = list(SCHOOLS)[i % len(SCHOOLS)]
    grade = 9 + (i * 7) % 4
    if athlete == SUBJECT:
        school, grade = 4101, 10
    meet = [660101, 660102][i % 2]
    date = "2026-04-%02dT00:00:00" % (4 + i % 20)
    GIRLS.append((athlete, FIRST[i % 30], LAST[i % 30], grade, school, mark, meet, date))


def write(rel, data, pretty=False):
    path = OUT / rel
    path.parent.mkdir(parents=True, exist_ok=True)
    text = json.dumps(data, indent=1 if pretty else None)
    path.write_text(text + "\n" if pretty else text)


def running(ms, suffix="a"):
    cs = (ms + 5) // 10
    minutes, rest = divmod(cs, 6000)
    seconds, hundredths = divmod(rest, 100)
    body = "%d:%02d.%02d" % (minutes, seconds, hundredths) if minutes else "%d.%02d" % (seconds, hundredths)
    return body + suffix


def field(inches_x100):
    feet, rest = divmod(inches_x100, 1200)
    return "%d-%05.2f" % (feet, rest / 100)


def row(rank, num, a, event="400 Meters", short="400m", display=None, sort=None):
    athlete, first, last, grade, school, mark, meet, date = a
    sort = mark if sort is None else sort
    return {
        "rank": rank, "rowNum": num, "AthleteID": athlete, "AthleteName": first + " " + last,
        "GradeID": grade, "display": display if display is not None else running(sort, ""),
        "SortIntRaw": sort, "SortIntCalc": sort, "SortIntOrig": sort, "TeamID": school,
        "TeamName": SCHOOLS[school][0], "ResultDate": date, "MeetID": meet,
        "MeetName": MEETS[meet], "Event": event, "EventShort": short, "SeasonID": SEASON,
        "State": "CA",
    }


def ranked(people, key):
    """Competition ranking: tied marks share a rank."""
    rows, last, rank = [], None, 0
    for num, a in enumerate(sorted(people, key=key), start=1):
        if key(a) != last:
            rank, last = num, key(a)
        rows.append((rank, num, a))
    return rows


def page(list_id, name, parent, base, event, grades, rows, min_count=None):
    settings = {"fat": True, "official": True}
    if grades:
        settings["grades"] = grades
    settings.update({"page": 1, "depth": 100})
    return {
        "eventShort": event,
        "division": {"ID": list_id, "ParentID": parent, "BaseDivID": base, "SeasonID": SEASON, "Name": name},
        "minCount": len(rows) if min_count is None else min_count,
        "settings": settings,
        "groupedRankings": [rows],
        "relayTeams": [],
        "jwtTFTopReport": "",
    }


def rankings():
    lists = [(CA, "California", HS, 903, lambda a: True), (CCS, "Central Coast", CA, 904, lambda a: SCHOOLS[a[4]][2])]
    for list_id, name, parent, base, keep in lists:
        groups = [list(range(9, top + 1)) for top in (9, 10, 11, 12)] + [[g] for g in (10, 11, 12)]
        for grades in groups:
            people = [a for a in GIRLS if keep(a) and a[3] in grades]
            rows = [row(r, n, a) for r, n, a in ranked(people, lambda a: a[5])]
            stem = "%d-f-400m-%s-p1" % (list_id, "_".join(map(str, grades)))
            write("rankings/%s.json" % stem, page(list_id, name, parent, base, "400m", grades, rows), pretty=True)
    # California, every grade: 25 rows, then the paywall's masked tail.
    rows = [row(r, n, a) for r, n, a in ranked(GIRLS, lambda a: a[5])][:25]
    rows.append({
        "rank": 26, "rowNum": 26, "AthleteID": 0, "AthleteName": "Xxxxxx Xxxxxx", "GradeID": 99,
        "display": "", "SortIntRaw": 63500, "SortIntCalc": 63500, "SortIntOrig": 63500, "TeamID": 0,
        "TeamName": "Xxxxxxxx Xxxx", "ResultDate": "2026-04-18T00:00:00", "MeetID": 0,
        "MeetName": "XXX Xxxxxxx Xxxxxxxxxxxxx Xxxxxx", "Event": "400 Meters", "EventShort": "400m",
        "SeasonID": SEASON, "State": "CA",
    })
    write("rankings/%d-f-400m-all-p1.json" % CA, page(CA, "California", HS, 903, "400m", None, rows, min_count=60), pretty=True)
    # Central Coast shot put, every grade: feet-inches marks, a tie.
    shots = [4210, 3925, 3925, 3650, 3412, 3300, 3075]
    people = [a for a in GIRLS if SCHOOLS[a[4]][2]][: len(shots)]
    rows = []
    scored = sorted(zip(people, shots), key=lambda p: -p[1])
    rank, last = 0, None
    for num, (a, inches_x100) in enumerate(scored, start=1):
        if inches_x100 != last:
            rank, last = num, inches_x100
        sort = 20_000_000 - inches_x100 * 10
        rows.append(row(rank, num, a, "Shot Put", "Shot", field(inches_x100), sort))
    write("rankings/%d-f-Shot-all-p1.json" % CCS, page(CCS, "Central Coast", CA, 904, "Shot", None, rows), pretty=True)


RESULT_IDS = iter(range(88000001, 88999999))


def result(athlete, display, sort, event, season=SEASON, date="2026-04-11T00:00:00", meet=660101, kind=0, school=4101, exhibition=False):
    return {
        "IDResult": next(RESULT_IDS),
        "AthleteID": athlete, "Result": display, "SortInt": sort, "EventID": event,
        "EventTypeID": kind, "SeasonID": season, "Exhibition": exhibition, "ResultDate": date,
        "MeetID": meet, "meetName": None, "SchoolID": school, "PersonalBest": 0, "SeasonBest": 0,
    }


GIRLS_EVENTS = {
    21: "400 Meters", 22: "800 Meters", 23: "1600 Meters", 59: "1 Mile", 33: "4x400 Relay",
    60: "4xMile Relay", 30: "Shot Put", 25: "High Jump",
}
BOYS_EVENTS = {4: "800 Meters", 39: "4x800 Relay"}


def bio(athlete, first, last, gender, school, grade, events, results, extra_teams=()):
    teams = {str(school): {"IDSchool": school, "SchoolName": SCHOOLS[school][0], "Level": 4, "Year": SEASON}}
    grades = {"%d_%d" % (school, SEASON): grade, "%d_%d" % (school, 10000 + SEASON): grade}
    for club, name in extra_teams:
        teams[str(club)] = {"IDSchool": club, "SchoolName": name, "Level": 6, "Year": SEASON}
        grades["%d_%d" % (club, SEASON)] = 402
    meets = {}
    for r in results:
        if r["MeetID"]:
            meets[str(r["MeetID"])] = {"IDMeet": r["MeetID"], "MeetName": MEETS[r["MeetID"]], "EndDate": r["ResultDate"]}
    table = GIRLS_EVENTS if gender == "F" else BOYS_EVENTS
    return {
        "athlete": {"IDAthlete": athlete, "SchoolID": school, "FirstName": first, "LastName": last, "Gender": gender},
        "allTeams": teams,
        "grades": grades,
        "allSeasons": [{"ID": SEASON, "Display": "2026 Outdoor"}, {"ID": 10000 + SEASON, "Display": "2026 Indoor"}],
        "eventsTF": [{"Event": table[e], "Type": "F" if e in (25, 30) else "T", "IDEvent": e, "IDEventType": 0} for e in events],
        "resultsTF": results,
        "meets": meets,
    }


def bios():
    subject = next(a for a in GIRLS if a[0] == SUBJECT)
    _, first, last, grade, school, mark, meet, date = subject
    relay_team = 7700123
    results = [
        result(SUBJECT, running(mark), mark, 21, date=date, meet=meet),
        result(SUBJECT, running(57400), 57400, 21, date="2026-03-21T00:00:00", meet=660102),
        result(SUBJECT, "57.9h", 57900, 21, date="2026-03-14T00:00:00"),
        result(SUBJECT, "DNS", 20000001, 21, date="2026-05-02T00:00:00", meet=660102),
        result(SUBJECT, running(58890), 58890, 21, season=10000 + SEASON, date="2025-12-30T00:00:00", meet=660103),
        result(SUBJECT, running(55950), 55950, 21, date="2026-04-25T00:00:00", meet=660102, kind=98),
        result(relay_team, running(236400), 236400, 33, date="2026-04-25T00:00:00", meet=660102),
        result(SUBJECT, "31-04.50", 20_000_000 - 37650, 30, date="2026-04-11T00:00:00"),
        result(SUBJECT, "4-10", 20_000_000 - 58000, 25, date="2026-04-11T00:00:00"),
    ]
    write("%d.json" % SUBJECT, bio(SUBJECT, first, last, "F", school, grade, [21, 33, 30, 25], results, [(5901, "Ellery Striders TC")]), pretty=True)


def teamnav():
    name, city, _, _ = SCHOOLS[4101]
    write("teamnav/4101-%d.json" % SEASON, {
        "team": {"ID": 4101, "Name": name, "Level": 4, "City": city, "State": "CA"},
        "userAthleteIdOnTeam": None, "hasCoachAccess": False, "mascotToken2": None, "grades": [],
        "divisions": [
            {"id": US, "b": 901, "name": "   United States"},
            {"id": HS, "b": 902, "name": "High School"},
            {"id": CA, "b": 903, "name": "California"},
            {"id": CCS, "b": 904, "name": "Central Coast"},
            {"id": 900511, "b": 905, "name": " Example Conference"},
        ],
        "customLists": [], "customDivisions": [], "linkedTeams": [],
    }, pretty=True)


def navinfo():
    nav = json.loads((REPO / "tests/fixtures/navinfo/2026.json").read_text())
    write("navinfo/%d.json" % SEASON, nav, pretty=True)
    state = dict(nav)
    state["tree"] = [r for r in nav["regions"] if r.get("state") == "CA"]
    write("navinfo/%d-%d.json" % (SEASON, CA), state, pretty=True)


MEET = 770001


def meet():
    write("meets/%d/meet.json" % MEET, {
        "meet": {
            "ID": MEET, "Name": "Harrowgate Winter Classic", "MeetDate": "2099-01-09T00:00:00",
            "StartDate": "2099-01-09T00:00:00", "EndDate": "2099-01-10T00:00:00", "SeasonID": 12099,
            "PublicEntries": "2098-12-20T00:00:00", "RegEnabled": True, "RelayAthletesRequired": 4,
            "Location": {"Indoor": True, "Name": "Harrowgate Fieldhouse", "City": "Callowmere", "State": "CA"},
        },
        "tfDivisions": [
            {"IDMeetDiv": 3300013, "MeetID": MEET, "IDDiv": 13, "Division": "Varsity", "DivisionShort": "V", "LevelMask": 4},
        ],
    }, pretty=True)
    events = [
        {"ID": 22, "Event": "800 Meters", "EventShort": "800m", "Gender": "F", "Type": "T", "Entry": 1, "sort": 4, "isPersonal": False},
        {"ID": 39, "Event": "4x800 Relay", "EventShort": "4x800m", "Gender": "M", "Type": "T", "Entry": 1, "sort": 9, "isPersonal": False},
        {"ID": 60, "Event": "4xMile Relay", "EventShort": "4xmile", "Gender": "F", "Type": "T", "Entry": 1, "sort": 10, "isPersonal": False},
    ]
    write("meets/%d/events.json" % MEET, {
        "events": events,
        "eventDivsWithEntries": [{"e": 22, "d": 13, "ge_p": None}, {"e": 39, "d": 13, "ge_p": None}, {"e": 60, "d": 13, "ge_p": None}],
        "eventDivsWithResults": [],
    }, pretty=True)

    def entry(athlete, first, last, gender, team, event, letter, school, grade, sort):
        name, _, _, code = SCHOOLS[school]
        return {
            "IDAthlete": athlete, "FirstName": first, "LastName": last, "Gender": gender, "TeamID": team,
            "EventID": event, "Letter": letter, "SchoolID": school, "FAT": None, "Grade": str(grade),
            "TeamName": name, "TeamCode": code, "DivID": 13, "Comment": "seeded by coach " + last if letter == "A1" else None,
            "Locked": False, "Scratched": False, "OverrideSortInt": None, "SortIntRaw": sort, "ConversionInt": 0,
            "SortInt": sort, "AcceptanceStatus": 10, "Source": 1, "OwnerUserId": 5550000 + athlete % 1000 if letter else None,
            "OriginalEvent": None,
        }

    def teams_of(entries, gender):
        seen = []
        for e in entries:
            if e["SchoolID"] not in seen:
                seen.append(e["SchoolID"])
        return [{"IDSchool": s, "SchoolName": SCHOOLS[s][0], "TeamCode": SCHOOLS[s][3], "LevelName": "High School",
                 "DivName": "Central Coast", "ParentDivName": "California", "GrandparentDivName": "High School", "Gender": gender}
                for s in seen]

    def entries_file(event, entries, gender):
        return {"eventId": event, "currentEventValid": True, "entries": entries, "teams": teams_of(entries, gender),
                "events": [], "eventDivsWithEntries": [], "eventTypes": [], "rounds": [], "releyLegs": []}

    # Girls 800m: three entrants, one with no seed.
    open_800 = []
    for i, (athlete, first, last, school, pr, seed) in enumerate([
        (1204001, "Linnea", "Rookwood", 4102, 131250, 131250),
        (1204002, "Odette", "Pendergast", 4103, 129980, 130000),
        (1204003, "Calla", "Stillwater", 4104, 133400, 0),
    ]):
        open_800.append(entry(athlete, first, last, "F", school, 22, "", school, 11, seed))
        write("%d.json" % athlete, bio(athlete, first, last, "F", school, 11, [22], [
            result(athlete, running(pr), pr, 22, date="2026-05-09T00:00:00", meet=660102, school=school),
            result(athlete, running(pr + 1700), pr + 1700, 22, date="2026-04-04T00:00:00", school=school),
        ]), pretty=True)
    write("meets/%d/entries/f-13-800m.json" % MEET, entries_file(22, open_800, "F"), pretty=True)

    # Boys 4x800: two teams of four, a tie between two runners' marks.
    relay = []
    boys = [
        (1205001, "Hollis", "Brackenridge", 4101, 118200), (1205002, "Merritt", "Tolliver", 4101, 119900),
        (1205003, "Quinby", "Ashgrove", 4101, 121050), (1205004, "Lark", "Marchbank", 4101, 121050),
        (1205005, "Kestrel", "Underhill", 4102, 117480), (1205006, "Greer", "Fairweather", 4102, 120300),
        (1205007, "Noor", "Gallowglass", 4102, 122600), (1205008, "Vesper", "Kittredge", 4102, 123990),
    ]
    for i, (athlete, first, last, school, pr) in enumerate(boys):
        team = 7710000 + school
        letter = "A%d" % (i % 4 + 1)
        relay.append(entry(athlete, first, last, "M", team, 39, letter, school, 12 - i % 3, 482000 if school == 4101 else 478500))
        write("%d.json" % athlete, bio(athlete, first, last, "M", school, 12 - i % 3, [4, 39], [
            result(athlete, running(pr), pr, 4, date="2026-05-16T00:00:00", meet=660101, school=school),
            result(athlete, running(pr - 2100), pr - 2100, 4, date="2026-05-30T00:00:00", meet=660102, kind=98, school=school),
            result(athlete, running(pr + 900), pr + 900, 4, season=10000 + SEASON, date="2026-02-07T00:00:00", meet=660103, school=school),
            result(team, running(484000), 484000, 39, date="2026-05-30T00:00:00", meet=660102, school=school),
        ]), pretty=True)
    write("meets/%d/entries/m-13-4x800m.json" % MEET, entries_file(39, relay, "M"), pretty=True)

    # Girls 4xMile: one team, some runners with a mile faster than their 1600m.
    mile = []
    girls = [
        (1206001, "Saoirse", "Lisle", 307000, 309100), (1206002, "Tindra", "Mountjoy", 311400, 0),
        (1206003, "Ulla", "Northcote", 300000, 305000), (1206004, "Xanthe", "Oakshott", 0, 318800),
    ]
    for i, (athlete, first, last, m1600, m_mile) in enumerate(girls):
        mile.append(entry(athlete, first, last, "F", 7714105, 60, "A%d" % (i + 1), 4105, 10 + i % 3, 1260000))
        results = []
        if m1600:
            results.append(result(athlete, running(m1600), m1600, 23, date="2026-04-18T00:00:00", meet=660101, school=4105))
        if m_mile:
            results.append(result(athlete, running(m_mile), m_mile, 59, date="2026-05-02T00:00:00", meet=660102, school=4105))
        write("%d.json" % athlete, bio(athlete, first, last, "F", 4105, 10 + i % 3, [23, 59], results), pretty=True)
    write("meets/%d/entries/f-13-4xmile.json" % MEET, entries_file(60, mile, "F"), pretty=True)


def boys_800m():
    """A list over two pages (rows 1 to 101, then 100 to 150, as Athletic.net pages them), with
    two relay runners who have bios (protected) among made-up people."""
    people = []
    for i in range(148):
        school = [4101, 4102, 4105][i % 3]
        mark = 113000 + i * 170 + (i % 7) * 10
        if i in (40, 41):
            mark = 113000 + 40 * 170  # a tie
        people.append((1207000 + i, FIRST[i % 30], LAST[(i * 7) % 30] + "-" + LAST[(i * 11) % 30],
                       9 + i % 4, school, mark, [660101, 660102][i % 2], "2026-05-%02dT00:00:00" % (1 + i % 28)))
    people.append((1205001, "Hollis", "Brackenridge", 12, 4101, 118200, 660101, "2026-05-16T00:00:00"))
    people.append((1205005, "Kestrel", "Underhill", 12, 4102, 117480, 660101, "2026-05-16T00:00:00"))
    rows = [row(r, n, a, "800 Meters", "800m") for r, n, a in ranked(people, lambda a: a[5])]
    for page_no, (lo, hi) in enumerate([(1, 101), (100, 150)], start=1):
        page_rows = [r for r in rows if lo <= r["rowNum"] <= hi]
        doc = page(CCS, "Central Coast", CA, 904, "800m", None, page_rows, min_count=len(rows))
        doc["settings"]["page"] = page_no
        write("rankings/%d-m-800m-all-p%d.json" % (CCS, page_no), doc, pretty=True)


def search():
    write("search/odette-pendergast.json", {"response": {"numFound": 2, "docs": [
        {"id_db": "1204002", "type": "Athlete", "textsuggest": "Odette Pendergast",
         "subtext": "Quarry Hill (HS)||Sunder Flats, CA", "score": 41.5},
        {"id_db": "4103", "type": "Team", "textsuggest": "Quarry Hill",
         "subtext": "High School||Sunder Flats, CA", "score": 3.0},
    ]}})


def main():
    if OUT.exists():
        shutil.rmtree(OUT)
    rankings()
    boys_800m()
    bios()
    teamnav()
    navinfo()
    meet()
    search()


if __name__ == "__main__":
    main()
