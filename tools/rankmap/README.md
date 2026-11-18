# rankmap

Where an athlete's season-best mark would rank on every US state's Athletic.net high school list, as a map page.
The athlete's home State is outlined on the map, keeping its rank color, and tagged "home" in the state-by-state table.
Their own Area (Section) is marked the same way on the Areas map and in its table; a Class card keeps its own tag.
The smallest states' ranks are in chips beside the map, listed under it at phone width so they stay readable.
A Colleges section ranks the athlete's best mark against college teams' season bests, this college season and last (the config's `colleges`, UC Berkeley, Colorado and Virginia by default).
Its data comes from `ath rank --in all,areas --ahead all` and `ath colleges`, through the browser relay.
College marks copied by hand from another results source can count too: `college_marks.py add` reads a pasted season list into your private `local/rankmap/college-marks/`, and `college_marks.py list` shows which lists there are (nothing is ever fetched from that source).

Full documentation, screenshots and the data format: [`docs/rankmap.md`](../../docs/rankmap.md).

```bash
cargo build --release                                                            # rankmap runs this checkout's ath
mkdir -p local/rankmap                                                           # your configs, data and pages (gitignored)
cp tools/rankmap/rankmap.config.example.json local/rankmap/rankmap.config.json   # athlete_id, event, graduation_year, output_dir
python3 tools/rankmap/rankmap.py                                                 # with an Athletic.net tab running the relay
pbpaste | python3 tools/rankmap/college_marks.py add --config <name> --team <id> --season 2026 --source "..."  # preview a pasted list; --yes writes it
python3 -m unittest discover -s tools/rankmap                                    # tests (cargo test runs them too)
```

Collected seasons go in `data/` and pages in `out/`, both beside the config (`local/rankmap/`); see the README's [`local/` section](../../README.md#your-own-configs-and-data-local).

To restyle the page, add `"theme": "<a theme file>"` to the config (start from [`tools/theme.example.json`](../theme.example.json)).
A theme with its own display font needs that font's `metrics`, which [`tools/maps/measure_font.html`](../maps/measure_font.html) measures in a browser.
See [Restyling](../../docs/rankmap.md#restyling).
