//! Mode A: a meet's events and entries (`CONTEXT.md`, Input modes).
//!
//! Three `api/v1/Meet` calls (`docs/agents/systems/athletic-net.md`, Meets):
//! `GetMeetData` (the meet, its divisions and the `jwtMeet` token),
//! `GetEventListData` (its events, and which event and division pairs have
//! entries) and `GetEntriesData` (one event's entries in one division).
//! The models here read only the fields `ath` uses; the rest are ignored.
//!
//! Only an entry Athletic.net has accepted (`AcceptanceStatus` 10) and that
//! is not scratched is an Entrant. Other `AcceptanceStatus` values have not
//! been seen, so every other value is treated as not accepted, and the entry
//! is reported as left out rather than seeded.
//!
//! A relay's entries are one row per leg, each with the team's `TeamID`, its
//! entered seed and a `Letter` such as `A1` (team A, leg 1; a school's second
//! team is `B1`..`B4`). [`relay_teams`] groups them into [`RelayTeam`]s.
//! A relay of equal legs (`4x800m`, `4xMile`) is seeded by summing its
//! runners' Personal records on its Seeding event ([`EqualLegs`],
//! `seed_relay`); medley and shuttle relays are not seeded yet.

use anyhow::{Context, Result, bail};
use chrono::NaiveDate;
use serde::{Deserialize, Deserializer, Serialize};

use crate::area::{Gender, flat_race_short};
use crate::contract::{Report, columns};
use crate::mark::format_running;
use crate::model::athlete_url;
use crate::roster::RosterRow;
use crate::source::DataSource;

/// The `AcceptanceStatus` of an accepted entry. The only value seen live
/// (2026-11-06); any other counts as not accepted.
pub const ACCEPTED: i64 = 10;

/// Athletic.net's page for a meet.
pub fn meet_url(meet_id: i64) -> String {
    format!("https://www.athletic.net/TrackAndField/meet/{meet_id}")
}

/// Athletic.net's entries page for one event of a meet, in one division: the
/// same view `GetEntriesData` answers for.
pub fn entries_url(meet_id: i64, gender: Gender, div_id: i64, event_short: &str) -> String {
    format!(
        "{}/entries/{}/{div_id}/{event_short}",
        meet_url(meet_id),
        gender.as_str()
    )
}

// ---------------------------------------------------------------------------
// Athletic.net responses

/// `GET Meet/GetMeetData?meetId=<id>&sport=tf` (only the fields `ath` uses).
/// Its `jwtMeet` token is read separately by the live source and never kept.
#[derive(Debug, Clone, Deserialize)]
pub struct MeetData {
    pub meet: MeetInfo,
    #[serde(rename = "tfDivisions", default)]
    pub divisions: Vec<Division>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MeetInfo {
    #[serde(rename = "ID")]
    pub id: i64,
    #[serde(rename = "Name", default)]
    pub name: String,
    /// `2026-12-29T00:00:00`.
    #[serde(rename = "MeetDate", default)]
    pub meet_date: Option<String>,
    #[serde(rename = "StartDate", default)]
    pub start_date: Option<String>,
    #[serde(rename = "EndDate", default)]
    pub end_date: Option<String>,
    /// The plain year outdoors (`2027`), `10000 + year` indoors (`12027`).
    #[serde(rename = "SeasonID", default)]
    pub season_id: Option<i64>,
    /// How many runners a relay team enters (4).
    #[serde(rename = "RelayAthletesRequired", default)]
    pub relay_athletes: Option<i64>,
    #[serde(rename = "Location", default)]
    pub location: Option<Location>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Location {
    #[serde(rename = "Indoor", default)]
    pub indoor: Option<bool>,
    #[serde(rename = "Name", default)]
    pub name: Option<String>,
    #[serde(rename = "City", default)]
    pub city: Option<String>,
    #[serde(rename = "State", default)]
    pub state: Option<String>,
}

/// One of a meet's divisions. `IDDiv` is what the entries page and
/// `GetEntriesData` take (`divId`); it is meet-specific.
#[derive(Debug, Clone, Deserialize)]
pub struct Division {
    #[serde(rename = "IDDiv")]
    pub id: i64,
    #[serde(rename = "Division", default)]
    pub name: String,
    #[serde(rename = "DivisionShort", default)]
    pub short: Option<String>,
}

/// `GET Meet/GetEventListData?meetId=<id>`, sent with the meet's token.
#[derive(Debug, Clone, Deserialize)]
pub struct MeetEventList {
    /// Required: an answer without it is not an event list.
    pub events: Vec<MeetEvent>,
    /// Every (event, division) pair with entries.
    #[serde(rename = "eventDivsWithEntries")]
    pub with_entries: Vec<EventDiv>,
}

/// One of a meet's events. `ID` is Athletic.net's event id (gender-specific,
/// so never hardcoded: events are found by name).
#[derive(Debug, Clone, Deserialize)]
pub struct MeetEvent {
    #[serde(rename = "ID")]
    pub id: i64,
    /// The event's name (`800 Meters`), as Profiles name it.
    #[serde(rename = "Event", default)]
    pub name: Option<String>,
    /// The code the entries page and `GetEntriesData` take (`800m`, `4x800m`).
    #[serde(rename = "EventShort", default)]
    pub short: String,
    /// `M` or `F`.
    #[serde(rename = "Gender", default)]
    pub gender: String,
    /// `T` (track) or `F` (field).
    #[serde(rename = "Type", default)]
    pub kind: Option<String>,
}

impl MeetEvent {
    /// The event's gender, when it is `M` or `F`.
    pub fn gender(&self) -> Option<Gender> {
        Gender::from_bio(&self.gender)
    }

    /// The event's name, or its code when it has none.
    pub fn label(&self) -> &str {
        self.name
            .as_deref()
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .unwrap_or(&self.short)
    }

    /// The event's code and name, lower case.
    fn texts(&self) -> impl Iterator<Item = String> + '_ {
        std::iter::once(self.short.as_str())
            .chain(self.name.as_deref())
            .map(|t| t.trim().to_ascii_lowercase())
    }

    /// True for a relay, by its code or its name: legs `x` distance
    /// (`4x400m`), a medley or shuttle (`SMR 1600m`, `sprintmed2248`,
    /// `4x55 Shuttle Hurdles`, `55shuttleh`), or a name that says relay.
    /// (Entries with a `Letter` also make one: [`EventEntries::from_data`].)
    pub fn is_relay(&self) -> bool {
        self.texts().any(|t| is_relay_name(&t))
    }

    /// True for an event only run indoors (55m, 55m hurdles, 60m, 60m
    /// hurdles, 300m, 500m, 600m, 1000m, shuttle hurdles), by its code or
    /// name. Seeding reads outdoor Personal records only, so these have
    /// nothing to seed on (indoor seeding comes later).
    pub fn is_indoor_only(&self) -> bool {
        self.texts().any(|t| is_indoor_only_event(&t))
    }

    /// True for a running event (`Type` `T`, or no type given).
    pub fn is_running(&self) -> bool {
        !self
            .kind
            .as_deref()
            .is_some_and(|k| k.trim().eq_ignore_ascii_case("F"))
    }

    /// What kind of relay this is for seeding, from its code and name (never
    /// its id): legs all one flat race ([`RelayShape::Equal`], `4x800m`,
    /// `4x100 Relay`, `4xMile`), a medley or shuttle ([`RelayShape::Mixed`]),
    /// a relay whose leg distance cannot be read (yards, no distance), or
    /// neither code nor name saying relay (only its entries do).
    pub fn relay_shape(&self) -> RelayShape {
        if self
            .texts()
            .any(|t| is_mixed_relay(&t) || t.contains("hurdle"))
        {
            return RelayShape::Mixed;
        }
        if let Some(legs) = self.texts().find_map(|t| equal_legs(&squash(&t))) {
            return RelayShape::Equal(legs);
        }
        if self.is_relay() { RelayShape::Unread } else { RelayShape::NotNamed }
    }
}

/// True for a relay's code or name, lower case: legs `x` distance
/// (`4x400m`), a name that says relay, or a medley or shuttle
/// ([`is_mixed_relay`]).
pub fn is_relay_name(text: &str) -> bool {
    let t = text.trim().to_ascii_lowercase();
    let digits = t.bytes().take_while(u8::is_ascii_digit).count();
    (digits > 0 && t.as_bytes().get(digits) == Some(&b'x'))
        || t.contains("relay")
        || is_mixed_relay(&t)
}

/// True for a medley or shuttle relay's code or name, lower case (`SMR
/// 1600m`, `sprintmed2248`, `Distance Medley`, `4x55 Shuttle Hurdles`): its
/// legs are not one flat race.
fn is_mixed_relay(t: &str) -> bool {
    ["medley", "shuttle", "sprintmed", "distmed"]
        .iter()
        .any(|w| t.contains(w))
        || t.split(|c: char| !c.is_ascii_alphanumeric())
            .any(|w| matches!(w, "smr" | "dmr" | "shr"))
}

/// What a relay is, for seeding ([`MeetEvent::relay_shape`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelayShape {
    /// Every leg the same flat race: seedable.
    Equal(EqualLegs),
    /// A medley or shuttle relay (or hurdles): its legs differ.
    Mixed,
    /// It says relay, but its leg distance cannot be read (yards, or no
    /// distance with a unit).
    Unread,
    /// Neither its code nor its name says relay; only its entries (legs with
    /// a `Letter`) do.
    NotNamed,
}

