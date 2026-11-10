# TODO / Backlog

Deferred work surfaced during design. v1 scope is in `CONTEXT.md`; locked
decisions are in `docs/adr/`.

## Access

- [x] **Browser relay transport** (replaces the dropped automatic
      `cf_clearance` harvest): a bookmarklet-started Athletic.net tab relays
      `ath`'s requests over `127.0.0.1`, as the default transport next to
      `--cookie`. See `docs/adr/0006-pluggable-transport-browser-relay-by-default.md`
      and the build order in `docs/rankmap.md`. Implemented
      (`ath relay install`, `src/relay.rs`).
- [x] Confirm the **entries JSON API** endpoint + shape on a meet with open
      entries (625323's were removed post-meet). Needed to finalize Mode A.
      Resolved 2026-11-06 on an open indoor meet: `Meet/GetMeetData`,
      `Meet/GetEventListData`, `Meet/GetEntriesData` (see
      `docs/agents/systems/athletic-net.md`).
      Implemented as Mode A (`ath seed --meet`, `ath meet`).
- [x] Confirm the `fq=` filter param on `AutoComplete/search` (scope to
      athletes vs teams/meets). Resolved: `fq=type:Athlete` (Solr filter query);
      see `CONTEXT.md`. Used by the athlete search (Mode B).

## Scope (post-v1 phases)

- [ ] **Field events** (shot put): add SortInt→inches unit handling and
      per-implement filtering.
- [x] **Relays**: multi-runner aggregation, per-meet seeding-event remapping (e.g. 4×Mile → 1600m), blank-on-incomplete.
      Done for relays of four equal legs (`ath seed --meet`, `--seed-on`).
- [ ] **Medley and shuttle relays** (SMR, DMR, shuttle hurdles): legs of different distances, so no one Seeding event.

## Output / UX

- [ ] Optional `mark_team` / `mark_team_url` column - the affiliation a seeding
      mark was actually set under (from the result's `SchoolID`), which can
      differ from current school/club (e.g. club marks at national meets).
- [ ] Multi-event batching - one combined input with a per-row `event` column
      producing many sheets per run (v1 is one `--event` per run).
- [ ] Optional `gender` roster column as an extra resolution disambiguator.

## Later

- [ ] Auth/cookie expiry UX (detect a Cloudflare challenge response mid-run and
      prompt for a fresh cookie rather than failing hard).
