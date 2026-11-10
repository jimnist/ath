use std::ffi::OsString;
use std::io::IsTerminal;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::error::{ContextKind, ContextValue, ErrorKind};
use clap::{ArgGroup, Args, CommandFactory, Parser, Subcommand};

use ath::area::{
    AreaStatus, AreasQuery, DEFAULT_FROM_LIST, DEFAULT_LINK_EVENT, Gender, areas, link_event,
    refuse_bare_number,
};
use ath::athletic::AthleticNet;
use ath::cache::{CacheMode, ResponseCache};
use ath::contract::{Format, emit};
use ath::http::{DEFAULT_USER_AGENT, ReqwestFetcher};
use ath::list::{Grades, ListQuery, Which, rankings_list};
use ath::meet::{
    EventSelect, MeetRef, RELAY_LEGS, RelaySeeding, RelayTeam, is_relay_name, meet_report,
    meet_resolve, read_event, relay_seeding, seedable_on,
};
use ath::output::{write_relay_csv, write_resolution_log, write_seed_csv, write_unresolved_csv};
use ath::profile::{EventStatus, ProfileQuery, profile};
use ath::rank::{Ahead, RankQuery, RankStatus, Targets, rank_with_progress};
use ath::record::Recorder;
use ath::relay::{self, RelayConfig, RelayFetcher};
use ath::replay::FixtureFetcher;
use ath::resolve::{ResolveOutcome, resolve};
use ath::roster::{RosterRow, apply_resolved_ids, read_roster};
use ath::seed::{
    FetchFailure, FetchOpts, SeedDate, SeedOutcome, SeedSheet, SheetMeet, not_resolved, seed,
};
use ath::seed_relay::{RelayOutcome, RelaySheet, seed_relay};
use ath::source::{DataSource, FixtureSource};

/// Generate meet seed sheets from Athletic.net.
///
/// See `docs/HANDOFF.md` for the full spec (data sources, encodings, filters).
#[derive(Parser, Debug)]
#[command(
    name = "ath",
    version,
    about,
    long_about = None,
    subcommand_required = true,
    arg_required_else_help = true
)]
struct Cli {
    #[command(flatten)]
    global: GlobalArgs,

    #[command(subcommand)]
    command: Command,
}

/// Transport and cache flags every subcommand shares. They are `global`, so they
/// are accepted before or after the subcommand name.
#[derive(Args, Debug)]
#[command(next_help_heading = "Global options")]
struct GlobalArgs {
    /// Athletic.net session cookie: fetch with it (the cookie transport) instead
    /// of through the browser relay. Also read from the `ATH_COOKIE` env var.
    // Not clap's `env =`: clap counts an env-filled flag as given, so plain
    // `ath` with ATH_COOKIE exported would error instead of showing help.
    // `resolve_cookie` reads the env var instead.
    #[arg(long, global = true, value_name = "COOKIE")]
    cookie: Option<String>,

    /// Read the session cookie from a file instead of `--cookie`.
    #[arg(long, global = true, value_name = "FILE")]
    cookie_file: Option<String>,

    /// User-Agent to send with the cookie. Must match the browser the cookie
    /// came from (`cf_clearance` is bound to it).
    #[arg(long, global = true, value_name = "UA")]
    user_agent: Option<String>,

    /// Max bios fetched concurrently (`ath list` reads its pages one at a time).
    #[arg(long, global = true, default_value_t = 5)]
    concurrency: usize,

    /// Delay between concurrent batches of bios, and between rankings list
    /// pages, in milliseconds.
    #[arg(long, global = true, default_value_t = 250)]
    delay_ms: u64,

    /// Ignore cached responses and refetch (still writes fresh entries to the cache).
    #[arg(long, global = true)]
    refresh: bool,

    /// Do not read or write the response cache.
    #[arg(long, global = true)]
    no_cache: bool,

    /// Response cache directory. Defaults to the OS cache dir under `ath/responses`.
    #[arg(long, global = true, value_name = "DIR")]
    cache_dir: Option<String>,

    /// Print JSON (the default when stdout is not a terminal).
    #[arg(long, global = true, overrides_with = "table")]
    json: bool,

    /// Print a readable table (the default when stdout is a terminal).
    #[arg(long, global = true, overrides_with = "json")]
    table: bool,

    /// Port on 127.0.0.1 for the browser relay (the bookmarklet is made for it).
    #[arg(long, global = true, value_name = "PORT", default_value_t = relay::DEFAULT_PORT)]
    relay_port: u16,

    /// Seconds to wait for the Athletic.net tab to connect to the browser relay
    /// (1 to 3600).
    #[arg(
        long,
        global = true,
        value_name = "SECONDS",
        default_value_t = relay::DEFAULT_TIMEOUT.as_secs(),
        value_parser = clap::value_parser!(u64).range(1..=3600)
    )]
    relay_timeout: u64,

    /// Also write every Athletic.net response this run reads (cached ones
    /// too) into DIR, in the layout `--fixtures-dir` reads, so the same
    /// command with `--fixtures-dir DIR` replays it offline. No tokens are
    /// written. Real people: DIR must be outside any git work tree, or one its
    /// git ignores (such as `local/recordings/<scenario>`). See
    /// docs/anonymize.md.
    #[arg(long, global = true, value_name = "DIR")]
    record_dir: Option<String>,

    /// Testing: answer Athletic.net requests from a fixtures dir instead of
    /// the network, through the live data source (cache, recording).
    #[arg(long, global = true, value_name = "DIR", hide = true)]
    fixture_transport: Option<String>,
}

/// One variant per noun command (ADR 0007). Add a variant + a `run_*` fn and
/// a dispatch arm in `main` to add a command.
#[derive(Subcommand, Debug)]
enum Command {
    /// Build a ranked seed sheet CSV for one event from a roster CSV or a
    /// meet's entries, and print it.
    Seed(SeedArgs),
    /// Show a meet: its date, season, divisions and the events with entries,
    /// or with --event one event's entries (relays by team and leg).
    Meet(MeetArgs),
    /// Show a Profile, with its Area and its Season best and Personal record in
    /// one event.
    Profile(ProfileArgs),
    /// List a State's Areas for a season, with the list id of each and of the
    /// State.
    Areas(AreasArgs),
    /// Print a whole Rankings list (a State's or an Area's) for one event,
    /// season and Grade group.
    List(ListArgs),
    /// Place an Athlete on State and Area Rankings lists, for each Grade
    /// group, with the people around them.
    Rank(RankArgs),
    /// Set up the browser relay that fetches Athletic.net through your browser.
    Relay(RelayArgs),
}

#[derive(Args, Debug)]
struct ProfileArgs {
    /// Athletic.net athlete id (the number in the Profile URL).
    #[arg(value_name = "ATHLETE_ID")]
    athlete_id: i64,

    /// Event to show the Season best and Personal record for, by Athletic.net
    /// name (e.g. "400 Meters"); a flat race also takes the short form "400m".
    #[arg(long, value_name = "NAME")]
    event: Option<String>,

    /// Compute both bests from marks set on or before this date; the Season
    /// best and the Area are for this date's season. Defaults to today.
    #[arg(long, value_name = "YYYY-MM-DD")]
    as_of: Option<chrono::NaiveDate>,

    /// Outdoor season (year) for the School, the Area and the Season best.
    /// Defaults to the as-of date's year. Indoor marks never count in v1, so
    /// indoor season ids (10000 and up) are refused.
    #[arg(long, value_name = "YEAR",
          value_parser = clap::value_parser!(i64).range(1..ath::mark::INDOOR_SEASON_BASE))]
    season: Option<i64>,

    /// Offline mode: read recorded responses from DIR (`<athlete_id>.json`,
    /// `teamnav/<school_id>-<season>.json`, `navinfo/<season>.json`) instead
    /// of the live API (skips the cookie, the relay and the network).
    #[arg(long, value_name = "DIR")]
    fixtures_dir: Option<String>,
}

#[derive(Args, Debug)]
struct AreasArgs {
    /// Two-letter State code (e.g. CA).
    #[arg(value_name = "STATE")]
    state: String,

    /// Season (year). Defaults to this year.
    #[arg(long, value_name = "YEAR")]
    season: Option<i64>,

    /// Gender of the rankings list links (list ids are the same for both).
    #[arg(long, value_enum, value_name = "GENDER", default_value_t = Gender::F)]
    gender: Gender,

    /// Event the rankings list links open on: a flat race ("400m" or "400
    /// Meters") or an Athletic.net event code such as Shot or LJ.
    #[arg(long, value_name = "EVENT", default_value = DEFAULT_LINK_EVENT)]
    event: String,

    /// Rankings list (in any season) to find the season's lists from; only
    /// its token is used.
    #[arg(long, value_name = "LIST_ID", default_value_t = DEFAULT_FROM_LIST)]
    from_list: i64,

    /// Offline mode: read recorded responses from DIR (`navinfo/<season>.json`)
    /// instead of the live API (skips the cookie, the relay and the network).
    #[arg(long, value_name = "DIR")]
    fixtures_dir: Option<String>,
}

#[derive(Args, Debug)]
#[command(group(ArgGroup::new("which").required(true).multiple(true).args(["state", "area", "list"])))]
struct ListArgs {
    /// A State's list, by two-letter code (e.g. CA). With --area, the State
    /// the Area is in.
    #[arg(long, value_name = "STATE")]
    state: Option<String>,

    /// An Area's list, by name or abbreviation, any case ("Central Coast" or
    /// CCS).
    #[arg(long, value_name = "AREA")]
    area: Option<String>,