/// A relay whose legs are all one flat race.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EqualLegs {
    /// How many runners (4 in `4x800m`).
    pub legs: u32,
    /// The Seeding event: the flat race each runner is seeded on, as its
    /// short form (`800m`; `1600m` for a mile leg).
    pub seeding_event: String,
    /// Mile legs (`4xMile`): each runner is seeded on the better of their
    /// 1600m and their 1 Mile, the mile converted to a 1600m Estimated mark
    /// (`seed_relay::MILE_EVENT`). False for every other relay.
    pub mile: bool,
}

/// Leg distances in meters a relay names without a unit (`4x800 Relay`,
/// `4x400R`): the standard flat races. Any other bare number (`4x4`, `4x1
/// Relay`) is not read as a distance.
const BARE_LEG_METERS: [&str; 6] = ["100", "200", "400", "800", "1600", "3200"];

/// `4x800m`, `4x800meterrelay`, `4x800relay`, `4xmile`, `4x1mi` (squashed)
/// -> the legs and the Seeding event. The distance needs a unit (`m`,
/// `meter(s)`, `mile`, `mi`), or must be a standard flat race in meters
/// ([`BARE_LEG_METERS`]); anything else is `None` (`4x200yd`, `4x4`,
/// `4x1relay`, `800m`).
fn equal_legs(t: &str) -> Option<EqualLegs> {
    let digits = |s: &str| s.bytes().take_while(u8::is_ascii_digit).count();
    let n = digits(t);
    let legs: u32 = t[..n].parse().ok()?;
    let rest = t[n..].strip_prefix('x')?;
    let (distance, unit) = rest.split_at(digits(rest));
    let mile = unit == "mi" || unit.starts_with("mile") || unit.starts_with("mirelay");
    let meters = unit == "m" || unit.starts_with("meter") || unit.starts_with("mrelay");
    let seeding_event = if mile && matches!(distance, "" | "1") {
        "1600m".to_string()
    } else if !mile
        && !distance.is_empty()
        && !distance.starts_with('0')
        && (meters || BARE_LEG_METERS.contains(&distance))
        && (meters || unit.is_empty() || unit.starts_with("relay") || unit == "r")
    {
        format!("{distance}m")
    } else {
        return None;
    };
    (legs > 1).then_some(EqualLegs {
        legs,
        seeding_event,
        mile,
    })
}

/// True for an event only run indoors, by its code or name (`55m`,
/// `60 Meter Hurdles`, `500 Meters`, `55shuttleh`), in any case and spacing.
pub fn is_indoor_only_event(event: &str) -> bool {
    indoor_only(&squash(event))
}

/// True for a squashed code or name (`55mh`, `60meterhurdles`, `500meters`,
/// `55shuttleh`) of an event only run indoors.
fn indoor_only(t: &str) -> bool {
    if t.contains("shuttle") {
        return true;
    }
    let digits = t.bytes().take_while(u8::is_ascii_digit).count();
    let (meters, rest) = t.split_at(digits);
    if !rest.starts_with('m') {
        return false; // a relay (`4x...`), yards, or no distance
    }
    let hurdles = rest == "mh" || rest.contains("hurdle");
    match meters {
        "55" | "60" => true,
        "300" | "500" | "600" | "1000" => !hurdles,
        _ => false,
    }
}

/// An (event, division) pair from `eventDivsWithEntries`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct EventDiv {
    /// The event's `ID`.
    pub e: i64,
    /// The division's `IDDiv`.
    pub d: i64,
}

/// `POST Meet/GetEntriesData`, sent with the meet's token.
#[derive(Debug, Clone, Deserialize)]
pub struct EntriesData {
    #[serde(rename = "eventId", default)]
    pub event_id: Option<i64>,
    /// Required: an answer without it is not an event's entries.
    pub entries: Vec<EntryRow>,
    #[serde(default)]
    pub teams: Vec<EntryTeam>,
}

/// One entry: an athlete in an individual event, or one leg of a relay team.
#[derive(Debug, Clone, Deserialize)]
pub struct EntryRow {
    #[serde(rename = "IDAthlete", default)]
    pub athlete_id: Option<i64>,
    #[serde(rename = "FirstName", default)]
    pub first_name: Option<String>,
    #[serde(rename = "LastName", default)]
    pub last_name: Option<String>,
    /// The athlete's team; for a relay, every leg's row has the team's.
    #[serde(rename = "TeamID", default)]
    pub team_id: Option<i64>,
    #[serde(rename = "SchoolID", default)]
    pub school_id: Option<i64>,
    #[serde(rename = "TeamName", default)]
    pub team_name: Option<String>,
    /// `null` in an individual event; `A1`..`A4` for team A's legs in a relay.
    #[serde(rename = "Letter", default)]
    pub letter: Option<String>,
    /// Sent as a string (`"11"`).
    #[serde(rename = "Grade", default, deserialize_with = "lenient_int")]
    pub grade: Option<i64>,
    #[serde(rename = "Scratched", default, deserialize_with = "lenient_bool")]
    pub scratched: bool,
    #[serde(rename = "AcceptanceStatus", default)]
    pub acceptance_status: Option<i64>,
    /// The entered seed mark (a relay team's on each leg's row), in the
    /// event's `SortInt` encoding.
    #[serde(rename = "SortInt", default)]
    pub sort_int: Option<i64>,
}

impl EntryRow {
    pub fn name(&self) -> String {
        let first = self.first_name.as_deref().unwrap_or_default().trim();
        let last = self.last_name.as_deref().unwrap_or_default().trim();
        format!("{first} {last}").trim().to_string()
    }

    /// The `Letter`, trimmed, when it says something.
    fn letter(&self) -> Option<&str> {
        self.letter
            .as_deref()
            .map(str::trim)
            .filter(|l| !l.is_empty())
    }

    /// Why this entry is not an Entrant, or `None` when it is one.
    fn left_out(&self) -> Option<LeftOutReason> {
        if self.scratched {
            Some(LeftOutReason::Scratched)
        } else if self.acceptance_status != Some(ACCEPTED) {
            Some(LeftOutReason::NotAccepted)
        } else if self.athlete_id.is_none() {
            Some(LeftOutReason::NoAthlete)
        } else {
            None
        }
    }
}

/// A team in an entries response.
#[derive(Debug, Clone, Deserialize)]
pub struct EntryTeam {
    /// Optional, like every field here, so one odd item never fails the
    /// whole answer.
    #[serde(rename = "IDSchool", default)]
    pub id: Option<i64>,
    #[serde(rename = "SchoolName", default)]
    pub name: Option<String>,
}

/// A whole number sent as a number or a string (`"11"`); anything else is
/// `None`.
fn lenient_int<'de, D: Deserializer<'de>>(d: D) -> Result<Option<i64>, D::Error> {
    Ok(match Option::<serde_json::Value>::deserialize(d)? {
        Some(serde_json::Value::Number(n)) => n.as_i64(),
        Some(serde_json::Value::String(s)) => s.trim().parse().ok(),
        _ => None,
    })
}

/// A flag sent as a boolean or a number (`0` / `1`); `null` is false.
fn lenient_bool<'de, D: Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    Ok(match Option::<serde_json::Value>::deserialize(d)? {
        Some(serde_json::Value::Bool(b)) => b,
        Some(serde_json::Value::Number(n)) => n.as_i64().is_some_and(|n| n != 0),
        _ => false,
    })
}

/// The request for one event's entries in one division.
#[derive(Debug, Clone, Copy)]
pub struct EntriesQuery<'a> {
    pub meet_id: i64,
    pub gender: Gender,
    /// The division's `IDDiv`.
    pub div_id: i64,
    /// The event's code (`800m`).
    pub event_short: &'a str,
}

// ---------------------------------------------------------------------------
// The meet, read

impl MeetInfo {
    /// The day the meet starts: the earlier of `StartDate` and `MeetDate`.
    pub fn date(&self) -> Option<NaiveDate> {
        [&self.start_date, &self.meet_date]
            .into_iter()
            .filter_map(|d| day(d.as_deref()?))
            .min()
    }

    /// The meet's last day: the latest of `EndDate`, `MeetDate` and
    /// `StartDate`.
    pub fn end(&self) -> Option<NaiveDate> {
        [&self.end_date, &self.meet_date, &self.start_date]
            .into_iter()
            .filter_map(|d| day(d.as_deref()?))
            .max()
    }

    /// Indoor by its location, or failing that by its season.
    pub fn indoor(&self) -> bool {
        self.location
            .as_ref()
            .and_then(|l| l.indoor)
            .unwrap_or_else(|| self.season_id.is_some_and(crate::mark::is_indoor_season))
    }
}

/// `2026-12-29T00:00:00` -> 2026-12-29.
fn day(raw: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(raw.trim().get(..10)?, "%Y-%m-%d").ok()
}

