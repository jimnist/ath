---
name: rankmap
description: Build or update a rankmap page - where an athlete's season-best mark would rank on every state's Athletic.net high school list - collecting new seasons with `ath rank` through the browser relay (a Claude in Chrome tab), building the page, and asking whether to republish it when the config has a shared page (`artifact_url`). Use when the user asks to add, refresh, or rebuild a season of state rankings or the rankings map.
---

# rankmap

Tool: `tools/rankmap/`. Full docs: `docs/rankmap.md` (read it first).
Collecting needs a browser signed in to a paid Athletic.net subscription: without one, State lists stop at 25 rows and `ath` refuses to rank against them.

## 0. Find or write the config

Configs live in `local/rankmap/` of the main checkout (gitignored, see the README's `local/` section): `rankmap.config.json` by default, others as `rankmap.<athlete_id>-<event>.config.json`, one per athlete and event.
Pass a config by its name, `--config rankmap.<athlete_id>-<event>` (no folder): rankmap looks names up in the main checkout's `local/rankmap/` even from a git worktree, and prints the file it uses (`Config: ...`).
Never pass a relative path such as `local/rankmap/...`: in a worktree it points at the worktree's own empty `local/`.
To see which configs exist, list `local/rankmap/*.config.json` in the main checkout: the first `worktree` line of `git worktree list --porcelain` (or `$ATH_LOCAL/rankmap/` when that is set).
If there is none for the athlete and event, ask the user for the athlete's Athletic.net id (or profile URL), event and graduation year, and write it from `tools/rankmap/rankmap.config.example.json` into the main checkout's `local/rankmap/`.
Pages go to the config's `output_dir` (required, relative to the config's folder, e.g. `out` for `local/rankmap/out/`), and collected seasons to its `data_dir` (`data` beside the config when it has none).

rankmap gets its data from `ath rank --in all,areas --ahead all` (every State, and every Area of the home State when `areas/<ST>.json` has it, such as California's CIF Sections), which fetches through the browser relay.
Run `cargo build --release` first (a no-op when the build is current), as the `ath` skill says.

## 1. Start the browser relay (only when a season needs collecting)

Skip this for `--no-fetch`.

Follow section 1, "Connect to Athletic.net", of the `ath` skill (`.claude/skills/ath/SKILL.md`): reuse any tab with `window.__athRelay` set (`waiting`, `serving` or `standby`), or install the relay in a new signed-in Athletic.net tab, before running rankmap.
The browser must be signed in to a paid Athletic.net subscription.
Leave the tab open until rankmap is done.

## 2. Run it

Run `python3 tools/rankmap/rankmap.py --config rankmap.<athlete_id>-<event>` (or no `--config` for `rankmap.config.json`) in the background (no terminal, so every question takes its default):

- seasons in progress are re-collected, finished seasons without data are collected, finished seasons with data are left alone;
- a season saved by the retired browser collector (`fetch.js`) is collected again through `ath`;
- add `--refresh <season>` only when the user asked to refresh a finished season (they stay frozen otherwise);
- `--no-fetch` just rebuilds.

Each season is one `ath rank` run: about 200 lists, around three minutes the first time, instant from `ath`'s cache after that.
Do not raise `ath`'s `--concurrency` or lower its `--delay-ms` to go faster.
`ath`'s warnings print as it goes.
A season is saved only when every list was read; otherwise rankmap says why and keeps what it had (re-run it to retry: lists already read come from the cache).
If `ath` says a list is capped, the browser session has no paid subscription; tell the user rather than retrying.
If `ath` says the athlete has no mark yet, the season has not started for them; tell the user rather than guessing.

## 3. Check and open it

`rankmap.py` prints one line per season and grade group (#1 count, top 5 count, toughest state) and the output path.
Spot-check one number against the live Athletic.net list.
Open the page: serve the output folder with `python3 -m http.server` and look at the US map, and the Areas view (`#areas-<season>-<grades>`) when the athlete's home State has Areas: a county map for a State whose Area file has a `map` (California's Sections), cards otherwise (Maine's Classes).
Give the user the output path.

## 4. Share it (optional)

A page built from real data aggregates Athletic.net's lists about real athletes: it carries a `robots` noindex tag and is shared privately (a private Claude Artifact, or a host behind a login), never posted openly.

If the config has `artifact_url`, a page is already shared there, and it is now stale: republishing is what updates a shared link, pages do not refresh themselves.
Tell the user so and ask whether to republish it there; do not republish without a yes.
On a yes, publish the HTML with the Artifact tool to that `url` (read it first if this conversation has not published it).

Without `artifact_url`, share the page only when the user asks: publish it as a new private Artifact, and suggest adding its link to the config as `artifact_url`.

Collected data and output live in `local/`, which the repo ignores; there is nothing to commit for a data refresh.
If `local/` is its own git repo (`git -C local status` says so), commit the new seasons there when the user asks.
