# CONTEXT

Ubiquitous language and durable domain facts for `ath`.
The full build spec is `docs/HANDOFF.md`; this file is the glossary and the "why".
Architectural decisions live in `docs/adr/`.

## Language

**Seed sheet**:
For one event, a ranked list of entrants by Seed mark (fastest/farthest first; no-mark rows to the bottom).
The output artifact.
Seed sheets are only built for upcoming events that have no results yet, so their marks are always as of today (as of the day before, for a meet today, so the meet's own results never count).
_Avoid_: results, rankings, start list

**Seed mark**:
The mark an entrant is seeded on: their Personal record in the event (the fastest/farthest they have ever done it, not just this season).
With no mark in the event, an Estimated mark may stand in, flagged as such.

**Estimated mark**:
A mark made up for an event the entrant has no mark in, derived from related evidence: other events (a 400m from 200m and/or 800m marks) or relay splits.
Always flagged; never mixed silently with real marks.
The first one `ath` makes, for relays: on a 4xMile, a runner's 1 Mile Personal record converted to the 1600m at even pace (`mile x 1600 / 1609.344`, on `SortInt`, rounded up to the next hundredth), used when it is faster than their 1600m Personal record; the leg says it is estimated and what it came from, and the team that it includes estimated legs.

**Mark**:
A single performance result - a time (running) or a distance (field).
_Avoid_: result, PR

**Qualifying mark**:
A mark that passes the `HANDOFF.md` §4 filter (event resolved by name, outdoor, individual not a relay split, not exhibition, not hand-timed, a real mark not DNS/DNF/NT/SCR/DQ/FS).
v1 counts outdoor marks only: an indoor mark (`SeasonID` 10000 + year, same `EventID` as outdoor) is never a Qualifying mark, so it never becomes a Seed mark, Season best or Personal record.
Season best and Personal record are each the best Qualifying mark over their window.
_Avoid_: best time

**Season best**:
A person's best mark in one event within one season, as of a date (default: today).
Computed from the Profile's results: the lowest `SortInt` among Qualifying marks in that season dated on or before the date.
For a Competitor, the season's rankings list gives it directly when the date is today or the season is over; any other date needs their Profile.
_Avoid_: SR, SB (in prose)

**Personal record**:
A person's best mark in one event across all seasons, as of a date (default: today).
Computed from the Profile's results: the lowest `SortInt` among Qualifying marks in any season dated on or before the date.
Athletic.net's per-result `PersonalBest` / `SeasonBest` fields are bitmasks that mark only the results that are the best today, not results that were a best when set, so they cannot answer "as of a date" (checked 2026-10-26 on a live Profile); at most they cross-check today's answer.
_Avoid_: PR (in prose), lifetime best

**Area**:
The level directly below a State in Athletic.net's hierarchy, which each state names its own way (California: the CIF sections, such as Central Coast).
A person belongs to an Area through their school, never their club.
Their school for a season is the one the Profile's `grades` map lists them on that season (`Year` only when it lists none); a middle school decides no Area.
Athletic.net's Area list decides membership; we never infer it.
Membership is per season: each season every Area gets a new list id, and a school's Area for a season is the Area list in its division chain for that season.
Which level counts as the Area is configured per State, in `areas/<ST>.json` (`docs/areas.md`); a State without a file has no Areas.
Pages call Areas by the State's own word from that file (California: Section).
_Avoid_: section (except when speaking about California), region, district

**Rankings list**:
Athletic.net's own ranked list of people by Season best for one State or Area, event, season and Grade group.
The source of truth for who is in an Area and for Competitors' current marks.
_Avoid_: performance list (Athletic.net's page title), leaderboard

**Grade group**:
The grades a Rankings list is filtered to.
Reports compare an Athlete against 9th grade through their current grade, then widen one class at a time up to all of high school ("All HS", 9th-12th); a 9th grader also gets 9th alone.

**Neighborhood**:
The Competitors a report shows around an Athlete on one Rankings list.
At rank 50 or better: everyone ahead plus the 3 just behind.
Past 50: the leader, the 3 just ahead and the 3 just behind.
Rank counts ties (T50 is "50 or better").

**SortInt**:
Athletic.net's numeric sort key.
Running = milliseconds; field = `20_000_000 − inches×1000`.
Lower is always better.
Sort and sum on this, never the display string.
Rankings list rows carry it as `SortIntCalc` (alongside `SortIntRaw` and `SortIntOrig`); there is no plain `SortInt` there.

**Seeding event**:
The event a seed is computed from, which may differ from the nominal event (a 4×Mile relay seeds on each runner's 1600m mark).
Relay-only: a relay whose four legs are one flat race seeds on that race (4x100m on the 100m, 4x200m on the 200m, 4x400m on the 400m, 4x800m on the 800m, 4xMile on the 1600m, with the mile as an Estimated mark when faster), read from the relay's code or name, and `ath seed --seed-on` names another (with no Estimated marks).
A team's seed is its aggregate: the sum of its four runners' Personal records in the Seeding event, summed on `SortInt`.
Medley and shuttle relays have no one Seeding event (their legs differ), so they are not seeded yet.

**Entrant**:
Who competes in an event - a person (individual/field) or a team + its runners (relay).

**Candidate**:
A possible Profile match for a Mode B roster row, carrying the locators (school, city, state) and score used to rank it.
_Avoid_: match, hit, result

**Profile**:
A person's record on Athletic.net, identified by its athlete id.
Every Athlete and every Competitor has one.
_Avoid_: bio (that is one endpoint that returns a Profile's data)

**Athlete**:
A person a report is about.
We know their full picture: every event and season, the teams they have competed for, and their marks over time.
An Athlete's data supports everything a Competitor's does.
A report has zero or more Athletes.
_Avoid_: subject, tracked athlete

**Competitor**:
Anyone else who appears in a report.
We need only what the report needs: name, team, grade, Profile, and their mark for the report's event and season.
_Avoid_: opponent, rival

**Masked row**:
A placeholder row Athletic.net puts on a Rankings list past what the session may see (25 rows signed in without a paid subscription): an all-`X` name, athlete and team id 0, an empty mark.
Never an Entrant, never ranked; a list that ends in one is capped, not complete (ADR 0001).

**Recording**:
Real Athletic.net responses saved locally under `local/` to build from, written by `ath --record-dir` in the layout `--fixtures-dir` reads, so a command replays offline (`docs/anonymize.md`).
Never committed, and never holds a token.

**Anonymizer**:
The deterministic tool (`tools/anonymize/anonymize.py`) that turns Recordings into committed fixtures, golden pages and Examples: invented people, schools, teams, meets and ids, fictional seasons ending at 2099 with the dates moved along, one rank-preserving mark map per event (a better-mark baseline plus a jitter bounded by the gaps, so ties and standards hold but the gaps do not), reshuffled lists, some athletes swapped for invented ones a few places away, so ranks are realistic, not real, and grades swapped in pairs among the other athletes of the same lists (ADR 0010).

**Example**:
A published report page built from anonymized data, with a footer that says so.
Its links go to one demo page, never to Athletic.net.
_Avoid_: demo, sample (for the pages themselves)

## Input modes (how we learn who to seed)

- **Mode A - meet entries.** Given a meet, a gender, a division and an event, fetch the Entrants from Athletic.net's Meet API: entries that are accepted (`AcceptanceStatus` 10) and not scratched.
  It reads the meet's event list too, so a seed sheet config can name its events (the default) or take all of them, and relay entries carry the team and its legs.
  A roster CSV (Mode B) stays the fallback for meets with no entries online and an override per event; one `ath seed` run takes one or the other, never both.
- **Mode B - supplied roster.** The user hands in a CSV with `name` (required) and optional `school`, `city`, `state`.
  `ath` resolves each row to an Athletic.net athlete via search.
  **When the match is not unambiguous, return ranked candidate matches rather than silently picking one.**

## Athletic.net access (discovered 2026-07-07)

- Everything is an Angular SPA backed by `https://www.athletic.net/api/v1/...` JSON endpoints.
- **The API is behind Cloudflare anti-bot.** A plain HTTP client (curl/reqwest) gets a Cloudflare "Just a moment…" challenge, **not** JSON - confirmed for both `AutoComplete/search` and `AthleteBio/GetAthleteBioData`.
  A real browser works because it carries a `cf_clearance` cookie + matching fingerprint + login.
  **This - not merely "does it need auth" - is the core access problem.**
  (The handoff's "~40 calls → HTML instead of JSON" throttle was almost certainly Cloudflare, too.)
- **Athlete search:** `GET /api/v1/AutoComplete/search?q=<name>&fq=type:Athlete`.
  A Solr response: `response.docs[]` mixes types (`Athlete`, `XCMeet`, `Team`, ...) - filter to `type == "Athlete"`.
  Each athlete doc: `id_db` (athlete id, string), `textsuggest` (name), `subtext` (`"School (HS)||City, State"`), `gender`, `l` (team levels), `score` (relevance).
  `fq=type:Athlete` scopes the query to athletes.
  Maps directly onto Mode B's `name, school, city, state`.
  (Verified 2026-07-29.)
- **Bio (marks):** `GET /api/v1/AthleteBio/GetAthleteBioData?athleteId=<id>&sport=tf&level=0`.
- **Entries API:** `Meet/GetMeetData` (the meet and a `jwtMeet` token), `Meet/GetEventListData` and `Meet/GetEntriesData` (both sent with the token), confirmed 2026-11-06 on an open indoor meet with relays; the shapes are in `docs/agents/systems/athletic-net.md` (Meets).
  Past meets cannot be used: no account (signed out, unpaid or paid) sees a meet's entries after the meet ("Entries will not be published").
- **Paid subscription:** full Rankings lists and Profile histories need a paid Athletic.net subscription, enforced by the server (ADR 0001).

### Bio API shape (verified 2026-07-09 against a real Profile)

- `athlete`: `{ IDAthlete, SchoolID (current/primary), FirstName, LastName, Gender, Handle, isClaimed, ... }`.
  `synonyms[]` lists merged identities.
- **`allTeams`** - map keyed by SchoolID, one entry per affiliation: `{ IDSchool, SchoolName, TeamCode, Level, Year }`.
  **`Level` classifies the team**: `2` = middle school, `4` = high school, `16` = club (higher levels ≈ college).
  This is how we tell **school** vs **club**.
- **`grades`** (checked 2026-10-26) - map keyed `"<schoolId>_<season>"` to the grade, one entry per team per season the athlete was on it, clubs included (club values can be odd, such as 402), and indoor seasons too (`12026` is 2026 indoor).
  It is the best evidence of which school was a person's in a season, better than `allTeams` `Year` (the last season on a team, sometimes missing).
- **`resultsTF[]`** - each result carries its **own `SchoolID`** = the team the mark was set *under* (e.g. a high schooler's marks at a club meet are under the club, not her school).
  Fields confirmed: `Result`, `SortInt` (for example 27.64a→27640, an illustrative value; DNS→20000001), `EventID`, `EventTypeID` (0=individual), `SeasonID`, `Exhibition`, `Round`, `FAT`, `Wind`, `AthleteID`.
  Also (2026-10-26): `ResultDate` (`2026-05-16T00:00:00`), `MeetID`, `meetName`, `SchoolID`, and the `PersonalBest` / `SeasonBest` bitmasks (0, 3, 6, 7, 8, 14, 15, 16, 30 seen), which flag only the results that are a best today.
  `SortInt` keeps the thousandths the display rounds away (an illustrative example: `12.93a` → 12925).
- **Relay noise:** relay results appear in `resultsTF` under the *relay's* TeamID as `AthleteID`.
  Filter an individual's marks by `AthleteID == target` + resolved `EventID` (by name) + `EventTypeID == 0`.
- **Browser-with-cookie reaches this JSON** (past Cloudflare) - confirms the `--cookie` approach works end to end.

### Team model (school + club)

An athlete has **multiple team affiliations** (typically a school + a club), changing over time; marks generally show the same regardless.
State and Area lists rank people, not teams: a person is listed by school-based membership, but their marks count whichever team they were set under (verified 2026-10-26: a 9th grader's 100m set under her club ranks on the Central Coast Section 9th-grade list through her school; a school in San Francisco can be in the Central Coast Section through its league, not the San Francisco Section).
Location (via team) is mainly a **disambiguation** aid.
Output shows both **school** and **club** when determinable from `allTeams` (classified by `Level`).

## Scope

- **v1:** individual running events only, **via Mode B** (roster CSV) fed the 625323 running entrants, validated by diffing against the hand-built seed sheet.
  That validation is historical: the hand-built sheet seeds on 2026 Season best, and `ath seed` now seeds on the Personal record, so the two differ wherever an entrant ran faster in an earlier season.
  Mode B is unblocked today (search + bio both reachable with `--cookie`); Mode A's entries API was confirmed 2026-11-06, and `ath seed --meet` seeds individual running events from it.
- **After v1:** Mode A (meet entries, including relays and a meet's whole event list), relays in `ath seed` and the seed sheet page, and a meet index page.
  Mode A is confirmed on an open indoor meet; seeding stays on outdoor Personal records.
  Relays of four equal legs are seeded from a meet's entries (`ath seed --meet`) and rendered by the seed sheet page; a roster (Mode B) names no teams, so it never seeds a relay, and medley and shuttle relays come later.
- **Next:** field events (shot put), and indoor seeding.
