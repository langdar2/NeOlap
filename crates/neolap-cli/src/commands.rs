//! Command definitions and their implementation.

use std::io::{IsTerminal, Read};
use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};
use neolap_core::cube::{BulkMode, CellValue};
use neolap_core::dimension::{DimensionKind, ImportMode};
use neolap_core::exchange::{self, ExportSpec, ImportSpec, Layout, SourceKind};
use neolap_core::persist::{JournalQuery, Role};
use neolap_core::{Cube, Database, Dimension, Input, WriteContext};

use crate::output::{Output, V};

type Res<T> = std::result::Result<T, String>;

fn e<E: std::fmt::Display>(err: E) -> String {
    err.to_string()
}

#[derive(Parser)]
#[command(
    name = "neolap",
    version,
    about = "NeOlap – lightweight multidimensional database"
)]
pub struct Cli {
    /// Output as JSON instead of tables.
    #[arg(long, global = true)]
    pub json: bool,
    /// User name recorded in the journal for changes (default: $NEOLAP_USER, else $USER).
    #[arg(long, global = true, env = "NEOLAP_USER")]
    pub user: Option<String>,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Print version information.
    Version,
    /// Create an empty database file.
    New { db: PathBuf },
    /// Create a database with a built-in sample model (tiny).
    Demo { name: String, db: PathBuf },
    /// Show dimensions, cubes, rules and users of a database.
    Info { db: PathBuf },
    /// Manage users.
    #[command(subcommand)]
    User(UserCommand),
    /// Dimensions: import members from CSV.
    #[command(subcommand)]
    Dim(DimCommand),
    /// Run an import specification.
    Load {
        db: PathBuf,
        /// Specification file (TOML), or the name of one stored in the database.
        spec: String,
        /// Only check and report, change nothing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Read one cell: `get <db> <cube> <member>…` (one member per dimension, in cube order).
    Get {
        db: PathBuf,
        cube: String,
        members: Vec<String>,
        /// Calculate with the cube's rules.
        #[arg(long)]
        rules: bool,
    },
    /// Write one cell: `set <db> <cube> <member>… <value>`. Aggregates are splashed.
    Set {
        db: PathBuf,
        cube: String,
        /// One member per dimension, then the value (`500`, `#500`, `+50`, `+5%`, empty = delete).
        args: Vec<String>,
        /// Distribution mode for aggregates: proportional (default) or equal.
        #[arg(long, value_parser = ["proportional", "equal"])]
        mode: Option<String>,
    },
    /// Export an area as CSV or xlsx.
    Export {
        db: PathBuf,
        cube: String,
        /// Area, e.g. `years=2026,version=Actual` (members separated by `|` within a dimension).
        #[arg(long)]
        area: Option<String>,
        /// Target file (.csv or .xlsx); without it, CSV goes to the standard output.
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Wide format: the members of this dimension become columns.
        #[arg(long)]
        wide: Option<String>,
        /// Include aggregates (default: base members only).
        #[arg(long)]
        aggregates: bool,
        /// Calculate with the cube's rules.
        #[arg(long)]
        rules: bool,
        #[arg(long, default_value = ";")]
        delimiter: String,
        /// Decimal separator for CSV: `.` or `,`.
        #[arg(long, default_value = ".")]
        decimal: String,
    },
    /// Rules of a cube.
    #[command(subcommand)]
    Rules(RulesCommand),
    /// Model as readable files (dimensions, cubes, rules, import specifications).
    #[command(subcommand)]
    Model(ModelCommand),
    /// Consistent copy of the database.
    Backup { db: PathBuf, target: PathBuf },
    /// Show the change journal.
    Journal {
        db: PathBuf,
        #[arg(long)]
        cube: Option<String>,
        #[arg(long = "by")]
        by: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Start the server with web UI.
    #[command(flatten)]
    Data(DataCommand),
}

/// Commands handled by the binary itself (they need the server runtime).
#[derive(Subcommand)]
pub enum DataCommand {
    /// Start the server with web UI.
    Serve {
        #[arg(long, env = "NEOLAP_LISTEN", default_value = "127.0.0.1:8080")]
        listen: std::net::SocketAddr,
        #[arg(long, env = "NEOLAP_DATA_DIR", default_value = "./data")]
        data_dir: PathBuf,
    },
    /// Check whether a running server is healthy (exit code 0 = healthy).
    Healthcheck {
        #[arg(
            long,
            env = "NEOLAP_HEALTHCHECK_ADDR",
            default_value = "127.0.0.1:8080"
        )]
        addr: String,
        #[arg(long, default_value_t = 3)]
        timeout: u64,
    },
}

