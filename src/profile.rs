//! `ath profile`: a person's Profile - name, gender, teams, school and Area -
//! and, for one event, their Season best and Personal record as of a date.
//!
//! Both bests are computed from the Profile's results ([`bests_as_of`]), never
//! read from Athletic.net's `SeasonBest` / `PersonalBest` flags (`CONTEXT.md`).
//! The Area is the one Athletic.net lists the Profile's high school in
//! ([`profile_area`]).

use anyhow::Result;
use chrono::{Datelike, NaiveDate};
use serde::Serialize;

use crate::area::{AreaLookup, AreaSummary, DEFAULT_LINK_EVENT, flat_race_short, profile_area};
use crate::contract::{Report, columns};
use crate::mark::{
    AsOf, NEVER_COUNT, bests_as_of, is_field_mark, resolve_event, strip_timing_suffix,
};
use crate::model::{Bio, ResultRow, Team, athlete_url};
use crate::source::DataSource;

/// What to show for a Profile beyond the person: one event's bests.
#[derive(Debug, Clone, Copy)]
pub struct ProfileQuery<'a> {
    /// The event, by Athletic.net name or a flat race's `400m` short form.
    pub event: Option<&'a str>,
    /// Count marks set on or before this date; `None` means `today`.
    pub as_of: Option<NaiveDate>,
    /// The Season best's season; `None` means the as-of date's year (right for
    /// outdoor; an indoor season spans the new year).
    pub season: Option<i64>,
    /// Today's date. A result with no `ResultDate` counts only when the as-of
    /// date is today or later, since it can only have been set by today.
    pub today: NaiveDate,
}

impl ProfileQuery<'_> {
    /// No event, as of today.
    pub fn today(today: NaiveDate) -> Self {
        ProfileQuery {
            event: None,
            as_of: None,
            season: None,
            today,
        }
    }

    /// The as-of date: `as_of`, or today.
    pub fn as_of(&self) -> NaiveDate {
        self.as_of.unwrap_or(self.today)
    }

    /// The season the Season best and the Area are for: `season`, or the
    /// as-of date's year.
    pub fn season(&self) -> i64 {
        self.season
            .unwrap_or_else(|| i64::from(self.as_of().year()))
    }
}

/// Fetch a Profile and its Area, and summarize them (see [`build_profile`]).
pub async fn profile<S: DataSource + ?Sized>(
    source: &S,
    athlete_id: i64,
    query: &ProfileQuery<'_>,
) -> Result<Profile> {
    let bio = source.get_bio(athlete_id).await?;
    // The Area link opens on the asked-for event when it is a flat race.
    let link_event = query
        .event
        .and_then(flat_race_short)
        .unwrap_or_else(|| DEFAULT_LINK_EVENT.to_string());
    let area = profile_area(source, &bio, query.season(), &link_event).await;
    Ok(build_profile(&bio, query, area))
}

/// A Profile as `ath profile` prints it.
#[derive(Debug, Clone, Serialize)]
pub struct Profile {
    pub athlete_id: i64,
    pub name: String,
    /// `F` or `M`, as Athletic.net sends it.
    pub gender: Option<String>,
    /// The Athletic.net Profile page.
    pub url: String,
    /// The school for the query's season (`Bio::school_in`): the one the
    /// bio's `grades` lists for the season, otherwise the school whose last
    /// listed year is the earliest at or after the season, otherwise the
    /// current school. The same school decides the Area.
    pub school: Option<TeamSummary>,
    /// Every team the person has been affiliated with: schools first, then
    /// clubs, most recent first.
    pub teams: Vec<TeamSummary>,
    /// The Area Athletic.net lists the Profile's high school in for the
    /// season; `null` when there is none (`area_lookup` says why).
    pub area: Option<AreaSummary>,
    /// How the Area was looked up: its status, season, school and State.
    pub area_lookup: AreaLookup,
    /// Season best and Personal record for `--event`; `null` without it.
    pub event: Option<EventBests>,
}

