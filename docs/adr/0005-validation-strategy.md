# Validation: offline fixture tests + a live oracle diff; fixtures anonymized

Two layers. **Offline core tests** drive the pure logic (SortInt math, the
qualifying-mark filter, event-resolution-by-name, relay-noise exclusion by
`AthleteID`, sort/blanks-last) from recorded bio-JSON fixtures and the handoff
§8 reference values - fast, deterministic, no network/Cloudflare. **End-to-end
acceptance** runs Mode B on the 625323 running roster and diffs against the
hand-built seed sheet (gated; needs `--cookie`; real data stays local).

Committed fixtures are **anonymized**: keep every field the logic depends on
(`SortInt`, IDs, event/type codes, seasons, flags) but scrub athlete names -
so the repo never ships a roster of minors' PII, with zero loss of test
coverage.

## Consequences

- Capture 3–4 fixtures covering the tricky cases: multi-team/club marks, a DNS,
  a hand-time, relay noise, and a resolved athlete with no 2026 mark.
- The oracle diff is the real proof of correctness; the fixture tests are the
  everyday safety net.
