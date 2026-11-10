# State Area files

An Area is the level directly below a State in Athletic.net's hierarchy (`CONTEXT.md`).
Which level that is, what people call it, and how a map draws it differ from State to State, so each State that has Areas gets one file: `areas/<ST>.json`.
A State without a file has no Areas: `ath areas` lists only the State's own list, and rankmap shows no second view.
Today there are two files: `areas/CA.json` for California's CIF Sections, which are places and get a county map, and `areas/ME.json` for Maine's MPA Classes, which are school sizes and get cards (see "Areas that are not places").

## Who reads it

- `ath` embeds every `areas/*.json` at build time (`build.rs`), so adding a State is adding a file and rebuilding.
  It reads `state`, `level` and each Area's `name` and `abbr`: which divisions in Athletic.net's season tree are the State's Areas, and the abbreviations `--in` and `--area` also take (`CCS`).
  The `area_files_are_valid` test checks every file: named for its State, a known State code, a level, Areas, and no name or abbreviation used twice (an abbreviation that is the name itself, Maine's `A`, counts once).
- rankmap reads the rest: `label` and `labels` for what its pages call an Area (`Section`, `Sections`), `body` for whose they are (`CIF`), `note` for the line under the map or cards, and, for a State whose Areas are places, `map`, `areas[].counties`, `city` and `county` for the State map.

## Format

```json
{
  "state": "CA",
  "level": "Section",
  "label": "Section",
  "labels": "Sections",
  "body": "CIF",
  "map": "tools/maps/CA-counties.json",
  "note": "Counties are colored by the Section most of their schools play in; ...",
  "areas": [
    {"name": "Central Coast", "abbr": "CCS", "counties": ["Monterey", "San Benito", "San Mateo", "Santa Clara", "Santa Cruz"]},
    {"name": "Oakland", "abbr": "OS", "counties": [], "city": "Oakland", "county": "Alameda", "note": "Oakland Unified public schools"}
  ],
  "split_counties": {"Alameda": "Oakland Unified public schools are the Oakland Section; the rest ... North Coast"}
}
```

| Key | Meaning |
| --- | --- |
| `state` | Two-letter code; the file is `areas/<state>.json`. |
| `level` | The Area level's `divType` in Athletic.net's tree (`GetNavInfo`): California `Section`. Athletic.net uses other words elsewhere (New York `Association`, Texas `League`), and in some States that level is not what people mean by an Area, so each State is added deliberately. |
| `label`, `labels` | What pages call one Area and several. |
| `body` | Whose Areas they are, for page text (`CIF`); optional. |
| `areas[].name` | Athletic.net's name for the Area, which decides which list it is. |
| `areas[].abbr` | The abbreviation people use; `ath rank --in CCS` and `ath list --area CCS` take it. |
| `areas[].counties` | The counties the map colors for this Area: each county goes to the Area most of its schools play in. |
| `areas[].city`, `areas[].county` | For an Area that is one city's schools (Los Angeles Unified, Oakland Unified): no counties of its own, drawn as a chip pointing into its county. |
| `areas[].note` | Who is in it, when that is not obvious. |
| `areas[].display` | The name pages show, when Athletic.net's is ambiguous: its "Los Angeles" is the Los Angeles City Section (Los Angeles Unified), not the county, so it shows as "Los Angeles City". Optional; the map labels every Area by this or its name. |
| `map` | The State's county shapes (`tools/maps/<ST>-counties.json`, made with `tools/maps/extract_counties.py`). Only for Areas that are places; without it the Areas get cards, and `counties`, `city`, `county`, `split_counties` and `outlines` are not allowed. |
| `note` | One line the page shows under the map or the cards: how the Areas are drawn or what they are. |
| `split_counties` | Counties whose schools play in more than one Area, with how they split: the record behind the `counties` choices. |
| `outlines` | School district outlines for Areas that are one district's schools (`tools/maps/<ST>-districts.json`, made with `tools/maps/extract_districts.py` from a Census school district file), drawn over their county. Optional. |
| `state_meet` | The State championship's qualifying rules, when the page should show them, in one of two shapes. By places (California): `name`, `short`, `places` (each Area's entries per event, by Athletic.net name), `places_note`, `at_large_rule`, `at_large` (season, then `f`/`m`, then Athletic.net event code, then the standard as published: `55.60`, `2:11.70`, `18-02.75`), `sources` and `checked`. By standards (Maine, recognized by its `standards` key): `name`, `short`, `field` (the field size filled from the provisional standard), `rule`, `new_england`, `standards` (season, then Area name, then `f`/`m`, then event code, then `[automatic, provisional]`), `notes`, `sources` and `checked`. Optional. |

## How `ath` finds a State's Areas

