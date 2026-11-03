//! Reading tabular sources (CSV, xlsx) as a header plus a stream of records.

use std::io::Cursor;
use std::path::Path;

use calamine::{Data, Reader, Xlsx, open_workbook_from_rs};
use csv::StringRecord;

use super::spec::SourceSpec;
use crate::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    Csv,
    Xlsx,
}

impl SourceKind {
    pub fn from_path(path: &Path) -> SourceKind {
        match path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("xlsx" | "xlsm") => SourceKind::Xlsx,
            _ => SourceKind::Csv,
        }
    }
}

/// `(line, record)`; the line is the row number in the file or sheet.
pub type Row = Result<(u64, StringRecord)>;

pub struct Table<'a> {
    pub headers: Vec<String>,
    pub rows: Box<dyn Iterator<Item = Row> + 'a>,
}

fn cell_text(d: &Data) -> String {
    match d {
        Data::Empty => String::new(),
        Data::String(s) | Data::DateTimeIso(s) | Data::DurationIso(s) => s.trim().to_string(),
        Data::Int(i) => i.to_string(),
        Data::Float(f) if f.fract() == 0.0 && f.abs() < 1e15 => format!("{}", *f as i64),
        Data::Float(f) => f.to_string(),
        Data::Bool(b) => b.to_string(),
        Data::DateTime(t) => t.as_f64().to_string(),
        Data::Error(e) => format!("#{e:?}"),
    }
}

pub fn read_table<'a>(
    kind: SourceKind,
    data: &'a [u8],
    src: &SourceSpec,
    delimiter: u8,
) -> Result<Table<'a>> {
    match kind {
        SourceKind::Csv => {
            let data = data.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(data);
            let mut rdr = csv::ReaderBuilder::new()
                .delimiter(delimiter)
                .has_headers(true)
                .flexible(true)
                .trim(csv::Trim::All)
                .from_reader(data);
            let headers = rdr
                .headers()
                .map_err(|e| Error::Parse(e.to_string()))?
                .iter()
                .map(|h| h.trim().to_string())
                .collect();
            let rows = rdr.into_records().map(|r| {
                let rec = r.map_err(|e| Error::Parse(e.to_string()))?;
                let line = rec.position().map_or(0, csv::Position::line);
                Ok((line, rec))
            });
            Ok(Table {
                headers,
                rows: Box::new(rows),
            })
        }
        SourceKind::Xlsx => {
            let mut wb: Xlsx<_> = open_workbook_from_rs(Cursor::new(data))
                .map_err(|e| Error::Parse(format!("xlsx file: {e}")))?;
            let sheet = match &src.sheet {
                Some(s) => s.clone(),
                None => wb
                    .sheet_names()
                    .first()
                    .cloned()
                    .ok_or_else(|| Error::Parse("the xlsx file has no worksheet".into()))?,
            };
            let range = wb
                .worksheet_range(&sheet)
                .map_err(|e| Error::Parse(format!("worksheet '{sheet}': {e}")))?;
            let first = range.start().map_or(1, |s| u64::from(s.0) + 1);
            let mut rows = range.rows();
            let headers = rows
                .next()
                .map(|r| r.iter().map(cell_text).collect())
                .ok_or_else(|| Error::Parse(format!("worksheet '{sheet}' is empty")))?;
            let records: Vec<Row> = rows
                .enumerate()
                .map(|(i, r)| {
                    Ok((
                        first + 1 + i as u64,
                        StringRecord::from(r.iter().map(cell_text).collect::<Vec<_>>()),
                    ))
                })
                .collect();
            Ok(Table {
                headers,
                rows: Box::new(records.into_iter()),
            })
        }
    }
}
