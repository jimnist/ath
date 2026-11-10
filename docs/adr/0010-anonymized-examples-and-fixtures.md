---
status: accepted
---

# Fixtures and examples come from one anonymizer over real recordings

Real Athletic.net responses are recorded locally (`ath --record-dir local/recordings/<scenario>`, never committed).
One deterministic anonymizer (`tools/anonymize/anonymize.py`) turns them into everything public that needs data: the parser fixtures, the rankmap and seedsheet golden pages, and the published examples.
It replaces every person, school, team and meet with an invented one, consistently across files, and keeps the shapes that matter: the raw JSON structure, list lengths, grades, ranks and ties.
`docs/anonymize.md` has the details.

## How it anonymizes

- **Names and places**: athlete, school, club, team code, city, venue and meet names become invented ones; the same real thing maps to the same invented thing in every file.
  Invented last names and places are never one of the recording's real values; first names come from a list of common names, since a first name alone names nobody.
- **Ids**: athlete, team, meet, result and user ids, and list and division ids, come from a reserved range (nine digits starting with 9, above any id Athletic.net has issued), so a stray id never reaches a real person and a list cannot be matched with the public one by its id; file and folder names named by an id move with it.
- **Seasons**: fictional, ending at 2099 (the latest recorded outdoor season is 2099, the rest keep their order and gaps), so no season names a real year; every date moves with its season, plus a fixed 1 to 3 days either way that keeps it in its season.
  Replays pin the clock (`ATH_TODAY`) to a date moved the same way.
- **Rows**: kept list ids and ranks would let anyone pair each row with the public list, so a keyed 10% to 20% of the athletes on the lists (never a subject, an entrant or a kept id) leave every list, each replaced by an invented athlete of the same school and grade placed a few places away, strictly between two marks; ranks, `rowNum`s and pages are rebuilt, lengths kept.
  Ranks in examples are therefore realistic, not real.
- **Grades**: shuffled in pairs among those same athletes (protected ones keep theirs), partners on the same lists (same Area) and far apart in mark, so every list keeps its per-grade counts and each Area its grade make-up while most rows show another grade than the real one.
  The Area is kept: what remains to narrow a row down is its Area and an approximate mark, an accepted residual risk.
- **Marks**: one rank-preserving map per event, applied to every mark of that event in every file (State and Area lists, the subject, entries' seed marks), with `SortInt` updated together with the display string.
  Each distinct real mark moves by its own amount: a baseline toward a better mark plus a keyed jitter, in whole display units, bounded by half the gap to each neighbour, so no two marks cross or meet, ties stay ties, and every rank and "meets the standard" outcome stays as in the recording, while every displayed mark differs from the one it replaces and the gaps between marks do not stay as recorded.
  The State Meet standards are points of the same map, so no mark crosses one; examples show the mapped standards (`--areas-out`), which differ slightly from the published ones; the real standards stay in `areas/*.json` for real use.
  The 1 Mile moves through its 1600m estimate, and every runner's mile-against-1600m decision is checked to stay as recorded.
- **The key**: the run is repeatable from a private 256-bit key (`--key-file local/anonymize.key`); the invented values are keyed hashes of the real ones, so the key stays in `local/` with the recordings (a small `--seed` is for tests only).
- **Reviewed fields**: every key of every response is on a reviewed list per response kind; locations, contacts, outside links and tokens are blanked, and any key not on the list stops the run until someone reviews it.

Every run checks its own output and writes nothing when a real name, id, date or mark is left where it was, or a real id or whole name turns up anywhere outside the public structure.

## Considered options

- **Real grades** (until 2026-11-08): with the Area and a mark window, a row's grade could single out one real athlete; shuffling grades among the other athletes of the same lists breaks that (decided 2026-11-08).
- **Real seasons, list ids and ranks** (the first builds): every row could be paired with the public list by its list id, season and rank, whatever the names and marks said; fictional seasons, reserved list ids and reshuffled rows break that, chosen over keeping the real seasons, ids and ranks or generating the lists outright (decided 2026-11-08).
- **One constant shift per event** (the anonymizer's first build): exact ranks and ties, but every gap between two marks stays as recorded, and a list's gap pattern is a fingerprint that can be matched back to the real list; rank-preserving jitter breaks that (decided 2026-11-08).

- **Pure generation** (a seeded generator writes pages from scratch): fully synthetic, but the parser tests would test the parser against the generator's own idea of Athletic.net, losing the real pages' quirks (the encodings, odd markup, ties) that the fixtures exist to catch.
- **Generated fixtures, anonymized examples**: two tools, and the fixtures still lose real-page fidelity.
- **Per-mark random jitter**, unbounded: breaks true ranks and ties, so examples stop showing what the tool really does; bounded by the gaps, per distinct value, it is what the map does.
- **Real marks with mid-list subjects**: State leaders are on every rankmap, exact and searchable.

## Examples

Four examples, published on GitHub Pages from a landing page:

1. A California rankmap (Sections map, district outlines, CIF State Meet places).
2. A Maine rankmap (Class cards, MPA automatic and provisional standards).
3. A rankmap for a State without an Area file (the US map and the State list only).
4. A seed sheet: a meet index page, relays and an open event.

Each subject is picked by a fixed rule from the recorded State list (such as the 6th-fastest 11th grader), never by hand.

- **The seed sheet is replayed with a pinned clock.**
  Seed sheets are only for upcoming meets, and `ath seed` keeps refusing a past `--meet-date` for real use.
  The examples build replays the recorded data through a fixture data source with today pinned to the day before the meet, as the golden tests already do, and the page says it is simulated as of that date.
- **Links stay but go to one demo page**, which explains that links work only on pages built from real data and that real use needs a paid Athletic.net subscription.
  A test fails the examples build if any `athletic.net` URL is in its output.
- **Examples are indexable and say what they are.**
  Unlike real pages (which keep the `robots` noindex tag and never name Athletic.net in visible text), each example carries a footer: names, schools, meets, seasons and marks are invented or shifted and ranks are realistic, not real; built with `ath` from Athletic.net data; not affiliated with or endorsed by Athletic.net; real use needs a paid Athletic.net subscription.

## Consequences

- The anonymizer replaces the generated test data planned before it.
  Re-recording after Athletic.net changes a page refreshes fixtures, goldens and examples together.
- The anonymizer and its tests are public; the recordings, the key and the maps (`--map-out`) are not.
  Every anonymizer run checks that its output holds no name or id from its input and that every mark changed, and fails otherwise.
  An exact "no input mark anywhere in the output" check is impossible on a long list (marks a hundredth apart: any shift lands on some other athlete's mark), so marks are checked one by one.
- The rule-picked real-athlete live suite reads recordings from `local/` and never commits them.
