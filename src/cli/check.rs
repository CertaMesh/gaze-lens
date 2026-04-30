use std::io::Write;
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
    let mut stdout = std::io::stdout();
    run_with_writer(args, project_config, user_config, &mut stdout).await
}

async fn run_with_writer(
    args: CheckArgs,
    project_config: Option<&Path>,
    user_config: Option<&Path>,
    out: &mut dyn Write,
) -> Result<(), LensError> {
    let profile = load_profile(&args.profile, project_config, user_config)?;
    writeln!(out, "profile: ok ({})", profile.name).map_err(write_error)?;

    validate_policy(&profile)?;
    writeln!(out, "policy: ok").map_err(write_error)?;

    match validate_secret(&profile).await {
        Ok(meta) => {
            writeln!(out, "secret: ok ({meta})").map_err(write_error)?;
        }
        Err(err) => {
            render_secret_error(out, &err)?;
            return Err(err);
        }
    }

    validate_source(&profile).await?;
    writeln!(out, "source: ok").map_err(write_error)?;

    let _ = runtime_policy(&profile)?;
    writeln!(out, "pipeline: ok").map_err(write_error)?;
    Ok(())
}

#[doc(hidden)]
pub async fn run_with_writer_for_test(
    args: CheckArgs,
    project_config: Option<&Path>,
    user_config: Option<&Path>,
    out: &mut dyn Write,
) -> Result<(), LensError> {
    run_with_writer(args, project_config, user_config, out).await
}

fn write_error(err: std::io::Error) -> LensError {
    LensError::Internal {
        detail: format!("failed to write check output: {err}"),
    }
}

fn render_secret_error(out: &mut dyn Write, err: &LensError) -> Result<(), LensError> {
    match err {
        LensError::SecretKeyringMissing { service, account } => writeln!(
            out,
            "secret: NOT FOUND (keyring service={service} account={account}); create the entry via your OS keychain or rerun `gaze-lens init --secret-backend keyring`"
        ),
        LensError::SecretKeyringDenied { service, account } => writeln!(
            out,
            "secret: ACCESS DENIED (keyring service={service} account={account}); unlock the OS keychain or approve access, then retry"
        ),
        LensError::SecretBackendUnavailable { backend, .. } => writeln!(
            out,
            "secret: BACKEND UNAVAILABLE (backend={backend}); on Linux start a DBus session with an unlocked Secret Service agent, or fall back to password_env"
        ),
        _ => writeln!(out, "secret: ERROR"),
    }
    .map_err(write_error)
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

#[doc(hidden)]
#[derive(Debug)]
pub struct SecretMetadata {
    pub backend: &'static str,
    pub identity: String,
}

impl std::fmt::Display for SecretMetadata {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.backend, self.identity)
    }
}

#[doc(hidden)]
pub async fn validate_secret(
    profile: &crate::profile::Profile,
) -> Result<SecretMetadata, LensError> {
    match &profile.source {
        SourceSpec::Mysql {
            password_env,
            secret,
            ..
        }
        | SourceSpec::Postgres {
            password_env,
            secret,
            ..
        } => {
            let metadata = match (password_env, secret) {
                (Some(env), None) => SecretMetadata {
                    backend: "env",
                    identity: format!("var={env}"),
                },
                (None, Some(crate::profile::SecretSpec::Env { var })) => SecretMetadata {
                    backend: "env",
                    identity: format!("var={var}"),
                },
                (None, Some(crate::profile::SecretSpec::Keyring { service, account })) => {
                    SecretMetadata {
                        backend: "keyring",
                        identity: format!("service={service} account={account}"),
                    }
                }
                _ => SecretMetadata {
                    backend: "profile",
                    identity: "invalid".to_string(),
                },
            };
            let _ = profile.resolve_password().await?;
            Ok(metadata)
        }
        SourceSpec::Sqlite { .. } | SourceSpec::SshLog { .. } => Ok(SecretMetadata {
            backend: "none",
            identity: "not required".to_string(),
        }),
    }
}
