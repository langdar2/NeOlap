//! Declarative import specification (TOML).

use std::collections::BTreeMap;
use std::str::FromStr;

use serde::{Deserialize, Deserializer};

use crate::{Error, Result};

/// How the data of the file is combined with the cube.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoadMode {
    /// Deletes exactly the area the file covers (every member that occurs in the file, per
    /// dimension), then loads. Repeatable: loading the same file twice equals loading it once.
    ReplaceArea,
    /// Adds the values to the existing cells.
    Add,
    /// Overwrites the cells named in the file; other cells stay.
    #[default]
    Overwrite,
}

/// What to do with rows that name members the dimension does not know.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum UnknownPolicy {
    /// The import is rejected as a whole and lists the rows.
    #[default]
    Reject,
    /// Such rows are skipped and counted.
    Skip,
    /// Unknown members are created as children of the named member.
    CreateUnder(String),
}

impl FromStr for UnknownPolicy {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, String> {
        let t = s.trim();
        match t {
            "reject" => Ok(UnknownPolicy::Reject),
            "skip" => Ok(UnknownPolicy::Skip),
            _ => {
                match t.strip_prefix("create_under:") {
                    Some(name) => {
                        let name = name.trim().trim_matches(['"', '\'']).trim();
                        if name.is_empty() {
                            Err("create_under needs a member name, e.g. create_under:Nicht zugeordnet".into())
                        } else {
                            Ok(UnknownPolicy::CreateUnder(name.to_string()))
                        }
                    }
                    None => Err(format!(
                        "unknown_member must be reject, skip or create_under:<member>, not '{t}'"
                    )),
                }
            }
        }
    }
}

impl<'de> Deserialize<'de> for UnknownPolicy {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        String::deserialize(d)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceSpec {
    /// Path, relative to the directory of the specification.
    pub file: Option<String>,
    /// One character, default `,`.
    pub delimiter: Option<String>,
    /// `.` (default) or `,`. Ignored for xlsx, which holds real numbers.
    pub decimal: Option<String>,
    /// Worksheet of an xlsx file (default: the first).
    pub sheet: Option<String>,
}

/// Column of a dimension: `years = "Jahr"` or `months = { column = "Monat", alias = true }`.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum MapEntry {
    Column(String),
    Detailed {
        column: String,
        /// Whether aliases are accepted besides member names (default: names only).
        #[serde(default)]
        alias: bool,
    },
}

impl MapEntry {
    pub fn column(&self) -> &str {
        match self {
            MapEntry::Column(c) | MapEntry::Detailed { column: c, .. } => c,
        }
    }

    pub fn alias(&self) -> bool {
        matches!(self, MapEntry::Detailed { alias: true, .. })
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    #[serde(default)]
    pub mode: LoadMode,
    #[serde(default)]
    pub unknown_member: UnknownPolicy,
    /// Dimension that holds the `[values]` columns (default: the only dimension of kind Measures).
    pub value_dimension: Option<String>,
}

/// Import specification. Every dimension of the cube must be given exactly once: in `map`
/// (column), `fixed` (one member for the whole file) or as the dimension of the `values` columns.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportSpec {
    pub name: String,
    pub cube: String,
    #[serde(default)]
    pub source: SourceSpec,
    #[serde(default)]
    pub map: BTreeMap<String, MapEntry>,
    #[serde(default)]
    pub fixed: BTreeMap<String, String>,
    /// Wide format: member of the value dimension → column.
    #[serde(default)]
    pub values: BTreeMap<String, String>,
    /// Long format: the column that holds the number (the measure is then a normal `map` column).
    pub value_column: Option<String>,
    #[serde(default)]
    pub options: Options,
}

impl ImportSpec {
    pub fn from_toml(text: &str) -> Result<ImportSpec> {
        toml::from_str(text).map_err(|e| Error::Parse(format!("import specification: {e}")))
    }

    pub fn delimiter(&self) -> Result<u8> {
        match self.source.delimiter.as_deref() {
            None => Ok(b','),
            Some("\\t") => Ok(b'\t'),
            Some(d) if d.len() == 1 => Ok(d.as_bytes()[0]),
            Some(d) => Err(Error::Parse(format!(
                "delimiter must be one character, not '{d}'"
            ))),
        }
    }

    pub fn decimal(&self) -> Result<char> {
        match self.source.decimal.as_deref() {
            None | Some(".") => Ok('.'),
            Some(",") => Ok(','),
            Some(d) => Err(Error::Parse(format!(
                "decimal must be '.' or ',', not '{d}'"
            ))),
        }
    }
}
