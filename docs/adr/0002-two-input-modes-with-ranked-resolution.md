# Two input modes; Mode B resolves names and ranks ambiguous matches

`ath` learns who to seed two ways: **Mode A** fetches a meet's accepted entrants
from Athletic.net; **Mode B** takes a user-supplied roster CSV (`name` required;
optional `school`, `city`, `state`) and resolves each row to an Athletic.net
athlete via `AutoComplete/search`. When a row does not resolve to a single
confident match, `ath` returns **ranked candidate matches** (using the optional
locators to score) rather than silently picking one - a wrong silent pick would
seed the wrong person's marks.

## Consequences

- Mode B needs an athlete-resolution/scoring component the handoff never
  described; the search endpoint conveniently returns name + school + city/state
  + athleteId, which are exactly the locators to score against.
- The output must be able to represent "unresolved / needs review" rows, not
  just clean seeds.
