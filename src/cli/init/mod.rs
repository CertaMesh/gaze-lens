use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use clap::{ArgAction, Args, ValueEnum};

use crate::errors::LensError;

pub mod agents_md;
pub mod atomic;
pub mod batch;
pub mod flow;
pub mod mcp_writer;
pub mod plan;
pub mod profile_writer;
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
    if args.non_interactive || args.allow_overwrite {
        commit_plan(&args, &plan, &mut writer)?;
    } else {
        let mut migration_prompter = prompter::DialoguerPrompter::new();
        commit_plan_with_prompter(&args, &plan, &mut writer, Some(&mut migration_prompter))?;
    }

    if args.smoke_check {
        run_smoke_check(&args, &plan)?;
    }
    Ok(())
}

/// `commit_plan`: profile → MCP → AGENTS, byte-compare-skip via `would_write`.
///
/// Profile bytes are validated in-memory via `crate::profile::validate_profile_bytes`
/// BEFORE the atomic-write rename (MS1) — preserves the no-bad-TOML-on-disk
/// guarantee. CB-r2-4: self-crate path is `crate::*`, NOT `gaze_lens::*`.
///
/// Errors are wrapped in `LensError::BatchPartial` so callers see what landed,
/// what didn't, and which step failed (CB6).
pub(crate) fn commit_plan(
    args: &InitArgs,
    plan: &plan::InitPlan,
    w: &mut dyn batch::BatchWriter,
) -> Result<(), LensError> {
    commit_plan_with_prompter(args, plan, w, None)
}

fn commit_plan_with_prompter(
    args: &InitArgs,
    plan: &plan::InitPlan,
    w: &mut dyn batch::BatchWriter,
    mut migration_prompter: Option<&mut dyn prompter::Prompter>,
) -> Result<(), LensError> {
    // Phase A: render + validate every candidate destination before the first
    // write. This is the atomicity contract for parse/collision failures:
    // malformed MCP JSON/TOML, malformed AGENTS markers, profile parse errors,
    // or name collisions return here with zero file writes.
    let writes = render_plan_writes(args, plan, &mut migration_prompter)?;

    // Phase B: ordered write/rename. Only write bytes that differ.
    let mut applied: Vec<PathBuf> = Vec::new();
    let mut pending: Vec<PathBuf> = writes.iter().map(|write| write.path.clone()).collect();
    let mut unchanged: Vec<PathBuf> = Vec::new();

    for write in &writes {
        ensure_parent_dir_for_write(&write.path, plan)?;
        if atomic::would_write(&write.path, &write.bytes) {
            write_one(w, &mut applied, &mut pending, &write.path, &write.bytes)?;
        } else {
            unchanged.push(write.path.clone());
            if let Some(idx) = pending.iter().position(|p| p == &write.path) {
                pending.remove(idx);
            }
        }
    }

    // Idempotency UX: when nothing changed, print "no changes".
    let total = applied.len() + unchanged.len();
    if !applied.is_empty() {
        for p in &applied {
            println!("wrote {}", p.display());
        }
    }
    if applied.is_empty() && unchanged.len() == total && total > 0 {
        println!("no changes");
    }
    Ok(())
}

struct RenderedWrite {
    path: PathBuf,
    bytes: Vec<u8>,
}

