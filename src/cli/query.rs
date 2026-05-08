use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::{Args, ValueEnum};

use crate::errors::LensError;
use crate::profile::{SourceSpec, load_profile};
use crate::session::{Session, ToolCall, ToolResult};
use crate::source::db::DbSource;
use crate::source::db::mysql::MysqlSource;
use crate::source::db::postgres::PostgresSource;
use crate::source::db::query::{CannedQuery, OrderBy, WhereClause, WhereCombinator};
use crate::source::db::sqlite::SqliteSource;
use crate::source::{DbSourceWrapper, SchemaPresentation, Source, ToolArgs};

use super::serve::runtime_policy;

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
    let session = build_db_session(
        &args.profile,
        project_config,
        user_config,
        &args.manifest,
        &args.snapshot_dir,
    )
    .await?;
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
        .await?;
    print_tool_result(&result)?;
    Ok(())
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
    let db_source: Arc<dyn DbSource> = match &profile.source {
        SourceSpec::Mysql { .. } => Arc::new(MysqlSource::connect(&profile, limit_cap).await?),
        SourceSpec::Postgres { .. } => {
            Arc::new(PostgresSource::connect(&profile, limit_cap).await?)
        }
        SourceSpec::Sqlite { .. } => Arc::new(SqliteSource::connect(&profile, limit_cap).await?),
        SourceSpec::SshLog { .. } => {
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

fn print_tool_result(result: &ToolResult) -> Result<(), LensError> {
    let json = serde_json::to_string(result).map_err(|err| LensError::Internal {
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
