# seedsheet

A meet's seed sheets as shareable pages: one standalone page per event, entrants in seed order with their Personal records, your Athletes highlighted, and a CSV download.
Given a meet id (meet mode), it seeds the meet's own entries for the events you list, or all of them, and writes an index page for the meet that links every event's page and CSV, or says why an event is not seeded.
Given a meet name and a roster per event (roster mode), it seeds the rosters.
Its data comes from `ath seed` and `ath meet`, through the browser relay.

Full documentation: [`docs/seedsheet.md`](../../docs/seedsheet.md).

```bash
cargo build --release                                                                         # seedsheet runs this checkout's ath
mkdir -p local/seedsheet                                                                      # your configs, rosters, sheets and pages (gitignored)
cp tools/seedsheet/seedsheet.meet.config.example.json local/seedsheet/seedsheet.config.json   # meet (an id), athletes, output_dir, events ("all" or a list)
python3 tools/seedsheet/seedsheet.py                                                          # with an Athletic.net tab running the relay
python3 tools/seedsheet/seedsheet.py --no-fetch                                               # rebuild the index and pages from saved sheets
python3 -m unittest discover -s tools/seedsheet                                               # tests (cargo test runs them too)
```

For roster mode, start from `seedsheet.config.example.json` instead (meet, meet_date, athletes, output_dir, events with rosters).
Rosters sit beside the config, saved sheets go in `data/` and pages in `out/` (all in `local/seedsheet/`); a meet's index and pages are in `out/<meet id>/`, one config's to a folder.
See the README's [`local/` section](../../README.md#your-own-configs-and-data-local).