/// `2027 Indoor` for 12027, `2027 Outdoor` for 2027.
pub fn season_label(season_id: i64) -> String {
    if crate::mark::is_indoor_season(season_id) {
        format!("{} Indoor", season_id - crate::mark::INDOOR_SEASON_BASE)
    } else {
        format!("{season_id} Outdoor")
    }
}

/// `abc 1 mile` -> `abc1mile`: lower case, no spaces, for matching names.
fn squash(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

/// The meet's division `query` names: its `IDDiv` (`13`), or its name or
/// short name in any case (`Varsity`, `V`).
pub fn find_division<'a>(meet: &'a MeetData, query: &str) -> Result<&'a Division> {
    let q = query.trim();
    let by_id = q.parse::<i64>().ok();
    let want = squash(q);
    let found = meet
        .divisions
        .iter()
        .find(|d| Some(d.id) == by_id)
        .or_else(|| {
            meet.divisions.iter().find(|d| {
                squash(&d.name) == want || d.short.as_deref().is_some_and(|s| squash(s) == want)
            })
        });
    found.with_context(|| {
        format!(
            "meet {} has no division {q:?}; its divisions are {}",
            meet.meet.id,
            list_or_none(meet.divisions.iter().map(|d| division_label(
                &d.name,
                d.short.as_deref(),
                d.id
            )))
        )
    })
}

/// `Varsity (V, 13)`, or `Varsity (13)` without a short name.
fn division_label(name: &str, short: Option<&str>, id: i64) -> String {
    match short.map(str::trim).filter(|s| !s.is_empty()) {
        Some(short) => format!("{} ({short}, {id})", name.trim()),
        None => format!("{} ({id})", name.trim()),
    }
}

fn list_or_none(items: impl Iterator<Item = String>) -> String {
    let items: Vec<String> = items.collect();
    if items.is_empty() { "none".to_string() } else { items.join(", ") }
}

/// The meet's `gender` event `query` names, by its code (`800m`, `4x800m`)
/// or its name (`800 Meters`), in any case and spacing. Found by name, never
/// by a hardcoded event id.
pub fn find_event<'a>(
    list: &'a MeetEventList,
    meet_id: i64,
    gender: Gender,
    query: &str,
) -> Result<&'a MeetEvent> {
    let want = squash(query);
    let short = flat_race_short(query).map(|s| squash(&s));
    let matches: Vec<&MeetEvent> = list
        .events
        .iter()
        .filter(|e| e.gender() == Some(gender))
        .filter(|e| {
            let code = squash(&e.short);
            code == want
                || short.as_ref() == Some(&code)
                || e.name.as_deref().is_some_and(|n| squash(n) == want)
        })
        .collect();
    match matches.as_slice() {
        [one] => Ok(one),
        [] => bail!(
            "meet {meet_id} has no {} event {:?}; its {} events are {}",
            gender.possessive(),
            query.trim(),
            gender.possessive(),
            list_or_none(
                list.events
                    .iter()
                    .filter(|e| e.gender() == Some(gender))
                    .map(|e| e.short.clone())
            )
        ),
        many => bail!(
            "{:?} names {} of meet {meet_id}'s {} events ({}); use the event's code",
            query.trim(),
            many.len(),
            gender.possessive(),
            many.iter()
                .map(|e| e.short.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

// ---------------------------------------------------------------------------
// Entrants

/// Why an entry is not an Entrant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LeftOutReason {
    /// Scratched from the event.
    Scratched,
    /// Not accepted: an `AcceptanceStatus` other than 10 (or none). The only
    /// value seen live is 10 (accepted), so every other one is left out.
    NotAccepted,
    /// The entry names no athlete.
    NoAthlete,
}

/// An entry that is not an Entrant, with why.
#[derive(Debug, Clone, Serialize)]
pub struct LeftOut {
    pub name: String,
    pub athlete_id: Option<i64>,
    /// A relay leg's `Letter` (`A1`), `null` in an individual event.
    pub letter: Option<String>,
    pub team_id: Option<i64>,
    pub reason: LeftOutReason,
    pub acceptance_status: Option<i64>,
}

impl LeftOut {
    fn new(row: &EntryRow, reason: LeftOutReason) -> Self {
        Self {
            name: row.name(),
            athlete_id: row.athlete_id,
            letter: row.letter().map(str::to_string),
            team_id: row.team_id,
            reason,
            acceptance_status: row.acceptance_status,
        }
    }

    /// `scratched`, `not accepted (AcceptanceStatus 5)`.
    pub fn why(&self) -> String {
        match self.reason {
            LeftOutReason::Scratched => "scratched".to_string(),
            LeftOutReason::NotAccepted => match self.acceptance_status {
                Some(s) => format!("not accepted (AcceptanceStatus {s})"),
                None => "not accepted (no AcceptanceStatus)".to_string(),
            },
            LeftOutReason::NoAthlete => "the entry names no athlete".to_string(),
        }
    }
}

/// An Entrant in an individual event.
#[derive(Debug, Clone, Serialize)]
pub struct IndividualEntrant {
    pub athlete_id: i64,
    pub name: String,
    pub athlete_url: String,
    pub grade: Option<i64>,
    pub team_id: Option<i64>,
    pub school: String,
    /// The mark entered with the entry, as its `SortInt`.
    pub entered_sort_int: Option<i64>,
}

impl IndividualEntrant {
    /// The Entrant as a roster row pinned to its athlete id, so it is seeded
    /// exactly as a Mode B row is, with no name search.
    pub fn roster_row(&self) -> RosterRow {
        RosterRow {
            name: self.name.clone(),
            athlete_id: Some(self.athlete_id),
            school: Some(self.school.clone()).filter(|s| !s.is_empty()),
            city: None,
            state: None,
        }
    }
}

/// One runner of a relay team.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RelayLeg {
    /// The leg from the `Letter` (`A3` is leg 3); `null` when it has no
    /// number.
    pub leg: Option<u32>,
    pub athlete_id: i64,
    pub name: String,
    pub athlete_url: String,
    pub grade: Option<i64>,
}

/// A relay Entrant: one team of a school (`A`, or `B` for its second team)
/// and its runners in leg order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RelayTeam {
    pub team_id: Option<i64>,
    pub school_id: Option<i64>,
    pub school: String,
    /// `A`, `B`, ...
    pub letter: String,
    /// The team's entered seed, as its `SortInt`.
    pub entered_sort_int: Option<i64>,
    /// In leg order; legs without a number last, in entry order.
    pub legs: Vec<RelayLeg>,
}

/// `A3` -> (`A`, Some(3)); `A` -> (`A`, None).
fn split_letter(letter: &str) -> (String, Option<u32>) {
    let digits = letter.len() - letter.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    let (team, leg) = letter.split_at(letter.len() - digits);
    (team.to_string(), leg.parse().ok())
}

/// The school an entry is for: its `TeamName`, or the response's team whose
/// `IDSchool` is the entry's `SchoolID`. `TeamID` is another id space (a
/// relay team's own id), so it is never matched against `IDSchool`; with
/// neither, the school is blank.
fn school_of(row: &EntryRow, teams: &[EntryTeam]) -> String {
    let named = |n: Option<&str>| {
        n.map(str::trim)
            .filter(|n| !n.is_empty())
            .map(str::to_string)
    };
    named(row.team_name.as_deref())
        .or_else(|| {
            let id = row.school_id?;
            teams
                .iter()
                .find(|t| t.id == Some(id))
                .and_then(|t| named(t.name.as_deref()))
        })
        .unwrap_or_default()
}

/// Which relay team an entry is a leg of, besides its letter: its `TeamID`,
/// else its `SchoolID`, else its school's name, so legs of two schools never
/// merge.
#[derive(Debug, Clone, PartialEq, Eq)]
enum TeamKey {
    Team(i64),
    School(i64),
    Name(String),
}

fn team_key(row: &EntryRow, teams: &[EntryTeam]) -> TeamKey {
    match (row.team_id, row.school_id) {
        (Some(id), _) => TeamKey::Team(id),
        (None, Some(id)) => TeamKey::School(id),
        (None, None) => TeamKey::Name(school_of(row, teams)),
    }
}

/// One event's entries, sorted into Entrants and the entries left out.
#[derive(Debug, Clone, Default, Serialize)]
pub struct EventEntries {
    /// An individual event's Entrants, in Athletic.net's order.
    pub entrants: Vec<IndividualEntrant>,
    /// A relay's Entrants, in order of their first leg's entry.
    pub teams: Vec<RelayTeam>,
    /// Entries that are not Entrants (scratched, not accepted), with why.
    pub left_out: Vec<LeftOut>,
}

impl EventEntries {
    /// Sort `data` into Entrants and left-out entries: individual Entrants,
    /// or with `relay` (or any entry with a `Letter`), relay teams.
    pub fn from_data(data: &EntriesData, relay: bool) -> Self {
        let relay = relay || data.entries.iter().any(|r| r.letter().is_some());
        let mut out = Self::default();
        let mut kept = Vec::new();
        for row in &data.entries {
            match row.left_out() {
                Some(reason) => out.left_out.push(LeftOut::new(row, reason)),
                None => kept.push(row),
            }
        }
        if relay {
            out.teams = relay_teams(kept.into_iter(), &data.teams);
        } else {
            out.entrants = kept
                .into_iter()
                .filter_map(|row| {
                    let athlete_id = row.athlete_id?;
                    Some(IndividualEntrant {
                        athlete_id,
                        name: row.name(),
                        athlete_url: athlete_url(athlete_id),
                        grade: row.grade,
                        team_id: row.team_id,
                        school: school_of(row, &data.teams),
                        entered_sort_int: row.sort_int,
                    })
                })
                .collect();
        }
        out
    }
}

