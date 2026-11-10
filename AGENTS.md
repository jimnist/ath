# AGENTS.md

`ath` is a Rust command-line app that generates meet **seed sheets** from
Athletic.net: CSV in (one row per event to build) → CSV out (one ranked seed
list per event). See `docs/HANDOFF.md` for the full build spec.

Before making changes, read the relevant docs under `docs/agents/`.

## Agent docs

Start here:

- `docs/agents/README.md` - what lives where
- `docs/agents/systems.md` - external systems this app talks to
- `docs/agents/skills.md` - installed skills and how to add more
- `.claude/skills/ath/SKILL.md` - fetching Athletic.net data (Profiles, marks, Rankings, meets, seed sheets): always through the `ath` CLI

For the one external system, read:

- `docs/agents/systems/athletic-net.md` - the entries pages + bio API, and the
  hard-won encodings (points at `docs/HANDOFF.md` as the source of truth)

## Working rules

- `docs/HANDOFF.md` is the authoritative spec. When behavior and the doc
  disagree, reconcile them - don't silently diverge.
- Never hardcode Athletic.net event IDs; resolve events by name (see the
  athletic-net system doc for why).
- Sort and sum on the numeric `SortInt`, never by parsing the display string.
- Prefer small, reviewable changes. Keep the CLI surface (CSV in / CSV out)
  stable unless a change is deliberate and documented.
- Be a good API citizen: bounded concurrency, delays, and retry-with-backoff
  against the bio API's rate limiting.
- Anything personal goes in `local/` (gitignored; see the README): report configs, rosters, collected data, output, Recordings, and session handoff notes in `local/handoffs/`.
  Never commit it; `tests/local.rs` fails if git tracks anything `.gitignore` ignores.
- The repo is public: no ids, branch names or links of the earlier private issue tracker, links to the private repos, home folder paths, claude.ai links, email addresses or em dashes in code, tests or docs (say why, not which ticket).
  `tests/public_tree.rs` fails on them; `tools/anonymize/real_values.py` guards real people's names and ids.
- Worktrees share the main checkout's `local/`, which the reports find through git (`$ATH_LOCAL` overrides).
  In a worktree under `.claude/worktrees/`, read and write configs, data and handoffs in the main checkout's `local/`, never a `local/` of the worktree's own.

## Agent skills

### Issue tracker

Issues live in **GitHub Issues** on `jimnist/ath`, created and updated via `gh api` (the REST API).
PRs are not a triage surface.
See `docs/agents/issue-tracker.md`.

### Triage labels

Five canonical roles, default names, one at a time per issue, plus a type label (`bug`, `feature`, `improvement`), as GitHub labels on `jimnist/ath`.
See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: `CONTEXT.md` + `docs/adr/` at the repo root.
See `docs/agents/domain.md`.
