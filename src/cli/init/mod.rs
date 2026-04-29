use std::io::Write;
use std::path::{Path, PathBuf};

use clap::Args;

use crate::errors::LensError;

pub mod prompter;

const PROJECT_PROFILE: &str = r#"# Project-owned schema policy and logical source shape.
[[profiles]]
name = "prod"
schema_allowlist = ["id", "created_at", "updated_at"]

[profiles.source]
kind = "mysql"
host = "prod-db.internal"
port = 3306
database = "app"
username = "gaze_ro"
password_env = "GAZE_LENS_DB_PASSWORD"
readonly_required = true
"#;

const USER_PROFILE: &str = r#"# User-owned transport overrides for the same profile.
[[profiles]]
name = "prod"

[profiles.source]
kind = "mysql"
host = "127.0.0.1"
port = 13306
database = "app"
username = "gaze_ro"
password_env = "GAZE_LENS_DB_PASSWORD"
ssh_host = "deploy@prod.example.com"
local_port = 13306
readonly_required = true
"#;

#[derive(Debug, Args)]
pub struct InitArgs {
    #[arg(long, conflicts_with = "print_only")]
    pub write_all: bool,
    #[arg(long, conflicts_with = "write_all")]
    pub print_only: bool,
}

pub fn run(
    args: InitArgs,
    project_config: Option<&Path>,
    user_config: Option<&Path>,
) -> Result<(), LensError> {
    let project_path = expand_path(project_config.unwrap_or_else(|| Path::new(".gaze-lens.toml")))?;
    let user_path =
        expand_path(user_config.unwrap_or_else(|| Path::new("~/.gaze-lens/profiles.toml")))?;
    let files = [
        ("project profile", project_path, PROJECT_PROFILE),
        ("user profile", user_path, USER_PROFILE),
    ];

    for (label, path, contents) in &files {
        println!("--- {label}: {} ---\n{contents}", path.display());
    }

    if args.print_only {
        return Ok(());
    }

    let confirmed = args.write_all || prompt_confirm()?;
    if !confirmed {
        println!("No files written.");
        return Ok(());
    }

    for (_, path, _) in &files {
        if path.exists() {
            return Err(LensError::Profile {
                detail: format!("{} already exists; refusing to overwrite", path.display()),
            });
        }
    }
    for (_, path, contents) in &files {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|err| LensError::Profile {
                detail: format!("failed to create {}: {err}", parent.display()),
            })?;
        }
        std::fs::write(path, contents).map_err(|err| LensError::Profile {
            detail: format!("failed to write {}: {err}", path.display()),
        })?;
        println!("wrote {}", path.display());
    }
    Ok(())
}

fn prompt_confirm() -> Result<bool, LensError> {
    print!("Write these files? [y/N] ");
    std::io::stdout()
        .flush()
        .map_err(|err| LensError::Profile {
            detail: err.to_string(),
        })?;
    let mut input = String::new();
    std::io::stdin()
        .read_line(&mut input)
        .map_err(|err| LensError::Profile {
            detail: err.to_string(),
        })?;
    Ok(matches!(input.trim(), "y" | "Y" | "yes" | "YES" | "Yes"))
}

fn expand_path(path: &Path) -> Result<PathBuf, LensError> {
    shellexpand::full(&path.to_string_lossy())
        .map(|path| PathBuf::from(path.into_owned()))
        .map_err(|err| LensError::Profile {
            detail: err.to_string(),
        })
}
