---
name: college-marks
description: Help the user enter a college team's season list for an event, copied by hand from another results source, so rankmap's Colleges section counts it - ask which college and season, take the pasted list (and a source note the first time), preview the parsed rows with `tools/rankmap/college_marks.py`, write them only on the user's OK, then offer to rebuild the page. Use when the user wants to add, replace or check hand-entered college marks. Never fetches anything from the web.
---

# college-marks

Tool: `tools/rankmap/college_marks.py`. Docs: "Hand-entered college marks" in `docs/rankmap.md` (read it first).

## Never fetch the list

The marks are only what the user copies in their own browser and pastes to you.
Never open, fetch, search or scrape the other results source, in a browser tool or otherwise, even when the user gives a link: the link is a note for their private file, nothing to visit.
Never name a particular results source in anything you commit or publish; what the user uses is theirs to note in their private file.

## Steps

1. Find the config as the `rankmap` skill does (by its name, `--config rankmap.<athlete_id>-<event>`, looked up in the main checkout's `local/rankmap/`).
   To see what is entered already, run `python3 tools/rankmap/college_marks.py list --config <name>`.
2. Ask which college (its name in the config's `colleges`, or its Athletic.net team id) and which college season (year).
   The college has to be one of the config's `colleges`; to add one, add it to the config first.
3. Ask the user to copy the team's season list for the athlete's event (every athlete's season best) and paste it to you.
   The first time for that college and season, also ask for a short note, in their words, of where it was copied from, and optionally the page's link.
4. Save the paste exactly as given to a temporary file, then preview it (nothing is written without `--yes`):

   ```bash
   python3 tools/rankmap/college_marks.py add --config <name> --team <id or name> --season <year> --file <paste file> --source "<their note>" [--url <link>]
   ```

   `--gender` and `--event` are only for a list of another gender or event than the athlete's.
5. Show the user the rows it printed (name, class year, time, meet, date) and ask whether they are right.
   If the paste was refused, say why (the message names the line) and ask for a fixed paste; never edit their marks yourself, and never guess at a row it could not read.
6. On the user's OK, run the same command with `--yes`.
   Entering the same college, season, gender and event again replaces the earlier list.
7. Offer to rebuild the page: `python3 tools/rankmap/rankmap.py --config <name> --no-fetch` counts the new list without collecting anything.
   If the config has `artifact_url`, ask whether to republish it, as the `rankmap` skill does.

The private file lands in the main checkout's `local/rankmap/college-marks/` (never committed); do not copy its contents, the source note or the link anywhere public.
