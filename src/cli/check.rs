use std::path::Path;
use std::sync::Arc;

use clap::Args;

use crate::errors::LensError;
use crate::policy::{PolicyFile, build_pipeline};
use crate::profile::{SourceSpec, load_profile};
use crate::source::db::DbSource;
use crate::source::db::mysql::MysqlSource;
use crate::source::db::postgres::PostgresSource;
use crate::source::db::sqlite::SqliteSource;
use crate::source::log::ssh_log::{SshLogCaps, SshLogSource};

use super::serve::runtime_policy;

#[derive(Debug, Args)]
pub struct CheckArgs {
    #[arg(long, default_value = "default")]
    pub profile: String,
}

pub async fn run(
    args: CheckArgs,
    project_config: Option<&Path>,
    user_config: Option<&Path>,
) -> Result<(), LensError> {
    let profile = load_profile(&args.profile, project_config, user_config)?;
    println!("profile: ok ({})", profile.name);

    validate_policy(&profile)?;
    println!("policy: ok");

    validate_source(&profile).await?;
    println!("source: ok");

    let _ = runtime_policy(&profile)?;
    println!("pipeline: ok");
    Ok(())
}

fn validate_policy(profile: &crate::profile::Profile) -> Result<(), LensError> {
    let Some(path) = &profile.policy else {
        let policy =
            PolicyFile::from_toml("[policy.database]\n").map_err(|err| LensError::Profile {
                detail: format!("failed to parse policy: {err}"),
            })?;
        let _ = build_pipeline(&policy).map_err(|err| LensError::Profile {
            detail: format!("failed to build policy pipeline: {err}"),
        })?;
        return Ok(());
    };
    let path = shellexpand::full(&path.to_string_lossy())
        .map(|path| std::path::PathBuf::from(path.into_owned()))
        .map_err(|err| LensError::Profile {
            detail: err.to_string(),
        })?;
    let input = std::fs::read_to_string(&path).map_err(|err| LensError::Profile {
        detail: format!("failed to read policy {}: {err}", path.display()),
    })?;
    let policy = PolicyFile::from_toml(&input).map_err(|err| LensError::Profile {
        detail: format!("failed to parse policy: {err}"),
    })?;
    let _ = build_pipeline(&policy).map_err(|err| LensError::Profile {
        detail: format!("failed to build policy pipeline: {err}"),
    })?;
    Ok(())
}

async fn validate_source(profile: &crate::profile::Profile) -> Result<(), LensError> {
    let limit_cap = crate::session::OutputCaps::default()
        .rows
        .min(u32::MAX as usize) as u32;
    match &profile.source {
        SourceSpec::Mysql { .. } => {
            let source: Arc<dyn DbSource> =
                Arc::new(MysqlSource::connect(profile, limit_cap).await?);
            let _ = source.list_tables().await?;
        }
        SourceSpec::Postgres { .. } => {
            let source: Arc<dyn DbSource> =
                Arc::new(PostgresSource::connect(profile, limit_cap).await?);
            let _ = source.list_tables().await?;
        }
        SourceSpec::Sqlite { .. } => {
            let source: Arc<dyn DbSource> =
                Arc::new(SqliteSource::connect(profile, limit_cap).await?);
            let _ = source.list_tables().await?;
        }
        SourceSpec::SshLog { host, path } => {
            let caps = crate::session::OutputCaps::default();
            let _ = SshLogSource::new(
                profile.name.clone(),
                host.clone(),
                path.clone(),
                SshLogCaps {
                    line_bytes: caps.line_bytes,
                    bytes: caps.bytes,
                    timeout: caps.timeout,
                },
            )?;
        }
    }
    Ok(())
}
