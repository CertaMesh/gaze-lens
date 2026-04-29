use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use clap::{ArgAction, Args, ValueEnum};

use crate::errors::LensError;

pub mod atomic;
pub mod batch;
pub mod flow;
pub mod plan;
pub mod prompter;

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

    let env = flow::InitEnv::detect(
        project_config.map(PathBuf::from),
        user_config.map(PathBuf::from),
    )?;

    // Directive 10: TTY check covers stdin AND stdout. `--non-interactive` and
    // `--print-only` are explicit opt-outs of the guard.
    if !args.non_interactive
        && !args.print_only
        && (!std::io::stdin().is_terminal() || !std::io::stdout().is_terminal())
    {
        return Err(LensError::Profile {
            detail: "stdin or stdout is not a tty; rerun with --non-interactive (with required flags) or --print-only".into(),
        });
    }

    let plan = if args.non_interactive {
        let mut p = prompter::FakePrompter::new();
        flow::run_guided(&args, &mut p, &env)?
    } else {
        let mut p = prompter::DialoguerPrompter::new();
        flow::run_guided(&args, &mut p, &env)?
    };

    // Always render preview so operators see what will be written.
    let preview = flow::render_preview(&plan);
    print!("{preview}");

    if args.print_only {
        return Ok(());
    }

    let mut writer = batch::RealBatchWriter;
    commit_plan(&args, &plan, &mut writer)?;

    if args.smoke_check {
        run_smoke_check(&args, &plan)?;
    }
    Ok(())
}

/// commit_plan: profile → MCP → AGENTS, byte-compare-skip via `would_write`.
/// Full body lands in P6; for now the stub writes the profile section only
/// using a minimal serializer so existing tests keep working.
fn commit_plan(
    _args: &InitArgs,
    plan: &plan::InitPlan,
    w: &mut dyn batch::BatchWriter,
) -> Result<(), LensError> {
    // P5/P6 will replace this body with the canonical multi-target writer.
    // For now: write a minimal valid profile so tests/cli_init.rs file-write
    // assertions keep working and the legacy --write-all path still functions.
    if matches!(plan.profile_scope, InitScope::User)
        && let Some(parent) = plan.profile_path.parent()
    {
        atomic::create_dir_0700_if_missing(parent)?;
    }
    let bytes = render_minimal_profile(&plan.profile_section).into_bytes();
    if atomic::would_write(&plan.profile_path, &bytes) {
        if !plan.profile_path.exists() || _args.allow_overwrite || _args.write_all {
            w.write(&plan.profile_path, &bytes)?;
            println!("wrote {}", plan.profile_path.display());
        } else {
            return Err(LensError::Profile {
                detail: format!(
                    "{} already exists; refusing to overwrite",
                    plan.profile_path.display()
                ),
            });
        }
    } else {
        println!("no changes");
    }
    Ok(())
}

fn render_minimal_profile(s: &plan::ProfileSection) -> String {
    use plan::AutoPurgeChoice;
    let mut out = String::new();
    out.push_str("[[profiles]]\n");
    out.push_str(&format!("name = {:?}\n", s.name));
    if !s.schema_allowlist.is_empty() {
        let items: Vec<String> = s
            .schema_allowlist
            .iter()
            .map(|c| format!("{c:?}"))
            .collect();
        out.push_str(&format!("schema_allowlist = [{}]\n", items.join(", ")));
    }
    if let Some(d) = s.snapshot_retention_days {
        out.push_str(&format!("snapshot_retention_days = {d}\n"));
    }
    match s.auto_purge {
        AutoPurgeChoice::Off => {}
        AutoPurgeChoice::Warn => out.push_str("auto_purge = \"warn\"\n"),
        AutoPurgeChoice::Purge => out.push_str("auto_purge = \"purge\"\n"),
    }
    out.push_str("\n[profiles.source]\n");
    let kind_str = match s.source_kind {
        SourceKind::Mysql => "mysql",
        SourceKind::Postgres => "postgres",
        SourceKind::Sqlite => "sqlite",
        SourceKind::SshLog => "ssh_log",
    };
    out.push_str(&format!("kind = \"{kind_str}\"\n"));
    if let Some(h) = &s.source_host {
        out.push_str(&format!("host = {h:?}\n"));
    }
    if let Some(p) = s.source_port {
        out.push_str(&format!("port = {p}\n"));
    }
    if let Some(d) = &s.source_database {
        out.push_str(&format!("database = {d:?}\n"));
    }
    if let Some(u) = &s.source_username {
        out.push_str(&format!("username = {u:?}\n"));
    }
    if let Some(env) = &s.source_password_env {
        out.push_str(&format!("password_env = {env:?}\n"));
    }
    if let Some(h) = &s.source_ssh_host {
        out.push_str(&format!("ssh_host = {h:?}\n"));
    }
    if let Some(p) = s.source_local_port {
        out.push_str(&format!("local_port = {p}\n"));
    }
    if let Some(p) = &s.source_path {
        out.push_str(&format!("path = {:?}\n", p.display().to_string()));
    }
    if matches!(s.source_kind, SourceKind::Mysql | SourceKind::Postgres) {
        out.push_str("readonly_required = true\n");
    }
    if !s.source_json_text_columns.is_empty() && matches!(s.source_kind, SourceKind::Sqlite) {
        let items: Vec<String> = s
            .source_json_text_columns
            .iter()
            .map(|c| format!("{c:?}"))
            .collect();
        out.push_str(&format!("json_text_columns = [{}]\n", items.join(", ")));
    }
    out
}

fn run_smoke_check(_args: &InitArgs, _plan: &plan::InitPlan) -> Result<(), LensError> {
    // Implementation lands in P9.
    Ok(())
}
