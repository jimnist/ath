---
status: accepted
---

# `ath` owns all Athletic.net data; reports render in Python against its versioned JSON

All Athletic.net knowledge lives in one place, so `ath` is the only thing that talks to Athletic.net: it answers data questions through noun commands (`ath profile`, `ath areas`, `ath list`, `ath rank`, `ath seed`) and never renders pages.
Reports (rankmap, the Area ranking report, the seed sheet page) are Python tools under `tools/` that call `ath` and render standalone HTML plus CSV; they never call Athletic.net themselves.
The contract between them is `ath`'s JSON output, versioned with a `schema` field and pinned by `ath`'s fixture tests; reports adapt to it.

## Considered options

- **`ath` exposes only low-level fetches and reports keep the collection logic**: splits Athletic.net knowledge across Rust and Python, which is what this decision exists to stop.
- **`ath` renders the reports too**: one binary, but it rewrites a working Python renderer, and HTML templating in Rust is costlier to iterate on.

## Consequences

- The command surface changes deliberately: the bare `ath --roster ... --event ...` form is removed in favor of `ath seed` (no alias).
- Every command prints JSON when stdout is not a terminal and a table when it is (`--json` / `--table` force either), so agents and scripts get a stable format for ad-hoc queries.
- Every result carries the Athletic.net URL for the same view; when Athletic.net already has a straightforward report, reports link to it instead of rebuilding it.
- rankmap's browser collector (`tools/rankmap/fetch.js`) retires once `ath rank` reproduces its numbers; shared page pieces move into one Python module the reports share.
