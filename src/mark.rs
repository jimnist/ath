//! Mark parsing, `SortInt` conversions, and the qualifying-mark filter.
//!
//! All ordering and selection is done on `SortInt` (the reliable numeric key),
//! never by parsing the display string. See `CONTEXT.md` and `docs/HANDOFF.md`
//! §3–§4.

use std::collections::HashSet;

use crate::model::{Bio, ResultRow};

/// The `SortInt` base for field-event marks: `SortInt = base − inches×1000`.
const FIELD_SORTINT_BASE: i64 = 20_000_000;

/// Strip a trailing timing-quality suffix (`a` = FAT, `h` = hand-timed) from a
/// result display string.
pub fn strip_timing_suffix(result: &str) -> &str {
    let t = result.trim();
    match t.chars().last() {
        Some('a') | Some('A') | Some('h') | Some('H') => &t[..t.len() - 1],
        _ => t,
    }
}

/// True if the mark was hand-timed (display ends in `h`).
pub fn is_hand_timed(result: &str) -> bool {
    matches!(result.trim().chars().last(), Some('h') | Some('H'))
}

/// True if the display string is a real time (not `DNS`/`DNF`/`NT`/`SCR`/`DQ`/…).
/// A real running time is `(MM:)?SS(.hh)?` after stripping the timing suffix.
pub fn is_real_time(result: &str) -> bool {
    parse_running_seconds(result).is_some()
}

/// Parse a running display string to seconds, or `None` if it is not a real
/// time. Used both as the validity check and for the §8 reference-value tests.
pub fn parse_running_seconds(result: &str) -> Option<f64> {
    let body = strip_timing_suffix(result).trim();
    if body.is_empty() {
        return None;
    }
    let segs: Vec<&str> = body.split(':').collect();
    let mut total = 0.0_f64;
    for (i, seg) in segs.iter().enumerate() {
        if seg.is_empty() {
            return None;
        }
        // Only the final segment (seconds) may carry a decimal; earlier
        // segments (minutes, hours) must be whole numbers.
        let value: f64 = if i == segs.len() - 1 {
            seg.parse().ok()?
        } else {
            let n: u64 = seg.parse().ok()?;
            n as f64
        };
        total = if i == 0 { value } else { total * 60.0 + value };
    }
    Some(total)
}

/// Seconds for a running `SortInt` (milliseconds). Lower is faster.
pub fn running_seconds_from_sortint(sort_int: i64) -> f64 {
    sort_int as f64 / 1000.0
}

/// Format a running `SortInt` (ms) as a display string, e.g. `62470 → "1:02.47"`.
pub fn format_running(sort_int: i64) -> String {
    // Round to centiseconds BEFORE splitting into minutes/seconds so a boundary
    // carry propagates (e.g. 119_999 ms → "2:00.00", never "1:60.00").
    let total_cs = (sort_int.max(0) + 5) / 10;
    let minutes = total_cs / 6000;
    let seconds = (total_cs % 6000) / 100;
    let hundredths = total_cs % 100;
    if minutes > 0 {
        format!("{minutes}:{seconds:02}.{hundredths:02}")
    } else {
        format!("{seconds}.{hundredths:02}")
    }
}

/// Inches for a field-event `SortInt`. Lower `SortInt` = farther.
pub fn inches_from_sortint(sort_int: i64) -> f64 {
    (FIELD_SORTINT_BASE - sort_int) as f64 / 1000.0
}

/// Format inches as US feet-inches, e.g. `431.0 → "35-11.00"`.
pub fn format_feet_inches(inches: f64) -> String {
    let feet = (inches / 12.0).floor() as i64;
    let rem = inches - (feet as f64) * 12.0;
    format!("{feet}-{rem:05.2}")
}

/// Indoor seasons are `SeasonID` = this + year (12026 is "2026 Indoor"; a
/// December meet belongs to the next year's indoor season). Outdoor seasons
/// are the plain year. Indoor results share the outdoor `EventID`s.
pub const INDOOR_SEASON_BASE: i64 = 10_000;

/// What never counts as a Qualifying mark, worded once for every message.
pub const NEVER_COUNT: &str =
    "indoor marks, relay splits, exhibition, hand-timed marks and DNS/DNF never count";

/// True for an indoor `SeasonID` (see [`INDOOR_SEASON_BASE`]).
pub fn is_indoor_season(season_id: i64) -> bool {
    season_id >= INDOOR_SEASON_BASE
}

