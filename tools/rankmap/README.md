# rankmap

Where an athlete's season-best mark would rank on every US state's Athletic.net high school list, as a map page.
The athlete's home State is outlined on the map, keeping its rank color, and tagged "home" in the state-by-state table.
Its data comes from `ath rank --in all --ahead all`, through the browser relay.

Full documentation, screenshots and the data format: [`docs/rankmap.md`](../../docs/rankmap.md).

```bash
cargo build --release                                                            # rankmap runs this checkout's ath
mkdir -p local/rankmap                                                           # your configs, data and pages (gitignored)
cp tools/rankmap/rankmap.config.example.json local/rankmap/rankmap.config.json   # athlete_id, event, graduation_year, output_dir
python3 tools/rankmap/rankmap.py                                                 # with an Athletic.net tab running the relay
python3 -m unittest discover -s tools/rankmap                                    # tests (cargo test runs them too)
```

Collected seasons go in `data/` and pages in `out/`, both beside the config (`local/rankmap/`); see the README's [`local/` section](../../README.md#your-own-configs-and-data-local).

To restyle the page, add `"theme": "<a theme file>"` to the config (start from [`tools/theme.example.json`](../theme.example.json)).
A theme with its own display font needs that font's `metrics`, which [`tools/maps/measure_font.html`](../maps/measure_font.html) measures in a browser.
See [Restyling](../../docs/rankmap.md#restyling).
