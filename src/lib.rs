//! `ath` - generate meet seed sheets from Athletic.net.
//!
//! See `docs/HANDOFF.md` for the full spec and `CONTEXT.md` for the domain
//! vocabulary. The seeding pipeline is driven through a single [`source::DataSource`]
//! seam so it can be tested hermetically against fixtures.

pub mod area;
pub mod athletic;
pub mod cache;
pub mod contract;
pub mod http;
pub mod list;
pub mod mark;
pub mod meet;
pub mod model;
pub mod output;
pub mod profile;
pub mod progress;
pub mod rank;
pub mod record;
pub mod relay;
pub mod replay;
pub mod resolve;
pub mod roster;
pub mod seed;
pub mod seed_relay;
pub mod source;
