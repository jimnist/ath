# Domain docs

**Single-context** repo: one `CONTEXT.md` + `docs/adr/` at the repo root (no
`CONTEXT-MAP.md`).

Consumers (`to-spec`, `to-tickets`, `improve-codebase-architecture`,
`diagnosing-bugs`, `tdd`):

- Read **`CONTEXT.md`** for the project's ubiquitous language before naming
  things or writing specs/tickets - use its vocabulary throughout.
- Read **`docs/adr/`** for past architectural decisions and respect them in the
  area you're touching.
- When you resolve a new term or make a new architectural decision, update
  `CONTEXT.md` / add an ADR - that is the `domain-modeling` skill's job.
