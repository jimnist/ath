# Recordings and the anonymizer

Fixtures, golden pages and the published examples are built from real Athletic.net responses, anonymized (ADR 0010).
Two tools do it:

1. `ath --record-dir <DIR>` saves every response a run reads: a Recording.
2. `tools/anonymize/anonymize.py` turns a Recording into an anonymized copy with the same layout.

Both copies replay offline with `--fixtures-dir`: a test or an example runs on the anonymized copy as `ath` ran on the real data, with what the privacy model below changes.

## The privacy model

- **Names** of athletes, schools, clubs, cities, venues and meets are invented, the same real one becoming the same invented one everywhere; free text, tokens, locations and contacts are blanked; every response key is on a reviewed list, or the run stops.
- **Ids** of athletes, schools, teams, meets, results, users, and of lists and divisions are private: all move into a reserved range, so nothing can be matched with the public site by an id.
- **Seasons** are fictional, ending at 2099, and every date moves with its season (and a few days).
- **Marks** move by one rank-preserving map per event on the `SortInt` lists sort by: order, ties and State Meet standards hold, gaps do not.
- **Rows**: some athletes leave every list for invented stand-ins a few marks away, so ranks are realistic, not real.
- **Grades** are swapped in pairs among those same athletes, so every list keeps its per-grade counts while most rows show another grade.
- **Kept as recorded**: subjects (athletes with a bio), meet entrants and `--keep` athletes, with their grades (and the few athletes whose recorded lists their grade does not explain, and those a replay needs where they were); the names of the nation, the levels (`High School`), the States and their Areas (division names below the Area level, leagues and conferences, are blanked); event names and ids.
- **Residual risk**: a row's Area and approximate mark still narrow it down to that Area's athletes near that mark (ADR 0010).

## Recording a scenario

Add `--record-dir` to any command that reads Athletic.net (`profile`, `areas`, `list`, `rank`, `meet`, `seed`):

```bash
ath rank 12345678 --event 400m --season 2026 --in CA,CCS --ahead all \
  --record-dir local/recordings/rank-ca-ccs-girls-400m
```

The command runs as usual and also writes each response into the folder, in exactly the layout `--fixtures-dir` reads:

| Response | File |
| --- | --- |
| Bio (`GetAthleteBioData`) | `<athlete_id>.json` |
| School's division chain (`TeamNav/Team`) | `teamnav/<school_id>-<season>.json` |
| Season's division tree (`GetNavInfo`) | `navinfo/<season>.json` |
| A State's own subtree (`GetNavInfo` with the State list's token) | `navinfo/<season>-<state_list>.json` |
| Rankings page (`GetRankings`) | `rankings/<list_id>-<gender>-<event>-<grades>-p<page>.json` |
| Meet (`GetMeetData`) | `meets/<meet_id>/meet.json` |
| Meet's event list (`GetEventListData`) | `meets/<meet_id>/events.json` |
| One event's entries (`GetEntriesData`) | `meets/<meet_id>/entries/<gender>-<div_id>-<eventShort>.json` |
| Athlete search (`AutoComplete/search`) | `search/<query>.json`, with the query added to it as `athQuery` |

A search file's name is the query lower-cased, each run of other characters one `-` (`Jane O'Doe` is `jane-o-doe`); when that drops a character beyond ASCII, or leaves nothing, the first 8 hex digits of the SHA-256 of the lower-cased query follow (`José Núñez` is `jos-n-ez-43884fd3`, `山田 太郎` is `ed6d53c9`), so every query has its own name.
`ath` (`source::search_fixture_name`) and the anonymizer (`search_fixture_name`) spell it the same way.

So the same command with `--fixtures-dir local/recordings/rank-ca-ccs-girls-400m` (and no `--record-dir`) prints the same output, offline.
Several commands can record into one folder; a response read twice is written once, with the same content.

What it records, and what it never does:

- **Every response the run answers with, cached ones too.**
  A run the cache serves records the same files as one that fetched them, so recording never needs `--refresh` or `--no-cache`.
  Responses read only along the way (the rankings request that fetches a token, the season hop in `ath areas`) are not recorded, since replaying never asks for them.
- **The season's tree is the national one.**
  A tree answer that is only part of the national tree (as some lists' tokens answer) is recorded as its list's own (`navinfo/<season>-<list>.json`), and as the season's only while no national tree is recorded, which always takes its place.
- **Safe file names.**
  A name part that comes from Athletic.net or the command line (an event code) is used as it is when it is only ASCII letters and digits (`400m`, `Shot`); anything else is spelled as its lower-cased letters and digits plus a short hash, as search names are, so it can never name another folder, and nothing is ever written outside the record dir.
- **No tokens.**
  A rankings page's `jwtTFTopReport` is written as `""` (the rest of the page as Athletic.net sent it), and a meet is written as the cache keeps it, without `jwtMeet`.
  The session cookie is never part of any response.
- **Only into a private folder.**
  A Recording holds real people, so `ath` refuses a record dir inside a git work tree unless that work tree's git ignores it (`git check-ignore`; the innermost work tree decides, symlinks followed first, and without a working `git` only its `local/` passes).
  Use `local/recordings/<scenario>/` (the repo ignores `local/`), or a folder outside any git work tree.
  When `local/` is its own private git repo, that repo decides: add `recordings/` to its `.gitignore`.
- **Not with `--fixtures-dir`**, which already reads a recording; `ath` refuses the two together.

A command that fails part way still keeps what it recorded until then (a rankings page `ath` refuses, such as the 100m list Athletic.net answers for an unknown event code, is recorded before it is refused).

## Anonymizing a Recording

```bash
python3 tools/anonymize/anonymize.py local/recordings/rank-ca-ccs-girls-400m /tmp/rank-ca-ccs \
  --key-file local/anonymize.key
