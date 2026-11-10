---
name: ath
description: Fetch Athletic.net track and field data through the `ath` CLI - an athlete's Profile, marks, Season best and Personal record, where a mark ranks on a State or Area Rankings list, a whole Rankings list, a State's Areas (Sections, Classes), a meet's events and entries (relay teams and legs), or a seed sheet (individual events or relays) for a meet or a roster. Use whenever a user or agent wants Athletic.net data, instead of scraping athletic.net or calling its API directly.
---

# ath

`ath` reads Athletic.net through the user's own signed-in browser session and prints JSON.
Always go through `ath`: never scrape athletic.net, call its API with curl or a script, or drive the site page by page in a browser.
Athletic.net sits behind Cloudflare, and `ath` already does the pacing, caching, retries and paywall checks.

Run it from the repo root as `./target/release/ath`.
Run `cargo build --release` first, every session (a no-op when the build is current), so the binary matches the checkout.
Every subcommand has `--help`; the README section for each command (linked below) has the details.

## 1. Connect to Athletic.net (live runs only)

`--fixtures-dir` runs need neither transport, and a run the response cache answers needs no tab.

**Browser relay (the default).**
`ath` hands each request to an https://www.athletic.net tab, which performs it with its own session; no cookie leaves the browser.
The user must be signed in to Athletic.net in that browser; never ask for or type their password, and never sign in for them.

With Claude in Chrome:

1. Load every Chrome tool you need in ONE ToolSearch call (at least `tabs_context_mcp`, `tabs_create_mcp`, `navigate`, `get_page_text`, `javascript_tool`, `tabs_close_mcp`).
2. Check the open tabs first (`tabs_context_mcp`): an https://www.athletic.net tab that already runs the relay has `window.__athRelay` set.
   Its `mode` is `waiting` (no `ath` running yet, normal), `serving`, or `standby` (another tab is serving, which is fine).
   If one exists, use it and skip to step 5.
3. Otherwise open a new tab on https://www.athletic.net.
   Read the page (`get_page_text`); if it shows the user signed out, stop and ask them to sign in.
4. Run the output of `./target/release/ath relay install --console` in the tab with a single `javascript_tool` call.
   The relay then polls quietly in `waiting` until `ath` runs.
5. Now run the `ath` command, with the default `--relay-timeout`.
   The tab serves it (`mode` becomes `serving`) and every later `ath` run until it is reloaded; close a tab you opened when you are done.

Without Claude in Chrome, ask the user to set it up: copy the script to their clipboard with `./target/release/ath relay install --console | pbcopy` (macOS) and have them paste it into the DevTools console of a signed-in Athletic.net tab (Chrome may ask them to type "allow pasting" first), or point them at the README's "Setting up the browser relay" for the bookmarklet.
Run live `ath` commands one after another, never two at once: one tab serves one relay, and parallel runs break the request budget.

**Cookie (the alternative).**
`--cookie`, `--cookie-file <FILE>` or `ATH_COOKIE` with the browser's whole `Cookie` header, plus `--user-agent` set to that same browser's User-Agent (`cf_clearance` is bound to it).
Any cookie picks this transport for the run.
Cookies expire within minutes to hours; only the user supplies one, and it never goes into a file in the repo, a commit or a message.

## 2. What the session can see

Athletic.net enforces its paywall on the server ([ADR 0001](../../../docs/adr/0001-athletic-net-access-is-cloudflare-gated.md), "Paid subscription"):

| Session | Rankings lists | Profiles, meet entries |
|---|---|---|
| Signed out | first 5 rows | not relied on: sign in first |
| Signed in, unpaid | first 25 rows | readable: `ath profile`, `ath meet`, `ath seed` work |
| Signed in, paid | whole lists | readable |

An unpaid session still answers whatever fits in a list's first 25 rows: `ath list --limit 25` or less, and `ath rank` on a list where the athlete and their Neighborhood sit in those rows.
Any answer that needs rows past the cap (a whole list, an athlete ranked lower, `ath rank --in all` and rankmap in practice) needs a paid subscription.
`ath` never ranks against a capped list: it stops with `Athletic.net returned only 25 of at least N rows ...; full lists need a paid Athletic.net subscription`.
Tell the user that; do not retry or work around it.

## 3. Question -> command

