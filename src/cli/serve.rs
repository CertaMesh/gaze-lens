use std::future::Future;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::{Arg, ArgAction, ArgMatches, Command, Error, FromArgMatches};
use serde::Serialize;

use crate::errors::LensError;
use crate::frontend::mcp::McpFrontend;
use crate::frontend::{Frontend, ShutdownToken};
use crate::policy::{
    ColumnActionPolicy, PolicyError, PolicyFile, build_pipeline, enforce_production_ner,
};
use crate::profile::Profile;
use crate::profile::{SourceSpec, load_profiles, validate_profile_name};
use crate::session::{OutputCaps, Session, SourceClass, schema_hash};
use crate::source::db::TableSchema;
use crate::source::db::connect_db_source;
use crate::source::db::schema::SchemaTokenizer;
use crate::source::log::SshLogSourceWrapper;
use crate::source::log::ssh_log::{SshLogCaps, SshLogSource};
use crate::source::{DbSourceWrapper, SchemaPresentation, Source};

const PRINT_DISCOVERY_SENTINEL: &str = "\0gaze-lens-print-discovery";

#[derive(Debug)]
pub struct ServeArgs {
    pub profile: Vec<String>,
    pub manifest: PathBuf,
    pub snapshot_dir: PathBuf,
}

impl FromArgMatches for ServeArgs {
    fn from_arg_matches(matches: &ArgMatches) -> Result<Self, Error> {
        let mut profile = matches
            .get_many::<String>("profile")
            .map(|values| values.cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        if matches.get_flag("print_discovery") {
            profile.push(PRINT_DISCOVERY_SENTINEL.to_string());
        }
        let manifest = matches
            .get_one::<PathBuf>("manifest")
            .cloned()
            .unwrap_or_else(|| PathBuf::from("~/.gaze-lens/manifest.sqlite"));
        let snapshot_dir = matches
            .get_one::<PathBuf>("snapshot_dir")
            .cloned()
            .unwrap_or_else(|| PathBuf::from("~/.gaze-lens/snapshots"));
        Ok(Self {
            profile,
            manifest,
            snapshot_dir,
        })
    }

    fn update_from_arg_matches(&mut self, matches: &ArgMatches) -> Result<(), Error> {
        *self = Self::from_arg_matches(matches)?;
        Ok(())
    }
}

impl clap::Args for ServeArgs {
    fn augment_args(cmd: Command) -> Command {
        cmd.arg(
            Arg::new("profile")
                .long("profile")
                .value_name("PROFILE")
                .action(ArgAction::Append),
        )
        .arg(
            Arg::new("manifest")
                .long("manifest")
                .env("GAZE_LENS_MANIFEST")
                .default_value("~/.gaze-lens/manifest.sqlite")
                .value_parser(clap::value_parser!(PathBuf)),
        )
        .arg(
            Arg::new("snapshot_dir")
                .long("snapshot-dir")
                .env("GAZE_LENS_SNAPSHOT_DIR")
                .default_value("~/.gaze-lens/snapshots")
                .value_parser(clap::value_parser!(PathBuf)),
        )
        .arg(
            Arg::new("print_discovery")
                .long("print-discovery")
                .help("Print configured profile discovery inventory as JSON and exit without starting MCP")
                .action(ArgAction::SetTrue),
        )
    }