/// Group relay leg entries into teams by `TeamID` (else `SchoolID`, else the
/// school's name) and the `Letter`'s team letter (`A1`..`A4` are team A), in
/// order of each team's first entry, with legs in leg order. Rows without an
/// athlete are skipped.
pub fn relay_teams<'a>(
    rows: impl Iterator<Item = &'a EntryRow>,
    teams: &[EntryTeam],
) -> Vec<RelayTeam> {
    let mut out: Vec<RelayTeam> = Vec::new();
    // Each team's key, by index into `out`.
    let mut keys: Vec<(TeamKey, String)> = Vec::new();
    for row in rows {
        let Some(athlete_id) = row.athlete_id else {
            continue;
        };
        let (letter, leg) = split_letter(row.letter().unwrap_or_default());
        let key = (team_key(row, teams), letter.clone());
        let i = match keys.iter().position(|k| *k == key) {
            Some(i) => i,
            None => {
                keys.push(key);
                out.push(RelayTeam {
                    team_id: row.team_id,
                    school_id: row.school_id,
                    school: school_of(row, teams),
                    letter,
                    entered_sort_int: row.sort_int,
                    legs: Vec::new(),
                });
                out.len() - 1
            }
        };
        let team = &mut out[i];
        team.entered_sort_int = team.entered_sort_int.or(row.sort_int);
        team.legs.push(RelayLeg {
            leg,
            athlete_id,
            name: row.name(),
            athlete_url: athlete_url(athlete_id),
            grade: row.grade,
        });
    }
    for team in &mut out {
        // Stable: legs without a number keep their entry order, last.
        team.legs.sort_by_key(|l| (l.leg.is_none(), l.leg));
    }
    out
}

// ---------------------------------------------------------------------------
// One event, selected

/// Which event's entries to read: `ath seed --meet` and `ath meet --event`.
#[derive(Debug, Clone, Copy)]
pub struct EventSelect<'a> {
    pub meet_id: i64,
    pub gender: Gender,
    /// A division's `IDDiv`, name or short name.
    pub division: &'a str,
    /// An event's code or name.
    pub event: &'a str,
}

/// A meet, as a seed sheet or `ath meet` names it.
#[derive(Debug, Clone, Serialize)]
pub struct MeetRef {
    pub id: i64,
    pub name: String,
    /// The day the meet starts.
    pub date: Option<NaiveDate>,
    pub season_id: Option<i64>,
    pub indoor: bool,
    pub url: String,
}

impl MeetRef {
    pub fn new(info: &MeetInfo) -> Self {
        Self {
            id: info.id,
            name: info.name.trim().to_string(),
            date: info.date(),
            season_id: info.season_id,
            indoor: info.indoor(),
            url: meet_url(info.id),
        }
    }
}

/// One event of a meet in one division.
#[derive(Debug, Clone, Serialize)]
pub struct EventRef {
    /// The meet (its `url` names it too; not printed again).
    #[serde(skip)]
    pub meet_id: i64,
    pub gender: Gender,
    pub division_id: i64,
    pub division: String,
    pub event_id: i64,
    pub event: String,
    pub event_short: String,
    pub relay: bool,
    /// Its entries page.
    pub url: String,
}

impl EventRef {
    fn new(meet_id: i64, gender: Gender, div: &Division, event: &MeetEvent) -> Self {
        Self {
            meet_id,
            gender,
            division_id: div.id,
            division: div.name.trim().to_string(),
            event_id: event.id,
            event: event.label().to_string(),
            event_short: event.short.clone(),
            relay: event.is_relay(),
            url: entries_url(meet_id, gender, div.id, &event.short),
        }
    }
}

/// One event's entries in one division, read and sorted.
#[derive(Debug, Clone)]
pub struct SelectedEvent {
    pub meet: MeetData,
    pub event: EventRef,
    /// Whether it is a running event (`Type` `T`).
    pub running: bool,
    /// For a relay, what kind it is for seeding
    /// ([`MeetEvent::relay_shape`]).
    pub relay_shape: RelayShape,
    pub entries: EventEntries,
}

/// One event of a meet in one division, found in the meet's event list, its
/// entries not read yet.
#[derive(Debug, Clone)]
pub struct Located {
    /// What kind of relay the event is for seeding ([`MeetEvent::relay_shape`]),
    /// read once from its code and name.
    pub relay_shape: RelayShape,
    pub division: Division,
    pub event: MeetEvent,
    pub event_ref: EventRef,
}

/// Find the division and event `select` names in the meet and its event
/// list, and check that the event has entries in the division. Errors name
/// what the meet does have.
pub fn locate(meet: &MeetData, list: &MeetEventList, select: &EventSelect<'_>) -> Result<Located> {
    let div = find_division(meet, select.division)?.clone();
    let event = find_event(list, select.meet_id, select.gender, select.event)?.clone();
    let event_ref = EventRef::new(select.meet_id, select.gender, &div, &event);
    if !list.with_entries.contains(&EventDiv {
        e: event.id,
        d: div.id,
    }) {
        let have = list_or_none(
            list.with_entries
                .iter()
                .filter(|p| p.d == div.id)
                .filter_map(|p| list.events.iter().find(|e| e.id == p.e))
                .filter(|e| e.gender() == Some(select.gender))
                .map(|e| e.short.clone()),
        );
        bail!(
            "meet {} has no {} entries in the {} division (yet); its {} events with \
             entries there are {have}",
            select.meet_id,
            event_ref.event_short,
            event_ref.division,
            select.gender.possessive()
        );
    }
    Ok(Located {
        relay_shape: event.relay_shape(),
        division: div,
        event,
        event_ref,
    })
}

/// Read and sort the entries of a [`located`](locate) event.
pub async fn read_entries<S: DataSource + ?Sized>(
    source: &S,
    meet: MeetData,
    located: Located,
) -> Result<SelectedEvent> {
    let Located {
        relay_shape,
        division,
        event,
        mut event_ref,
    } = located;
    let meet_id = meet.meet.id;
    let data = source
        .meet_entries(&EntriesQuery {
            meet_id,
            gender: event_ref.gender,
            div_id: division.id,
            event_short: &event.short,
        })
        .await?;
    if let Some(id) = data.event_id
        && id != event.id
    {
        bail!(
            "Athletic.net answered meet {meet_id}'s {} entries with event {id}, not {} ({})",
            event.short,
            event.id,
            event_ref.url
        );
    }
    let entries = EventEntries::from_data(&data, event_ref.relay);
    event_ref.relay = !entries.teams.is_empty() || event_ref.relay;
    Ok(SelectedEvent {
        meet,
        event: event_ref,
        running: event.is_running(),
        relay_shape,
        entries,
    })
}

/// The one path from a meet and its event list to an event's entries, for
/// `ath meet --event` and `ath seed --meet`: [`locate`] the event `select`
/// names, run `check` on it (an error stops before the entries are read;
/// [`seedable`] for seeding, nothing for `ath meet`), then [`read_entries`].
pub async fn read_event<S: DataSource + ?Sized>(
    source: &S,
    meet: MeetData,
    list: &MeetEventList,
    select: &EventSelect<'_>,
    check: impl FnOnce(&Located) -> Result<()>,
) -> Result<SelectedEvent> {
    let located = locate(&meet, list, select)?;
    check(&located)?;
    read_entries(source, meet, located).await
}

/// Whether `ath seed --meet` can seed a located event, from the meet's event
/// list alone: a relay only when its legs are four of one flat race (a
/// medley or shuttle relay is not seeded yet), a field event is not a
/// running event, and an event only run indoors has no outdoor Personal
/// records to seed on.
pub fn seedable(located: &Located) -> Result<()> {
    seedable_on(located, None)
}

/// [`seedable`] with `--seed-on`: a relay is checked with the event its
/// runners will be seeded on ([`relay_seeding`]), so a relay whose own legs
/// are only run indoors, or a `--seed-on` that is no outdoor flat race, is
/// refused before any entries are read.
pub fn seedable_on(located: &Located, seed_on: Option<&str>) -> Result<()> {
    let (event, at) = (&located.event, &located.event_ref);
    if event.is_relay() {
        return relay_seeding(at, &located.relay_shape, seed_on).map(|_| ());
    }
    if event.is_indoor_only() {
        bail!(
            "{} ({}) is only run indoors, and ath seeds on outdoor Personal records, so it has \
             no marks to seed on; indoor seeding comes later",
            at.event,
            at.event_short
        );
    }
    if !event.is_running() {
        bail!(
            "{} ({}) is a field event; ath seeds running events only",
            at.event,
            at.event_short
        );
    }
    Ok(())
}

/// How many runners a seeded relay team has.
pub const RELAY_LEGS: u32 = 4;

