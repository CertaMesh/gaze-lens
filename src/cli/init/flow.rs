//! Guided init flow.
//!
//! `run_guided` walks the operator through profile creation interactively
//! and returns an in-memory `InitPlan`. `commit_plan` (in `mod.rs`) consumes
//! the plan via `BatchWriter`. Pure-function design — no FS writes happen
//! inside `run_guided`.
//!
//! ## Flow order (interactive)
//!
//! 1. Profile name (text, default "dev").
//! 2. Source kind (select 4: mysql / postgres / sqlite / ssh-log).
//! 3. Source params (per-kind: path / host+port+db+user+passenv+optional tunnel / host+path).
//! 4. Scope (select 3: user / project / project-auto-purge).
//! 5. Destructive consent (only for `project-auto-purge`). Decline → Err.
//! 6. MCP clients (skipped if `--no-mcp-config`). Empty `--client` defaults to claude-code.
//! 7. AGENTS.md patch (skipped if `--no-agents-md`). Default-N if file exists w/o markers.
//!
//! In `--non-interactive` mode, every step uses values supplied via flags.
//! ZERO prompter calls — `FakePrompter::new()` (strict) suffices.

use std::path::{Path, PathBuf};

use crate::cli::init::plan::{AgentsMdPatch, AutoPurgeChoice, InitPlan, McpTarget, ProfileSection};
use crate::cli::init::prompter::{PromptError, Prompter};
use crate::cli::init::{InitArgs, InitScope, McpClient, SourceKind};
use crate::errors::LensError;

/// CB4: carry `--project-config` and `--user-config` overrides so flow
/// resolves the destination path through the same CLI path as
/// `gaze_lens::profile::load_profiles`.
pub struct InitEnv {
    pub home: PathBuf,
    pub cwd: PathBuf,
    pub project_config: Option<PathBuf>,
    pub user_config: Option<PathBuf>,
}

impl InitEnv {
    pub fn detect(
        project_config: Option<PathBuf>,
        user_config: Option<PathBuf>,
    ) -> Result<Self, LensError> {
        let home =
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .ok_or_else(|| LensError::Profile {
                    detail: "HOME unset".into(),
                })?;
        let cwd = std::env::current_dir().map_err(|err| LensError::Profile {
            detail: format!("current_dir: {err}"),
        })?;
        Ok(Self {
            home,
            cwd,
            project_config,
            user_config,
        })
    }

    #[doc(hidden)]
    pub fn test_with_home(
        home: impl Into<PathBuf>,
        cwd: impl Into<PathBuf>,
        project_config: Option<PathBuf>,
        user_config: Option<PathBuf>,
    ) -> Self {
        Self {
            home: home.into(),
            cwd: cwd.into(),
            project_config,
            user_config,
        }
    }
}

/// CB4: explicit `--project-config` / `--user-config` overrides take precedence
/// over scope-default paths.
pub fn resolve_profile_path(scope: InitScope, env: &InitEnv) -> PathBuf {
    match scope {
        InitScope::Project | InitScope::ProjectAutoPurge => env
            .project_config
            .clone()
            .unwrap_or_else(|| env.cwd.join(".gaze-lens.toml")),
        InitScope::User => env
            .user_config
            .clone()
            .unwrap_or_else(|| env.home.join(".gaze-lens").join("profiles.toml")),
    }
}

const SOURCE_KINDS: &[&str] = &["mysql", "postgres", "sqlite", "ssh-log"];
const SCOPES: &[&str] = &["user", "project", "project-auto-purge"];