/// One team affiliation, classified by Athletic.net's team `Level`.
#[derive(Debug, Clone, Serialize)]
pub struct TeamSummary {
    pub id: i64,
    pub name: String,
    pub kind: TeamKind,
    /// Athletic.net's team level: 2 middle school, 4 high school, 16 club.
    pub level: i64,
    pub year: Option<i64>,
    pub url: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TeamKind {
    School,
    Club,
    Other,
}

/// Season best and Personal record in one event, as of a date.
#[derive(Debug, Clone, Serialize)]
pub struct EventBests {
    /// The event's Athletic.net name (as given, when the Profile has no such
    /// event).
    pub name: String,
    /// Whether the Profile lists this event at all (it may still have no
    /// Qualifying mark in it; see `status`).
    pub on_profile: bool,
    /// Whether there is a Personal record, and if not, why.
    pub status: EventStatus,
    pub as_of: NaiveDate,
    /// The season the Season best is limited to (the as-of date's year unless
    /// `--season` overrides it).
    pub season: i64,
    pub season_best: Option<MarkSummary>,
    pub personal_record: Option<MarkSummary>,
}

/// Whether an event has a Qualifying mark on or before the as-of date.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EventStatus {
    /// There is a Personal record (the Season best may still be empty).
    Ok,
    /// The Profile has no results in this event.
    NotOnProfile,
    /// A field event: v1 reads running events only.
    FieldEvent,
    /// Results exist, but none is a Qualifying mark on or before the as-of
    /// date (relay splits, exhibition, hand-timed, DNS/DNF, or later marks).
    NoQualifyingMark,
}

/// One Qualifying mark.
#[derive(Debug, Clone, Serialize)]
pub struct MarkSummary {
    /// The display mark, FAT `a` suffix stripped (`56.85`).
    pub mark: String,
    /// Athletic.net's numeric sort key; lower is better.
    pub sort_int: i64,
    pub date: Option<NaiveDate>,
    pub season: i64,
    pub meet: Option<String>,
    pub meet_id: Option<i64>,
}

/// Summarize `bio` with its looked-up Area; with an event, include its Season
/// best and Personal record as of the query's date.
pub fn build_profile(
    bio: &Bio,
    query: &ProfileQuery<'_>,
    (area, area_lookup): (Option<AreaSummary>, AreaLookup),
) -> Profile {
    let mut teams: Vec<TeamSummary> = bio.all_teams.values().map(team_summary).collect();
    teams.sort_by(|a, b| {
        (a.kind, std::cmp::Reverse(a.year), &a.name, a.id).cmp(&(
            b.kind,
            std::cmp::Reverse(b.year),
            &b.name,
            b.id,
        ))
    });
    // The same rule (and season) that picks the school deciding the Area.
    let school = bio.school_in(query.season()).map(|p| team_summary(p.team));

    Profile {
        athlete_id: bio.athlete.id,
        name: bio.athlete.full_name(),
        gender: bio.athlete.gender.clone(),
        url: athlete_url(bio.athlete.id),
        school,
        teams,
        area,
        area_lookup,
        event: query.event.map(|event| event_bests(bio, event, query)),
    }
}

fn team_summary(t: &Team) -> TeamSummary {
    TeamSummary {
        id: t.id,
        name: t.name.clone(),
        kind: if t.is_school() {
            TeamKind::School
        } else if t.is_club() {
            TeamKind::Club
        } else {
            TeamKind::Other
        },
        level: t.level,
        year: t.year,
        url: t.url(),
    }
}

fn event_bests(bio: &Bio, event: &str, query: &ProfileQuery<'_>) -> EventBests {
    let as_of = query.as_of();
    let season = query.season();
    let resolved = resolve_event(bio, event);
    let ids = resolved.as_ref().map(|e| e.ids.clone()).unwrap_or_default();
    let bests = bests_as_of(
        bio,
        &ids,
        season,
        AsOf {
            date: as_of,
            today: query.today,
        },
    );
    let field = resolved.as_ref().is_some_and(|e| {
        e.field
            || bio.results_tf.iter().any(|r| {
                r.athlete_id == bio.athlete.id && e.ids.contains(&r.event_id) && is_field_mark(r)
            })
    });
    let status = match (&resolved, bests.personal_record) {
        (_, Some(_)) => EventStatus::Ok,
        (None, None) => EventStatus::NotOnProfile,
        (Some(_), None) if field => EventStatus::FieldEvent,
        (Some(_), None) => EventStatus::NoQualifyingMark,
    };
    EventBests {
        name: resolved.map_or_else(|| event.trim().to_string(), |e| e.name),
        on_profile: !ids.is_empty(),
        status,
        as_of,
        season,
        season_best: bests.season_best.map(|r| mark_summary(bio, r)),
        personal_record: bests.personal_record.map(|r| mark_summary(bio, r)),
    }
}