```

It reads the Recording (dotfiles, such as `ath`'s own `.<name>.<pid>.<n>.tmp` and the output marker, are skipped; any other file outside the layout stops it), writes the anonymized copy to the output folder, and prints a summary: how many ids, names, dates and marks changed, the date shift and each event's mark shift.
Python 3.9 or later, standard library only.

- **The output folder** must be empty or missing, and neither hold the Recording nor be inside it.
  Each output carries a `.anonymized` marker; `--replace` replaces only a folder with that marker (an earlier output), never anything else.

- **`--key-file FILE`** holds the secret every invented value and mark map is derived from: 256 random bits (`secrets.token_hex(32)`), written to FILE (mode 600) when it is missing.
  The same Recording and key always give byte-for-byte the same output.
  Keep it private, like the Recording: the invented names are keyed hashes of the real ones, so with the key and this public tool anyone could check a guessed name against the output.
  The tool refuses FILE in a git work tree that does not ignore it (`local/anonymize.key` is fine), and prints where the key is, never the key.
- **`--seed N`** is for tests and made-up data only: a small number standing in for the key, not secret (anyone can try every seed).
  One of `--key-file` and `--seed` is required.
- **`--map-out FILE`** also writes the maps (real to invented ids and names, the date shift and each event's shift) to FILE, for translating a command's arguments (an athlete or meet id) to the anonymized copy.
  It undoes the anonymization, so the tool refuses FILE in a git work tree that does not ignore it, the same rule as `ath --record-dir`.
  It is written before the output folder, so a run whose output fails to write still leaves the map.
- Every destination (the output folder, `--key-file`, `--map-out`, `--areas-out`) is checked before anything is written: it exists as the right kind or can be made, and the private ones pass the guard.
  The summary prints even when a late write fails.
- **`--areas DIR`** (default: the repo's `areas/`) names the Area files whose State Meet standards no mark may cross; **`--areas-out DIR`** also writes a copy of them with the standards mapped as the marks are (see below).
- **`--latest-season YEAR`** names the real outdoor season that becomes 2099 (by default the Recording's latest with data).
  Give every Recording whose copies are used together the same one, so a season is the same fictional year in all of them (the test fixtures and the examples use `--latest-season 2026`).
  It must be an outdoor season (a year, never an indoor `12026`) the Recording has (in its data, or in a division tree's list of seasons), and no earlier than its latest outdoor season with data, which would move past 2099: anything else stops the run.
  With the same key, the same real person, school or list gets the same invented name and id in every copy; marks do not: each Recording's event maps are its own.
- **`--list-unreviewed`** writes nothing: it lists the distinct key paths no one has reviewed in the Recording (and in OUT, when given as a second Recording), each with how often it occurs, how many distinct keys stand behind it and the shapes of its values (`int`, `string`, `link`, `date`...).
  It never prints a value, nor any key it does not know by name: every such part of a path is `<key>` (`athlete/<key>`), since an unknown key may be an id, a name, a handle or a composite (`<id>_<season>`); open the Recording at that path to review it.
  Review each, add it to `ALLOWED_KEYS` (or `REVIEWED_PATHS`, `BLANKED_PATHS`) with its handling, and add it to the tables below.

Then run the same commands on the copy, with the ids and seasons translated (`--map-out` has them) and the clock pinned to the moved date (`ATH_TODAY`, which `ath` reads as today: the copy's seasons end at 2099, so today's real date would find no marks yet; `ath` takes it only when replaying, with `--fixtures-dir`, refuses it on a run that reads Athletic.net, and always refuses a malformed one):

```bash
ATH_TODAY=2099-11-06 ath rank 9xxxxxxxx --event 400m --season 2099 --in CA,CCS --ahead all \
  --fixtures-dir /tmp/rank-ca-ccs