    /// Any rankings list by id (as `ath areas` prints them). It must be in
    /// --season.
    #[arg(long, value_name = "LIST_ID", conflicts_with_all = ["state", "area"])]
    list: Option<i64>,

    /// Event: a flat race ("400m" or "400 Meters") or an Athletic.net event
    /// code such as Shot or LJ.
    #[arg(long, value_name = "EVENT")]
    event: String,

    /// The girls' (f) or boys' (m) list.
    #[arg(long, value_enum, value_name = "GENDER")]
    gender: Gender,

    /// Outdoor season (year). Defaults to this year.
    #[arg(long, value_name = "YEAR",
          value_parser = clap::value_parser!(i64).range(1..ath::mark::INDOOR_SEASON_BASE))]
    season: Option<i64>,

    /// Grade group: "9-10", "9,11" or "11" (grades 9 to 12). Every grade when
    /// left out.
    #[arg(long, value_name = "GRADES", value_parser = Grades::parse)]
    grades: Option<Grades>,

    /// Only the first N rows: stops reading pages once it has them.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u64).range(1..))]
    limit: Option<u64>,

    /// Rankings list (in any season) to find the season's State and Area
    /// lists from; only its token is used.
    #[arg(long, value_name = "LIST_ID", default_value_t = DEFAULT_FROM_LIST)]
    from_list: i64,

    /// Offline mode: read recorded responses from DIR
    /// (`rankings/<list_id>-<gender>-<event>-<grades>-p<page>.json`,
    /// `navinfo/<season>.json`) instead of the live API (skips the cookie, the
    /// relay and the network).
    #[arg(long, value_name = "DIR")]
    fixtures_dir: Option<String>,
}

#[derive(Args, Debug)]
struct RankArgs {
    /// Athletic.net athlete id (the number in the Profile URL).
    #[arg(value_name = "ATHLETE_ID")]
    athlete_id: i64,

    /// Event: a flat race ("400m" or "400 Meters") or an Athletic.net event
    /// code such as Shot or LJ.
    #[arg(long, value_name = "EVENT")]
    event: String,

    /// Outdoor season (year). Defaults to this year.
    #[arg(long, value_name = "YEAR",
          value_parser = clap::value_parser!(i64).range(1..ath::mark::INDOOR_SEASON_BASE))]
    season: Option<i64>,

    /// The lists to place the Athlete on, comma-separated: State codes and
    /// Area names or abbreviations ("CA,CCS,TX"), `all` for every State and
    /// DC, `areas` for every Area of the home State, `areas:CA` for a State's,
    /// `home` for the home State and Area. Defaults to `home`.
    #[arg(long = "in", value_name = "LISTS", value_parser = Targets::parse)]
    lists: Option<Targets>,

    /// The Athlete's grade this season, when their Profile does not say it.
    #[arg(long, value_name = "GRADE", value_parser = clap::value_parser!(i64).range(9..=12))]
    grade: Option<i64>,

    /// The Athlete's home State (two-letter code), whose list their mark is
    /// read from, when their school does not say.
    #[arg(long, value_name = "STATE")]
    home: Option<String>,

    /// Who each list's Neighborhood shows ahead of the Athlete: everyone at
    /// rank 50 or better, past it the leader and the 3 just ahead
    /// (neighborhood), or everyone at any rank (all). Same pages read either
    /// way.
    #[arg(long, value_enum, value_name = "WHO", default_value_t = Ahead::Neighborhood)]
    ahead: Ahead,

    /// Rankings list (in any season) to find the season's State and Area
    /// lists from; only its token is used.
    #[arg(long, value_name = "LIST_ID", default_value_t = DEFAULT_FROM_LIST)]
    from_list: i64,

    /// Offline mode: read recorded responses from DIR (`<athlete_id>.json`,
    /// `teamnav/`, `navinfo/` and `rankings/`) instead of the live API (skips
    /// the cookie, the relay and the network).
    #[arg(long, value_name = "DIR")]
    fixtures_dir: Option<String>,
}

#[derive(Args, Debug)]
struct RelayArgs {
    #[command(subcommand)]
    command: RelayCommand,
}

#[derive(Subcommand, Debug)]
enum RelayCommand {
    /// Print the bookmarklet that connects an Athletic.net tab to ath, with setup steps.
    Install(RelayInstallArgs),
}

#[derive(Args, Debug)]
struct RelayInstallArgs {
    /// Print only the script, to paste into the tab's DevTools console.
    #[arg(long)]
    console: bool,
}

#[derive(Args, Debug)]
struct MeetArgs {
    /// Athletic.net meet id (the number in the meet's URL).
    #[arg(value_name = "MEET_ID")]
    meet_id: i64,

    /// With --div and --event: the girls' (f) or boys' (m) event.
    #[arg(long, value_enum, value_name = "GENDER", requires_all = ["div", "event"])]
    gender: Option<Gender>,

    /// With --gender and --event: the division, by id (as the entries page's
    /// URL has it), name or short name ("13", "Varsity" or "V").
    #[arg(long, value_name = "DIV", requires_all = ["gender", "event"])]
    div: Option<String>,

    /// With --gender and --div: show this event's entries, by its code
    /// ("800m", "4x800m") or name ("800 Meters").
    #[arg(long, value_name = "EVENT", requires_all = ["gender", "div"])]
    event: Option<String>,

    /// With --gender, --div and --event: only find the event and division,
    /// as `ath seed --meet` finds them, and print it (`resolved`), without
    /// reading its entries. Fails, saying why, when the meet has no such event
    /// with entries in the division.
    #[arg(long, requires = "event")]
    resolve: bool,

    /// Offline mode: read recorded responses from DIR
    /// (`meets/<meet_id>/meet.json`, `events.json` and
    /// `entries/<gender>-<div_id>-<eventShort>.json`) instead of the live API
    /// (skips the cookie, the relay and the network).
    #[arg(long, value_name = "DIR")]
    fixtures_dir: Option<String>,
}

/// Who to seed comes from exactly one place: a roster (Mode B) or a meet's
/// entries (Mode A). A roster standing in for one event of a meet is the
/// seed sheet page's job (it runs `ath seed` once per event).
#[derive(Args, Debug)]
#[command(group(ArgGroup::new("entrants").required(true).args(["roster", "meet"])))]
struct SeedArgs {
    /// Roster CSV (one athlete per row; `name` required, `athlete_id` pins a
    /// known athlete).
    #[arg(long, value_name = "FILE")]
    roster: Option<String>,

    /// Seed a meet's accepted entrants instead of a roster: the Athletic.net
    /// meet id. Needs --gender and --div; the meet date comes from the meet.
    #[arg(long, value_name = "MEET_ID", requires_all = ["gender", "div"],
          conflicts_with_all = ["meet_date", "resolution_log"])]
    meet: Option<i64>,

    /// With --meet: the girls' (f) or boys' (m) event.
    #[arg(long, value_enum, value_name = "GENDER", requires = "meet")]
    gender: Option<Gender>,

    /// With --meet: the division, by id (as the entries page's URL has it),
    /// name or short name ("13", "Varsity" or "V").
    #[arg(long, value_name = "DIV", requires = "meet")]
    div: Option<String>,

    /// Event to seed on, by Athletic.net name (e.g. "800 Meters"); a flat race
    /// also takes the short form "800m". With --meet, the meet's event, by its
    /// code ("800m", "1mile", "4x800m") or name. Entrants are seeded on their
    /// Personal record in it as of today, across all outdoor seasons. A relay
    /// of four equal legs (with --meet only; a roster cannot name teams) is
    /// seeded per team on the sum of its runners' Personal records in its
    /// Seeding event: 4x100m on the 100m, 4x200m on the 200m, 4x400m on the
    /// 400m, 4x800m on the 800m, 4xMile on the 1600m (or each runner's mile,
    /// converted to the 1600m and flagged as an Estimated mark, when faster).
    /// Medley and shuttle relays are not seeded yet.
    #[arg(long, value_name = "NAME")]
    event: String,

    /// With --meet and a relay: seed each runner on this event instead of the
    /// relay's own leg distance, by Athletic.net name ("1 Mile") or a flat
    /// race's short form ("1600m"), with no conversion (a 4xMile then reads no
    /// mile).
    #[arg(long, value_name = "EVENT", requires = "meet")]
    seed_on: Option<String>,

    /// Date of the meet being seeded (not with --meet, which reads it from
    /// the meet). Refused if it is in the past: a seed sheet is for an
    /// upcoming meet. Marks are as of today, or as of the day before for a
    /// meet today.
    #[arg(long, value_name = "YYYY-MM-DD")]
    meet_date: Option<chrono::NaiveDate>,

    /// Output seed sheet CSV.
    #[arg(long, value_name = "FILE", default_value = "seed.csv")]
    out: String,

    /// Offline mode: read bios from `<DIR>/<athlete_id>.json`, a roster's
    /// name searches from `<DIR>/search/<name>.json`, and with --meet the meet
    /// from `<DIR>/meets/<meet_id>/`, instead of the live API (skips the
    /// cookie, the relay and the network entirely).
    #[arg(long, value_name = "DIR")]
    fixtures_dir: Option<String>,

    /// Write an opt-in CSV log of what auto-resolved and why.
    #[arg(long, value_name = "FILE")]
    resolution_log: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = parse_cli(std::env::args_os()).unwrap_or_else(|e| e.exit());
    match &cli.command {
        Command::Seed(args) => run_seed(&cli.global, args).await,
        Command::Meet(args) => run_meet(&cli.global, args).await,
        Command::Profile(args) => run_profile(&cli.global, args).await,
        Command::Areas(args) => run_areas(&cli.global, args).await,
        Command::List(args) => run_list(&cli.global, args).await,
        Command::Rank(args) => run_rank(&cli.global, args).await,
        Command::Relay(args) => match &args.command {
            RelayCommand::Install(install) => {
                print!("{}", relay_install(&cli.global, install));
                Ok(())
            }
        },
    }
}

