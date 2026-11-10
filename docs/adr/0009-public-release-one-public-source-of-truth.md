---
status: accepted
---

# The public repo becomes the only source of truth, starting from a fresh snapshot

`ath` went public as `jimnist/ath`, created from one squashed snapshot of a scrubbed tree on 2026-11-10.
From then on the public repo is the only source of truth: every change, issue and pull request happens there.
The private repo was renamed `jimnist/ath-archive` and archived; it keeps the full history for its owner alone.
Personal things (report configs, collected seasons, raw recordings, handoffs) live in a private repo, `jimnist/ath-local`, checked out at `local/` inside the public clone, which the public repo ignores.

## Considered options

- **The private repo stays primary and exports snapshots**: a private place for real-name fixtures, but every export is another scrub and audit, and outside pull requests are ported back by hand.
- **Make this repo public after rewriting its history** (`git filter-repo`): keeps the commits, but the history holds real people's profiles and rankings pages in 65 fixture and golden-page paths, and missing one is permanent.
- **Filtered history in a new repo**: every old commit needs auditing, old commits no longer build once their fixtures are swapped, and their messages cite a private issue tracker.
- **A few curated commits telling the story**: invented history that costs effort and adds no provenance.

The design reasoning that history would carry already lives in the tree: these ADRs, `CONTEXT.md` and `docs/HANDOFF.md`.

## Consequences

- Nothing real may be in the tree.
  The committed fixtures, golden pages and examples come from the anonymizer (ADR 0010), and the tree was audited for names, athlete and team ids, home paths and issue-tracker references before the snapshot.
- Issues live in GitHub Issues on the public repo.
  Earlier work was tracked privately; references to it in code, tests and docs were reworded or dropped before the snapshot, and its open issues moved to GitHub with the triage labels.
- GitHub's redirect from the old name ended once the new repo took it, so an old link to a private repo's pull request or issue opens the new repo's of the same number; the private tracker notes where the archive is.
- The license is MIT.
  Third-party data (the county and district outlines under `tools/maps`) is credited with its own terms.
- `AGENTS.md`, `docs/agents/` and the rankmap skill ship, rewritten for GitHub Issues; the rankmap skill holds no personal links, and republishes a config's own shared page (`artifact_url`) only after asking.
  Third-party agent skills are not vendored: `docs/agents/skills.md` credits them and says how to install them.
- New States come by pull request: an "Add my State" issue template, a pull request checklist (tests pass, `label_spots.py` re-run for a map, every `state_meet` number cites its handbook or page), and `docs/areas.md` as the contributor guide.
  CI runs `cargo test`, clippy, fmt and the Python tests on 3.9 and current for every pull request.
- The `local/` pattern is documented, so anyone can keep their own configs and data the same way.
- Before the snapshot, the report pages' colors, fonts and inks moved into one token set in `tools/report.py` with no visual change, so a fork restyles in one place; customizing the theme from config comes after.