#[derive(Subcommand)]
pub enum UserCommand {
    /// Add a user. The password is read from $NEOLAP_PASSWORD or asked for (never an argument).
    Add {
        db: PathBuf,
        name: String,
        #[arg(long, default_value = "planner")]
        role: String,
    },
    /// List users.
    List { db: PathBuf },
    /// Change the role of a user.
    Role {
        db: PathBuf,
        name: String,
        role: String,
    },
    /// Set a new password (invalidates the user's tokens).
    Password { db: PathBuf, name: String },
    /// Remove a user.
    Remove { db: PathBuf, name: String },
    /// Create an API token (shown once).
    Token {
        db: PathBuf,
        name: String,
        #[arg(long, default_value = "")]
        label: String,
    },
}

#[derive(Subcommand)]
pub enum DimCommand {
    /// Import members from a CSV file (`child;parent;weight;alias;<attributes>`) or level columns.
    Import {
        db: PathBuf,
        dimension: String,
        file: PathBuf,
        /// `parent-child` (default) or `levels`.
        #[arg(long, default_value = "parent-child", value_parser = ["parent-child", "levels"])]
        format: String,
        /// Also remove members that are not in the file.
        #[arg(long)]
        sync: bool,
        #[arg(long, default_value = ";")]
        delimiter: String,
        /// Kind of a new dimension: generic, time, version or measures.
        #[arg(long, default_value = "generic")]
        kind: String,
    },
}

#[derive(Subcommand)]
pub enum RulesCommand {
    /// Check the syntax and the references of a rule file without saving.
    Check {
        db: PathBuf,
        cube: String,
        file: PathBuf,
    },
    /// Save the rule file as the rules of the cube.
    Apply {
        db: PathBuf,
        cube: String,
        file: PathBuf,
    },
    /// Print the rules of the cube.
    Show { db: PathBuf, cube: String },
}

#[derive(Subcommand)]
pub enum ModelCommand {
    /// Write the model (no data) as files into a directory.
    Export { db: PathBuf, dir: PathBuf },
    /// Read a model directory into the database.
    Import { db: PathBuf, dir: PathBuf },
}

fn user_of(cli: &Cli) -> String {
    cli.user
        .clone()
        .or_else(|| std::env::var("USER").ok())
        .or_else(|| std::env::var("USERNAME").ok())
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| "cli".into())
}

fn password(prompt: &str) -> Res<String> {
    if let Ok(p) = std::env::var("NEOLAP_PASSWORD") {
        return Ok(p);
    }
    if !std::io::stdin().is_terminal() {
        let mut s = String::new();
        std::io::stdin().read_to_string(&mut s).map_err(e)?;
        return Ok(s.trim_end_matches(['\n', '\r']).to_string());
    }
    let first = rpassword::prompt_password(format!("{prompt}: ")).map_err(e)?;
    let second = rpassword::prompt_password("Repeat: ").map_err(e)?;
    if first != second {
        return Err("the passwords differ".into());
    }
    Ok(first)
}

fn kind_of(s: &str) -> Res<DimensionKind> {
    match s.to_ascii_lowercase().as_str() {
        "generic" => Ok(DimensionKind::Generic),
        "time" => Ok(DimensionKind::Time),
        "version" => Ok(DimensionKind::Version),
        "measures" => Ok(DimensionKind::Measures),
        o => Err(format!(
            "unknown kind '{o}' (generic, time, version, measures)"
        )),
    }
}