    fn augment_args_for_update(cmd: Command) -> Command {
        Self::augment_args(cmd)
    }
}

#[doc(hidden)]
pub struct PreparedServe {
    pub session: Arc<Session>,
    pub loaded_profiles: Vec<String>,
}

pub async fn run(
    args: ServeArgs,
    project_config: Option<&Path>,
    user_config: Option<&Path>,
) -> Result<(), LensError> {
    if print_discovery_requested(&args) {
        return print_discovery_inventory(&args, project_config, user_config).await;
    }
    let prepared = prepare_session(args, project_config, user_config)?;
    eprintln!("{}", loaded_profiles_banner(&prepared.loaded_profiles));
    run_frontend_until_shutdown(
        McpFrontend::new(),
        prepared.session,
        wait_for_shutdown_signal(),
    )
    .await
}

fn prepare_session(
    args: ServeArgs,
    project_config: Option<&Path>,
    user_config: Option<&Path>,
) -> Result<PreparedServe, LensError> {
    let profiles = select_profiles(load_profiles(project_config, user_config)?, &args.profile)?;
    let manifest = expand_path(&args.manifest)?;
    let snapshot_dir = expand_path(&args.snapshot_dir)?;
    apply_multi_profile_retention(&profiles, &manifest, &snapshot_dir)?;

    let mut runtime = Vec::with_capacity(profiles.len());
    for profile in &profiles {
        runtime.push((profile.clone(), runtime_policy(profile)?));
    }
    let first_policy = if let Some((_, (policy, _, _))) = runtime.first() {
        policy.clone()
    } else {
        default_policy_file()?
            .to_gaze_policy()
            .map_err(policy_error)?
    };
    let session = Arc::new(Session::new_for_multi_profile(
        &first_policy,
        &manifest,
        &snapshot_dir,
    )?);
    for (profile, (_policy, pipeline, column_actions)) in runtime {
        session.register_pipeline(profile.name.clone(), Arc::new(pipeline))?;
        session.register_column_action_policy(profile.name.clone(), column_actions)?;
        register_lazy_source(&session, profile);
    }

    let loaded_profiles = profiles
        .iter()
        .map(|profile| profile.name.clone())
        .collect::<Vec<_>>();

    Ok(PreparedServe {
        session,
        loaded_profiles,
    })
}

#[doc(hidden)]
pub fn prepare_session_for_test(
    args: ServeArgs,
    project_config: Option<&Path>,
    user_config: Option<&Path>,
) -> Result<PreparedServe, LensError> {
    prepare_session(args, project_config, user_config)
}

#[doc(hidden)]
pub fn loaded_profiles_banner(profile_names: &[String]) -> String {
    format!(
        "gaze-lens serve: loaded profiles: [{}]",
        profile_names.join(", ")
    )
}

fn select_profiles(
    profiles: Vec<Profile>,
    restrict_list: &[String],
) -> Result<Vec<Profile>, LensError> {
    let mut valid = Vec::new();
    for profile in profiles {
        validate_profile_name(&profile.name)?;
        valid.push(profile);
    }
    if restrict_list.is_empty() {
        return Ok(valid);
    }
    for name in restrict_list {
        validate_profile_name(name)?;
    }
    let selected = restrict_list
        .iter()
        .map(|name| {
            valid
                .iter()
                .find(|profile| &profile.name == name)
                .cloned()
                .ok_or_else(|| LensError::Profile {
                    detail: format!("profile `{name}` not found"),
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(selected)
}

fn register_lazy_source(session: &Arc<Session>, profile: Profile) {
    match &profile.source {
        SourceSpec::Mysql { .. } | SourceSpec::Postgres { .. } | SourceSpec::Sqlite { .. } => {
            session.register_source_lazy(
                SourceClass::Database,
                profile.name.clone(),
                Arc::new(move || {
                    let profile = profile.clone();
                    Box::pin(async move { build_db_source(profile).await })
                }),
            );
        }
        SourceSpec::SshLog { .. } => {
            session.register_source_lazy(
                SourceClass::Log,
                profile.name.clone(),
                Arc::new(move || {
                    let profile = profile.clone();
                    Box::pin(async move { build_log_source(profile) })
                }),
            );
        }
    }
}

#[derive(Serialize)]
struct DiscoveryInventory {
    profiles: Vec<ProfileDiscovery>,
}

#[derive(Serialize)]
struct ProfileDiscovery {
    name: String,
    source_class: &'static str,
    supported_tools: Vec<&'static str>,
    scope: DiscoveryScope,
    schema_hash: String,
}

#[derive(Serialize)]
#[serde(untagged)]
enum DiscoveryScope {
    Database { tables: Vec<TableDiscovery> },
    Log { host: String, path: String },
}

#[derive(Serialize)]
struct TableDiscovery {
    name: String,
    allowed_columns: Vec<String>,
}

#[derive(Serialize)]
struct DatabaseInventoryDescriptor<'a> {
    profile: &'a str,
    source_class: &'static str,
    supported_tools: Vec<&'static str>,
    tables: Vec<TableHashDescriptor>,
}

#[derive(Serialize)]
struct TableHashDescriptor {
    name: String,
    columns: Vec<ColumnHashDescriptor>,
}

#[derive(Serialize)]
struct ColumnHashDescriptor {
    column: String,
    allowed: bool,
}

#[derive(Serialize)]
struct LogInventoryDescriptor<'a> {
    profile: &'a str,
    source_class: &'static str,
    supported_tools: Vec<&'static str>,
    host: &'a str,
    path: &'a str,
}

async fn print_discovery_inventory(
    args: &ServeArgs,
    project_config: Option<&Path>,
    user_config: Option<&Path>,
) -> Result<(), LensError> {
    let inventory = build_discovery_inventory(args, project_config, user_config).await?;
    let stdout = std::io::stdout();
    let mut stdout = stdout.lock();
    serde_json::to_writer_pretty(&mut stdout, &inventory).map_err(|err| LensError::Internal {
        detail: format!("failed to serialize discovery inventory: {err}"),
    })?;
    writeln!(stdout).map_err(|err| LensError::Internal {
        detail: format!("failed to write discovery inventory: {err}"),
    })?;
    Ok(())
}

async fn build_discovery_inventory(
    args: &ServeArgs,
    project_config: Option<&Path>,
    user_config: Option<&Path>,
) -> Result<DiscoveryInventory, LensError> {
    let profile_filter = selected_profile_names(args);
    let mut profiles =
        select_profiles(load_profiles(project_config, user_config)?, &profile_filter)?;
    profiles.sort_by(|left, right| left.name.cmp(&right.name));

    let mut discovered = Vec::with_capacity(profiles.len());
    for profile in profiles {
        let _ = runtime_policy(&profile)?;
        discovered.push(discover_profile(&profile).await?);
    }
    Ok(DiscoveryInventory {
        profiles: discovered,
    })
}

async fn discover_profile(profile: &Profile) -> Result<ProfileDiscovery, LensError> {
    match &profile.source {
        SourceSpec::Mysql { .. } | SourceSpec::Postgres { .. } | SourceSpec::Sqlite { .. } => {
            discover_database_profile(profile).await
        }
        SourceSpec::SshLog { host, path } => discover_log_profile(profile, host, path),
    }
}

async fn discover_database_profile(profile: &Profile) -> Result<ProfileDiscovery, LensError> {
    let source_class = SourceClass::Database;
    let supported_tools = supported_tools(source_class);
    let limit_cap = OutputCaps::default().rows.min(u32::MAX as usize) as u32;
    let source = connect_db_source(profile, limit_cap).await?;
    let mut raw_tables = source.list_tables().await?;
    raw_tables.sort();

    let tokenizer = SchemaTokenizer::default();
    let mut tables = Vec::with_capacity(raw_tables.len());
    let mut hash_tables = Vec::with_capacity(raw_tables.len());
    for raw_table in raw_tables {
        let schema = source.schema(&raw_table).await?;
        let presented = present_schema_for_discovery(&tokenizer, schema, profile);
        let table_name = presented_table_name(&presented);
        let mut columns = presented
            .columns
            .into_iter()
            .map(|column| ColumnHashDescriptor {
                column: column.name_token,
                allowed: column.allowed,
            })
            .collect::<Vec<_>>();
        columns.sort_by(|left, right| {
            left.column
                .cmp(&right.column)
                .then_with(|| left.allowed.cmp(&right.allowed))
        });
        let allowed_columns = columns
            .iter()
            .filter(|column| column.allowed)
            .map(|column| column.column.clone())
            .collect::<Vec<_>>();
        tables.push(TableDiscovery {
            name: table_name.clone(),
            allowed_columns,
        });
        hash_tables.push(TableHashDescriptor {
            name: table_name,
            columns,
        });
    }

    tables.sort_by(|left, right| left.name.cmp(&right.name));
    hash_tables.sort_by(|left, right| left.name.cmp(&right.name));
    let descriptor = DatabaseInventoryDescriptor {
        profile: &profile.name,
        source_class: source_class.as_str(),
        supported_tools: supported_tools.clone(),
        tables: hash_tables,
    };
    Ok(ProfileDiscovery {
        name: profile.name.clone(),
        source_class: source_class.as_str(),
        supported_tools,
        scope: DiscoveryScope::Database { tables },
        schema_hash: schema_hash(&descriptor)?,
    })
}

fn discover_log_profile(
    profile: &Profile,
    host: &str,
    path: &str,
) -> Result<ProfileDiscovery, LensError> {
    let source_class = SourceClass::Log;
    let supported_tools = supported_tools(source_class);
    let descriptor = LogInventoryDescriptor {
        profile: &profile.name,
        source_class: source_class.as_str(),
        supported_tools: supported_tools.clone(),
        host,
        path,
    };
    Ok(ProfileDiscovery {
        name: profile.name.clone(),
        source_class: source_class.as_str(),
        supported_tools,
        scope: DiscoveryScope::Log {
            host: host.to_string(),
            path: path.to_string(),
        },
        schema_hash: schema_hash(&descriptor)?,
    })
}

fn supported_tools(source_class: SourceClass) -> Vec<&'static str> {
    match source_class {
        SourceClass::Database => vec!["query", "schema", "list_tables"],
        SourceClass::Log => vec!["log_tail", "log_grep"],
    }
}

fn present_schema_for_discovery(
    tokenizer: &SchemaTokenizer,
    schema: TableSchema,
    profile: &Profile,
) -> TableSchema {
    if profile.schema_tokenize() {
        tokenizer.tokenize_table_schema(&schema, profile.schema_allowlist.as_deref())
    } else {
        schema
    }
}

fn presented_table_name(schema: &TableSchema) -> String {
    if schema.table_token.is_empty() {
        schema.table.clone()
    } else {
        schema.table_token.clone()
    }
}

fn print_discovery_requested(args: &ServeArgs) -> bool {
    args.profile
        .iter()
        .any(|profile| profile == PRINT_DISCOVERY_SENTINEL)
}

fn selected_profile_names(args: &ServeArgs) -> Vec<String> {
    args.profile
        .iter()
        .filter(|profile| profile.as_str() != PRINT_DISCOVERY_SENTINEL)
        .cloned()
        .collect()
}

async fn build_db_source(profile: Profile) -> Result<Arc<dyn Source>, LensError> {
    let limit_cap = OutputCaps::default().rows.min(u32::MAX as usize) as u32;
    let db_source = match &profile.source {
        SourceSpec::Mysql { .. } | SourceSpec::Postgres { .. } | SourceSpec::Sqlite { .. } => {
            connect_db_source(&profile, limit_cap).await?
        }
        SourceSpec::SshLog { .. } => {
            return Err(LensError::Profile {
                detail: format!("profile `{}` is not a database source", profile.name),
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
    Ok(Arc::new(DbSourceWrapper::with_schema_presentation(
        db_source,
        schema_presentation,
    )))
}

fn build_log_source(profile: Profile) -> Result<Arc<dyn Source>, LensError> {
    let SourceSpec::SshLog { host, path } = &profile.source else {
        return Err(LensError::Profile {
            detail: format!("profile `{}` is not a log source", profile.name),
        });
    };
    let caps = OutputCaps::default();
    let log_source = Arc::new(SshLogSource::new(
        profile.name.clone(),
        host.clone(),
        path.clone(),
        SshLogCaps {
            line_bytes: caps.line_bytes,
            bytes: caps.bytes,
            timeout: caps.timeout,
        },
    )?);
    Ok(Arc::new(SshLogSourceWrapper::new(log_source)))
}

#[doc(hidden)]
pub fn apply_multi_profile_retention(
    profiles: &[Profile],
    manifest: &Path,
    snapshot_dir: &Path,
) -> Result<(), LensError> {
    let retention_days = profiles
        .iter()
        .filter_map(|profile| profile.snapshot_retention_days)
        .min();
    let Some(retention_days) = retention_days else {
        return Ok(());
    };
    let mut merged = profiles[0].clone();
    merged.snapshot_retention_days = Some(retention_days);
    merged.auto_purge = profiles
        .iter()
        .map(|profile| profile.auto_purge)
        .reduce(|merged, next| merged.cap_with(next))
        .unwrap_or(merged.auto_purge);
    super::retention::apply_retention_policy(&merged, manifest, snapshot_dir)
}

#[doc(hidden)]
pub async fn run_frontend_until_shutdown<F, S>(
    frontend: F,
    session: Arc<Session>,
    shutdown_signal: S,
) -> Result<(), LensError>
where
    F: Frontend + 'static,
    S: Future<Output = ()>,
{
    let shutdown = ShutdownToken::new();
    let frontend_shutdown = shutdown.clone();
    let mut frontend =
        tokio::spawn(async move { frontend.serve(session, frontend_shutdown).await });
    tokio::select! {
        result = &mut frontend => {
            let result = result.map_err(|err| LensError::FrontendError {
                frontend: "mcp".to_string(),
                detail: err.to_string(),
            })?;
            result.map_err(|err| LensError::FrontendError {
                frontend: "mcp".to_string(),
                detail: err.to_string(),
            })
        }
        _ = shutdown_signal => {
            shutdown.cancel();
            let result = frontend.await.map_err(|err| LensError::FrontendError {
                frontend: "mcp".to_string(),
                detail: err.to_string(),
            })?;
            result.map_err(|err| LensError::FrontendError {
                frontend: "mcp".to_string(),
                detail: err.to_string(),
            })
        }
    }
}

#[doc(hidden)]
pub fn runtime_policy(
    profile: &Profile,
) -> Result<(gaze::Policy, gaze::Pipeline, ColumnActionPolicy), LensError> {
    let policy_file = match &profile.policy {
        Some(path) => {
            let path = expand_path(path)?;
            let input = std::fs::read_to_string(&path).map_err(|err| LensError::Profile {
                detail: format!("failed to read policy {}: {err}", path.display()),
            })?;
            PolicyFile::from_toml(&input).map_err(policy_error)?
        }
        None => default_policy_file()?,
    };
    // #988: a production profile must configure an NER model. Enforced before
    // the pipeline is built so a misconfigured prod profile fails closed at
    // session build (serve/query) rather than leaking names at retrieval time.
    enforce_production_ner(&profile.name, profile.production, &policy_file)
        .map_err(policy_error)?;
    let policy = policy_file.to_gaze_policy().map_err(policy_error)?;
    let pipeline = build_pipeline(&policy_file).map_err(policy_error)?;
    let column_actions =
        ColumnActionPolicy::from_policy_file(&policy_file).map_err(policy_error)?;
    Ok((policy, pipeline, column_actions))
}

fn default_policy_file() -> Result<PolicyFile, LensError> {
    PolicyFile::from_toml(
        r#"
        [policy.database]
        "#,
    )
    .map_err(policy_error)
}

async fn wait_for_shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        let mut sigterm = match signal(SignalKind::terminate()) {
            Ok(signal) => signal,
            Err(err) => {
                tracing::error!("failed to install SIGTERM handler: {err}");
                let _ = tokio::signal::ctrl_c().await;
                return;
            }
        };
        tokio::select! {
            result = tokio::signal::ctrl_c() => {
                if let Err(err) = result {
                    tracing::error!("ctrl_c signal handler failed: {err}");
                }
            }
            _ = sigterm.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

fn policy_error(err: PolicyError) -> LensError {
    LensError::Profile {
        detail: err.to_string(),
    }
}

fn expand_path(path: &Path) -> Result<PathBuf, LensError> {
    shellexpand::full(&path.to_string_lossy())
        .map(|path| PathBuf::from(path.into_owned()))
        .map_err(|err| LensError::Profile {
            detail: err.to_string(),
        })
}
