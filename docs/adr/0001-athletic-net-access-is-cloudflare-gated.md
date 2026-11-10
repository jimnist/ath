---
status: superseded in part by ADR-0006 (the cookie is now one of two transports; the browser relay is the default, and the automatic cookie harvest is dropped)
---

# Athletic.net access is Cloudflare-gated; v1 uses a supplied cookie

Athletic.net's `api/v1` endpoints (search, athlete bio) sit behind Cloudflare
anti-bot: a plain HTTP client gets a "Just a moment…" challenge, not JSON
(confirmed 2026-07-07 for `AutoComplete/search` and `AthleteBio/GetAthleteBioData`).
A browser works only because it holds a `cf_clearance` cookie + matching
fingerprint + login. For v1 we take the cookie via a `--cookie` flag (paste the
logged-in browser's `Cookie` header + matching User-Agent) and fetch with
`reqwest` - pure HTTP, and the same path the meet was seeded by hand.

## Consequences

- `cf_clearance` is short-lived (~30 min–hours) and bound to the User-Agent + IP
  that solved the challenge, so the CLI must also send the matching UA, and the
  user re-pastes when it expires. Acceptable for a batch "build the sheets" run.
- Build the fetch layer so the **cookie source is pluggable** (manual paste now;
  auto-harvest later) - see the browser-assisted harvest item in `docs/TODO.md`.
- Rejected for v1: TLS/JA3 impersonation (`rquest`) as primary (fragile
  cat-and-mouse) and a full headless browser for every call (too heavy for
  hundreds of bio fetches).

## Paid subscription (checked 2026-11-06)

Full lists and histories need a paid Athletic.net subscription, and Athletic.net enforces that on the server, not only on the page.
Signed out, pages blur after the 5th row; signed in without a subscription, after the 25th, and an athlete's rankings are blurred.
Through the relay in an unpaid session, `GetRankings` for California 2026 girls 400m returned 25 real rows of 696, then a masked row: name `Xxxxxx Xxxxxx`, athlete and team id 0, an empty mark, but a `SortIntCalc`.

- `ath` cannot read past the paywall, so it is not a way around it.
- `ath` must recognize masked rows, never rank them, and stop with a message that the list is capped and a paid subscription is needed, instead of treating the capped list as complete.
- The README and the example pages state the paid requirement.
