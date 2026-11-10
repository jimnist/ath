## What and why

<!-- What this changes and why, in a few sentences. Link the issue: Closes #nn. -->

## Checklist

- [ ] `cargo fmt --check`
- [ ] `cargo clippy --all-targets -- -D warnings`
- [ ] `cargo test`
- [ ] `python3 -m unittest discover -s tools/rankmap -p 'test_*.py'` and `python3 -m unittest discover -s tools/seedsheet -p 'test_*.py'`
- [ ] Test data uses invented names: no real athletes, schools or ids in fixtures, golden pages or screenshots.
- [ ] Markdown has one sentence per line, and no em dashes (use "-").

For an Area file (`areas/<ST>.json`, see [`docs/areas.md`](https://github.com/jimnist/ath/blob/main/docs/areas.md#adding-a-state)):

- [ ] Area names match Athletic.net's exactly (`ath areas <ST>` lists them).
- [ ] If it has a `map`: `python3 -I tools/maps/label_spots.py` re-run and `tools/maps/label-spots.json` committed.
- [ ] Every `state_meet` number cites its handbook or page in this pull request.
