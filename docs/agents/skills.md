# Skills

## Built with Matt Pocock's skills

ath was built with Matt Pocock's agent skills, [Skills For Real Engineers](https://github.com/mattpocock/skills).
These are the ones used:

| Skill | Purpose |
|---|---|
| `grill-with-docs` | A relentless interview that sharpens a plan or design **and** writes docs (ADRs and the glossary) as you go. |
| `grilling` | The interview engine `grill-with-docs` drives: one hard question at a time until there is shared understanding. |
| `domain-modeling` | The doc-writing engine: builds the glossary (`CONTEXT.md`) and ADRs (`docs/adr/`). |
| `setup-matt-pocock-skills` | One-time setup: picks the issue tracker, triage labels, and where docs live. Already done for this repo (`issue-tracker.md`, `triage-labels.md`, `domain.md`). |
| `to-spec` | Synthesizes the current conversation into a spec and publishes it to the issue tracker (`ready-for-agent`, see `issue-tracker.md`). |
| `to-tickets` | Breaks a spec or plan into tracer-bullet, vertical-slice tickets with blocking edges, published to the issue tracker. |
| `implement` | Builds a ticket end to end. |

They are not part of this repo.
Anyone who wants them can install them for Claude Code with the [`skills`](https://github.com/vercel-labs/skills) CLI (needs Node.js), from the repo root:

```bash
npx skills@latest add mattpocock/skills -a claude-code \
  --skill grill-with-docs --skill grilling --skill domain-modeling \
  --skill setup-matt-pocock-skills --skill to-spec --skill to-tickets --skill implement
```

This writes them into `.agents/skills/` and links each one into `.claude/skills/`.
`.gitignore` ignores what the install writes (the links by name), so it never shows up as a change.

All are user-invoked (`/skill-name`), and the usual flow is:

```
/grill-with-docs   (align on the design → CONTEXT.md + docs/adr/)
        │
        ▼
/to-spec           (synthesize the spec → issue tracker)
        │
        ▼
/to-tickets        (slice into tracer-bullet tickets → issue tracker)
        │
        ▼
/implement         (build ticket by ticket)
```

## The repo's own skills

Repo-specific skills live directly in `.claude/skills/` as real, tracked directories.

| Skill | Purpose |
|---|---|
| `ath` | Fetch Athletic.net data through the `ath` CLI: Profiles and marks, Rankings lists and where a mark ranks, a State's Areas, a meet's events and entries, and seed sheets. Covers connecting the browser relay, what a paid subscription unlocks, privacy, and what each error means. |
| `rankmap` | Build or update a rankmap page (where an athlete's season best would rank in every state), collecting seasons with `ath rank` through the browser relay in a Claude in Chrome tab. See `docs/rankmap.md`. |

To add another, create `.claude/skills/<name>/SKILL.md`; it is tracked like any other file.
