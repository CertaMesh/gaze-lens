use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;
use std::{io, io::IsTerminal, io::Write};

use clap::{Args, ValueEnum};

use crate::errors::LensError;
use crate::profile::{SourceSpec, load_profile};
use crate::session::{Session, ToolCall, ToolResult};
use crate::source::db::connect_db_source;
use crate::source::db::query::{CannedQuery, OrderBy, WhereClause, WhereCombinator};
use crate::source::{DbSourceWrapper, SchemaPresentation, Source, ToolArgs};

use super::serve::runtime_policy;

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum QueryFormat {
    /// Indented JSON for human CLI reading.
    PrettyJson,
    /// Compact JSON for scripts and tools.
    Json,
}

#[derive(Debug, Args)]
pub struct QueryArgs {
    #[arg(long, default_value = "default")]
    pub profile: String,
    #[arg(
        long,
        env = "GAZE_LENS_MANIFEST",
        default_value = "~/.gaze-lens/manifest.sqlite"
    )]
    pub manifest: PathBuf,
    #[arg(
        long,
        env = "GAZE_LENS_SNAPSHOT_DIR",
        default_value = "~/.gaze-lens/snapshots"
    )]
    pub snapshot_dir: PathBuf,
    #[arg(long)]
    pub table: String,
    #[arg(long = "column")]
    pub columns: Vec<String>,
    #[arg(long = "where-json")]
    pub where_json: Option<String>,
    #[arg(long, value_enum)]
    pub where_combinator: Option<QueryWhereCombinator>,
    #[arg(long = "order-by-json")]
    pub order_by_json: Option<String>,
    #[arg(long)]
    pub limit: Option<u32>,
    #[arg(long, value_enum, default_value_t = QueryFormat::PrettyJson)]
    pub format: QueryFormat,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum QueryWhereCombinator {
    And,
    Or,
}

impl From<QueryWhereCombinator> for WhereCombinator {
    fn from(value: QueryWhereCombinator) -> Self {
        match value {
            QueryWhereCombinator::And => Self::And,
            QueryWhereCombinator::Or => Self::Or,
        }
    }
}

pub async fn run(
    args: QueryArgs,
    project_config: Option<&Path>,
    user_config: Option<&Path>,
) -> Result<(), LensError> {
    let query = CannedQuery {
        profile: args.profile.clone(),
        table: args.table,
        columns: if args.columns.is_empty() {
            None
        } else {
            Some(args.columns)
        },
        r#where: parse_json_arg::<Vec<WhereClause>>(args.where_json, "where-json")?,
        where_combinator: args.where_combinator.map(Into::into),
        order_by: parse_order_by(args.order_by_json)?,
        limit: args.limit,
    };
    let _status = QueryStatus::start();
    let session = build_db_session(
        &args.profile,
        project_config,
        user_config,
        &args.manifest,
        &args.snapshot_dir,
    )
    .await
    .map_err(|err| annotate_source_error(&args.profile, err))?;
    let result = session
        .dispatch_tool(ToolCall {
            call_id: ulid::Ulid::new().to_string(),
            tool_name: "query".to_string(),
            args: ToolArgs(
                serde_json::to_value(query).map_err(|err| LensError::Internal {
                    detail: err.to_string(),
                })?,
            ),
        })
        .await
        .map_err(|err| annotate_source_error(&args.profile, err))?;
    print_tool_result(&result, args.format)?;
    Ok(())
}

fn annotate_source_error(profile: &str, err: LensError) -> LensError {
    if matches!(err, LensError::SourceError { .. }) {
        eprintln!(
            "source failed while connecting/querying profile `{profile}`. If the database host is private, configure source ssh_host/local_port or rerun `gaze-lens init` with tunnel settings."
        );
    }
    err
}