/// `ath relay install`: the setup steps and the bookmarklet, or with
/// `--console` just the script.
fn relay_install(global: &GlobalArgs, args: &RelayInstallArgs) -> String {
    let port = global.relay_port;
    if args.console {
        return relay::console_script(port);
    }
    format!(
        "{}\nBookmarklet (for the relay on 127.0.0.1:{port}):\n\n{}\n",
        relay::setup_steps(port, true),
        relay::bookmarklet(port)
    )
}

/// `ath profile`: print one Profile in the output contract's format.
async fn run_profile(global: &GlobalArgs, args: &ProfileArgs) -> Result<()> {
    if let Some(event) = &args.event {
        refuse_bare_number(event)?;
    }
    let source = build_source(global, args.fixtures_dir.as_deref()).await?;
    let query = ProfileQuery {
        event: args.event.as_deref(),
        as_of: args.as_of,
        season: args.season,
        today: today(global, args.fixtures_dir.as_deref())?,
    };
    let report = profile(source.as_ref(), args.athlete_id, &query).await?;
    if let Some(e) = &report.event {
        let (name, event) = (&report.name, &e.name);
        match e.status {
            EventStatus::Ok => {}
            EventStatus::NotOnProfile => eprintln!(
                "ath: warning - {name} has no {event:?} results. --event takes the \
                 Athletic.net event name (e.g. \"400 Meters\") or a flat race's \
                 short form (\"400m\")."
            ),
            EventStatus::FieldEvent => eprintln!(
                "ath: warning - {event:?} is a field event; ath v1 supports running \
                 events only, so it has no Season best or Personal record."
            ),
            EventStatus::NoQualifyingMark => eprintln!(
                "ath: warning - {name} has {event:?} results but no Qualifying mark \
                 on or before {} ({}).",
                e.as_of,
                ath::mark::NEVER_COUNT
            ),
        }
    }
    let format = Format::detect(global.json, global.table);
    // The table shows the Area's note itself; JSON readers get it on stderr
    // when it is something to act on (a failed lookup, or a school taken from
    // another season, whatever the Area).
    let lookup = &report.area_lookup;
    if format == Format::Json
        && (lookup.status == AreaStatus::LookupFailed || lookup.school_not_listed)
        && let Some(note) = &lookup.note
    {
        eprintln!("ath: warning - Area: {note}");
    }
    emit(&report, format, &mut std::io::stdout().lock())
}

/// `ath meet`: a meet and its events with entries, or one event's entries,
/// in the output contract's format.
async fn run_meet(global: &GlobalArgs, args: &MeetArgs) -> Result<()> {
    let source = build_source(global, args.fixtures_dir.as_deref()).await?;
    let select = match (args.gender, args.div.as_deref(), args.event.as_deref()) {
        (Some(gender), Some(division), Some(event)) => Some(EventSelect {
            meet_id: args.meet_id,
            gender,
            division,
            event,
        }),
        _ => None,
    };
    let report = match (&select, args.resolve) {
        (Some(select), true) => meet_resolve(source.as_ref(), args.meet_id, select).await?,
        _ => meet_report(source.as_ref(), args.meet_id, select.as_ref()).await?,
    };
    emit(
        &report,
        Format::detect(global.json, global.table),
        &mut std::io::stdout().lock(),
    )
}

/// `ath areas`: one State's Areas for a season, in the output contract's format.
async fn run_areas(global: &GlobalArgs, args: &AreasArgs) -> Result<()> {
    let event = link_event(&args.event)?;
    let source = build_source(global, args.fixtures_dir.as_deref()).await?;
    let season = match args.season {
        Some(s) => s,
        None => this_year(global, args.fixtures_dir.as_deref())?,
    };
    let query = AreasQuery {
        state: &args.state,
        season,
        gender: args.gender,
        event,
        from_list: args.from_list,
    };
    let report = areas(source.as_ref(), &query).await?;
    let format = Format::detect(global.json, global.table);
    // The table prints the note itself.
    if format == Format::Json
        && let Some(note) = &report.note
    {
        eprintln!("ath: warning - {note}");
    }
    emit(&report, format, &mut std::io::stdout().lock())
}

/// The default season: this year.
fn this_year(global: &GlobalArgs, fixtures_dir: Option<&str>) -> Result<i64> {
    Ok(i64::from(chrono::Datelike::year(&today(
        global,
        fixtures_dir,
    )?)))
}

/// The env var that pins today's date (`YYYY-MM-DD`): a pinned clock, for
/// replaying recorded or anonymized data whose seasons are not this year's
/// (ADR 0010: anonymized data lives in fictional seasons ending at 2099).
const TODAY_ENV: &str = "ATH_TODAY";

/// The pinned date (`ATH_TODAY`), if one is set. Only a replay (offline:
/// `--fixtures-dir`, or the hidden fixture transport) may pin the clock; a live
/// run refuses one, so it never reads Athletic.net or ages its cache as of a
/// made-up day. A malformed date is always an error.
fn pinned_today(replay: bool) -> Result<Option<chrono::NaiveDate>> {
    let Ok(day) = std::env::var(TODAY_ENV) else {
        return Ok(None);
    };
    if day.trim().is_empty() {
        return Ok(None);
    }
    let date = chrono::NaiveDate::parse_from_str(day.trim(), "%Y-%m-%d")
        .with_context(|| format!("{TODAY_ENV}={day} is not a YYYY-MM-DD date"))?;
    if !replay {
        bail!(
            "{TODAY_ENV} pins the clock only for replays (--fixtures-dir); unset it for a run \
             that reads Athletic.net"
        );
    }
    Ok(Some(date))
}

/// Whether this run replays recorded data instead of reading Athletic.net.
fn is_replay(global: &GlobalArgs, fixtures_dir: Option<&str>) -> bool {
    fixtures_dir.is_some() || global.fixture_transport.is_some()
}

/// Today: the pinned date of a replay (`ATH_TODAY`), otherwise the local date.
fn today(global: &GlobalArgs, fixtures_dir: Option<&str>) -> Result<chrono::NaiveDate> {
    Ok(pinned_today(is_replay(global, fixtures_dir))?
        .unwrap_or_else(|| chrono::Local::now().date_naive()))
}

/// `ath list`: one whole Rankings list, in the output contract's format.
async fn run_list(global: &GlobalArgs, args: &ListArgs) -> Result<()> {
    let event = link_event(&args.event)?;
    let which = match (args.list, args.area.as_deref(), args.state.as_deref()) {
        (Some(id), _, _) => Which::List(id),
        (None, Some(name), state) => Which::Area { name, state },
        (None, None, Some(state)) => Which::State(state),
        (None, None, None) => unreachable!("clap requires one of --state, --area, --list"),
    };
    let source = build_source(global, args.fixtures_dir.as_deref()).await?;
    let query = ListQuery {
        which,
        season: match args.season {
            Some(s) => s,
            None => this_year(global, args.fixtures_dir.as_deref())?,
        },
        gender: args.gender,
        event,
        grades: args.grades.clone().unwrap_or_default(),
        limit: args.limit.map(|n| usize::try_from(n).unwrap_or(usize::MAX)),
        from_list: args.from_list,
    };
    let report = rankings_list(source.as_ref(), &query).await?;
    let format = Format::detect(global.json, global.table);
    // The table prints the note itself.
    if format == Format::Json
        && let Some(note) = &report.note
    {
        eprintln!("ath: warning - {note}");
    }
    emit(&report, format, &mut std::io::stdout().lock())
}

/// `ath rank`: an Athlete's places, in the output contract's format.
async fn run_rank(global: &GlobalArgs, args: &RankArgs) -> Result<()> {
    let event = link_event(&args.event)?;
    let source = build_source(global, args.fixtures_dir.as_deref()).await?;
    let query = RankQuery {
        athlete_id: args.athlete_id,
        event,
        season: match args.season {
            Some(s) => s,
            None => this_year(global, args.fixtures_dir.as_deref())?,
        },
        targets: args.lists.clone().unwrap_or_else(Targets::home),
        grade: args.grade,
        home: args.home.as_deref(),
        ahead: args.ahead,
        from_list: args.from_list,
        today: today(global, args.fixtures_dir.as_deref())?,
    };
    // A count on one line while lists are placed, for a person watching a
    // long run (`--in all` takes minutes); nothing when stderr is not a
    // terminal.
    let watching = std::io::stderr().is_terminal();
    let progress = |done: usize, total: usize| {
        if watching && total > 1 {
            ath::progress::line(&format!(
                "ath: placed on {done} of {total} lists and Grade groups"
            ));
        }
    };
    let report = rank_with_progress(source.as_ref(), &query, &progress).await;
    ath::progress::clear();
    let report = report?;
    let format = Format::detect(global.json, global.table);
    let all_failed = !report.results.is_empty()
        && report
            .results
            .iter()
            .all(|r| r.status == RankStatus::LookupFailed);
    let lists = report
        .results
        .iter()
        .map(|r| r.list.id)
        .collect::<std::collections::HashSet<_>>()
        .len();
    // The table prints the notes itself.
    if format == Format::Json {
        if let Some(note) = &report.note {
            eprintln!("ath: warning - {note}");
        }
        for r in report
            .results
            .iter()
            .filter(|r| r.status == RankStatus::LookupFailed)
        {
            eprintln!(
                "ath: warning - {} {}: {}",
                r.list.name,
                r.group,
                r.note.as_deref().unwrap_or("could not be read")
            );
        }
    }
    emit(&report, format, &mut std::io::stdout().lock())?;
    // Printed, but not a success: a script must not take it for one.
    if all_failed {
        bail!(
            "none of the {lists} list(s) could be read, for any Grade group ({} results)",
            report.results.len()
        );
    }
    Ok(())
}

