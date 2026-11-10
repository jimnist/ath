#!/usr/bin/env python3
"""Anonymize a Recording of Athletic.net responses (ADR 0010, docs/anonymize.md).

    python3 tools/anonymize/anonymize.py <recording dir> <out dir> --key-file local/anonymize.key

A Recording is what `ath --record-dir` writes: the `--fixtures-dir` layout (`<athlete_id>.json`,
`teamnav/`, `navinfo/`, `rankings/`, `meets/<meet_id>/`, `search/`). The anonymizer writes the same
layout to <out dir>, so `ath ... --fixtures-dir <out dir>` replays it (with `ATH_TODAY` pinned to a
date moved like the data). The privacy model:

- names: every athlete, school, club, team code, city, venue and meet name invented, the same real
  thing becoming the same invented thing in every file; free text and tokens blanked; every key of
  every response on a reviewed list, locations and contacts blanked;
- ids: athlete, school, team, meet, result and user ids, and list and division ids, moved into a
  reserved range (nine digits starting with 9), file and folder names included: no id, a list's
  included, can be matched with the public site;
- seasons and dates: fictional seasons ending at 2099 (the latest recorded outdoor season is
  2099), every date moved with its season and by a few days within it;
- marks: one rank-preserving map per event on the SortInt the lists sort by (a better-mark
  baseline plus a keyed jitter per distinct mark, bounded by the gaps to its neighbours), the same
  in every file, so order, ties and the side of every State Meet standard hold while the gaps do
  not;
- rows: a keyed 10% to 20% of the athletes on the lists leave every list for invented stand-ins a
  few marks away, so ranks are realistic, not real;
- grades: swapped in pairs among those athletes, far apart in mark on the same lists, so every
  list keeps its per-grade counts;
- kept: subjects (bios), meet entrants and `--keep` athletes stay on every list with their own
  grades; States, Areas and other division names, event names and ids, grades as numbers;
- residual risk: a row's Area and approximate mark narrow it down to that Area's athletes near
  that mark.

The JSON text stays as recorded but for the values replaced and the rows rebuilt. Every run checks
its own output before writing it, and fails (writing nothing) when a real name or id survives
anywhere, a value it changes is left as recorded, a list is not consistent in itself, or the maps
break order, ties, standards, grade counts or mile estimates. The key makes a run repeatable: keep
it private, since with it and the tool anyone could test a guessed name.

Python 3.9+, standard library only.
"""

import argparse
import datetime
import hashlib
import hmac
import json
import math
import os
import re
import secrets
import shutil
import statistics
import subprocess
import sys
import tempfile
from decimal import ROUND_FLOOR, Decimal
from pathlib import Path

RESERVED_BASE = 900_000_000
RESERVED_SPAN = 100_000_000

# A field mark's SortInt: 20,000,000 - inches x 1000. Anything at or past the base is no mark.
FIELD_BASE = 20_000_000
FIELD_FLOOR = 10_000_000

# ---------------------------------------------------------------------------------------------
# JSON with spans: parse a file's text into nodes that know where they are, so a value can be
# replaced in place and everything else stays byte for byte.


class Node:
    __slots__ = ("kind", "start", "end", "value", "items")

    def __init__(self, kind, start, end, value=None, items=None):
        self.kind = kind  # object, array, string, number, literal
        self.start = start
        self.end = end
        self.value = value
        self.items = items

    def get(self, key):
        if self.kind != "object":
            return None
        for k, v in self.items:
            if k.value == key:
                return v
        return None


_NUMBER = re.compile(r"-?(?:0|[1-9]\d*)(?:\.\d+)?(?:[eE][+-]?\d+)?")
_WS = " \t\r\n"


class JsonError(ValueError):
    pass


def parse_json(text):
    def ws(i):
        while i < len(text) and text[i] in _WS:
            i += 1
        return i

    def string(i):
        j = i + 1
        while True:
            if j >= len(text):
                raise JsonError("unterminated string at %d" % i)
            c = text[j]
            if c == "\\":
                j += 2
                continue
            if c == '"':
                break
            j += 1
        return Node("string", i, j + 1, json.loads(text[i : j + 1])), j + 1

    def value(i):
        i = ws(i)
        if i >= len(text):
            raise JsonError("unexpected end")
        c = text[i]
        if c == "{":
            items, j = [], ws(i + 1)
            if text[j] == "}":
                return Node("object", i, j + 1, items=items), j + 1
            while True:
                j = ws(j)
                if text[j] != '"':
                    raise JsonError("expected a key at %d" % j)
                key, j = string(j)
                j = ws(j)
                if text[j] != ":":
                    raise JsonError("expected ':' at %d" % j)
                val, j = value(j + 1)
                items.append((key, val))
                j = ws(j)
                if text[j] == ",":
                    j += 1
                elif text[j] == "}":
                    return Node("object", i, j + 1, items=items), j + 1
                else:
                    raise JsonError("expected ',' or '}' at %d" % j)
        if c == "[":
            items, j = [], ws(i + 1)
            if text[j] == "]":
                return Node("array", i, j + 1, items=items), j + 1
            while True:
                val, j = value(j)
                items.append(val)
                j = ws(j)
                if text[j] == ",":
                    j += 1
                elif text[j] == "]":
                    return Node("array", i, j + 1, items=items), j + 1
                else:
                    raise JsonError("expected ',' or ']' at %d" % j)
        if c == '"':
            return string(i)
        for word, val in (("true", True), ("false", False), ("null", None)):
            if text.startswith(word, i):
                return Node("literal", i, i + len(word), val), i + len(word)
        m = _NUMBER.match(text, i)
        if not m:
            raise JsonError("unexpected %r at %d" % (c, i))
        raw = m.group(0)
        num = int(raw) if re.fullmatch(r"-?\d+", raw) else float(raw)
        return Node("number", i, m.end(), num), m.end()

    root, end = value(0)
    if ws(end) != len(text):
        raise JsonError("trailing text at %d" % end)
    return root


def splice(text, edits):
    """`text` with each (start, end, replacement) applied."""
    out, last = [], 0
    for start, end, new in sorted(edits, key=lambda e: e[0]):
        if start < last:
            raise AssertionError("overlapping edits at %d" % start)
        out.append(text[last:start])
        out.append(new)
        last = end
    out.append(text[last:])
    return "".join(out)


def encode(value):
    return json.dumps(value, ensure_ascii=False)


# ---------------------------------------------------------------------------------------------
# What the responses hold, by key.

# Integer (or digit-string) ids of people, schools, teams, meets, results and users.
ID_KEYS = {
    "AthleteID", "IDAthlete", "AthleteId", "athleteId", "SchoolID", "IDSchool", "TeamID", "IDTeam",
    "teamId", "MeetID", "IDMeet", "meetId", "IDResult", "ResultID", "IDMeetDiv", "OwnerUserId",
    "userAthleteIdOnTeam", "id_db", "UserID", "IDUser", "CoachID", "RelayTeamID", "IDCustomList",
    "ReplacementID",
}
# An `ID` is one of those under these parents (a meet's, a school's, an athlete's own id); under
# any other (events, divisions) it is public structure.
ID_PARENTS = {"meet", "team", "athlete"}
# Dicts keyed by such ids.
ID_KEYED = {"allTeams", "meets", "relayTeams", "customLists"}
FIRST_KEYS = {"FirstName", "firstName", "AthleteFirstName"}
LAST_KEYS = {"LastName", "lastName", "AthleteLastName"}
PERSON_KEYS = {"AthleteName", "athleteName", "FullName", "fullName"}
SCHOOL_KEYS = {"SchoolName", "TeamName", "schoolName", "teamName"}
MEET_KEYS = {"MeetName", "meetName"}
CITY_KEYS = {"City", "city"}
CODE_KEYS = {"TeamCode", "teamCode", "SchoolCode"}
FREE_TEXT_KEYS = {"Comment", "comment", "Notes", "notes"}
# Division names: public structure, unless one is a school's name.
DIVISION_NAME_KEYS = {"DivName", "ParentDivName", "GrandparentDivName"}
DISPLAY_KEYS = {"Result", "display"}
SORT_KEYS = {"SortInt", "SortIntRaw", "SortIntCalc", "SortIntOrig", "OverrideSortInt"}
# Public structure and labels: never changed.
KEEP_KEYS = {
    "Event", "EventShort", "eventShort", "Division", "DivisionShort", "divType", "subDivType",
    "State", "state", "LevelName", "Gender", "gender", "Type", "type", "Display", "name", "Letter",
    "Grade", "mode", "reportType",
    # The site's own vocabulary (a page's filters and settings, an event's codes, a division's
    # kind and notes): words such as `altitude` or `wind` that a real name may also be.
    "disabledFilters", "hiddenFilters", "include", "convert", "convertToEvents", "followed",
    "primaryMeasure", "secondaryMeasure", "trackType", "Measure", "EventType", "template",
    "uncategorized", "homepage", "fmt", "m", "t", "short", "Note", "SubType", "StateName",
    "Country", "GradeDesc", "FieldMeasureType",
}
# Public numbers, never ids: event ids, a meet's own division ids, grades, seasons, counts, ranks
# and flags (marks are SORT_KEYS). Every other number that equals a real id of any kind fails the
# self-check; one of these that does is only counted (it is an event id or a rank that happens to
# equal an id) and reported in the summary.
PUBLIC_INT_KEYS = {
    "rank", "rowNum", "GradeID", "Grade", "EventID", "IDEvent", "IDEventType", "EventTypeID",
    "eventId", "SeasonID", "seasonId", "Year", "minCount", "IDDiv", "DivID", "divId", "d", "e", "sort",
    "Entry", "Level", "LevelMask", "AcceptanceStatus", "Source", "PersonalBest", "SeasonBest", "depth",
    "page", "numFound", "RelayAthletesRequired", "ConversionInt", "Round", "Wind", "FAT", "score",
    "IDSeason", "IDGrade", "SortID", "PublicList", "EditDepth", "RegionID", "EventSelectionID", "Filter",
    "Results", "DefaultSortOrder", "TeamRecords", "siteSupport", "PrefConvert", "PrefMetric", "level",
    "so", "visibility", "wind", "queryDepth", "priority", "depth_standardsListId", "meetList", "events_main",
    "windLimits", "convert", "convertToEvents", "include", "hiddenFilters", "disabledFilters", "sections",
}
# An `ID` under these parents is an event's id, public too.
PUBLIC_ID_PARENTS = {"events", "eventsTF"}
# The query a recorded search carries (`ath`'s `record::SEARCH_QUERY_FIELD`).
SEARCH_QUERY_KEY = "athQuery"
# Every key the anonymizer has reviewed, by response kind (navinfo is public structure, copied as
# is). A key outside its kind's list, or the location keys below, stops the run with its path
# named, so a new field Athletic.net adds is reviewed before anything is written.
ALLOWED_KEYS = {
    "bio": {
        "athlete", "IDAthlete", "SchoolID", "FirstName", "LastName", "Gender", "allTeams", "IDSchool",
        "SchoolName", "Level", "Year", "City", "State", "grades", "allSeasons", "ID", "Display",
        "eventsTF", "Event", "Type", "IDEvent", "IDEventType", "resultsTF", "IDResult", "AthleteID",
        "Result", "SortInt", "EventID", "EventTypeID", "SeasonID", "Exhibition", "ResultDate",
        "MeetID", "meetName", "PersonalBest", "SeasonBest", "Round", "FAT", "Wind", "meets", "IDMeet",
        "MeetName", "EndDate",
        # Seen in real bios (2026-11-09). Seasons: `IDSeason` moves like `ID`. Kept, no one in
        # them: flags and preferences (`Selected`, `NoSearch`, `isClaimed`, `canEdit`,
        # `hasOtherSport`, `level`, `PrefConvert`, `PrefMetric`, `Blurred`, `ignoreDay`,
        # `ignoreMonth`), events' public structure (`ConversionInt`, `DefaultSortOrder`,
        # `FieldMeasureType`, `PersonalEvent`), a result's meet division (`Division`,
        # `DivisionShort`: `Varsity`, `9th Grade`), empty containers (`photos`, `distancesXC`,
        # `resultsXC`, `unattachedCalendars`: their keys are reviewed in turn). Moved: the team
        # code (`TeamCode`, invented), a relay team's id (`TeamID`), `SortIntRaw` (a mark).
        # `relayTeamMembers` and `synonyms` hold people: see REVIEWED_PATHS and BLANKED_PATHS.
        "IDSeason", "Selected", "PrefConvert", "PrefMetric", "TeamCode", "NoSearch", "isClaimed",
        "canEdit", "hasOtherSport", "level", "distancesXC", "resultsXC", "photos",
        "unattachedCalendars", "ConversionInt", "DefaultSortOrder", "FieldMeasureType",
        "PersonalEvent", "relayTeamMembers", "SortID", "TeamID", "Blurred", "Division",
        "DivisionShort", "SortIntRaw", "ignoreDay", "ignoreMonth", "synonyms",
    },
    "rankings": {
        "eventShort", "division", "ID", "ParentID", "BaseDivID", "SeasonID", "Name", "minCount",
        "settings", "fat", "official", "grades", "page", "depth", "conversion", "groupedRankings",
        "relayTeams", "jwtTFTopReport", "rank", "rowNum", "AthleteID", "AthleteName", "GradeID",
        "display", "SortIntRaw", "SortIntCalc", "SortIntOrig", "TeamID", "TeamName", "ResultDate",
        "MeetID", "MeetName", "Event", "EventShort", "State", "defaultSettings", "includeDivisions",
        "sportRankings", "eventId", "gender", "eventType", "eventTypes", "reportTitle", "level",
        # Seen in real rankings pages (2026-11-09). Kept, public structure and settings: the
        # division's own record (`BaseDiv` and its flags, `Country`, `StateName`, `SubType`,
        # `Note`; its `ID` and `ParentID` move as list ids, LIST_ID_KEYS), its home page and
        # rankings settings, the page's filter settings, event types and their result counts, and
        # a row's result flags (`EventType`, `Measure`, `PersonalEvent`, `blurred`,
        # `isFieldSeries`, `ignoreDay`, `ignoreMonth`, `ConversionInt`). Moved: `IDResult` (an id).
        # A row's own details (age, country, handle, photo, mascot, result code, media counts)
        # are blanked: BLANKED_PATHS.
        "BaseDiv", "BestsRecordSetID_tfi", "BestsRecordSetID_tfo", "BestsRecordSetID_xc",
        "Country", "CustomDivision", "EditDepth", "Level", "LockTeamAlignment", "Major", "Note",
        "RegionID", "StateName", "SubType", "Type", "featureIndoor", "hasTrack", "hasTrackIndoor",
        "hasXC", "CustomRankingsSettings", "qualifying", "priority", "allFiltersDisabled", "altitude",
        "convert", "convertToEvents", "depth_standardsListId", "disabledFilters", "excludeMeets",
        "followed", "hiddenFilters", "include", "indoor", "meetList", "primaryMeasure", "restrict",
        "secondaryMeasure", "showItems", "trackType", "visibility", "wind", "windLimits",
        "EventSelectionID", "Filter", "HomeSettings", "divisionsSettings", "showFilter", "template",
        "uncategorized", "homepage", "sections", "showRankingsLink", "showTabs", "teamsSettings",
        "LastMajor", "RankingsSettings", "Settings", "TFRIEndDate", "TFRIRefreshInterval",
        "TFRISettings", "inherit", "queryDepth", "useNjcaaRules", "TFRIStartDate", "EventTypeID",
        "Results", "ConversionInt", "EventID", "EventType", "FAT", "IDResult", "Measure",
        "PersonalBest", "PersonalEvent", "Round", "Wind", "blurred", "ignoreDay", "ignoreMonth",
        "isFieldSeries", "hasSiteSupport", "bestsRecordSet", "customList", "debug", "recordSet",
    },
    "teamnav": {
        "team", "ID", "Name", "Level", "City", "State", "userAthleteIdOnTeam", "hasCoachAccess",
        "mascotToken2", "grades", "divisions", "id", "b", "name", "customLists", "customDivisions",
        "linkedTeams",
        # Seen in real TeamNav answers (2026-11-09). Kept: `Country`, `TeamRecords`, `hasIndoor`,
        # `siteSupport`, the grade list (`IDGrade`, `GradeDesc`), a division's `gender`, a custom
        # list's `PublicList` and `Type`. Moved: a linked team (`IDSchool`, `SchoolName`), a custom
        # list's id (`IDCustomList`, and the `customLists` keys), a custom division's id
        # (`IDDivision`, a list id), `ReplacementID`. The school's mascot, colors and photos, and
        # custom lists' and divisions' names, are blanked: BLANKED_PATHS.
        "Country", "ReplacementID", "TeamRecords", "hasIndoor", "siteSupport", "IDGrade",
        "GradeDesc", "gender", "IDSchool", "SchoolName", "IDCustomList", "PublicList", "Type",
        "IDDivision",
    },
    "meet": {
        "meet", "ID", "Name", "MeetDate", "StartDate", "EndDate", "SeasonID", "PublicEntries",
        "RegEnabled", "RelayAthletesRequired", "Location", "Indoor", "City", "State", "tfDivisions",
        "IDMeetDiv", "MeetID", "IDDiv", "Division", "DivisionShort", "LevelMask", "jwtMeet",
    },
    "events": {
        "events", "ID", "Event", "EventShort", "Gender", "Type", "Entry", "sort", "isPersonal",
        "eventDivsWithEntries", "e", "d", "ge_p", "eventDivsWithResults",
    },
    "entries": {
        "eventId", "currentEventValid", "entries", "IDAthlete", "FirstName", "LastName", "Gender",
        "TeamID", "EventID", "Letter", "SchoolID", "FAT", "Grade", "TeamName", "TeamCode", "DivID",
        "Comment", "Locked", "Scratched", "OverrideSortInt", "SortIntRaw", "ConversionInt", "SortInt",
        "AcceptanceStatus", "Source", "OwnerUserId", "OriginalEvent", "teams", "IDSchool",
        "SchoolName", "LevelName", "DivName", "ParentDivName", "GrandparentDivName", "events", "ID",
        "Event", "EventShort", "Type", "Entry", "sort", "isPersonal", "eventDivsWithEntries", "e",
        "d", "ge_p", "eventTypes", "rounds", "releyLegs",
    },
    "search": {"athQuery", "response", "numFound", "docs", "id_db", "type", "textsuggest", "subtext", "score"},
    "navinfo": {
        "divListId", "levels", "levelDivId", "seasons", "countries", "countryDivId", "regions",
        "regionDivId", "tree", "events", "events_main", "recordSets", "id", "name", "state", "divType",
        "subDivType", "depth", "customDivision",
        # An event's public definition (seen 2026-11-09): format, flags, short name, sort order.
        "fmt", "h", "m", "r", "short", "so", "t", "w",
    },
}
# Reviewed by their whole path where the key alone is too general to allow everywhere: a relay
# team member's `Name` is a person (invented like `AthleteName`); an event's `Description`
# (`[100-200-300-400]`) is public, its names replaced like any text.
REVIEWED_PATHS = {
    "bio": {"relayTeamMembers[]/Name", "eventsTF[]/Description"},
    "rankings": {"division/Description", "eventTypes[]/Description"},
}
# Blanked by their whole path (strings to "", numbers to 0, everything inside too), reviewed as
# identifying (2026-11-09): an age (with a grade, close to a birth date), a handle, a USATF
# membership id, a photo, a result's code (its public page) and media counts, a place in a race,
# a row's country (rare ones single someone out), the alias in `synonyms`, a school's mascot,
# colors and pictures (each names the school), a custom list's or division's name and
# description (free text), a division's logo and the year in a custom ranking's name.
BLANKED_PATHS = {
    "bio": {
        "allSeasons[]/age", "athlete/age", "athlete/Handle", "athlete/PhotoUrl", "athlete/rsMugshot",
        "athlete/UsatfId", "allTeams/<id>/MascotUrl", "resultsTF[]/Place", "resultsTF[]/shortCode",
        "resultsTF[]/MediaCount", "synonyms[]/RealFirstName",
    },
    "rankings": {
        "groupedRankings[][]/Age", "groupedRankings[][]/Country", "groupedRankings[][]/Handle",
        "groupedRankings[][]/PhotoUrl", "groupedRankings[][]/TeamMascot", "groupedRankings[][]/shortCode",
        "groupedRankings[][]/mediaCount", "groupedRankings[][]/MediaCountJson", "division/ImageUrl",
        "division/CustomRankingsSettings/qualifying/settings/name", "location",
    },
    "teamnav": {
        "team/Mascot", "team/MascotUrl", "team/colors", "team/coverPhoto", "customLists/<id>/Name",
        "customLists/<id>/Description", "customDivisions[]/DivName",
    },
}
# Season values (`2026`, indoor `12026`): moved into the fictional seasons.
SEASON_KEYS = {"SeasonID", "seasonId", "Year", "IDSeason"}
# A list's or division's id, by response kind and parent key: moved into the reserved range like
# a person's id, so a list cannot be matched with the public one by its id.
LIST_ID_KEYS = {
    "rankings": {"division": {"ID", "ParentID", "BaseDivID"}, "BaseDiv": {"ID", "ParentID"}},
    "teamnav": {"divisions": {"id", "b"}, "customDivisions": {"IDDivision"}},
    "navinfo": {None: {"divListId", "levelDivId", "countryDivId", "regionDivId"}, "regions": {"id"}, "tree": {"id"},
                "levels": {"id"}, "countries": {"id"}},
}
LATEST_SEASON = 2099
# Where someone or something is: blanked anywhere (strings to "", numbers to 0, every value
# inside an object or list too).
LOCATION_KEYS = {
    "Address", "Address1", "Address2", "Address3", "Street", "Street1", "Street2", "Zip", "ZipCode",
    "PostalCode", "Postal", "Lat", "Long", "Lng", "Latitude", "Longitude", "GoogleData", "GooglePlace",
    "PlaceID", "PlaceId", "Phone", "PhoneNumber", "Fax", "Email", "EmailAddress", "Website",
    "WebSite", "Twitter", "Instagram", "Facebook", "TikTok", "Youtube", "YouTube", "Snapchat",
    "Directions", "MapUrl", "MapURL",
}
# Links: an Athletic.net page link keeps its shape (its ids and names replaced); any other is
# blanked.
URL_KEYS = {"Url", "URL", "url", "Link", "link", "href", "Href"}
VENUE_KEYS = {"Venue", "VenueName"}
TOKEN_KEY = re.compile(r"(?i)^jwt|token")
DATE = re.compile(r"^(\d{4})-(\d{2})-(\d{2})((?:T\d{2}:\d{2}(?::\d{2}(?:\.\d+)?)?)?(?:Z|[+-]\d{2}:?\d{2})?)$")
X_ONLY = re.compile(r"^[Xx\s]+$")
BR = re.compile(r"(?i)(\s*<br\s*/?>\s*)")
WORD_DIGITS = re.compile(r"(?<![0-9])[0-9]{5,}(?![0-9])")