```

### What changes

- **Names.**
  Athletes (first, last and full names, a relay's `<BR>`-joined names, a search's `textsuggest`), schools and clubs (`SchoolName`, `TeamName`, a TeamNav `team.Name`, a search's `subtext`), team codes, cities, venues (`Location.Name`) and meet names become invented ones.
  The same real value becomes the same invented value in every file: a last name is mapped on its own and a full name is mapped as its first and last name, so `AthleteName`, `FirstName` / `LastName` and a search agree.
  Invented last names and places are made from syllables (`Brandford`, `Alderbank Springs`), never one of the Recording's real values; a school keeps its kind (`High School`, `Academy`, `Prep`, `Christian`, `TC` and so on), a venue its kind (`Park`, `Stadium`) and a meet its kind and year (`Relays 2026`).
  First names come from a list of common names: a first name alone names nobody, so one may happen to be a real athlete's first name.
  No invented part (a first name, a last name, a place) ever holds a real whole name the self-check forbids (an athlete's full name, a school, meet, venue or city), so a real city `Dallas` keeps `Dallas` off the first names; the kept kind is checked only as part of the whole invented value, so a real `Christian` or `Park` does not rule out `... Christian` or `... Park`.
  Division names stay only for the national, level, State and Area divisions (the Area level of each State's Area file, `Section` in California; the State and Area names from the Area files, and the ids and names from the recorded trees, so a Recording with no tree, of a meet or a Profile, keeps them too): a league or conference below the Area (a school's own, in its TeamNav chain, or in the season's tree, such as Texas' leagues) would narrow a school down past its Area, so its name is blanked, in a school's chain, the tree, and an entry's `DivName` and its parents; a division name that is a school's is invented as the school's.
  So the residual risk stays an Area and an approximate mark.
  A recorded search's query (`athQuery`) is mapped as a person's name, and the file is renamed by the mapped query.
  A relay team member's `Name` in a bio (`relayTeamMembers`) is a person's, invented as on the lists.
  Free text (`Comment`) and any token are blanked.
  Any other string has each real whole name in it replaced (also as a URL writes it, `jane-doe`), and each real id written as five or more digits.
- **Ids.**
  Athlete, school, team, relay team, meet, result, meet division row and user ids (`AthleteID`, `IDAthlete`, `SchoolID`, `IDSchool`, `TeamID`, `MeetID`, `IDMeet`, `IDResult`, `IDMeetDiv`, `OwnerUserId`, `userAthleteIdOnTeam`, a search's `id_db`, and the `ID` of a `meet`, `team` or `athlete`) move into the reserved range 900000000 to 999999999: `900000000 + HMAC(key, id) mod 100000000`, moved up past any collision.
  So do list and division ids, so a list cannot be matched with the public one by its id: a rankings page's `division` (`ID`, `ParentID`, `BaseDivID`, and its `BaseDiv`'s `ID` and `ParentID`), a TeamNav chain's `id` and `b` and a custom division's `IDDivision`, and the division tree (`divListId`, `levelDivId`, `countryDivId`, `regionDivId`, every region's, level's, country's and subtree's `id`, `seasons` values); the names of States, Areas and other divisions stay, so `ath` still finds a State's or an Area's list by name.
  So do a result's `IDResult`, a relay team member's `TeamID`, a school's custom list (`IDCustomList`, and the `customLists` keys) and a team's `ReplacementID`.
  One map serves every kind, so a relay team's id in a bio and in a rankings list stay the same id.
  Ids in keys and file names move too: `allTeams`, `meets` and `customLists` keys, a bio's `grades` keys (`<school>_<season>`), `<athlete_id>.json`, `teamnav/<school_id>-...`, `meets/<meet_id>/`, `rankings/<list_id>-...` and `navinfo/<season>-<state_list>.json`.
  Id `0` (a Masked row's, or an unattached athlete's team: `allTeams` key `0`, `grades` key `0_<season>`) stays `0`.
- **Seasons.**
  Every season moves by one number of years `Y`: the Recording's latest outdoor season with data becomes 2099 (with no outdoor season, the latest indoor season's year does; `--latest-season` names it instead), and every other season keeps its order and its distance from it.
  A division tree's `seasons` list does not count: it already has next season (no data yet), which moves past 2099 with the rest (2027 is 2100).
  An outdoor season `S` becomes `S + Y` and an indoor one `10000 + year` becomes `10000 + year + Y` (with 2026 the latest, `Y` is 73: 2026 is 2099, 2025 is 2098, indoor 12027 is 12100).
  Everywhere a season is: `SeasonID`, `seasonId`, a team's `Year`, `allSeasons` (`ID`, `IDSeason`, and the year in `Display`, `2099 Outdoor`), the season half of a bio's `grades` keys, the division tree's `seasons` keys, file names (`navinfo/<season>...`, `teamnav/<school>-<season>`), and the year a meet's name carries (`Relays 2099`).
  Grades stay as recorded: a 10th grader in 2026 is a 10th grader in 2099.
- **Dates.**
  Every date (`ResultDate`, `MeetDate`, `StartDate`, `EndDate`, `PublicEntries`, a bio meet's `EndDate`, any `YYYY-MM-DD[THH:MM:SS]` value) moves by the seasons' `Y` years (Feb 29 to Feb 28 when the new year has none), then by one number of days, from 1 to 3 in either direction (picked by the key).
  A day shift that would move any (moved) date into another year is skipped for the next candidate, so every date stays in its season (an indoor season's December stays in December); when none of the six works (dates on both Jan 1 and Dec 31), the tool stops and asks for the Recording to be split.
  A replay pins `ATH_TODAY` (and `--as-of`, `--meet-date`) to a recorded date moved the same way, which the summary prints.
- **Marks.**
  Each event has one map, applied to every mark of that event in every file: bio results (indoor marks and relay splits too, by their `EventID`), rankings rows (a Masked row's `SortIntCalc` too), and entries' seed marks.
  The map works on each mark's key `D`: the `SortInt` its list sorts by (`SortIntCalc` on a rankings row, the bio's `SortInt`, an entry's seed), never its display, so a hand time (`57.9h`, whose `SortInt` carries the hand-time conversion) keeps its own place against a FAT one (`58.10`); a State Meet standard, which has no `SortInt`, is keyed by its display on the same scale (a metric field mark, `12.34m`, converted to inches as Athletic.net does). The same real mark becomes the same new mark everywhere, and ties stay ties.
  The display moves by its mark's delta, keeping its own relation to its `SortInt`.
  Every distinct value `D` of the event (across the whole Recording, and the State Meet standards of the event) moves by its own `delta(D) = (b + j(D)) x u`:
  - `u` is the event's coarsest display step: 0.01 s, 0.1 s (when any mark is hand-timed in tenths) or 1 s for running; 0.01 in, a quarter inch (when every mark is on quarters) or 1 in (when any mark or standard is in whole inches) for field;
  - `b`, the baseline, is minus the event's median mark times a keyed 0.4% to 1.2%, in units, at least one: always toward a better mark (faster, farther);
  - `j(D)`, the jitter, is keyed per value, between `-(G_below - u) // 2u` and `(G_above - u) // 2u`, with `G` the gap to each neighbouring value (0 where the gap is under three units), and short of `-b`, so every delta stays better and nonzero.
  Two neighbours `a < b` then move at most `G - u` toward each other: they never cross or meet, so the order of every distinct value is kept (values closer than a unit keep their order and never meet), while the gaps change, so the list's gap pattern cannot be matched back to the real list.
  The leader moves to a better mark than any in the Recording.
  `SortInt` and its siblings (`SortIntRaw`, `SortIntCalc`, `SortIntOrig`, `OverrideSortInt`) move by the mark's delta, and so does the display string (`Result`, `display`), in its own format: minutes appear or go as the time crosses a minute, a `a` or `h` suffix stays, feet-inches keep their inch width and decimals.
  Every rank, tie, Grade group and "meets the standard" outcome comes out as recorded; buckets of a fixed width may change by a place at their edges, since gaps change.
  - **Linked events:** the 1 Mile (and 2 Mile) moves through its 1600m (3200m) estimate, which `ath` compares with a runner's 1600m: the estimate of every mile mark (`mile x 1600 / 1609.344`, rounded up to a hundredth, as `seed_relay::mile_to_1600m`) is a point of the 1600m map, and the mile's delta is that point's delta times 1609.344 / 1600, rounded to the mile's own display step.
    Every athlete's mile-against-1600m decision (estimate faster, equal or slower) is then checked; when one would change, the 1600m jitter is drawn again (up to 50 keyed draws), else the run fails.
    A mile standard is a point of the mile's map (its estimate a point of the 1600m's), so no mile crosses it either.
  - **Points:** a multi-event's score (Pentathlon, Heptathlon, Decathlon: any event named `...athlon` or points) is no mark: left as it is, never read as a time.
    An event's kind comes from its name (`Meters`, `Mile`, `Relay`, `Hurdles`... run; `Jump`, `Vault`, `Shot`, `Discus`... field); a name that says neither is a time only when its displays have a `:` or decimals.
  Non-marks (`DNS`, `DNF`, `NT`, `SCR`, `DQ`, `FS`, an empty display) and their sentinel `SortInt` (`20000001`) stay as they are.
  A mark display the tool cannot read for its event's kind stops the run.