struct QueryStatus {
    done: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl QueryStatus {
    fn start() -> Self {
        if !io::stderr().is_terminal() {
            let _ = writeln!(io::stderr(), "Running query...");
            return Self {
                done: Arc::new(AtomicBool::new(true)),
                handle: None,
            };
        }

        let done = Arc::new(AtomicBool::new(false));
        let thread_done = Arc::clone(&done);
        let handle = thread::spawn(move || {
            let frames = ["|", "/", "-", "\\"];
            let mut stderr = io::stderr();
            let mut index = 0;
            while !thread_done.load(Ordering::Relaxed) {
                let _ = write!(
                    stderr,
                    "\rRunning query... {}",
                    frames[index % frames.len()]
                );
                let _ = stderr.flush();
                index += 1;
                thread::sleep(Duration::from_millis(120));
            }
        });

        Self {
            done,
            handle: Some(handle),
        }
    }
}

impl Drop for QueryStatus {
    fn drop(&mut self) {
        self.done.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
            let mut stderr = io::stderr();
            let _ = write!(stderr, "\r\x1b[2K");
            let _ = stderr.flush();
        }
    }
}

pub(crate) async fn build_db_session(
    profile_name: &str,
    project_config: Option<&Path>,
    user_config: Option<&Path>,
    manifest: &Path,
    snapshot_dir: &Path,
) -> Result<Arc<Session>, LensError> {
    let profile = load_profile(profile_name, project_config, user_config)?;
    let manifest = expand_path(manifest)?;
    let snapshot_dir = expand_path(snapshot_dir)?;
    super::retention::apply_retention_policy(&profile, &manifest, &snapshot_dir)?;
    let (policy, pipeline, column_actions) = runtime_policy(&profile)?;
    let session = Arc::new(Session::new_with_pipeline_for_profile(
        &policy,
        pipeline,
        profile.name.clone(),
        &manifest,
        &snapshot_dir,
    )?);
    session.register_column_action_policy(profile.name.clone(), column_actions)?;
    let limit_cap = crate::session::OutputCaps::default()
        .rows
        .min(u32::MAX as usize) as u32;
    let db_source = match &profile.source {
        SourceSpec::Mysql { .. } | SourceSpec::Postgres { .. } | SourceSpec::Sqlite { .. } => {
            connect_db_source(&profile, limit_cap).await?
        }
        SourceSpec::SshLog { .. } | SourceSpec::LocalLog { .. } => {
            return Err(LensError::Profile {
                detail: format!("profile `{profile_name}` is not a database source"),
            });
        }
    };
    let schema_presentation = if profile.schema_tokenize() {
        SchemaPresentation::Tokenized {
            allowlist: profile.schema_allowlist,
        }
    } else {
        SchemaPresentation::Raw
    };
    let source: Arc<dyn Source> = Arc::new(DbSourceWrapper::with_schema_presentation(
        db_source,
        schema_presentation,
    ));
    session.register_source_for_profile(
        crate::session::SourceClass::Database,
        &profile.name,
        source.clone(),
    );
    Ok(session)
}

fn parse_order_by(input: Option<String>) -> Result<Option<Vec<OrderBy>>, LensError> {
    parse_json_arg(input, "order-by-json")
}

fn parse_json_arg<T: serde::de::DeserializeOwned>(
    input: Option<String>,
    name: &str,
) -> Result<Option<T>, LensError> {
    input
        .map(|input| {
            serde_json::from_str(&input).map_err(|err| LensError::Profile {
                detail: format!("failed to parse {name}: {err}"),
            })
        })
        .transpose()
}

fn print_tool_result(result: &ToolResult, format: QueryFormat) -> Result<(), LensError> {
    let json = match format {
        QueryFormat::PrettyJson => serde_json::to_string_pretty(result),
        QueryFormat::Json => serde_json::to_string(result),
    }
    .map_err(|err| LensError::Internal {
        detail: err.to_string(),
    })?;
    println!("{json}");
    Ok(())
}

fn expand_path(path: &Path) -> Result<PathBuf, LensError> {
    shellexpand::full(&path.to_string_lossy())
        .map(|path| PathBuf::from(path.into_owned()))
        .map_err(|err| LensError::Profile {
            detail: err.to_string(),
        })
}
