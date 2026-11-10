# Mode B resolves athletes in two passes (resolve → pin → seed)

Mode B resolves each roster row to an Athletic.net athlete without ever silently
guessing. A row with an `athlete_id` is used directly; otherwise `ath` searches
by `name` and scores candidates against the optional locators (`school`, `city`,
`state`). A **confident** match (single hit, or a top hit that matches the
locators and clearly outscores the rest) auto-resolves and its `athlete_id` is
written back so later runs skip the search. Anything not confident is left
**unresolved** and emitted with its **ranked candidates** for the user to
review, pin the right `athlete_id`, and re-run.

## Consequences

- A run can produce two artifacts: the seed sheet (confidently resolved rows)
  and an `*.unresolved.csv` (roster row + ranked candidates).
- Chosen because it is deterministic and batch-friendly; rejected interactive
  mid-run prompting (breaks unattended runs) and silent best-guess (violates
  ADR-0002).
- Resolution ("which athlete") is independent of having-a-mark ("their 2026
  best"): a resolved athlete with no qualifying mark still sorts to the bottom,
  blank.