# ---------------------------------------------------------------------------------------------
# Marks.

_RUNNING = re.compile(r"^(\d+(?::\d+){0,2})(?:\.(\d+))?([A-Za-z]*)$")
_FEET = re.compile(r"^(\d+)-(\d+)(?:\.(\d+))?$")
_METRIC = re.compile(r"^(\d+)\.(\d+)m$")


def parse_running(display):
    """(milliseconds, decimals, suffix, had_minutes) of a running mark, or None."""
    m = _RUNNING.match(display.strip())
    if not m or m.group(3).lower() == "m":
        return None
    parts = [int(p) for p in m.group(1).split(":")]
    decimals = m.group(2) or ""
    seconds = 0
    for p in parts:
        seconds = seconds * 60 + p
    frac = int(decimals) * 10 ** (3 - len(decimals)) if decimals else 0
    if len(decimals) > 3:
        return None
    return seconds * 1000 + frac, len(decimals), m.group(3), len(parts)


def format_running(ms, decimals, suffix):
    unit = 10 ** (3 - decimals)
    if ms % unit:
        raise AnonymizeError("cannot show %d ms with %d decimals" % (ms, decimals))
    whole, frac = divmod(ms, 1000)
    hours, rest = divmod(whole, 3600)
    minutes, seconds = divmod(rest, 60)
    if hours:
        body = "%d:%02d:%02d" % (hours, minutes, seconds)
    elif minutes:
        body = "%d:%02d" % (minutes, seconds)
    else:
        body = "%d" % seconds
    if decimals:
        body += "." + ("%03d" % frac)[:decimals]
    return body + suffix


def parse_field(display):
    """(the mark in 10^-decimals inches, decimals, the inches' width) of `F-II.dd`, or None."""
    m = _FEET.match(display.strip())
    if not m:
        return None
    feet, inches, decimals = int(m.group(1)), m.group(2), m.group(3) or ""
    scale = 10 ** len(decimals)
    total = (feet * 12 + int(inches)) * scale + (int(decimals) if decimals else 0)
    return total, len(decimals), len(inches)


def format_field(total, decimals, width):
    scale = 10 ** decimals
    whole, frac = divmod(total, scale)
    feet, inches = divmod(whole, 12)
    body = "%d-%0*d" % (feet, width, inches)
    if decimals:
        body += "." + "%0*d" % (decimals, frac)
    return body


def is_mark_display(display):
    if not isinstance(display, str) or not display.strip():
        return False
    return bool(parse_running(display) or parse_field(display) or _METRIC.match(display.strip()))


_FIELD_WORDS = ("jump", "vault", "put", "shot", "discus", "javelin", "hammer", "throw", "weight")
_RUNNING_WORDS = ("meter", "mile", "relay", "hurdle", "steeple", "walk", "yard", "dash", "medley",
                  "shuttle", "smr", "dmr", "run")


def event_kind(name):
    """`running`, `field` or `points` (a multi-event's score) by the event's name, or None."""
    words = name.lower()
    if "athlon" in words or "points" in words:
        return "points"
    if any(w in words for w in _FIELD_WORDS):
        return "field"
    if any(w in words for w in _RUNNING_WORDS) or re.search(r"\d\s*m\b|\d+m", words):
        return "running"
    return None


