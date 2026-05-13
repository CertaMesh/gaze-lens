use std::io::Write;
use std::path::Path;

use clap::Args;
use zeroize::Zeroizing;

use crate::errors::LensError;
use crate::policy::{PolicyFile, build_pipeline};
use crate::profile::{SourceSpec, load_profile};
use crate::source::db::connect_db_source_with_password;
use crate::source::log::ssh_log::{SshLogCaps, SshLogSource};

use super::check_trust::{TrustFormat, build_report, render_text, validate_text_report};

#[derive(Debug, Args)]
pub struct CheckArgs {
    #[arg(long, default_value = "default")]
    pub profile: String,
    /// Emit the trust report: exposed surfaces, redaction posture, and residual risks.
    #[arg(long)]
    pub explain_risk: bool,
    /// Output format for `--explain-risk`. Ignored otherwise.
    #[arg(long, value_enum, default_value_t = TrustFormat::Text, requires = "explain_risk")]
    pub format: TrustFormat,
}

pub async fn run(
    args: CheckArgs,
    project_config: Option<&Path>,
    user_config: Option<&Path>,
) -> Result<(), LensError> {
    let mut stdout = std::io::stdout();
    let mut stderr = std::io::stderr();
    run_with_writer(args, project_config, user_config, &mut stdout, &mut stderr).await
}

async fn run_with_writer(
    args: CheckArgs,
    project_config: Option<&Path>,
    user_config: Option<&Path>,
    out: &mut dyn Write,
    stderr: &mut dyn Write,
) -> Result<(), LensError> {
    let profile = load_profile(&args.profile, project_config, user_config)?;
    let json_mode = args.explain_risk && matches!(args.format, TrustFormat::Json);
    write_status_line(
        json_mode,
        out,
        stderr,
        &format!("profile: ok ({})", profile.name),
    )?;
    if profile
        .schema_allowlist
        .as_ref()
        .is_some_and(|items| !items.is_empty())
        && !profile.schema_tokenize()
    {
        write_status_line(
            json_mode,
            out,
            stderr,
            "warning: schema_allowlist has no presentation-tokenization effect in raw schema mode; set schema_tokenize = true to use it for schema/list_tables presentation, then restart/reload the MCP server",
        )?;
    }

    let validated_policy = validate_policy(&profile)?;
    write_status_line(json_mode, out, stderr, "policy: ok")?;

    if args.explain_risk {
        write_status_line(
            json_mode,
            out,
            stderr,
            "secret: skipped (--explain-risk local-only)",
        )?;
        write_status_line(
            json_mode,
            out,
            stderr,
            "source: skipped (--explain-risk local-only)",
        )?;
        write_status_line(json_mode, out, stderr, "pipeline: ok")?;

        let manifest = default_manifest_path();
        let snapshot_dir = default_snapshot_dir();
        let parsed_policy = validated_policy.parsed.as_ref().map(|parsed| {
            (
                parsed.path.as_path(),
                parsed.raw_bytes.as_slice(),
                &parsed.toml,
            )
        });
        let report = build_report(&profile, &manifest, &snapshot_dir, parsed_policy)?;
        match args.format {
            TrustFormat::Text => {
                validate_text_report(&report)?;
                render_text(&report, out).map_err(write_error)?;
            }
            TrustFormat::Json => {
                serde_json::to_writer_pretty(&mut *out, &report).map_err(|err| {
                    LensError::Internal {
                        detail: format!("serialize trust report: {err}"),
                    }
                })?;
                writeln!(out).map_err(write_error)?;
            }
        }
        return Ok(());
    }

    let validated_secret = match validate_secret_for_check(&profile).await {
        Ok(meta) => {
            let metadata = &meta.metadata;
            writeln!(out, "secret: ok ({metadata})").map_err(write_error)?;
            meta
        }
        Err(err) => {
            render_secret_error(out, &err)?;
            return Err(err);
        }
    };

    if let Err(err) = validate_source(
        &profile,
        validated_secret
            .db_password
            .as_ref()
            .map(|password| password.as_str()),
    )
    .await
    {
        render_source_error(stderr, &profile.name, &err)?;
        return Err(err);
    }
    writeln!(out, "source: ok").map_err(write_error)?;

    let _pipeline = validated_policy.pipeline;
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
    let mut stderr = Vec::new();
    run_with_writer(args, project_config, user_config, out, &mut stderr).await
}

#[doc(hidden)]
pub async fn run_with_writers_for_test(
    args: CheckArgs,
    project_config: Option<&Path>,
    user_config: Option<&Path>,
    out: &mut dyn Write,
    stderr: &mut dyn Write,
) -> Result<(), LensError> {
    run_with_writer(args, project_config, user_config, out, stderr).await
}