pub fn run_guided<P: Prompter>(
    args: &InitArgs,
    p: &mut P,
    env: &InitEnv,
) -> Result<InitPlan, LensError> {
    // Step 1 — profile name.
    let name = match args.profile.as_deref() {
        Some(s) => s.to_string(),
        None => {
            require_interactive(args)?;
            p.input("Profile name?", Some("dev"))
                .map_err(prompt_to_lens)?
        }
    };

    // Step 2 — source kind.
    let kind = match args.source_kind {
        Some(k) => k,
        None => {
            require_interactive(args)?;
            let i = p
                .select("Source kind?", SOURCE_KINDS)
                .map_err(prompt_to_lens)?;
            kind_from_index(i)
        }
    };

    // Step 3 — source params (per kind).
    let mut section = build_profile_section_skeleton(args, &name, kind);
    populate_source_params(&mut section, args, kind, p)?;
    // D15 / directive 13 / CB-r2-3 — validate host BEFORE any FS commit so a
    // dash-prefixed `--source-host -evil` can't slip into the rendered TOML.
    validate_section_hosts(&section)?;

    // Step 4 — scope.
    let scope = match args.scope {
        Some(s) => s,
        None => {
            require_interactive(args)?;
            let i = p
                .select("Where to write the profile?", SCOPES)
                .map_err(prompt_to_lens)?;
            scope_from_index(i)
        }
    };

    // Step 5 — destructive consent for ProjectAutoPurge.
    //
    // Non-interactive: the `--scope project-auto-purge` flag IS the consent
    // (CB1 — clap-level), so auto_purge = Purge. No extra prompt.
    //
    // Interactive: a destructive double-confirm is shown. Decline → abort.
    section.auto_purge = match scope {
        InitScope::ProjectAutoPurge if args.non_interactive => AutoPurgeChoice::Purge,
        InitScope::ProjectAutoPurge => {
            let n = section.snapshot_retention_days.unwrap_or(7);
            let ok = p
                .confirm(
                    &format!("This deletes snapshot files older than {n} days. Continue?"),
                    false,
                )
                .map_err(prompt_to_lens)?;
            if !ok {
                return Err(LensError::Profile {
                    detail: "auto-purge consent declined; rerun without --scope project-auto-purge to write a non-purging profile".into(),
                });
            }
            AutoPurgeChoice::Purge
        }
        _ => AutoPurgeChoice::Off,
    };

    // Step 6 — MCP targets.
    let mcp_targets = if args.no_mcp_config {
        Vec::new()
    } else {
        choose_mcp_targets(args, &name, scope, env, p)?
    };

    // Step 7 — AGENTS.md patch.
    let agents_md = if args.no_agents_md {
        None
    } else {
        build_agents_md_patch(args, env, p)?
    };

    let profile_path = resolve_profile_path(scope, env);
    Ok(InitPlan {
        profile_path,
        profile_scope: scope,
        profile_section: section,
        mcp_targets,
        agents_md,
        smoke_check_password_env_value: None,
    })
}

fn require_interactive(args: &InitArgs) -> Result<(), LensError> {
    if args.non_interactive {
        return Err(LensError::Profile {
            detail: "--non-interactive requires all required inputs as flags".into(),
        });
    }
    Ok(())
}

fn kind_from_index(i: usize) -> SourceKind {
    match i {
        0 => SourceKind::Mysql,
        1 => SourceKind::Postgres,
        2 => SourceKind::Sqlite,
        _ => SourceKind::SshLog,
    }
}

fn scope_from_index(i: usize) -> InitScope {
    match i {
        0 => InitScope::User,
        1 => InitScope::Project,
        _ => InitScope::ProjectAutoPurge,
    }
}

fn build_profile_section_skeleton(args: &InitArgs, name: &str, kind: SourceKind) -> ProfileSection {
    ProfileSection {
        name: name.to_string(),
        source_kind: kind,
        source_host: args.source_host.clone(),
        source_port: args.source_port,
        source_database: args.source_database.clone(),
        source_username: args.source_username.clone(),
        source_password_env: args.source_password_env.clone(),
        source_ssh_host: args.source_ssh_host.clone(),
        source_local_port: args.source_local_port,
        source_path: args.source_path.clone(),
        source_json_text_columns: args.source_json_text_columns.clone(),
        policy_path: None,
        schema_allowlist: Vec::new(),
        snapshot_retention_days: None,
        auto_purge: AutoPurgeChoice::Off,
    }
}

