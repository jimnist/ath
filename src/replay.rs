//! A transport that answers Athletic.net requests from a fixtures dir
//! (`--fixture-transport`, hidden): the [`crate::source::layout`] read back
//! the other way, from request to file.
//!
//! It drives the live data source (`AthleticNet`: the cache, the token hops,
//! the recording) with no network, which is how the tests check that
//! `--record-dir` writes what `--fixtures-dir` replays to the same output.
//! Tokens are made up: a rankings answer carries `fixture-token-<list_id>`,
//! which tells `GetNavInfo` which list asked, and `GetMeetData` carries
//! [`MEET_TOKEN`], which the event list and entries require.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use async_trait::async_trait;
use percent_encoding::percent_decode_str;

use crate::area::Gender;
use crate::http::{FetchResponse, Fetcher, Method, Request};
use crate::list::{Grades, PageQuery};
use crate::meet::EntriesQuery;
use crate::record::set_top_level_string;
use crate::source::layout;

/// The `jwtMeet` token [`FixtureFetcher`] hands out.
pub const MEET_TOKEN: &str = "fixture-meet-token";

/// The prefix of the rankings tokens [`FixtureFetcher`] hands out.
pub const RANKINGS_TOKEN_PREFIX: &str = "fixture-token-";

/// Answers requests from a fixtures dir ([`crate::source::FixtureSource`]'s
/// layout). A request with no file gets a 404.
#[derive(Debug, Clone)]
pub struct FixtureFetcher {
    dir: PathBuf,
}

impl FixtureFetcher {
    pub fn new(dir: impl AsRef<Path>) -> Self {
        Self {
            dir: dir.as_ref().to_path_buf(),
        }
    }

    fn read(&self, rel: &Path) -> Option<String> {
        std::fs::read_to_string(self.dir.join(rel)).ok()
    }

    /// `GetRankings`: the page's file with a token added, or for a list with
    /// no such page (a token request), just a token and the list's division.
    fn rankings(&self, body: &serde_json::Value) -> Option<FetchResponse> {
        let list_id = body.get("divListId")?.as_i64()?;
        let gender = Gender::from_bio(body.get("gender")?.as_str()?)?;
        let event = body.get("eventShort")?.as_str()?;
        let params = body.get("qParams");
        let grades: Vec<String> = params
            .and_then(|p| p.get("grades"))
            .and_then(|g| g.as_array())
            .map(|g| g.iter().map(|v| v.to_string()).collect())
            .unwrap_or_default();
        let grades = if grades.is_empty() {
            Grades::all()
        } else {
            Grades::parse(&grades.join(",")).ok()?
        };
        let page = params
            .and_then(|p| p.get("page"))
            .and_then(|p| p.as_u64())
            .unwrap_or(1) as u32;
        let q = PageQuery {
            list_id,
            gender,
            event,
            grades: &grades,
            page,
            season: 0,
        };
        let token = format!("{RANKINGS_TOKEN_PREFIX}{list_id}");
        let body = match self.read(&layout::rankings(&q)) {
            Some(raw) => set_top_level_string(&raw, "jwtTFTopReport", &token),
            None => {
                let division = self
                    .list_division(list_id)
                    .unwrap_or(serde_json::Value::Null);
                format!("{{\"jwtTFTopReport\": \"{token}\", \"division\": {division}}}")
            }
        };
        Some(ok(body))
    }

    /// The `division` of any rankings page of `list_id`, or one made from a
    /// season's tree that lists it.
    fn list_division(&self, list_id: i64) -> Option<serde_json::Value> {
        let prefix = format!("{list_id}-");
        for (stem, page) in json_files(&self.dir.join("rankings")) {
            if !stem.starts_with(&prefix) {
                continue;
            }
            if let Some(division) = page.get("division").filter(|d| !d.is_null()) {
                return Some(division.clone());
            }
        }
        for (stem, nav) in json_files(&self.dir.join("navinfo")) {
            let Some(Ok(season)) = stem.split('-').next().map(|s| s.parse::<i64>()) else {
                continue;
            };
            let listed = ["regions", "tree"].iter().any(|k| {
                nav.get(k)
                    .and_then(|r| r.as_array())
                    .is_some_and(|r| r.iter().any(|d| d.get("id") == Some(&list_id.into())))
            });
            if listed {
                return Some(serde_json::json!({"ID": list_id, "SeasonID": season}));
            }
        }
        None
    }

    /// `GetNavInfo`: the State's own subtree when the token is of a State
    /// list that has one, otherwise the season's tree.
    fn navinfo(&self, query: &HashMap<String, String>, req: &Request) -> Option<FetchResponse> {
        let season: i64 = query.get("seasonId")?.parse().ok()?;
        let list = header(req, "anettokens")
            .and_then(|t| t.strip_prefix(RANKINGS_TOKEN_PREFIX))
            .and_then(|l| l.parse::<i64>().ok());
        let own = list.and_then(|l| self.read(&layout::state_navinfo(season, l)));
        own.or_else(|| self.read(&layout::navinfo(season))).map(ok)
    }

