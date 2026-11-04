//! Export of cube areas as a table (CSV, xlsx).

use serde::Serialize;

use crate::cube::{CellValue, Cube, CubeProvider};
use crate::{Error, MemberId, Result};

/// Long: one row per cell. Wide: the members of one dimension become columns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Layout {
    Long,
    Wide { dimension: String },
}

#[derive(Debug, Clone)]
pub struct ExportSpec {
    /// Members per dimension (name → member names); dimensions not listed export all base members,
    /// or all members with `aggregates`.
    pub area: Vec<(String, Vec<String>)>,
    pub layout: Layout,
    /// Include aggregates (otherwise only base members).
    pub aggregates: bool,
    /// Calculate cells with the rules (otherwise stored values and plain aggregation).
    pub with_rules: bool,
    /// Skip empty cells (long format) / rows without any value (wide format).
    pub suppress_empty: bool,
}

impl Default for ExportSpec {
    fn default() -> Self {
        ExportSpec {
            area: Vec::new(),
            layout: Layout::Long,
            aggregates: false,
            with_rules: false,
            suppress_empty: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum Cell {
    Empty,
    Num(f64),
    Text(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExportTable {
    pub headers: Vec<String>,
    pub rows: Vec<Vec<Cell>>,
    /// How many of the first columns are member names; every column after them holds values.
    /// Consumers take the kind of a column from this, never from its content (a member may be
    /// called "2026", and a measure may be empty everywhere).
    pub n_name_columns: usize,
}

/// Safety limit for one export.
const MAX_EXPORT_CELLS: u128 = 50_000_000;

fn members_of(
    cube: &Cube,
    d: usize,
    wanted: Option<&Vec<String>>,
    aggregates: bool,
) -> Result<Vec<MemberId>> {
    let dim = &cube.dimensions()[d];
    match wanted {
        Some(names) => names.iter().map(|n| dim.member(n)).collect(),
        None => {
            let mut v: Vec<MemberId> = (0..dim.len_slots() as u32)
                .filter(|&m| dim.member_name(m).is_some() && (aggregates || dim.is_leaf(m)))
                .collect();
            // tree order: DFS ordinal for base members, then aggregates by id
            v.sort_by_key(|&m| (dim.leaf_ordinal(m).map_or((1, m), |o| (0, o)), m));
            Ok(v)
        }
    }
}

/// Evaluates the area into a table.
pub fn export_table(
    cube: &Cube,
    spec: &ExportSpec,
    db: Option<&dyn CubeProvider>,
) -> Result<ExportTable> {
    let n = cube.dimensions().len();
    let mut lists: Vec<Vec<MemberId>> = Vec::with_capacity(n);
    for d in 0..n {
        let wanted = spec
            .area
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(cube.dimensions()[d].name()))
            .map(|(_, v)| v);
        lists.push(members_of(cube, d, wanted, spec.aggregates)?);
    }
    for (k, _) in &spec.area {
        if !cube
            .dimensions()
            .iter()
            .any(|d| d.name().eq_ignore_ascii_case(k))
        {
            return Err(Error::NotFound {
                kind: "dimension",
                name: k.clone(),
            });
        }
    }
    let total: u128 = lists.iter().map(|l| l.len() as u128).product();
    if total > MAX_EXPORT_CELLS {
        return Err(Error::InvalidOperation(format!(
            "the area has {total} cells; the limit of one export is {MAX_EXPORT_CELLS}, please narrow it down"
        )));
    }
    let value_of = |addr: &[MemberId]| -> Result<CellValue> {
        if spec.with_rules {
            cube.cell(addr, db)
        } else {
            Ok(cube.value(addr)?.map_or(CellValue::Empty, CellValue::Num))
        }
    };
    let to_cell = |v: CellValue| match v {
        CellValue::Empty => Cell::Empty,
        CellValue::Num(x) => Cell::Num(x),
        CellValue::Text(t) => Cell::Text(t),
        CellValue::Error(e) => Cell::Text(e.to_string()),
    };
    let name = |d: usize, m: MemberId| {
        Cell::Text(
            cube.dimensions()[d]
                .member_name(m)
                .unwrap_or("")
                .to_string(),
        )
    };

    let mut headers: Vec<String> = Vec::new();
    let mut rows: Vec<Vec<Cell>> = Vec::new();
    let mut idx = vec![0usize; n];
    let wide = match &spec.layout {
        Layout::Long => None,
        Layout::Wide { dimension } => Some(
            cube.dimensions()
                .iter()
                .position(|d| d.name().eq_ignore_ascii_case(dimension))
                .ok_or_else(|| Error::NotFound {
                    kind: "dimension",
                    name: dimension.clone(),
                })?,
        ),
    };
    match wide {
        None => headers.extend(
            cube.dimensions()
                .iter()
                .map(|d| d.name().to_string())
                .chain(["value".to_string()]),
        ),
        Some(w) => {
            headers.extend(
                cube.dimensions()
                    .iter()
                    .enumerate()
                    .filter(|(d, _)| *d != w)
                    .map(|(_, d)| d.name().to_string()),
            );
            headers.extend(lists[w].iter().map(|&m| {
                cube.dimensions()[w]
                    .member_name(m)
                    .unwrap_or("")
                    .to_string()
            }));
        }
    }
    if lists.iter().any(Vec::is_empty) {
        return Ok(ExportTable {
            headers,
            rows,
            n_name_columns: if wide.is_some() { n - 1 } else { n },
        });
    }
    let mut addr: Vec<MemberId> = vec![0; n];
    'outer: loop {
        for d in 0..n {
            addr[d] = lists[d][idx[d]];
        }
        match wide {
            None => {
                let v = to_cell(value_of(&addr)?);
                if !(spec.suppress_empty && v == Cell::Empty) {
                    let mut row: Vec<Cell> = (0..n).map(|d| name(d, addr[d])).collect();
                    row.push(v);
                    rows.push(row);
                }
            }
            Some(w) if idx[w] == 0 => {
                let mut vals = Vec::with_capacity(lists[w].len());
                for &m in &lists[w] {
                    addr[w] = m;
                    vals.push(to_cell(value_of(&addr)?));
                }
                if !(spec.suppress_empty && vals.iter().all(|v| *v == Cell::Empty)) {
                    let mut row: Vec<Cell> = (0..n)
                        .filter(|&d| d != w)
                        .map(|d| name(d, addr[d]))
                        .collect();
                    row.extend(vals);
                    rows.push(row);
                }
            }
            Some(_) => {}
        }
        let mut d = n;
        loop {
            if d == 0 {
                break 'outer;
            }
            d -= 1;
            idx[d] += 1;
            if idx[d] < lists[d].len() {
                break;
            }
            idx[d] = 0;
        }
    }
    Ok(ExportTable {
        headers,
        rows,
        n_name_columns: if wide.is_some() { n - 1 } else { n },
    })
}

fn csv_value(c: &Cell, decimal: char) -> String {
    match c {
        Cell::Empty => String::new(),
        Cell::Text(t) => t.clone(),
        Cell::Num(v) => {
            let s = v.to_string();
            if decimal == ',' {
                s.replace('.', ",")
            } else {
                s
            }
        }
    }
}

/// CSV text with the given delimiter; `decimal` is `.` or `,`.
pub fn to_csv(table: &ExportTable, delimiter: u8, decimal: char) -> Result<Vec<u8>> {
    let mut w = csv::WriterBuilder::new()
        .delimiter(delimiter)
        .from_writer(Vec::new());
    w.write_record(&table.headers)
        .map_err(|e| Error::Storage(e.to_string()))?;
    for row in &table.rows {
        w.write_record(row.iter().map(|c| csv_value(c, decimal)))
            .map_err(|e| Error::Storage(e.to_string()))?;
    }
    w.into_inner().map_err(|e| Error::Storage(e.to_string()))
}

/// xlsx file with real numbers; rows beyond the sheet limit are refused.
pub fn to_xlsx(table: &ExportTable, sheet_name: &str) -> Result<Vec<u8>> {
    use rust_xlsxwriter::Workbook;
    if table.rows.len() >= 1_048_575 {
        return Err(Error::InvalidOperation(
            "too many rows for one xlsx sheet (limit 1,048,575); export as CSV instead".into(),
        ));
    }
    let err = |e: rust_xlsxwriter::XlsxError| Error::Storage(e.to_string());
    let mut wb = Workbook::new();
    let ws = wb.add_worksheet();
    let safe: String = sheet_name
        .chars()
        .filter(|c| !"[]:*?/\\".contains(*c))
        .take(31)
        .collect();
    ws.set_name(if safe.is_empty() { "Export" } else { &safe })
        .map_err(err)?;
    for (c, h) in table.headers.iter().enumerate() {
        ws.write_string(0, c as u16, h).map_err(err)?;
    }
    for (r, row) in table.rows.iter().enumerate() {
        for (c, cell) in row.iter().enumerate() {
            let (r, c) = (r as u32 + 1, c as u16);
            match cell {
                Cell::Empty => None,
                Cell::Num(v) => Some(ws.write_number(r, c, *v)),
                Cell::Text(t) => Some(ws.write_string(r, c, t)),
            }
            .transpose()
            .map_err(err)?;
        }
    }
    wb.save_to_buffer().map_err(err)
}
