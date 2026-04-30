use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::Deserialize;
use zeroize::Zeroizing;

use crate::errors::LensError;
use crate::session::maintenance::AutoPurge;

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Profile {
    pub name: String,
    pub source: SourceSpec,
    #[serde(default)]
    pub policy: Option<PathBuf>,
    #[serde(default)]
    pub schema_allowlist: Option<Vec<String>>,
    /// Snapshot retention TTL in days. `None` (default) = unlimited (D3 default).
    /// When `Some(n)`, snapshots older than `n` days are eligible for sweep
    /// at session start. Sweep is gated on `auto_purge` for destructive vs warn.
    #[serde(default)]
    pub snapshot_retention_days: Option<u32>,
    /// Destructive operational policy. `Off` (default) = no sweep;
    /// `Warn` = read-only scan with per-day-suppressed stderr warning;
    /// `Purge` = silently purge expired snapshots and tombstone manifest rows.
    ///
    /// Merge rule is `min(project, user)` over `Off < Warn < Purge`. The user
    /// can opt to a less destructive mode but cannot escalate above what the
    /// project authorizes. Profiles defined ONLY in the user file are forced
    /// to `Off` regardless — destructive ops require project-level opt-in.
    #[serde(default)]
    pub auto_purge: AutoPurge,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SourceSpec {
    Mysql {
        host: String,
        port: u16,
        database: String,
        username: String,
        #[serde(default)]
        password_env: Option<String>,
        #[serde(default)]
        secret: Option<SecretSpec>,
        #[serde(default)]
        ssh_host: Option<String>,
        #[serde(default)]
        local_port: Option<u16>,
        #[serde(default = "default_readonly_required")]
        readonly_required: bool,
    },
    Postgres {
        host: String,
        port: u16,
        database: String,
        username: String,
        #[serde(default)]
        password_env: Option<String>,
        #[serde(default)]
        secret: Option<SecretSpec>,
        #[serde(default)]
        ssh_host: Option<String>,
        #[serde(default)]
        local_port: Option<u16>,
        #[serde(default = "default_readonly_required")]
        readonly_required: bool,
    },
    Sqlite {
        path: PathBuf,
        #[serde(default = "default_readonly_required")]
        readonly_required: bool,
        #[serde(default)]
        json_text_columns: Vec<String>,
    },
    SshLog {
        host: String,
        path: String,
    },
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum SecretSpec {
    Env { var: String },
    Keyring { service: String, account: String },
}

#[derive(Debug, Clone, Deserialize, Default)]
struct ProfileFile {
    #[serde(default)]
    profiles: Vec<Profile>,
}

#[derive(Debug, Deserialize)]
struct ProfileShape {
    name: Option<String>,
    source: Option<toml::Value>,
}

impl Profile {
    pub fn resolve_password(&self) -> Result<Zeroizing<String>, LensError> {
        let (legacy_env, secret) = match &self.source {
            SourceSpec::Mysql {
                password_env,
                secret,
                ..
            }
            | SourceSpec::Postgres {
                password_env,
                secret,
                ..
            } => (password_env, secret),
            SourceSpec::Sqlite { .. } => {
                return Err(LensError::Profile {
                    detail: "sqlite profiles do not have database passwords".to_string(),
                });
            }
            SourceSpec::SshLog { .. } => {
                return Err(LensError::Profile {
                    detail: "ssh_log profiles do not have database passwords".to_string(),
                });
            }
        };
        match (legacy_env, secret) {
            (Some(_), Some(_)) => Err(LensError::Profile {
                detail: format!(
                    "profile `{}` sets both `password_env` and `secret`; specify exactly one",
                    self.name
                ),
            }),
            (Some(env), None) | (None, Some(SecretSpec::Env { var: env })) => std::env::var(env)
                .map(Zeroizing::new)
                .map_err(|_| LensError::ProfileEnvMissing { env: env.clone() }),
            (None, Some(SecretSpec::Keyring { .. })) => Err(LensError::FeatureDeferred(
                "keyring secret backend resolution deferred until async resolver phase".into(),
            )),
            (None, None) => Err(LensError::Profile {
                detail: format!(
                    "profile `{}` has neither `password_env` nor `secret`; one is required for mysql/postgres profiles",
                    self.name
                ),
            }),
        }
    }
}

pub(crate) fn validate_profile_name(name: &str) -> Result<(), LensError> {
    static PROFILE_NAME: OnceLock<regex::Regex> = OnceLock::new();
    let regex = PROFILE_NAME
        .get_or_init(|| regex::Regex::new(r"^[a-z0-9][a-z0-9_-]{0,63}$").expect("profile regex"));
    if regex.is_match(name) {
        Ok(())
    } else {
        Err(LensError::Profile {
            detail: format!("invalid profile name `{name}`; expected ^[a-z0-9][a-z0-9_-]{{0,63}}$"),
        })
    }
}

/// MS1 (rev 3): in-memory parse of generated profile TOML BEFORE `atomic_write`
/// renames it onto disk. Returns `LensError::Profile` with the same
/// `failed to parse {label} {path} at line N, column M: {err}` format as the
/// on-disk loader (`tests/profile.rs:155-177` bar).
///
/// Used by `src/cli/init/mod.rs::commit_plan` to preserve the no-bad-TOML-on-disk
/// guarantee Codex's r1 insurance directive intended.
pub fn validate_profile_bytes(bytes: &[u8], dest_label: &Path) -> Result<(), LensError> {
    let input = std::str::from_utf8(bytes).map_err(|err| LensError::Profile {
        detail: format!(
            "generated profile bytes for {} are not utf-8: {err}",
            dest_label.display()
        ),
    })?;
    let bytes_inner = input.as_bytes();
    let needle = b"password";
    let mut i = 0usize;
    while i + needle.len() <= bytes_inner.len() {
        if &bytes_inner[i..i + needle.len()] == needle {
            let prev_ok = i == 0
                || !matches!(
                    bytes_inner[i - 1],
                    b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_'
                );
            let after = i + needle.len();
            let is_password_env = bytes_inner.get(after..after + 4) == Some(b"_env");
            if prev_ok && !is_password_env {
                let mut j = after;
                while j < bytes_inner.len() && matches!(bytes_inner[j], b' ' | b'\t') {
                    j += 1;
                }
                if j < bytes_inner.len() && bytes_inner[j] == b'=' {
                    return Err(LensError::Profile {
                        detail: format!(
                            "rendered profile {} contains a literal `password = ...` assignment; use `password_env` or `secret = {{ type = \"keyring\", ... }}` instead",
                            dest_label.display()
                        ),
                    });
                }
            }
        }
        i += 1;
    }
    // Reuses the same internal `ProfileFile` deserializer as `read_profiles_if_exists`
    // so error format matches the existing test bar.
    let _file: ProfileFile = match toml::from_str(input) {
        Ok(f) => f,
        Err(err) => {
            return Err(profile_parse_error(
                "rendered profile",
                dest_label,
                input,
                err,
            ));
        }
    };
    Ok(())
}

pub fn load_profiles(
    project_config: Option<&Path>,
    user_config: Option<&Path>,
) -> Result<Vec<Profile>, LensError> {
    let (profiles, warnings) = load_profiles_with_warnings(project_config, user_config)?;
    for warning in &warnings {
        emit_merge_warning(warning);
    }
    Ok(profiles)
}

/// Like [`load_profiles`], but returns merge-time warnings (e.g. user-only
/// `auto_purge` downgrades) alongside the merged profiles instead of emitting
/// them to stderr. CLI builders use [`load_profiles`] which prints warnings;
/// this variant exists so tests can assert warning content directly without
/// stderr capture.
pub fn load_profiles_with_warnings(
    project_config: Option<&Path>,
    user_config: Option<&Path>,
) -> Result<(Vec<Profile>, Vec<MergeWarning>), LensError> {
    let project_config_is_explicit = project_config.is_some();
    let project_config = project_config
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(".gaze-lens.toml"));
    let user_config = user_config
        .map(PathBuf::from)
        .unwrap_or_else(default_user_config_path);

    let user_profiles = read_profiles_if_exists(&user_config, "user profile config", false)?;
    let project_profiles = read_profiles_if_exists(
        &project_config,
        "project profile config",
        project_config_is_explicit,
    )?;
    let (merged, warnings) = merge_profiles(user_profiles, project_profiles);
    validate_post_merge(&merged)?;
    Ok((merged, warnings))
}

fn validate_post_merge(profiles: &[Profile]) -> Result<(), LensError> {
    for profile in profiles {
        let (password_env, secret) = match &profile.source {
            SourceSpec::Mysql {
                password_env,
                secret,
                ..
            }
            | SourceSpec::Postgres {
                password_env,
                secret,
                ..
            } => (password_env, secret),
            SourceSpec::Sqlite { .. } | SourceSpec::SshLog { .. } => continue,
        };
        match (password_env.is_some(), secret.is_some()) {
            (true, true) => {
                return Err(LensError::Profile {
                    detail: format!(
                        "profile `{}` sets both `password_env` and `secret`; specify exactly one",
                        profile.name
                    ),
                });
            }
            (false, false) => {
                return Err(LensError::Profile {
                    detail: format!(
                        "profile `{}` has neither `password_env` nor `secret`; one is required for mysql/postgres profiles",
                        profile.name
                    ),
                });
            }
            _ => {}
        }
    }
    Ok(())
}

fn emit_merge_warning(warning: &MergeWarning) {
    eprintln!("{}", warning.message());
    match &warning.kind {
        MergeWarningKind::UserOnlyAutoPurgeDowngrade { requested } => {
            tracing::warn!(
                target = "gaze_lens::profile",
                profile = warning.profile,
                requested = requested.as_str(),
                "user-only profile downgraded to auto_purge=off (project opt-in required)"
            );
        }
    }
}

pub fn load_profile(
    name: &str,
    project_config: Option<&Path>,
    user_config: Option<&Path>,
) -> Result<Profile, LensError> {
    load_profiles(project_config, user_config)?
        .into_iter()
        .find(|profile| profile.name == name)
        .ok_or_else(|| LensError::Profile {
            detail: format!("profile `{name}` not found"),
        })
}

fn read_profiles_if_exists(
    path: &Path,
    label: &str,
    required: bool,
) -> Result<Vec<Profile>, LensError> {
    let expanded = expand_path(path)?;
    if !expanded.exists() {
        if required {
            return Err(LensError::ProfileNotFound {
                label: label.to_string(),
                path: expanded,
            });
        }
        return Ok(Vec::new());
    }
    let input = std::fs::read_to_string(&expanded).map_err(|err| LensError::Profile {
        detail: format!("failed to read {label} {}: {err}", expanded.display()),
    })?;
    validate_profile_shape(&input, label, &expanded)?;
    let file: ProfileFile =
        toml::from_str(&input).map_err(|err| profile_parse_error(label, &expanded, &input, err))?;
    Ok(file.profiles)
}

fn validate_profile_shape(input: &str, label: &str, path: &Path) -> Result<(), LensError> {
    let file: ProfileShapeFile =
        toml::from_str(input).map_err(|err| profile_parse_error(label, path, input, err))?;
    for (index, profile) in file.profiles.into_iter().enumerate() {
        let profile_name = profile
            .name
            .unwrap_or_else(|| format!("<profile {}>", index + 1));
        if profile.source.is_none() {
            return Err(LensError::Profile {
                detail: format!(
                    "{label} {} profile `{profile_name}` is missing required field `source`",
                    path.display()
                ),
            });
        }
    }
    Ok(())
}

#[derive(Debug, Deserialize, Default)]
struct ProfileShapeFile {
    #[serde(default)]
    profiles: Vec<ProfileShape>,
}

fn profile_parse_error(label: &str, path: &Path, input: &str, err: toml::de::Error) -> LensError {
    let location = err
        .span()
        .map(|span| line_column(input, span.start))
        .map(|(line, column)| format!(" at line {line}, column {column}"))
        .unwrap_or_default();
    LensError::Profile {
        detail: format!(
            "failed to parse {label} {}{location}: {err}",
            path.display()
        ),
    }
}

fn line_column(input: &str, byte_index: usize) -> (usize, usize) {
    let mut line = 1;
    let mut column = 1;
    for (index, ch) in input.char_indices() {
        if index >= byte_index {
            break;
        }
        if ch == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }
    (line, column)
}

/// Warning emitted during profile merge — surfaced both via stderr (when the
/// public [`load_profiles`] entry-point is used) and as a structured value
/// (via [`load_profiles_with_warnings`]) so tests can assert content without
/// stderr capture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeWarning {
    pub profile: String,
    pub kind: MergeWarningKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeWarningKind {
    /// A profile defined ONLY in the user config requested a destructive
    /// `auto_purge` mode. Destructive ops require project-level opt-in, so
    /// `auto_purge` was forced to `Off`.
    UserOnlyAutoPurgeDowngrade { requested: AutoPurge },
}

impl MergeWarning {
    /// Operator-facing message, identical to the stderr line emitted by
    /// [`load_profiles`].
    pub fn message(&self) -> String {
        match &self.kind {
            MergeWarningKind::UserOnlyAutoPurgeDowngrade { requested } => format!(
                "gaze-lens: warning — profile `{name}` is defined only in the user config and \
                 requested auto_purge = \"{requested}\". Destructive purge requires \
                 project-level opt-in. Forcing auto_purge = \"off\" for this profile.",
                name = self.profile,
                requested = requested.as_str(),
            ),
        }
    }
}

fn merge_profiles(
    user_profiles: Vec<Profile>,
    project_profiles: Vec<Profile>,
) -> (Vec<Profile>, Vec<MergeWarning>) {
    let mut names = BTreeSet::new();
    for profile in user_profiles.iter().chain(project_profiles.iter()) {
        names.insert(profile.name.clone());
    }

    let users: BTreeMap<_, _> = user_profiles
        .into_iter()
        .map(|profile| (profile.name.clone(), profile))
        .collect();
    let projects: BTreeMap<_, _> = project_profiles
        .into_iter()
        .map(|profile| (profile.name.clone(), profile))
        .collect();

    let mut warnings = Vec::new();
    let merged = names
        .into_iter()
        .filter_map(|name| match (users.get(&name), projects.get(&name)) {
            (Some(user), Some(project)) => Some(merge_one(user, project)),
            (Some(user), None) => {
                let (downgraded, warning) = downgrade_user_only_profile(user);
                if let Some(warning) = warning {
                    warnings.push(warning);
                }
                Some(downgraded)
            }
            (None, Some(project)) => Some(project.clone()),
            (None, None) => None,
        })
        .collect();
    (merged, warnings)
}

/// Profiles defined ONLY in the user file cannot enable destructive
/// `auto_purge` — destructive operations require project-level opt-in.
/// Force `Off` and surface a [`MergeWarning`] so the operator notices.
fn downgrade_user_only_profile(user: &Profile) -> (Profile, Option<MergeWarning>) {
    let warning = if user.auto_purge != AutoPurge::Off {
        Some(MergeWarning {
            profile: user.name.clone(),
            kind: MergeWarningKind::UserOnlyAutoPurgeDowngrade {
                requested: user.auto_purge,
            },
        })
    } else {
        None
    };
    let mut downgraded = user.clone();
    downgraded.auto_purge = AutoPurge::Off;
    (downgraded, warning)
}

fn merge_one(user: &Profile, project: &Profile) -> Profile {
    // `snapshot_retention_days`: project file overrides; user fallback.
    let snapshot_retention_days = project
        .snapshot_retention_days
        .or(user.snapshot_retention_days);
    // `auto_purge`: destructive-default cap merge — `min(project, user)` over
    // `Off < Warn < Purge`. User can downgrade; user cannot escalate above
    // what project has authorized.
    let auto_purge = project.auto_purge.cap_with(user.auto_purge);
    Profile {
        name: project.name.clone(),
        source: merge_source(&user.source, &project.source),
        policy: project.policy.clone().or_else(|| user.policy.clone()),
        schema_allowlist: project
            .schema_allowlist
            .clone()
            .or_else(|| user.schema_allowlist.clone()),
        snapshot_retention_days,
        auto_purge,
    }
}

fn merge_source(user: &SourceSpec, project: &SourceSpec) -> SourceSpec {
    match (user, project) {
        (
            SourceSpec::Mysql {
                host: user_host,
                port: user_port,
                ssh_host: user_ssh_host,
                local_port: user_local_port,
                ..
            },
            SourceSpec::Mysql {
                host: project_host,
                port: project_port,
                database,
                username,
                password_env,
                secret,
                ssh_host: project_ssh_host,
                local_port: project_local_port,
                readonly_required,
            },
        ) => SourceSpec::Mysql {
            host: if user_host.is_empty() {
                project_host.clone()
            } else {
                user_host.clone()
            },
            port: if *user_port == 0 {
                *project_port
            } else {
                *user_port
            },
            database: database.clone(),
            username: username.clone(),
            password_env: password_env.clone(),
            secret: secret.clone(),
            ssh_host: user_ssh_host.clone().or_else(|| project_ssh_host.clone()),
            local_port: user_local_port.or(*project_local_port),
            readonly_required: *readonly_required,
        },
        (
            SourceSpec::Postgres {
                host: user_host,
                port: user_port,
                ssh_host: user_ssh_host,
                local_port: user_local_port,
                ..
            },
            SourceSpec::Postgres {
                host: project_host,
                port: project_port,
                database,
                username,
                password_env,
                secret,
                ssh_host: project_ssh_host,
                local_port: project_local_port,
                readonly_required,
            },
        ) => SourceSpec::Postgres {
            host: if user_host.is_empty() {
                project_host.clone()
            } else {
                user_host.clone()
            },
            port: if *user_port == 0 {
                *project_port
            } else {
                *user_port
            },
            database: database.clone(),
            username: username.clone(),
            password_env: password_env.clone(),
            secret: secret.clone(),
            ssh_host: user_ssh_host.clone().or_else(|| project_ssh_host.clone()),
            local_port: user_local_port.or(*project_local_port),
            readonly_required: *readonly_required,
        },
        (
            SourceSpec::Sqlite {
                path: user_path,
                json_text_columns: user_json_text_columns,
                ..
            },
            SourceSpec::Sqlite {
                path: project_path,
                readonly_required,
                json_text_columns: project_json_text_columns,
            },
        ) => SourceSpec::Sqlite {
            path: if user_path.as_os_str().is_empty() {
                project_path.clone()
            } else {
                user_path.clone()
            },
            readonly_required: *readonly_required,
            json_text_columns: if project_json_text_columns.is_empty() {
                user_json_text_columns.clone()
            } else {
                project_json_text_columns.clone()
            },
        },
        (
            SourceSpec::SshLog {
                host: user_host,
                path: user_path,
            },
            SourceSpec::SshLog {
                host: project_host,
                path: project_path,
            },
        ) => SourceSpec::SshLog {
            host: if user_host.is_empty() {
                project_host.clone()
            } else {
                user_host.clone()
            },
            path: if user_path.is_empty() {
                project_path.clone()
            } else {
                user_path.clone()
            },
        },
        (_, project) => project.clone(),
    }
}

fn expand_path(path: &Path) -> Result<PathBuf, LensError> {
    let text = path.to_string_lossy();
    shellexpand::full(&text)
        .map(|expanded| PathBuf::from(expanded.into_owned()))
        .map_err(|err| LensError::Profile {
            detail: format!("failed to expand path {}: {err}", path.display()),
        })
}

fn default_user_config_path() -> PathBuf {
    PathBuf::from("~/.gaze-lens/profiles.toml")
}

fn default_readonly_required() -> bool {
    true
}