/// Parse the command line. The pre-subcommand bare form (`ath --roster ...
/// --event ...`) was removed without an alias (ADR 0007), so point anyone still
/// using it at `ath seed` instead of clap's generic "unexpected argument".
fn parse_cli<I, T>(args: I) -> Result<Cli, clap::Error>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let args: Vec<OsString> = args.into_iter().map(Into::into).collect();
    let mut cli = Cli::try_parse_from(&args).map_err(|err| match misplaced_flag(&err, &args) {
        Some(Misplaced::MeetFlag(flag)) => Cli::command().error(
            ErrorKind::InvalidSubcommand,
            format!(
                "--{flag} goes after a subcommand: `ath seed --meet <MEET_ID> --gender <f|m> \
                 --div <DIV> --event <EVENT>` seeds a meet's entrants, and `ath meet <MEET_ID>` \
                 shows the meet"
            ),
        ),
        Some(Misplaced::SeedFlag) => Cli::command().error(
            ErrorKind::InvalidSubcommand,
            "the bare `ath --roster ... --event ...` form was removed; \
             use `ath seed --roster ... --event ...` instead",
        ),
        None => err,
    })?;
    // `overrides_with` makes the last of --json / --table win on one side of
    // the subcommand name, but clap merges global flags from both sides after
    // that, so `ath --json profile 1 --table` comes back with both set. Settle
    // it the same way: the later flag on the command line wins. (No ath flag
    // takes a value starting with `--`, so these tokens are always the flags.)
    if cli.global.json && cli.global.table {
        let last = args
            .iter()
            .rev()
            .find_map(|a| match a.to_str() {
                Some("--json") => Some(true),
                Some("--table") => Some(false),
                _ => None,
            })
            .unwrap_or(true);
        cli.global.json = last;
        cli.global.table = !last;
    }
    Ok(cli)
}

/// A subcommand's flag given where no subcommand was named.
#[derive(Debug, PartialEq, Eq)]
enum Misplaced {
    /// One of Mode A's flags (`--meet`, `--div`, or `--gender` beside them),
    /// which belong to `ath seed --meet` or `ath meet`, never to the removed
    /// bare form.
    MeetFlag(String),
    /// Another of `ath seed`'s flags: the removed bare `ath --roster ...`.
    SeedFlag,
}

/// Mode A's own flags, which point at `ath seed --meet` / `ath meet`.
const MEET_FLAGS: [&str; 2] = ["meet", "div"];

/// Which kind of misplaced flag clap rejected as an unexpected argument,
/// which only happens when no subcommand was named. Deciding from clap's own
/// error (not by scanning tokens) keeps `--help` / `--version` working and is
/// not fooled by a flag value that happens to be a subcommand name; reading
/// the flag names off the `seed` command keeps the list in step with
/// `SeedArgs`.
///
/// `--gender` also belongs to `ath list` and `ath areas`, so it points at
/// Mode A only beside `--meet` or `--div` on the same command line;
/// otherwise clap's own error stands.
fn misplaced_flag(err: &clap::Error, args: &[OsString]) -> Option<Misplaced> {
    if err.kind() != ErrorKind::UnknownArgument {
        return None;
    }
    let Some(ContextValue::String(arg)) = err.get(ContextKind::InvalidArg) else {
        return None;
    };
    let flag = arg.strip_prefix("--")?;
    let flag = flag.split('=').next().unwrap_or(flag);
    if MEET_FLAGS.contains(&flag) {
        return Some(Misplaced::MeetFlag(flag.to_string()));
    }
    if flag == "gender" {
        let with_meet = args.iter().filter_map(|a| a.to_str()).any(|a| {
            let name = a
                .strip_prefix("--")
                .map(|f| f.split('=').next().unwrap_or(f));
            name.is_some_and(|n| MEET_FLAGS.contains(&n))
        });
        return with_meet.then(|| Misplaced::MeetFlag(flag.to_string()));
    }
    let cmd = Cli::command();
    cmd.find_subcommand("seed")
        .is_some_and(|seed| seed.get_arguments().any(|a| a.get_long() == Some(flag)))
        .then_some(Misplaced::SeedFlag)
}

/// `ath seed`: find the entrants (a roster's rows resolved to athletes, or a
/// meet's accepted entries), then rank their Personal records (as of today,
/// or the day before a meet today) for one event into a seed sheet CSV.
async fn run_seed(global: &GlobalArgs, args: &SeedArgs) -> Result<()> {
    match (args.roster.as_deref(), args.meet) {
        (Some(roster), None) => run_seed_roster(global, args, roster).await,
        (None, Some(meet)) => run_seed_meet(global, args, meet).await,
        _ => unreachable!("clap requires exactly one of --roster and --meet"),
    }
}

/// `ath seed --roster` (Mode B).
async fn run_seed_roster(global: &GlobalArgs, args: &SeedArgs, roster_path: &str) -> Result<()> {
    // Refusals, in this order and before anything is read (the roster, the
    // browser relay): a past meet, then an --event a roster cannot seed (a
    // bare number, then a relay).
    let date = SeedDate::new(args.meet_date, today(global, args.fixtures_dir.as_deref())?)?;
    refuse_bare_number(&args.event)?;
    // A roster names people, not teams, so it never seeds a relay (told by
    // the same code and name rules as a meet's events).
    if is_relay_name(&args.event) {
        bail!(
            "--event {:?} is a relay, and relays are seeded from a meet's entries: use --meet \
             (with --gender, --div and the relay as --event)",
            args.event
        );
    }

    let source: Box<dyn DataSource> = build_source(global, args.fixtures_dir.as_deref()).await?;

    let roster = read_roster(roster_path)?;

    // Pass 1: resolve names to athletes. Confident matches are pinned; ambiguous
    // rows are written out for review.
    let resolution = resolve(source.as_ref(), &roster).await?;

    if !resolution.auto_matches.is_empty() {
        apply_resolved_ids(roster_path, &resolution.updated_roster)?;
        eprintln!(
            "ath: resolved {} name(s) and wrote athlete_id back to {}",
            resolution.auto_matches.len(),
            roster_path
        );
    }
    let format = Format::detect(global.json, global.table);
    for warning in search_failure_warnings(&resolution, format) {
        eprintln!("{warning}");
    }
    if let Some(log) = &args.resolution_log {
        write_resolution_log(log, &resolution.auto_matches)?;
    }
    let review = unresolved_path(roster_path);
    if !resolution.unresolved.is_empty() {
        write_unresolved_csv(&review, &resolution.unresolved)?;
        eprintln!(
            "ath: {} row(s) did not resolve - review {}, pin an athlete_id, and re-run",
            resolution.unresolved.len(),
            review.display()
        );
    }
    let unresolved = not_resolved(&resolution, &review.display().to_string());

    // Nothing resolved but there was work to do (all unresolved / search-failed):
    // a first-pass "go pin ids" state. Don't clobber an existing --out with an
    // empty sheet, but still print the (empty) sheet, which says why.
    if resolution.resolved.is_empty() && !roster.is_empty() {
        eprintln!(
            "ath: 0 of {} rows resolved to an athlete; nothing to seed. Pin \
             athlete_id(s) and re-run. {} left untouched.",
            roster.len(),
            args.out
        );
        let empty = SeedOutcome::default();
        return emit(
            &SeedSheet::new(&args.event, date, &empty, unresolved),
            format,
            &mut std::io::stdout().lock(),
        );
    }

    // Pass 2: seed the resolved athletes.
    let sheet =
        |outcome: &SeedOutcome| SeedSheet::new(&args.event, date, outcome, unresolved.clone());
    seed_and_print(
        global,
        args,
        source.as_ref(),
        &resolution.resolved,
        &args.event,
        date,
        &sheet,
    )
    .await
}