def ceil_to(value, unit):
    return -(-value // unit) * unit


def running_ms(display):
    p = parse_running(display)
    return p[0] if p else None


def field_sort_int(display):
    """A feet-inches display as a SortInt (`20,000,000 - inches x 1000`), or None."""
    p = parse_field(display)
    if not p:
        return None
    total, decimals, _ = p
    return FIELD_BASE - total * 1000 // 10**decimals


def metric_sort_int(display):
    """A metric field display (`12.34m`) in the same SortInt space as feet and inches
    (`20,000,000 - inches x 1000`), or None."""
    m = _METRIC.match(display.strip())
    if not m:
        return None
    inches = Decimal(m.group(1) + "." + m.group(2)) / Decimal("0.0254")
    return FIELD_BASE - int((inches * 1000).to_integral_value())


class EventMap:
    """One event's marks, mapped: every distinct real mark (by its display value, `D`, in
    SortInt units) moves by its own `delta`, always toward a better mark, a whole number of the
    event's display `unit`s. A baseline of `base` units plus a seeded jitter per value, bounded so
    that two neighbours can never cross or meet: the order of every distinct value, and every tie,
    stays as recorded, while the gaps between marks do not."""

    def __init__(self, name, kind, unit, fine, base, deltas, label):
        self.name = name
        self.kind = kind  # running or field
        self.unit = unit  # SortInt units of one display step (the event's coarsest)
        self.fine = fine  # SortInt units of its finest display step
        self.base = base  # baseline, in units (negative: better)
        self.deltas = deltas  # D -> SortInt delta
        self.label = label

    def in_range(self, sort_int):
        if not isinstance(sort_int, int) or isinstance(sort_int, bool):
            return False
        if self.kind == "running":
            return 0 < sort_int < FIELD_FLOOR
        return FIELD_FLOOR <= sort_int < FIELD_BASE

    def key(self, display, sort_int=None):
        """A mark's key in the map: the SortInt the list sorts on (so a hand time keeps its own
        place against a FAT one); for a standard, which has none, its display on the same
        scale."""
        if isinstance(sort_int, int) and not isinstance(sort_int, bool) and self.in_range(sort_int):
            return sort_int
        if display is None:
            raise AnonymizeError("%s: a mark with neither a SortInt nor a display" % self.name)
        d = running_ms(display) if self.kind == "running" else field_sort_int(display)
        if d is None and self.kind == "field":
            d = metric_sort_int(display)
        if d is None:
            raise AnonymizeError("%s: %r is not a %s mark this tool can read" % (self.name, display, self.kind))
        return d

    def delta(self, d):
        try:
            return self.deltas[d]
        except KeyError:
            raise AnonymizeError("%s: no mapped value for %r (a mark shape the map did not see)" % (self.name, d))

    def display(self, display, delta):
        """`display` moved by `delta`, in its own format, or None when it is not a mark of this
        kind."""
        text = display.strip()
        lead = display[: len(display) - len(display.lstrip())]
        trail = display[len(display.rstrip()) :]
        if self.kind == "running":
            p = parse_running(text)
            if not p:
                return None
            ms, decimals, suffix, _ = p
            return lead + format_running(ms + delta, decimals, suffix) + trail
        p = parse_field(text)
        if p:
            total, decimals, width = p
            steps = -delta * 10**decimals
            if steps % 1000:
                raise AnonymizeError("a field shift (%d) finer than the display %r" % (delta, display))
            return lead + format_field(total + steps // 1000, decimals, width) + trail
        m = _METRIC.match(text)
        if m:
            decimals = len(m.group(2))
            meters = Decimal(m.group(1) + "." + m.group(2)) + Decimal(-delta) / 1000 * Decimal("0.0254")
            q = Decimal(1).scaleb(-decimals)
            return lead + str(meters.quantize(q, rounding=ROUND_FLOOR)) + "m" + trail
        return None


def jitter_deltas(points, unit, base, pick):
    """Each of the sorted distinct `points` (ints) -> its delta: `(base + j) x unit`, with `j`
    from `pick(point, lo, hi)` in a range that keeps the order strict. For neighbours `a < b`
    a gap `G = b - a` allows `j_a <= (G - unit) // (2 unit)` and `j_b >= -(that)`, so
    `(j_a - j_b) x unit <= G - unit` and the mapped values stay at least a unit apart (or, for
    values closer than a unit, keep their order); `j < -base` keeps every delta better."""
    deltas = {}
    n = -base
    for i, d in enumerate(points):
        down = (d - points[i - 1] - unit) // (2 * unit) if i > 0 else n
        up = (points[i + 1] - d - unit) // (2 * unit) if i + 1 < len(points) else n
        lo, hi = -min(max(0, down), n), min(max(0, up), n - 1)
        j = pick(d, lo, hi) if hi > lo else lo
        deltas[d] = (base + j) * unit
    return deltas


# Events whose marks `ath` converts between: a mile's estimate on the 1600m (`mile x 1600 /
# 1609.344`, on SortInt, rounded up to the next hundredth), compared with a 1600m.
LINKED = {"1 mile": ("1600 meters", (1600.0, 1609.344)), "2 mile": ("3200 meters", (3200.0, 3218.688)),
          "2 miles": ("3200 meters", (3200.0, 3218.688))}


def estimate(sort_int, factor):
    """`ath`'s estimate (`seed_relay::mile_to_1600m`): `mile x 1600 / 1609.344`, rounded up to
    the next hundredth."""
    num, den = factor
    return int(math.ceil(sort_int * num / den / 10.0)) * 10


# ---------------------------------------------------------------------------------------------
# Invented names.

FIRST_POOL = [
    "Adair", "Alden", "Amari", "Arden", "Ash", "Avery", "Bailey", "Blair", "Blake", "Brooke",
    "Cameron", "Campbell", "Carson", "Casey", "Charlie", "Dakota", "Dallas", "Darcy", "Drew",
    "Eden", "Elliot", "Ellis", "Emerson", "Emery", "Finley", "Frankie", "Gray", "Harley", "Harper",
    "Hayden", "Indigo", "Jamie", "Jessie", "Jody", "Jordan", "Jules", "Kai", "Keegan", "Kendall",
    "Kennedy", "Kerry", "Lane", "Lee", "Leighton", "Logan", "London", "Lou", "Mackenzie", "Marley",
    "Mason", "Micah", "Morgan", "Navy", "Noel", "Oakley", "Parker", "Payton", "Peyton", "Phoenix",
    "Presley", "Quinn", "Raleigh", "Reagan", "Reese", "Remy", "Riley", "River", "Robin", "Rowan",
    "Ryan", "Sage", "Sawyer", "Scout", "Shay", "Shiloh", "Sidney", "Skyler", "Spencer", "Stevie",
    "Sutton", "Taylor", "Teagan", "Toby", "Tracy", "Tyler", "Wesley", "Winter", "Wren", "Wynn",
]
_SURNAME_HEADS = [
    "Abber", "Bram", "Carn", "Dorr", "Ellis", "Fenn", "Garr", "Hask", "Ibber", "Jarr", "Kell",
    "Lamb", "Morr", "Nesb", "Ott", "Parr", "Quarr", "Rusk", "Sall", "Tamm", "Udd", "Vass", "Wedd",
    "Yarr", "Zell", "Brand", "Crow", "Dunn", "Ebb", "Frost", "Gill", "Holl", "Ives", "Kent",
    "Lyle", "Mott", "Nash", "Pell", "Rowe", "Sloane",
]
_SURNAME_MIDS = [
    "en", "er", "ing", "el", "ow", "an", "ett", "ard", "is", "ock", "ley", "ham", "ins", "ott",
    "ell", "ay", "ord", "um", "ers", "ick",
]
_SURNAME_TAILS = [
    "ton", "by", "wick", "ford", "dale", "well", "worth", "more", "stead", "holt", "mere", "combe",
    "field", "shaw", "brook", "hurst", "ridge", "wood", "low", "ham", "ley", "son", "cott", "den",
    "gate", "thorpe", "ville", "burn", "croft", "lund", "vale", "marsh", "moor", "ash", "end",
    "fell", "haven", "hill", "ings", "lake",
]
_PLACE_HEADS = [
    "Alder", "Amber", "Arrow", "Aspen", "Bay", "Beacon", "Birch", "Bramble", "Briar", "Cedar",
    "Cinder", "Clover", "Copper", "Coral", "Crane", "Crest", "Dapple", "Dove", "Elm", "Ember",
    "Fallow", "Fern", "Flint", "Fox", "Glen", "Golden", "Granite", "Hazel", "Heron", "Holly",
    "Iron", "Ivy", "Juniper", "Kestrel", "Lark", "Laurel", "Linden", "Maple", "Marsh", "Meadow",
    "Mill", "Mist", "Moss", "North", "Oak", "Otter", "Pebble", "Pine", "Quail", "Raven", "Reed",
    "Ridge", "Robin", "Rose", "Rowan", "Sable", "Sage", "Silver", "Sparrow", "Spruce", "Stone",
    "Swan", "Thistle", "Thorn", "Timber", "Willow", "Wren", "Yew",
]
_PLACE_TAILS = [
    "bank", "bourne", "brook", "by", "combe", "crest", "dale", "fell", "field", "ford", "gate",
    "haven", "holm", "hurst", "ley", "mere", "mont", "moor", "port", "ridge", "stead", "ton",
    "vale", "view", "wick", "wood", "worth",
]
_PLACE_SUFFIXES = ["", " Falls", " Springs", " Heights", " Hollow", " Landing", " Junction", " Point"]
SCHOOL_KINDS = [
    "High School", "Middle School", "Track Club", "Running Club", "Christian", "Catholic",
    "Charter", "Academy", "Prep", "Preparatory", "Senior High", "High", "School", "HS", "TC", "Club",
    "Athletics", "Elite", "Striders", "Cathedral",
]
MEET_KINDS = [
    "Invitational", "Invite", "Relays", "Classic", "Championships", "Championship", "Meet", "Open",
    "Games", "Opener", "Qualifier", "Finals", "Prelims", "Challenge", "Showcase", "Dual", "Tri",
    "Quad", "Festival", "Carnival", "Series",
]
VENUE_KINDS = ["Stadium", "Fieldhouse", "Field", "Track", "Complex", "Center", "Park", "Arena",
               "Dome", "High School", "Sports Center"]


def _surnames():
    out = [h + t for h in _SURNAME_HEADS for t in _SURNAME_TAILS]
    out += [h + m + t for h in _SURNAME_HEADS for m in _SURNAME_MIDS for t in _SURNAME_TAILS]
    return out


def _places():
    base = [h + t for h in _PLACE_HEADS for t in _PLACE_TAILS]
    return [b + s for s in _PLACE_SUFFIXES for b in base]


SURNAMES = _surnames()
PLACES = _places()


def split_kind(name, kinds):
    """(base, kind suffix with its leading space) for a name that ends in a known kind."""
    for kind in sorted(kinds, key=len, reverse=True):
        if name.endswith(" " + kind) and len(name) > len(kind) + 1:
            return name[: -len(kind) - 1], " " + kind
    return name, ""


def slug(text):
    """`ath`'s search fixture name: lower case, runs of other characters as one '-'."""
    return re.sub(r"[^a-z0-9]+", "-", text.lower()).strip("-")


def search_fixture_name(query):
    """`ath`'s `search/<name>.json` stem for a search query (`source::search_fixture_name`):
    lower-cased ASCII letters and digits, other runs as one `-`; unless that spelling is exact
    (ASCII letters and digits in words one space apart), the first 8 hex digits of the SHA-256 of
    the lower-cased query follow, so every query has its own non-empty name."""
    query = query.strip()
    out = ""
    for c in query:
        if c.isascii() and c.isalnum():
            out += c.lower()
        elif out and not out.endswith("-"):
            out += "-"
    out = out.rstrip("-")
    if not out or out.replace("-", " ") != query.lower():
        digest = hashlib.sha256(query.lower().encode("utf-8")).hexdigest()[:8]
        out = out + "-" + digest if out else digest
    return out


_TOKEN = re.compile(r"[A-Za-z0-9]+")


def norm(text):
    """A name as its words: lower case, anything between them one space (`Jane O'Doe`,
    `jane-o-doe` and `JANE O DOE` are all `jane o doe`)."""
    return " ".join(t.lower() for t in _TOKEN.findall(text))


class NameFinder:
    """Finds whole names (any of a set, as `norm` spells them) in text, by their words: fast for
    tens of thousands of names, and blind to case and punctuation, so `jane-doe` in a URL is
    found as `Jane Doe`."""

    def __init__(self, names, min_chars=4):
        self.names = {n for n in names if len(n.replace(" ", "")) >= min_chars}
        self.longest = max((n.count(" ") + 1 for n in self.names), default=0)

    def find(self, text):
        """(start, end, name) of each name in `text`, longest first, never overlapping."""
        if not self.names:
            return []
        tokens = list(_TOKEN.finditer(text))
        words = [t.group(0).lower() for t in tokens]
        found, i = [], 0
        while i < len(tokens):
            for n in range(min(self.longest, len(tokens) - i), 0, -1):
                key = " ".join(words[i : i + n])
                if key in self.names:
                    found.append((tokens[i].start(), tokens[i + n - 1].end(), key))
                    i += n
                    break
            else:
                i += 1
        return found


# ---------------------------------------------------------------------------------------------
# The anonymizer.


class AnonymizeError(Exception):
    pass


class Report:
    """What a run did: the maps (private: they undo the anonymization) and a summary."""

    def __init__(self):
        self.id_map = {}
        self.value_map = {}
        self.value_kinds = {}  # real value -> what it names (person, school, meet, ...)
        self.date_shift = 0
        self.year_shift = 0
        self.event_maps = {}
        self.standards = []
        self.dropped = {}
        self.inserted = {}
        self.grade_offsets = {}
        self.collisions = 0
        self.files = 0
        self.counts = {}

    def summary(self, source):
        lines = [
            "anonymized %d files with %s" % (self.files, source),
            "  ids: %d moved to %d..%d" % (len(self.id_map), RESERVED_BASE, RESERVED_BASE + RESERVED_SPAN - 1),
            "  names: %d replaced (%s)" % (
                sum(self.counts.get(k, 0) for k in ("person", "first", "last", "school", "meet", "venue", "city", "code")),
                ", ".join("%s %d" % (k, self.counts[k]) for k in sorted(self.counts) if k not in ("date", "mark", "id"))),
            "  lists: %d athletes dropped from every list, %d invented ones placed elsewhere" % (
                len(self.dropped), len(self.inserted)),
            "  grades: %d athletes swapped grades in pairs (counts per list and grade as recorded)" % len(self.grade_offsets),
            "  seasons: moved by %+d years (the latest outdoor season is %d)" % (self.year_shift, LATEST_SEASON),
            "  dates: %d moved by %+d years and %+d days" % (self.counts.get("date", 0), self.year_shift, self.date_shift),
            "  marks: %d moved, by event:" % self.counts.get("mark", 0),
        ]
        for name in sorted(self.event_maps):
            lines.append("    %-28s %s" % (name, self.event_maps[name].label))
        if self.collisions:
            lines.append("  note: %d public numbers (event ids, ranks, counts, marks) equal a real id; "
                         "they are not ids and stay" % self.collisions)
        if self.standards:
            lines.append("  State Meet standards: %d mapped with their events (no mark crosses one)" % len(self.standards))
        lines.append("  self-check: passed (no real name, id, date or mark left where it was; order, ties, "
                     "standards and mile estimates as recorded)")
        return "\n".join(lines)


class Anonymizer:
    def __init__(self, key, areas_dir=None, keep=(), latest=None):
        """`key`: the secret (bytes) every invented value and mark map is derived from; `keep`:
        athletes (real ids) never dropped from the lists; `latest`: the real outdoor season that
        becomes 2099 (by default the Recording's latest)."""
        self.latest = latest
        self.areas_dir = areas_dir
        self.protected = set(keep)
        self.key = key
        self.report = Report()
        # Collected from the recording.
        self.ids = set()
        self.first_names = {}
        self.last_names = {}
        self.pairs = {}  # lower full name -> (first, last)
        self.persons = set()
        self.schools = set()
        self.meets = set()
        self.venues = set()
        self.cities = set()
        self.codes = set()
        self.dates = []
        self.seasons = set()
        self.year_shift = 0
        self.event_names = {}  # event id -> name
        self.short_names = {}  # eventShort -> name
        self.marks = {}  # event name (lower) -> [(display or None, [sort ints])]
        self.marks_by_athlete = {}  # bio -> event name -> [(display or None, sort int)]
        self.standards = []
        self.real_values = set()  # every real name-like value, lower case
        # Built.
        self.maps = {}
        self.event_maps = {}
        self.date_shift = 0

    # -- hashing -------------------------------------------------------------------------------

    def h(self, *parts):
        msg = "\x1f".join(str(p) for p in parts).encode()
        return int.from_bytes(hmac.new(self.key, msg, hashlib.sha256).digest()[:8], "big")

    # -- files ---------------------------------------------------------------------------------

    @staticmethod
    def kind_of(rel):
        parts = rel.parts
        if len(parts) == 1 and re.fullmatch(r"\d+\.json", parts[0]):
            return "bio"
        if len(parts) == 2 and parts[0] == "teamnav" and re.fullmatch(r"\d+-\d+\.json", parts[1]):
            return "teamnav"
        if len(parts) == 2 and parts[0] == "navinfo":
            return "navinfo"
        if len(parts) == 2 and parts[0] == "rankings":
            return "rankings"
        if len(parts) == 3 and parts[0] == "meets" and parts[1].isdigit() and parts[2] in ("meet.json", "events.json"):
            return parts[2][:-5] if parts[2] == "meet.json" else "events"
        if len(parts) == 4 and parts[0] == "meets" and parts[1].isdigit() and parts[2] == "entries":
            return "entries"
        if len(parts) == 2 and parts[0] == "search":
            return "search"
        return None

    def read(self, recording):
        files = []
        for path in sorted(recording.rglob("*")):
            if path.is_dir():
                continue
            rel = path.relative_to(recording)
            if rel.name.startswith("."):
                continue  # dotfiles: ath's own temp files (`.<name>.<pid>.<n>.tmp`), markers
            if rel.suffix != ".json":
                raise AnonymizeError("%s is not part of a Recording's layout" % rel)
            kind = self.kind_of(rel)
            if kind is None:
                raise AnonymizeError("%s is not part of a Recording's layout (see docs/anonymize.md)" % rel)
            text = path.read_text(encoding="utf-8")
            try:
                tree = parse_json(text)
            except JsonError as e:
                raise AnonymizeError("%s: %s" % (rel, e))
            files.append({"rel": rel, "kind": kind, "text": text, "tree": tree})
        if not files:
            raise AnonymizeError("%s holds no Recording" % recording)
        return files

    # -- classify: one walk that says what each value is -----------------------------------------

    def actions(self, f):
        """(action, node, extra) for every value of `f` that the anonymizer changes or checks."""
        out = []
        page_event = None
        root = f["tree"]
        if f["kind"] == "rankings":
            short = root.get("eventShort")
            if short is not None and short.kind == "string":
                page_event = ("short", short.value)
        if f["kind"] == "entries":
            ev = root.get("eventId")
            if ev is not None and ev.kind == "number":
                page_event = ("id", ev.value)

        allowed = ALLOWED_KEYS[f["kind"]]
        reviewed = REVIEWED_PATHS.get(f["kind"], set())
        blanked = BLANKED_PATHS.get(f["kind"], set())

        def walk(node, path, parent_key, event):
            if node.kind == "object":
                keys = {k.value for k, _ in node.items}
                here = event
                ev = node.get("EventID")
                if ev is not None and ev.kind == "number":
                    here = ("id", ev.value)
                elif node.get("Event") is not None and node.get("Event").kind == "string":
                    here = ("name", node.get("Event").value)
                if keys & (DISPLAY_KEYS | SORT_KEYS):
                    disp = None
                    for k in DISPLAY_KEYS:
                        if node.get(k) is not None:
                            disp = node.get(k)
                    # The value the list sorts on first (`SortIntCalc` on a rankings row, the
                    # bio's `SortInt`): the mark's key in its event's map.
                    first = {"SortIntCalc": 0, "SortInt": 1}
                    sorts = [v for k, v in sorted((p for p in node.items if p[0].value in SORT_KEYS),
                                                  key=lambda p: first.get(p[0].value, 2)) if v.kind == "number"]
                    out.append(("mark", node, (here, disp, sorts)))
                first = node.get("FirstName") or node.get("firstName")
                last = node.get("LastName") or node.get("lastName")
                if first is not None and last is not None and first.kind == last.kind == "string":
                    out.append(("pair", node, (first.value, last.value)))
                doc_type = node.get("type")
                doc_type = doc_type.value if doc_type is not None and doc_type.kind == "string" else None
                # A division's name in a school's chain or the season's tree: kept for the
                # national, level, State and Area divisions only (`public_divisions`).
                if (f["kind"], parent_key) in (("teamnav", "divisions"), ("navinfo", "regions"), ("navinfo", "tree")):
                    name, ident = node.get("name"), node.get("id")
                    if name is not None and name.kind == "string" and ident is not None and ident.kind == "number":
                        get = lambda k: node.get(k).value if node.get(k) is not None else None
                        out.append(("divname", name, (ident.value, get("divType"), get("state"), f["kind"])))
                for k, v in node.items:
                    key = k.value
                    by_id = parent_key in ID_KEYED and key.isdigit()
                    if by_id and int(key) > 0:  # `0` (no school, unattached) stays `0`, as id 0 does
                        out.append(("idkey", k, None))
                    graded = parent_key == "grades" and re.fullmatch(r"\d+_\d+", key)
                    if graded:
                        out.append(("gradekey", k, None))
                    seasoned = f["kind"] == "navinfo" and parent_key == "seasons" and key.isdigit()
                    if seasoned:
                        out.append(("seasonkey", k, None))
                        if v.kind == "number" and isinstance(v.value, int) and v.value > 0:
                            out.append(("listid", v, None))
                        continue
                    where = path + (key if not (by_id or graded) else "<id>",)
                    if key in LOCATION_KEYS or (key in URL_KEYS and not athletic_link(v)) or (
                            key in VENUE_KEYS and v.kind != "string") or "/".join(where) in blanked:
                        out.extend(("scrub", leaf, None) for leaf in scalars(v))
                        continue
                    if not (by_id or graded or key in URL_KEYS or key in VENUE_KEYS
                            or TOKEN_KEY.search(key) or key in allowed or "/".join(where) in reviewed):
                        self.unknown.append((f["rel"], f["kind"], "/".join(where), v))
                    self.classify(key, v, parent_key, doc_type, out, f["kind"])
                    walk(v, where, key, here)
            elif node.kind == "array":
                for item in node.items:
                    walk(item, path[:-1] + (path[-1] + "[]",) if path else path, parent_key, event)

        walk(root, (), None, page_event)
        return out

    def classify(self, key, v, parent_key, doc_type, out, kind=None):
        by_parent = LIST_ID_KEYS.get(kind, {})
        if key in by_parent.get(parent_key, ()) or (key in by_parent.get(None, ()) and parent_key is None):
            if v.kind == "number" and isinstance(v.value, int) and v.value > 0:
                out.append(("listid", v, None))
            return
        if (key in SEASON_KEYS or (key == "ID" and parent_key == "allSeasons")) and v.kind == "number":
            if isinstance(v.value, int) and v.value > 0:
                out.append(("season", v, None))
            return
        if key == "Display" and parent_key == "allSeasons" and v.kind == "string":
            out.append(("seasontext", v, None))
            return
        if v.kind == "string" and TOKEN_KEY.search(key):
            if v.value:
                out.append(("blank", v, None))
            return
        if key in ID_KEYS or (key == "ID" and parent_key in ID_PARENTS):
            if v.kind == "number" and isinstance(v.value, int) and v.value > 0:
                out.append(("id", v, None))
            elif v.kind == "string" and v.value.isdigit() and int(v.value) > 0:
                out.append(("id", v, None))
            return
        if v.kind != "string" or not v.value.strip() or X_ONLY.match(v.value):
            return
        if DATE.match(v.value):
            out.append(("date", v, None))
            return
        if key in KEEP_KEYS or key in DISPLAY_KEYS:
            return
        if key in FIRST_KEYS:
            out.append(("first", v, None))
        elif key in LAST_KEYS:
            out.append(("last", v, None))
        elif key in PERSON_KEYS or (key == "Name" and parent_key == "relayTeamMembers"):
            out.append(("person", v, None))
        elif key in SCHOOL_KEYS or (key == "Name" and parent_key == "team"):
            out.append(("school", v, None))
        elif key in VENUE_KEYS:
            out.append(("venue", v, None))
        elif key in URL_KEYS:
            out.append(("text", v, None))
        elif key in MEET_KEYS or (key == "Name" and parent_key == "meet"):
            out.append(("meet", v, None))
        elif key == "Name" and parent_key == "Location":
            out.append(("venue", v, None))
        elif key in CITY_KEYS:
            out.append(("city", v, None))
        elif key in CODE_KEYS:
            out.append(("code", v, None))
        elif key in FREE_TEXT_KEYS:
            out.append(("blank", v, None))
        elif key in DIVISION_NAME_KEYS:
            out.append(("division", v, None))
        elif key == "textsuggest":
            kind = {"athlete": "person", "team": "school"}.get((doc_type or "").lower())
            if kind is None and "meet" in (doc_type or "").lower():
                kind = "meet"
            out.append((kind or "text", v, None))
        elif key == "subtext":
            out.append(("subtext", v, doc_type))
        elif key == SEARCH_QUERY_KEY:
            out.append(("person", v, None))
        elif key == "Name":
            return  # a division's or list's own name
        else:
            out.append(("text", v, None))

    # -- collect -----------------------------------------------------------------------------

    def collect(self, files):
        self.unknown = []
        for f in files:
            self.collect_events(f)
        for f in files:
            rel = f["rel"]
            if f["kind"] == "bio":
                self.ids.add(int(rel.stem))
                self.protected.add(int(rel.stem))
            elif f["kind"] == "entries":
                entries = f["tree"].get("entries")
                for e in (entries.items if entries is not None and entries.kind == "array" else ()):
                    who = e.get("IDAthlete") if e.kind == "object" else None
                    if who is not None and who.kind == "number" and who.value:
                        self.protected.add(who.value)
            elif f["kind"] == "teamnav":
                self.ids.add(int(rel.stem.split("-")[0]))
                self.seasons.add(int(rel.stem.split("-")[1]))
            elif f["kind"] == "navinfo":
                bits = rel.stem.split("-")
                self.seasons.add(int(bits[0]))
                if len(bits) > 1:
                    self.ids.add(int(bits[1]))
            elif f["kind"] == "rankings":
                self.ids.add(int(rel.stem.split("-")[0]))
            elif f["kind"] in ("meet", "events", "entries"):
                self.ids.add(int(rel.parts[1]))
            f["actions"] = self.actions(f)
            for action, node, extra in f["actions"]:
                self.collect_one(action, node, extra)
                if action == "mark" and f["kind"] == "bio":
                    self.collect_athlete_mark(str(f["rel"]), extra)
        for full, (first, last) in self.pairs.items():
            self.persons.add(full)
        if self.unknown:
            known = reviewed_literals()
            paths = sorted({"%s: %s" % (kind, masked_path(where, known)) for _, kind, where, _ in self.unknown})
            shown = "\n  ".join(paths[:40])
            more = "\n  ... and %d more" % (len(paths) - 40) if len(paths) > 40 else ""
            raise AnonymizeError(
                "fields no one has reviewed, nothing written (add each to ALLOWED_KEYS once it is "
                "handled, see docs/anonymize.md; --list-unreviewed lists them):\n  " + shown + more)

    def collect_events(self, f):
        def walk(node):
            if node.kind == "object":
                # An event's id: `IDEvent` (a bio's events), `EventID` (a rankings row, which
                # names its event too: a Recording with no bio still knows the row's event), or
                # the `ID` of an event list's entry (one with an `EventShort`).
                ident = node.get("IDEvent") or node.get("EventID") or node.get("ID")
                name = node.get("Event")
                short = node.get("EventShort")
                if name is not None and name.kind == "string":
                    if ident is not None and ident.kind == "number" and (
                            node.get("IDEvent") is not None or node.get("EventID") is not None
                            or node.get("EventShort") is not None):
                        self.event_names.setdefault(ident.value, name.value)
                    if short is not None and short.kind == "string":
                        self.short_names.setdefault(short.value, name.value)
                for _, v in node.items:
                    walk(v)
            elif node.kind == "array":
                for item in node.items:
                    walk(item)

        if f["kind"] != "navinfo":
            walk(f["tree"])

    def event_of(self, ref):
        if ref is None:
            return None
        how, value = ref
        if how == "id":
            name = self.event_names.get(value)
        elif how == "short":
            name = self.short_names.get(value, value)
        else:
            name = value
        return name.strip().lower() if name else None

    def collect_one(self, action, node, extra):
        v = node.value
        if action == "id":
            self.ids.add(int(v))
        elif action == "idkey":
            self.ids.add(int(v))
        elif action == "gradekey":
            if int(v.split("_")[0]) > 0:
                self.ids.add(int(v.split("_")[0]))
            self.seasons.add(int(v.split("_")[1]))
        elif action == "listid":
            self.ids.add(int(v))
        elif action == "season":
            self.seasons.add(int(v))
        elif action == "seasonkey":
            # A division tree's list of seasons, which runs ahead of any data (next season):
            # not data, but a season the Recording knows of.
            self.__dict__.setdefault("tree_seasons", set()).add(int(v))
        elif action == "pair":
            first, last = extra
            if first.strip() and last.strip():
                self.pairs[(first.strip() + " " + last.strip()).lower()] = (first.strip(), last.strip())
        elif action == "first":
            self.first_names[v.strip().lower()] = v.strip()
        elif action == "last":
            self.last_names[v.strip().lower()] = v.strip()
        elif action == "person":
            for part in BR.split(v):
                if part.strip() and not BR.fullmatch(part) and not X_ONLY.match(part):
                    self.persons.add(part.strip().lower())
        elif action in ("school", "meet", "venue", "city", "code"):
            getattr(self, {"school": "schools", "meet": "meets", "venue": "venues", "city": "cities", "code": "codes"}[action]).add(v.strip())
        elif action == "subtext":
            school, city = self.subtext_parts(v, extra)
            if school:
                self.schools.add(school)
            if city:
                self.cities.add(city)
        elif action == "date":
            self.dates.append(v)
        elif action == "mark":
            event, disp, sorts = extra
            name = self.event_of(event)
            if name is None:
                return
            if event_kind(name) == "points":
                return  # a multi-event's score: no mark, left as it is
            entry = self.marks.setdefault(name, [])
            values = [s.value for s in sorts[:1] if isinstance(s.value, int) and 0 < s.value < FIELD_BASE]
            if disp is not None and disp.kind == "string" and disp.value.strip():
                if is_mark_display(disp.value):
                    entry.append((disp.value.strip(), values))
            elif values:
                entry.append((None, values))

    def collect_athlete_mark(self, bio, extra):
        event, disp, sorts = extra
        name = self.event_of(event)
        sort = next((x.value for x in sorts if isinstance(x.value, int)), None)
        if name is None or sort is None or not 0 < sort < FIELD_BASE or event_kind(name) == "points":
            return
        display = disp.value.strip() if disp is not None and disp.kind == "string" else None
        if display is not None and not is_mark_display(display):
            return
        self.marks_by_athlete.setdefault(bio, {}).setdefault(name, []).append((display, sort))

    @staticmethod
    def subtext_parts(text, doc_type):
        """(school, city) named in a search result's `School (HS)||City, ST` subtext."""
        if "||" not in text:
            return None, None
        left, right = text.split("||", 1)
        school = None
        if (doc_type or "").lower() == "athlete":
            school = re.sub(r"\s*\([^)]*\)\s*$", "", left).strip() or None
        city = right.rsplit(",", 1)[0].strip() if right.strip() else None
        return school, city

    # -- build the maps ------------------------------------------------------------------------

    def build(self):
        reals = set()
        for group in (self.persons, self.schools, self.meets, self.venues, self.cities, self.codes):
            reals.update(x.lower() for x in group)
        reals.update(self.first_names)
        reals.update(self.last_names)
        self.real_values = reals
        self.build_ids()
        self.build_seasons()
        self.build_dates()
        self.build_marks()

    def build_ids(self):
        taken = set(self.ids)
        for real in sorted(self.ids):
            n = self.h("id", real) % RESERVED_SPAN
            while RESERVED_BASE + n in taken:
                n = (n + 1) % RESERVED_SPAN
            new = RESERVED_BASE + n
            taken.add(new)
            self.report.id_map[real] = new

    def clashes(self, invented):
        low = invented.lower()
        if low in self.real_values:
            return True
        words = low.split()
        for i in range(len(words)):
            for j in range(i + 1, len(words) + 1):
                if " ".join(words[i:j]) in self.real_values:
                    return True
        return False

    def invent(self, kind, real, pool, used, make=lambda x: x, avoid_real=True):
        """A deterministic invented value for `real`, `make` of a `pool` value (which adds a kept
        kind, such as `High School`). Never `real` itself; never with a real whole name in its
        invented part (what the self-check forbids); with `avoid_real`, never a real value of any
        kind as a whole or (the invented part) word by word; and (when `used` is a set) never
        another real value's."""
        key = (kind, real.lower())
        if key in self.maps:
            return self.maps[key]
        self.replacements()
        forbidden = self._finder
        n = self.h(kind, real.lower())
        for probe in range(len(pool) * 4):
            base = pool[(n + probe) % len(pool)]
            candidate = make(base)
            if candidate.lower() == real.lower() or forbidden.find(base) or norm(candidate) in forbidden.names:
                continue
            if avoid_real and (self.clashes(base) or candidate.lower() in self.real_values):
                continue
            if used is not None and candidate.lower() in used:
                continue
            break
        else:
            raise AnonymizeError("ran out of invented %s names" % kind)
        if used is not None:
            used.add(candidate.lower())
        self.maps[key] = candidate
        return candidate

    def first(self, real):
        # A first name alone names nobody: any but the real one will do.
        return self.invent("first", real.strip(), FIRST_POOL, None, avoid_real=False)

    def last(self, real):
        return self.invent("last", real.strip(), SURNAMES, self._used("last"))

    def _used(self, kind):
        return self.__dict__.setdefault("_used_" + kind, set())

    def person(self, real):
        parts = BR.split(real)
        out = []
        for part in parts:
            if not part.strip() or BR.fullmatch(part) or X_ONLY.match(part):
                out.append(part)
                continue
            lead = part[: len(part) - len(part.lstrip())]
            trail = part[len(part.rstrip()) :]
            name = part.strip()
            pair = self.pairs.get(name.lower())
            if pair is None:
                bits = name.split(None, 1)
                pair = (bits[0], bits[1]) if len(bits) == 2 else (None, bits[0])
            first, last = pair
            new = (self.first(first) + " " if first else "") + self.last(last)
            out.append(lead + new + trail)
        return "".join(out)

    def school(self, real):
        base, kind = split_kind(real.strip(), SCHOOL_KINDS)
        return self.invent("school", real.strip(), PLACES, self._used("school"), lambda p: p + kind)

    def meet(self, real):
        name = real.strip()
        year = re.search(r"(^\d{4}\s+|\s+\d{4}$)", name)
        bare = re.sub(r"(^\d{4}\s+|\s+\d{4}$)", "", name)
        _, kind = split_kind(bare, MEET_KINDS)
        kind = kind or " Invitational"

        def make(place):
            text = place + kind
            if year:
                moved = re.sub(r"\d{4}", lambda y: str(self.season(y.group(0))), year.group(1))
                text = (moved + text) if year.start() == 0 else (text + moved)
            return text

        return self.invent("meet", name, PLACES, self._used("meet"), make)

    def venue(self, real):
        _, kind = split_kind(real.strip(), VENUE_KINDS)
        return self.invent("venue", real.strip(), PLACES, self._used("venue"), lambda p: p + (kind or " Field"))

    def city(self, real):
        return self.invent("city", real.strip(), PLACES, self._used("city"))

    def code(self, real):
        width = max(2, min(6, len(real.strip())))
        letters = "ABCDEFGHJKLMNPQRSTUVWXYZ"
        pool = []
        for i in range(400):
            m = self.h("code", real.strip(), i)
            pool.append("".join(letters[(m >> (5 * j)) % len(letters)] for j in range(width)))
        return self.invent("code", real.strip(), pool, self._used("code"))

    def build_seasons(self):
        """Every season moves by one number of years: the latest outdoor season with data (or
        `latest`) to 2099 (when there is none, the latest indoor season's year); indoor seasons
        (10000 + year) and every date's year move by the same number, so order and gaps hold. A
        division tree's list of seasons (which already has next season) does not count: its
        later seasons move past 2099."""
        if self.latest is not None:
            outdoor = {x for x in self.seasons if x < 10000}
            known = outdoor | {x for x in getattr(self, "tree_seasons", ()) if x < 10000}
            if not 1900 <= self.latest < 10000:
                raise AnonymizeError("--latest-season %d is not an outdoor season (a year; indoor seasons are "
                                     "10000 and up)" % self.latest)
            if self.latest not in known:
                raise AnonymizeError("--latest-season %d is not a season of this Recording (it has %s)"
                                     % (self.latest, ", ".join(str(x) for x in sorted(known)) or "none"))
            if outdoor and self.latest < max(outdoor):
                raise AnonymizeError("--latest-season %d would move the Recording's %d data past %d"
                                     % (self.latest, max(outdoor), LATEST_SEASON))
            self.year_shift = LATEST_SEASON - self.latest
            self.report.year_shift = self.year_shift
            return
        outdoor = [x for x in self.seasons if x < 10000]
        years = outdoor or [x - 10000 for x in self.seasons if x >= 10000]
        if not years:
            years = [int(DATE.match(d).group(1)) for d in self.dates] or [LATEST_SEASON]
        self.year_shift = LATEST_SEASON - max(years)
        self.report.year_shift = self.year_shift

    def season(self, value):
        return int(value) + self.year_shift

    def move_year(self, d):
        try:
            return d.replace(year=d.year + self.year_shift)
        except ValueError:  # Feb 29 in a year that has none
            return d.replace(year=d.year + self.year_shift, day=28)

    def build_dates(self):
        parsed = []
        for d in self.dates:
            m = DATE.match(d)
            parsed.append(self.move_year(datetime.date(int(m.group(1)), int(m.group(2)), int(m.group(3)))))
        prefer = 1 + self.h("days") % 3
        sign = 1 if self.h("days-sign") % 2 else -1
        order = []
        for magnitude in [prefer] + [m for m in (3, 2, 1) if m != prefer]:
            order += [sign * magnitude, -sign * magnitude]
        for shift in order:
            delta = datetime.timedelta(days=shift)
            if all((d + delta).year == d.year for d in parsed):
                self.date_shift = shift
                break
        else:
            raise AnonymizeError(
                "no date shift of 1 to 3 days keeps every date in its year (the recording has dates "
                "on both Jan 1 and Dec 31); split the recording")
        self.report.date_shift = self.date_shift

    def shift_date(self, text):
        m = DATE.match(text)
        d = self.move_year(datetime.date(int(m.group(1)), int(m.group(2)), int(m.group(3))))
        d += datetime.timedelta(days=self.date_shift)
        return "%04d-%02d-%02d%s" % (d.year, d.month, d.day, m.group(4))

    def build_marks(self):
        self.standards = load_standards(self.areas_dir) if self.areas_dir else []
        shown = {}  # event -> its standards' displays, which its display unit must fit too
        for std in self.standards:
            name = self.standard_event(std["code"])
            if name in self.marks and is_mark_display(std["display"]):
                shown.setdefault(name, []).append(std["display"].strip())
        plans = {name: self.plan(name, raw, shown.get(name, [])) for name, raw in sorted(self.marks.items())}
        for std in self.standards:
            name = self.standard_event(std["code"])
            plan = plans.get(name)
            if plan is not None:
                d = plan["probe"].key(std["display"], None) if is_mark_display(std["display"]) else None
                if d is not None and plan["probe"].in_range(d):
                    std["event"], std["d"] = name, d
                    plan["points"].add(d)
        linked = {name: LINKED[name] for name in plans if name in LINKED and LINKED[name][0] in plans}
        for name, (base_name, factor) in linked.items():
            for d in plans[name]["points"]:
                plans[base_name]["points"].add(estimate(d, factor))
        for salt in range(50):
            maps = {}
            for name, plan in plans.items():
                if name not in linked:
                    maps[name] = self.make_map(name, plan, salt)
            for name, (base_name, factor) in linked.items():
                maps[name] = self.linked_map(name, plans[name], maps[base_name], factor)
            self.event_maps = maps
            if not self.estimate_flips(linked) and not any(map_disorder(maps[n], plans[n]["points"]) for n in linked):
                break
        else:
            raise AnonymizeError("no jitter keeps every mile-to-1600m estimate decision as recorded")
        for std in self.standards:
            if "event" in std:
                m = maps[std["event"]]
                std["new"] = m.display(std["display"], m.delta(std["d"]))
        self.report.event_maps = maps
        self.report.standards = [s for s in self.standards if "new" in s]

    def plan(self, name, raw, extra_displays=()):
        displays = [d for d, _ in raw if d is not None] + list(extra_displays)
        values = [v for _, vs in raw for v in vs]
        kind = event_kind(name)
        if kind is None:
            # A name that does not say: a time has a `:` or decimals, never a bare number.
            if any(parse_field(d) or _METRIC.match(d) for d in displays):
                kind = "field"
            elif displays and all(re.search(r"[:.]", d) for d in displays):
                kind = "running"
            elif not displays and values:
                kind = "field" if statistics.median(values) >= FIELD_FLOOR else "running"
            else:
                raise AnonymizeError("cannot tell what kind of event %r is (marks %r)" % (name, displays[:3]))
        if kind == "running":
            displays = [d for d in displays if parse_running(d)]
        field = kind == "field"
        if field:
            decimals = [parse_field(d)[1] for d in displays if parse_field(d)]
            quarters = decimals and min(decimals) >= 2 and all(
                field_sort_int(d) % 250 == 0 for d in displays if parse_field(d))
            unit = 1000 if not decimals or min(decimals) == 0 else 250 if quarters else 10 ** (3 - min(decimals))
            fine = 10 ** (3 - max(decimals)) if decimals else 1000
            if any(_METRIC.match(d) for d in displays):
                unit = max(unit, 1000)
            kind = "field"
        else:
            decimals = [parse_running(d)[1] for d in displays if parse_running(d)]
            unit = 10 ** (3 - min(decimals)) if decimals else 100
            fine = 10 ** (3 - max(decimals)) if decimals else 10
            kind = "running"
        probe = EventMap(name, kind, unit, fine, 0, {}, "")
        points = set()
        for d, vs in raw:
            points.add(probe.key(d, vs[0] if vs else None))
        return {"kind": kind, "unit": unit, "fine": fine, "probe": probe, "points": points, "values": values}

    def make_map(self, name, plan, salt):
        unit, kind = plan["unit"], plan["kind"]
        p = 0.004 + 0.008 * (self.h("mark", name) % 1000) / 1000.0
        if kind == "field":
            inches = [(FIELD_BASE - v) / 1000.0 for v in plan["values"] if FIELD_FLOOR <= v < FIELD_BASE]
            middle = (statistics.median(inches) if inches else 100.0) * 1000
        else:
            ms = [v for v in plan["values"] if 0 < v < FIELD_FLOOR]
            middle = statistics.median(ms) if ms else 60000
        n = max(1, int(round(middle * p / unit)))
        deltas = jitter_deltas(sorted(plan["points"]), unit, -n,
                               lambda d, lo, hi: lo + self.h("jitter", name, salt, d) % (hi - lo + 1))
        what = "%g in" % (n * unit / 1000.0) if kind == "field" else "%g s" % (n * unit / 1000.0)
        label = "%s better, then each mark jittered by whole %s" % (
            what, "%g in" % (unit / 1000.0) if kind == "field" else "%g s" % (unit / 1000.0))
        return EventMap(name, kind, unit, plan["fine"], -n, deltas, label)

    def linked_map(self, name, plan, base_map, factor):
        """A mile moves as its 1600m estimate does: delta = the estimate's delta / factor,
        rounded to the mile's display unit, never zero."""
        unit = plan["unit"]
        probe = plan["probe"]
        deltas = {}
        for d in sorted(plan["points"]):
            e = estimate(d, factor)
            delta = int(round(base_map.delta(e) * factor[1] / factor[0] / unit)) * unit
            deltas[d] = min(delta, -unit)
        return EventMap(name, "running", unit, probe.fine, None, deltas, "with the %s, through the estimate" % base_map.name)

    def estimate_flips(self, linked):
        """Whether any athlete's mile estimate would compare with one of their 1600m marks
        otherwise than recorded (`ath` uses a converted mile only when it is faster)."""
        for name, (base_name, factor) in linked.items():
            mile, base = self.event_maps[name], self.event_maps[base_name]
            for marks in self.marks_by_athlete.values():
                for m_disp, m in marks.get(name, ()):
                    for q_disp, q in marks.get(base_name, ()):
                        before = estimate(m, factor) - q
                        after = (estimate(m + mile.delta(mile.key(m_disp, m)), factor)
                                 - (q + base.delta(base.key(q_disp, q))))
                        if (before > 0) != (after > 0) or (before == 0) != (after == 0):
                            return True
        return False

    def standard_event(self, code):
        name = self.short_names.get(code)
        if name is None:
            m = re.fullmatch(r"(\d+)m", code)
            mile = re.fullmatch(r"(?i)([12]?)\s*mile", code)
            name = "%s Meters" % m.group(1) if m else "%s Mile" % (mile.group(1) or "1") if mile else None
        return name.strip().lower() if name else None

    # -- rewrite -------------------------------------------------------------------------------

    def new_id(self, real):
        return 0 if int(real) == 0 else self.report.id_map[int(real)]

    def count(self, what, n=1):
        self.report.counts[what] = self.report.counts.get(what, 0) + n

    def rewrite(self, f):
        edits = []
        checks = []  # (node, why) values that must change
        for action, node, extra in f["actions"]:
            v = node.value
            new = None
            if action == "id":
                new = encode(self.new_id(v) if node.kind == "number" else str(self.new_id(v)))
                self.count("id")
            elif action == "idkey":
                new = encode(str(self.new_id(v)))
            elif action == "gradekey":
                school, season = v.split("_", 1)
                new = encode("%d_%d" % (self.new_id(school), self.season(season)))
            elif action == "listid":
                new = encode(self.new_id(v))
            elif action == "season":
                new = encode(self.season(v))
            elif action == "seasonkey":
                new = encode(str(self.season(v)))
            elif action == "seasontext":
                new = encode(re.sub(r"\b(19|20|21)\d{2}\b", lambda y: str(self.season(y.group(0))), v))
            elif action == "blank":
                new = encode("")
            elif action == "scrub":
                if node.kind == "string":
                    new = encode("") if v else None
                elif node.kind == "number":
                    new = "0.0" if isinstance(v, float) else "0"
                    new = None if v == 0 else new
            elif action == "date":
                if self.date_shift:
                    new = encode(self.shift_date(v))
                    self.count("date")
            elif action in ("first", "last", "person", "school", "meet", "venue", "city", "code"):
                stripped = v.strip()
                lead, trail = v[: len(v) - len(v.lstrip())], v[len(v.rstrip()) :]
                new = encode(lead + getattr(self, action)(stripped) + trail)
                self.report.value_map[v] = json.loads(new)
                self.report.value_kinds.setdefault(v, set()).add(action)
                self.count(action)
            elif action == "division":
                if v.strip() in self.schools:
                    new = encode(self.school(v.strip()))
                elif v.strip() and v.strip() not in self.public_div_names:
                    new = encode("")  # a league or conference: below the Area
            elif action == "divname":
                if extra[0] not in self.public_div_ids and v.strip() not in self.public_div_names and v.strip():
                    new = encode("")
            elif action == "subtext":
                new = encode(self.rewrite_subtext(v, extra))
            elif action == "text":
                replaced = self.rewrite_text(v)
                if replaced != v:
                    new = encode(replaced)
            elif action == "mark":
                edits += self.rewrite_mark(extra, checks)
                continue
            elif action == "pair":
                continue
            if new is not None:
                edits.append((node.start, node.end, new))
            if action in ("id", "idkey", "gradekey", "listid", "season", "seasonkey", "first", "last", "person", "school", "meet", "venue", "city", "code") or (action == "date" and self.date_shift):
                checks.append((node, action))
        return edits, checks

    def rewrite_mark(self, extra, checks):
        event, disp, sorts = extra
        name = self.event_of(event)
        emap = self.event_maps.get(name) if name else None
        edits = []
        display = None
        if name and event_kind(name) == "points":
            return edits
        if disp is not None and disp.kind == "string" and disp.value.strip():
            if not is_mark_display(disp.value):
                return edits  # DNS, DNF, NT, SCR, DQ, FS ...: no mark, its SortInt a sentinel
            if emap is None:
                raise AnonymizeError("a mark (%r) whose event is unknown" % disp.value)
            display = disp.value.strip()
        elif emap is None:
            if any(isinstance(s.value, int) and 0 < s.value < FIELD_BASE for s in sorts):
                raise AnonymizeError("a mark (SortInt %r) whose event is unknown" % sorts[0].value)
            return edits
        in_range = [s for s in sorts if emap.in_range(s.value)]
        if display is None and not in_range:
            return edits
        delta = emap.delta(emap.key(display, in_range[0].value if in_range and in_range[0] is sorts[0] else None))
        if display is not None:
            new_disp = emap.display(disp.value, delta)
            if new_disp is None:
                raise AnonymizeError("cannot shift %r as a %s mark of %s" % (disp.value, emap.kind, name))
            edits.append((disp.start, disp.end, encode(new_disp)))
            checks.append((disp, "mark"))
            self.count("mark")
        for s in in_range:
            edits.append((s.start, s.end, encode(s.value + delta)))
            checks.append((s, "mark"))
        return edits

    def rewrite_subtext(self, text, doc_type):
        if "||" not in text:
            return self.rewrite_text(text)
        left, right = text.split("||", 1)
        school, city = self.subtext_parts(text, doc_type)
        if school:
            left = left.replace(school, self.school(school), 1)
        else:
            left = self.rewrite_text(left)
        if city:
            right = right.replace(city, self.city(city), 1)
        return left + "||" + right

    def replacements(self):
        """Every real whole name the recording holds (a person's full name, a school, meet,
        venue or city), by `norm`, with what replaces it."""
        if getattr(self, "_replacements", None) is None:
            names = {}
            for group, fn in ((self.cities, self.city), (self.venues, self.venue), (self.meets, self.meet),
                              (self.schools, self.school)):
                for real in sorted(group):
                    names[norm(real)] = (fn, real)
            for full in sorted(self.persons):
                names[norm(full)] = (self.person, full)
            self._replacements = names
            self._finder = NameFinder(names)
        return self._replacements

    def rewrite_text(self, text):
        """Free text in a field the anonymizer does not know: every real whole name in it (also
        as a URL writes it, `jane-doe`), and every real id written as digits, replaced."""
        names = self.replacements()
        out, last = [], 0
        for start, end, key in self._finder.find(text):
            fn, real = names[key]
            found = text[start:end]
            new = fn(real)
            out.append(text[last:start])
            out.append(slug(new) if found == slug(found) and "-" in found else new)
            last = end
        out.append(text[last:])
        text = "".join(out)
        return WORD_DIGITS.sub(lambda m: str(self.report.id_map.get(int(m.group(0)), m.group(0))), text)

    def rename(self, f):
        rel = f["rel"]
        parts = list(rel.parts)
        if f["kind"] == "bio":
            parts[0] = "%d.json" % self.new_id(rel.stem)
        elif f["kind"] == "teamnav":
            school, season = rel.stem.split("-", 1)
            parts[1] = "%d-%d.json" % (self.new_id(school), self.season(season))
        elif f["kind"] == "navinfo":
            bits = rel.stem.split("-")
            name = str(self.season(bits[0])) + ("-%d" % self.new_id(bits[1]) if len(bits) > 1 else "")
            parts[1] = name + ".json"
        elif f["kind"] == "rankings":
            list_id, rest = rel.stem.split("-", 1)
            parts[1] = "%d-%s.json" % (self.new_id(list_id), rest)
        elif f["kind"] in ("meet", "events", "entries"):
            parts[1] = str(self.new_id(parts[1]))
        elif f["kind"] == "search":
            parts[1] = self.search_name(f, rel.stem) + ".json"
        return Path(*parts)

    def search_name(self, f, stem):
        """The anonymized file's stem: its recorded query, mapped as a person's name is (`ath`
        records it in the file); for a file without one, its first matching doc's name, else the
        stem's words as a name."""
        query = f["tree"].get(SEARCH_QUERY_KEY)
        if query is not None and query.kind == "string":
            return search_fixture_name(self.person(query.value.strip()))
        docs = f["tree"].get("response")
        docs = docs.get("docs") if docs is not None else None
        for doc in (docs.items if docs is not None and docs.kind == "array" else []):
            text = doc.get("textsuggest")
            if text is not None and text.kind == "string" and search_fixture_name(text.value) == stem:
                kind = (doc.get("type").value if doc.get("type") is not None else "").lower()
                fn = {"athlete": self.person, "team": self.school}.get(kind, self.person)
                return search_fixture_name(fn(text.value.strip()))
        return search_fixture_name(self.person(" ".join(stem.split("-"))))

    # -- self-check ----------------------------------------------------------------------------

    def check_maps(self):
        """Every event's map keeps the strict order of its distinct real marks and of the State
        Meet standards among them (so ties stay ties and no mark crosses a standard), changes
        every mark, and no mile estimate compares with a 1600m otherwise than recorded."""
        problems = []
        for name, emap in self.event_maps.items():
            points = {emap.key(d, vs[0] if vs else None) for d, vs in self.marks[name]}
            points.update(std["d"] for std in self.standards if std.get("event") == name)
            if map_disorder(emap, points):
                problems.append("%s: the map changes the order of its marks" % name)
            if any(emap.delta(d) >= 0 for d in points):
                problems.append("%s: a mark left as recorded" % name)
        for std in self.standards:
            if std.get("event"):
                emap = self.event_maps[std["event"]]
                for d, vs in self.marks[std["event"]]:
                    for v in [emap.key(d, vs[0] if vs else None)]:
                        before = (v > std["d"]) - (v < std["d"])
                        after = ((v + emap.delta(v)) > (std["d"] + emap.delta(std["d"]))) - (
                            (v + emap.delta(v)) < (std["d"] + emap.delta(std["d"])))
                        if before != after:
                            problems.append("%s: a mark crosses the standard %s" % (std["event"], std["display"]))
        linked = {n: LINKED[n] for n in self.event_maps if n in LINKED and LINKED[n][0] in self.event_maps}
        if self.estimate_flips(linked):
            problems.append("a mile estimate compares with a 1600m otherwise than recorded")
        if problems:
            raise AnonymizeError("self-check failed, nothing written:\n  " + "\n  ".join(problems[:40]))

    def self_check(self, files, outputs, positional=True):
        """With `positional`, also each changed value against its place in the recording (only
        before the rows are reshuffled); always, no real name or id anywhere."""
        problems = []
        collisions = set()
        real_ids = self.ids
        self.replacements()
        finder = self._finder
        last_names = set(self.last_names)

        def name_in(text):
            hits = finder.find(text)
            return text[hits[0][0] : hits[0][1]] if hits else None

        for f in files:
            out_rel, out_text = outputs[f["rel"]]
            where = str(out_rel)
            for m in WORD_DIGITS.finditer(where):
                if int(m.group(0)) in real_ids:
                    problems.append("%s: its name holds a real id" % where)
            if name_in(where):
                problems.append("%s: its name holds a real name (%r)" % (where, name_in(where)))
            try:
                out_tree = parse_json(out_text)
            except JsonError as e:
                problems.append("%s: not JSON after anonymizing: %s" % (where, e))
                continue
            twins = {}
            if positional:
                problems += same_shape(f["tree"], out_tree, where, twins)
            # Every value the anonymizer changes must differ from the recording's.
            for node, why in (f["checks"] if positional else ()):
                new = twins.get(id(node))
                if new is None or new.value == node.value:
                    problems.append("%s: a %s left as recorded (%r)" % (where, why, node.value))
                elif why == "last" and new.value.strip().lower() in last_names:
                    problems.append("%s: a last name that is a real one (%r)" % (where, new.value))
            # Anywhere else: no real name, no real id. An invented school, meet or venue keeps
            # its kind (`Christian` or `Park`): only its invented part is checked.
            kinds = {id(twins[id(node)]): {"school": SCHOOL_KINDS, "meet": MEET_KINDS, "venue": VENUE_KINDS}[why]
                     for node, why in f["checks"] if why in ("school", "meet", "venue") and id(node) in twins}
            for path, key, node in leaves(out_tree):
                if node.kind == "number" and isinstance(node.value, int) and node.value in real_ids:
                    public = key in PUBLIC_INT_KEYS or key in SORT_KEYS or (
                        key in ("ID", "id") and len(path) > 1 and path[-2] in PUBLIC_ID_PARENTS)
                    if public:
                        collisions.add((where, "/".join(path), node.value))
                    else:
                        problems.append("%s: %s holds a real id (%d)" % (where, "/".join(path), node.value))
                if node.kind != "string" or key in KEEP_KEYS or key in DISPLAY_KEYS or DATE.match(node.value):
                    continue
                if key == "Name" and path[-2:-1] not in (("team",), ("meet",), ("Location",)):
                    continue
                text = node.value
                # After reshuffling, the rows are rebuilt (no node to follow back): every school,
                # meet and venue is an invented one by then (checked before), known by its key.
                kept_kind = kinds.get(id(node)) if positional else (
                    SCHOOL_KINDS if key in SCHOOL_KEYS or (key == "Name" and path[-2:-1] == ("team",))
                    else MEET_KINDS if key in MEET_KEYS or (key == "Name" and path[-2:-1] == ("meet",))
                    else VENUE_KINDS if key in VENUE_KEYS or (key == "Name" and path[-2:-1] == ("Location",))
                    else None)
                if kept_kind is not None:
                    text = re.sub(r"(^\d{4}\s+|\s+\d{4}$)", "", text.strip())
                    text = split_kind(text, kept_kind)[0]
                hit = name_in(text)
                if hit:
                    problems.append("%s: %s holds a real name (%r)" % (where, "/".join(path), hit))
                for m in WORD_DIGITS.finditer(node.value):
                    if int(m.group(0)) in real_ids:
                        problems.append("%s: %s holds a real id (%s)" % (where, "/".join(path), m.group(0)))
            # A division tree's `seasons` keys are seasons (moved, and checked as such), which a
            # small real id may equal: only counted, as a public number is.
            seasons = out_tree.get("seasons") if f["kind"] == "navinfo" else None
            season_keys = {id(k) for k, _ in seasons.items} if seasons is not None and seasons.kind == "object" else set()
            for key_node in object_keys(out_tree):
                if key_node.value.isdigit() and int(key_node.value) in real_ids:
                    if id(key_node) in season_keys:
                        collisions.add((where, "seasons", int(key_node.value)))
                    else:
                        problems.append("%s: a key is a real id (%s)" % (where, key_node.value))
        self.report.collisions = len(collisions)
        if problems:
            shown = "\n  ".join(problems[:40])
            more = "\n  ... and %d more" % (len(problems) - 40) if len(problems) > 40 else ""
            raise AnonymizeError("self-check failed, nothing written:\n  " + shown + more)

    # -- run -----------------------------------------------------------------------------------

    def public_divisions(self, files):
        """The divisions whose names stay (`public_divisions`), from the Area files and the
        Recording's trees."""
        trees = [json.loads(f["text"]) for f in files if f["kind"] == "navinfo"]
        self.public_div_ids, self.public_div_names = public_divisions(area_files(self.areas_dir or AREAS), trees)

    def run(self, recording):
        files = self.read(recording)
        self.collect(files)
        self.public_divisions(files)
        self.build()
        outputs = {}
        for f in files:
            edits, checks = self.rewrite(f)
            f["checks"] = checks
            outputs[f["rel"]] = (self.rename(f), splice(f["text"], edits))
        # A `FirstName` and `LastName` together name a person (the map's `value_kinds`, which
        # the real-value guard reads), though each was replaced on its own.
        for first, last in self.pairs.values():
            self.report.value_kinds.setdefault(first + " " + last, set()).add("person")
        names = [o[0] for o in outputs.values()]
        if len(set(names)) != len(names):
            raise AnonymizeError("two files would get the same name")
        self.check_maps()
        self.self_check(files, outputs)
        outputs = self.reshuffle(files, outputs)
        self.self_check(files, outputs, positional=False)
        self.report.files = len(files)
        return self.report, list(outputs.values())

    def reshuffle(self, files, outputs):
        """Rankings lists with some athletes dropped and invented ones placed elsewhere
        (`Reshuffler`); athletes with a bio, on a meet's entries or kept (`--keep`) never go."""
        protected = {self.new_id(a) for a in self.protected if a in self.report.id_map}
        held = set()
        while True:
            self.__dict__.pop("_invented_ids", None)  # the same stand-ins' ids each time
            shuffler = Reshuffler(self, files, outputs, protected | held)
            shuffler.choose()
            shuffler._entries = None
            shuffler.swap_grades()
            more = shuffler.short_reads(protected) - held
            if not more:
                break
            held |= more
        if shuffler.short:
            # Holding everyone it could did not make the read as long as recorded: refuse, as
            # a replay of the recorded command would fail on the copy.
            raise AnonymizeError("self-check failed, nothing written:\n  " + "\n  ".join(
                "list %s: fewer than %d people behind a subject's mark (%d) on its recorded pages"
                % (key, JUST_BEHIND, m) for key, m in shuffler.short[:40]))
        final = shuffler.write()
        shuffler.check(final)
        final = self.drop_from_searches(files, final, set(shuffler.dropped))
        real = {new: old for old, new in self.report.id_map.items()}
        self.report.dropped = {real[a]: n for a, n in shuffler.dropped.items()}
        self.report.inserted = {n: name for n, (name, _) in shuffler.invented.items()}
        self.report.grade_offsets = {real.get(a, a): d for a, d in shuffler.offset.items()}
        return final

    def drop_from_searches(self, files, outputs, dropped):
        """A dropped athlete's search docs go too."""
        out = dict(outputs)
        for f in files:
            if f["kind"] != "search":
                continue
            new_rel, text = outputs[f["rel"]]
            tree = parse_json(text)
            response = tree.get("response")
            docs = response.get("docs") if response is not None else None
            if docs is None or docs.kind != "array" or not docs.items:
                continue
            gone = [d for d in docs.items if d.get("id_db") is not None and str(d.get("id_db").value).isdigit()
                    and int(d.get("id_db").value) in dropped]
            if not gone:
                continue
            keep = [d for d in docs.items if d not in gone]
            sep = text[docs.items[0].end:docs.items[1].start] if len(docs.items) > 1 else ", "
            inner = sep.join(text[d.start:d.end] for d in keep)
            head = text[docs.start:docs.items[0].start] if keep else "["
            tail = text[docs.items[-1].end:docs.end] if keep else "]"
            edits = [(docs.start, docs.end, head + inner + tail)]
            found = response.get("numFound")
            if found is not None and found.kind == "number":
                edits.append((found.start, found.end, encode(max(0, found.value - len(gone)))))
            out[f["rel"]] = (new_rel, splice(text, edits))
        return out

    def invented_id(self, n):
        taken = set(self.report.id_map.values()) | self.ids | self.__dict__.setdefault("_invented_ids", set())
        k = self.h("invented-id", n) % RESERVED_SPAN
        while RESERVED_BASE + k in taken:
            k = (k + 1) % RESERVED_SPAN
        self._invented_ids.add(RESERVED_BASE + k)
        return RESERVED_BASE + k

    def invented_person(self, n):
        # As an invented athlete's first name: never a real whole name (a school or city that is
        # also a first name, `Kennedy`), which the self-check forbids anywhere.
        first = self.invent("first", "\x00invented %d" % n, FIRST_POOL, None, avoid_real=False)
        return first + " " + self.last("\x00invented %d" % n)

# ---------------------------------------------------------------------------------------------
# Reshuffled rows (ADR 0010): kept list ids and ranks would let anyone pair each row with the
# public list, so some athletes leave every list and invented ones take places elsewhere.

_RANKINGS_NAME = re.compile(r"^(\d+)-([fm])-(.+)-([0-9_]+|all)-p(\d+)$")
PAGE_ROWS = 100
# ath reads a list down to this many rows past a mark (`rank::JUST_BEHIND`).
JUST_BEHIND = 3


def display_value(display, kind):
    """A row's display as a number on its event's scale (running ms, field SortInt), or None."""
    if not isinstance(display, str) or not display.strip():
        return None
    if kind == "running":
        return running_ms(display)
    return field_sort_int(display) if parse_field(display) else metric_sort_int(display)


def display_unit(display, kind):
    """The step of one display (`56.85` 10 ms, `57.9h` 100 ms, `31-04.50` 10), or None."""
    if kind == "running":
        p = parse_running(display)
        return 10 ** (3 - p[1]) if p else None
    p = parse_field(display)
    return 1000 // 10 ** p[1] if p and p[1] <= 3 else None


def format_like(display, value, kind):
    """`value` (on `display`'s scale) shown as `display` is: same decimals, suffix, width."""
    if kind == "running":
        _, decimals, suffix, _ = parse_running(display)
        return format_running(value, decimals, suffix)
    total, decimals, width = parse_field(display)
    return format_field((FIELD_BASE - value) * 10 ** decimals // 1000, decimals, width)


class Row:
    def __init__(self, node, text, kind):
        self.node, self.text = node, text
        get = lambda k: node.get(k).value if node.get(k) is not None else None
        self.athlete = get("AthleteID") or 0
        self.name = get("AthleteName") or ""
        self.team, self.grade = get("TeamID"), get("GradeID")
        self.rank, self.rownum = get("rank"), get("rowNum")
        self.display = get("display") or ""
        self.value = display_value(self.display, kind)
        self.sort = get("SortIntCalc")
        if not isinstance(self.sort, int):
            self.sort = self.value
        self.masked = self.athlete <= 0 or (bool(X_ONLY.match(self.name or "x")) and not self.display.strip())
        self.invented = False


class Family:
    """One list for one gender, event and Grade group, all its recorded pages."""

    def __init__(self, key, kind, season=None):
        self.key, self.kind, self.season = key, kind, season
        self.list_id, self.gender, self.event, grades = key
        self.grades = None if grades == "all" else {int(g) for g in grades.split("_")}
        self.pages = []  # (page number, rel, tree, array node, [Row])

    @property
    def group(self):
        """The marks this list shares with others: one season, gender and event."""
        return (self.season, self.gender, self.event)

    def fits(self, grade):
        return self.grades is None or grade in self.grades

    def finish(self):
        self.pages.sort(key=lambda p: p[0])
        # Pages overlap (page 2 starts at row 100), the same people, each once on the list. By
        # athlete, not by rowNum: athletes tied at a page's edge can come in another order on the
        # next page (one at row 101 on page 1, at 102 on page 2), and Athletic.net does that.
        seen, self.rows = set(), []
        for _, _, _, _, rows in self.pages:
            for r in rows:
                if not r.masked and r.athlete not in seen:
                    seen.add(r.athlete)
                    self.rows.append(r)
        self.rows.sort(key=lambda r: r.rownum)
        last = self.pages[-1][4]
        self.capped = any(r.masked for _, _, _, _, rows in self.pages for r in rows)
        self.complete = not self.capped and len([r for r in last if not r.masked]) < PAGE_ROWS
        # Lists sort on SortIntCalc: their reach is measured on it, never on the display.
        values = [r.sort for r in self.rows if r.sort is not None]
        self.first = min(values) if values else None
        self.last = max(values) if values else None
        self.recorded = [(r.rank, r.athlete) for r in self.rows]
        # Equal marks Athletic.net ranked one after another (8 and 9 for two 54.37s, as it
        # sometimes does): the pair keeps its two ranks where it stands together again.
        self.apart = {(a.athlete, b.athlete) for a, b in zip(self.rows, self.rows[1:])
                      if a.sort == b.sort and a.rank != b.rank}


class Reshuffler:
    def __init__(self, anon, files, outputs, protected):
        self.anon, self.outputs, self.protected = anon, outputs, protected
        self.families = {}
        self.texts = {}
        for f in files:
            if f["kind"] != "rankings":
                continue
            new_rel, text = outputs[f["rel"]]
            m = _RANKINGS_NAME.match(new_rel.stem)
            if not m:
                raise AnonymizeError("%s is not named <list>-<gender>-<event>-<grades>-p<page>" % new_rel)
            tree = parse_json(text)
            groups = tree.get("groupedRankings")
            if groups is None or groups.kind != "array" or not groups.items:
                continue
            if len(groups.items) != 1 or groups.items[0].kind != "array":
                raise AnonymizeError("%s: a rankings page with more than one group of rows" % new_rel)
            key = (m.group(1), m.group(2), m.group(3), m.group(4))
            name = anon.event_of(("short", m.group(3)))
            emap = anon.event_maps.get(name)
            kind = emap.kind if emap else event_kind(name or "") or "running"
            division = tree.get("division")
            season = division.get("SeasonID") if division is not None and division.kind == "object" else None
            fam = self.families.setdefault(key, Family(key, kind, season.value if season is not None else None))
            array = groups.items[0]
            if any(n.kind != "object" for n in array.items):
                raise AnonymizeError("%s: a rankings row that is not an object" % new_rel)
            rows = [Row(n, text[n.start:n.end], kind) for n in array.items]
            # Masked rows are only ever a tail (as ath expects): kept there, as recorded.
            seen_masked = False
            for r in rows:
                if r.masked:
                    seen_masked = True
                elif seen_masked:
                    raise AnonymizeError(
                        "%s: a masked row with people after it; ath refuses such a page, and so does "
                        "the anonymizer" % new_rel)
            fam.pages.append((int(m.group(5)), f["rel"], tree, array, rows))
            self.texts[f["rel"]] = text
        for fam in self.families.values():
            fam.finish()
        self.groups = {}
        for fam in self.families.values():
            self.groups.setdefault(fam.group, []).append(fam)
        # Which lists a school is seen on (any event or gender).
        self.seen = {}
        for fam in self.families.values():
            for r in fam.rows:
                self.seen.setdefault(fam.list_id, set()).add(r.team)
        self.dropped = {}  # athlete -> invented athlete id
        self.placed = {}  # (group, invented id) -> value
        self.invented = {}  # invented id -> (name, template athlete)
        # Each list's marks (stand-ins' too, as they are placed): a stand-in ties no one on the
        # lists it stands on. Other lists (another State's) may hold its mark: no one compares.
        self.marks = {f.key: sorted({r.sort for r in f.rows if r.sort is not None}) for f in self.families.values()}
        # Athletes whose recorded lists their grade does not explain: no grade, or a Grade group
        # list that has them while their grade is not its own (or lacks them while it is, inside
        # its recorded pages). Athletic.net's lists have a few (a grade changed after the mark,
        # say). They stay as recorded, like protected athletes: never dropped, their grade kept,
        # on exactly the lists they were on.
        self.misfits = self.find_misfits()
        self.protected = set(protected) | self.misfits

    def short_reads(self, subjects):
        """Who to hold where the lists after would read short. ath reads a list recorded only in
        part down past a mark (a subject's) until 3 people are behind it, and the recorded pages
        were enough for that; a grade swapped in there (a tie made) or out, or a stand-in placed
        ahead, can leave too few behind on those pages. Then the people with that mark and the 3
        just behind it as recorded are to be held where they are (as protected athletes are),
        and the lists made again: returned, empty when every read is as long as recorded. The
        lists still short are left in `self.short` (list, mark)."""
        self.short = []
        marks = {}
        for fam in self.families.values():
            for r in fam.rows:
                if r.athlete in subjects and r.sort is not None:
                    marks.setdefault(fam.group, set()).add(r.sort)
        out = set()
        for fam in self.families.values():
            if fam.complete:
                continue
            for m in sorted(marks.get(fam.group, ())):
                behind = [r for r in fam.rows if r.sort is not None and r.sort > m]
                if len(behind) < JUST_BEHIND:
                    continue
                if sum(1 for x in self.rebuilt(fam) if x["sort"] > m) >= JUST_BEHIND:
                    continue
                self.short.append(("-".join(fam.key), m))
                pool, _ = self.entries()
                tied = [w for w, entry in pool.get((fam.list_id, fam.gender, fam.event), {}).items() if entry[2] == m]
                out.update(w for w in tied if w not in self.invented)
                out.update(r.athlete for r in behind[:JUST_BEHIND])
        return out - set(subjects)

    def find_misfits(self):
        pool = {}
        for fam in self.families.values():
            for r in fam.rows:
                pool.setdefault((fam.list_id, fam.gender, fam.event), {}).setdefault(r.athlete, r)
        out = set()
        for fam in self.families.values():
            on = {r.athlete for r in fam.rows}
            for who, r in pool.get((fam.list_id, fam.gender, fam.event), {}).items():
                if r.grade is None:
                    out.add(who)
                    continue
                by_grade = fam.fits(r.grade) and (fam.complete or (fam.last is not None and r.sort is not None
                                                                   and r.sort <= fam.last))
                if fam.grades is not None and by_grade != (who in on):
                    out.add(who)
        return out

    # -- choosing ------------------------------------------------------------------------------

    def athletes(self):
        found = {}
        for fam in self.families.values():
            for r in fam.rows:
                found.setdefault(r.athlete, []).append(fam)
        return found

    def place(self, athlete, group, n):
        """A SortInt for athlete's stand-in in `group` (one season, gender and event), or None:
        strictly between two neighbouring marks some places away from the athlete's, its display
        (the athlete's own display relation kept: a hand time stays one) on the athlete's display
        step, and inside exactly the recorded lists the athlete is inside (so every list keeps
        its length)."""
        fams = self.groups[group]
        mine = [(f, r) for f in fams for r in f.rows if r.athlete == athlete]
        row = mine[0][1]
        unit = display_unit(row.display, mine[0][0].kind)
        if row.value is None or row.sort is None or unit is None or row.grade is None:
            return None
        off = row.value - row.sort
        # Its lists' marks (every Grade group of them: a grade swap may bring it into another).
        lists = {f.list_id for f, _ in mine}
        marks = sorted({m for f in fams if f.list_id in lists for m in self.marks[f.key]})
        i = marks.index(row.sort)
        member = {id(f) for f, _ in mine}
        gaps = [g for g in range(i - 14, i + 14) if g not in (i - 1, i) and 0 <= g < len(marks) - 1]
        gaps.sort(key=lambda g: self.anon.h("gap", athlete, group, g))
        for g in gaps:
            lo, hi = marks[g], marks[g + 1]
            first = ((lo + off) // unit + 1) * unit - off
            choices = list(range(first, hi, unit))
            if not choices:
                continue
            v = choices[self.anon.h("value", athlete, group, g) % len(choices)]
            if all(self.fits(f, v, id(f) in member, row) for f in fams):
                return v
        return None

    def fits(self, fam, v, member, row):
        if not fam.fits(row.grade) or fam.first is None:
            return True
        if member:
            return v > fam.first and (fam.complete or v < fam.last)
        if not fam.complete:
            # Not on a list part of which was recorded: when the athlete's own mark is inside
            # what was recorded, their school is not on that list (another State's or Area's),
            # so the stand-in may go anywhere; otherwise it stays past what was recorded.
            return v > fam.last or (row.sort is not None and row.sort <= fam.last)
        return row.team not in self.seen.get(fam.list_id, ())

    def drop(self, athlete, n):
        """Drop `athlete` everywhere for an invented stand-in, when every group places one."""
        values, lists = {}, {}
        for group, fams in self.groups.items():
            mine = {f.list_id for f in fams if any(r.athlete == athlete for r in f.rows)}
            on = [f for f in fams if f.list_id in mine]
            if on:
                v = self.place(athlete, group, n)
                if v is None:
                    return False
                values[group], lists[group] = v, on
        new = self.anon.invented_id(n)
        name = self.anon.invented_person(n)
        self.dropped[athlete] = new
        self.invented[new] = (name, athlete)
        for group, v in values.items():
            self.placed[(group, new)] = v
            for f in lists[group]:
                bisect_insort(self.marks[f.key], v)
        return True

    # -- the lists after -----------------------------------------------------------------------

    # -- grades -------------------------------------------------------------------------------

    def entries(self):
        """Every athlete who stands on a list of `(list, gender, event)` (by the athlete there
        after the drops: a dropped one's stand-in), with the row to write them from, their mark
        (display value and SortInt) and their recorded grade; and where they were recorded."""
        if getattr(self, "_entries", None) is not None:
            return self._entries
        pool, recorded = {}, {}
        # The widest list first: everyone stands in its order (Athletic.net's, among equal marks
        # too), on every list of theirs.
        for fam in sorted(self.families.values(), key=lambda f: (-len(f.rows), f.key)):
            lkey = (fam.list_id, fam.gender, fam.event)
            for r in fam.rows:
                who = r.athlete
                value, sort = r.sort, r.sort
                if who in self.dropped:
                    new = self.dropped[who]
                    placed = self.placed.get((fam.group, new))
                    if placed is None:
                        continue
                    who, value, sort = new, placed, placed
                pool.setdefault(lkey, {}).setdefault(who, (r, value, sort, r.grade))
                recorded.setdefault(fam.key, set()).add(who)
        self._entries = pool, recorded
        return self._entries

    def in_range(self, fam, value):
        """Whether a mark falls inside what the list's recorded pages show."""
        return fam.complete or (fam.last is not None and value is not None and value <= fam.last)

    def member(self, fam, athlete, offset):
        """Whether `athlete` (with their grade moved by `offset`) stands on `fam`."""
        pool, recorded = self.entries()
        if athlete in self.misfits:
            return offset == 0 and athlete in recorded.get(fam.key, ())
        entry = pool.get((fam.list_id, fam.gender, fam.event), {}).get(athlete)
        if entry is None or entry[3] is None:
            return False
        _, value, _, grade = entry
        if fam.fits(grade):
            return athlete in recorded.get(fam.key, ()) and fam.fits(grade + offset)
        return offset != 0 and fam.fits(grade + offset) and self.in_range(fam, value)

    def swap_grades(self):
        """Swap grades between pairs of athletes who may go (stand-ins too), chosen with the key:
        partners stand on the same lists (so the same Area), far apart in mark, with the same
        grade gap in every season they share, and the swap leaves every list's per-grade counts
        and length as recorded. Protected athletes keep their grades."""
        self.offset = {}
        pool, _ = self.entries()
        lists = {}
        for lkey, people in pool.items():
            for who, (_, value, _, grade) in people.items():
                if who not in self.protected and grade is not None:
                    lists.setdefault(who, {})[lkey] = (value, grade)
        groups = {}
        for who, where in lists.items():
            groups.setdefault(frozenset(where), []).append(who)
        families_of = {}
        for fam in self.families.values():
            families_of.setdefault((fam.list_id, fam.gender, fam.event), []).append(fam)
        for sig, people in sorted(groups.items(), key=lambda g: sorted(g[0])):
            lkey = sorted(sig)[0]
            people.sort(key=lambda w: (lists[w][lkey][0] is None, lists[w][lkey][0] or 0, w))
            spot = {w: i for i, w in enumerate(people)}
            free = set(people)
            for a in sorted(people, key=lambda w: self.anon.h("grade-order", w)):
                if a not in free:
                    continue
                # Partners from far away in mark: a keyed sample of the people at least a third of
                # the group away (anyone, in a small group), so a mark window cannot pair them.
                n, i = len(people), spot[a]
                far = n // 3 if n >= 6 else 1
                sample = {people[self.anon.h("grade-pick", a, t) % n] for t in range(min(4 * n, 80))}
                options = []
                for b in sample:
                    if b == a or b not in free or abs(spot[b] - i) < far:
                        continue
                    gaps = {lists[b][k][1] - lists[a][k][1] for k in sig}
                    if len(gaps) != 1 or 0 in gaps:
                        continue
                    options.append((-abs(spot[a] - spot[b]), self.anon.h("grade-pair", a, b), b, gaps.pop()))
                for _, _, b, d in sorted(options)[:6]:
                    if all(self.grades_on(f, a, b, 0) == self.grades_on(f, a, b, d)
                           for k in sig for f in families_of[k]):
                        self.offset[a], self.offset[b] = d, -d
                        free -= {a, b}
                        break

    def grades_on(self, fam, a, b, d):
        """The grades `a` and `b` stand on `fam` with, after `a` moves by `d` and `b` by `-d`."""
        pool, _ = self.entries()
        people = pool.get((fam.list_id, fam.gender, fam.event), {})
        out = []
        for who, off in ((a, d), (b, -d)):
            if self.member(fam, who, off):
                out.append(people[who][3] + off)
        return sorted(out)

    def rebuilt(self, fam):
        """The family's rows after: everyone who stands on it (kept athletes, stand-ins, and
        athletes whose new grade brings them in), in mark order; ranks worked out again (equal
        SortInt is one rank) and rowNums 1, 2, 3."""
        pool, _ = self.entries()
        people = pool.get((fam.list_id, fam.gender, fam.event), {})
        offsets = getattr(self, "offset", {})
        order = {w: i for i, w in enumerate(people)}
        members = []
        for who, (row, value, sort, grade) in people.items():
            if self.member(fam, who, offsets.get(who, 0)):
                members.append((sort, order[who], who, row, value, grade))
        members.sort(key=lambda m: (m[0], m[1]))
        rows = []
        for pos, (sort, _, who, row, value, grade) in enumerate(members, start=1):
            same = rows and rows[-1]["sort"] == sort and (rows[-1]["athlete"], who) not in fam.apart
            item = {"rank": rows[-1]["rank"] if same else pos, "rowNum": pos, "row": row, "athlete": who,
                    "sort": sort, "grade": grade + offsets.get(who, 0) if grade is not None else None}
            if who in self.invented:
                item["value"] = value
            rows.append(item)
        return rows

    def changed(self, fam):
        self._entries = None
        rows = self.rebuilt(fam)
        kept = [(x["rank"], x["athlete"]) for x in rows if "value" not in x]
        recorded = [(rank, a) for rank, a in fam.recorded if a not in self.dropped]
        return kept != recorded

    def choose(self, keep_rate=None):
        athletes = self.athletes()
        droppable = sorted(a for a in athletes if a not in self.protected)
        rate = 0.10 + 0.10 * (self.anon.h("drop-rate") % 1000) / 1000.0
        want = max(1, int(round(rate * len(droppable)))) if droppable else 0
        order = sorted(droppable, key=lambda a: self.anon.h("drop", a))
        n = 0
        for a in order:
            if len(self.dropped) >= want:
                break
            n += 1
            self.drop(a, n)
        # Every list with an athlete who may go changes: one more drop where one did not.
        for _ in range(len(self.families)):
            stale = [f for f in self.families.values()
                     if not self.changed(f) and any(r.athlete not in self.protected for r in f.rows)]
            progress = False
            for fam in stale:
                if self.changed(fam):
                    continue
                for a in sorted((r.athlete for r in fam.rows if r.athlete not in self.protected
                                 and r.athlete not in self.dropped), key=lambda a: self.anon.h("drop", a)):
                    n += 1
                    if self.drop(a, n):
                        progress = True
                        break
            if not progress:
                break

    # -- writing ------------------------------------------------------------------------------

    def row_text(self, item):
        r = item["row"]
        tree = parse_json(r.text)
        edits = []

        def put(key, value):
            node = tree.get(key)
            if node is not None:
                edits.append((node.start, node.end, encode(value)))

        put("rank", item["rank"])
        put("rowNum", item["rowNum"])
        if item.get("grade") is not None and item["grade"] != r.grade:
            put("GradeID", item["grade"])
        if "value" in item:
            new = item["athlete"]
            name = self.invented[new][0]
            put("AthleteID", new)
            put("AthleteName", name)
            shown = item["value"] + (r.value - r.sort)
            put("display", format_like(r.display, shown, "running" if parse_running(r.display) else "field"))
            for key in ("SortIntRaw", "SortIntCalc", "SortIntOrig", "SortInt"):
                put(key, item["value"])
        return splice(r.text, edits)

    def write(self):
        new_texts = dict(self.texts)
        for fam in self.families.values():
            rows = self.rebuilt(fam)
            if len(rows) != len(fam.rows):
                raise AnonymizeError("list %s changed its length" % "-".join(fam.key))
            # The last page takes the rest: a torn list (pages from different moments, so their
            # overlap disagrees) has one or two people more or fewer than its rowNums say.
            last_page = max((p[0] for p in fam.pages if any(not r.masked for r in p[4])), default=None)
            for number, rel, tree, array, page_rows in fam.pages:
                real = [r.rownum for r in page_rows if not r.masked]
                if not real:
                    continue
                lo, hi = min(real), (max(real) if number != last_page else math.inf)
                items = [self.row_text(x) for x in rows if lo <= x["rowNum"] <= hi]
                items += [r.text for r in page_rows if r.masked]
                text = self.texts[rel]
                first, last = array.items[0], array.items[-1]
                sep = text[array.items[0].end:array.items[1].start] if len(array.items) > 1 else ", "
                new_array = text[array.start:first.start] + sep.join(items) + text[last.end:array.end]
                new_texts[rel] = text[:array.start] + new_array + text[array.end:]
        out = {}
        for rel, (new_rel, text) in self.outputs.items():
            out[rel] = (new_rel, new_texts.get(rel, text))
        return out

    # -- checking ------------------------------------------------------------------------------

    def check(self, final):
        problems = []
        by_athlete = {}
        final_rows = {}
        for fam in self.families.values():
            rows = []
            minimum = None
            for number, rel, _, _, _ in fam.pages:
                tree = parse_json(final[rel][1])
                count = tree.get("minCount")
                if count is not None and count.kind == "number":
                    minimum = count.value if minimum is None else max(minimum, count.value)
                groups = tree.get("groupedRankings")
                for n in (groups.items[0].items if groups is not None and groups.items else ()):
                    r = Row(n, "", fam.kind)
                    if not r.masked and all(x.rownum != r.rownum for x in rows):
                        rows.append(r)
            rows.sort(key=lambda r: r.rownum)
            where = "list " + "-".join(fam.key)
            # Masked tails as recorded: the same rows, at the end, on the same pages.
            for number, rel, _, _, page_rows in fam.pages:
                before = [r.text for r in page_rows if r.masked]
                items = parse_json(final[rel][1]).get("groupedRankings").items[0].items
                text = final[rel][1]
                after = [text[n.start:n.end] for n in items[len(items) - len(before):]] if before else []
                if before != after:
                    problems.append("%s: a masked tail moved on page %d" % (where, number))
            if not rows:
                continue  # no one but Masked rows: kept as recorded
            if [r.rownum for r in rows] != list(range(rows[0].rownum, rows[0].rownum + len(rows))):
                problems.append("%s: rowNums are not one after another" % where)
            for a, b in zip(rows, rows[1:]):
                # Marks never go down the list; a shared rank is a shared mark; a new rank is the
                # row's place. (Equal displays may still rank apart, as Athletic.net ranks them.)
                if b.sort < a.sort or b.rank < a.rank or (b.rank == a.rank and b.sort != a.sort) or (
                        b.rank != a.rank and b.rank != b.rownum):
                    problems.append("%s: ranks do not follow the marks at row %d" % (where, b.rownum))
            # A complete list's minCount covers its rows, but where the recorded pages already
            # showed more people than their minCount (a torn list, its pages from different
            # moments): kept as recorded.
            recorded = [p[2].get("minCount") for p in fam.pages]
            recorded = max((n.value for n in recorded if n is not None and n.kind == "number"), default=None)
            short_as_recorded = recorded is not None and recorded < len(fam.rows)
            if minimum is not None and fam.complete and minimum < len(rows) and not short_as_recorded:
                problems.append("%s: minCount %d below its %d rows" % (where, minimum, len(rows)))
            # Grades: each list's per-grade counts as recorded; protected grades as recorded.
            count = lambda rs: sorted(((g, sum(1 for r in rs if r.grade == g)) for g in {r.grade for r in rs}),
                                      key=lambda c: (c[0] is None, c[0] or 0))  # a row with no grade: last
            if count(rows) != count(fam.rows):
                problems.append("%s: its per-grade counts changed" % where)
            recorded_grade = {r.athlete: r.grade for r in fam.rows}
            for r in rows:
                if r.athlete in self.protected and recorded_grade.get(r.athlete, r.grade) != r.grade:
                    problems.append("%s: protected athlete %d changed grade" % (where, r.athlete))
                if not fam.fits(r.grade) and not (r.athlete in self.misfits and recorded_grade.get(r.athlete) == r.grade):
                    problems.append("%s: athlete %d's grade %s is not the list's" % (where, r.athlete, r.grade))
            final_rows[fam.key] = rows
            kept = [(r.rank, r.athlete) for r in rows if r.athlete not in self.invented]
            if kept == fam.recorded and any(a not in self.protected for _, a in fam.recorded):
                problems.append("%s: its kept athletes stand as recorded, rank for rank" % where)
            present = {r.athlete for r in rows}
            for _, a in fam.recorded:
                if a in self.protected and a not in present:
                    problems.append("%s: protected athlete %d is missing" % (where, a))
                if a in self.dropped and a in present:
                    problems.append("%s: dropped athlete %d is still there" % (where, a))
            for r in rows:
                seen = by_athlete.setdefault(fam.group + (r.athlete,), (r.name, r.team, r.grade, r.display))
                if seen != (r.name, r.team, r.grade, r.display):
                    problems.append("%s: athlete %d differs from another list" % (where, r.athlete))
        # A Grade group's list holds exactly its grades' people from the same list for every
        # grade, as far as both lists' recorded pages reach, in the same order.
        for key, rows in final_rows.items():
            fam = self.families[key]
            whole = self.families.get(key[:3] + ("all",))
            if fam.grades is None or whole is None or whole.key not in final_rows:
                continue
            reach = min(x for x in (None if fam.complete else fam.last, None if whole.complete else whole.last)
                        if x is not None) if not (fam.complete and whole.complete) else None
            inside = lambda r: reach is None or r.sort <= reach
            # (A misfit, kept as recorded, is in where it was recorded.)
            on = {r.athlete for r in fam.rows}
            want = [r.athlete for r in final_rows[whole.key] if inside(r) and (
                r.athlete in on if r.athlete in self.misfits else fam.fits(r.grade))]
            have = [r.athlete for r in rows if inside(r)]
            if want != have:
                problems.append("list %s: not the Grade group's people of list %s" % ("-".join(key), "-".join(whole.key)))
        # An athlete's grade moves by one amount in every list (season) they are on.
        moves = {}
        for key, rows in final_rows.items():
            recorded = {r.athlete: r.grade for r in self.families[key].rows}
            for r in rows:
                if r.athlete in recorded and r.grade is not None and recorded[r.athlete] is not None:
                    moves.setdefault(r.athlete, set()).add(r.grade - recorded[r.athlete])
        for who, ds in moves.items():
            if len(ds) > 1:
                problems.append("athlete %d's grade moves differently on different lists" % who)
        if problems:
            raise AnonymizeError("self-check failed, nothing written:\n  " + "\n  ".join(problems[:40]))


def bisect_insort(values, v):
    import bisect
    bisect.insort(values, v)


def athletic_link(node):
    """Whether `node` is an Athletic.net page link (absolute or site-relative)."""
    return node.kind == "string" and (
        node.value.startswith(("https://www.athletic.net", "http://www.athletic.net", "/")) or not node.value)


def scalars(node):
    """Every value in `node` that is not an object or a list (`node` itself when it is one)."""
    if node.kind == "object":
        for _, v in node.items:
            yield from scalars(v)
    elif node.kind == "array":
        for v in node.items:
            yield from scalars(v)
    else:
        yield node


def map_disorder(emap, points):
    """Whether `emap` moves the sorted `points` out of strict order."""
    mapped = [d + emap.delta(d) for d in sorted(points)]
    return any(b <= a for a, b in zip(mapped, mapped[1:]))


def load_standards(areas_dir):
    """Every State Meet standard in `areas_dir/*.json` (`state_meet`, any season, Class or
    level): a mark string, or a list of them, under a `f` / `m` key, by event code."""
    out = []
    for path in sorted(Path(areas_dir).glob("*.json")):
        text = path.read_text(encoding="utf-8")
        tree = parse_json(text)
        meet = tree.get("state_meet")
        if meet is None:
            continue

        def walk(node):
            if node.kind != "object":
                return
            for k, v in node.items:
                if k.value in ("f", "m") and v.kind == "object":
                    for code, val in v.items:
                        for mark in ([val] if val.kind == "string" else val.items if val.kind == "array" else []):
                            if mark.kind == "string":
                                out.append({"file": path.name, "code": code.value, "gender": k.value,
                                            "display": mark.value, "node": mark})
                else:
                    walk(v)

        walk(meet)
    return out


def write_areas(areas_dir, out_dir, standards, year_shift=0):
    """A copy of `areas_dir/*.json` with each mapped standard replaced and the seasons its
    State Meet section is keyed by moved with the seasons, for pages built from anonymized data
    (ADR 0010: their standards move with the marks, their seasons with the seasons)."""
    out_dir = Path(out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)
    for path in sorted(Path(areas_dir).glob("*.json")):
        text = path.read_text(encoding="utf-8")
        edits = [(s["node"].start, s["node"].end, encode(s["new"])) for s in standards if s["file"] == path.name]
        meet = parse_json(text).get("state_meet")
        if meet is not None and year_shift:
            for key in object_keys(meet):
                if re.fullmatch(r"(19|20|21)\d{2}", key.value):
                    edits.append((key.start, key.end, encode(str(int(key.value) + year_shift))))
        (out_dir / path.name).write_text(splice(text, edits), encoding="utf-8")


def same_shape(a, b, where, twins):
    """What differs in shape between `a` and `b` (none, after anonymizing: only values change),
    filling `twins` with each node of `a` (by id) and its node in `b`."""
    twins[id(a)] = b
    if a.kind != b.kind:
        return ["%s: a %s became a %s" % (where, a.kind, b.kind)]
    if a.kind == "object":
        if len(a.items) != len(b.items):
            return ["%s: an object changed its keys" % where]
        out = []
        for (ka, va), (kb, vb) in zip(a.items, b.items):
            twins[id(ka)] = kb
            renamed = ka.value != kb.value
            if renamed and not (ka.value.isdigit() and kb.value.isdigit()) and not (
                    re.fullmatch(r"\d+_\d+", ka.value) and re.fullmatch(r"\d+_\d+", kb.value)):
                out.append("%s: key %r became %r" % (where, ka.value, kb.value))
            out += same_shape(va, vb, where, twins)
        return out
    if a.kind == "array":
        if len(a.items) != len(b.items):
            return ["%s: a list changed its length" % where]
        out = []
        for x, y in zip(a.items, b.items):
            out += same_shape(x, y, where, twins)
        return out
    if a.kind == "literal" and a.value != b.value:
        return ["%s: %r became %r" % (where, a.value, b.value)]
    if a.kind == "number" and type(a.value) is not type(b.value):
        return ["%s: a number changed its type" % where]
    return []


def leaves(node, path=(), key=None):
    if node.kind == "object":
        for k, v in node.items:
            yield from leaves(v, path + (k.value,), k.value)
    elif node.kind == "array":
        for v in node.items:
            yield from leaves(v, path, key)
    else:
        yield path, key, node


def object_keys(node):
    if node.kind == "object":
        for k, v in node.items:
            yield k
            yield from object_keys(v)
    elif node.kind == "array":
        for v in node.items:
            yield from object_keys(v)


MARKER = ".anonymized"


def write_dir(out_dir, outputs):
    """Write `outputs` as `out_dir`, with the marker that lets a later `--replace` replace it."""
    out_dir = Path(out_dir)
    parent = out_dir.parent
    parent.mkdir(parents=True, exist_ok=True)
    tmp = Path(tempfile.mkdtemp(prefix=".anonymize-", dir=str(parent)))
    try:
        for rel, text in outputs:
            path = tmp / rel
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text, encoding="utf-8")
        (tmp / MARKER).write_text("Written by tools/anonymize/anonymize.py (docs/anonymize.md).\n")
        if out_dir.exists():
            shutil.rmtree(out_dir)
        os.replace(str(tmp), str(out_dir))
    except BaseException:
        shutil.rmtree(tmp, ignore_errors=True)
        raise


AREAS = Path(__file__).resolve().parents[2] / "areas"

# The States and DC by code: public structure, the names Athletic.net gives their lists.
STATE_NAMES = {
    "AL": "Alabama", "AK": "Alaska", "AZ": "Arizona", "AR": "Arkansas", "CA": "California", "CO": "Colorado",
    "CT": "Connecticut", "DE": "Delaware", "DC": "District of Columbia", "FL": "Florida", "GA": "Georgia",
    "HI": "Hawaii", "ID": "Idaho", "IL": "Illinois", "IN": "Indiana", "IA": "Iowa", "KS": "Kansas",
    "KY": "Kentucky", "LA": "Louisiana", "ME": "Maine", "MD": "Maryland", "MA": "Massachusetts",
    "MI": "Michigan", "MN": "Minnesota", "MS": "Mississippi", "MO": "Missouri", "MT": "Montana",
    "NE": "Nebraska", "NV": "Nevada", "NH": "New Hampshire", "NJ": "New Jersey", "NM": "New Mexico",
    "NY": "New York", "NC": "North Carolina", "ND": "North Dakota", "OH": "Ohio", "OK": "Oklahoma",
    "OR": "Oregon", "PA": "Pennsylvania", "RI": "Rhode Island", "SC": "South Carolina", "SD": "South Dakota",
    "TN": "Tennessee", "TX": "Texas", "UT": "Utah", "VT": "Vermont", "VA": "Virginia", "WA": "Washington",
    "WV": "West Virginia", "WI": "Wisconsin", "WY": "Wyoming",
}


def area_files(areas_dir=AREAS):
    """Each Area file in `areas_dir`, read once: [(path, text, config)]."""
    out = []
    for path in sorted(Path(areas_dir).glob("*.json")):
        text = path.read_text(encoding="utf-8")
        try:
            cfg = json.loads(text)
        except ValueError:
            continue
        if isinstance(cfg, dict):
            out.append((path, text, cfg))
    return out


def public_divisions(areas, trees=()):
    """The divisions whose names stay (public structure that names no school), as (ids, names):
    the nation, the levels (`High School`), the States and each State's Areas (its Area level in
    its Area file, `Section` in California), from the Area files (`areas`, as `area_files` gives
    them: every State's name, and the Areas' names) and from division trees (`GetNavInfo`
    answers as dicts: their ids too). A league or conference below them (a school's own, in its
    chain) would narrow a school down past its Area: its name is not one of these."""
    levels, names, ids = {}, {"High School", "United States"}, set()
    names.update(STATE_NAMES.values())
    for _, _, cfg in areas:
        if cfg.get("state") and cfg.get("level"):
            levels[cfg["state"]] = cfg["level"]
        for area in cfg.get("areas") or []:
            if isinstance(area, dict) and isinstance(area.get("name"), str):
                names.add(area["name"].strip())
    for tree in trees:
        for key in ("divListId", "levelDivId", "countryDivId", "regionDivId"):
            if isinstance(tree.get(key), int):
                ids.add(tree[key])
        for key in ("levels", "countries", "regions", "tree"):
            for r in tree.get(key) or []:
                if not isinstance(r, dict):
                    continue
                kept = key in ("levels", "countries") or r.get("divType") in ("Country", "Level", "State") or (
                    r.get("state") and r.get("divType") == levels.get(r.get("state")))
                if kept:
                    if isinstance(r.get("id"), int):
                        ids.add(r["id"])
                    if isinstance(r.get("name"), str):
                        names.add(r["name"].strip())
    return ids, names


def seed_key(seed):
    """A key from a small number: for tests and made-up data only, never secret."""
    return ("seed:%d" % seed).encode()


def load_key(path):
    """The secret key in `path` (64 hex digits), made from 256 random bits when the file is
    missing. The file undoes the anonymization, so it must be private (`check_private`)."""
    path = Path(path)
    check_private(path)
    no_symlink(path)
    if path.exists():
        text = path.read_text().strip()
        if not re.fullmatch(r"[0-9a-f]{64}", text):
            raise AnonymizeError("%s does not hold a key (64 hex digits)" % path)
        return bytes.fromhex(text)
    path.parent.mkdir(parents=True, exist_ok=True)
    fd = os.open(str(path), os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "w") as out:
        out.write(secrets.token_hex(32) + "\n")
    return bytes.fromhex(path.read_text().strip())


def prepare(recording, out_dir, key, replace=False, areas_dir=AREAS, keep=(), latest=None):
    """Anonymize `recording` for `out_dir`, writing nothing: the Report and the outputs, which
    `write_dir` writes."""
    recording, out_dir = Path(recording), Path(out_dir)
    if not recording.is_dir():
        raise AnonymizeError("%s is not a directory" % recording)
    rec, out = recording.resolve(), out_dir.resolve()
    if out == rec or rec in out.parents or out in rec.parents:
        raise AnonymizeError("the output and the recording must not hold each other")
    if out_dir.exists() and any(out_dir.iterdir()):
        if not replace:
            raise AnonymizeError("%s is not empty; pass --replace to replace it" % out_dir)
        if not (out_dir / MARKER).is_file():
            raise AnonymizeError(
                "%s is not an earlier anonymizer output (no %s in it), so --replace leaves it alone"
                % (out_dir, MARKER))
    if areas_dir is not None and not Path(areas_dir).is_dir():
        areas_dir = None
    return Anonymizer(key, areas_dir, keep, latest).run(recording)


def anonymize(recording, out_dir, seed=None, replace=False, areas_dir=AREAS, key=None, keep=(), latest=None):
    """Anonymize `recording` into `out_dir` with `key` (or, for tests, `seed`); the Report says
    what changed."""
    report, outputs = prepare(recording, out_dir, key if key is not None else seed_key(seed),
                              replace=replace, areas_dir=areas_dir, keep=keep, latest=latest)
    write_dir(out_dir, outputs)
    return report


def resolved(path):
    """`path` absolute with every symlink in it resolved, a dangling one too (to where it points),
    as `ath`'s `record::resolved` does."""
    return Path(os.path.realpath(os.path.abspath(str(path))))


def no_symlink(path):
    """Refuse a private destination that is itself a symlink: it would be written through."""
    if os.path.islink(str(path)):
        raise AnonymizeError("refusing to write %s: it is a symlink" % path)


def check_private(path):
    """Refuse a private file (`--map-out`) in a git work tree that does not ignore it, as `ath`
    refuses a record dir (`record::check_private`): the innermost work tree decides, by `git
    check-ignore`; outside any work tree anything goes; without git, only its `local/` passes."""
    path = resolved(path)
    root = next((p for p in [path] + list(path.parents) if (p / ".git").exists()), None)
    if root is None:
        return
    try:
        ignored = subprocess.run(["git", "-C", str(root), "check-ignore", "-q", "--", str(path)],
                                 stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode == 0
    except OSError:
        rel = path.relative_to(root)
        ignored = len(rel.parts) >= 2 and rel.parts[0] == "local"
    if not ignored:
        raise AnonymizeError(
            "refusing to write %s: it undoes the anonymization, and the git work tree %s does not "
            "ignore it; put it under %s/local/ (git ignores local/) or outside any git work tree"
            % (path, root, root))


def writable(path, kind):
    """Refuse `path` (a file or a dir to write) unless it is, or can be made, of that kind:
    checked before anything is written."""
    path = Path(path)
    if path.exists() and (path.is_dir() if kind == "file" else not path.is_dir()):
        raise AnonymizeError("%s is not a %s" % (path, kind))
    parent = path.parent if kind == "file" else path
    while not parent.exists():
        parent = parent.parent
    if not parent.is_dir() or not os.access(str(parent), os.W_OK):
        raise AnonymizeError("cannot write %s (%s is not a writable folder)" % (path, parent))


def shape(node):
    """What a value is, never the value itself (`--list-unreviewed` prints it next to real data):
    its JSON type, and for a string what it looks like."""
    if node.kind == "string":
        v = node.value
        if not v:
            return "empty string"
        if DATE.match(v):
            return "date"
        if v.isdigit():
            return "digit string"
        if re.match(r"(?i)^(https?:)?//|^/", v):
            return "link"
        return "string"
    if node.kind == "number":
        return "int" if isinstance(node.value, int) else "float"
    if node.kind == "literal":
        return "null" if node.value is None else "bool"
    return node.kind


def reviewed_literals():
    """Every key the anonymizer knows by name (any kind's reviewed keys, the keys handled
    anywhere, the reviewed and blanked paths' parts)."""
    out = set().union(*ALLOWED_KEYS.values()) | LOCATION_KEYS | URL_KEYS | VENUE_KEYS | {"<id>"}
    for paths in list(REVIEWED_PATHS.values()) + list(BLANKED_PATHS.values()):
        for path in paths:
            out.update(part.replace("[]", "") for part in path.split("/"))
    return out


def masked_path(where, known):
    """`where` with every key it has that is no known literal shown as `<key>`: an unknown key
    may be an id, a name or a handle (a dict keyed by them), never printed."""
    parts = []
    for part in where.split("/"):
        bare = part.replace("[]", "")
        parts.append(part if bare in known else "<key>" + part[len(bare):])
    return "/".join(parts)


def unreviewed(recordings):
    """Every distinct key path no one has reviewed in `recordings` (response kind, path with
    each key no one knows masked, `<key>`): how often it occurs, how many distinct keys stand
    behind it, and the shapes of its values (never a value or an unknown key: the Recordings
    are real). Open the Recording at that path to review it."""
    found, known = {}, reviewed_literals()
    for recording in recordings:
        anon = Anonymizer(seed_key(0))
        anon.unknown = []
        for f in anon.read(Path(recording)):
            anon.actions(f)
        for _, kind, where, node in anon.unknown:
            entry = found.setdefault((kind, masked_path(where, known)), {"count": 0, "shapes": set(), "keys": set()})
            entry["count"] += 1
            entry["keys"].add(where)
            entry["shapes"].add(shape(node))
    return found


def map_json(report, source):
    return json.dumps({
        "key": source,
        "ids": {str(k): v for k, v in sorted(report.id_map.items())},
        "values": dict(sorted(report.value_map.items())),
        "value_kinds": {k: sorted(v) for k, v in sorted(report.value_kinds.items())},
        "date_shift_days": report.date_shift,
        "mark_maps": {k: {"kind": m.kind, "unit": m.unit, "sort_int_deltas": {str(d): x for d, x in sorted(m.deltas.items())}}
                      for k, m in sorted(report.event_maps.items())},
        "standards": [{"file": x["file"], "gender": x["gender"], "event": x["code"], "real": x["display"], "mapped": x["new"]}
                      for x in report.standards],
        "year_shift": report.year_shift,
        "dropped": {str(k): v for k, v in sorted(report.dropped.items())},
        "inserted": {str(k): v for k, v in sorted(report.inserted.items())},
        "grade_offsets": {str(k): v for k, v in sorted(report.grade_offsets.items())},
    }, indent=1, ensure_ascii=False) + "\n"


def main(argv=None):
    parser = argparse.ArgumentParser(
        description="Anonymize a Recording (ath --record-dir) into fixtures (ADR 0010, docs/anonymize.md).")
    parser.add_argument("recording", help="the Recording: a --fixtures-dir layout of real responses")
    parser.add_argument("out", nargs="?", help="where to write the anonymized copy (the same layout)")
    secret = parser.add_mutually_exclusive_group()
    secret.add_argument("--key-file", metavar="FILE",
                        help="the secret key (256 random bits, made when FILE is missing): keep it private, "
                             "in a path git ignores such as local/anonymize.key")
    secret.add_argument("--seed", type=int, help="tests and made-up data only: a small, NOT secret key")
    parser.add_argument("--replace", action="store_true", help="replace an earlier output of this tool")
    parser.add_argument("--keep", metavar="IDS", default="",
                        help="athletes (real ids, comma-separated) never dropped from the lists, beyond those "
                             "with a bio or on a meet's entries")
    parser.add_argument("--map-out", metavar="FILE",
                        help="also write the maps (real to invented ids and names, the date shift, each "
                             "event's mark map and the mapped standards) to FILE, which undoes the "
                             "anonymization: only where git ignores it")
    parser.add_argument("--areas", metavar="DIR", default=str(AREAS),
                        help="the Area files whose State Meet standards no mark may cross (default: the repo's areas/)")
    parser.add_argument("--areas-out", metavar="DIR",
                        help="also write a copy of the Area files with the standards mapped as the marks are, "
                             "for pages built from the anonymized copy")
    parser.add_argument("--latest-season", metavar="YEAR", type=int,
                        help="the real outdoor season that becomes 2099 (default: the Recording's latest with "
                             "data); give the same one to every Recording whose copies are used together")
    parser.add_argument("--list-unreviewed", action="store_true",
                        help="only list the distinct key paths no one has reviewed, in the recording and in "
                             "OUT when given as another recording (paths, counts and value shapes, never a "
                             "value), writing nothing")
    args = parser.parse_args(argv)
    if args.list_unreviewed:
        try:
            found = unreviewed([args.recording] + ([args.out] if args.out else []))
        except AnonymizeError as e:
            print("anonymize: %s" % e, file=sys.stderr)
            return 1
        for (kind, where), entry in sorted(found.items()):
            print("%-9s %-60s %7d  %4d keys  %s" % (kind, where, entry["count"], len(entry["keys"]),
                                                    ", ".join(sorted(entry["shapes"]))))
        print("%d unreviewed key paths" % len(found))
        return 0
    if args.out is None or (args.key_file is None and args.seed is None):
        parser.error("OUT and one of --key-file and --seed are required")
    report = None
    try:
        # Every destination is checked before anything is written.
        if args.map_out:
            check_private(args.map_out)
            no_symlink(args.map_out)
            writable(args.map_out, "file")
        if args.areas_out:
            writable(args.areas_out, "folder")
            out = resolved(args.areas_out)
            for kept in {resolved(args.areas), resolved(AREAS)}:
                if out == kept or kept in out.parents or out in kept.parents:
                    raise AnonymizeError(
                        "--areas-out %s must be apart from the Area files it copies (%s): the real "
                        "standards stay for real use" % (args.areas_out, kept))
        writable(args.out, "folder")
        if args.key_file:
            writable(args.key_file, "file")
            key, source = load_key(args.key_file), "the key in %s" % args.key_file
        else:
            key, source = seed_key(args.seed), "seed %d (tests only: not secret)" % args.seed
        keep = [int(x) for x in args.keep.split(",") if x.strip()]
        report, outputs = prepare(args.recording, args.out, key, replace=args.replace, areas_dir=args.areas, keep=keep,
                                  latest=args.latest_season)
        if args.map_out:
            Path(args.map_out).parent.mkdir(parents=True, exist_ok=True)
            Path(args.map_out).write_text(map_json(report, source))
        write_dir(args.out, outputs)
        if args.areas_out:
            write_areas(args.areas, args.areas_out, report.standards, report.year_shift)
    except (AnonymizeError, OSError) as e:
        if report is not None:
            print(report.summary(source))
        print("anonymize: %s" % e, file=sys.stderr)
        return 1
    print(report.summary(source))
    return 0


if __name__ == "__main__":
    sys.exit(main())