/// The legs of a relay `ath seed` can seed: four of one flat race. Anything
/// else is refused, saying why ([`relay_refusal`]) and where to see its
/// teams.
pub fn relay_seedable<'a>(at: &EventRef, shape: &'a RelayShape) -> Result<&'a EqualLegs> {
    match shape {
        RelayShape::Equal(l) if l.legs == RELAY_LEGS => Ok(l),
        _ => bail!("{}", relay_refusal(at, shape)),
    }
}

/// The event a seedable relay's runners are seeded on: `seed_on`
/// (`--seed-on`) when given, else the relay's own leg distance. Neither may
/// be an event only run indoors, since seeding reads outdoor marks; the
/// error says which one it was.
pub fn seeding_event(at: &EventRef, legs: &EqualLegs, seed_on: Option<&str>) -> Result<String> {
    match seed_on {
        Some(s) => {
            check_seed_on(s)?;
            Ok(s.trim().to_string())
        }
        None if is_indoor_only_event(&legs.seeding_event) => bail!(
            "{} ({})'s legs are {}, a distance only run indoors, and ath seeds on outdoor \
             Personal records, so its runners have no marks to seed on",
            at.event,
            at.event_short,
            legs.seeding_event
        ),
        None => Ok(legs.seeding_event.clone()),
    }
}

/// `--seed-on` must name one individual flat running event that is run
/// outdoors: a flat race (`1600m`, `1600 Meters`) or the mile (`1 Mile`).
/// A relay, a field event, hurdles or anything else is refused, saying so.
fn check_seed_on(s: &str) -> Result<()> {
    let takes = "it takes one individual flat race, by name (\"1600 Meters\", \"1 Mile\") or \
                 short form (\"1600m\")";
    if is_relay_name(s) {
        bail!("--seed-on {s:?} is a relay; {takes}");
    }
    let mile = matches!(squash(s).as_str(), "1mile" | "mile");
    if flat_race_short(s).is_none() && !mile {
        bail!(
            "--seed-on {s:?} is not a flat running race (such as a field event or hurdles); {takes}"
        );
    }
    if is_indoor_only_event(s) {
        bail!(
            "--seed-on {s:?} is only run indoors, and ath seeds on outdoor Personal records, so \
             its runners have no marks to seed on"
        );
    }
    Ok(())
}

/// What a seedable relay's runners are seeded on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelaySeeding {
    /// The Seeding event: `--seed-on`'s, else the relay's leg distance
    /// (`800m`; `1600m` for a mile leg).
    pub event: String,
    /// True on a 4xMile with no `--seed-on`: each runner's mile is read too,
    /// converted to the 1600m as an Estimated mark when faster.
    pub mile: bool,
}

/// The one way from a relay's shape and `--seed-on` to what its runners are
/// seeded on, for `ath seed --meet` and the tests alike: refused unless the
/// relay is four of one flat race ([`relay_seedable`]) and the event is an
/// outdoor flat race ([`seeding_event`]).
pub fn relay_seeding(
    at: &EventRef,
    shape: &RelayShape,
    seed_on: Option<&str>,
) -> Result<RelaySeeding> {
    let legs = relay_seedable(at, shape)?;
    Ok(RelaySeeding {
        event: seeding_event(at, legs, seed_on)?,
        mile: legs.mile && seed_on.is_none(),
    })
}

/// Why `ath seed --meet` does not seed a relay of this shape, by its cause,
/// and where to see its teams.
pub fn relay_refusal(at: &EventRef, shape: &RelayShape) -> String {
    let (event, code) = (&at.event, &at.event_short);
    let why = match shape {
        RelayShape::Equal(l) => format!(
            "{event} ({code}) is a relay of {} legs, and ath seed seeds relays of {RELAY_LEGS} \
             legs only",
            l.legs
        ),
        RelayShape::Mixed => format!(
            "{event} ({code}) is a relay whose legs are not all one flat race (a medley or \
             shuttle relay), and ath seed seeds only relays of four equal legs (such as 4x800m \
             or 4xMile) so far"
        ),
        RelayShape::Unread => format!(
            "{event} ({code}) is a relay, but its leg distance could not be read from {event:?} \
             or {code:?} (ath reads legs in meters or miles, such as 4x800m or 4xMile), so it \
             has no Seeding event"
        ),
        RelayShape::NotNamed => format!(
            "{event} ({code}) has relay entries (legs lettered A1 to A4), but neither its name \
             nor its code says a relay or its leg distance, so it has no Seeding event"
        ),
    };
    let indoors = if is_indoor_only_event(event) || is_indoor_only_event(code) {
        " (it is also only run indoors, and ath seeds on outdoor Personal records)"
    } else {
        ""
    };
    format!("{why}; {}{indoors}", show_teams(at))
}

/// `` `ath meet ...` shows its teams and legs ``.
fn show_teams(at: &EventRef) -> String {
    format!(
        "`ath meet {} --gender {} --div {} --event {}` shows its teams and legs",
        at.meet_id,
        at.gender.as_str(),
        at.division_id,
        at.event_short
    )
}

// ---------------------------------------------------------------------------
// `ath meet`

/// `ath meet`: a meet, its divisions and its events with entries (one row per
/// event and division), and with `--event` that event's entries. The rows are
/// what a seed sheet for "every event" goes through.
#[derive(Debug, Clone, Serialize)]
pub struct MeetReport {
    #[serde(flatten)]
    pub meet: MeetRef,
    /// The meet's last day (its `date` for a one-day meet).
    pub end_date: Option<NaiveDate>,
    /// `2027 Indoor`.
    pub season: Option<String>,
    pub location: Option<String>,
    pub city: Option<String>,
    pub state: Option<String>,
    /// How many runners a relay team enters.
    pub relay_athletes: Option<i64>,
    pub divisions: Vec<DivisionOut>,
    /// Every event and division with entries, by gender, division and the
    /// meet's own event order.
    pub events: Vec<EventRef>,
    /// With `--event`: that event's entries.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entries: Option<EntriesOut>,
    /// With `--event --resolve`: the event and division found, from the
    /// meet's event list alone (its entries are not read).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved: Option<EventRef>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DivisionOut {
    pub id: i64,
    pub name: String,
    pub short: Option<String>,
}

/// One event's entries as `ath meet --event` prints them.
#[derive(Debug, Clone, Serialize)]
pub struct EntriesOut {
    #[serde(flatten)]
    pub event: EventRef,
    #[serde(flatten)]
    pub entries: EventEntries,
    /// Whether the entered marks are times (`SortInt` in milliseconds).
    #[serde(skip)]
    pub running: bool,
}

impl MeetReport {
    pub fn new(meet: &MeetData, list: &MeetEventList) -> Self {
        let info = &meet.meet;
        let loc = info.location.clone().unwrap_or_default();
        let clean = |s: Option<String>| s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        let mut events = Vec::new();
        for gender in [Gender::F, Gender::M] {
            for div in &meet.divisions {
                for event in &list.events {
                    if event.gender() == Some(gender)
                        && list.with_entries.contains(&EventDiv {
                            e: event.id,
                            d: div.id,
                        })
                    {
                        events.push(EventRef::new(info.id, gender, div, event));
                    }
                }
            }
        }
        Self {
            meet: MeetRef::new(info),
            end_date: info.end(),
            season: info.season_id.map(season_label),
            location: clean(loc.name),
            city: clean(loc.city),
            state: clean(loc.state),
            relay_athletes: info.relay_athletes,
            divisions: meet
                .divisions
                .iter()
                .map(|d| DivisionOut {
                    id: d.id,
                    name: d.name.trim().to_string(),
                    short: d.short.clone(),
                })
                .collect(),
            events,
            entries: None,
            resolved: None,
        }
    }
}

/// `ath meet`'s report for `meet_id`, with `select`'s event's entries when
/// given.
pub async fn meet_report<S: DataSource + ?Sized>(
    source: &S,
    meet_id: i64,
    select: Option<&EventSelect<'_>>,
) -> Result<MeetReport> {
    let meet = source.meet(meet_id).await?;
    let list = source.meet_events(meet_id).await?;
    let mut report = MeetReport::new(&meet, &list);
    if let Some(select) = select {
        let sel = read_event(source, meet, &list, select, |_| Ok(())).await?;
        report.entries = Some(EntriesOut {
            event: sel.event,
            entries: sel.entries,
            running: sel.running,
        });
    }
    Ok(report)
}

/// `ath meet --resolve`'s report for `meet_id`: the meet, and the event and
/// division `select` names, found as `ath seed --meet` finds them
/// ([`locate`]: by code, flat race or name, and by division id, name or short
/// name), with no entries read. An event with no entries in the division is
/// an error, as it is for `ath seed --meet`.
pub async fn meet_resolve<S: DataSource + ?Sized>(
    source: &S,
    meet_id: i64,
    select: &EventSelect<'_>,
) -> Result<MeetReport> {
    let meet = source.meet(meet_id).await?;
    let list = source.meet_events(meet_id).await?;
    let mut report = MeetReport::new(&meet, &list);
    report.resolved = Some(locate(&meet, &list, select)?.event_ref);
    Ok(report)
}