/// `ath seed --meet` (Mode A): the meet's accepted, unscratched entrants in
/// one event and division, each seeded from their Profile by athlete id.
async fn run_seed_meet(global: &GlobalArgs, args: &SeedArgs, meet_id: i64) -> Result<()> {
    let (Some(gender), Some(division)) = (args.gender, args.div.as_deref()) else {
        unreachable!("clap requires --gender and --div with --meet");
    };
    // A bare number names no event, so it is refused before anything is read.
    refuse_bare_number(&args.event)?;
    let source: Box<dyn DataSource> = build_source(global, args.fixtures_dir.as_deref()).await?;
    // The meet first: its dates decide whether there is anything to do.
    let meet = source.meet(meet_id).await?;
    let (Some(start), Some(end)) = (meet.meet.date(), meet.meet.end()) else {
        bail!("meet {meet_id} has no date on Athletic.net");
    };
    let date = SeedDate::for_meet(
        meet_id,
        start,
        end,
        today(global, args.fixtures_dir.as_deref())?,
    )?;
    let select = EventSelect {
        meet_id,
        gender,
        division,
        event: &args.event,
    };
    let list = source.meet_events(meet_id).await?;
    // Medley and shuttle relays, field and indoor-only events are refused
    // from the event list, before their entries are read.
    let seed_on = args.seed_on.as_deref();
    let selected = read_event(source.as_ref(), meet, &list, &select, |l| {
        seedable_on(l, seed_on)
    })
    .await?;
    let event = &selected.event;
    // A relay, also when only its entries say so (legs with a `Letter`):
    // its legs decide the Seeding event, unless --seed-on names one (a 4xMile
    // also reads the mile, as an Estimated mark, unless --seed-on is given).
    let seeding = if event.relay {
        Some(relay_seeding(event, &selected.relay_shape, seed_on)?)
    } else if let Some(seed_on) = &args.seed_on {
        bail!(
            "--seed-on {seed_on:?} is for relays, and {} ({}) is not one: its entrants are \
             seeded on the event itself",
            event.event,
            event.event_short
        );
    } else {
        None
    };
    let format = Format::detect(global.json, global.table);
    // The table lists them under "Not on the sheet"; JSON readers get them
    // here too.
    if format == Format::Json {
        for l in &selected.entries.left_out {
            eprint!("ath: note - not seeded: {}", ath::meet::left_out_line(l));
        }
    }
    let meet_ref = SheetMeet {
        meet: MeetRef::new(&selected.meet.meet),
        event: event.clone(),
        left_out: selected.entries.left_out.clone(),
    };
    if let Some(seeding) = seeding {
        return seed_relay_and_print(
            global,
            args,
            source.as_ref(),
            &selected.entries.teams,
            &seeding,
            date,
            meet_ref,
        )
        .await;
    }
    let roster: Vec<RosterRow> = selected
        .entries
        .entrants
        .iter()
        .map(|e| e.roster_row())
        .collect();
    let sheet = |outcome: &SeedOutcome| {
        SeedSheet::new(&args.event, date, outcome, Vec::new()).with_meet(meet_ref.clone())
    };
    if roster.is_empty() {
        eprintln!(
            "ath: meet {meet_id} has no accepted entrants in {} {} {}; nothing to seed. {} \
             left untouched.",
            event.gender.as_str(),
            event.division,
            event.event_short,
            args.out
        );
        return emit(
            &sheet(&SeedOutcome::default()),
            format,
            &mut std::io::stdout().lock(),
        );
    }
    // Seeded on the event as the meet names it (`800 Meters`), which is how
    // Profiles name it too.
    seed_and_print(
        global,
        args,
        source.as_ref(),
        &roster,
        &event.event,
        date,
        &sheet,
    )
    .await
}

/// Seed a relay's `teams` on `seeding_event` (with `mile`, a 4xMile's mile
/// as an Estimated mark too), write the relay CSV, warn,
/// and print the relay sheet.
async fn seed_relay_and_print(
    global: &GlobalArgs,
    args: &SeedArgs,
    source: &dyn DataSource,
    teams: &[RelayTeam],
    seeding: &RelaySeeding,
    date: SeedDate,
    meet: SheetMeet,
) -> Result<()> {
    let format = Format::detect(global.json, global.table);
    let seeding_event = seeding.event.as_str();
    let event = meet.event.clone();
    if teams.is_empty() {
        eprintln!(
            "ath: meet {} has no accepted teams in {} {} {}; nothing to seed. {} left untouched.",
            event.meet_id,
            event.gender.as_str(),
            event.division,
            event.event_short,
            args.out
        );
        let empty = RelayOutcome::default();
        return emit(
            &RelaySheet::new(&args.event, seeding_event, date, &empty, meet),
            format,
            &mut std::io::stdout().lock(),
        );
    }
    // Runners are legs 1 to 4 by their leg number; entries with none on any
    // team would make an all-blank sheet, so stop and say why.
    if !teams.iter().any(|t| {
        t.legs
            .iter()
            .any(|l| l.leg.is_some_and(|n| (1..=RELAY_LEGS).contains(&n)))
    }) {
        bail!(
            "meet {}'s {} entries carry no leg numbers (Letter A1 to A4) on any of its {} teams, \
             so no team has runners to seed; nothing written. `ath meet {} --gender {} --div {} \
             --event {}` shows the entries",
            event.meet_id,
            event.event_short,
            teams.len(),
            event.meet_id,
            event.gender.as_str(),
            event.division_id,
            event.event_short
        );
    }
    let opts = FetchOpts {
        concurrency: global.concurrency,
        delay: Duration::from_millis(global.delay_ms),
    };
    let outcome = seed_relay(source, teams, seeding_event, seeding.mile, date, &opts).await?;
    fail_if_every_fetch_failed(&outcome.failures, outcome.runners)?;
    write_relay_csv(&args.out, &outcome.teams)?;
    let sheet = RelaySheet::new(&args.event, seeding_event, date, &outcome, meet);
    let with = outcome
        .teams
        .iter()
        .filter(|t| t.sort_int.is_some())
        .count();
    eprintln!(
        "ath: wrote {} teams ({with} with an aggregate on the {}) to {}",
        outcome.teams.len(),
        sheet.seeding_event_label(),
        args.out
    );
    // Runners with a mark, each once (as `outcome.runners` counts them),
    // however many teams or legs they run.
    let marked = outcome
        .teams
        .iter()
        .flat_map(|t| &t.legs)
        .filter(|l| l.sort_int.is_some())
        .map(|l| l.athlete_id)
        .collect::<std::collections::HashSet<_>>()
        .len();
    let check = format!(
        "The Seeding event {seeding_event:?} came from the relay, or --seed-on if given (the \
         Athletic.net name, \"1600 Meters\", or a flat race's short form, \"1600m\"); the \
         runners' Profiles may name it differently, or none of them has an outdoor mark in it"
    );
    fetch_warnings(
        format,
        &outcome.failures,
        "no mark (Profile could not be read) for",
        (marked, outcome.runners, "runners"),
        &check,
        date,
    );
    emit(&sheet, format, &mut std::io::stdout().lock())
}

/// Seed `entrants` (every one pinned to an athlete id) on `seed_event`,
/// write the CSV, warn, and print the sheet `sheet` makes of the outcome.
async fn seed_and_print(
    global: &GlobalArgs,
    args: &SeedArgs,
    source: &dyn DataSource,
    entrants: &[RosterRow],
    seed_event: &str,
    date: SeedDate,
    sheet: &dyn Fn(&SeedOutcome) -> SeedSheet,
) -> Result<()> {
    let format = Format::detect(global.json, global.table);
    let opts = FetchOpts {
        concurrency: global.concurrency,
        delay: Duration::from_millis(global.delay_ms),
    };
    let outcome = seed(source, entrants, seed_event, date, &opts).await?;

    // A single bad athlete is skipped, but if EVERY fetch failed, that's
    // systemic (usually an expired cookie): fail loudly and write nothing.
    fail_if_every_fetch_failed(&outcome.failures, entrants.len())?;

    write_seed_csv(&args.out, &outcome.rows)?;

    let seeded = outcome
        .rows
        .iter()
        .filter(|r| r.pr_numeric.is_some())
        .count();
    eprintln!(
        "ath: wrote {} rows ({} with a {} mark) to {}",
        outcome.rows.len(),
        seeded,
        args.event,
        args.out
    );
    let check = if args.meet.is_some() {
        // The event (and its name) came from the meet, not from --event.
        format!(
            "The event name {seed_event:?} came from the meet; the entrants' Profiles may \
             name the event differently, or none of them has an outdoor mark in it"
        )
    } else {
        "Check that --event is the Athletic.net name (e.g. \"400 Meters\") or a flat \
         race's short form (\"400m\") and that it is a running event (v1 supports \
         running events only)"
            .to_string()
    };
    // The table lists skipped athletes itself; JSON readers get them here too.
    fetch_warnings(
        format,
        &outcome.failures,
        "skipped",
        (seeded, outcome.rows.len(), "athletes"),
        &check,
        date,
    );
    // Last, after every warning: the same sheet on stdout, in the output
    // contract's format (ADR 0007). A reader that stops early (`| head`) gets
    // a broken pipe here, with nothing left unsaid.
    emit(&sheet(&outcome), format, &mut std::io::stdout().lock())
}

/// Stop, writing nothing, when every one of the `attempted` Profiles failed
/// to read: that is systemic (usually an expired cookie), not one bad id, so
/// an empty sheet with exit 0 would mislead. Each error is printed first.
fn fail_if_every_fetch_failed(failures: &[FetchFailure], attempted: usize) -> Result<()> {
    if attempted == 0 || failures.len() < attempted {
        return Ok(());
    }
    for f in failures {
        eprintln!("ath: {} (athlete {}): {}", f.name, f.athlete_id, f.error);
    }
    bail!(
        "all {} bio fetches failed - no seed sheet written. The errors above say what to do.",
        failures.len()
    )
}