fn delimiter_of(s: &str) -> Res<u8> {
    match s {
        "\\t" | "tab" => Ok(b'\t'),
        s if s.len() == 1 => Ok(s.as_bytes()[0]),
        s => Err(format!("the delimiter must be one character, not '{s}'")),
    }
}

fn open(path: &Path) -> Res<Database> {
    Database::open(path).map_err(e)
}

fn cube_of<'a>(db: &'a Database, name: &str) -> Res<&'a Cube> {
    db.cube(name).ok_or_else(|| {
        format!(
            "cube '{name}' not found (available: {})",
            db.cubes().map(|c| c.name()).collect::<Vec<_>>().join(", ")
        )
    })
}

/// Runs a command (everything except `serve` and `healthcheck`). The error text is for the user.
pub fn run(cli: Cli, out: &mut Output) -> Res<()> {
    out.json = cli.json;
    let who = user_of(&cli);
    match cli.command {
        Command::Version => {
            let info = neolap_core::build_info();
            out.record(&[
                ("name", V::text(info.name)),
                ("version", V::text(info.version)),
            ]);
        }
        Command::New { db } => {
            Database::create(&db).map_err(e)?;
            out.note(format!("created {}", db.display()));
        }
        Command::Demo { name, db } => {
            if !name.eq_ignore_ascii_case("tiny") {
                return Err(format!("unknown demo '{name}' (available: tiny)"));
            }
            let mut d = Database::create(&db).map_err(e)?;
            build_tiny(&mut d, &who)?;
            out.note(format!(
                "created {} with the demo model 'tiny'",
                db.display()
            ));
        }
        Command::Info { db } => info(&open(&db)?, out)?,
        Command::User(c) => user(c, &who, out)?,
        Command::Dim(DimCommand::Import {
            db,
            dimension,
            file,
            format,
            sync,
            delimiter,
            kind,
        }) => {
            let mut d = open(&db)?;
            let mut ed = match d.dimension(&dimension) {
                Some(x) => x.edit(),
                None => Dimension::new(&dimension, kind_of(&kind)?).edit(),
            };
            let f = std::fs::File::open(&file)
                .map_err(|er| format!("cannot read '{}': {er}", file.display()))?;
            let mode = if sync {
                ImportMode::Sync
            } else {
                ImportMode::AddOnly
            };
            let delim = delimiter_of(&delimiter)?;
            let rep = if format == "levels" {
                neolap_core::dimension::import_levels(&mut ed, f, delim, mode)
            } else {
                neolap_core::dimension::import_parent_child(&mut ed, f, delim, mode)
            }
            .map_err(e)?;
            let dim = ed.commit().map_err(e)?;
            let leaves = dim.n_leaves();
            let total = dim.len();
            d.save_dimension(dim, &who).map_err(e)?;
            out.record(&[
                ("dimension", V::text(&dimension)),
                ("rows", V::Num(rep.rows as f64)),
                ("members_added", V::Num(rep.members_added as f64)),
                ("members_removed", V::Num(rep.removed.len() as f64)),
                ("members", V::Num(total as f64)),
                ("base_members", V::Num(f64::from(leaves))),
            ]);
        }
        Command::Load { db, spec, dry_run } => load(&db, &spec, dry_run, &who, out)?,
        Command::Get {
            db,
            cube,
            members,
            rules,
        } => {
            let d = open(&db)?;
            let c = cube_of(&d, &cube)?;
            let refs: Vec<&str> = members.iter().map(String::as_str).collect();
            let addr = c.address(&refs).map_err(e)?;
            let v = if rules {
                c.cell(&addr, Some(&CubeLookup(&d))).map_err(e)?
            } else {
                c.value(&addr)
                    .map_err(e)?
                    .map_or(CellValue::Empty, CellValue::Num)
            };
            match v {
                CellValue::Num(n) => out.line(if cli.json {
                    serde_json::json!(n).to_string()
                } else {
                    n.to_string()
                }),
                CellValue::Empty => out.line(if cli.json {
                    "null".to_string()
                } else {
                    String::new()
                }),
                CellValue::Text(t) => out.line(if cli.json {
                    serde_json::json!(t).to_string()
                } else {
                    t
                }),
                CellValue::Error(er) => return Err(format!("the cell is {er}")),
            }
        }
        Command::Set {
            db,
            cube,
            mut args,
            mode,
        } => {
            let mut d = open(&db)?;
            let n = cube_of(&d, &cube)?.dimensions().len();
            if args.len() != n + 1 && args.len() != n {
                return Err(format!(
                    "expected {n} members and a value, got {} argument(s)",
                    args.len()
                ));
            }
            let value = if args.len() == n + 1 {
                args.pop().unwrap_or_default()
            } else {
                String::new()
            };
            let text = match (mode.as_deref(), value.trim()) {
                (Some("equal"), v)
                    if !v.is_empty()
                        && !v.starts_with(['#', '@', '+', '-', '='])
                        && !v.ends_with('%') =>
                {
                    format!("#{v}")
                }
                (Some("equal"), v) if v.is_empty() || v.starts_with('#') => v.to_string(),
                (Some("equal"), v) => {
                    return Err(format!("--mode equal needs a plain number, not '{v}'"));
                }
                (_, v) => v.to_string(),
            };
            let input = Input::parse(&text).map_err(e)?;
            let c = d.cube_mut(&cube).ok_or("cube vanished")?;
            let refs: Vec<&str> = args.iter().map(String::as_str).collect();
            let addr = c.address(&refs).map_err(e)?;
            let r = c.write(&WriteContext::new(&who), &addr, input).map_err(e)?;
            out.record(&[
                ("cells_changed", V::Num(r.cells_changed as f64)),
                ("generation", V::Num(r.generation as f64)),
            ]);
        }
        Command::Export {
            db,
            cube,
            area,
            output,
            wide,
            aggregates,
            rules,
            delimiter,
            decimal,
        } => {
            let d = open(&db)?;
            let c = cube_of(&d, &cube)?;
            let spec = ExportSpec {
                area: parse_area(area.as_deref().unwrap_or(""))?,
                layout: wide.map_or(Layout::Long, |dimension| Layout::Wide { dimension }),
                aggregates,
                with_rules: rules,
                suppress_empty: true,
            };
            let table = exchange::export_table(c, &spec, Some(&CubeLookup(&d))).map_err(e)?;
            let dec = match decimal.as_str() {
                "." => '.',
                "," => ',',
                o => {
                    return Err(format!(
                        "the decimal separator must be '.' or ',', not '{o}'"
                    ));
                }
            };
            let is_xlsx = output
                .as_ref()
                .is_some_and(|p| SourceKind::from_path(p) == SourceKind::Xlsx);
            let bytes = if is_xlsx {
                exchange::to_xlsx(&table, c.name())
            } else {
                exchange::to_csv(&table, delimiter_of(&delimiter)?, dec)
            }
            .map_err(e)?;
            match output {
                Some(p) => {
                    std::fs::write(&p, bytes)
                        .map_err(|er| format!("cannot write '{}': {er}", p.display()))?;
                    out.note(format!("{} rows → {}", table.rows.len(), p.display()));
                }
                None => {
                    out.out.write_all(&bytes).map_err(e)?;
                }
            }
        }
        Command::Rules(c) => rules(c, &who, out)?,
        Command::Model(ModelCommand::Export { db, dir }) => {
            neolap_core::modelfiles::export_model(&open(&db)?, &dir).map_err(e)?;
            out.note(format!("model written to {}", dir.display()));
        }
        Command::Model(ModelCommand::Import { db, dir }) => {
            let mut d = open(&db)?;
            let r = neolap_core::modelfiles::import_model(&mut d, &dir, &who).map_err(e)?;
            out.record(&[
                ("dimensions", V::text(r.dimensions.join(", "))),
                ("cubes", V::text(r.cubes.join(", "))),
                ("rules", V::text(r.rules.join(", "))),
                ("import_specs", V::text(r.import_specs.join(", "))),
            ]);
        }
        Command::Backup { db, target } => {
            open(&db)?.backup(&target).map_err(e)?;
            out.note(format!("backup written to {}", target.display()));
        }
        Command::Journal {
            db,
            cube,
            by,
            limit,
        } => {
            let d = open(&db)?;
            let rows = d
                .journal(&JournalQuery {
                    cube,
                    user: by,
                    since: None,
                    limit: Some(limit),
                })
                .map_err(e)?;
            let table: Vec<Vec<V>> = rows
                .into_iter()
                .map(|r| {
                    vec![
                        V::Num(r.id as f64),
                        V::Num(r.ts as f64),
                        V::text(r.user),
                        V::text(r.op),
                        V::text(r.cube),
                        V::Num(r.n_cells as f64),
                        V::text(r.detail),
                    ]
                })
                .collect();
            out.table(
                &["id", "time", "user", "op", "cube", "cells", "detail"],
                &table,
            );
        }
        Command::Data(_) => return Err("this command is handled by the neolap binary".into()),
    }
    Ok(())
}

