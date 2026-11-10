//! Reading a Mode B roster CSV.
//!
//! Only `name` is required. `athlete_id` pins a known athlete (skips search -
//! the only path exercised in the walking skeleton). `school`/`city`/`state`
//! are optional locators used later for resolution.

use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RosterRow {
    pub name: String,
    #[serde(default)]
    pub athlete_id: Option<i64>,
    #[serde(default)]
    pub school: Option<String>,
    #[serde(default)]
    pub city: Option<String>,
    #[serde(default)]
    pub state: Option<String>,
}

/// Read a roster CSV (headers required).
pub fn read_roster(path: impl AsRef<Path>) -> Result<Vec<RosterRow>> {
    let path = path.as_ref();
    let mut reader = csv::Reader::from_path(path)
        .with_context(|| format!("opening roster {}", path.display()))?;
    let mut rows = Vec::new();
    for (i, rec) in reader.deserialize().enumerate() {
        // i is 0-based over data rows; the physical CSV line is +1 for the header.
        let row: RosterRow =
            rec.with_context(|| format!("parsing roster data row {} (CSV line {})", i + 1, i + 2))?;
        rows.push(row);
    }
    Ok(rows)
}

/// Persist resolved `athlete_id`s back into the roster file, in place, so later
/// runs skip the search.
///
/// This patches only the `athlete_id` column of the existing CSV - every other
/// column (including ones `ath` does not model) is preserved verbatim. `updated`
/// must be in the same order and count as the file's data rows. Rows whose
/// `athlete_id` is `None` are left untouched.
pub fn apply_resolved_ids(path: impl AsRef<Path>, updated: &[RosterRow]) -> Result<()> {
    let path = path.as_ref();

    let mut reader = csv::Reader::from_path(path)
        .with_context(|| format!("opening roster {} to update", path.display()))?;
    let mut headers: Vec<String> = reader
        .headers()
        .with_context(|| format!("reading roster headers {}", path.display()))?
        .iter()
        .map(str::to_string)
        .collect();
    let records: Vec<csv::StringRecord> =
        reader
            .records()
            .collect::<std::result::Result<_, _>>()
            .with_context(|| format!("reading roster {}", path.display()))?;

    if records.len() != updated.len() {
        bail!(
            "roster {} changed underneath us ({} rows on disk, {} resolved)",
            path.display(),
            records.len(),
            updated.len()
        );
    }

    let id_idx = match headers.iter().position(|h| h == "athlete_id") {
        Some(i) => i,
        None => {
            headers.push("athlete_id".to_string());
            headers.len() - 1
        }
    };

    let mut writer = csv::Writer::from_path(path)
        .with_context(|| format!("opening roster {} for write", path.display()))?;
    writer.write_record(&headers)?;
    for (record, row) in records.iter().zip(updated) {
        let mut fields: Vec<String> = record.iter().map(str::to_string).collect();
        fields.resize(headers.len(), String::new());
        if let Some(id) = row.athlete_id {
            fields[id_idx] = id.to_string();
        }
        writer.write_record(&fields)?;
    }
    writer
        .flush()
        .with_context(|| format!("writing roster {}", path.display()))?;
    Ok(())
}
