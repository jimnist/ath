# Athletic.net Seed-Time CLI - Spec & Handoff

A build spec for a CLI that generates meet **seed sheets** from Athletic.net, driven by CSV input/output. This document captures everything learned from doing the task manually for the 2026 Nike Outdoor Nationals (meet 625323), so it can be reproduced programmatically.

---

## 1. Goal

Given a meet and a set of events, produce, for each event, a ranked seed list built from each entrant's **Personal record** in the event as of today, or as of the day before for a meet held today (the Seed mark, `CONTEXT.md`):

> **Changed:** the seed mark was each entrant's **2026 season-best mark** when this spec was written and when the 625323 sheet was hand-built; it is now the Personal record across all seasons.
> Read "2026 PR" / "2026 mark" below as that history; the validation notes against meet 625323 describe the season-best behavior and are historical.
> Seed sheets are only for upcoming meets, so marks are as of today, or as of the day before for a meet held today, so the meet's own results never count (§4).

- **Individual running events** (100m, 400m, …): one row per athlete → `name, athlete URL, PR, team, team URL`, sorted fastest → slowest.
- **Field events** (Shot Put): same shape, but marks are distances; sort longest → shortest.
- **Relays** (4×800, 4×Mile, …): one row per team → `team, aggregate time, runner1..4 + each runner's PR`, where aggregate = sum of the four runners' individual PRs in the seeding event; sort fastest → slowest.

Athletes/teams with **no Qualifying mark** in the event are left blank and sorted to the bottom.

The seeding event is not always the relay's nominal distance - e.g. the **4×Mile is seeded on each runner's 1600m PR** (a per-event mapping the user supplies).

> **As built (relays):** `ath seed --meet` maps a relay of four equal legs to its Seeding event itself, from the relay's code or name (4x100m → 100m, 4x200m → 200m, 4x400m → 400m, 4x800m → 800m, 4xMile → 1600m, with each runner's mile converted to the 1600m as an Estimated mark when faster), and `--seed-on` is the user's override.
> Medley and shuttle relays are not seeded yet; the as-built CLI and CSV are in §6.

---

## 2. Data sources

### 2.1 Entries page (which athletes/teams are entered)

URL pattern:

```
https://www.athletic.net/TrackAndField/meet/{meetId}/entries/{gender}/{division}/{event}
```

- `{gender}`: `f` (female) | `m` (male)
- `{division}`: numeric, **meet-specific** (observed: `3`=Freshman, `1`=Championship, `2`=Emerging Elite)
- `{event}`: slug - `100m`, `400m`, `800m`, `shot`, `4x800m`, `4xmile`, etc.

Examples used:
- `…/entries/f/3/400m` → Womens Freshman 400 Meters
- `…/entries/m/2/shot` → Mens Emerging Elite Shot Put
- `…/entries/f/1/4x800m` → Womens Championship 4×800 Relay
- `…/entries/f/1/4xmile` → Womens Championship 4×Mile Relay
- `…/entries/m/2/4x800m` → Mens Emerging Elite 4×800 Relay

**The page is an Angular SPA** - raw HTML has no data; it must be rendered (we scraped the live DOM). For a CLI, prefer finding the underlying entries JSON API (see Open Questions); DOM structure as a fallback:

- **Individual events:** table rows; each accepted entrant row contains a `.badge` reading `Accepted`/`Rejected`/`Waitlisted`, an athlete link `a[href*="/athlete/{id}"]`, and a team cell. Team is sometimes a link `a[href*="/team/{id}"]` (clubs) and sometimes plain text (schools). Only keep `Accepted`.
- **Relay events:** a **team row** (`/team/{id}` link + `Accepted` badge + rank `"N."`) is followed by four **leg rows** ranked `A1`, `A2`, `A3`, `A4`, each holding one athlete link. Take the **first four** runners per team.

`{id}` for athletes/teams comes from the href: `/athlete/(\d+)/…`, `/team/(\d+)/…`.

### 2.2 Athlete bio API (the source of marks)

```
GET https://www.athletic.net/api/v1/AthleteBio/GetAthleteBioData?athleteId={id}&sport=tf&level=0
Accept: application/json
```

Returns JSON. Relevant fields:

- `athlete` → `{ IDAthlete, SchoolID, FirstName, LastName, … }` (`SchoolID` is the athlete's current/primary school; useful to build a team URL when the entries page didn't give one: `https://www.athletic.net/team/{SchoolID}/track-and-field-outdoor`).
- `eventsTF` → array of event **definitions** for events this athlete has competed in:
  `{ IDEvent, Event ("800 Meters"), IDEventType, Description ("12lb"), FieldMeasureType, Type ("T"/"F") }`
- `resultsTF` → array of **results**:
  `{ EventID, EventTypeID, SeasonID, Result ("1:02.47a"), SortInt, SortIntRaw, Exhibition, Place, Round, ResultDate, FAT, SchoolID, … }`

Was called with session cookies (`credentials: include`) while logged in; the endpoint may be public - **verify whether auth is required** (see Open Questions).

---

## 3. Critical encodings & gotchas (the hard-won part)

### 3.1 Event IDs are **gender-specific** - resolve by name, don't hardcode

The same distance has different `IDEvent` values for boys vs girls. Observed:

| Event | Girls IDEvent | Boys IDEvent |
|---|---|---|
| 100 Meters | 19 | - |
| 400 Meters | 21 | - |
| 800 Meters | **22** | **4** |
| 1500 Meters | 23 | - |
| 1600 Meters | 53 | - |
| 1 Mile | 59 | - |
| Shot Put | 30 (4kg) | 12 |