struct CubeLookup<'a>(&'a Database);

impl neolap_core::CubeProvider for CubeLookup<'_> {
    fn cube(&self, name: &str) -> Option<&Cube> {
        self.0.cube(name)
    }
}

fn parse_area(text: &str) -> Res<Vec<(String, Vec<String>)>> {
    text.split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(|part| {
            let (dim, members) = part.split_once('=').ok_or_else(|| {
                format!("area part '{part}' needs the form dimension=member|member")
            })?;
            Ok((
                dim.trim().to_string(),
                members.split('|').map(|m| m.trim().to_string()).collect(),
            ))
        })
        .collect()
}

fn info(d: &Database, out: &mut Output) -> Res<()> {
    let mut rows: Vec<Vec<V>> = Vec::new();
    for name in d.dimension_names() {
        let dim = d.dimension(name).ok_or("dimension vanished")?;
        rows.push(vec![
            V::text("dimension"),
            V::text(name),
            V::Num(dim.len() as f64),
            V::text(format!("{} base members", dim.n_leaves())),
        ]);
    }
    for c in d.cubes() {
        let dims: Vec<&str> = c.dimensions().iter().map(|x| x.name()).collect();
        let rules = c.rule_status();
        let bad = rules.iter().filter(|r| !r.valid).count();
        rows.push(vec![
            V::text("cube"),
            V::text(c.name()),
            V::Num(c.n_cells() as f64),
            V::text(format!(
                "{}; {} rule(s){}",
                dims.join(" × "),
                rules.len(),
                if bad > 0 {
                    format!(", {bad} invalid")
                } else {
                    String::new()
                }
            )),
        ]);
    }
    for u in d.users().map_err(e)? {
        rows.push(vec![
            V::text("user"),
            V::text(u.name),
            V::Empty,
            V::text(u.role.as_str()),
        ]);
    }
    out.table(&["kind", "name", "count", "detail"], &rows);
    Ok(())
}