/// Every Qualifying mark (`CONTEXT.md`, `docs/HANDOFF.md` §4) in `event_ids`,
/// in any outdoor season: this athlete's own individual mark, outdoor (v1
/// never counts indoor marks), not a relay split, not exhibition, not
/// hand-timed, and a real time (not DNS/DNF/NT/SCR/DQ/FS).
pub fn qualifying_marks<'a, 'e>(
    bio: &'a Bio,
    event_ids: &'e HashSet<i64>,
) -> impl Iterator<Item = &'a ResultRow> + use<'a, 'e> {
    bio.results_tf
        .iter()
        .filter(|r| r.athlete_id == bio.athlete.id) // exclude relay-team results
        .filter(|r| event_ids.contains(&r.event_id))
        .filter(|r| !is_indoor_season(r.season_id))
        .filter(|r| r.event_type_id == 0) // individual; excludes relay splits (98)
        .filter(|r| !r.exhibition)
        .filter(|r| !is_hand_timed(&r.result))
        .filter(|r| is_real_time(&r.result))
}

/// The date marks are read as of, with the real today beside it: a result with
/// no `ResultDate` is only known to be on or before today, so whether it counts
/// depends on both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AsOf {
    /// The last day whose marks count.
    pub date: chrono::NaiveDate,
    /// Today, the latest an undated result can be.
    pub today: chrono::NaiveDate,
}

impl AsOf {
    /// Marks as of `today` itself.
    pub fn today(today: chrono::NaiveDate) -> Self {
        Self { date: today, today }
    }

    /// Whether a result dated `date` (None: undated) in `season_id` counts.
    /// A dated result counts on or before [`AsOf::date`]. An undated one counts
    /// when the as-of date is today or later, or when its outdoor season ended
    /// before the as-of date's year (so it cannot be after the as-of date).
    pub fn includes(self, season_id: i64, date: Option<chrono::NaiveDate>) -> bool {
        use chrono::Datelike;
        match date {
            Some(d) => d <= self.date,
            None => self.date >= self.today || season_id < i64::from(self.date.year()),
        }
    }
}

/// A person's Season best and Personal record in one event, as of a date.
#[derive(Debug, Clone, Copy)]
pub struct Bests<'a> {
    pub season_best: Option<&'a ResultRow>,
    pub personal_record: Option<&'a ResultRow>,
}

/// Season best and Personal record in `event_ids` as of `as_of`, computed from
/// the Profile's results (never from Athletic.net's `SeasonBest` /
/// `PersonalBest` flags, which only mark today's bests).
///
/// Both are the lowest `SortInt` among Qualifying marks that count as of
/// `as_of` ([`AsOf::includes`]); the Season best is limited to `season`.
pub fn bests_as_of<'a>(
    bio: &'a Bio,
    event_ids: &HashSet<i64>,
    season: i64,
    as_of: AsOf,
) -> Bests<'a> {
    Bests {
        season_best: best_of(
            marks_as_of(bio, event_ids, as_of).filter(|(r, _)| r.season_id == season),
        ),
        personal_record: personal_record_as_of(bio, event_ids, as_of),
    }
}

/// The Personal record in `event_ids` as of `as_of`: the lowest `SortInt`
/// among Qualifying marks in any outdoor season that count as of `as_of`
/// ([`AsOf::includes`]). This is the Seed mark.
pub fn personal_record_as_of<'a>(
    bio: &'a Bio,
    event_ids: &HashSet<i64>,
    as_of: AsOf,
) -> Option<&'a ResultRow> {
    best_of(marks_as_of(bio, event_ids, as_of))
}

/// The slowest Qualifying mark in `event_ids` in `season` that counts as of
/// `as_of` (the same marks a Season best is chosen from): the highest
/// `SortInt`. A rankings list ranks one of these marks, so a person's row on
/// it is no slower. `None` without such a mark.
pub fn slowest_season_mark(
    bio: &Bio,
    event_ids: &HashSet<i64>,
    season: i64,
    as_of: AsOf,
) -> Option<i64> {
    marks_as_of(bio, event_ids, as_of)
        .filter(|(r, _)| r.season_id == season)
        .map(|(r, _)| r.sort_int)
        .max()
}

/// Qualifying marks in `event_ids` that count as of `as_of`, each with its
/// `ResultDate` (parsed once, here).
fn marks_as_of<'a, 'e>(
    bio: &'a Bio,
    event_ids: &'e HashSet<i64>,
    as_of: AsOf,
) -> impl Iterator<Item = (&'a ResultRow, Option<chrono::NaiveDate>)> + use<'a, 'e> {
    qualifying_marks(bio, event_ids)
        .map(|r| (r, r.date()))
        .filter(move |(r, date)| as_of.includes(r.season_id, *date))
}