- **Unchanged:** an entry's `ConversionInt` (0 in every answer seen, and its meaning unknown: `ath` never reads it), the JSON text but for the values replaced (same keys in the same order, same types, same whitespace; a rankings page's rows are rebuilt from its own rows' text, below), `IDDiv` (a meet's own division id, in its entries' file names), event ids, names and codes, grades as numbers (who has which is swapped, below), the names of the nation, the levels, the States and their Areas (below the Area level, division names are blanked), `minCount`, each list's length and pages, Masked rows.

### Reshuffled lists

Real ranks on kept lists would let anyone pair each row with the public list, mark or no mark.
So each list loses some of its athletes and gains invented ones elsewhere in it (ADR 0010: ranks in examples are realistic, not real):

- **Dropped, per athlete.**
  A keyed 10% to 20% of the athletes on any rankings list go, from every list and page they are on (State, Area, every Grade group), and from any search's docs; then, for every list whose kept athletes would still stand rank for rank as recorded, one more of its athletes goes.
  Never dropped (protected): an athlete with a bio in the Recording (a subject), one on a recorded meet's entries (seed sheets stay whole), and any `--keep <id>,...` (real ids).
  Nor are the few athletes whose recorded lists their grade does not explain (Athletic.net lists some: no `GradeID`, or a Grade group list that has them though their grade is another, or lacks them though it is theirs): they stay as recorded, like protected athletes, on exactly the lists they were on, with their grade.
- **A stand-in for each.**
  Each dropped athlete gets one invented athlete (an invented name, a reserved id), with the dropped one's school and grade, so the stand-in belongs to exactly the same lists: the State's, its Area's, and every Grade group with its grade.
  Its one mark per event lies strictly between two neighbouring displayed marks of its own lists (every Grade group of its State's and Area's lists: another State's list may hold the same mark, as no one compares them), a keyed few distinct marks (up to 14) above or below the dropped one's, never right beside it, on that mark's display step: never tying anyone on its lists, never at the top, and inside exactly the recorded lists the dropped one was inside (a list recorded only in part, or capped by a paywall's Masked tail, keeps the stand-in where the dropped one was, before the end), so every list keeps its length and every page its rows.
  Another list recorded only in part (another State's first pages, in a Recording of every State) says nothing against a mark when the dropped one's own mark is inside its pages without them: their school is not on it.
  An athlete with no such place in some event is not dropped (a Recording of every State's lists drops about 14% of its athletes; one State's, more).
- **Lists rebuilt.**
  Every list's rows are its kept rows in their recorded order with the stand-ins merged in by mark; ranks are worked out again (a recorded tie stays one, a stand-in ties no one), `rowNum` runs 1, 2, 3, and the rows are split back over the recorded pages by their `rowNum` ranges (Athletic.net's: page 1 is rows 1 to 101, page `p` rows `100(p-1)` to `100p+1`, the overlap the same people), with a Masked tail where it was: the same rows, last, on the same page (a masked row with people after it stops the run, as `ath` refuses such a page). A list with no one but Masked rows, or an empty page, stays as recorded.
  Equal marks share a rank, as `ath` counts places, but two equal marks Athletic.net ranked one after another (it often does: 11 and 12 for two 53.97s) keep their two ranks wherever they stand together again; equal marks keep the order of the widest list they are on.
  A list is its people, each once: pages read at different moments (cached apart, or the list changed between them) can disagree where they overlap (two tied people the other way round, someone new), and the list keeps everyone they show, the last page taking the one more or fewer.
  Lengths do not change, so neither does `minCount` (a list whose pages showed more people than its `minCount` keeps them).