Output is JSON when stdout is not a terminal (always, for an agent); `--table` prints the human view.
Every JSON object has a top-level `schema`.
Links: `ath profile`, `ath list` and `ath meet` have a top-level `url`; `ath rank` has `athlete.url` and each `results[].url`; `ath areas` has `state_list.url` and each `areas[].url`; `ath seed` has `entrants[].athlete_url` and `entrants[].school_url` on an individual sheet, `teams[].team_url` and `teams[].legs[].athlete_url` on a relay sheet, and `meet.url` with `--meet`.
Events: always write a flat race as `400m` or `"400 Meters"`, never a bare number.
`ath profile`, `ath seed --event`, `ath list`, `ath rank` and `ath areas` refuse `--event 400` with a hint; `ath meet --event 800` and `--seed-on 1600` instead report no such event or not a flat race.
`ath list`, `ath rank` and `ath areas` also take Athletic.net codes such as `Shot` or `LJ` (not `SP`).
`--gender` is `f` or `m`.
Ids are the numbers in Athletic.net URLs (`/athlete/<id>/`, `/meet/<id>/`).

| Question | Command | JSON to read |
|---|---|---|
| Who is this athlete, their school, Area, Season best and PR? | `ath profile <ATHLETE_ID> --event 400m` (`--as-of YYYY-MM-DD`, `--season YEAR`) | `name`, `school`, `teams`, `area` / `area_lookup`, `event.status`, `event.season_best` and `event.personal_record` (`mark`, `sort_int`, `date`, `meet`) |
| What lists does a State have? | `ath areas CA --season 2026` | `state_list`, `areas[]` (`id`, `name`, `url`), `level`, `note` |
| Who is on a State's or Area's list? | `ath list --state CA --event 400m --gender f --season 2026` (`--area CCS`, `--list <LIST_ID>`, `--grades 9-10`, `--limit 50`) | `total`, `complete`, `note`, `rows[]` (`place`, `tied`, `name`, `athlete_id`, `mark`, `sort_int`, `grade`, `team`) |
| Where does an athlete's mark rank? | `ath rank <ATHLETE_ID> --event 400m --season 2026` (`--in CA,CCS,TX`, `--in all`, `--in all,areas`, `--ahead all`) | `status` (`ok`/`no_mark`), `mark`, `results[]` (`list`, `group`, `status`, `place.label` like `T23`, `place.neighborhood`) |
| What events and divisions does a meet have? | `ath meet <MEET_ID>` | `name`, `date`, `indoor`, `divisions[]`, `events[]` (`gender`, `division_id`, `event_short`, `relay`, `url`) |
| Who is entered in one event? | `ath meet <MEET_ID> --gender m --div V --event 4x800m` | `entries.entrants[]` (individual) or `entries.teams[]` (relay legs), `entries.left_out[]` |
| Seed sheet for one meet event | `ath seed --meet <MEET_ID> --gender f --div V --event 800m --out <dir>/<meet>-f-800m.csv` | `entrants[]` (`rank`, `athlete`, `pr`, `pr_season`, `sort_int`), `meet.left_out[]` |
| Seed sheet for a meet's relay | `ath seed --meet <MEET_ID> --gender m --div V --event 4x800m --out <dir>/<meet>-m-4x800m.csv` (`--seed-on 1600m`) | `kind` `"relay"`, `seeding_event`, `estimate_from`, `teams[]` (`rank`, `team`, `letter`, `aggregate`, `sort_int`, `estimated`, `no_aggregate_reason`, `legs[]` with `athlete`, `pr`, `sort_int`, `estimated`, `estimated_from`) |
| Seed sheet for a roster | `ath seed --roster <dir>/roster.csv --event 400m --meet-date 2027-05-15 --out <dir>/seed.csv` | `entrants[]`, `unresolved[]` (`name`, `reason`, `note`), `skipped[]` |

Notes that change answers:

- Marks: sort and compare on `sort_int` (lower is better for running events), never on the `mark` string.
- `ath seed` seeds on the outdoor Personal record as of today (the day before, for a meet today), across all seasons; `--div` takes the id, name or short name (`13`, `Varsity`, `V`); `ath meet` lists the values.
- `ath seed` writes the CSV to `--out` (default `seed.csv` in the current directory, replaced): always pass `--out` into a private folder (section 5), one file per event.
- A roster CSV needs `name`; an `athlete_id` column pins a known athlete and skips the name search.
- Relays of four equal legs (`4x100m`, `4x200m`, `4x400m`, `4x800m`, `4xMile`) are seeded from a meet's entries only, never a roster.
  A team's aggregate is the sum of its four runners' Personal records in the Seeding event (the leg distance; `--seed-on <flat running race>` picks another, with no conversion).
  On a 4xMile each runner gets the better of their 1600m and their 1 Mile converted to the 1600m (rounded up, flagged `estimated: true` on the leg and the team).
  A team with a leg missing, a runner with no mark, or one runner on two legs has no aggregate (`no_aggregate_reason`, e.g. `Leg 4 has no 800 Meters mark`, `Leg 3 has no runner`, `Same runner on legs 1 and 3`) and goes to the bottom.
  An individual sheet has no `kind` and is unchanged.
- `ath rank --in all` is about 200 lists; `--in all,areas` adds the home State's Areas.
- `ath profile` reports field events with `event.status` `field_event`: v1 reads running events only for bests and seeding.

Offline, on made-up data in `tests/fixtures` (no transport needed):

```bash
./target/release/ath profile 2001 --event 400m --as-of 2026-04-01 --fixtures-dir tests/fixtures
./target/release/ath meet 900700 --gender m --div V --event 4x800m --fixtures-dir tests/fixtures --table
./target/release/ath seed --meet 900700 --gender m --div V --event 800m --fixtures-dir tests/fixtures --out /tmp/seed.csv
./target/release/ath seed --meet 900700 --gender m --div V --event 4x800m --fixtures-dir tests/fixtures --out /tmp/r.csv
./target/release/ath seed --meet 900702 --gender f --div V --event 4xMile --fixtures-dir tests/fixtures --out /tmp/m.csv
cp tests/roster_400.csv /tmp/roster.csv
./target/release/ath seed --roster /tmp/roster.csv --event 400m --fixtures-dir tests/fixtures --out /tmp/seed.csv
./target/release/ath areas CA --season 2026 --fixtures-dir tests/fixtures
```

## 4. Be a good API citizen