/// An entered mark for the table: a time for a running event, the raw
/// `SortInt` otherwise, blank with none.
fn entered(sort_int: Option<i64>, running: bool) -> String {
    match sort_int {
        Some(s) if s > 0 && running => format_running(s),
        Some(s) if s > 0 => s.to_string(),
        _ => String::new(),
    }
}

fn grade(g: Option<i64>) -> String {
    g.map_or_else(String::new, |g| g.to_string())
}

impl Report for MeetReport {
    const SCHEMA: u32 = 1;

    fn table(&self) -> String {
        let m = &self.meet;
        let mut head = vec![m.name.clone()];
        head.extend(m.date.map(|d| d.to_string()));
        // The season says indoor or outdoor; say it only without one.
        head.push(
            self.season
                .clone()
                .unwrap_or_else(|| if m.indoor { "indoor" } else { "outdoor" }.to_string()),
        );
        let place: Vec<&str> = [&self.location, &self.city, &self.state]
            .into_iter()
            .filter_map(|s| s.as_deref())
            .collect();
        if !place.is_empty() {
            head.push(place.join(", "));
        }
        let mut out = format!("Meet {}: {}\n{}\n", m.id, head.join(", "), m.url);
        out.push_str(&format!(
            "Divisions: {}\n",
            list_or_none(self.divisions.iter().map(|d| division_label(
                &d.name,
                d.short.as_deref(),
                d.id
            )))
        ));
        if let Some(e) = &self.entries {
            out.push('\n');
            out.push_str(&e.table());
            return out;
        }
        if let Some(e) = &self.resolved {
            out.push_str(&format!(
                "\n{} ({}), {} {} (division {}){}\n{}\n",
                e.event,
                e.event_short,
                e.gender.possessive(),
                e.division,
                e.division_id,
                if e.relay { ", relay" } else { "" },
                e.url
            ));
            return out;
        }
        if self.events.is_empty() {
            out.push_str("\nNo events have entries yet.\n");
            return out;
        }
        out.push_str(&format!("\n{} events with entries:\n", self.events.len()));
        let mut rows = vec![
            ["Gender", "Division", "Event", "Code", "Relay", "Entries"]
                .map(String::from)
                .to_vec(),
        ];
        rows.extend(self.events.iter().map(|e| {
            vec![
                e.gender.as_str().to_string(),
                e.division.clone(),
                e.event.clone(),
                e.event_short.clone(),
                if e.relay { "relay" } else { "" }.to_string(),
                e.url.clone(),
            ]
        }));
        out.push_str(&columns(&rows));
        out
    }
}

impl EntriesOut {
    fn table(&self) -> String {
        let e = &self.event;
        let n = if e.relay { self.entries.teams.len() } else { self.entries.entrants.len() };
        let unit = match (e.relay, n) {
            (true, 1) => "team",
            (true, _) => "teams",
            (false, 1) => "entrant",
            (false, _) => "entrants",
        };
        let mut out = format!(
            "{} ({}), {} {}: {n} {unit}, {} left out\n{}\n",
            e.event,
            e.event_short,
            e.gender.noun(),
            e.division,
            self.entries.left_out.len(),
            e.url
        );
        if e.relay && !self.entries.teams.is_empty() {
            let mut rows = vec![
                ["Team", "Entered", "Leg", "Athlete", "Grade", "Profile"]
                    .map(String::from)
                    .to_vec(),
            ];
            for t in &self.entries.teams {
                for (i, leg) in t.legs.iter().enumerate() {
                    let first = i == 0;
                    rows.push(vec![
                        if first { format!("{} {}", t.school, t.letter) } else { String::new() },
                        if first {
                            entered(t.entered_sort_int, self.running)
                        } else {
                            String::new()
                        },
                        leg.leg.map_or_else(String::new, |l| l.to_string()),
                        leg.name.clone(),
                        grade(leg.grade),
                        leg.athlete_url.clone(),
                    ]);
                }
            }
            out.push('\n');
            out.push_str(&columns(&rows));
        } else if !self.entries.entrants.is_empty() {
            let mut rows = vec![
                ["Athlete", "Grade", "School", "Entered", "Profile"]
                    .map(String::from)
                    .to_vec(),
            ];
            rows.extend(self.entries.entrants.iter().map(|a| {
                vec![
                    a.name.clone(),
                    grade(a.grade),
                    a.school.clone(),
                    entered(a.entered_sort_int, self.running),
                    a.athlete_url.clone(),
                ]
            }));
            out.push('\n');
            out.push_str(&columns(&rows));
        }
        if !self.entries.left_out.is_empty() {
            out.push_str("\nLeft out:\n");
            for l in &self.entries.left_out {
                out.push_str(&left_out_line(l));
            }
        }
        out
    }
}

