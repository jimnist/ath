---
status: accepted
---

# Athletic.net access goes through a pluggable transport, browser relay by default

Cloudflare blocks plain HTTP clients (HTTP 403, reconfirmed 2026-10-25) and headless browsers, and a pasted cookie (ADR 0001) expires within minutes to hours, which cannot carry bulk work such as the ~200 rankings requests per season that rankmap needs.
So `ath`'s `Fetcher` seam gets two implementations, chosen per run: a **browser relay**, the default, where a real Athletic.net tab started from a one-time bookmarklet loops "ask `ath` for the next request, run it, post the response back" against `127.0.0.1`; and the existing **cookie** transport (`--cookie`, matching User-Agent).
All Athletic.net knowledge (URLs, paging, retries, pacing, parsing) stays in `ath`; the tab is a dumb pipe and no credential ever leaves the browser.

## Considered options

- **Cookie only** (ADR 0001 as is): every run starts with copying a cookie from DevTools, and an expiry mid-collection loses the run.
- **Automatic `cf_clearance` harvest** (the old `docs/TODO.md` item, now dropped): reading a browser's cookie store or driving a browser through the challenge is the most fragile option and the closest to defeating the bot protection, and it handles a credential.
- **Headless browser per run**: blocked by Cloudflare outright.

## Consequences

- `Fetcher` gains POST (`tfRankings/GetRankings` is a POST).
- The relay loop keeps polling `127.0.0.1` and quietly retries when no `ath` is running, so one bookmarklet click per browser session serves every later command, by a person or an agent (Claude in Chrome can start the same loop). Chrome asks once to allow local network access.
- The cache stores raw responses keyed by request, with freshness by data kind instead of ADR 0004's single 12-hour TTL:

  | Data | Freshness |
  | --- | --- |
  | Area tree for a season | forever |
  | Rankings list, finished season | forever; refresh only on request |
  | Rankings list, season in progress | 12 hours |
  | Profile | 12 hours in season; forever after the person's last season |
  | Meet entries | about 1 hour |

  `--refresh` and `--no-cache` keep their meaning. Backing off and asking for help on persistent challenges (ADR 0004) still applies to both transports.
