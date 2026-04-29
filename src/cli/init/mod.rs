use std::io::Write;
use std::path::{Path, PathBuf};

use clap::{ArgAction, Args, ValueEnum};

use crate::errors::LensError;

pub mod atomic;
pub mod batch;
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

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum SourceKind {
    Mysql,
    Postgres,
    Sqlite,
    #[value(name = "ssh-log")]
    SshLog,
}

/// Where to write the profile section.
///
/// `project` → `<cwd>/.gaze-lens.toml` (auto_purge omitted = `off`).
/// `user`    → `~/.gaze-lens/profiles.toml` (auto_purge cannot be set).
/// `project-auto-purge` → `<cwd>/.gaze-lens.toml` with `auto_purge = "purge"`.
///
/// CB1: clap rejects `--scope user --auto-purge` because there is no
/// `--auto-purge` flag — destructive consent rides on the scope value itself.
#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum InitScope {
    Project,
    User,
    #[value(name = "project-auto-purge")]
    ProjectAutoPurge,
}

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq, Hash)]
pub enum McpClient {
    Codex,
    #[value(name = "claude-code")]
    ClaudeCode,
    Cursor,
}

#[derive(Debug, Args)]
pub struct InitArgs {
    /// Profile name. Required when `--non-interactive`.
    #[arg(long)]
    pub profile: Option<String>,

    /// Source kind. Required when `--non-interactive`.
    #[arg(long, value_enum)]
    pub source_kind: Option<SourceKind>,

    /// Where to write the profile (project / user / project-auto-purge).
    #[arg(long, value_enum)]
    pub scope: Option<InitScope>,

    /// DB / SSH host (required for ssh-log).
    #[arg(long)]
    pub source_host: Option<String>,
    /// DB port.
    #[arg(long)]
    pub source_port: Option<u16>,
    /// DB database name.
    #[arg(long)]
    pub source_database: Option<String>,
    /// DB username.
    #[arg(long)]
    pub source_username: Option<String>,
    /// Env var name holding the DB password.
    #[arg(long)]
    pub source_password_env: Option<String>,
    /// SSH tunnel jump host (mysql / postgres only).
    #[arg(long)]
    pub source_ssh_host: Option<String>,
    /// SSH tunnel local port (mysql / postgres only).
    #[arg(long)]
    pub source_local_port: Option<u16>,
    /// Path for sqlite or ssh-log.
    #[arg(long)]
    pub source_path: Option<PathBuf>,
    /// SQLite TEXT-as-JSON column allowlist (comma-separated).
    #[arg(long, value_delimiter = ',')]
    pub source_json_text_columns: Vec<String>,

    /// MCP clients to configure. Repeatable. Empty = none.
    #[arg(long = "client", value_enum, action = ArgAction::Append)]
    pub clients: Vec<McpClient>,
    /// Skip writing any MCP client config.
    #[arg(long, conflicts_with = "clients")]
    pub no_mcp_config: bool,
    /// Skip patching `AGENTS.md`.
    #[arg(long)]
    pub no_agents_md: bool,
    /// Also patch `CLAUDE.md` if it exists in cwd.
    #[arg(long)]
    pub also_claude_md: bool,
    /// Allow overwriting an existing profile / MCP entry of the same name.
    #[arg(long)]
    pub allow_overwrite: bool,
    /// Run without prompts. Missing required input → exit 1.
    #[arg(long)]
    pub non_interactive: bool,
    /// Render preview only. Performs no writes. Exits 0.
    #[arg(long, conflicts_with = "write_all")]
    pub print_only: bool,
    /// Skip per-step confirms but still validate + write.
    #[arg(long, conflicts_with = "print_only")]
    pub write_all: bool,
    /// Run an in-process `check` after the batch write. Opt-in only (directive 17).
    #[arg(long)]
    pub smoke_check: bool,
}

impl InitArgs {
    /// Runtime validation called from `run` before any prompter is built.
    /// CB1 (`--scope user --auto-purge` rejection) lives in clap. This catches
    /// non-interactive missing inputs and the CB-r2-3 ssh-log host invariant.
    pub fn validate(&self) -> Result<(), LensError> {
        if self.non_interactive {
            if self.profile.is_none() {
                return Err(LensError::Profile {
                    detail: "--non-interactive requires --profile <name>".into(),
                });
            }
            if self.source_kind.is_none() {
                return Err(LensError::Profile {
                    detail:
                        "--non-interactive requires --source-kind <mysql|postgres|sqlite|ssh-log>"
                            .into(),
                });
            }
            // CB-r2-3: ssh-log host renders to TOML field `host` per
            // src/profile.rs:70-73. The validator-gated host is `--source-host`,
            // NOT `--source-ssh-host` (which is the db-tunnel jump host).
            if matches!(self.source_kind, Some(SourceKind::SshLog)) {
                if self.source_host.is_none() {
                    return Err(LensError::Profile {
                        detail: "--source-kind ssh-log requires --source-host <host>".into(),
                    });
                }
                if self.source_path.is_none() {
                    return Err(LensError::Profile {
                        detail: "--source-kind ssh-log requires --source-path <log-path>".into(),
                    });
                }
            }
        }
        Ok(())
    }

    /// CB5: NOT `#[cfg(test)]`. Integration tests under `tests/*.rs` link the
    /// lib without `cfg(test)`, so cfg(test) helpers are invisible there.
    /// Use `#[doc(hidden)] pub fn` to keep the public surface clean while
    /// remaining linkable. Same recipe as `FakePrompter::last_prompt` (CB-r2-1).
    #[doc(hidden)]
    pub fn default_for_test() -> InitArgs {
        InitArgs {
            profile: None,
            source_kind: None,
            scope: None,
            source_host: None,
            source_port: None,
            source_database: None,
            source_username: None,
            source_password_env: None,
            source_ssh_host: None,
            source_local_port: None,
            source_path: None,
            source_json_text_columns: Vec::new(),
            clients: Vec::new(),
            no_mcp_config: false,
            no_agents_md: false,
            also_claude_md: false,
            allow_overwrite: false,
            non_interactive: false,
            print_only: false,
            write_all: false,
            smoke_check: false,
        }
    }
}

pub fn run(
    args: InitArgs,
    project_config: Option<&Path>,
    user_config: Option<&Path>,
) -> Result<(), LensError> {
    args.validate()?;

    // Legacy v0.2.0 body kept until P4 lands the guided flow. The only changes
    // are: validate() runs at the top, and the new flag matrix is parsed by clap.
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