fn write_error(err: std::io::Error) -> LensError {
    LensError::Internal {
        detail: format!("failed to write check output: {err}"),
    }
}

fn write_status_line(
    json_mode: bool,
    out: &mut dyn Write,
    stderr: &mut dyn Write,
    line: &str,
) -> Result<(), LensError> {
    if json_mode {
        writeln!(stderr, "{line}")
    } else {
        writeln!(out, "{line}")
    }
    .map_err(write_error)
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

fn render_source_error(
    stderr: &mut dyn Write,
    profile_name: &str,
    err: &LensError,
) -> Result<(), LensError> {
    if matches!(err, LensError::SourceError { .. }) {
        writeln!(
            stderr,
            "source failed while connecting/querying profile `{profile_name}`. If the database host is private, configure source ssh_host/local_port or rerun `gaze-lens init` with tunnel settings."
        )
        .map_err(write_error)?;
    }
    Ok(())
}

struct ValidatedPolicy {
    parsed: Option<ParsedPolicy>,
    pipeline: gaze::Pipeline,
}

struct ParsedPolicy {
    path: std::path::PathBuf,
    raw_bytes: Vec<u8>,
    toml: toml::Value,
}

fn validate_policy(profile: &crate::profile::Profile) -> Result<ValidatedPolicy, LensError> {
    let Some(path) = &profile.policy else {
        let policy =
            PolicyFile::from_toml("[policy.database]\n").map_err(|err| LensError::Profile {
                detail: format!("failed to parse policy: {err}"),
            })?;
        let _ = policy.to_gaze_policy().map_err(|err| LensError::Profile {
            detail: err.to_string(),
        })?;
        let pipeline = build_pipeline(&policy).map_err(|err| LensError::Profile {
            detail: format!("failed to build policy pipeline: {err}"),
        })?;
        return Ok(ValidatedPolicy {
            parsed: None,
            pipeline,
        });
    };
    let path = shellexpand::full(&path.to_string_lossy())
        .map(|path| std::path::PathBuf::from(path.into_owned()))
        .map_err(|err| LensError::Profile {
            detail: err.to_string(),
        })?;
    let raw_bytes = std::fs::read(&path).map_err(|err| LensError::Profile {
        detail: format!("failed to read policy {}: {err}", path.display()),
    })?;
    let input = std::str::from_utf8(&raw_bytes).map_err(|err| LensError::Profile {
        detail: format!("failed to parse policy: {err}"),
    })?;
    let toml: toml::Value = toml::from_str(input).map_err(|err| LensError::Profile {
        detail: format!("failed to parse policy: {err}"),
    })?;
    let policy: PolicyFile = toml.clone().try_into().map_err(|err| LensError::Profile {
        detail: format!("failed to parse policy: {err}"),
    })?;
    let _ = policy.to_gaze_policy().map_err(|err| LensError::Profile {
        detail: err.to_string(),
    })?;
    let pipeline = build_pipeline(&policy).map_err(|err| LensError::Profile {
        detail: format!("failed to build policy pipeline: {err}"),
    })?;
    Ok(ValidatedPolicy {
        parsed: Some(ParsedPolicy {
            path,
            raw_bytes,
            toml,
        }),
        pipeline,
    })
}

fn default_manifest_path() -> std::path::PathBuf {
    std::path::PathBuf::from("~/.gaze-lens/manifest.sqlite")
}

fn default_snapshot_dir() -> std::path::PathBuf {
    std::path::PathBuf::from("~/.gaze-lens/snapshots")
}

async fn validate_source(
    profile: &crate::profile::Profile,
    db_password: Option<&str>,
) -> Result<(), LensError> {
    let limit_cap = crate::session::OutputCaps::default()
        .rows
        .min(u32::MAX as usize) as u32;
    match &profile.source {
        SourceSpec::Mysql { .. } | SourceSpec::Postgres { .. } | SourceSpec::Sqlite { .. } => {
            let source = connect_db_source_with_password(profile, limit_cap, db_password).await?;
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

struct ValidatedSecret {
    metadata: SecretMetadata,
    db_password: Option<Zeroizing<String>>,
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
    validate_secret_for_check(profile)
        .await
        .map(|validated| validated.metadata)
}

async fn validate_secret_for_check(
    profile: &crate::profile::Profile,
) -> Result<ValidatedSecret, LensError> {
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
            let password = profile.resolve_password().await?;
            Ok(ValidatedSecret {
                metadata,
                db_password: Some(password),
            })
        }
        SourceSpec::Sqlite { .. } | SourceSpec::SshLog { .. } => Ok(ValidatedSecret {
            metadata: SecretMetadata {
                backend: "none",
                identity: "not required".to_string(),
            },
            db_password: None,
        }),
    }
}