- Keep the defaults for `--concurrency` (5) and `--delay-ms` (250); never raise the one or lower the other to go faster.
- Rely on the response cache: repeat runs are answered from it (finished seasons' lists forever, in-season lists for 12 hours, Profiles for about 12 hours, meets for about an hour).
  Use `--refresh` only when fresh data matters (marks from today's meet, an entry list that just changed); never `--no-cache` for live runs.
- Ask the user before anything that reads hundreds of Profiles or lists: `ath rank --in all` (about 200 lists, several minutes), a whole State list with no `--limit` (California's can be 100+ pages), or a big roster or meet.
- Run live `ath` commands one after another, never in parallel.

## 5. Privacy

Live output is about real athletes, mostly minors.

- Write it only to `local/` of the main checkout (gitignored, shared by every worktree; its path is the first `worktree` line of `git worktree list --porcelain`) or to a temp dir.
  When `ATH_LOCAL` is set, it replaces that `local/` (the reports read it), so write there instead.
  In a worktree, never write to the worktree's own `local/` or anywhere tracked.
- Rosters go there too: `ath seed --roster` rewrites the roster CSV in place with the `athlete_id`s it resolved, and writes `<roster>.unresolved.csv` beside it.
  Only ever pass a roster that lives in `local/` or a temp copy, never a tracked file (the roster example in section 3 copies the fixture roster first).
- Never put real names, ids or marks into commits, test fixtures, PRs, any issue tracker, or public pages; fixtures and examples use made-up names ([ADR 0010](../../../docs/adr/0010-anonymized-examples-and-fixtures.md)).
- Summarize results with names only in the chat, to the user who asked.

## 6. Errors -> meaning -> what to do

| `ath` says | Meaning | Do |
|---|---|---|
| `no Athletic.net browser tab connected to the ath relay on 127.0.0.1:8767 within 30s` | No relay tab is polling | Set up the tab (section 1) first, then re-run |
| `the Athletic.net tab running the ath relay ... stopped answering` / `took ... but did not answer` | The tab was closed or reloaded mid-run | Reload the tab, then run the console script again (it refuses to install twice in a tab that was not reloaded), and re-run. Requests already answered come from the cache, unless the run used `--refresh`: then re-run without it if the data is fresh enough, or accept the refetch |
| `Athletic.net returned a Cloudflare challenge (HTTP ...) after N attempts` | The relay tab must pass a check, or the session is rate limited; with a cookie, the cookie expired or the User-Agent does not match | Relay: ask the user to reload the tab, pass any "verify you are human" check, then re-run the console script; wait a few minutes if it repeats. Cookie: ask for a fresh cookie and matching `--user-agent` |
| `Athletic.net returned an empty response` (or `a non-JSON page`) `... Athletic.net is throttling this burst of requests` | Rate limited | Wait a few minutes and re-run (the cache resumes where it stopped); do not reload the tab or re-authenticate, and do not raise `--concurrency` |
| `returned only 5` / `only 25 of at least N rows ...; full lists need a paid Athletic.net subscription` | Signed out (5) or unpaid (25) session; the list is capped | Tell the user; it needs a signed-in paid subscription. `--limit` within the cap still works for `ath list` |
| `Athletic.net refused ... even with a fresh meet token` / `gave no jwtMeet token for meet N` | The meet's entries are not readable by this session, or not published | Check the user is signed in (a paid subscription may be needed), or that the meet publishes entries |
| `meet N was on YYYY-MM-DD, in the past` / `--meet-date ... is in the past` | Seed sheets are only for upcoming meets | Tell the user; `ath` does not reconstruct a past seeding. Use `ath meet` for its entries |
| `... is a relay whose legs are not all one flat race (a medley or shuttle relay), and ath seed seeds only relays of four equal legs (such as 4x800m or 4xMile) so far` | Medley and shuttle relays are not seeded | Use the `ath meet ... --event <relay>` command it prints for the teams and legs |
| `--event "4x400m" is a relay, and relays are seeded from a meet's entries: use --meet` | A relay given with `--roster` | Re-run with `--meet`, `--gender`, `--div` and the relay as `--event` |
| `--seed-on "..." is a relay` / `is not a flat running race` / `is only run indoors` / `is for relays, and ... is not one` | `--seed-on` takes one outdoor flat running race, and only with a relay | Pick a flat race such as `1600m` or `"1 Mile"`, or drop `--seed-on` |
| `... is a field event; ath seeds running events only` / `... is only run indoors` | Out of v1 seeding scope (field events; 55m, 60m, 300m, 500m, 600m, 1000m and indoor hurdles) | Tell the user; show the entries with `ath meet` instead |
| `N row(s) did not resolve - review <roster>.unresolved.csv, pin an athlete_id, and re-run` | Roster rows that did not resolve to one Profile (`unresolved[].reason`) | `ambiguous`: show the user the ranked candidates in `<roster>.unresolved.csv` (`cand_rank`, `cand_name`, `cand_school`, `athlete_url`), and after they pick, add `athlete_id` to the roster row and re-run; never guess. `no_match`: no candidates; fix the name or school, or ask for the athlete's id. `search_failed`: transient and not in the CSV; just re-run |
| `ath rank` result `status` `no_mark` | The athlete has no mark this season yet | Tell the user; do not guess a mark |
| `ath rank` result `status` `lookup_failed` | That one list could not be read | Re-run later; the lists already read come from the cache |
| `--event "400" is a bare number; did you mean 400m?` | Event spelled as a number | Use `400m` or `"400 Meters"` |
| `meet N has no boys' event "800"; its boys' events are ...` | `ath meet --event` with an event the meet does not have, often a bare number | Use a code from the list it prints, such as `800m` |
| `--seed-on "1600" is not a flat running race ...` | A bare number (or a field event or hurdles) given to `--seed-on` | Use `1600m` or `"1 Mile"` |

## 7. More

- README: "Usage" (`ath seed`), "`ath seed --meet`", "`ath meet`", "`ath profile`", "`ath areas`", "`ath list`", "`ath rank`", "Output: JSON or table", "Reaching Athletic.net", "Your own configs and data: `local/`".
- [`CONTEXT.md`](../../../CONTEXT.md): the glossary (Personal record, Season best, Qualifying mark, Area, Grade group).
- [`docs/agents/systems/athletic-net.md`](../../../docs/agents/systems/athletic-net.md): the endpoints, encodings and the relay protocol.
- [ADR 0006](../../../docs/adr/0006-pluggable-transport-browser-relay-by-default.md): why the relay is the default transport.
- The `rankmap` and seedsheet reports drive `ath` for you: `.claude/skills/rankmap/SKILL.md`, `docs/seedsheet.md`.