/// The best mark: lowest `SortInt`. A tie goes to the earliest dated mark,
/// since that is when it was set; an undated mark loses a tie to a dated one.
fn best_of<'a>(
    marks: impl Iterator<Item = (&'a ResultRow, Option<chrono::NaiveDate>)>,
) -> Option<&'a ResultRow> {
    marks
        .min_by_key(|&(r, date)| (r.sort_int, date.is_none(), date))
        .map(|(r, _)| r)
}

/// An event as the bio defines it, resolved from what a user typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedEvent {
    /// The bio's own spelling of the event name (`400 Meters`).
    pub name: String,
    /// Its gender-specific `IDEvent`(s).
    pub ids: HashSet<i64>,
    /// True when the bio marks it a field event (`Type == "F"`).
    pub field: bool,
}

/// Resolve a target event to its gender-specific `IDEvent`(s) by NAME, never by
/// a hardcoded id. `None` when the bio has no such event.
///
/// Accepts the Athletic.net event name (`400 Meters`, any case) or, for a flat
/// race, the `<meters>m` short form (`400m`). The bio's event definitions carry
/// no short code, so other short codes (`LJ`, `Shot`) are not accepted.
pub fn resolve_event(bio: &Bio, query: &str) -> Option<ResolvedEvent> {
    let want = event_name_for(query);
    let mut found: Option<ResolvedEvent> = None;
    for e in bio
        .events_tf
        .iter()
        .filter(|e| e.name.trim().to_ascii_lowercase() == want)
    {
        let r = found.get_or_insert_with(|| ResolvedEvent {
            name: e.name.trim().to_string(),
            ids: HashSet::new(),
            field: false,
        });
        r.ids.insert(e.id_event);
        r.field |= e
            .kind
            .as_deref()
            .is_some_and(|k| k.eq_ignore_ascii_case("F"));
    }
    found
}

/// The lowercased event name `query` stands for: `400m` -> `400 meters`.
fn event_name_for(query: &str) -> String {
    let q = query.trim().to_ascii_lowercase();
    match q.strip_suffix('m') {
        Some(meters) if !meters.is_empty() && meters.bytes().all(|b| b.is_ascii_digit()) => {
            format!("{meters} meters")
        }
        _ => q,
    }
}

/// True if a result looks like a field mark, which v1 (running events only)
/// cannot read: an implement-weight `EventTypeID` (anything but individual 0
/// or relay split 98), or a display string in a field format (`56-03.00`,
/// `54' 9"`, `17.52m`; `docs/HANDOFF.md` §3.4).
pub fn is_field_mark(r: &ResultRow) -> bool {
    let t = r.result.trim();
    !matches!(r.event_type_id, 0 | 98) || t.contains('-') || t.contains('\'') || t.ends_with('m')
}

#[cfg(test)]
mod tests {
    use super::*;

