use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::errors::LensError;

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Profile {
    pub name: String,
    pub source: SourceSpec,
    #[serde(default)]
    pub policy: Option<PathBuf>,
    #[serde(default)]
    pub schema_allowlist: Option<Vec<String>>,
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

impl Profile {
    pub fn resolve_password(&self) -> Result<String, LensError> {
        let env = match &self.source {
            SourceSpec::Mysql { password_env, .. } => password_env,
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
    let project_config = project_config
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(".gaze-lens.toml"));
    let user_config = user_config
        .map(PathBuf::from)
        .unwrap_or_else(default_user_config_path);

    let user_profiles = read_profiles_if_exists(&user_config)?;
    let project_profiles = read_profiles_if_exists(&project_config)?;
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

fn read_profiles_if_exists(path: &Path) -> Result<Vec<Profile>, LensError> {
    let expanded = expand_path(path)?;
    if !expanded.exists() {
        return Ok(Vec::new());
    }
    let input = std::fs::read_to_string(&expanded).map_err(|err| LensError::Profile {
        detail: format!("failed to read {}: {err}", expanded.display()),
    })?;
    let file: ProfileFile = toml::from_str(&input).map_err(|err| LensError::Profile {
        detail: format!("failed to parse {}: {err}", expanded.display()),
    })?;
    Ok(file.profiles)
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
            (Some(user), None) => Some(user.clone()),
            (None, Some(project)) => Some(project.clone()),
            (None, None) => None,
        })
        .collect()
}

fn merge_one(user: &Profile, project: &Profile) -> Profile {
    Profile {
        name: project.name.clone(),
        source: merge_source(&user.source, &project.source),
        policy: project.policy.clone().or_else(|| user.policy.clone()),
        schema_allowlist: project
            .schema_allowlist
            .clone()
            .or_else(|| user.schema_allowlist.clone()),
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