/// `Owen Pike (athlete 900100004): scratched`.
pub fn left_out_line(l: &LeftOut) -> String {
    let mut who = l.name.clone();
    let mut tags = Vec::new();
    if let Some(id) = l.athlete_id {
        tags.push(format!("athlete {id}"));
    }
    if let Some(letter) = &l.letter {
        tags.push(format!("leg {letter}"));
    }
    if !tags.is_empty() {
        who.push_str(&format!(" ({})", tags.join(", ")));
    }
    format!("{who}: {}\n", l.why())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(json: serde_json::Value) -> EntryRow {
        serde_json::from_value(json).unwrap()
    }

    fn leg(id: i64, team: i64, letter: &str, seed: i64) -> EntryRow {
        row(serde_json::json!({
            "IDAthlete": id, "FirstName": format!("Runner{id}"), "LastName": "Test",
            "TeamID": team, "SchoolID": 900401, "TeamName": "Pinecrest Academy",
            "Letter": letter, "Grade": "11", "Scratched": false,
            "AcceptanceStatus": 10, "SortInt": seed
        }))
    }

    #[test]
    fn letters_split_into_team_and_leg() {
        assert_eq!(split_letter("A1"), ("A".to_string(), Some(1)));
        assert_eq!(split_letter("B4"), ("B".to_string(), Some(4)));
        assert_eq!(split_letter("A"), ("A".to_string(), None));
        assert_eq!(split_letter("A12"), ("A".to_string(), Some(12)));
        assert_eq!(split_letter(""), (String::new(), None));
    }

    /// A team as the test compares it: team id, letter, entered `SortInt`
    /// and its legs' athlete ids.
    type TeamSummary<'a> = (Option<i64>, &'a str, Option<i64>, Vec<i64>);

    #[test]
    fn relay_legs_group_by_team_and_letter_in_leg_order() {
        // One school's A and B teams under the same TeamID, entered out of
        // leg order, and another school's A team.
        let rows = [
            leg(1, 77, "A2", 490_000),
            leg(2, 77, "B1", 510_000),
            leg(3, 77, "A1", 490_000),
            leg(4, 88, "A1", 500_000),
            leg(5, 77, "A4", 490_000),
            leg(6, 77, "A3", 490_000),
            leg(7, 77, "B2", 510_000),
        ];
        let teams = relay_teams(rows.iter(), &[]);
        let summary: Vec<TeamSummary> = teams
            .iter()
            .map(|t| {
                (
                    t.team_id,
                    t.letter.as_str(),
                    t.entered_sort_int,
                    t.legs.iter().map(|l| l.athlete_id).collect(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                (Some(77), "A", Some(490_000), vec![3, 1, 6, 5]),
                (Some(77), "B", Some(510_000), vec![2, 7]),
                (Some(88), "A", Some(500_000), vec![4]),
            ]
        );
        assert_eq!(teams[0].legs[0].leg, Some(1));
        assert_eq!(teams[0].legs[0].grade, Some(11));
        assert_eq!(teams[0].school, "Pinecrest Academy");
    }

    #[test]
    fn a_leg_without_a_number_goes_last() {
        let rows = [leg(1, 77, "A", 1), leg(2, 77, "A2", 1), leg(3, 77, "A1", 1)];
        let teams = relay_teams(rows.iter(), &[]);
        let legs: Vec<(Option<u32>, i64)> = teams[0]
            .legs
            .iter()
            .map(|l| (l.leg, l.athlete_id))
            .collect();
        assert_eq!(legs, [(Some(1), 3), (Some(2), 2), (None, 1)]);
    }

    #[test]
    fn only_accepted_unscratched_entries_are_entrants() {
        let data: EntriesData = serde_json::from_value(serde_json::json!({
            "eventId": 4,
            "entries": [
                {"IDAthlete": 1, "FirstName": "Ann", "LastName": "A", "AcceptanceStatus": 10,
                 "Scratched": false, "Grade": "10", "TeamName": "X High", "SortInt": 118000},
                {"IDAthlete": 2, "FirstName": "Bea", "LastName": "B", "AcceptanceStatus": 10,
                 "Scratched": true},
                {"IDAthlete": 3, "FirstName": "Cy", "LastName": "C", "AcceptanceStatus": 5,
                 "Scratched": false},
                {"IDAthlete": 4, "FirstName": "Di", "LastName": "D", "Scratched": 0},
                {"FirstName": "Nobody", "AcceptanceStatus": 10}
            ],
            "teams": []
        }))
        .unwrap();
        let e = EventEntries::from_data(&data, false);
        assert_eq!(e.entrants.len(), 1);
        assert_eq!(e.entrants[0].athlete_id, 1);
        assert_eq!(e.entrants[0].grade, Some(10));
        assert_eq!(e.entrants[0].entered_sort_int, Some(118000));
        let why: Vec<(Option<i64>, String)> =
            e.left_out.iter().map(|l| (l.athlete_id, l.why())).collect();
        assert_eq!(
            why,
            [
                (Some(2), "scratched".to_string()),
                (Some(3), "not accepted (AcceptanceStatus 5)".to_string()),
                (Some(4), "not accepted (no AcceptanceStatus)".to_string()),
                (None, "the entry names no athlete".to_string()),
            ]
        );
        assert!(e.teams.is_empty());
    }

    #[test]
    fn an_answer_without_its_data_does_not_parse_as_empty() {
        let err = |json: &str| serde_json::from_str::<MeetEventList>(json).is_err();
        assert!(err(
            r#"{"Message": "Authorization has been denied for this request."}"#
        ));
        assert!(err(r#"{"eventDivsWithEntries": []}"#));
        assert!(serde_json::from_str::<EntriesData>(r#"{"teams": []}"#).is_err());
    }

    #[test]
    fn entries_with_letters_are_relay_teams_whatever_the_event_says() {
        let data = EntriesData {
            event_id: None,
            entries: vec![leg(1, 77, "A1", 1), leg(2, 77, "A2", 1)],
            teams: Vec::new(),
        };
        let e = EventEntries::from_data(&data, false);
        assert!(e.entrants.is_empty());
        assert_eq!(e.teams.len(), 1);
    }

    #[test]
    fn the_school_falls_back_to_the_responses_teams() {
        let r = row(serde_json::json!({"IDAthlete": 1, "SchoolID": 5}));
        let teams = [EntryTeam {
            id: Some(5),
            name: Some("Elm Ridge High".into()),
        }];
        assert_eq!(school_of(&r, &teams), "Elm Ridge High");
        assert_eq!(school_of(&r, &[]), "");
        // A TeamID is another id space: never matched against IDSchool.
        let r = row(serde_json::json!({"IDAthlete": 1, "TeamID": 5}));
        assert_eq!(school_of(&r, &teams), "");
    }

    #[test]
    fn an_odd_team_item_does_not_fail_the_answer() {
        let data: EntriesData = serde_json::from_value(serde_json::json!({
            "entries": [{"IDAthlete": 1, "SchoolID": 5, "AcceptanceStatus": 10}],
            "teams": [{"SchoolName": null}, {"IDSchool": 5, "SchoolName": "Elm Ridge High"}, {}]
        }))
        .unwrap();
        assert_eq!(data.teams.len(), 3);
        let e = EventEntries::from_data(&data, false);
        assert_eq!(e.entrants[0].school, "Elm Ridge High");
    }

    #[test]
    fn legs_without_a_team_id_from_two_schools_never_merge() {
        let leg = |id: i64, school: serde_json::Value, name: &str, letter: &str| {
            row(serde_json::json!({
                "IDAthlete": id, "SchoolID": school, "TeamName": name, "Letter": letter,
                "AcceptanceStatus": 10
            }))
        };
        let rows = [
            leg(1, 41.into(), "Alder High", "A1"),
            leg(2, 42.into(), "Birch High", "A1"),
            leg(3, 41.into(), "Alder High", "A2"),
            leg(4, serde_json::Value::Null, "Cedar High", "A1"),
            leg(5, serde_json::Value::Null, "Dogwood High", "A1"),
            leg(6, 42.into(), "Birch High", "A2"),
        ];
        let teams = relay_teams(rows.iter(), &[]);
        let got: Vec<(&str, Vec<i64>)> = teams
            .iter()
            .map(|t| {
                (
                    t.school.as_str(),
                    t.legs.iter().map(|l| l.athlete_id).collect(),
                )
            })
            .collect();
        assert_eq!(
            got,
            [
                ("Alder High", vec![1, 3]),
                ("Birch High", vec![2, 6]),
                ("Cedar High", vec![4]),
                ("Dogwood High", vec![5]),
            ]
        );
    }

    fn event(id: i64, name: &str, short: &str, gender: &str) -> MeetEvent {
        MeetEvent {
            id,
            name: Some(name.to_string()),
            short: short.to_string(),
            gender: gender.to_string(),
            kind: Some("T".to_string()),
        }
    }

    #[test]
    fn relays_are_told_by_code_or_name() {
        for (name, short) in [
            ("4x800 Relay", "4x800m"),
            ("4x200 Meter Relay", "4x200m"),
            ("Sprint Medley", "SMR 1600m"),
            ("Distance Medley Relay", "DMR"),
            ("Shuttle Hurdle Relay", "SHR"),
            // Seen live on a meet: the code says nothing, the name does.
            ("SMR 1600m", "sprintmed2248"),
            ("4x55 Shuttle Hurdles", "55shuttleh"),
            ("Distance Medley", "dmed4000"),
        ] {
            assert!(event(1, name, short, "M").is_relay(), "{short}");
        }
        for (name, short) in [
            ("800 Meters", "800m"),
            ("1 Mile", "1mile"),
            ("55 Meters", "55m"),
        ] {
            assert!(!event(1, name, short, "M").is_relay(), "{short}");
        }
    }

    #[test]
    fn equal_legs_give_the_seeding_event_by_code_or_name() {
        let shape = |name: &str, short: &str| event(1, name, short, "M").relay_shape();
        let four = |seed: &str| {
            RelayShape::Equal(EqualLegs {
                legs: 4,
                seeding_event: seed.to_string(),
                mile: false,
            })
        };
        // Mile legs seed on the 1600m, with the mile as an Estimated mark.
        let mile = || {
            RelayShape::Equal(EqualLegs {
                legs: 4,
                seeding_event: "1600m".to_string(),
                mile: true,
            })
        };
        assert_eq!(shape("4x100 Relay", "4x100m"), four("100m"));
        assert_eq!(shape("4x200 Meter Relay", "4x200m"), four("200m"));
        assert_eq!(shape("4x400 Relay", "4x400R"), four("400m"));
        assert_eq!(shape("4x800 Relay", "4x800m"), four("800m"));
        assert_eq!(shape("4x1600 Relay", "4x1600m"), four("1600m"));
        assert_eq!(shape("4x800 Meters Relay", "x"), four("800m"));
        // A mile leg seeds on the 1600m (docs/HANDOFF.md, Relays).
        assert_eq!(shape("4xMile Relay", "4xmile"), mile());
        assert_eq!(shape("4x1 Mile Relay", "x"), mile());
        assert_eq!(shape("4x1mi", "x"), mile());
        // The name decides when the code says nothing; a standard flat race
        // reads without its unit.
        assert_eq!(shape("4x800 Relay", "relay39"), four("800m"));
        assert_eq!(
            shape("3x400 Relay", "3x400m"),
            RelayShape::Equal(EqualLegs {
                legs: 3,
                seeding_event: "400m".to_string(),
                mile: false,
            })
        );
        for (name, short) in [
            ("SMR 1600m", "sprintmed2248"),
            ("Sprint Medley", "SMR 1600m"),
            ("Distance Medley Relay", "DMR"),
            ("4x55 Shuttle Hurdles", "55shuttleh"),
            ("4x100 Shuttle Hurdle Relay", "4x100mH"),
            ("4x110 Hurdle Relay", "4x110h"),
        ] {
            assert_eq!(shape(name, short), RelayShape::Mixed, "{name} {short}");
        }
        // A bare number that is no standard race, a hurdles code, and yards
        // are not leg distances.
        for (name, short) in [
            ("4x4", "4x4"),
            ("4x8", "4x8"),
            ("4x1 Relay", "4x1"),
            ("4x200 Yard Relay", "4x200yd"),
            ("4x100mh", "4x100mh"),
            ("Relay", "relay1"),
        ] {
            assert_eq!(shape(name, short), RelayShape::Unread, "{name} {short}");
        }
        for (name, short) in [("800 Meters", "800m"), ("1 Mile", "1mile")] {
            assert_eq!(shape(name, short), RelayShape::NotNamed, "{name} {short}");
        }
    }

    #[test]
    fn a_relay_is_refused_by_its_cause() {
        let at = |name: &str, short: &str| {
            let e = event(39, name, short, "M");
            let div = Division {
                id: 13,
                name: "Varsity".into(),
                short: None,
            };
            (EventRef::new(7, Gender::M, &div, &e), e.relay_shape())
        };
        let refused = |name: &str, short: &str| {
            let (r, shape) = at(name, short);
            relay_seedable(&r, &shape).unwrap_err().to_string()
        };
        let teams = "`ath meet 7 --gender m --div 13 --event";
        let msg = refused("SMR 1600m", "sprintmed2248");
        assert!(
            msg.contains("not all one flat race (a medley or shuttle"),
            "{msg}"
        );
        assert!(msg.contains(teams), "{msg}");
        let msg = refused("4x200 Yard Relay", "4x200yd");
        assert!(
            msg.starts_with(
                "4x200 Yard Relay (4x200yd) is a relay, but its leg distance could not be read \
                 from \"4x200 Yard Relay\" or \"4x200yd\""
            ),
            "{msg}"
        );
        let msg = refused("4x4", "4x4");
        assert!(msg.contains("leg distance could not be read"), "{msg}");
        let msg = refused("800 Meters", "800m");
        assert!(
            msg.starts_with("800 Meters (800m) has relay entries (legs lettered A1 to A4)"),
            "{msg}"
        );
        let msg = refused("3x400 Relay", "3x400m");
        assert!(
            msg.contains("is a relay of 3 legs, and ath seed seeds relays of 4 legs only"),
            "{msg}"
        );
        let (r, shape) = at("4x800 Relay", "4x800m");
        assert_eq!(relay_seedable(&r, &shape).unwrap().seeding_event, "800m");
    }

    #[test]
    fn an_indoor_only_seeding_event_is_refused_naming_where_it_came_from() {
        let div = Division {
            id: 13,
            name: "Varsity".into(),
            short: None,
        };
        let e = event(903, "4x55 Meter Relay", "4x55m", "M");
        let r = EventRef::new(7, Gender::M, &div, &e);
        let RelayShape::Equal(legs) = e.relay_shape() else {
            panic!("equal legs");
        };
        let derived = seeding_event(&r, &legs, None).unwrap_err().to_string();
        assert_eq!(
            derived,
            "4x55 Meter Relay (4x55m)'s legs are 55m, a distance only run indoors, and ath seeds \
             on outdoor Personal records, so its runners have no marks to seed on"
        );
        assert!(!derived.contains("--seed-on"), "{derived}");
        // --seed-on, when given, replaces the legs, and is what is refused.
        assert_eq!(seeding_event(&r, &legs, Some("100m")).unwrap(), "100m");
        let given = seeding_event(&r, &legs, Some("600m"))
            .unwrap_err()
            .to_string();
        assert!(
            given.starts_with("--seed-on \"600m\" is only run indoors"),
            "{given}"
        );
    }

    #[test]
    fn seed_on_takes_only_an_individual_flat_race() {
        let div = Division {
            id: 13,
            name: "Varsity".into(),
            short: None,
        };
        let e = event(39, "4x800 Relay", "4x800m", "M");
        let r = EventRef::new(7, Gender::M, &div, &e);
        let shape = e.relay_shape();
        let seed = |on: Option<&str>| relay_seeding(&r, &shape, on);
        for ok in ["1600m", "1600 Meters", "1 Mile", "Mile", "400m"] {
            assert_eq!(seed(Some(ok)).unwrap().event, ok, "{ok}");
        }
        let err = |on: &str| seed(Some(on)).unwrap_err().to_string();
        assert!(
            err("Shot Put").starts_with("--seed-on \"Shot Put\" is not a flat running race"),
            "{}",
            err("Shot Put")
        );
        assert!(err("110mH").contains("is not a flat running race"));
        assert!(err("4x400m").starts_with("--seed-on \"4x400m\" is a relay; it takes one"));
        assert!(err("SMR 1600m").contains("is a relay"));
        // The relay's own legs; a 4xMile reads the mile only without
        // --seed-on.
        assert_eq!(
            seed(None).unwrap(),
            RelaySeeding {
                event: "800m".into(),
                mile: false
            }
        );
        let m = event(40, "4xMile Relay", "4xmile", "F");
        let rm = EventRef::new(7, Gender::F, &div, &m);
        let mile_shape = m.relay_shape();
        let mile = |on| relay_seeding(&rm, &mile_shape, on).unwrap();
        assert_eq!(
            mile(None),
            RelaySeeding {
                event: "1600m".into(),
                mile: true
            }
        );
        assert!(!mile(Some("1600m")).mile);
    }

    #[test]
    fn relay_names_are_told_alone() {
        for t in [
            "4x800m",
            "4xMile",
            "4x800 Relay",
            "SMR 1600m",
            "DMR",
            "Sprint Medley",
        ] {
            assert!(is_relay_name(t), "{t}");
        }
        for t in ["800m", "800 Meters", "1 Mile", "110mH"] {
            assert!(!is_relay_name(t), "{t}");
        }
    }

    #[test]
    fn indoor_only_events_are_told_by_code_or_name() {
        for (name, short) in [
            ("55 Meters", "55m"),
            ("55 Meter Hurdles", "55mh"),
            ("60 Meters", "60m"),
            ("60 Meter Hurdles", "60mH"),
            ("300 Meters", "300m"),
            ("500 Meters", "500m"),
            ("600 Meters", "600m"),
            ("1000 Meters", "1000m"),
            ("4x55 Shuttle Hurdles", "55shuttleh"),
            // A name alone is enough.
            ("600 Meters", "x600"),
        ] {
            assert!(
                event(1, name, short, "M").is_indoor_only(),
                "{name} {short}"
            );
        }
        for (name, short) in [
            ("100 Meters", "100m"),
            ("300 Meter Hurdles", "300mH"),
            ("400 Meters", "400m"),
            ("800 Meters", "800m"),
            ("1 Mile", "1mile"),
            ("1600 Meters", "1600m"),
            ("4x400 Relay", "4x400m"),
            ("Shot Put", "Shot"),
        ] {
            assert!(
                !event(1, name, short, "M").is_indoor_only(),
                "{name} {short}"
            );
        }
    }

    fn list() -> MeetEventList {
        MeetEventList {
            events: vec![
                event(4, "800 Meters", "800m", "M"),
                event(22, "800 Meters", "800m", "F"),
                event(58, "1 Mile", "1mile", "M"),
                event(39, "4x800 Relay", "4x800m", "M"),
            ],
            with_entries: vec![EventDiv { e: 4, d: 13 }],
        }
    }

    #[test]
    fn events_are_found_by_code_or_name_for_the_gender() {
        let l = list();
        let id = |g, q| find_event(&l, 1, g, q).map(|e| e.id).ok();
        assert_eq!(id(Gender::M, "800m"), Some(4));
        assert_eq!(id(Gender::F, "800m"), Some(22));
        assert_eq!(id(Gender::M, "800 Meters"), Some(4));
        assert_eq!(id(Gender::M, "1 mile"), Some(58));
        assert_eq!(id(Gender::M, "1Mile"), Some(58));
        assert_eq!(id(Gender::M, "4X800M"), Some(39));
        let err = find_event(&l, 1, Gender::F, "1mile")
            .unwrap_err()
            .to_string();
        assert!(err.contains("no girls' event \"1mile\""), "{err}");
        assert!(err.contains("are 800m"), "{err}");
    }

    fn meet() -> MeetData {
        serde_json::from_value(serde_json::json!({
            "meet": {"ID": 7, "Name": "Test Invite", "MeetDate": "2099-01-10T00:00:00",
                     "StartDate": "2099-01-09T00:00:00", "SeasonID": 12099},
            "tfDivisions": [
                {"IDMeetDiv": 1, "MeetID": 7, "IDDiv": 13, "Division": "Varsity", "DivisionShort": "V"},
                {"IDMeetDiv": 2, "MeetID": 7, "IDDiv": 14, "Division": "Junior Varsity", "DivisionShort": "JV"}
            ]
        }))
        .unwrap()
    }

    #[test]
    fn divisions_are_found_by_id_name_or_short_name() {
        let m = meet();
        let id = |q| find_division(&m, q).map(|d| d.id).ok();
        assert_eq!(id("13"), Some(13));
        assert_eq!(id("varsity"), Some(13));
        assert_eq!(id("V"), Some(13));
        assert_eq!(id("jv"), Some(14));
        assert_eq!(id("junior varsity"), Some(14));
        let err = find_division(&m, "99").unwrap_err().to_string();
        assert!(
            err.contains(
                "no division \"99\"; its divisions are Varsity (V, 13), Junior Varsity (JV, 14)"
            ),
            "{err}"
        );
    }

    #[test]
    fn a_meet_starts_on_its_earliest_date_and_knows_its_season() {
        let m = meet();
        assert_eq!(m.meet.date(), "2099-01-09".parse().ok());
        assert_eq!(m.meet.end(), "2099-01-10".parse().ok(), "a two-day meet");
        assert!(m.meet.indoor(), "an indoor season without a location");
        assert_eq!(season_label(12027), "2027 Indoor");
        assert_eq!(season_label(2027), "2027 Outdoor");
    }

    #[test]
    fn the_report_lists_events_with_entries_by_gender_then_division() {
        let mut l = list();
        l.with_entries = vec![
            EventDiv { e: 39, d: 13 },
            EventDiv { e: 4, d: 14 },
            EventDiv { e: 22, d: 13 },
            EventDiv { e: 4, d: 13 },
        ];
        let r = MeetReport::new(&meet(), &l);
        let rows: Vec<(Gender, i64, &str)> = r
            .events
            .iter()
            .map(|e| (e.gender, e.division_id, e.event_short.as_str()))
            .collect();
        assert_eq!(
            rows,
            [
                (Gender::F, 13, "800m"),
                (Gender::M, 13, "800m"),
                (Gender::M, 13, "4x800m"),
                (Gender::M, 14, "800m"),
            ]
        );
        assert!(r.events[2].relay);
        assert_eq!(
            r.events[0].url,
            "https://www.athletic.net/TrackAndField/meet/7/entries/f/13/800m"
        );
    }
}
