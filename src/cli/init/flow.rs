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

use crate::cli::init::discovery::{DISCOVERY_PATH_CHOICES, DISCOVERY_PATH_PROMPT, DiscoveryPath};
use crate::cli::init::plan::{
    AgentsMdPatch, AutoPurgeChoice, CredentialClass, InitPlan, McpTarget, PlannedSecret,
    ProfileSection,
};
use crate::cli::init::prompter::{PromptError, Prompter};
use crate::cli::init::ssh_exec::SshExec;
use crate::cli::init::{InitArgs, InitScope, McpClient, SecretBackendChoice, SourceKind};
use crate::errors::LensError;

/// CB4: carry `--project-config` and `--user-config` overrides so flow
/// resolves the destination path through the same CLI path as
/// `gaze_lens::profile::load_profiles`.
pub struct InitEnv {
    pub home: PathBuf,
    pub cwd: PathBuf,
    pub project_config: Option<PathBuf>,
    pub user_config: Option<PathBuf>,
    pub ssh_exec: Box<dyn SshExec>,
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
            ssh_exec: Box::new(crate::cli::init::ssh_exec::RealSsh),
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
            ssh_exec: Box::new(crate::cli::init::ssh_exec::MockSsh::default()),
        }
    }

    pub fn with_ssh_exec(mut self, ssh_exec: Box<dyn SshExec>) -> Self {
        self.ssh_exec = ssh_exec;
        self
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
const SCOPES: &[&str] = &[
    "user - local-only config in ~/.gaze-lens/profiles.toml; good for personal experiments or machine-specific access; not committed to repo",
    "project - shared project config in .gaze-lens.toml; good for team policy/profile defaults; secrets still come from env/keyring",
    "project-auto-purge - same as project, plus automatic deletion of old raw replay snapshot files after the retention window",
];

pub fn run_guided<P: Prompter>(
    args: &InitArgs,
    p: &mut P,
    env: &InitEnv,
) -> Result<InitPlan, LensError> {
    // Step 1 — profile name.
    let name = match args.profile.as_deref() {
        Some(s) => s.to_string(),
        None => {
            require_interactive(args, "profile name", "--profile")?;
            p.input("Profile name?", Some("dev"))
                .map_err(prompt_to_lens)?
        }
    };

    // Step 2 — source kind.
    let discovery = match (&args.discover_ssh_host, &args.discover_env_path) {
        (Some(host), Some(path)) => Some((host.as_str(), path.as_path())),
        (None, None) => None,
        _ => {
            return Err(LensError::Profile {
                detail: "--discover-ssh-host requires --discover-env-path".into(),
            });
        }
    };
    if discovery.is_some() && args.print_only {
        return Err(LensError::Profile {
            detail: "--print-only conflicts with --discover-ssh-host".into(),
        });
    }

    let kind = if discovery.is_some() {
        args.source_kind.unwrap_or(SourceKind::Mysql)
    } else {
        match args.source_kind {
            Some(k) => k,
            None => {
                require_interactive(args, "source kind", "--source-kind")?;
                let i = p
                    .select("Source kind?", SOURCE_KINDS)
                    .map_err(prompt_to_lens)?;
                kind_from_index(i)
            }
        }
    };

    // Step 3 — source params (per kind).
    let mut section = build_profile_section_skeleton(args, &name, kind);
    if let Some((host, path)) = discovery {
        populate_discovered_source_params(
            &mut section,
            args,
            host,
            path,
            env.ssh_exec.as_ref(),
            p,
        )?;
    } else {
        populate_source_params(&mut section, args, kind, p)?;
    }
    // D15 / directive 13 / CB-r2-3 — validate host BEFORE any FS commit so a
    // dash-prefixed `--source-host -evil` can't slip into the rendered TOML.
    validate_section_hosts(&section)?;

    // Step 4 — scope.
    let scope = match args.scope {
        Some(s) => s,
        None => {
            require_interactive(args, "profile scope", "--scope")?;
            let i = p
                .select("Where to write the profile? Choose profile scope:", SCOPES)
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

fn require_interactive(
    args: &InitArgs,
    field: impl Into<String>,
    flag: &str,
) -> Result<(), LensError> {
    if args.non_interactive {
        return Err(LensError::Profile {
            detail: format!("missing required field: {} ({flag})", field.into()),
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
        source_secret: None,
        source_ssh_host: args.source_ssh_host.clone(),
        source_local_port: args.source_local_port,
        source_path: args.source_path.clone(),
        source_json_text_columns: args.source_json_text_columns.clone(),
        policy_path: None,
        schema_allowlist: Vec::new(),
        snapshot_retention_days: None,
        discovered_from_ssh_host: None,
        discovered_from_path: None,
        discovered_at: None,
        discovered_ssh_host_key_fingerprint: None,
        credential_class: CredentialClass::ManuallyEntered,
        auto_purge: AutoPurgeChoice::Off,
    }
}

fn populate_discovered_source_params<P: Prompter>(
    section: &mut ProfileSection,
    args: &InitArgs,
    host: &str,
    path: &Path,
    ssh: &dyn SshExec,
    p: &mut P,
) -> Result<(), LensError> {
    crate::source::ssh_tunnel::validate_ssh_login_host(host).map_err(|err| LensError::Profile {
        detail: format!("invalid ssh host: {err}"),
    })?;
    crate::cli::init::discovery::validate_env_path(path)?;
    let raw = ssh.cat_capped(host, path, args.allow_new_ssh_host)?;
    if raw.truncated {
        return Err(LensError::Profile {
            detail: "remote .env exceeds 64 KiB cap; refusing".into(),
        });
    }
    let text = std::str::from_utf8(&raw.bytes).map_err(|_| LensError::Profile {
        detail: ".env content is not UTF-8".into(),
    })?;
    let mut vars = crate::cli::init::discovery::parse_env(text)?;
    if vars.is_empty() {
        return Err(LensError::Profile {
            detail: "no DB_* keys found in remote .env".into(),
        });
    }
    let (meta, mut password) = crate::cli::init::discovery::extract_db(&mut vars)?;
    section.source_kind = args.source_kind.or(meta.kind).unwrap_or(SourceKind::Mysql);
    section.source_host = meta.host.clone();
    section.source_port = meta.source_port_or_default(section.source_kind);
    section.source_database = meta.database.clone();
    section.discovered_from_ssh_host = Some(host.to_string());
    section.discovered_from_path = Some(path.to_path_buf());
    section.discovered_at = Some(time::OffsetDateTime::now_utc());
    section.discovered_ssh_host_key_fingerprint =
        ssh.host_key_fingerprint(host, args.allow_new_ssh_host).ok();

    let chosen = if args.accept_prod_rw.is_some() {
        DiscoveryPath::AsIs
    } else {
        require_interactive(args, "discovery path consent", "--accept-prod-rw")?;
        let i = p
            .select(DISCOVERY_PATH_PROMPT, DISCOVERY_PATH_CHOICES)
            .map_err(prompt_to_lens)?;
        match i {
            0 => DiscoveryPath::HostDbOnly,
            1 => DiscoveryPath::AsIs,
            _ => DiscoveryPath::Abort,
        }
    };

    match chosen {
        DiscoveryPath::Abort => Err(LensError::Profile {
            detail: "discovery aborted by operator; rerun with the right host/path or create a readonly user manually".into(),
        }),
        DiscoveryPath::AsIs => {
            let pw = password.take().filter(|z| !z.is_empty()).ok_or_else(|| {
                LensError::Profile {
                    detail: "Path A unavailable: discovered DB_PASSWORD is empty".into(),
                }
            })?;
            if args.accept_prod_rw.is_none() {
                let expected = meta.username.as_deref().unwrap_or("");
                let typed = p
                    .input("Type discovered DB username to store production credential?", None)
                    .map_err(prompt_to_lens)?;
                if typed != expected {
                    return Err(LensError::Profile {
                        detail: "discovery aborted by operator; rerun with the right host/path or create a readonly user manually".into(),
                    });
                }
            }
            section.source_username = meta.username.clone();
            section.source_password_env = None;
            section.source_secret = Some(PlannedSecret::Keyring {
                service: args
                    .source_password_keyring_service
                    .clone()
                    .unwrap_or_else(|| "gaze-lens".into()),
                account: args
                    .source_password_keyring_account
                    .clone()
                    .unwrap_or_else(|| section.name.clone()),
                write_value: Some(pw),
            });
            section.credential_class = CredentialClass::ProdRwCloned;
            Ok(())
        }
        DiscoveryPath::HostDbOnly => {
            drop(password);
            require_interactive(args, "readonly database credentials", "--source-username")?;
            let username = p
                .input("Readonly database username?", meta.username.as_deref())
                .map_err(prompt_to_lens)?;
            let password = p
                .password("Readonly database password for keyring?")
                .map_err(prompt_to_lens)?;
            section.source_username = Some(username);
            section.source_password_env = None;
            section.source_secret = Some(PlannedSecret::Keyring {
                service: args
                    .source_password_keyring_service
                    .clone()
                    .unwrap_or_else(|| "gaze-lens".into()),
                account: args
                    .source_password_keyring_account
                    .clone()
                    .unwrap_or_else(|| section.name.clone()),
                write_value: Some(zeroize::Zeroizing::new(password)),
            });
            section.credential_class = CredentialClass::ManuallyEntered;
            Ok(())
        }
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
                require_interactive(args, "sqlite path", "--source-path")?;
                let s = p
                    .input("SQLite database path?", None)
                    .map_err(prompt_to_lens)?;
                section.source_path = Some(PathBuf::from(s));
            }
        }
        SourceKind::SshLog => {
            if section.source_host.is_none() {
                require_interactive(args, "ssh_log host", "--source-host")?;
                let s = p.input("SSH host?", None).map_err(prompt_to_lens)?;
                section.source_host = Some(s);
            }
            if section.source_path.is_none() {
                require_interactive(args, "ssh_log path", "--source-path")?;
                let s = p.input("Remote log path?", None).map_err(prompt_to_lens)?;
                section.source_path = Some(PathBuf::from(s));
            }
        }
        SourceKind::Mysql | SourceKind::Postgres => {
            let kind_label = kind_to_toml_str(kind);
            let default_port: u16 = if matches!(kind, SourceKind::Mysql) {
                3306
            } else {
                5432
            };
            if section.source_host.is_none() {
                require_interactive(args, format!("{kind_label} host"), "--source-host")?;
                let s = p.input("Database host?", None).map_err(prompt_to_lens)?;
                section.source_host = Some(s);
            }
            if section.source_port.is_none() {
                require_interactive(args, format!("{kind_label} port"), "--source-port")?;
                let s = p
                    .input("Database port?", Some(&default_port.to_string()))
                    .map_err(prompt_to_lens)?;
                section.source_port = Some(s.parse().map_err(|err| LensError::Profile {
                    detail: format!("invalid port `{s}`: {err}"),
                })?);
            }
            if section.source_database.is_none() {
                require_interactive(args, format!("{kind_label} database"), "--source-database")?;
                let s = p.input("Database name?", None).map_err(prompt_to_lens)?;
                section.source_database = Some(s);
            }
            if section.source_username.is_none() {
                require_interactive(args, format!("{kind_label} username"), "--source-username")?;
                let s = p
                    .input("Database username?", None)
                    .map_err(prompt_to_lens)?;
                section.source_username = Some(s);
            }
            match args.secret_backend {
                SecretBackendChoice::Env => {
                    if section.source_password_env.is_none() && section.source_secret.is_none() {
                        require_interactive(
                            args,
                            format!("{kind_label} password env"),
                            "--source-password-env",
                        )?;
                        let s = p
                            .input(
                                "Env var holding DB password?",
                                Some("GAZE_LENS_DB_PASSWORD"),
                            )
                            .map_err(prompt_to_lens)?;
                        section.source_password_env = Some(s);
                    }
                }
                SecretBackendChoice::Keyring => {
                    let service = match args.source_password_keyring_service.clone() {
                        Some(service) => service,
                        None => {
                            require_interactive(
                                args,
                                format!("{kind_label} keyring service"),
                                "--source-password-keyring-service",
                            )?;
                            p.input("Keyring service?", Some("gaze-lens"))
                                .map_err(prompt_to_lens)?
                        }
                    };
                    let account = match args.source_password_keyring_account.clone() {
                        Some(account) => account,
                        None => {
                            require_interactive(
                                args,
                                format!("{kind_label} keyring account"),
                                "--source-password-keyring-account",
                            )?;
                            p.input("Keyring account?", Some(&section.name))
                                .map_err(prompt_to_lens)?
                        }
                    };
                    let write_value = if args.no_keyring_write {
                        None
                    } else {
                        require_interactive(
                            args,
                            format!("{kind_label} keyring write policy"),
                            "--no-keyring-write",
                        )?;
                        let should_write = p
                            .confirm("Write password to keyring now?", true)
                            .map_err(prompt_to_lens)?;
                        if should_write {
                            let password = p
                                .password("Database password for keyring?")
                                .map_err(prompt_to_lens)?;
                            Some(zeroize::Zeroizing::new(password))
                        } else {
                            None
                        }
                    };
                    section.source_password_env = None;
                    section.source_secret = Some(PlannedSecret::Keyring {
                        service,
                        account,
                        write_value,
                    });
                }
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
            args: vec!["serve".into()],
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

/// D15 + CB-r2-3: route hosts through `validate_ssh_login_host` BEFORE the plan
/// reaches `commit_plan`. Single source of truth — same validator the runtime
/// argv builders (`SshTunnel::open`, `remote_argv`, `tail_argv`) and
/// `--discover-ssh-host` use (`src/source/ssh_tunnel.rs:94`).
///
/// #504: this accepts `user@host` (and still rejects `-`-prefixed hosts and
/// shell metacharacters). The init gate must not be stricter than the runtime
/// that ultimately spawns `ssh`, otherwise valid `deploy@host` profiles are
/// rejected here but would work end-to-end.
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
                crate::source::ssh_tunnel::validate_ssh_login_host(host).map_err(|err| {
                    LensError::Profile {
                        detail: format!("invalid ssh host: {err}"),
                    }
                })?;
            }
        }
        SourceKind::Mysql | SourceKind::Postgres => {
            if let Some(host) = section.source_ssh_host.as_deref() {
                crate::source::ssh_tunnel::validate_ssh_login_host(host).map_err(|err| {
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
        ssh_exec: Box::new(crate::cli::init::ssh_exec::MockSsh::default()),
    };
    resolve_profile_path(scope, &env)
}