fn user(c: UserCommand, who: &str, out: &mut Output) -> Res<()> {
    match c {
        UserCommand::Add { db, name, role } => {
            let mut d = open(&db)?;
            let role = Role::parse(&role).map_err(e)?;
            let pw = password("Password")?;
            d.add_user(&name, &pw, role, who).map_err(e)?;
            out.note(format!("user '{name}' added ({})", role.as_str()));
        }
        UserCommand::List { db } => {
            let rows: Vec<Vec<V>> = open(&db)?
                .users()
                .map_err(e)?
                .into_iter()
                .map(|u| vec![V::text(u.name), V::text(u.role.as_str())])
                .collect();
            out.table(&["user", "role"], &rows);
        }
        UserCommand::Role { db, name, role } => {
            open(&db)?
                .set_role(&name, Role::parse(&role).map_err(e)?, who)
                .map_err(e)?;
            out.note(format!("role of '{name}' set"));
        }
        UserCommand::Password { db, name } => {
            let mut d = open(&db)?;
            let pw = password("New password")?;
            d.set_password(&name, &pw, who).map_err(e)?;
            out.note(format!("password of '{name}' changed; its tokens are void"));
        }
        UserCommand::Remove { db, name } => {
            open(&db)?.remove_user(&name, who).map_err(e)?;
            out.note(format!("user '{name}' removed"));
        }
        UserCommand::Token { db, name, label } => {
            let t = open(&db)?.create_token(&name, &label, who).map_err(e)?;
            // the secret goes to the standard output (for scripts); the warning to the error stream
            out.line(t);
            out.note("This token is shown only once. Store it safely.");
        }
    }
    Ok(())
}