fn render_plan_writes(
    args: &InitArgs,
    plan: &plan::InitPlan,
    migration_prompter: &mut Option<&mut dyn prompter::Prompter>,
) -> Result<Vec<RenderedWrite>, LensError> {
    let mut writes = Vec::new();

    // Profile TOML first in write order, but still only rendered/validated here.
    let existing_profile = std::fs::read_to_string(&plan.profile_path).ok();
    let new_profile_bytes = profile_writer::render_profile_toml(
        existing_profile.as_deref(),
        &plan.profile_section,
        args.allow_overwrite || args.write_all,
    )
    .map_err(|e| match e {
        profile_writer::RenderError::Parse {
            line,
            column,
            source_msg,
            ..
        } => LensError::Profile {
            detail: format!(
                "malformed {} at line {line}, column {column}: {source_msg}",
                plan.profile_path.display(),
            ),
        },
        other => LensError::Profile {
            detail: other.to_string(),
        },
    })?
    .into_bytes();

    // 2.pre — MS1: in-memory schema-drift insurance BEFORE atomic_write rename.
    // CB-r2-4: self-crate path inside lib code is `crate::*`.
    crate::profile::validate_profile_bytes(&new_profile_bytes, &plan.profile_path)?;
    writes.push(RenderedWrite {
        path: plan.profile_path.clone(),
        bytes: new_profile_bytes,
    });

    // MCP JSON/TOML render validates existing config parse and entry collisions.
    for target in &plan.mcp_targets {
        let existing = std::fs::read_to_string(&target.path).ok();
        let rendered = render_mcp_target(
            target,
            existing.as_deref(),
            args.allow_overwrite,
            migration_prompter,
        )?;
        writes.push(RenderedWrite {
            path: target.path.clone(),
            bytes: rendered.into_bytes(),
        });
    }

    // AGENTS.md (+ optional CLAUDE.md) marker integrity is validated here.
    if let Some(patch) = &plan.agents_md {
        let existing = std::fs::read_to_string(&patch.path).ok();
        let rendered = crate::cli::init::agents_md::render_agents_md_patch(
            existing.as_deref(),
            &plan.profile_section.name,
        )
        .map_err(|e| LensError::Profile {
            detail: e.to_string(),
        })?;
        writes.push(RenderedWrite {
            path: patch.path.clone(),
            bytes: rendered.into_bytes(),
        });
        if let Some(cm) = &patch.also_claude_md {
            let existing = std::fs::read_to_string(cm).ok();
            let rendered = crate::cli::init::agents_md::render_agents_md_patch(
                existing.as_deref(),
                &plan.profile_section.name,
            )
            .map_err(|e| LensError::Profile {
                detail: e.to_string(),
            })?;
            writes.push(RenderedWrite {
                path: cm.clone(),
                bytes: rendered.into_bytes(),
            });
        }
    }

    Ok(writes)
}

fn ensure_parent_dir_for_write(path: &Path, plan: &plan::InitPlan) -> Result<(), LensError> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    if is_lens_owned_path(parent, plan) {
        atomic::create_dir_0700_if_missing(parent)?;
    } else if is_third_party_dotdir(parent) {
        if !parent.exists() {
            // Codex / Cursor user-scope dir doesn't exist yet. Create 0o700
            // because we own this creation; existing operator-set modes remain
            // sacrosanct and go through the read-only warning path.
            atomic::create_dir_0700_if_missing(parent)?;
        } else {
            atomic::assert_dir_0700_or_warn(parent)?;
        }
    }
    Ok(())
}

fn write_one(
    w: &mut dyn batch::BatchWriter,
    applied: &mut Vec<PathBuf>,
    pending: &mut Vec<PathBuf>,
    dest: &Path,
    contents: &[u8],
) -> Result<(), LensError> {
    match w.write(dest, contents) {
        Ok(()) => {
            applied.push(dest.to_path_buf());
            if let Some(idx) = pending.iter().position(|p| p == dest) {
                pending.remove(idx);
            }
            Ok(())
        }
        Err(e) => Err(LensError::BatchPartial {
            applied: applied.clone(),
            pending: pending.clone(),
            failed: dest.to_path_buf(),
            source: Box::new(e),
        }),
    }
}

fn is_lens_owned_path(parent: &Path, _plan: &plan::InitPlan) -> bool {
    parent
        .file_name()
        .and_then(|n| n.to_str())
        .map(|n| n == ".gaze-lens")
        .unwrap_or(false)
}

fn is_third_party_dotdir(parent: &Path) -> bool {
    parent
        .file_name()
        .and_then(|n| n.to_str())
        .map(|n| n == ".codex" || n == ".cursor")
        .unwrap_or(false)
}

