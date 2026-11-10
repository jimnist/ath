# Agent documentation

These files provide project-specific context for AI agents working in this
repository.

Use this directory for durable knowledge about the repository, its systems,
workflows, and conventions.

Skills do **not** live here.
The repo's own skills (`ath`, `rankmap`) are in `.claude/skills/`; Matt Pocock's skills, which ath was built with, are installed separately (see `skills.md`).
These docs describe *this* repository and the systems around it.

## Common starting points

- `systems.md` - external systems this app depends on
- `systems/` - one file per external system
- `skills.md` - the repo's skills, and how to install the third-party ones

## Source of truth

The authoritative build spec lives at `docs/HANDOFF.md`. It captures everything
learned from producing the 2026 Nike Outdoor Nationals seed sheets by hand.
These agent docs summarize and point into it - they do not replace it.
