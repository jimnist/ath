---
status: accepted
---

# Areas are configured per State in files, and pages say each State's own word

Which level of Athletic.net's tree counts as a State's Area, what people call it, and how to draw it differ by State (California's CIF Sections, New York's sections, Texas's regions and districts; Athletic.net's own `divType` is `Section`, `Association` or `League`).
So each State with Areas is one file, `areas/<ST>.json` (`docs/areas.md`), which `ath` embeds at build time and the reports under `tools/` read directly: the level and Area names and abbreviations for `ath`, and the label, county map and notes for pages.
"Area" stays the neutral word in `ath`'s CLI, JSON and `CONTEXT.md`; pages use the file's label, so California pages say "Section".

## Considered options

- **Rename Area to Section everywhere**: right for California and New York, wrong for States whose level is a league or a district, and a deliberate CLI break (`--area`, `kind: area`).
- **Keep the configuration in Rust constants**: one place for `ath`, but the reports need the same facts plus a map, and adding a State meant a code change.

## Consequences

- rankmap places the Athlete on every Area of their home State too (`ath rank --in all,areas`) and shows them on a second map, when the home State has a file.
- A State's county map colors each county by the Area most of its schools play in. Membership is by school, so split counties and city-only Areas are approximations, recorded in the file (`split_counties`); `ath` never uses the map.
- Not every State's Areas are places: Maine's Classes are school sizes (found when Maine's Classes were added).
  A file without a `map` is such a State, with no flag to keep in step: rankmap shows its Areas as cards rather than a map, and pages call each by its label and name ("Class A").
- Athletic.net's national tree does not list every State's Area level (Maine's Classes are missing), so `ath` reads a State's own subtree for the States with a file whose Areas are missing, cached under its own key and never in place of the national tree.
- State Meets differ too: California's Sections send places, Maine's Classes admit by standards (found when Maine's State Meet standards were added).
  The file's `state_meet` shape says which (a `standards` key), and a standards meet shows only the athlete's own Area's.
- The multi-Athlete Area ranking report (`tools/arearank`, built first as a separate page) was dropped in favor of this one-Athlete view inside rankmap.