- **Reads as long as recorded** (and refused when they cannot be).
  `ath rank` reads a list recorded only in part down past a subject's mark until 3 people are behind it, and the recorded pages were enough for that.
  Where the lists after would leave fewer behind on those pages (a grade swapped in with the subject's mark, or a row behind swapped out), the people tied with the subject and the 3 just behind, as recorded, are held where they are (as protected athletes are) and the lists are made again.
  When holding them is still not enough, the run stops, writing nothing: a replay of the recorded command would fail on the copy.
  Dropping and placing never makes or breaks a tie among kept athletes; a subject's rank moves by the stand-ins placed and the athletes dropped above them.
- A stand-in's row is the dropped one's row (its school, grade, meet and date, all anonymized) with the stand-in's name, id and mark.

### Shuffled grades

A row's Area, grade and a mark window would still single out one athlete on a public list, so grades are swapped too:

- **In pairs, among athletes who may go.**
  Kept athletes and stand-ins swap grades in pairs; protected athletes (subjects, entrants, `--keep`) keep theirs.
  Partners stand on exactly the same lists (so the same Area, event, gender and season), are a third of their group or more apart in mark (a keyed sample of such people, so a mark window does not pair them), and have the same grade gap in every season they share, so each moves by one amount everywhere (a 10th grader in 2098 and 11th grader in 2099 stays one grade apart from season to season).
- **Every list keeps its per-grade counts.**
  A pair swaps only when, on every list either stands on, the grades the two stand with are the same before and after; so each list's length and grade counts, and each Area's grade make-up, stay as recorded.
  An athlete whose grade changes leaves the Grade groups of their old grade and joins those of the new one, by mark, only where their mark falls inside the list's recorded pages (a list recorded in part, or capped, never gains or loses a row).
- **Rows rebuilt.**
  Every list's rows are everyone who stands on it in mark order, ranks (equal `SortInt` is one rank) and `rowNum`s worked out again and split back over the recorded pages; `GradeID` is the new grade.
  Most athletes who may go end up in another grade (in the made-up recording, about 85% of them).
- **What it leaves.**
  An Area and an approximate mark still narrow a row down to the Area's athletes near that mark: the residual risk, accepted in ADR 0010.

### Reviewed fields

Every key of every response must be one the anonymizer has reviewed (`ALLOWED_KEYS` in `anonymize.py`, by response kind); any other stops the run, naming the response kind and the path of each (`bio: athlete/<key>`: every key it does not know masked, as `--list-unreviewed` shows them), before anything is written.
A new field Athletic.net adds is reviewed (what it holds, whether it names anyone) and added to the list with its handling, then the run goes on.
`navinfo/` files (the division tree) are reviewed too: their ids and seasons move, their names stay.

| Kind | Reviewed keys |
| --- | --- |
| bio | `athlete`, `IDAthlete`, `SchoolID`, `FirstName`, `LastName`, `Gender`, `NoSearch`, `isClaimed`, `allTeams` (by school id: `IDSchool`, `SchoolName`, `TeamCode`, `Level`, `Year`, `City`, `State`, `PrefMetric`, `PrefConvert`), `grades` (by `<school>_<season>`), `allSeasons` (`ID`, `IDSeason`, `Display`, `Selected`), `eventsTF` (`Event`, `Type`, `IDEvent`, `IDEventType`, `ConversionInt`, `DefaultSortOrder`, `Description`, `FieldMeasureType`, `PersonalEvent`), `resultsTF` (`IDResult`, `AthleteID`, `Result`, `SortInt`, `SortIntRaw`, `EventID`, `EventTypeID`, `SeasonID`, `Exhibition`, `ResultDate`, `MeetID`, `meetName`, `SchoolID`, `PersonalBest`, `SeasonBest`, `Round`, `FAT`, `Wind`, `Blurred`, `Division`, `DivisionShort`, `ignoreDay`, `ignoreMonth`), `meets` (by meet id: `IDMeet`, `MeetName`, `EndDate`), `relayTeamMembers` (`Name`, a person; `SortID`; `TeamID`, an id), `canEdit`, `hasOtherSport`, `level`, `photos`, `distancesXC`, `resultsXC`, `unattachedCalendars`, `synonyms` |
| rankings | `eventShort`, `division` (`ID`, `ParentID`, `BaseDivID`, `SeasonID`, `Name`, `Description`, `EventSelectionID`, `Filter`, `LastMajor`, `Settings`, `HomeSettings`, `RankingsSettings`, `CustomRankingsSettings`, `TFRISettings`, `TFRIStartDate`, `TFRIEndDate`, `TFRIRefreshInterval`, and `BaseDiv`: `ID` and `ParentID` moved as list ids, `Name`, `State`, `StateName`, `Country`, `Type`, `SubType`, `Note`, `Level`, `RegionID`, `EditDepth` and flags), `minCount`, `settings` and `defaultSettings` (`fat`, `official`, `grades`, `page`, `depth`, `conversion`, and the filter settings: `convert`, `include`, `disabledFilters`, `hiddenFilters`, `followed`, `primaryMeasure`, `secondaryMeasure`, `trackType`, `wind`, `windLimits`, `visibility`...), `groupedRankings` rows (`rank`, `rowNum`, `AthleteID`, `AthleteName`, `GradeID`, `display`, `SortIntRaw`, `SortIntCalc`, `SortIntOrig`, `TeamID`, `TeamName`, `ResultDate`, `MeetID`, `MeetName`, `Event`, `EventShort`, `EventID`, `EventType`, `EventTypeID`, `IDResult`, `SeasonID`, `State`, `Measure`, `Round`, `FAT`, `Wind`, `PersonalBest`, `PersonalEvent`, `ConversionInt`, `blurred`, `isFieldSeries`, `ignoreDay`, `ignoreMonth`), `eventTypes` (`EventTypeID`, `Description`, `Results`), `relayTeams`, `jwtTFTopReport`, `includeDivisions`, `sportRankings`, `eventId`, `gender`, `eventType`, `reportTitle`, `level`, `hasSiteSupport`, `bestsRecordSet`, `customList`, `recordSet`, `debug` |
| teamnav | `team` (`ID`, `Name`, `Level`, `City`, `State`, `Country`, `ReplacementID`, `TeamRecords`, `hasIndoor`, `siteSupport`), `userAthleteIdOnTeam`, `hasCoachAccess`, `mascotToken2`, `grades` (`IDGrade`, `GradeDesc`), `divisions` (`id`, `b`, `name`, `gender`), `customLists` (by list id: `IDCustomList`, `PublicList`, `Type`), `customDivisions` (`IDDivision`), `linkedTeams` (`IDSchool`, `SchoolName`, `Level`) |
| meet | `meet` (`ID`, `Name`, `MeetDate`, `StartDate`, `EndDate`, `SeasonID`, `PublicEntries`, `RegEnabled`, `RelayAthletesRequired`, `Location`: `Indoor`, `Name`, `City`, `State`), `tfDivisions` (`IDMeetDiv`, `MeetID`, `IDDiv`, `Division`, `DivisionShort`, `LevelMask`), `jwtMeet` |
| events | `events` (`ID`, `Event`, `EventShort`, `Gender`, `Type`, `Entry`, `sort`, `isPersonal`), `eventDivsWithEntries` (`e`, `d`, `ge_p`), `eventDivsWithResults` |
| entries | `eventId`, `currentEventValid`, `entries` (`IDAthlete`, `FirstName`, `LastName`, `Gender`, `TeamID`, `EventID`, `Letter`, `SchoolID`, `FAT`, `Grade`, `TeamName`, `TeamCode`, `DivID`, `Comment`, `Locked`, `Scratched`, `OverrideSortInt`, `SortIntRaw`, `ConversionInt`, `SortInt`, `AcceptanceStatus`, `Source`, `OwnerUserId`, `OriginalEvent`), `teams` (`IDSchool`, `SchoolName`, `TeamCode`, `LevelName`, `DivName`, `ParentDivName`, `GrandparentDivName`, `Gender`), `events`, `eventDivsWithEntries`, `eventTypes`, `rounds`, `releyLegs` |
| search | `athQuery`, `response` (`numFound`, `docs`: `id_db`, `type`, `textsuggest`, `subtext`, `score`) |
| navinfo | `divListId`, `levelDivId`, `countryDivId`, `regionDivId`, `seasons`, `levels`, `countries`, `regions`, `tree` (each `id`, `name`, `state`, `divType`, `subDivType`, `depth`, `customDivision`), `events` (an event's public definition: `id`, `name`, `short`, `fmt`, `h`, `m`, `r`, `so`, `t`, `w`), `events_main`, `recordSets` |

Blanked by their whole path (strings to `""`, numbers to 0, everything inside too), reviewed as identifying when real responses were first anonymized (2026-11-09):

| Kind | Blanked | Why |
| --- | --- | --- |
| bio | `athlete/age`, `allSeasons[]/age` | an age with a grade is close to a birth date |
| bio | `athlete/Handle`, `athlete/UsatfId`, `athlete/PhotoUrl`, `athlete/rsMugshot`, `synonyms[]/RealFirstName` | a username, a national membership id, a photo, another name: each names the person |
| bio | `allTeams/<id>/MascotUrl` | a school's logo names the school |
| bio | `resultsTF[]/shortCode`, `resultsTF[]/MediaCount`, `resultsTF[]/Place` | a result's public page, its photo and video counts, a place in a race: each points at the real result |
| rankings | `groupedRankings[][]/Age`, `Country`, `Handle`, `PhotoUrl`, `TeamMascot`, `shortCode`, `mediaCount`, `MediaCountJson` | the same, on a row; a rare country singles someone out |
| rankings | `division/ImageUrl`, `division/CustomRankingsSettings/qualifying/settings/name`, `location` | a division's logo; a custom ranking's name, which carries a real year; a location |
| teamnav | `team/Mascot`, `team/MascotUrl`, `team/colors`, `team/coverPhoto` | each names the school |
| teamnav | `customLists/<id>/Name`, `customLists/<id>/Description`, `customDivisions[]/DivName` | free text a school wrote |

Kept, as public structure: the site's own vocabulary (filter and settings words, event codes, a division's kind and notes, which a real name may also be: `altitude`), flags, preferences and counts.

Anywhere, too:

- **Locations and contacts** are blanked, strings to `""` and numbers to 0, everything inside an object or list included: `Address`, `Address1` to `Address3`, `Street`, `Zip`, `ZipCode`, `PostalCode`, `Lat`, `Long`, `Lng`, `Latitude`, `Longitude`, `GoogleData`, `GooglePlace`, `PlaceID`, `Phone`, `PhoneNumber`, `Fax`, `Email`, `EmailAddress`, `Website`, `Twitter`, `Instagram`, `Facebook`, `TikTok`, `YouTube`, `Snapchat`, `Directions`, `MapUrl`.
- **Links** (`Url`, `URL`, `url`, `Link`, `href`): an Athletic.net page link keeps its shape with its ids and names replaced; any other link is blanked.
- **Venues** (`Venue`, `VenueName`) are invented like a meet's `Location.Name`.
- **Tokens** (any key starting `jwt` or holding `token`) are blanked.

### The self-check

Every run checks its output before writing anything, and fails, writing nothing, on any of:

- a structure change: a key renamed (but for an id key), a list's length, a value's type;
- a value the anonymizer changes left as recorded: every id, name, date and mark is checked at its own place;
- a real id of any kind (an athlete's, a school's, a meet's, a list's or division's) anywhere: in any number but the public ones (event ids, a meet's own division ids, grades, seasons, ranks, counts, flags and marks, listed by key in `PUBLIC_INT_KEYS`; one of those that equals a real id is only counted, and the summary says how many), in five or more digits inside a string, in an object key or a file name;
- a real whole name (an athlete's full name, a school, meet, venue or city of the Recording) in any string outside the public structure, or in a file name, matched by words so `jane-doe` counts;
- an invented last name that is a real one (an invented school, meet or venue is checked by its invented part, not its kept kind);
- an event map that changes the order of its distinct marks, meets two of them, crosses a State Meet standard, leaves a mark as recorded, or changes a mile-against-1600m decision;
- a list after reshuffling that is not consistent in itself (ranks not following the marks and the recorded ties, `rowNum`s not one after another, a complete list's `minCount` below its rows unless its pages showed more), whose kept athletes stand rank for rank as recorded, that lost a protected athlete or kept a dropped one, or an athlete shown differently on two lists of an event;
- grades after shuffling: a list whose per-grade counts changed, a protected athlete in another grade, an athlete on a list their grade does not belong to (but where recorded so: an athlete whose recorded lists their grade does not explain), a Grade group's list that is not exactly its grades' people of the whole list (as far as both lists' recorded pages reach, in mark order; such an athlete where recorded), or an athlete whose grade moves by different amounts on different lists.

The value-by-value checks run on the anonymized lists before they are reshuffled, the name and id scans on the final files too (an invented school, meet or venue there known by its key: its kind is not checked).

An unknown field holding a real id therefore stops the run, naming the field, rather than leak it; teach `anonymize.py` the field and run it again.

The check on marks is per mark: every mark in the output differs from the same mark in the Recording.
It cannot be global (no output mark equals any input mark): on a list of thousands, marks are a few hundredths apart, and any map lands most of them on some other athlete's real mark.
The better-direction baseline keeps the most searchable marks, the leaders', off every real mark in the Recording.

### State Meet standards

Marks move, but `areas/*.json` keeps the real State Meet standards for real use.
The anonymizer reads them (`state_meet`: CIF's at-large marks, the MPA's automatic and provisional standards, any season, Class and gender), and for each event in the Recording makes every standard a point of the event's map, as if it were a mark: so no mark crosses a standard (a mark faster than, equal to or slower than a standard stays so against the standard's own mapped value), and the event's display step fits the standards too.
An example page built from anonymized data must compare against the mapped standards (ADR 0010): `--map-out` lists them (`standards`), and `--areas-out DIR` writes a copy of the Area files with them in place and the seasons they are keyed by moved with the seasons (`2025` is `2098`), for the examples build.

## Tests

`tools/anonymize/test_anonymize.py` runs on a made-up Recording, `tools/anonymize/testdata/recording/` (written by `testdata/make_recording.py`): bios with outdoor, indoor, hand-timed, DNS, relay split, relay team and field results, a TeamNav chain, the season's tree and a State's subtree, rankings pages with ties and a Masked tail, a field event list, a meet with an open event and two relays (one a 4xMile with converted miles), and a search.
They check consistency across files and file names, seasons from 2099 and the dates with them, list ids gone, the shuffled grades (each list's per-grade counts, most grades changed, protected grades kept, Grade group lists exactly their grades' people of the whole list, one grade per athlete), the reshuffled lists (dropped athletes gone everywhere, stand-ins on every list they belong to, lengths, pages, ranks, `rowNum`s and protected athletes as they should be, no list rank for rank as recorded), ranks and ties re-ranked from the mapped marks, that the gaps are not kept, that no mark crosses a mapped standard, the shifted Area files, every runner's mile-against-1600m decision, displays against `SortInt`, dates, the structure byte for byte, determinism, the self-check on a planted leak, the edge cases (an unreviewed field, links and locations, metric and feet-inches field marks together, a mile standard, dotfiles in a Recording, the key file and every destination checked before writing, an output that would hold the Recording and `--replace` without the marker, a kept kind that is a real name, a city named like a pool first name, a whole-second mile, unique search names and a recorded query with no docs, a points event, the git-asked privacy guard), the shapes seen in real responses (`RealShapes`: `--list-unreviewed` naming paths with every unknown key masked, never values, every reviewed field kept, moved or blanked as listed, the latest season with data and a tree that runs ahead, `--latest-season`, team `0`, a list with no bio, rows whose grade their list does not explain, equal marks ranked apart, pages read at different moments, another State's list leaving room for stand-ins, a read cut short held and one still short refused, `--latest-season` checked, divisions below the Area blanked, a kept kind that is a real name after reshuffling), and, with `ATH_TEST_BIN` set (as `cargo test` does), with each side's clock pinned (`ATH_TODAY`), that `ath` prints the same on the anonymized copy as on the Recording, but for the anonymized values, for `profile`, `areas`, `meet` and `seed --meet` (open event, 4x800m, 4xMile), and that `list` and `rank` read lists consistent in themselves, as long as recorded, without the dropped athletes, the subject placed within the drops of their recorded rank.

`tests/record.rs` checks `--record-dir`: each of `profile`, `areas`, `list`, `rank`, `meet` and `seed --meet` (open event and relays) runs through the live data source over a fixture transport (`--fixture-transport`, hidden, which answers Athletic.net requests from a fixtures dir), records, and the replay with `--fixtures-dir` prints byte-identical JSON; a cached re-run records the same files; no token reaches the Recording; a record dir git does not ignore is refused; the fixture transport never touches the user's cache (it uses none unless given `--cache-dir`).

## The examples

The published examples (ADR 0010) live in `examples/` and are built from anonymized data only:

| Path | What it is |
| --- | --- |
| `examples/build_site.py` | Builds `examples/site/` (git-ignored; `--out DIR` for another folder): the landing page `index.html`, each example page (rankmap's own build of its config, under rankmap's own file names, in the config's `output_dir`), and `demo.html`, the page every example link goes to. No network, no `ath` binary, no Recording. |
| `examples/rankmap/<key>/rankmap.config.json` | A rankmap example's config, with `"example": true` and `"areas_dir": "areas"` ([`docs/rankmap.md`](rankmap.md#publishing)). |
| `examples/rankmap/<key>/data/<athlete_id>-<event>/2099.json` | Its saved season: the `ath rank` report of the anonymized copy, as rankmap saves it. |
| `examples/rankmap/<key>/areas/<ST>.json` | Its State Meet standards: only the anonymizer's mapped `at_large` or `standards` (`--areas-out`, mapped as this example's marks are) and a note that they are moved, merged over the repo's `areas/<ST>.json` at build time (`report.state_areas`), so the Areas, map and rules stay the repo's. Empty (but for `.gitkeep`) for a State without an Area file (Oregon). |
| `examples/test_examples.py` | Builds the site into a temporary folder (or checks the one `ATH_EXAMPLES_SITE` names) and fails on any Athletic.net link, any web link outside the project's pages and the fonts (`http`, `https` or `//host`), any other kind of link (`mailto:`, `javascript:`, `data:`...), a page without the examples' footer or with a `robots` tag, a State Meet without the note that its standards are moved, or a link inside the site that leads nowhere. `cargo test` runs it (`tests/tools.rs`), and so does CI. |
| `.github/workflows/pages.yml` | Builds the site once on every push to `main`, checks that build (`ATH_EXAMPLES_SITE`), and deploys it to GitHub Pages once Pages is on and the repository variable `PAGES_ENABLED` is `true`. |
| `docs/images/example-rankmap-<key>.png` | Each example's screenshot (the README's, and the landing page's cards), taken from the built page at 1280 px wide by `examples/screenshots.py`. |
| `examples/screenshots.py` | Builds the example pages into a temporary folder and shoots the screenshots of them into `docs/images/` with headless Chrome: each example's, and `rankmap-map.png` and `rankmap-table.png` of [`docs/rankmap.md`](rankmap.md) (the California example's US map and table, the content column and 30 px each side). The window is fixed (1280 by 1100 CSS px, device scale factor 1, light scheme), it waits for the web fonts (so it needs the network for Google Fonts), and each crop ends on the page's own elements, never mid-element. `python3 examples/screenshots.py [NAME ...]` takes some of them (`rankmap-map`...); `CHROME` names the browser, else it looks for Chrome or Chromium where macOS and Linux put them; oxipng or optipng, when on `PATH`, shrinks each PNG. `examples/test_screenshots.py` checks that its pages build and, where Chrome is found, shoots every screenshot into a temporary folder at its size. |

The three rankmap examples are Recordings of the 2026-11-09 season, one per scenario (`example-ca-rankmap`: California girls 400m, an 11th grader, `ath rank <id> --event 400m --in all,areas --ahead all --grade 11` and the Profile; `example-me-rankmap`: Maine, the same; `example-or-rankmap`: Oregon boys 800m, `--in all`, Oregon having no Area file), each subject picked by a fixed rule from the recorded State list, never by hand.
Each is anonymized on its own, with the same key and `--latest-season 2026`:

```bash
python3 tools/anonymize/anonymize.py local/recordings/example-ca-rankmap /tmp/ex/ca --key-file local/anonymize.key \
  --latest-season 2026 --map-out local/recordings/example-ca-rankmap.map.json --areas-out /tmp/ex/ca-areas
```

then replayed with the clock pinned to the recording day moved (`ATH_TODAY`, the summary's date shift: `2099-11-12` for California and Maine, `2099-11-06` for Oregon) and the anonymized subject's id (the copy's one bio, `<id>.json`):

```bash
ATH_TODAY=2099-11-12 ath rank <id> --event 400m --season 2099 --in all,areas --ahead all --grade 11 --json \
  --fixtures-dir /tmp/ex/ca > /tmp/ex/ca.json
```

The report is saved as rankmap saves a season (`build.season_file(report, ATH_TODAY)`, one line of JSON) into `data/`, and the `state_meet` standards of the State's mapped Area file from the same run into `areas/<ST>.json` (each Recording's event maps are its own, so an example takes the standards of its own run).
Then `python3 tools/anonymize/real_values.py refresh local/recordings/example-*.map.json` adds the Recordings' real values to the guard, and `python3 examples/build_site.py` rebuilds the site.
After a change to the examples or to how rankmap draws its page, `python3 examples/screenshots.py` retakes the screenshots; look at each before committing it.

## The fixtures and the real-value guard

The test fixtures that come from Athletic.net (`tests/fixtures/`: the anonymized season 2099 world, beside the made-up one in 2026) and rankmap's saved report and pages are one anonymizer run over one combined Recording: what the tests' commands read from the Recordings of 2026-11-09 (scenarios `rank-ca-tx-girls-400m`, `rank-ca-ccs-girls-400m`, `profile-ccs-girls-400m`, `list-ccs-girls-400m-9-10`, `list-ccs-sp`, `list-ccs-2025` and `areas-2026`), re-recorded through the fixture transport into one folder (`ath ... --fixture-transport local/recordings/<scenario> --record-dir local/recordings/_fixtures`), then:

```bash
python3 tools/anonymize/anonymize.py local/recordings/_fixtures /tmp/fixtures --key-file local/anonymize.key \
  --latest-season 2026 --map-out local/recordings/fixtures.map.json --areas-out /tmp/fixtures-areas
```

Its output goes into `tests/fixtures/` as it is, its mapped Area files' State Meet standards (`state_meet`'s standards, notes and sources) into `tools/rankmap/testdata/areas/` (rankmap's tests build the anonymized report with them as `areas_dir`), and the saved report and pages are blessed from it (`ATH_BLESS=1 cargo test`, `RANKMAP_BLESS=1`).
Tests pin the clock to `ATH_TODAY=2099-10-30` (2026-10-27 moved).
Their expected places are counted from the fixture pages, not copied from `ath`'s output.

`tools/anonymize/real_values.py` keeps real values out of the repo: `real-values.txt` lists every real id of six or more digits and every real person's whole name (first-last and last-first; and school, meet and venue names of three or more words, but for names the real public sources have: the Area files and the national, level, State and Area names of the recorded trees in `local/recordings/`, never the fixtures it checks) of the Recordings, each as the first 16 hex digits of an HMAC-SHA256 under a secret guard key.
`test_real_values.py` scans every tracked text file for them (`python3 tools/anonymize/real_values.py scan` by hand), naming each match's place, never its value.
Names are compared folded (accents and apostrophes dropped, any case and punctuation: `Zoë O'Doe`, `zoe-odoe`, `O'Doe, Zoë`), and in a JSON file a `FirstName` and `LastName` together count as the name.
Ids count standing alone (not inside a word such as `#17793f`), plain or in groups of three (`1_234_567`), and not as a JSON value of a key that holds no id (a mark's `SortInt`, a count).
A match that is no real value there (the default `--from-list`, a mark equal to an id, the public meet the project began with) is allowed by path, with its reason, in an `allow` line.

**The guard key** is `HMAC(anonymize.key, "real-values-guard")`: without it no one can test a guess against the list, so the list says nothing.
The guard reads it from `ATH_GUARD_KEY` (hex) or, without that, derives it from `anonymize.key` in your local/ folder (`ATH_LOCAL`, else the main checkout's `local/`); with neither, the scan test skips, saying so (a fork, or CI before the secret is set).
The list's `check` line is the key's hash of a fixed label: a key that does not give it (a wrong secret or key file) fails the scan test, never skips it, and `refresh` refuses to add under it.
CI passes the repository secret `ATH_GUARD_KEY` to both test jobs; the maintainer sets it once, from the main checkout, the key going through the pipe only (`key` refuses to print to a terminal):

```bash
python3 tools/anonymize/real_values.py key | gh secret set ATH_GUARD_KEY
```

After anonymizing a new Recording, add its values (refresh keeps what the list has and its `allow` lines):

```bash
python3 tools/anonymize/real_values.py refresh local/recordings/<scenario>.map.json
```

`--rebuild` makes the list from only the files given, so give it every map and extra file:

```bash
python3 tools/anonymize/real_values.py refresh --rebuild local/recordings/*.map.json \
  --extra local/recordings/old-fixtures.values.json
```

`--extra` adds real values from elsewhere (`{"ids": [...], "people": [...], "names": [...]}`): `old-fixtures.values.json` holds those of the fixtures the anonymized ones replaced.