**Do not hardcode IDs.** For each athlete, look up the target event in `eventsTF` by **`Event` name string** (e.g. `Event === "800 Meters"`), collect the matching `IDEvent`(s), then filter `resultsTF` by those IDs. This is robust across gender and divisions. (Hardcoding 800m=22 silently returned zero marks for the boys' relay until we switched to name resolution.)

### 3.2 `EventTypeID` - exclude relay splits; encodes implement weight for field events

- `EventTypeID === 0` → standard **individual** mark. Keep.
- `EventTypeID === 98` → **relay split** (e.g. a 4×400 leg recorded under "400 Meters"). **Exclude**: these are faster than open marks and corrupt PRs (caught an athlete seeded off a relay split almost three seconds faster than her real open 400).
- **Shot Put:** the implement weight is carried by `IDEventType` (in `eventsTF`) / `EventTypeID` (in results), matched to `Description`: `0`="12lb", `10`="10lb", also "8lb", "4kg", "2kg". To get a specific implement, find the `IDEventType` whose `Description` matches the wanted weight, then filter results by that `EventTypeID`. (12lb is `EventTypeID 0` for boys.)

### 3.3 `SortInt` - the reliable numeric key (never parse the display string)

- **Running/relay events:** `SortInt` = time in **milliseconds** (e.g. 1:02.47 → 62470). Lower = faster. Sum `SortInt`s for relay aggregates; `seconds = SortInt/1000`.
- **Field events:** `SortInt = 20_000_000 − (distance_in_inches × 1000)`. Lower = farther. Therefore:
  `inches = (20_000_000 − SortInt) / 1000`. The encoding was verified against both imperial and metric marks, so SortInt unifies the two unit systems for free (illustrative values: `51' 5"` → 617 in, `15.07m` → 593.31 in).
- **Best mark = min `SortInt`** among the filtered results (works for both running *and* field, since lower is better in both encodings).

Use SortInt for all sorting and summing. The `Result` display string is unreliable to parse (mixed formats, see below).

### 3.4 `Result` string formats

- Running: `"1:02.47a"`, `"58.50a"`, `"2:05.43"`. Trailing **`a` = fully-automatic timing (FAT)** - strip for display. Trailing **`h` = hand-timed**.
- Field (Shot Put): **mixed units within one column** - for example (illustrative values) imperial `"56-03.00"`, `"54' 9\""`, `"53-7"`, and metric `"17.52m"`. Inconsistent even within one athlete.
- Non-marks appear as results too: `"DNS"`, `"DNF"`, `"NT"`, `"SCR"`, `"DQ"`, `"FS"`. These carry a `SortInt` (a large placeholder for running) so they'll masquerade as a "best" if not filtered out - this produced nonsense relay aggregates like `670:52` until excluded.

### 3.5 Rate limiting

The bio API throttles rapid calls: after ~40 quick requests it returns an **HTML page instead of JSON** (detect: response doesn't start with `{`). Handle with: bounded concurrency (≈5–6 in flight), a small delay between batches, and retry-with-backoff on the "blocked"/non-JSON response. Expect to need 1–2 retry passes for a ~100-athlete event.

---

## 4. The Seed mark - the Qualifying mark filter (applies per athlete, per event)

From `resultsTF`, keep a result only if **all** hold (a Qualifying mark):

1. `EventID` ∈ the IDs resolved by name for the target event (§3.1)
2. **Outdoor:** `SeasonID < 10000` (indoor seasons are `10000 + year` and share the outdoor `EventID`s; v1 never counts indoor marks)
3. `EventTypeID === 0` (individual; excludes relay splits) - *or* the specific weight's `EventTypeID` for shot put
4. `Exhibition === false`
5. **Not hand-timed:** `Result` does **not** end in `h`
6. **Is a real time/mark:** after stripping a trailing `a`/`h`, `Result` matches `/^(\d+:)?\d+(\.\d+)?$/` (excludes DNS/DNF/NT/SCR/DQ/FS)
7. Dated on or before the as-of date (`ResultDate`); a result with no date counts when the as-of date is today or later (it can only be on or before today), or when its season ended before the as-of date's year (so it cannot be after the as-of date)

Then **the Seed mark = the result with the minimum `SortInt`, in any outdoor season**: the Personal record as of the as-of date.
If none qualify → the athlete has **no mark** (blank).
A tie on `SortInt` goes to the earliest dated result, since that is when the mark was set; an undated result loses a tie to a dated one.

There is no restriction to one season.
Until seeding moved to the Personal record, the season rule was `SeasonID === 2026` (outdoor season), so entrants were seeded on their 2026 Season best; it was dropped because an entrant who ran faster in an earlier season is seeded on that faster mark.
The seed sheet's `pr_season` column says which season each Personal record was set in.

The as-of date is today, except for a meet today (`ath seed --meet-date` today), when it is the day before: a result from the meet itself must never become its seed.
A seed sheet is only for an upcoming meet, so `--meet-date` refuses a date in the past; `ath` never reconstructs a past seeding.

Display value: running → `Result` with trailing `a` stripped. Field → normalize via SortInt (§5).

---

## 5. Per-event-type output behavior

### Individual running (100m, 400m, 800m…)
- Columns: `Athlete` (hyperlink to athlete page), `PR` (string; the hand-built 625323 sheet had `2026 PR`), `Team` (hyperlink).
- Sort ascending by `SortInt` (fastest first). Blanks to bottom.
- Sub-minute times are pure numbers; minute+ times contain a colon. Keep a hidden numeric **seconds** key (`SortInt/1000`) for sorting since the display column mixes numeric and text.

### Field (Shot Put)
- Capture the requested implement(s) (e.g. 12lb; optionally a second like 2kg in its own column).
- **Normalize display to one unit.** US meet → **feet-inches**: from `inches = (20000000 − SortInt)/1000`, format `FF-II.II` = `floor(inches/12) + "-" + (inches mod 12).toFixed(2).padStart(5,"0")`. This makes metric and imperial source marks consistent and is exact for imperial.
- Sort **descending** by inches (longest first). Hidden numeric `inches` key drives the sort.

### Relays (4×800, 4×Mile, …)
- Seeding event per the meet's mapping (e.g. 4×Mile → **1600 Meters**; 4×800 → 800 Meters).
- Take first 4 listed runners (A1–A4). For each, get the Seed mark (Personal record) in the seeding event (§4).
- **Aggregate** = sum of the four runners' `SortInt` (ms) → display `M:SS.00` = `floor(total/60000) + ":" + ((total%60000)/1000).toFixed(2).padStart(5,"0")`.
- **If any of the four runners lacks a qualifying mark, leave the aggregate blank** (don't sum partial) and drop the team to the bottom; still show the runner names + whatever individual PRs exist.
- Columns: `Team, Aggregate Time, Runner 1, Runner 1 PR, Runner 2, Runner 2 PR, Runner 3, Runner 3 PR, Runner 4, Runner 4 PR`. Hidden numeric aggregate-seconds key for sorting (ascending).
  As built, the relay CSV follows §6's relay output, with the team's letter, each runner's athlete id and the Estimated-mark columns added (§6, Usage).
- A team with fewer than four runners entered is incomplete too: blank aggregate, at the bottom.
- 4×Mile (as built): each runner is seeded on the faster of their 1600m and their mile converted to the 1600m at even pace (`x 1600 / 1609.344`), the converted mile flagged as an Estimated mark (§6, Usage).

---

## 6. Proposed CLI design (CSV in / CSV out)

### Usage (as built)

`ath` is a subcommand CLI (one noun command per data question, per ADR 0007).
Seeding is `ath seed`, which takes a roster CSV and one event and writes one ranked seed sheet CSV, plus `<roster>.unresolved.csv` for rows that need an `athlete_id` pinned:

```
ath seed --roster roster.csv --event "400 Meters" [--meet-date YYYY-MM-DD] [--out seed.csv]
```

Entrants are seeded on their Personal record as of today, or as of the day before for a meet held today (§4, `--meet-date`).
`--meet-date` is optional; for a meet today marks are as of the day before, and a date in the past is refused with a message saying why, and nothing is written.
`--season` was removed when seeding moved to the Personal record (a deliberate CLI break): a Personal record spans every season, so there is no season to pick.
The seed sheet's columns are `rank, athlete, athlete_id, athlete_url, pr, pr_numeric, school, school_url, club, club_url, pr_season`, where `pr` is the Personal record and `pr_season` the season it was set in.
Since the seed sheet page was built, `ath seed` also prints the sheet under the output contract (ADR 0007), a table in a terminal and JSON otherwise; the CSV it writes is unchanged.
The JSON (`schema` 1) has `event`, `event_name`, `as_of`, `meet_date`, `entrants` (the CSV's rows in the CSV's order, each with the Personal record's `SortInt` as `sort_int`), `skipped` (Profiles that could not be read) and `unresolved` (roster rows that did not resolve to one athlete, with a `reason` and a `note`).
It is printed even when no roster row resolves (then no CSV is written), and it has no top-level `url`: a roster's sheet has no Athletic.net page, so its links are per entrant.
The seed sheet page, `tools/seedsheet/`, renders it (`docs/seedsheet.md`).

Mode A takes the entrants from a meet's entries instead of a roster, for one running event in one division (an individual event, or a relay, below):

```
ath seed --meet 900700 --gender m --div 13 --event 800m [--out seed.csv]
```

`--roster` and `--meet` are mutually exclusive, and `--meet` takes neither `--meet-date` (the date is the meet's own) nor `--resolution-log` (entries carry athlete ids, so nothing is resolved by name).
A meet is refused once it is over (its last day before today); marks are as of today, or at the latest the day before the meet's first day, so a two-day meet on its second day never seeds on its first day's results.
Each accepted, unscratched entrant's Profile is read by athlete id and seeded exactly as a pinned roster row is, on the event as the meet names it (`800 Meters`).
The JSON adds `meet`: the meet (`id`, `name`, `date`, `season_id`, `indoor`, `url`), the `event` in its division (with its entries page as `url`) and `left_out`, the entries that are not Entrants, with a `reason` (`scratched`, `not_accepted`, `no_athlete`).
From the meet's event list, before any entries are read, it refuses a medley or shuttle relay (by code or name: `SMR 1600m`, `sprintmed2248`, `DMR`, shuttle hurdles) with a pointer to `ath meet`, a field event, and an event only run indoors (55m, 60m and their hurdles, 300m, 500m, 600m, 1000m), since seeding reads outdoor Personal records only.
A 401/403 or an authorization denial from the event list or entries drops the meet token and retries once with a fresh one; a second refusal fails, saying a paid Athletic.net subscription may be needed (ADR 0001).
Any other error is reported with Athletic.net's message, and no refusal or error is ever cached.

Relays: a relay whose four legs are one flat race is seeded per team (§5 Relays), from a meet's entries only; `ath seed --roster` with a relay as `--event` is refused, saying to use `--meet`, since a roster names no teams:

```
ath seed --meet 900700 --gender m --div 13 --event 4x800m [--seed-on 800m] [--out relay.csv]
```

The Seeding event comes from the relay's code or name (4x100m → 100m, 4x200m → 200m, 4x400m → 400m, 4x800m → 800m, 4xMile → 1600m); `--seed-on` names another, which must be one individual outdoor flat race (a relay, a field event, hurdles or an indoor-only event is refused, as is `--seed-on` with an individual event), checked from the event list before any entries are read.
A leg distance is read only with a unit (`m`, meters, mile, `mi`) or as a standard flat race in meters (100, 200, 400, 800, 1600, 3200: `4x800 Relay`); yards and other bare numbers (`4x4`) are refused, saying the distance could not be read.
A team's runners are legs 1 to 4, each the first entry with that leg number; alternates (a second entry for a filled leg, or one with no leg number) never take a leg, and a leg with no entry leaves the team a runner short.
One athlete on two legs of a team leaves it without an aggregate ("Same runner on legs 1 and 3"), and entries with no leg numbers on any team stop the run with that reason, writing nothing.
Each runner is seeded exactly as an individual entrant is (§4: outdoor Qualifying marks, relay splits excluded, as of the same day).
On a 4xMile (with no `--seed-on`), each runner's mark is the faster of their 1600m Personal record and their 1 Mile Personal record converted to the 1600m at even pace (`mile x 1600 / 1609.344`, on `SortInt` milliseconds, rounded up to the next hundredth so `sort_int` is the mark shown and the aggregate is the sum of the legs shown; a tie goes to the 1600m).
A converted mile is an Estimated mark: the leg is flagged `estimated` with the mile it came from, and the team is flagged as including estimated legs.
The aggregate is the sum of the four `SortInt`s, in 1600m terms on a 4xMile; a team with a leg that has no runner, or with a runner without a mark (or whose Profile could not be read), has a blank aggregate and goes to the bottom in entry order.
The relay CSV's columns are `rank, team, letter, team_url, aggregate, aggregate_seconds, aggregate_estimated, no_aggregate_reason` (why a team has no aggregate, leg by leg number), then `runnerN, runnerN_id, runnerN_url, runnerN_pr, runnerN_estimated_from` for leg N = 1 to 4 (`runnerN_estimated_from` is what an Estimated mark came from, `1 Mile 5:02.10`, blank otherwise).
The JSON is `schema` 1 with `kind: "relay"` (an individual sheet has no `kind`, and readers choose by it): the individual sheet's `event`, `event_name` (the meet's), `as_of`, `meet_date`, `meet`, `skipped` and `unresolved`, plus `seeding_event`, `seeding_event_name` and `estimate_from`, and `teams` (each with `missing_legs`, `same_runner_legs`, `no_aggregate_reason`, `estimated` and its runners as `legs`) where an individual sheet has `entrants`; the README (`ath seed --meet`, Relays) lists the fields.
`ath meet <id>` lists the meet and every event and division with entries; with `--gender`, `--div` and `--event` it prints that event's entries, a relay's as teams with their legs.
The seed sheet page's meet mode (`docs/seedsheet.md`) runs `ath meet` and then `ath seed --meet` per event (listed, or every event `ath meet` lists), and writes an index page for the meet; an event `ath seed` refuses is listed there as not seeded, with `ath`'s reason.

Transport and cache flags (`--cookie` / `ATH_COOKIE`, `--cookie-file`, `--user-agent`, `--relay-port`, `--relay-timeout`, `--refresh`, `--no-cache`, `--cache-dir`, `--concurrency`, `--delay-ms`) are global: every subcommand shares them, and they are accepted before or after the subcommand name.
The transport is chosen per run (ADR 0006): a cookie (`--cookie`, `--cookie-file` or `ATH_COOKIE`) picks the cookie transport, and without one `ath` fetches through the browser relay, set up once per browser session with the bookmarklet `ath relay install` prints.
This is a deliberate CLI break: the earlier bare form `ath --roster ... --event ...` was removed with no alias, and running it exits with an error that points at `ath seed`.
The events-CSV design below is the original proposal and is not built yet.

### Input CSV - one row per event to build
Columns:

```
event_label,        # human label e.g. "Womens Freshman 400 Meters"
entries_url,        # full athletic.net entries URL (or supply meet_id+gender+division+event_slug instead)
type,               # individual | field | relay
seed_event_name,    # Athletic.net event NAME to seed on, e.g. "400 Meters", "1600 Meters", "Shot Put"
meet_date,          # e.g. 2027-05-15 (refused if past; was `season` before seeding moved to the Personal record)
implement,          # field events only, e.g. "12lb" (optional second col can be added)
legs                # relays only, default 4
```

### Output CSV - one file per event (or a combined file with an `event_label` column)

Individual / field:
```
rank, athlete, athlete_url, pr, pr_numeric, team, team_url
```
Relay:
```
rank, team, team_url, aggregate, aggregate_seconds,
runner1, runner1_url, runner1_pr, runner2, runner2_url, runner2_pr,
runner3, runner3_url, runner3_pr, runner4, runner4_url, runner4_pr
```
- `pr_numeric` / `aggregate_seconds`: the SortInt-derived numeric key (seconds, or inches for field) - already sorted, but exposed so downstream tools can re-sort/merge.
- Rows with no mark sorted last; `pr`/`aggregate` blank.

### Processing flow
1. Read input CSV.
2. For each event: fetch entries (entries API if found, else rendered DOM) → list of accepted athletes (individual/field) or teams+4 runners (relay).
3. For every unique athlete, call `GetAthleteBioData` (throttled, cached) → resolve target `IDEvent`(s) by name → apply §4 filter → best `SortInt` + display.
4. Field: convert SortInt→inches→feet-inches. Relay: sum legs, blank if incomplete.
5. Sort (running/relay ascending by ms; field descending by inches; blanks last).
6. Write output CSV.

Suggested flags: `--meet`, `--in events.csv`, `--out-dir ./out/`, `--concurrency 5`, `--delay-ms 250`, `--cache ./cache/` (cache bio JSON by athleteId to avoid re-fetching across events - many athletes appear in multiple events), `--cookie` (if auth needed).

---

## 7. Open questions / to investigate

- **Entries API (answered 2026-11-06):** the entries pages were scraped from the rendered DOM until a JSON API was found behind the SPA.
  It is three calls: `Meet/GetMeetData` (the meet, its divisions and a `jwtMeet` token), then `Meet/GetEventListData` and `POST Meet/GetEntriesData` with that token in an `anettokens` header, confirmed on an open indoor meet.
  `ath meet` and `ath seed --meet` (Mode A) read them, so the CLI needs no rendered page; the shapes are in `docs/agents/systems/athletic-net.md` (Meets).
  An entry is an Entrant only when its `AcceptanceStatus` is 10 (accepted, the only value seen) and it is not `Scratched`; a relay's entries are one row per leg, lettered `A1`..`A4` (`B1`..`B4` for a school's second team).
  Still open: what a session without a paid subscription sees, and what other `AcceptanceStatus` values mean.
- **Auth:** confirm whether `GetAthleteBioData` requires a logged-in cookie or works anonymously. If a cookie is needed, accept it via `--cookie`/env var.
- **`level=0` param:** meaning unconfirmed; worked for all queries. Verify it doesn't filter anything out.
- **Division codes (answered 2026-11-06):** the `{division}` integer in entries URLs is meet-specific, so it is never guessed: the meet's divisions come from `GetMeetData`'s `tfDivisions` (`IDDiv` is the URL's number).
  `ath seed --meet` and `ath meet` take a division by that id, its name or its short name, and refuse one the meet does not have, listing the ones it has.
- **Field-event SortInt base (20,000,000):** verified for Shot Put across imperial+metric. Confirm the same base holds for other field events (discus, jav, jumps) if you extend beyond shot put; the `FieldMeasureType` in `eventsTF` (`S`/`V`/`L`) may matter for vertical vs horizontal.
- **Season model (resolved for v1):** `SeasonID === 2026` captured 2026 outdoor.
  Indoor is a separate season id, `10000 + year` (`12026` is "2026 Indoor" in the bio's `allSeasons[].Display`; checked live 2026-10-26), with the same `EventID` as outdoor, and a December meet belongs to the next year's indoor season (2021-12-11 is in `12022`).
  v1 counts outdoor marks only: indoor results are never Qualifying marks, for the Seed mark, the Season best or the Personal record (§4).
  Whether indoor marks should ever count is left for later.

---

## 8. Worked reference values (for unit tests)

The values are illustrative examples of the encodings in §3, which were verified on real responses; they are not real marks.

- Running ms: `1:02.47` ⇄ SortInt 62470 ; sum of legs = aggregate ms.
- Field inches: SortInt 19569000 → 431 in (`35-11.00`); 19383000 → 617 in (`51-05.00`); 19406693 → 593.31 in (`15.07m` → `49-05.31`).
- Relay aggregate: four 800m legs summed → e.g. `1:59.10 + 1:59.62 + 1:59.24 + 2:00.50` = `7:58.46`.
- Gender event-id switch: girls 800m `EventID 22`, boys 800m `EventID 4` (resolve by name "800 Meters").
- Exclusions that mattered: relay split (`EventTypeID 98`, a 400m split almost three seconds faster than the open mark), hand time (`…h`), `DNS` (inflated a relay aggregate to 670:52 until filtered).