fn load(db: &Path, spec: &str, dry_run: bool, who: &str, out: &mut Output) -> Res<()> {
    let mut d = open(db)?;
    let path = Path::new(spec);
    let (text, base) = if path.is_file() {
        (
            std::fs::read_to_string(path).map_err(e)?,
            path.parent().map(Path::to_path_buf).unwrap_or_default(),
        )
    } else {
        (
            d.import_spec(spec).map_err(|_| {
                format!("'{spec}' is neither a file nor a stored import specification")
            })?,
            std::env::current_dir().map_err(e)?,
        )
    };
    let spec = ImportSpec::from_toml(&text).map_err(e)?;
    let file = spec
        .source
        .file
        .as_deref()
        .ok_or("the specification has no source.file")?;
    let src = base.join(file);
    let data =
        std::fs::read(&src).map_err(|er| format!("cannot read '{}': {er}", src.display()))?;
    let r = d
        .run_import(
            &spec,
            SourceKind::from_path(&src),
            &data,
            &WriteContext::new(who),
            dry_run,
        )
        .map_err(e)?;
    out.record(&[
        ("import", V::text(&r.spec)),
        ("dry_run", V::text(r.dry_run.to_string())),
        ("rows_read", V::Num(r.rows_read as f64)),
        ("cells_created", V::Num(r.cells_created as f64)),
        ("cells_updated", V::Num(r.cells_updated as f64)),
        ("cells_deleted", V::Num(r.cells_deleted as f64)),
        ("cells_unchanged", V::Num(r.cells_unchanged as f64)),
        ("rows_rejected", V::Num(r.rows_rejected as f64)),
        ("members_created", V::Num(r.members_created.len() as f64)),
    ]);
    for rej in &r.rejected {
        out.note(format!("line {}: {}", rej.line, rej.reason));
    }
    if r.rows_rejected as usize > r.rejected.len() {
        out.note(format!(
            "… and {} more rejected rows",
            r.rows_rejected as usize - r.rejected.len()
        ));
    }
    match r.failed {
        Some(why) => Err(why),
        None => Ok(()),
    }
}

fn rules(c: RulesCommand, who: &str, out: &mut Output) -> Res<()> {
    match c {
        RulesCommand::Show { db, cube } => {
            let d = open(&db)?;
            let text = cube_of(&d, &cube)?.rules_text().to_string();
            out.line(text.trim_end());
        }
        RulesCommand::Check { db, cube, file } => {
            let d = open(&db)?;
            let c = cube_of(&d, &cube)?;
            let text = std::fs::read_to_string(&file)
                .map_err(|er| format!("cannot read '{}': {er}", file.display()))?;
            let set = neolap_core::rules::RuleSet::build(&text, c.dimensions()).map_err(e)?;
            report_rules(&set.report(), out)?;
        }
        RulesCommand::Apply { db, cube, file } => {
            let mut d = open(&db)?;
            let text = std::fs::read_to_string(&file)
                .map_err(|er| format!("cannot read '{}': {er}", file.display()))?;
            let report = d.set_rules(&cube, &text, who).map_err(e)?;
            report_rules(&report, out)?;
            out.note(format!("rules of '{cube}' saved"));
        }
    }
    Ok(())
}