fn populate_source_params<P: Prompter>(
    section: &mut ProfileSection,
    args: &InitArgs,
    kind: SourceKind,
    p: &mut P,
) -> Result<(), LensError> {
    match kind {
        SourceKind::Sqlite => {
            if section.source_path.is_none() {
                require_interactive(args)?;
                let s = p
                    .input("SQLite database path?", None)
                    .map_err(prompt_to_lens)?;
                section.source_path = Some(PathBuf::from(s));
            }
        }
        SourceKind::SshLog => {
            if section.source_host.is_none() {
                require_interactive(args)?;
                let s = p.input("SSH host?", None).map_err(prompt_to_lens)?;
                section.source_host = Some(s);
            }
            if section.source_path.is_none() {
                require_interactive(args)?;
                let s = p.input("Remote log path?", None).map_err(prompt_to_lens)?;
                section.source_path = Some(PathBuf::from(s));
            }
        }
        SourceKind::Mysql | SourceKind::Postgres => {
            let default_port: u16 = if matches!(kind, SourceKind::Mysql) {
                3306
            } else {
                5432
            };
            if section.source_host.is_none() {
                require_interactive(args)?;
                let s = p.input("Database host?", None).map_err(prompt_to_lens)?;
                section.source_host = Some(s);
            }
            if section.source_port.is_none() {
                require_interactive(args)?;
                let s = p
                    .input("Database port?", Some(&default_port.to_string()))
                    .map_err(prompt_to_lens)?;
                section.source_port = Some(s.parse().map_err(|err| LensError::Profile {
                    detail: format!("invalid port `{s}`: {err}"),
                })?);
            }
            if section.source_database.is_none() {
                require_interactive(args)?;
                let s = p.input("Database name?", None).map_err(prompt_to_lens)?;
                section.source_database = Some(s);
            }
            if section.source_username.is_none() {
                require_interactive(args)?;
                let s = p
                    .input("Database username?", None)
                    .map_err(prompt_to_lens)?;
                section.source_username = Some(s);
            }
            if section.source_password_env.is_none() {
                require_interactive(args)?;
                let s = p
                    .input(
                        "Env var holding DB password?",
                        Some("GAZE_LENS_DB_PASSWORD"),
                    )
                    .map_err(prompt_to_lens)?;
                section.source_password_env = Some(s);
            }
        }
    }
    Ok(())
}

fn choose_mcp_targets<P: Prompter>(
    args: &InitArgs,
    name: &str,
    scope: InitScope,
    env: &InitEnv,
    p: &mut P,
) -> Result<Vec<McpTarget>, LensError> {
    let clients: Vec<McpClient> = if !args.clients.is_empty() {
        args.clients.clone()
    } else if args.non_interactive {
        // Non-interactive without explicit `--client` → no MCP targets.
        Vec::new()
    } else {
        // Interactive: single confirm gate. Yes → default to claude-code.
        // Operators wanting other clients pass `--client` explicitly.
        let yes = p
            .confirm(
                "Configure MCP server entry for Claude Code (.mcp.json in cwd)?",
                true,
            )
            .map_err(prompt_to_lens)?;
        if yes {
            vec![McpClient::ClaudeCode]
        } else {
            Vec::new()
        }
    };

    let mut targets = Vec::new();
    for client in clients {
        let path = match client {
            McpClient::Codex => env.home.join(".codex").join("config.toml"),
            McpClient::ClaudeCode => env.cwd.join(".mcp.json"),
            McpClient::Cursor => match scope {
                InitScope::User => env.home.join(".cursor").join("mcp.json"),
                _ => env.cwd.join(".cursor").join("mcp.json"),
            },
        };
        targets.push(McpTarget {
            client,
            path,
            command: "gaze-lens".to_string(),
            args: vec!["serve".into(), "--profile".into(), name.to_string()],
            profile_name: name.to_string(),
        });
    }
    Ok(targets)
}

fn build_agents_md_patch<P: Prompter>(
    args: &InitArgs,
    env: &InitEnv,
    p: &mut P,
) -> Result<Option<AgentsMdPatch>, LensError> {
    let path = env.cwd.join("AGENTS.md");
    let exists = path.exists();
    let yes = if args.non_interactive {
        // Non-interactive: only patch if `--no-agents-md` is NOT set (we are
        // already past that gate) AND a project AGENTS.md exists. Default-N
        // for fresh files in non-interactive mode (don't litter).
        exists
    } else {
        // Default-N when file already exists without markers (directive 12).
        // Default-Y for fresh files (helpful UX).
        let default = !exists;
        let prompt = if exists {
            "AGENTS.md exists. Append a gaze-lens snippet (bounded by markers)?"
        } else {
            "Create AGENTS.md with the gaze-lens snippet?"
        };
        p.confirm(prompt, default).map_err(prompt_to_lens)?
    };
    if !yes {
        return Ok(None);
    }
    let also_claude_md = if args.also_claude_md {
        let cm = env.cwd.join("CLAUDE.md");
        if cm.exists() { Some(cm) } else { None }
    } else {
        None
    };
    Ok(Some(AgentsMdPatch {
        path,
        also_claude_md,
    }))
}

