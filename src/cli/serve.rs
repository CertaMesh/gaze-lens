use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::Args;

use crate::errors::LensError;
use crate::frontend::mcp::McpFrontend;
use crate::frontend::{Frontend, ShutdownToken};
use crate::policy::{ColumnActionPolicy, PolicyError, PolicyFile, build_pipeline};
use crate::profile::Profile;
use crate::profile::{SourceSpec, load_profiles, validate_profile_name};
use crate::session::{OutputCaps, Session, SourceClass};
use crate::source::db::DbSource;
use crate::source::db::mysql::MysqlSource;
use crate::source::db::postgres::PostgresSource;
use crate::source::db::sqlite::SqliteSource;
use crate::source::log::SshLogSourceWrapper;
use crate::source::log::ssh_log::{SshLogCaps, SshLogSource};
use crate::source::{DbSourceWrapper, SchemaPresentation, Source};

#[derive(Debug, Args)]
pub struct ServeArgs {
    #[arg(long)]
    pub profile: Vec<String>,
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
    let first_policy = &runtime
        .first()
        .ok_or_else(|| LensError::Profile {
            detail: "no profiles configured".to_string(),
        })?
        .1
        .0;
    let session = Arc::new(Session::new_for_multi_profile(
        first_policy,
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
        if valid.is_empty() {
            return Err(LensError::Profile {
                detail: "no profiles configured".to_string(),
            });
        }
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

async fn build_db_source(profile: Profile) -> Result<Arc<dyn Source>, LensError> {
    let limit_cap = OutputCaps::default().rows.min(u32::MAX as usize) as u32;
    let db_source: Arc<dyn DbSource> = match &profile.source {
        SourceSpec::Mysql { .. } => Arc::new(MysqlSource::connect(&profile, limit_cap).await?),
        SourceSpec::Postgres { .. } => {
            Arc::new(PostgresSource::connect(&profile, limit_cap).await?)
        }
        SourceSpec::Sqlite { .. } => Arc::new(SqliteSource::connect(&profile, limit_cap).await?),
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
