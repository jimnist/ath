# Athletic.net

The only external system. **`docs/HANDOFF.md` is the source of truth** - this
file is a fast index into it, not a replacement. Read the handoff doc before
changing anything that touches the network or the mark-selection logic.

## Surfaces

- **Entries page** (`/TrackAndField/meet/{meetId}/entries/{gender}/{divId}/{eventShort}`) -
  an Angular SPA backed by the Meet API ([below](#meets-mode-a)), which `ath`
  reads instead of the page.
- **Bio API** - `GET /api/v1/AthleteBio/GetAthleteBioData?athleteId={id}&sport=tf&level=0`
  returns JSON with `athlete`, `eventsTF` (event definitions), and `resultsTF`
  (results). This is where marks come from. `athlete` also has `FirstName`,
  `LastName`, `Gender` (`F`/`M`).
  The top-level `grades` maps `"<schoolId>_<season>"` to grade and records exactly which teams the athlete was on each season, clubs included (club values can be odd, e.g. 402) and indoor seasons too (`12026`).
  Checked live 2026-10-26: one athlete has `<high school>_2026: 9`, `<middle school>_2025: 8` and `<middle school>_2024: 7`, plus club seasons; another has `<high school>_2026: 9`, `<middle school>_2025: 8` and `<club>_2026: 9` (the anonymized `tests/fixtures/982389051.json` has the same shape).
  `ath` splits a key on its last `_`, ignores indoor seasons (>= 10000), and decides a Profile's school for a season by it (see A Profile's Area below).
- **Rankings API** (used by `ath list` and `ath rank`, and through `ath rank` by rankmap, `docs/rankmap.md`) -
  `POST /api/v1/tfRankings/GetRankings` with `{reportType: "list", mode, divListId,
  eventShort, gender, qParams: {grades: [9, 10], page}, version: 2, ...}` returns
  `groupedRankings` (a page of rows; see Rankings lists below), `division`
  (`SeasonID`, `ParentID`, `Name`), `minCount`, `settings` and a `jwtTFTopReport` token.
  `GET /api/v1/tfRankings/GetNavInfo` with that token in an `anettokens` header
  returns the division tree for the token's division: `seasons` (season →
  division id for that place) and `regions` (the high school level plus each
  state's division with its two-letter `state`). Division ids change every
  season - discover them, never hardcode them.
  Rankings rows have **no `SortInt`**; they carry `SortIntRaw`, `SortIntCalc`
  and `SortIntOrig`, in the same encoding as the bio API's `SortInt`.
  `SortIntCalc` is the key to rank on: it is the list's comparable value.
  Checked 2026-10-26 on California 2026 lists (400m FAT and hand-timed with
  `qParams.fat: false`, shot put, long jump, high jump, and `conversion` 1/2):
  all three were equal on every row.
  The default filters are FAT-only (`settings.fat: true`) and official marks.
  An `eventShort` Athletic.net does not know (`SP`, `LongJump`) silently returns
  the **100m** list; check the response's `eventShort` matches the request.
  Known codes include `100m`, `400m`, `Shot`, `LJ`, `HJ`.
- **Navigation API** (`ath areas`, a Profile's Area; checked 2026-10-26) -
  `GET /api/v1/tfRankings/GetNavInfo?seasonId=<season>&level=4&gender=<f|m>&recordSetId=0&locationId=0&teamId=0&indoor=false` with header `anettokens: <jwtTFTopReport>` from any rankings response.
  Keys: `divListId, levels, levelDivId, seasons, countries, countryDivId, regions, regionDivId, tree, events, events_main, recordSets`.
  `seasons` maps season to the list id of the token's place in that season, which is how one list leads to the same place in another season.
  `regions[]` items are `{id, name, state, divType, subDivType, depth, customDivision}`; names may carry leading spaces (trim them).
  The 2026 high school tree had `divType` counts Level 1, State 52, Section 10, Association 7, League 6.
  `tree[]` has the same items: the token's own division at depth 0 and the levels below it.
  Some States' Areas are only there: with a Maine list's token (its 2026 State list, checked live 2026-11-02 for 2025 to 2027), `regions` is the national tree with Maine and nothing below it, and `tree` is Maine plus its Classes `A`, `B`, `C` (`divType: "Class"`, depth 1, each its own list).
  `ath` reads a State's `tree` only when the national `regions` lack the Areas its Area file names (`docs/areas.md`).
  The token changes every call but the answer does not, so the cache key leaves it out (`Request` headers are never part of the key, and their values are never printed).
- **TeamNav API** (a school's division chain) -
  `GET /api/v1/TeamNav/Team?team=<schoolId>&sport=tf&season=<year>`, no token needed.
  Keys: `team, userAthleteIdOnTeam, hasCoachAccess, mascotToken2, grades, divisions, customLists, customDivisions, linkedTeams`.
  `team` has `ID, Name, Level (4 = high school), City, State, ...`.
  `divisions` is the school's chain for that season, top down: United States, High School, California, Central Coast, then the school's conference and league for a Central Coast school (`tests/fixtures/teamnav/920353302-2099.json`, anonymized, has the chain with those two names blanked).
  Each link is `{id, b, name}`: `id` is the season's list id and `b` the base division id, which stays the same across seasons (Central Coast is another list every season, with the same `b`).
  Names have leading spaces (`"   United States"`, and a league's name); trim them.

### Meets (Mode A)

Confirmed live 2026-11-06 on an open indoor meet (a paid session through the relay); `ath meet` and `ath seed --meet` read these three calls (`src/meet.rs`).

- **`GET Meet/GetMeetData?meetId=<id>&sport=tf`** needs no token.
  It answers `{meet, tfDivisions, jwtMeet, ...}`.
  `meet` has `ID`, `Name`, `MeetDate` (`2026-12-29T00:00:00`), `StartDate`, `EndDate`, `SeasonID` (`12027` is 2027 indoor, `2027` outdoor), `PublicEntries` (when entries go public, or null), `RegEnabled`, `RelayAthletesRequired` (4) and `Location` (`Indoor`, `State`, `City`, `Name`).
  `tfDivisions[]` items are `{IDMeetDiv, MeetID, IDDiv, Division ("Varsity"), DivisionShort ("V"), LevelMask}`; `IDDiv` is the `divId` the entries page and `GetEntriesData` take, and it is meet-specific.
  `jwtMeet` is a token the other two calls need.
  `ath` seeds as of the day before the meet's first day at the latest (the earlier of `StartDate` and `MeetDate`), and refuses a meet once its last day (the latest of `EndDate`, `MeetDate` and `StartDate`) is past.
- **`GET Meet/GetEventListData?meetId=<id>`** with header `anettokens: <jwtMeet>` (403 without).
  It answers `{events, eventDivsWithEntries, eventDivsWithResults, ...}`.
  `events[]` items are `{ID, Event, EventShort, Gender ("M"/"F"), Type ("T"/"F"), Entry, sort, isPersonal, ...}`; `ID` is the same gender-specific event id Profiles use (800m is 4 for boys, 22 for girls; 55m 41/46; 1 mile 58/59), so `ath` finds events by `EventShort` or `Event`, never by id.
  Relay codes are not reliable: `4x800m` says relay, but on that meet the `SMR 1600m` event's code is `sprintmed2248` and `4x55 Shuttle Hurdles`' is `55shuttleh`, so `ath` tells a relay by code or name (`<n>x`, relay, medley, shuttle, SMR, DMR, SHR) and by entries with a `Letter`.
  Indoor-only events (55m, 60m and their hurdles, 300m, 500m, 600m, 1000m, shuttle hurdles) are told by code or name too; `ath seed --meet` refuses them, since seeding reads outdoor marks only.
  `eventDivsWithEntries[]` items are `{e: <event ID>, d: <IDDiv>, ge_p}`: the event and division pairs that have entries, which is what `ath meet` lists.
- **`POST Meet/GetEntriesData`** with headers `anettokens: <jwtMeet>` and `Content-Type: application/json`, body `{"meetId": 900700, "sport": "tf", "gender": "m", "divId": 13, "eventShort": "4x800m", "seasonId": 0}` (the entries page's own path parts).
  It answers `{eventId, currentEventValid, entries, teams, events, eventDivsWithEntries, eventTypes, rounds, releyLegs}` (`releyLegs` spelled so, and empty on a relay).
  `entries[]` items carry `IDAthlete`, `FirstName`, `LastName`, `Gender`, `TeamID`, `EventID`, `Letter`, `SchoolID`, `FAT`, `Grade` (a string, `"11"`), `TeamName`, `TeamCode`, `DivID`, `Comment`, `Locked`, `Scratched`, `OverrideSortInt`, `SortIntRaw`, `ConversionInt`, `SortInt` (the entered seed mark), `AcceptanceStatus`, `Source`, `OwnerUserId` and `OriginalEvent`.
  `teams[]` items are `{IDSchool, SchoolName, TeamCode, LevelName, DivName, ParentDivName, GrandparentDivName, Gender, ...}`.
- **Entrants.** `AcceptanceStatus` 10 is accepted, the only value seen; `ath` counts an entry as an Entrant only when it is 10 and `Scratched` is false, and lists every other entry as left out, with why.
- **Relays** are one entry per leg, each with the team's `TeamID` and its entered seed in `SortInt`, and a `Letter`: `A1`..`A4` are team A's legs, and a school's second team is `B1`..`B4`.
  `ath` groups them by `TeamID` and the letter into teams with legs in leg order (`meet::relay_teams`).
  `ath seed --meet` seeds a relay whose four legs are one flat race on the first four legs' runners (`seed_relay`); it finds the leg distance in the event's code or name (`4x800m`, `4x800 Relay`, `4xMile`), never its id, and refuses medley and shuttle relays.
- **Token.** `jwtMeet` is a credential, never data: `ath` keeps it for the run only, and caches `GetMeetData` with it removed.
  A 401/403, or an authorization denial (`{"Message": "Authorization has been denied ..."}`) in place of the data, drops the held token and retries once with a fresh one; a second refusal, or a fresh `GetMeetData` with no token, fails with that refusal and says a paid subscription may be needed.
  Any other non-2xx answer, another error object (reported with its `Message`), or an answer without its data (`meet`, `events`, `entries`) is an error; none of these is cached.
  A `GetMeetData` this run read without a token is not sent again for one.
  A run whose meet came from the cache fetches `GetMeetData` again only when it needs the token for an event list or entries that are not cached.
- **Cache.** All three are cached as `DataKind::MeetEntries`, fresh for about an hour (ADR 0006), keyed by request as usual (the `anettokens` header is never part of the key).
- **Unknown.** What a session without a paid subscription sees, and what other `AcceptanceStatus` values mean.

### Rankings lists (`ath list`)

Checked live 2026-10-27 through the relay on the 2026 Central Coast girls' 400m list for grades 9-10 (584 people, 6 pages) and California's girls' 400m list, every grade (11,389 people, 114 pages).

- **Paging.** Athletic.net's own page sends `qParams: {grades: [9, 10]}` for page 1 and adds `page: <n>` from page 2; `qParams` is `{}` with no filter.
  Page 1 holds rows 1 to 101 and page `p` rows `100(p-1)` to `100p+1` (by `rowNum`), so neighboring pages share **two** rows (not one, as rankmap's notes said).
  A full page is 101 or 102 rows; the last is shorter, and pages past the end are empty.
  `ath` dedupes by `AthleteID` and stops at a page with fewer than 100 rows or nobody new.
- **`minCount` is not the list's length** until the end: it is a lower bound that grows as the pages go deeper (California: 696 on page 1, 5741 on page 6, 11389 from page 15 on).
  It equals the length on every page of a small list, and on the last page of any list, which `ath` checks against what it read.
  Stopping when the rows read reach page 1's `minCount` would cut California's list at about 700 rows.
- **Rows** carry `rank` (tied marks share it: two at 159 on the Central Coast list), `rowNum`, `AthleteID`, `AthleteName`, `GradeID`, `display`, `SortIntRaw` / `SortIntCalc` / `SortIntOrig` (equal on every row checked), `TeamID`, `TeamName`, `ResultDate`, `MeetID`, `MeetName`, `Event` (`400 Meters`), `EventShort`, `SeasonID`, `State` and more.
  Rows come sorted by `SortIntCalc`.
- **Masked rows (paywall).** Full lists need a paid subscription, enforced on the server (ADR 0001): signed out a list shows 5 rows, signed in without a subscription 25, then placeholder rows.
  Seen live 2026-11-06 through the relay in an unpaid session (California 2026 girls 400m, every grade, `minCount` 696): 25 real rows, then one row with `AthleteID: 0`, `AthleteName: "Xxxxxx Xxxxxx"`, `display: ""`, `TeamID: 0`, `TeamName` all `X`s, `MeetID: 0`, `MeetName` all `X`s (`"XXX Xxxxxxx Xxxxxxxxxxxxx Xxxxxx"`), `GradeID: 99`, but a real-looking `SortIntCalc` (55400) and `ResultDate`.
  The page is short, so before Masked rows were handled `ath` took it for the end of the list, ranked the placeholder 26th and only noted the length mismatch.
  `ath` treats a row as Masked (`RankingsRow::is_masked`) when its `AthleteID` is 0 or an explicit null (no Profile has id 0), or when its name is only `X`s and spaces and its `display` is empty (in case the id ever comes back filled in).
  A row with no `AthleteID` key at all is a parse error, not a Masked row, so a renamed field on a paid list is not misreported as the paywall.
  Masked rows are taken out when a page is parsed, which also records how many there were, whether a person's row came after one, and the page's last `rowNum`, so the rule below is worked out once.
  Masked rows are only ever a tail: one with people after it is refused ("unexpected masked row in the middle of ..."), since dropping it would shift places.
  A page is capped (`RankingsPage::is_capped_after`, given the people read from earlier pages) when it had a Masked row; when it is empty and states a `minCount` above the people read (a signed-out page 1, or an unpaid live page 2 after a page 1 a paid run cached); or when it is short and its rows end exactly at a known cap (`SIGNED_OUT_ROWS` 5, `UNPAID_ROWS` 25) below its `minCount` (a session may get its rows without placeholders).
  Any other short page below its `minCount` is not capped, since a paid list's count can lag its rows by a row or two: `read_pages` only notes the mismatch.
  The rows shown before a cap are still the list's first rows, so `read_pages` answers from them when its `enough` check is met there (`ath list --limit 10`; `--limit 25` on a 25-row cap prints them with a note that row 25's tie is unknown; an Athlete placed with their Neighborhood inside the rows shown).
  When the answer needs rows past the cap, `read_pages` fails with `list::Capped` ("Athletic.net returned only 25 of at least 696 rows for California 2026 girls 400m (all grades); full lists need a paid Athletic.net subscription, ..."), counting rows as Athletic.net numbers them (the last `rowNum` shown), and `ath rank` stops the whole run rather than fail one list (a partial rank map is misleading to share).
  Whether signed-out pages mask the same way, and whether the bio API (`GetAthleteBioData`) masks anything for an unpaid session (the Profile page blurs an athlete's rankings), is unchecked.
- **Checks on every page.** An unknown `eventShort` answers with `eventShort: "100m"` (checked again with `SP`), so `ath` refuses an answer whose `eventShort` differs from the request.
  A list id from another season answers with that season's list; `ath` refuses an answer whose `division.SeasonID` is not `--season`.
  A list id Athletic.net does not have answers with `division: null` and no rows; `ath` refuses it, and an answer whose `division.ID` is not the list asked for.
  `settings.grades` echoes the Grade group applied (absent for every grade); `ath` refuses an answer whose grades differ from the request.
  `ath` keeps the previous page's marks by `rowNum` and notes when the next page's overlap disagrees (pages from different moments: cached hours apart in season, or the list changed while it was read).
  It compares marks, not people, since tied people could trade places between requests.
  A page past the end of a list may not say its list, so only page 1 must have a `division`.
- **Relays.** Relay codes are `4x100m`, `4x400m` (`4x400` and `4x400R` get the 100m list).
  A relay list's rows are teams: `AthleteID` is a relay team id, `AthleteName` joins the runners' names with `<BR>`, and `relayTeams` maps each result to its `Members`.
  Relays are out of v1 scope, so `ath` refuses a page whose `relayTeams` is not empty.
- **Filters.** `settings` says what was applied: `fat: true`, `official: true` and `grades`, with `depth: 100`.
  `ath` keeps the defaults and reports `fat` and `official`.
- **The web page** for the same view is `/TrackAndField/rankings/list/<list_id>/<f|m>/<eventShort>?grades=9,10` (the Grade filter's buttons write `?grades=`).
- **Pacing.** Pages are read one at a time, `--delay-ms` (250 ms) apart; 115 requests in a row drew no throttling.
  Pacing lives in `AthleticNet` (`with_page_delay`), around every rankings request actually sent (list pages and the token requests for trees): a page from the cache never waits.
  The first rankings request that draws a throttled or challenged answer slows the rest of the run to one page a second, doubling on each further one up to 8 seconds.
- **Cache.** Each page is cached as `DataKind::Rankings` by its request: forever once the season is over, 12 hours while it is in progress.
  A page that may be capped (judged with no earlier pages, so an empty page stating a `minCount` too) is never written, and one already in the cache is a miss and is deleted, so an unpaid run cannot stand in for a later paid one (the `Cacheable` trait in `cache.rs`).
  The delete is compare-and-delete (`ResponseCache::remove_if`): the entry is moved aside atomically and deleted only if it is still the one read, so a good answer another run wrote meanwhile stays.

### Areas

An Area is the level directly below a State (`CONTEXT.md`).
Which `divType` counts as the Area level is configured per State in `areas/<ST>.json` (`docs/areas.md`, embedded by `build.rs`): California is `Section` (the 10 CIF sections, `state: "CA"`, depth 2).
Other States are out of scope until configured: New York's level below the State is `Association`, Texas' is `League`.
California's 2026 State list is 168546, `--from-list`'s default (below); the High School level and the ten sections have their own list ids, a new set every season, which `ath areas CA --season <year>` prints.
`ath` never hardcodes them: it discovers them every season.

**A season's tree through a list in that season** (`AthleticNet::season_area_tree`): rankings for the list (token; its `division.SeasonID` must be the season when the response has one) → `GetNavInfo?seasonId=<season>` with the token → `regions`.
Two requests, none once the tree is cached (forever).
Only the national tree is cached: one with the `High School` `Level` region and `State` regions for at least 50 of the 50 States and DC (`NavInfo::is_national`).
A token for some other list may answer with a smaller tree; that tree serves the one answer but is never cached, and a State it lacks is reported as possibly only missing from that partial tree.
The profile path uses this with the State link of the school's own TeamNav chain (the link right below `High School`), so it needs no anchor and no hop, and checks that the tree's State list for the school's State is that link.

**A season's tree from any list** (`AthleticNet::area_tree`, for `ath areas`, which has no school): rankings for the start list (`--from-list`, default California 2026, 168546, used only for its token) → `GetNavInfo` for the start list's own season (`seasonId` = its `division.SeasonID`, never the target) → `seasons[<season>]` → the two requests above.
Four requests, two when the start list is already in the season, none once cached.
The hop's `GetNavInfo` is never read from the cache, since a cached tree's `seasons` would never learn of a new season; it is written as that season's tree when it is national.
When the rankings response has no `division.SeasonID`, the hop instead reads `seasons` from `GetNavInfo` for the season asked for, with the start list's token (the hop before rankings were read for their season), and remembers nothing about a missing season.
A start list that cannot be read fails with a message naming `--from-list`.
A season missing from `seasons` is "no such season yet"; a marker is cached for 6 hours (`DataKind::MissingSeason`).
The marker's only evidence is the start list's own `seasons` map, so it is keyed by the season and the start list (the tree request plus `&athFromList=<list>`, never sent), records the start list's season, and is honored only for that list and only when that season is not the one asked for.
It never stands in for a tree: `season_area_tree` (a school's chain) never reads it.
A tree with no `State` regions is an error and is never cached.
Trees are always fetched with `gender=f`: list ids are the same for both genders, so one cache entry serves both, and gender only changes the links.
The rankings requests are never cached, since the token expires.

**A Profile's Area** (`area::profile_area`): take the Profile's school for the season (`Bio::school_in`, the same rule as the Profile's School row: the school, `Level` 2 or 4 from `allTeams` and never a club, with a `grades` key `<id>_<season>`, preferring a high school, then the Profile's own `SchoolID`; only without one, the school whose `allTeams` `Year` is the earliest at or after the season, since `Year` is the last season on that team and a missing `Year` counts as current; a season before every school's `Year` gives the earliest school as a guess, with a note; when no `Year` reaches the season, the current school, with a note), require a high school (`Level` 4; never a club), read its TeamNav chain for the season, look up its `State`'s Area level, and pick the chain link whose `id` is one of that season's Area list ids for the State.
An empty chain is `not_listed`.
Any error talking to Athletic.net is `lookup_failed` with the error as the note; the Profile still prints.
Athletic.net's lists decide membership; `ath` never infers it (a school in San Francisco can be in the Central Coast Section through its league, not the San Francisco Section).
Area links open on the Profile's `--event` when it is a flat race (`<meters>m`), otherwise on the 100m.
Checked live 2026-10-26 through the relay: 2026 California and its 10 sections; the 2025 hop gives 2025's California and Central Coast lists; 2028 is "no such season yet"; two Profiles of a San Francisco school are in the Central Coast through it.
Matching on `b` against the Area regions' base ids would also work across seasons, but `GetNavInfo` regions carry no base id, so `ath` matches the season's list ids directly.

## Non-negotiable encodings (the hard-won part - full detail in the handoff doc)

- **Event IDs are gender-specific.** Never hardcode them. Resolve the target
  event by its `Event` name string (e.g. `"800 Meters"`), collect matching
  `IDEvent`s, then filter results by those IDs.
- **`EventTypeID`** - `0` is a standard individual mark (keep); `98` is a relay
  split (exclude - splits are faster than open marks and corrupt PRs). For shot
  put it encodes the implement weight.
- **`SortInt` is the numeric key** - never parse the display string. Running:
  milliseconds (lower = faster). Field: `20_000_000 − inches×1000` (lower =
  farther). Best mark = min `SortInt`. Sum `SortInt`s for relay aggregates.
- **Filter for a Qualifying mark:** right event IDs, an outdoor season
  (`SeasonID < 10000`), `EventTypeID == 0` (or the specific implement for shot),
  `Exhibition == false`, not hand-timed (`Result` not ending in `h`), and a real
  time/mark (excludes `DNS`/`DNF`/`NT`/`SCR`/`DQ`/`FS`).
  No single-season filter: `ath seed` seeds on the Personal record across all outdoor seasons (it was `SeasonID == 2026` before).
- **`SeasonID` encodes indoor vs outdoor.**
  Outdoor seasons are the plain year (`2026`); indoor seasons are `10000 + year` (`12026`).
  The bio's `allSeasons[]` names them in `Display`: `"2026 Outdoor"`, `"2026 Indoor"`.
  Indoor results use the same `EventID` as outdoor (`"400 Meters"` is one event), so only `SeasonID` tells them apart.
  A December meet belongs to the next year's indoor season (a 2021-12-11 result is in `12022`).
  Checked live 2026-10-26 on a Profile with indoor results.
  v1 counts outdoor marks only, so `ath` drops every result with `SeasonID >= 10000` before picking any best.
- **`PersonalBest` / `SeasonBest` only mark today's bests.**
  Each result in `resultsTF` carries them as integer bitmasks (0, 3, 6, 7, 8, 14, 15, 16, 30 seen), set on the results that are the best *now* in some scope, not on results that were a best when set.
  Checked 2026-10-26 on a live Profile: of a 400m history of seven marks, several of them a best when they were set, only today's best is flagged.
  So Season best and Personal record as of a date are computed from results: the lowest `SortInt` among qualifying marks with `ResultDate` on or before the date (Season best within that `SeasonID`, Personal record across seasons).
  `ath` does not parse the flags.
  A result with no `ResultDate` counts as of today (it can only be on or before today) but not as of an earlier date.
  On a tie in `SortInt` the earliest dated result is the best; an undated result loses a tie to a dated one.
  Results also carry `ResultDate` (`2026-05-16T00:00:00`), `MeetID`, `meetName` and `SchoolID` (the team the mark was set under), and `SortInt` keeps the thousandths the display rounds away (an illustrative example: `12.93a` is 12925).
  On real bios each result's `meetName` is null; meet names come from the bio's top-level `meets` map, keyed by MeetID (`{IDMeet, MeetName, EndDate}`).

## Operational notes

- **Rate limiting:** after ~40 quick requests the bio API returns HTML instead
  of JSON. Detect (response doesn't start with `{`), and use bounded
  concurrency (~5–6 in flight), inter-batch delay, and retry-with-backoff.
- **Auth:** Cloudflare-gated. A plain HTTP client gets a challenge (HTTP 403),
  and headless browsers get Cloudflare's block page (reconfirmed 2026-10-25 for
  the bio API; ADR 0001).
  So `ath` has two transports behind the `Fetcher` seam, chosen per run (ADR 0006).
  The **browser relay** ([below](#browser-relay)) is the default.
  The **cookie** transport (`src/http.rs`, `ReqwestFetcher`) sends the browser's `Cookie` header and matching User-Agent, and is picked whenever a cookie is given (`--cookie-file` beats `--cookie` beats `ATH_COOKIE`).
- **Caching:** `src/cache.rs` stores raw responses keyed by request (method, URL and POST body), one `<sha256>.json` file per request that also records the request, its data kind and when it was fetched.
  Freshness comes from the data kind the caller passes (ADR 0006): Area tree and finished-season rankings forever, in-season rankings and profiles 12 hours, meet entries 1 hour.
  A school's division chain (TeamNav) is forever once its season is finished and 12 hours while it is in progress; a season is finished after July 31 of its year (`cache::season_finished`, rankmap's `season_end` default).
  "No such season yet" is remembered for 6 hours.
  Extra request headers (such as `anettokens`) are not part of the key.
  An entry is fresh under the stricter of the kind it was stored as and the kind asked for, so a list cached mid-season still expires after the season ends.
  `--refresh` refetches each request once per run (a repeat within the run is served from what the run just wrote).
  The cache never fails a request: an unreadable or unparseable entry is a miss, and a failed write is only a warning.
  The "profile forever after the person's last season" rule is not implemented yet (it needs the bio parsed at cache time), so every profile is fresh for 12 hours.
  Search is not cached.
  The default dir is `ath/responses` under the OS cache dir; the old `ath/bios/<athlete_id>.json` layout is simply ignored.
- **Recording:** `--record-dir <DIR>` also writes each response a run answers with (from the cache too) to DIR in the `--fixtures-dir` layout (`AthleticNet::with_recorder`, `src/record.rs`): rankings pages with `jwtTFTopReport` emptied, meets as cached (no `jwtMeet`), searches with their query (`athQuery`), and only into a path no git work tree tracks (`git check-ignore`; `docs/anonymize.md`).
- **POST:** the `Fetcher` seam sends a `Request` (GET, or POST with a JSON body); both transports send both, and challenge retry and stop-and-ask apply to both.
- **Challenges:** retry with backoff and stop-and-ask (ADR 0004) live in `src/athletic.rs`, above the `Fetcher`, so they behave identically on both transports.
- **Throttling:** a 2xx answer with an empty or non-JSON body (seen live as an empty 200 during a burst through the relay) is retried with the same backoff; if it persists, `ath` stops with throttling advice (wait, lower `--concurrency`, raise `--delay-ms`), not cookie advice. Error statuses (404/500) are never retried, and the relay reports a tab result without a body as a relay error rather than an empty answer.
  Only the final advice differs: each `Fetcher` supplies its own `challenge_advice` (refresh the cookie, or pass the check in the relay tab).

## Browser relay

`src/relay.rs` (`RelayFetcher`) and the tab script `src/relay.js` (embedded with `include_str!`, printed by `ath relay install`).
`ath` serves a small HTTP/1.1 API on `127.0.0.1:<--relay-port>` (default 8767) for the life of the run, and an Athletic.net tab runs a loop against it.

### Protocol

1. The tab sends `GET /next?tab=<tab id>&loop=<n>`, a long poll held for about 10 seconds.
   The tab id is a random UUID made when the script starts in a page; `n` numbers the tab's loops from 0.
   Without both, `ath` answers `400`.
   `ath` answers `200` with one job, `{"id": 7, "method": "GET" | "POST", "url": "https://www.athletic.net/api/v1/...", "headers": {"accept": "application/json", "content-type": "application/json"}, "body": "<JSON text>" | null}`, `204` when nothing arrived (the tab polls again at once), or `200 {"standby": true}` when another tab is serving.
   `content-type` is only there for a POST.
2. The tab performs the job with `fetch` in the page: same origin, `credentials: "include"`, the job's method, headers and body.
3. The tab sends `POST /result` with `{"id": 7, "status": 200, "body": "<response text>"}`, or `{"id": 7, "error": "<message>"}` when the browser could not perform it at all (the fetch fails with that message).
   It retries a failed post a few times.
   `ath` answers `204`, or `404` for an id it is no longer waiting for (already answered, or its fetch gave up).

The tab runs 5 such loops, so up to 5 requests are in flight; `ath`'s own `--concurrency` bounds how many it queues.
Five also stays under Chrome's 6 connections per host, so a waiting long poll never blocks a result.

**One serving tab.**
The first tab to poll becomes the active tab, and only it gets jobs.
Another tab polling gets the standby answer at once; on standby only its first loop polls, every 2 seconds, so two tabs never exceed Chrome's connection limit.
A standby tab takes over when the active tab has no poll waiting and has not polled or answered for 5 seconds (it was closed or reloaded; a reloaded page is a new tab id).

**Lost jobs.**
A loop polls only when it holds no job, so a poll from a loop that still holds a job means the job never reached it or its answer was lost (the page aborted the fetch, or the response could not be read): the job goes back to the front of the queue.
When a standby tab takes over, the old tab's unanswered jobs go back too.
Requests are idempotent reads, so handing one out again is safe; a job lost twice fails its request instead of looping.
A poll whose browser side closes is dropped by the server (hyper closes the connection on EOF), so it does not take jobs.
The job carries the full request, so the tab knows nothing about Athletic.net beyond "perform this same-origin request"; URLs, paging, pacing, retries and parsing all stay in `ath`.
`headers` is generic so new endpoints that need a header (e.g. `GetNavInfo`'s `anettokens`) work without changing the installed bookmarklet.

### Timeouts

- A live run binds the relay at startup but waits for a tab only when a request needs one, so a run the cache answers needs no tab, and an input error is reported at once.
- The tab counts as there while it has a poll waiting, holds a request still within its 60s answer window, or polled or answered within `--relay-timeout` (1 to 3600 seconds, default 30).
  A busy tab with every loop working on slow answers is therefore not mistaken for a missing one.
- A queued request fails, with the setup steps, once the tab has not been there for `--relay-timeout`.
  The relay then marks the tab lost, and every later request fails at once with the same steps (instead of each waiting out the timeout), until a tab polls again.
- A handed-out request fails if no result comes back within 60s.
- With no `ath` running, the tab's first loop retries `/next` with a short backoff (0.5s doubling to 3s) and the other loops wait for it, so the console stays quiet and the next `ath` run is picked up within a few seconds.
  A failure after a job was taken (an unreadable job, a failed result post) does not count as `ath` being away.
  Running the script again in the same tab only logs that it is already running (`window.__athRelay` holds its state, including `mode`: `waiting`, `standby` or `serving`).
- When the `RelayFetcher` is dropped, its server task and every connection task (parked long polls included) are aborted.

### Security model

- The server binds `127.0.0.1` only, never other interfaces.
- It answers only requests whose `Origin` header is exactly `https://www.athletic.net`.
  Any other origin, or none, gets a bare `403` with no CORS headers, so no other web page can take jobs, read them, or post results; a DNS-rebinding page would still carry its own origin.
- Responses to the Athletic.net origin carry `Access-Control-Allow-Origin: https://www.athletic.net` and `Vary: Origin`.
  The CORS preflight (needed for the JSON `POST /result`) also answers `Access-Control-Allow-Private-Network: true` for Chrome's private network access checks; current Chrome instead asks the user once for permission for www.athletic.net to reach apps and services on this device (the wording here is approximate and unverified; the 2026-10-26 live check ran in a profile that had already granted it).
- `ath` only hands out `https://www.athletic.net/` URLs (`RelayFetcher` refuses anything else), and the tab refuses any URL whose origin is not the page's, so the relay cannot be used to send the session anywhere else.
- No cookie or other credential leaves the browser: `ath` sees only response status and body text.
- A local process can spoof `Origin`, but local processes are outside the threat model (they already run as the user).

### Browsers

Built for Chrome, which treats `http://127.0.0.1` as potentially trustworthy, so an `https` page may call it.
Other browsers may block that call as mixed content (Safari historically has); use Chrome, or the cookie transport.

See `docs/HANDOFF.md` §3–§5 and §8 (worked reference values for unit tests).