/// The warnings after a sheet is written, for an individual sheet and a
/// relay's alike: with JSON output, each Profile that could not be read
/// (`what` says what that meant, such as `skipped`), since the table lists
/// them itself; and when none of `total` people (`noun`) had a mark, why
/// that may be (`check`), what never counts and, for a meet today, the
/// as-of date.
fn fetch_warnings(
    format: Format,
    failures: &[FetchFailure],
    what: &str,
    (marked, total, noun): (usize, usize, &str),
    check: &str,
    date: SeedDate,
) {
    if format == Format::Json {
        for f in failures {
            eprintln!(
                "ath: warning - {what} {} (athlete {}): {}",
                f.name, f.athlete_id, f.error
            );
        }
    }
    if total == 0 || marked > 0 {
        return;
    }
    eprintln!(
        "ath: warning - 0 of {total} {noun} had a qualifying mark. {check}. Note: {}.{}",
        ath::mark::NEVER_COUNT,
        if date.is_meet_day() {
            format!(
                " For a meet that starts today or has started, marks are read as of {}, \
                 so results from the meet (and this season's undated results) don't count.",
                date.as_of()
            )
        } else {
            String::new()
        }
    );
}

/// The stderr warnings for roster rows whose search failed. The table lists
/// them under "Not on the sheet", so they are printed only with JSON output
/// (as with skipped Profiles), never twice.
fn search_failure_warnings(resolution: &ResolveOutcome, format: Format) -> Vec<String> {
    if format != Format::Json {
        return Vec::new();
    }
    resolution
        .search_failures
        .iter()
        .map(|(name, err)| format!("ath: warning - search failed for {name:?} (skipped): {err}"))
        .collect()
}

/// `roster.csv` -> `roster.unresolved.csv`.
fn unresolved_path(roster: &str) -> PathBuf {
    PathBuf::from(roster).with_extension("unresolved.csv")
}

/// How this run reaches Athletic.net (ADR 0006).
#[derive(Debug, PartialEq, Eq)]
enum Transport {
    /// A session cookie was given: send it with plain HTTP requests.
    Cookie(String),
    /// The default: fetch through an Athletic.net browser tab via the relay.
    Relay,
}

/// A cookie from `--cookie-file`, `--cookie` or `ATH_COOKIE` (passed in as
/// `env_cookie`) picks the cookie transport; with none, the browser relay.
fn select_transport(global: &GlobalArgs, env_cookie: Option<String>) -> Result<Transport> {
    Ok(match resolve_cookie(global, env_cookie)? {
        Some(cookie) => Transport::Cookie(cookie),
        None => Transport::Relay,
    })
}

async fn build_source(
    global: &GlobalArgs,
    fixtures_dir: Option<&str>,
) -> Result<Box<dyn DataSource>> {
    // A pinned clock is for replays only, and is checked before anything is read.
    let pinned = pinned_today(is_replay(global, fixtures_dir))?;
    if let Some(dir) = fixtures_dir {
        if global.record_dir.is_some() {
            bail!(
                "--record-dir records live Athletic.net responses; with --fixtures-dir there are \
                 none (the fixtures dir already is a recording)"
            );
        }
        return Ok(Box::new(FixtureSource::from_dir(dir)?));
    }
    let recorder = global
        .record_dir
        .as_deref()
        .map(Recorder::new)
        .transpose()?;

    // The fixture transport answers from test files: it never reads or writes
    // the user's own cache, only a --cache-dir given with it.
    let mode =
        if global.no_cache || (global.fixture_transport.is_some() && global.cache_dir.is_none()) {
            CacheMode::Off
        } else if global.refresh {
            CacheMode::Refresh
        } else {
            CacheMode::Normal
        };
    let cache_dir = match &global.cache_dir {
        Some(d) => std::path::PathBuf::from(d),
        None => dirs::cache_dir()
            .unwrap_or_else(|| std::path::PathBuf::from(".ath-cache"))
            .join("ath")
            .join("responses"),
    };
    // Freshness is per data kind (ADR-0006), chosen by each request's caller.
    let cache = ResponseCache::new(cache_dir, mode)?;
    let page_delay = Duration::from_millis(global.delay_ms);

    /// The live source over `fetcher`, with the cache and the recorder.
    fn live<F: ath::http::Fetcher + 'static>(
        fetcher: F,
        cache: ResponseCache,
        page_delay: Duration,
        recorder: Option<Recorder>,
        pinned: Option<chrono::NaiveDate>,
    ) -> Box<dyn DataSource> {
        let mut source = AthleticNet::new(fetcher)
            .with_cache(cache)
            .with_page_delay(page_delay);
        if let Some(day) = pinned {
            source = source.with_today(day);
        }
        match recorder {
            Some(recorder) => Box::new(source.with_recorder(recorder)),
            None => Box::new(source),
        }
    }

    if let Some(dir) = &global.fixture_transport {
        return Ok(live(
            FixtureFetcher::new(dir),
            cache,
            page_delay,
            recorder,
            pinned,
        ));
    }
    match select_transport(global, std::env::var("ATH_COOKIE").ok())? {
        Transport::Cookie(cookie) => {
            let user_agent = global
                .user_agent
                .clone()
                .unwrap_or_else(|| DEFAULT_USER_AGENT.to_string());
            let fetcher = ReqwestFetcher::new(cookie, user_agent)?;
            Ok(live(fetcher, cache, page_delay, recorder, None))
        }
        Transport::Relay => {
            let config =
                RelayConfig::new(global.relay_port, Duration::from_secs(global.relay_timeout));
            // Bound now, but the tab is only waited for when a request needs it
            // (a run the cache serves needs none).
            let relay = RelayFetcher::bind(config).await?;
            Ok(live(relay, cache, page_delay, recorder, None))
        }
    }
}