fn render_mcp_target(
    target: &plan::McpTarget,
    existing: Option<&str>,
    allow_overwrite: bool,
    migration_prompter: &mut Option<&mut dyn prompter::Prompter>,
) -> Result<String, LensError> {
    let migration = migration_decision(target, existing, allow_overwrite, migration_prompter)?;
    let result = match target.client {
        McpClient::Codex => mcp_writer::render_codex_toml_with_migration(
            existing,
            &target.profile_name,
            &target.command,
            &target.args,
            allow_overwrite,
            migration,
        ),
        McpClient::ClaudeCode => mcp_writer::render_claude_code_json_with_migration(
            existing,
            &target.profile_name,
            &target.command,
            &target.args,
            allow_overwrite,
            migration,
        ),
        McpClient::Cursor => mcp_writer::render_cursor_json_with_migration(
            existing,
            &target.profile_name,
            &target.command,
            &target.args,
            allow_overwrite,
            migration,
        ),
    };
    result.map_err(|e| match e {
        profile_writer::RenderError::Parse {
            line,
            column,
            source_msg,
            ..
        } => LensError::Profile {
            detail: format!(
                "malformed {} at line {line}, column {column}: {source_msg}",
                target.path.display(),
            ),
        },
        other => LensError::Profile {
            detail: other.to_string(),
        },
    })
}

fn migration_decision(
    target: &plan::McpTarget,
    existing: Option<&str>,
    allow_overwrite: bool,
    migration_prompter: &mut Option<&mut dyn prompter::Prompter>,
) -> Result<mcp_writer::LegacyMigration, LensError> {
    if allow_overwrite {
        return Ok(mcp_writer::LegacyMigration::Migrate);
    }

    let prompt = match target.client {
        McpClient::Codex => mcp_writer::codex_toml_legacy_migration_prompt(existing),
        McpClient::ClaudeCode | McpClient::Cursor => {
            mcp_writer::mcp_json_legacy_migration_prompt(existing)
        }
    }
    .map_err(|e| match e {
        profile_writer::RenderError::Parse {
            line,
            column,
            source_msg,
            ..
        } => LensError::Profile {
            detail: format!(
                "malformed {} at line {line}, column {column}: {source_msg}",
                target.path.display(),
            ),
        },
        other => LensError::Profile {
            detail: other.to_string(),
        },
    })?;

    let Some(prompt) = prompt else {
        return Ok(mcp_writer::LegacyMigration::Migrate);
    };
    let Some(prompter) = migration_prompter.as_deref_mut() else {
        return Ok(mcp_writer::LegacyMigration::PreserveExisting);
    };

    if prompter
        .confirm(&prompt, false)
        .map_err(|err| LensError::Profile {
            detail: format!("prompt failed: {err}"),
        })?
    {
        Ok(mcp_writer::LegacyMigration::Migrate)
    } else {
        Ok(mcp_writer::LegacyMigration::PreserveExisting)
    }
}

/// Opt-in smoke check (directive 17). Calls the in-process `check` subcommand
/// via the same `pub async fn run(CheckArgs, Option<&Path>, Option<&Path>)`
/// signature exposed at `src/cli/check.rs:23-27`.
///
/// CB-r2-2: `CheckArgs.profile` is `String` (positional, default "default"),
/// NOT `Option<String>`. The (project_config, user_config) tuple is built
/// once from `plan.profile_scope` so role semantics cannot drift between the
/// caller and the smoke-check call.
fn run_smoke_check(_args: &InitArgs, plan: &plan::InitPlan) -> Result<(), LensError> {
    let (project_config, user_config): (Option<&Path>, Option<&Path>) = match plan.profile_scope {
        InitScope::Project | InitScope::ProjectAutoPurge => {
            (Some(plan.profile_path.as_path()), None)
        }
        InitScope::User => (None, Some(plan.profile_path.as_path())),
    };
    let check_args = crate::cli::check::CheckArgs {
        profile: plan.profile_section.name.clone(),
    };
    let runtime = tokio::runtime::Runtime::new().map_err(|err| LensError::Internal {
        detail: err.to_string(),
    })?;
    runtime.block_on(crate::cli::check::run(
        check_args,
        project_config,
        user_config,
    ))
}

/// `#[doc(hidden)] pub` test entry-point so integration tests can drive
/// `commit_plan` with a custom `BatchWriter` (e.g. `FailingWriter` for the
/// CB6 partial-failure assertion). Mirrors the `default_for_test` exposure
/// recipe (CB5).
#[doc(hidden)]
pub fn commit_plan_for_test(
    args: &InitArgs,
    plan: &plan::InitPlan,
    w: &mut dyn batch::BatchWriter,
) -> Result<(), LensError> {
    commit_plan(args, plan, w)
}
