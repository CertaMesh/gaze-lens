use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::Args;

use crate::errors::LensError;
use crate::frontend::mcp::McpFrontend;
use crate::frontend::{Frontend, ShutdownToken};
use crate::policy::{PolicyError, PolicyFile, build_pipeline};
use crate::profile::Profile;
use crate::profile::{SourceSpec, load_profile};
use crate::session::{OutputCaps, Session};
use crate::source::db::DbSource;
use crate::source::db::mysql::MysqlSource;
use crate::source::{DbSourceWrapper, Source};

#[derive(Debug, Args)]
pub struct ServeArgs {
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
}

pub async fn run(
    args: ServeArgs,
    project_config: Option<&Path>,
    user_config: Option<&Path>,
) -> Result<(), LensError> {
    let profile = load_profile(&args.profile, project_config, user_config)?;
    let manifest = expand_path(&args.manifest)?;
    let snapshot_dir = expand_path(&args.snapshot_dir)?;
    let (policy, pipeline) = runtime_policy(&profile)?;
    let session = Arc::new(Session::new_with_pipeline(
        &policy,
        pipeline,
        &manifest,
        &snapshot_dir,
    )?);

    match &profile.source {
        SourceSpec::Mysql { .. } => {
            let limit_cap = OutputCaps::default().rows.min(u32::MAX as usize) as u32;
            let db_source: Arc<dyn DbSource> =
                Arc::new(MysqlSource::connect(&profile, limit_cap).await?);
            let source: Arc<dyn Source> = Arc::new(DbSourceWrapper::with_schema_allowlist(
                db_source,
                profile.schema_allowlist.clone(),
            ));
            for tool_name in ["query", "schema", "list_tables"] {
                session.register_source(tool_name, source.clone());
            }
        }
        SourceSpec::SshLog { .. } => {}
    }

    install_sigterm_exit_handler()?;
    let shutdown = ShutdownToken::new();
    let frontend_shutdown = shutdown.clone();
    let mut frontend =
        tokio::spawn(async move { McpFrontend::new().serve(session, frontend_shutdown).await });
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
        result = wait_for_shutdown_signal() => {
            result?;
            shutdown.cancel();
            frontend.abort();
            let _ = frontend.await;
            Ok(())
        }
    }
}

#[doc(hidden)]
pub fn runtime_policy(profile: &Profile) -> Result<(gaze::Policy, gaze::Pipeline), LensError> {
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
    Ok((policy, pipeline))
}

fn default_policy_file() -> Result<PolicyFile, LensError> {
    PolicyFile::from_toml(
        r#"
        [policy.database]
        "#,
    )
    .map_err(policy_error)
}

async fn wait_for_shutdown_signal() -> Result<(), LensError> {
    let _ = tokio::signal::ctrl_c().await;
    Ok(())
}

fn install_sigterm_exit_handler() -> Result<(), LensError> {
    #[cfg(unix)]
    {
        use std::sync::Arc;
        use std::sync::atomic::AtomicBool;

        signal_hook::flag::register_conditional_shutdown(
            signal_hook::consts::SIGTERM,
            0,
            Arc::new(AtomicBool::new(true)),
        )
        .map_err(|err| LensError::Internal {
            detail: format!("failed to install SIGTERM handler: {err}"),
        })?;
    }

    Ok(())
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