/// Prints one line per rule; invalid rules make the command fail (they would be skipped).
fn report_rules(report: &[neolap_core::rules::RuleStatus], out: &mut Output) -> Res<()> {
    let rows: Vec<Vec<V>> = report
        .iter()
        .map(|r| {
            vec![
                V::Num(r.line as f64),
                V::text(if r.valid { "ok" } else { "invalid" }),
                V::text(r.message.clone().unwrap_or_default()),
            ]
        })
        .collect();
    out.table(&["line", "status", "message"], &rows);
    let bad = report.iter().filter(|r| !r.valid).count();
    if bad > 0 {
        Err(format!("{bad} rule(s) invalid"))
    } else {
        Ok(())
    }
}

/// The built-in demo model `tiny`: a small sales cube with a profit measure and a margin rule.
fn build_tiny(db: &mut Database, who: &str) -> Res<()> {
    let mut years = Dimension::new("years", DimensionKind::Time).edit();
    let total = years.add("Year").map_err(e)?;
    for (q, months) in [
        ("Q1", ["Jan", "Feb", "Mar"]),
        ("Q2", ["Apr", "May", "Jun"]),
        ("Q3", ["Jul", "Aug", "Sep"]),
        ("Q4", ["Oct", "Nov", "Dec"]),
    ] {
        let q = years.add(q).map_err(e)?;
        years.add_edge(total, q, 1.0).map_err(e)?;
        years.add_children(q, &months).map_err(e)?;
    }
    let mut regions = Dimension::new("regions", DimensionKind::Generic).edit();
    let all = regions.add("Total").map_err(e)?;
    regions
        .add_children(all, &["North", "South", "East", "West"])
        .map_err(e)?;
    let mut products = Dimension::new("products", DimensionKind::Generic).edit();
    let all = products.add("All").map_err(e)?;
    let cars = products.add("cars").map_err(e)?;
    products.add_edge(all, cars, 1.0).map_err(e)?;
    products.add_children(cars, &["sedan", "suv"]).map_err(e)?;
    let mut m = Dimension::new("measures", DimensionKind::Measures).edit();
    let profit = m.add("Profit").map_err(e)?;
    let (sales, cost) = (m.add("Sales").map_err(e)?, m.add("Cost").map_err(e)?);
    m.add_edge(profit, sales, 1.0).map_err(e)?;
    m.add_edge(profit, cost, -1.0).map_err(e)?;
    m.add("Profit in %").map_err(e)?;
    for ed in [years, regions, products, m] {
        db.save_dimension(ed.commit().map_err(e)?, who).map_err(e)?;
    }
    db.create_cube("sales", &["years", "regions", "products", "measures"], who)
        .map_err(e)?;
    db.set_rules(
        "sales",
        "['Profit in %'] = IF(['Sales'] = 0, NULL, ['Profit'] / ['Sales']);\n",
        who,
    )
    .map_err(e)?;
    let cube = db.cube_mut("sales").ok_or("cube vanished")?;
    let ctx = WriteContext::new(who);
    // deterministic sample values
    let months = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let mut cells = Vec::new();
    for (mi, month) in months.iter().enumerate() {
        for (ri, region) in ["North", "South", "East", "West"].iter().enumerate() {
            for (pi, product) in ["sedan", "suv"].iter().enumerate() {
                let base = 100.0 + (mi * 7 + ri * 13 + pi * 29) as f64;
                let a = |m: &str| cube.address(&[month, region, product, m]).map_err(e);
                cells.push((a("Sales")?, base * 10.0));
                cells.push((a("Cost")?, base * 6.5));
            }
        }
    }
    cube.bulk_write(
        &ctx,
        neolap_core::cube::BulkWrite {
            clear: None,
            cells,
            mode: BulkMode::Overwrite,
            dry_run: false,
        },
    )
    .map_err(e)?;
    Ok(())
}