fn mark_summary(bio: &Bio, r: &ResultRow) -> MarkSummary {
    MarkSummary {
        mark: strip_timing_suffix(&r.result).to_string(),
        sort_int: r.sort_int,
        date: r.date(),
        season: r.season_id,
        meet: bio.meet_name(r),
        meet_id: r.meet_id,
    }
}

impl Report for Profile {
    const SCHEMA: u32 = 1;

    fn table(&self) -> String {
        let none = || "none".to_string();
        let mut head = vec![
            vec!["Name".into(), self.name.clone()],
            vec!["Gender".into(), self.gender.clone().unwrap_or_else(none)],
            vec![
                "School".into(),
                self.school
                    .as_ref()
                    .map(|t| t.name.clone())
                    .unwrap_or_else(none),
            ],
        ];
        head.extend(self.area_rows());
        head.push(vec!["Profile".into(), self.url.clone()]);
        let mut out = columns(&head);

        out.push('\n');
        let mut rows = vec![vec![
            "Team".to_string(),
            "Kind".into(),
            "Year".into(),
            "URL".into(),
        ]];
        rows.extend(self.teams.iter().map(|t| {
            vec![
                t.name.clone(),
                t.kind.as_str().into(),
                t.year.map(|y| y.to_string()).unwrap_or_default(),
                t.url.clone(),
            ]
        }));
        if self.teams.is_empty() {
            rows.push(vec!["(no teams)".into()]);
        }
        out.push_str(&columns(&rows));

        if let Some(e) = &self.event {
            out.push('\n');
            out.push_str(&format!(
                "{} as of {} (season {})\n",
                e.name, e.as_of, e.season
            ));
            if let Some(note) = e.status.note() {
                out.push_str(&note);
                out.push('\n');
            }
            let mut rows = vec![vec![
                String::new(),
                "Mark".into(),
                "SortInt".into(),
                "Date".into(),
                "Meet".into(),
            ]];
            for (label, m) in [
                ("Season best", &e.season_best),
                ("Personal record", &e.personal_record),
            ] {
                rows.push(match m {
                    Some(m) => vec![
                        label.into(),
                        m.mark.clone(),
                        m.sort_int.to_string(),
                        m.date.map(|d| d.to_string()).unwrap_or_default(),
                        m.meet.clone().unwrap_or_default(),
                    ],
                    None => vec![label.into(), none()],
                });
            }
            out.push_str(&columns(&rows));
        }
        out
    }
}

impl Profile {
    /// The table's Area row: the Area with its list page, or why there is none.
    fn area_rows(&self) -> Vec<Vec<String>> {
        let season = self.area_lookup.season;
        let note = self.area_lookup.note.as_deref();
        match &self.area {
            // One cell, so the URL does not widen the column the Profile URL
            // shares.
            Some(a) => {
                let mut rows = vec![vec![
                    "Area".into(),
                    format!("{} ({} {}, {season})  {}", a.name, a.state, a.level, a.url),
                ]];
                rows.extend(note.map(|n| vec![String::new(), format!("({n})")]));
                rows
            }
            None => vec![vec![
                "Area".into(),
                format!("none for {season} - {}", note.unwrap_or("not found")),
            ]],
        }
    }
}

impl TeamKind {
    /// The same word the JSON uses.
    pub fn as_str(self) -> &'static str {
        match self {
            TeamKind::School => "school",
            TeamKind::Club => "club",
            TeamKind::Other => "other",
        }
    }
}

impl EventStatus {
    /// Why there is no Personal record, for people; `None` when there is one.
    pub fn note(self) -> Option<String> {
        match self {
            EventStatus::Ok => None,
            EventStatus::NotOnProfile => Some("This Profile has no results in this event.".into()),
            EventStatus::FieldEvent => {
                Some("This is a field event; ath v1 supports running events only.".into())
            }
            EventStatus::NoQualifyingMark => Some(format!(
                "No Qualifying mark on or before this date ({NEVER_COUNT})."
            )),
        }
    }
}