/// D15 + CB-r2-3: route hosts through `validate_ssh_host` BEFORE the plan
/// reaches `commit_plan`. Single source of truth — same validator that
/// `serve` and `check` use (`src/source/ssh_tunnel.rs:72`).
///
/// - `SourceKind::SshLog`: `source_host` is the SSH host (TOML field `host`
///   per `src/profile.rs:70-73`); validate it.
/// - `SourceKind::{Mysql, Postgres}` with `source_ssh_host` set: validate the
///   tunnel jump host (TOML field `ssh_host`). The DB host (`source_host`)
///   is not an SSH target so it's not run through this validator.
/// - `SourceKind::Sqlite`: no host fields; nothing to validate.
fn validate_section_hosts(section: &ProfileSection) -> Result<(), LensError> {
    match section.source_kind {
        SourceKind::SshLog => {
            if let Some(host) = section.source_host.as_deref() {
                crate::source::ssh_tunnel::validate_ssh_host(host).map_err(|err| {
                    LensError::Profile {
                        detail: format!("invalid ssh host: {err}"),
                    }
                })?;
            }
        }
        SourceKind::Mysql | SourceKind::Postgres => {
            if let Some(host) = section.source_ssh_host.as_deref() {
                crate::source::ssh_tunnel::validate_ssh_host(host).map_err(|err| {
                    LensError::Profile {
                        detail: format!("invalid ssh host: {err}"),
                    }
                })?;
            }
        }
        SourceKind::Sqlite => {}
    }
    Ok(())
}

fn prompt_to_lens(err: PromptError) -> LensError {
    LensError::Profile {
        detail: format!("prompt failed: {err}"),
    }
}

/// Render an in-memory text preview of the plan. Used by `--print-only`.
pub fn render_preview(plan: &InitPlan) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "--- profile: {} ({}) ---\n",
        plan.profile_path.display(),
        scope_label(plan.profile_scope)
    ));
    out.push_str(&format!("name = {:?}\n", plan.profile_section.name));
    out.push_str(&format!(
        "kind = {}\n",
        kind_to_toml_str(plan.profile_section.source_kind)
    ));
    if let Some(p) = &plan.profile_section.source_path {
        out.push_str(&format!("path = {}\n", p.display()));
    }
    if let Some(h) = &plan.profile_section.source_host {
        out.push_str(&format!("host = {h}\n"));
    }
    if matches!(plan.profile_section.auto_purge, AutoPurgeChoice::Purge) {
        out.push_str("auto_purge = \"purge\"\n");
    }
    out.push_str(&format!(
        "--- mcp targets ({}) ---\n",
        plan.mcp_targets.len()
    ));
    for t in &plan.mcp_targets {
        out.push_str(&format!("{:?} -> {}\n", t.client, t.path.display()));
    }
    if let Some(am) = &plan.agents_md {
        out.push_str(&format!("--- agents.md: {} ---\n", am.path.display()));
    }
    out
}

fn scope_label(s: InitScope) -> &'static str {
    match s {
        InitScope::User => "user",
        InitScope::Project => "project",
        InitScope::ProjectAutoPurge => "project-auto-purge",
    }
}

fn kind_to_toml_str(k: SourceKind) -> &'static str {
    match k {
        SourceKind::Mysql => "mysql",
        SourceKind::Postgres => "postgres",
        SourceKind::Sqlite => "sqlite",
        SourceKind::SshLog => "ssh_log",
    }
}

/// Helper exposed for tests asserting `path` resolution behavior.
#[doc(hidden)]
pub fn _resolve_profile_path(scope: InitScope, env_home: &Path, env_cwd: &Path) -> PathBuf {
    let env = InitEnv {
        home: env_home.to_path_buf(),
        cwd: env_cwd.to_path_buf(),
        project_config: None,
        user_config: None,
    };
    resolve_profile_path(scope, &env)
}
