use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;

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
        password_env: String,
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
        password_env: String,
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
    pub fn resolve_password(&self) -> Result<String, LensError> {
        let env = match &self.source {
            SourceSpec::Mysql { password_env, .. } | SourceSpec::Postgres { password_env, .. } => {
                password_env
            }
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
        std::env::var(env).map_err(|_| LensError::ProfileEnvMissing { env: env.clone() })
    }
}

pub fn load_profiles(
    project_config: Option<&Path>,
    user_config: Option<&Path>,
) -> Result<Vec<Profile>, LensError> {
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
    Ok(merge_profiles(user_profiles, project_profiles))
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

fn merge_profiles(user_profiles: Vec<Profile>, project_profiles: Vec<Profile>) -> Vec<Profile> {
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

    names
        .into_iter()
        .filter_map(|name| match (users.get(&name), projects.get(&name)) {
            (Some(user), Some(project)) => Some(merge_one(user, project)),
            (Some(user), None) => Some(downgrade_user_only_profile(user)),
            (None, Some(project)) => Some(project.clone()),
            (None, None) => None,
        })
        .collect()
}

/// Profiles defined ONLY in the user file cannot enable destructive
/// `auto_purge` — destructive operations require project-level opt-in.
/// Force `Off` and emit a stderr warning so the operator notices.
fn downgrade_user_only_profile(user: &Profile) -> Profile {
    if user.auto_purge != AutoPurge::Off {
        eprintln!(
            "gaze-lens: warning — profile `{name}` is defined only in the user config and \
             requested auto_purge = \"{requested}\". Destructive purge requires project-level \
             opt-in. Forcing auto_purge = \"off\" for this profile.",
            name = user.name,
            requested = user.auto_purge.as_str(),
        );
        tracing::warn!(
            target = "gaze_lens::profile",
            profile = user.name,
            requested = user.auto_purge.as_str(),
            "user-only profile downgraded to auto_purge=off (project opt-in required)"
        );
    }
    let mut downgraded = user.clone();
    downgraded.auto_purge = AutoPurge::Off;
    downgraded
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