    fn answer(&self, req: &Request) -> Option<FetchResponse> {
        let (path, query) = split_url(req.url());
        let body: serde_json::Value = req
            .body()
            .and_then(|b| serde_json::from_str(b).ok())
            .unwrap_or(serde_json::Value::Null);
        let num = |k: &str| query.get(k).and_then(|v| v.parse::<i64>().ok());
        let meet_token_ok = header(req, "anettokens") == Some(MEET_TOKEN);
        match (req.method(), path.rsplit('/').next()?) {
            (Method::Post, "GetRankings") => self.rankings(&body),
            (Method::Get, "GetNavInfo") => self.navinfo(&query, req),
            (Method::Get, "GetAthleteBioData") => {
                self.read(&layout::bio(num("athleteId")?)).map(ok)
            }
            (Method::Get, "Team") => self
                .read(&layout::teamnav(num("team")?, num("season")?))
                .map(ok),
            (Method::Get, "GetMeetData") => {
                let raw = self.read(&layout::meet(num("meetId")?))?;
                Some(ok(set_top_level_string(&raw, "jwtMeet", MEET_TOKEN)))
            }
            (Method::Get, "GetEventListData") if !meet_token_ok => Some(refused()),
            (Method::Get, "GetEventListData") => {
                self.read(&layout::meet_events(num("meetId")?)).map(ok)
            }
            (Method::Post, "GetEntriesData") if !meet_token_ok => Some(refused()),
            (Method::Post, "GetEntriesData") => {
                let q = EntriesQuery {
                    meet_id: body.get("meetId")?.as_i64()?,
                    gender: Gender::from_bio(body.get("gender")?.as_str()?)?,
                    div_id: body.get("divId")?.as_i64()?,
                    event_short: body.get("eventShort")?.as_str()?,
                };
                self.read(&layout::meet_entries(&q)).map(ok)
            }
            (Method::Get, "search") => Some(ok(self
                .read(&layout::search(query.get("q")?))
                .unwrap_or_else(|| r#"{"response": {"docs": []}}"#.to_string()))),
            _ => None,
        }
    }
}

#[async_trait]
impl Fetcher for FixtureFetcher {
    async fn fetch(&self, req: &Request) -> Result<FetchResponse> {
        Ok(self.answer(req).unwrap_or_else(|| FetchResponse {
            status: 404,
            body: format!(r#"{{"Message": "no fixture for {req}"}}"#),
        }))
    }

    fn challenge_advice(&self) -> &'static str {
        "the fixture transport never sees a challenge"
    }
}

fn ok(body: String) -> FetchResponse {
    FetchResponse { status: 200, body }
}

fn refused() -> FetchResponse {
    FetchResponse {
        status: 403,
        body: r#"{"Message": "Authorization has been denied for this request."}"#.to_string(),
    }
}

fn header<'a>(req: &'a Request, name: &str) -> Option<&'a str> {
    req.headers().find(|(n, _)| *n == name).map(|(_, v)| v)
}

/// A URL's path and its decoded query parameters.
fn split_url(url: &str) -> (&str, HashMap<String, String>) {
    let (path, query) = url.split_once('?').unwrap_or((url, ""));
    let params = query
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| {
            let v = v.replace('+', " ");
            (
                k.to_string(),
                percent_decode_str(&v).decode_utf8_lossy().to_string(),
            )
        })
        .collect();
    (path, params)
}

/// The `*.json` files directly in `dir` that parse, by file stem, sorted
/// (none when it is missing).
fn json_files(dir: &Path) -> Vec<(String, serde_json::Value)> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().is_some_and(|e| e == "json"))
                .collect()
        })
        .unwrap_or_default();
    paths.sort();
    paths
        .into_iter()
        .filter_map(|path| {
            let stem = path.file_stem()?.to_string_lossy().to_string();
            let value = serde_json::from_str(&std::fs::read_to_string(&path).ok()?).ok()?;
            Some((stem, value))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A token request for a list with no page of its own is answered with
    /// the list's division (season included) from a page or the tree, as
    /// Athletic.net does, so the data source's season hops run.
    #[tokio::test]
    async fn a_token_answer_carries_the_lists_division() {
        let fetcher = FixtureFetcher::new("tests/fixtures");
        // California (its division from its 400m pages) and Central (from the tree).
        for (list, season) in [(985_316_289, 2099), (901_802_829, 2099)] {
            let grades = Grades::all();
            let q = PageQuery {
                list_id: list,
                season: 0,
                gender: Gender::F,
                event: "100m",
                grades: &grades,
                page: 1,
            };
            let body = serde_json::json!({"divListId": q.list_id, "gender": "f", "eventShort": q.event, "qParams": {}});
            let req = Request::post_json(
                "https://www.athletic.net/api/v1/tfRankings/GetRankings",
                &body,
            )
            .unwrap();
            let answer = fetcher.fetch(&req).await.unwrap();
            assert!(
                !std::path::Path::new(&format!(
                    "tests/fixtures/{}",
                    layout::rankings(&q).display()
                ))
                .exists()
            );
            let v: serde_json::Value = serde_json::from_str(&answer.body).unwrap();
            assert_eq!(v["division"]["ID"], list, "{v}");
            assert_eq!(v["division"]["SeasonID"], season, "{v}");
            assert_eq!(
                v["jwtTFTopReport"],
                format!("{RANKINGS_TOKEN_PREFIX}{list}")
            );
        }
    }

    #[test]
    fn splits_a_url() {
        let (path, q) = split_url("https://x/api/v1/AutoComplete/search?q=Jane%20O%27Doe&fq=a");
        assert_eq!(path, "https://x/api/v1/AutoComplete/search");
        assert_eq!(q["q"], "Jane O'Doe");
    }
}