/// The session cookie, if one was given. An explicitly-passed --cookie-file
/// wins over --cookie, which wins over a (possibly stale) ATH_COOKIE env var
/// (`env_cookie`). All are trimmed - a trailing newline makes an invalid Cookie
/// header. An empty ATH_COOKIE counts as unset; an empty flag is a mistake.
fn resolve_cookie(global: &GlobalArgs, env_cookie: Option<String>) -> Result<Option<String>> {
    if let Some(path) = &global.cookie_file {
        let raw =
            std::fs::read_to_string(path).with_context(|| format!("reading cookie file {path}"))?;
        let cookie = raw.trim();
        if cookie.is_empty() {
            bail!("cookie file {path} is empty");
        }
        return Ok(Some(cookie.to_string()));
    }
    if let Some(c) = &global.cookie {
        if c.trim().is_empty() {
            bail!("--cookie is empty; leave it out to use the browser relay");
        }
        return Ok(Some(c.trim().to_string()));
    }
    Ok(env_cookie
        .map(|c| c.trim().to_string())
        .filter(|c| !c.is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_failures_warn_on_stderr_only_with_json() {
        let resolution = ResolveOutcome {
            search_failures: vec![("Ivy Ironwood".to_string(), "timed out".to_string())],
            ..ResolveOutcome::default()
        };
        assert_eq!(
            search_failure_warnings(&resolution, Format::Json),
            ["ath: warning - search failed for \"Ivy Ironwood\" (skipped): timed out"]
        );
        // The table lists it under "Not on the sheet" instead.
        assert!(search_failure_warnings(&resolution, Format::Table).is_empty());
    }

    fn seed_args(cli: &Cli) -> &SeedArgs {
        match &cli.command {
            Command::Seed(a) => a,
            other => panic!("expected seed, got {other:?}"),
        }
    }

    fn profile_args(cli: &Cli) -> &ProfileArgs {
        match &cli.command {
            Command::Profile(a) => a,
            other => panic!("expected profile, got {other:?}"),
        }
    }

    #[test]
    fn parses_profile() {
        let cli = parse_cli([
            "ath",
            "profile",
            "2001",
            "--event",
            "400m",
            "--as-of",
            "2026-04-01",
            "--table",
        ])
        .unwrap();
        let a = profile_args(&cli);
        assert_eq!(a.athlete_id, 2001);
        assert_eq!(a.event.as_deref(), Some("400m"));
        assert_eq!(a.as_of, chrono::NaiveDate::from_ymd_opt(2026, 4, 1));
        assert!(cli.global.table && !cli.global.json);
    }

    #[test]
    fn json_and_table_are_global_and_the_last_one_wins() {
        let cli = parse_cli(["ath", "--json", "profile", "1"]).unwrap();
        assert!(cli.global.json && !cli.global.table);
        for (args, json) in [
            (["ath", "--json", "--table", "profile", "1"], false),
            (["ath", "profile", "1", "--table", "--json"], true),
            (["ath", "--json", "profile", "1", "--table"], false),
            (["ath", "--table", "profile", "1", "--json"], true),
        ] {
            let g = parse_cli(args).unwrap().global;
            assert_eq!((g.json, g.table), (json, !json), "{args:?}");
        }
    }

    #[test]
    fn parses_areas() {
        let cli = parse_cli(["ath", "areas", "CA", "--season", "2026", "--json"]).unwrap();
        let Command::Areas(a) = &cli.command else {
            panic!("expected areas, got {:?}", cli.command);
        };
        assert_eq!((a.state.as_str(), a.season), ("CA", Some(2026)));
        assert_eq!(a.gender, Gender::F);
        assert_eq!(a.event, "100m");
        assert_eq!(a.from_list, DEFAULT_FROM_LIST);
        assert!(cli.global.json);
        let cli = parse_cli([
            "ath",
            "areas",
            "CA",
            "--gender",
            "m",
            "--event",
            "400m",
            "--from-list",
            "900499",
        ])
        .unwrap();
        let Command::Areas(a) = &cli.command else {
            panic!("expected areas, got {:?}", cli.command);
        };
        assert_eq!(
            (a.gender, a.event.as_str(), a.from_list),
            (Gender::M, "400m", 900499)
        );
        let err = parse_cli(["ath", "areas"]).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::MissingRequiredArgument);
        let err = parse_cli(["ath", "areas", "CA", "--gender", "x"]).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidValue);
    }

    fn list_args(cli: &Cli) -> &ListArgs {
        match &cli.command {
            Command::List(a) => a,
            other => panic!("expected list, got {other:?}"),
        }
    }

    #[test]
    fn parses_list() {
        let cli = parse_cli([
            "ath", "list", "--state", "CA", "--area", "CCS", "--event", "400m", "--gender", "f",
            "--season", "2026", "--grades", "9-10", "--limit", "25",
        ])
        .unwrap();
        let a = list_args(&cli);
        assert_eq!(
            (a.state.as_deref(), a.area.as_deref(), a.list),
            (Some("CA"), Some("CCS"), None)
        );
        assert_eq!(
            (a.event.as_str(), a.gender, a.season),
            ("400m", Gender::F, Some(2026))
        );
        assert_eq!(a.grades.as_ref().map(Grades::as_slice), Some(&[9, 10][..]));
        assert_eq!((a.limit, a.from_list), (Some(25), DEFAULT_FROM_LIST));

        let cli = parse_cli([
            "ath", "list", "--list", "900502", "--event", "LJ", "--gender", "m",
        ])
        .unwrap();
        let a = list_args(&cli);
        assert_eq!((a.list, a.grades.clone()), (Some(900502), None));
    }

    fn rank_args(cli: &Cli) -> &RankArgs {
        match &cli.command {
            Command::Rank(a) => a,
            other => panic!("expected rank, got {other:?}"),
        }
    }

    #[test]
    fn parses_rank() {
        let cli = parse_cli([
            "ath",
            "rank",
            "982389051",
            "--event",
            "400m",
            "--season",
            "2099",
            "--in",
            "CA, ccs",
            "--grade",
            "10",
            "--home",
            "CA",
        ])
        .unwrap();
        let a = rank_args(&cli);
        assert_eq!(
            (a.athlete_id, a.event.as_str(), a.season),
            (982389051, "400m", Some(2099))
        );
        assert_eq!(
            a.lists,
            Some(Targets(vec![
                ath::rank::Target::Named("CA".into()),
                ath::rank::Target::Named("ccs".into())
            ]))
        );
        assert_eq!((a.grade, a.home.as_deref()), (Some(10), Some("CA")));

        let cli = parse_cli(["ath", "rank", "1", "--event", "400m", "--in", "ALL"]).unwrap();
        assert_eq!(
            rank_args(&cli).lists,
            Some(Targets(vec![ath::rank::Target::AllStates]))
        );
        let cli = parse_cli(["ath", "rank", "1", "--event", "400m"]).unwrap();
        assert_eq!(rank_args(&cli).lists, None, "home by default");
    }

    #[test]
    fn rank_refuses_bad_arguments() {
        let kind = |extra: &[&str]| {
            let mut args = vec!["ath", "rank", "1", "--event", "400m"];
            args.extend(extra);
            parse_cli(args).unwrap_err().kind()
        };
        assert_eq!(kind(&["--grade", "8"]), ErrorKind::ValueValidation);
        assert_eq!(kind(&["--in", " , "]), ErrorKind::ValueValidation);
        assert_eq!(kind(&["--season", "12026"]), ErrorKind::ValueValidation);
        let err = parse_cli(["ath", "rank", "1"]).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::MissingRequiredArgument);
    }

    #[test]
    fn list_refuses_bad_arguments() {
        let base = ["ath", "list", "--event", "400m", "--gender", "f"];
        let kind = |extra: &[&str]| {
            let mut args = base.to_vec();
            args.extend(extra);
            parse_cli(args).unwrap_err().kind()
        };
        assert_eq!(kind(&[]), ErrorKind::MissingRequiredArgument);
        assert_eq!(
            kind(&["--list", "1", "--state", "CA"]),
            ErrorKind::ArgumentConflict
        );
        assert_eq!(
            kind(&["--state", "CA", "--grades", "8"]),
            ErrorKind::ValueValidation
        );
        assert_eq!(
            kind(&["--state", "CA", "--limit", "0"]),
            ErrorKind::ValueValidation
        );
        assert_eq!(
            kind(&["--state", "CA", "--season", "12026"]),
            ErrorKind::ValueValidation
        );
        let err = parse_cli(["ath", "list", "--state", "CA", "--event", "400m"]).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::MissingRequiredArgument);
    }

    #[test]
    fn profile_season_and_as_of_work_without_an_event() {
        // They also pick the School and Area season.
        let cli = parse_cli(["ath", "profile", "1", "--event", "x", "--season", "2025"]).unwrap();
        assert_eq!(profile_args(&cli).season, Some(2025));
        let cli = parse_cli(["ath", "profile", "1", "--season", "2025"]).unwrap();
        assert_eq!(profile_args(&cli).season, Some(2025));
        let cli = parse_cli(["ath", "profile", "1", "--as-of", "2026-04-01"]).unwrap();
        assert_eq!(
            profile_args(&cli).as_of,
            chrono::NaiveDate::from_ymd_opt(2026, 4, 1)
        );
    }

    #[test]
    fn profile_rejects_bad_dates() {
        let err =
            parse_cli(["ath", "profile", "1", "--event", "x", "--as-of", "4/1/26"]).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::ValueValidation);
    }

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn profile_refuses_an_indoor_season() {
        let err = parse_cli([
            "ath", "profile", "1", "--event", "400m", "--season", "12026",
        ])
        .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::ValueValidation);
        assert!(parse_cli(["ath", "profile", "1", "--event", "400m", "--season", "2026"]).is_ok());
    }

    #[test]
    fn parses_seed_with_defaults() {
        let cli = parse_cli(["ath", "seed", "--roster", "r.csv", "--event", "400 Meters"]).unwrap();
        let a = seed_args(&cli);
        assert_eq!(a.roster.as_deref(), Some("r.csv"));
        assert_eq!(a.event, "400 Meters");
        assert_eq!(a.meet_date, None);
        assert_eq!(a.out, "seed.csv");
        assert_eq!(cli.global.concurrency, 5);
        assert_eq!(cli.global.delay_ms, 250);
        assert!(!cli.global.refresh && !cli.global.no_cache);
    }

    #[test]
    fn parses_seed_from_a_meet() {
        let cli = parse_cli([
            "ath", "seed", "--meet", "900700", "--gender", "m", "--div", "V", "--event", "800m",
        ])
        .unwrap();
        let a = seed_args(&cli);
        assert_eq!((a.meet, a.roster.as_deref()), (Some(900700), None));
        assert_eq!((a.gender, a.div.as_deref()), (Some(Gender::M), Some("V")));
        assert_eq!(a.event, "800m");
        // A roster run takes neither --gender nor --div.
        let cli = parse_cli(["ath", "seed", "--roster", "r.csv", "--event", "x"]).unwrap();
        let a = seed_args(&cli);
        assert_eq!((a.meet, a.gender, a.div.as_deref()), (None, None, None));
        let kind = |args: &[&str]| parse_cli(args.iter().copied()).unwrap_err().kind();
        assert_eq!(
            kind(&[
                "ath", "seed", "--meet", "1", "--gender", "m", "--event", "x"
            ]),
            ErrorKind::MissingRequiredArgument
        );
        assert_eq!(
            kind(&["ath", "seed", "--event", "x"]),
            ErrorKind::MissingRequiredArgument
        );
        assert_eq!(
            kind(&[
                "ath", "seed", "--meet", "1", "--gender", "m", "--div", "1", "--event", "x",
                "--roster", "r.csv",
            ]),
            ErrorKind::ArgumentConflict
        );
    }

    #[test]
    fn parses_meet() {
        let cli = parse_cli(["ath", "meet", "900700"]).unwrap();
        let Command::Meet(a) = &cli.command else {
            panic!("expected meet, got {:?}", cli.command);
        };
        assert_eq!((a.meet_id, a.event.as_deref()), (900700, None));
        let cli = parse_cli([
            "ath", "meet", "900700", "--gender", "f", "--div", "13", "--event", "4x800m",
        ])
        .unwrap();
        let Command::Meet(a) = &cli.command else {
            panic!("expected meet, got {:?}", cli.command);
        };
        assert_eq!(
            (a.gender, a.div.as_deref(), a.event.as_deref()),
            (Some(Gender::F), Some("13"), Some("4x800m"))
        );
        let err = parse_cli(["ath", "meet", "1", "--gender", "f"]).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::MissingRequiredArgument);
    }

    #[test]
    fn seed_takes_a_meet_date() {
        let args = ["ath", "seed", "--roster", "r.csv", "--event", "x"];
        let mut with = args.to_vec();
        with.extend(["--meet-date", "2027-05-01"]);
        let cli = parse_cli(with).unwrap();
        assert_eq!(
            seed_args(&cli).meet_date,
            chrono::NaiveDate::from_ymd_opt(2027, 5, 1)
        );
        let mut bad = args.to_vec();
        bad.extend(["--meet-date", "5/1/27"]);
        assert_eq!(
            parse_cli(bad).unwrap_err().kind(),
            ErrorKind::ValueValidation
        );
    }

    #[test]
    fn seed_season_was_removed() {
        // A Personal record spans every season, so `--season` is gone.
        let err = parse_cli([
            "ath", "seed", "--roster", "r.csv", "--event", "x", "--season", "2026",
        ])
        .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::UnknownArgument);
    }

    /// Every global flag, in one position or the other, lands on `cli.global`.
    fn assert_globals(cli: &Cli) {
        let g = &cli.global;
        assert_eq!(g.cookie.as_deref(), Some("c=1"));
        assert_eq!(g.cookie_file.as_deref(), Some("cookie.txt"));
        assert_eq!(g.user_agent.as_deref(), Some("UA/1"));
        assert_eq!(g.concurrency, 2);
        assert_eq!(g.delay_ms, 10);
        assert!(g.refresh);
        assert!(g.no_cache);
        assert_eq!(g.cache_dir.as_deref(), Some("/tmp/c"));
        assert_eq!(g.relay_port, 9001);
        assert_eq!(g.relay_timeout, 5);
        assert!(g.table && !g.json);
        assert_eq!(seed_args(cli).event, "800 Meters");
    }

    const GLOBALS: [&str; 18] = [
        "--table",
        "--relay-port",
        "9001",
        "--relay-timeout",
        "5",
        "--cookie",
        "c=1",
        "--cookie-file",
        "cookie.txt",
        "--user-agent",
        "UA/1",
        "--concurrency",
        "2",
        "--delay-ms",
        "10",
        "--refresh",
        "--no-cache",
        "--cache-dir=/tmp/c",
    ];
    const SEED: [&str; 5] = ["seed", "--roster", "r.csv", "--event", "800 Meters"];

    #[test]
    fn global_flags_before_subcommand() {
        let args = ["ath"].iter().chain(&GLOBALS).chain(&SEED);
        assert_globals(&parse_cli(args.copied()).unwrap());
    }

    #[test]
    fn global_flags_after_subcommand() {
        let args = ["ath"].iter().chain(&SEED).chain(&GLOBALS);
        assert_globals(&parse_cli(args.copied()).unwrap());
    }

    #[test]
    fn bare_form_is_rejected_with_pointer_to_seed() {
        for args in [
            vec!["ath", "--roster", "r.csv", "--event", "400 Meters"],
            vec![
                "ath",
                "--cookie",
                "c",
                "--roster=r.csv",
                "--event=400 Meters",
            ],
            vec!["ath", "--event", "400 Meters"],
            // Any seed-only flag first, not just --roster/--event.
            vec!["ath", "--fixtures-dir", "f", "--roster", "r.csv"],
            vec!["ath", "--meet-date", "2027-05-01", "--roster", "r.csv"],
            vec!["ath", "--out=o.csv", "--roster", "r.csv"],
            vec!["ath", "--resolution-log", "l.csv", "--event", "x"],
        ] {
            let err = parse_cli(args.clone()).unwrap_err();
            let msg = err.to_string();
            assert!(msg.contains("ath seed --roster"), "{args:?}: {msg}");
            assert_ne!(err.exit_code(), 0);
        }
    }

    #[test]
    fn a_top_level_meet_flag_points_at_seed_meet_and_meet() {
        for args in [
            vec!["ath", "--meet", "900700", "--event", "800m"],
            vec!["ath", "--gender", "m", "--meet", "900700"],
            vec!["ath", "--div=13", "seed"],
        ] {
            let err = parse_cli(args.clone()).unwrap_err();
            let msg = err.to_string();
            assert!(msg.contains("goes after a subcommand"), "{args:?}: {msg}");
            assert!(
                msg.contains("`ath seed --meet <MEET_ID>"),
                "{args:?}: {msg}"
            );
            assert!(msg.contains("`ath meet <MEET_ID>`"), "{args:?}: {msg}");
            assert!(!msg.contains("bare"), "{args:?}: {msg}");
            assert_ne!(err.exit_code(), 0);
        }
    }

    #[test]
    fn a_top_level_gender_alone_keeps_claps_own_error() {
        // `ath list` and `ath areas` take --gender too: no pointer to Mode A,
        // nor to the removed bare roster form.
        for args in [
            vec![
                "ath", "--gender", "f", "list", "--state", "CA", "--event", "400m",
            ],
            vec!["ath", "--gender", "m", "--roster", "r.csv"],
        ] {
            let err = parse_cli(args.clone()).unwrap_err();
            assert_eq!(err.kind(), ErrorKind::UnknownArgument, "{args:?}");
            let msg = err.to_string();
            assert!(
                msg.contains("unexpected argument '--gender'"),
                "{args:?}: {msg}"
            );
            assert!(!msg.contains("ath seed"), "{args:?}: {msg}");
        }
    }

    #[test]
    fn help_and_version_are_never_rewritten() {
        // clap answers `--help` / `--version` when it reaches them first.
        let err = parse_cli(["ath", "--help", "--roster", "r.csv"]).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::DisplayHelp);
        let err = parse_cli(["ath", "--version", "--event", "x"]).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::DisplayVersion);
        // After a stray seed flag clap stops before reaching them (plain clap
        // would say "unexpected argument"), so the pointer to `ath seed` wins.
        let msg = parse_cli(["ath", "--roster", "r.csv", "--help"])
            .unwrap_err()
            .to_string();
        assert!(msg.contains("ath seed --roster"), "{msg}");
    }

    #[test]
    fn a_flag_value_named_seed_is_not_a_subcommand() {
        for args in [
            vec!["ath", "--event", "seed", "--roster", "r.csv"],
            vec!["ath", "--cookie", "seed", "--roster", "r.csv"],
        ] {
            let msg = parse_cli(args.clone()).unwrap_err().to_string();
            assert!(msg.contains("ath seed --roster"), "{args:?}: {msg}");
        }
    }

    #[test]
    fn seed_errors_are_not_rewritten() {
        // A real `ath seed` mistake keeps clap's own error.
        let err = parse_cli(["ath", "seed", "--roster", "r.csv"]).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::MissingRequiredArgument);
        assert!(!err.to_string().contains("was removed"));
    }

    #[test]
    fn no_args_shows_help_listing_seed() {
        let err = parse_cli(["ath"]).unwrap_err();
        assert_eq!(
            err.kind(),
            ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
        );
        assert!(err.to_string().contains("seed"));
    }

    fn globals(args: &[&str]) -> GlobalArgs {
        let mut argv = vec!["ath"];
        argv.extend(args);
        argv.extend(SEED);
        parse_cli(argv).unwrap().global
    }

    #[test]
    fn relay_timeout_must_be_1_to_3600_seconds() {
        for bad in ["0", "3601", "18446744073709551615"] {
            let err = parse_cli(["ath", "--relay-timeout", bad, "relay", "install"]).unwrap_err();
            assert_eq!(err.kind(), ErrorKind::ValueValidation, "{bad}");
        }
        for ok in ["1", "3600"] {
            parse_cli(["ath", "--relay-timeout", ok, "relay", "install"]).unwrap();
        }
    }

    #[test]
    fn relay_flags_default_to_port_8767_and_30_seconds() {
        let g = globals(&[]);
        assert_eq!(g.relay_port, 8767);
        assert_eq!(g.relay_timeout, 30);
    }

    #[test]
    fn no_cookie_anywhere_selects_the_relay() {
        assert_eq!(
            select_transport(&globals(&[]), None).unwrap(),
            Transport::Relay
        );
        // A blank ATH_COOKIE counts as unset.
        assert_eq!(
            select_transport(&globals(&[]), Some(" \n".into())).unwrap(),
            Transport::Relay
        );
    }

    #[test]
    fn any_cookie_selects_the_cookie_transport() {
        let cookie = |c: &str| Transport::Cookie(c.to_string());
        assert_eq!(
            select_transport(&globals(&[]), Some("env=1\n".into())).unwrap(),
            cookie("env=1")
        );
        assert_eq!(
            select_transport(&globals(&["--cookie", " flag=1 "]), Some("env=1".into())).unwrap(),
            cookie("flag=1")
        );
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("cookie.txt");
        std::fs::write(&file, "file=1\n").unwrap();
        let file = file.to_str().unwrap();
        assert_eq!(
            select_transport(
                &globals(&["--cookie-file", file, "--cookie", "flag=1"]),
                Some("env=1".into())
            )
            .unwrap(),
            cookie("file=1")
        );
    }

    #[test]
    fn an_empty_cookie_flag_is_an_error_not_the_relay() {
        assert!(select_transport(&globals(&["--cookie", " "]), None).is_err());
    }

    fn install_args(cli: &Cli) -> &RelayInstallArgs {
        match &cli.command {
            Command::Relay(RelayArgs {
                command: RelayCommand::Install(a),
            }) => a,
            other => panic!("not relay install: {other:?}"),
        }
    }

    #[test]
    fn relay_install_prints_steps_then_the_bookmarklet() {
        let cli = parse_cli(["ath", "relay", "install"]).unwrap();
        let out = relay_install(&cli.global, install_args(&cli));
        assert!(out.contains("https://www.athletic.net"), "{out}");
        assert!(out.contains("click Allow"), "{out}");
        let last = out.lines().last().unwrap();
        assert!(last.starts_with("javascript:"), "{last}");
        assert!(last.contains("127.0.0.1%3A8767"), "{last}");
    }

    #[test]
    fn relay_install_console_prints_the_script_for_the_port() {
        let cli = parse_cli([
            "ath",
            "relay",
            "install",
            "--console",
            "--relay-port",
            "9100",
        ])
        .unwrap();
        let out = relay_install(&cli.global, install_args(&cli));
        assert!(out.starts_with("/*"), "{out}");
        assert!(
            out.contains("const ATH = \"http://127.0.0.1:9100\";"),
            "{out}"
        );
    }
}
