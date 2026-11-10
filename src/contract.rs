//! The output contract every data command follows (ADR 0007).
//!
//! - JSON when stdout is not a terminal, a readable table when it is; `--json`
//!   / `--table` force either.
//! - JSON is one object with a top-level `schema` version, so the Python
//!   reports under `tools/` can tell when the shape changes.
//! - Every result carries the Athletic.net URL for the same view (each
//!   [`Report`] includes it in its own fields).
//!
//! A command builds one value implementing [`Report`] and hands it to [`emit`].

use std::io::{self, IsTerminal, Write};

use anyhow::Result;
use serde::Serialize;

/// How a command prints its result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Json,
    Table,
}

impl Format {
    /// The format for this run: a forced choice wins, otherwise JSON unless
    /// stdout is a terminal.
    pub fn choose(force_json: bool, force_table: bool, stdout_is_terminal: bool) -> Format {
        match (force_json, force_table) {
            (true, _) => Format::Json,
            (_, true) => Format::Table,
            _ if stdout_is_terminal => Format::Table,
            _ => Format::Json,
        }
    }

    /// [`Format::choose`] against the real stdout.
    pub fn detect(force_json: bool, force_table: bool) -> Format {
        Format::choose(force_json, force_table, io::stdout().is_terminal())
    }
}

/// A command's result: serializable to JSON and renderable as a table.
pub trait Report: Serialize {
    /// The JSON shape's version. Bump it on any change a consumer could notice
    /// (a renamed, removed or retyped field); adding a field does not need one.
    const SCHEMA: u32;

    /// The human-readable rendering.
    fn table(&self) -> String;
}

/// The JSON envelope: `schema` first, then the report's own fields.
#[derive(Serialize)]
struct Envelope<'a, T> {
    schema: u32,
    #[serde(flatten)]
    report: &'a T,
}

/// The report as pretty JSON with its `schema` version (no trailing newline).
pub fn to_json<T: Report>(report: &T) -> Result<String> {
    Ok(serde_json::to_string_pretty(&Envelope {
        schema: T::SCHEMA,
        report,
    })?)
}

/// Write `report` to `out` in `format`, ending with a newline.
pub fn emit<T: Report>(report: &T, format: Format, out: &mut impl Write) -> Result<()> {
    let text = match format {
        Format::Json => to_json(report)?,
        Format::Table => report.table(),
    };
    writeln!(out, "{}", text.trim_end())?;
    Ok(())
}

/// Left-aligned text columns separated by two spaces, the building block for
/// [`Report::table`]. Trailing whitespace is trimmed from every line.
pub fn columns(rows: &[Vec<String>]) -> String {
    let n = rows.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<usize> = (0..n)
        .map(|i| {
            rows.iter()
                .filter_map(|r| r.get(i))
                .map(|c| c.chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect();
    let mut out = String::new();
    for row in rows {
        let mut line = String::new();
        for (i, cell) in row.iter().enumerate() {
            if i > 0 {
                line.push_str("  ");
            }
            line.push_str(cell);
            let pad = widths[i] - cell.chars().count();
            line.extend(std::iter::repeat_n(' ', pad));
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_unless_terminal_and_flags_force_either() {
        assert_eq!(Format::choose(false, false, false), Format::Json);
        assert_eq!(Format::choose(false, false, true), Format::Table);
        assert_eq!(Format::choose(true, false, true), Format::Json);
        assert_eq!(Format::choose(false, true, false), Format::Table);
    }

    #[derive(Serialize)]
    struct Demo {
        url: &'static str,
    }

    impl Report for Demo {
        const SCHEMA: u32 = 7;
        fn table(&self) -> String {
            columns(&[vec!["URL".into(), self.url.into()]])
        }
    }

    #[test]
    fn json_carries_schema_first() -> Result<()> {
        let mut out = Vec::new();
        emit(&Demo { url: "u" }, Format::Json, &mut out)?;
        assert_eq!(
            String::from_utf8(out)?,
            "{\n  \"schema\": 7,\n  \"url\": \"u\"\n}\n"
        );
        Ok(())
    }

    #[test]
    fn table_rendering() -> Result<()> {
        let mut out = Vec::new();
        emit(&Demo { url: "u" }, Format::Table, &mut out)?;
        assert_eq!(String::from_utf8(out)?, "URL  u\n");
        Ok(())
    }

    #[test]
    fn columns_align_and_trim() {
        let rows = vec![
            vec!["a".to_string(), "long".to_string(), "x".to_string()],
            vec!["bbb".to_string(), "s".to_string()],
        ];
        assert_eq!(columns(&rows), "a    long  x\nbbb  s\n");
    }
}
