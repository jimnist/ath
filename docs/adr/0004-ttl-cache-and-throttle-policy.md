---
status: superseded in part by ADR-0006 (freshness is now per data kind; the backoff and stop-and-ask behavior still applies)
---

# Bio fetches use a TTL cache and back off on Cloudflare challenges

Bio JSON is cached on disk keyed by `athleteId` with a **~12h TTL** (auto-refetch
when stale; `--refresh` forces, `--no-cache` bypasses). Fetching uses bounded
concurrency (~5), a small inter-request delay, and retry-with-backoff when a
response comes back as HTML (a Cloudflare "Just a moment…" challenge) instead of
JSON; if challenges persist, `ath` stops and asks for a fresh `--cookie` rather
than hammering.

## Consequences

- Athletes recur across events, so caching avoids refetching and minimizes hits
  on the Cloudflare-gated, rate-limited API.
- The TTL keeps seeds honest once seeding *live* meets (marks change daily);
  chosen over cache-forever (silently stale in-season) and no-cache (slowest,
  hammers the API hardest).
- Detecting the HTML-challenge response is the same signal the handoff saw as
  "~40 calls → HTML not JSON"; it drives both backoff and the cookie-refresh
  prompt.