    // SortInt for running is exactly milliseconds. The first pairs are as a real
    // bio response carries them: `Result` and `SortInt` of results in the
    // anonymized Profile fixture `tests/fixtures/982389051.json` (marks moved by
    // the anonymizer, format and encoding as Athletic.net sends them). The rest
    // are invented values for the formatting rules.
    #[test]
    fn running_sortint_reference_values() {
        let fixture = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/982389051.json"
        ))
        .unwrap();
        for (result, sort_int) in [("1:00.34a", 60340), ("4:51.84a", 291_840), ("8.46a", 8457)] {
            let pair = format!(r#""Result":"{result}","SortInt":{sort_int},"#);
            assert!(fixture.contains(&pair), "the fixture has {pair}");
            let shown = result.trim_end_matches('a');
            assert_eq!(format_running(sort_int), shown);
            // The display is the SortInt to the hundredth; SortInt keeps the
            // thousandths the display rounds away (8.46a is 8457).
            let seconds = parse_running_seconds(result).unwrap();
            assert!((seconds - running_seconds_from_sortint(sort_int)).abs() <= 0.005);
        }
        assert_eq!(format_running(62310), "1:02.31");
        assert_eq!(format_running(62470), "1:02.47");
        assert!((running_seconds_from_sortint(62470) - 62.470).abs() < 1e-9);
        assert!((parse_running_seconds("1:02.47").unwrap() - 62.47).abs() < 1e-9);
        assert_eq!(format_running(125430), "2:05.43");
        // sub-minute (no leading zero on seconds)
        assert_eq!(format_running(58240), "58.24");
        assert_eq!(format_running(9580), "9.58");
        assert!((parse_running_seconds("58.24a").unwrap() - 58.24).abs() < 1e-9);
        // rounding carry must not yield "1:60.00"
        assert_eq!(format_running(119_999), "2:00.00");
        assert_eq!(format_running(59_999), "1:00.00");
    }

    /// A one-event 400m bio from `(Result, SortInt, SeasonID, ResultDate)`s.
    fn bio(results: &[(&str, i64, i64, Option<&str>)]) -> Bio {
        let results: Vec<_> = results
            .iter()
            .map(|&(result, sort_int, season, date)| {
                serde_json::json!({
                    "AthleteID": 1, "Result": result, "SortInt": sort_int,
                    "EventID": 21, "EventTypeID": 0, "SeasonID": season,
                    "Exhibition": false, "ResultDate": date,
                })
            })
            .collect();
        serde_json::from_value(serde_json::json!({
            "athlete": { "IDAthlete": 1, "FirstName": "A", "LastName": "B" },
            "eventsTF": [{ "Event": "400 Meters", "Type": "T", "IDEvent": 21 }],
            "resultsTF": results,
        }))
        .unwrap()
    }

    fn day(s: &str) -> chrono::NaiveDate {
        s.parse().unwrap()
    }

    #[test]
    fn a_tie_goes_to_the_earliest_dated_mark_and_undated_loses() {
        let ids = HashSet::from([21]);
        let b = bio(&[
            ("58.00a", 58000, 2026, None),
            ("58.00a", 58000, 2026, Some("2026-04-18T00:00:00")),
            ("58.00a", 58000, 2025, Some("2025-05-10T00:00:00")),
        ]);
        let now = AsOf::today(day("2026-10-26"));
        let pr = personal_record_as_of(&b, &ids, now).unwrap();
        assert_eq!(pr.date(), Some(day("2025-05-10")));
        let sb = bests_as_of(&b, &ids, 2026, now).season_best.unwrap();
        assert_eq!(sb.date(), Some(day("2026-04-18")));
    }

    #[test]
    fn indoor_marks_are_never_qualifying() {
        assert!(is_indoor_season(12026) && is_indoor_season(10_000));
        assert!(!is_indoor_season(2026));
        let ids = HashSet::from([21]);
        let b = bio(&[
            ("55.00a", 55000, 12026, Some("2026-02-14T00:00:00")),
            ("57.00a", 57000, 2026, Some("2026-04-18T00:00:00")),
        ]);
        let now = AsOf::today(day("2026-10-26"));
        let pr = personal_record_as_of(&b, &ids, now).unwrap();
        assert_eq!(pr.result, "57.00a");
        assert!(bests_as_of(&b, &ids, 12026, now).season_best.is_none());
    }

    #[test]
    fn an_undated_mark_counts_unless_it_could_be_after_the_as_of_date() {
        let ids = HashSet::from([21]);
        let b = bio(&[
            ("56.00a", 56000, 2024, None), // an earlier season: before any 2026 date
            ("55.00a", 55000, 2026, None), // this season: could be from today
            ("57.00a", 57000, 2026, Some("2026-04-18T00:00:00")),
        ]);
        let today = day("2026-10-26");
        let pr = |date| personal_record_as_of(&b, &ids, AsOf { date, today }).unwrap();
        // As of today every undated mark counts.
        assert_eq!(pr(today).result, "55.00a");
        // As of yesterday (a meet today): this season's undated mark might be
        // from the meet, but the 2024 one cannot be.
        assert_eq!(pr(day("2026-10-25")).result, "56.00a");
    }

    #[test]
    fn event_short_form_maps_to_meters_name() {
        assert_eq!(event_name_for("400m"), "400 meters");
        assert_eq!(event_name_for(" 1600M "), "1600 meters");
        assert_eq!(event_name_for("400 Meters"), "400 meters");
        assert_eq!(event_name_for("m"), "m");
        assert_eq!(event_name_for("4x400m"), "4x400m");
    }

    #[test]
    fn field_sortint_reference_values() {
        // SortInt 19569000 → 431 in → 35-11.00
        assert!((inches_from_sortint(19_569_000) - 431.0).abs() < 1e-9);
        assert_eq!(format_feet_inches(431.0), "35-11.00");
        // 19383000 → 617 in → 51-05.00
        assert!((inches_from_sortint(19_383_000) - 617.0).abs() < 1e-9);
        assert_eq!(format_feet_inches(617.0), "51-05.00");
    }

    #[test]
    fn timing_suffix_and_validity() {
        assert_eq!(strip_timing_suffix("58.50a"), "58.50");
        assert_eq!(strip_timing_suffix("2:05.43"), "2:05.43");
        assert!(is_hand_timed("56.63h"));
        assert!(!is_hand_timed("56.63a"));
        // real vs non-marks
        assert!(is_real_time("1:02.47a"));
        assert!(is_real_time("58.50"));
        for bogus in ["DNS", "DNF", "NT", "SCR", "DQ", "FS", ""] {
            assert!(!is_real_time(bogus), "{bogus} should not be a real time");
        }
    }
}