Athletic.net's season tree (`GetNavInfo`'s `regions`) lists the level below some States (California's Sections) but not others.
Maine's Classes are only in the `tree` part of the answer to a Maine list's token: the subtree of that list's own division.
So when a State with a file has no Areas in the national `regions`, `ath` reads the State's own subtree through its State list (two requests, cached forever under their own key, never in place of the national tree) and takes the divisions right below the State.
Each must be of the file's `level`; anything else is an error naming what Athletic.net has, because it means the State's hierarchy changed.
Only the States a command needs are asked for: `ath areas ME`, a Maine Profile's Area, `ath list --area` without `--state` only when no State's Areas in the national tree have the name, and `ath rank --in areas` for a Maine Athlete.
Only the national tree is completed (a partial one may just not show the Areas), a subtree with nothing below the State yet is not kept (so Classes added later in a season are found), and a run reads each State's subtree at most once.
In `ath rank`, a State whose subtree cannot be read is a note, and every other list is still placed.

## Areas that are not places

Maine's Classes group schools by enrollment (Class A the largest), not by geography, so a county map would say nothing true.
Its file has no `map`: rankmap shows the Classes as cards, one per Class with the Athlete's rank on its rank color and their own Class marked, then the same table as for Sections, with no headline numbers or legend (three cards say it all).
Pages call such an Area by its label and Athletic.net's name together ("Class A"), since Athletic.net's names are just the letters.
`ath rank` reports the Athlete's own Area as `home_area` whenever `--in` has `home` or `areas`, which is how the page knows which card is theirs.

## The map is an illustration

Membership in an Area is by school, decided by Athletic.net's lists; `ath` never infers it from geography.
The county map only colors each county by the Area most of its schools play in, so split counties (Los Angeles, Alameda, San Francisco, Santa Barbara, Kern, Solano and a few small ones in California) and city Areas are approximations, recorded in `split_counties`.
California's county assignments were checked for 2025-26 against each Section's league lists and news of recent moves (Santa Maria Valley, Lompoc, Cabrillo and Santa Ynez to Central; the High Desert League to Central); the official CIF section pages were not reachable at the time.

## The State Meet

California's CIF State Meet takes a fixed number of entries per individual event from each Section (the same for every event and for girls and boys: Southern 6; Central, Central Coast, Los Angeles City, North Coast, Sac-Joaquin and San Diego 3; Northern, Oakland and San Francisco 1), chosen by place at the Section's own qualifying meet.
A Section also enters anyone whose mark at that meet met the at-large standard, set each year from the 9th-place qualifying marks of the three before (fully automatic timing for running events).
rankmap's Sections view sets each Section's places against the athlete's place on its season list in the grade group picked (All HS is the meet as it was; 9th-11th, the season's field without its seniors, is a look at next season), and gives the season's at-large standard: an estimate, as the real qualifiers are decided at the Section meets.
The Markdown report sets the places against the All HS place and says whether the season best meets the standard.
The standards are kept per season (2025, 2026 and 2027 so far); a season or event without one leaves the at-large line out.
Event codes are Athletic.net's: flat races (`400m`) and `LJ` and `Shot` are checked against `ath`; the hurdles (`100mH`, `110mH`, `300mH`), `HJ`, `TJ` and `PV` follow Athletic.net's naming and are not yet checked against a live list; discus and the relays are left out until they are.

## Maine's State Meets

Each MPA Class holds its own State Meet, entered by standards rather than places: everyone whose season mark meets the Class's automatic standard is in, and when fewer than 32 do, the field is filled up to 32 from the provisional standard (MPA Outdoor Track Bulletin 2025-26).
Marks count from regular-season meets and the league championship; Maine then sends 6 per event to the New England Championship from the Class meet finalists.
rankmap shows the State Meet of the athlete's own Class only (their school's, `ath rank`'s `home_area`), since another Class's standards say nothing about them: its automatic and provisional standards for the season, which the season best meets, and the seed the athlete's place on the Class list suggests in the grade group picked.
The Markdown report gives the seed against all high schoolers.
The standards are kept for 2025 (from MileSplit's transcription; the 2025 bulletin could not be fetched) and 2026 (the bulletin, updated 04/07/26); a season without them (2027 so far) shows no panel.
The official 2026 Class A meet program printed 2025's standards for some events; the file keeps the bulletin's and says so in `notes`.
Discus, javelin and the race walk are left out until their Athletic.net event codes are checked.

## Adding a State

1. Find the Area level: `ath areas <ST>` on a State without a file says it has none; Athletic.net's season tree (`GetNavInfo`, see `docs/agents/systems/athletic-net.md`) shows the `divType` below the State.
2. Write `areas/<ST>.json` with `state`, `level` and the Areas' names (exactly as Athletic.net has them) and abbreviations, then `cargo test` (`area_files_are_valid`).
   That is all `ath` needs; rankmap also needs `label`, `labels` and `note`, and its tests (`test_every_areas_file_is_complete`) fail until they are there.
   If the Areas are not places (school sizes, like Maine's Classes), stop here: rankmap shows them as cards.
3. For rankmap's map: `python3 -I tools/maps/extract_counties.py <counties-albers-10m.json> <State FIPS> <ST>` with us-atlas@3.0.1's `counties-albers-10m.json`, then fill in `map` and each Area's `counties` (every county in exactly one Area), and run `python3 -I tools/maps/label_spots.py` so the map knows where each Area's rank goes.
4. For an Area that is one school district's schools, outline it: `python3 -I tools/maps/extract_districts.py <Census unsd shapefile> <ST> "<district name>=<Area name>"`, with the Census Bureau's cartographic boundary file of the State's unified school districts, and set `outlines`.
5. For the State championship: `state_meet`, from the State association's handbook, by places or by standards (see the table).
   California's (CIF, 2025 to 2027) came from the 2026 State Track and Field Handbook, the 2026-27 Blue Pages and CIF's at-large standards page.
