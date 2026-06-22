use assert_cmd::Command;
use gaze_lens::cli::init::flow::{InitEnv, run_guided};
use gaze_lens::cli::init::plan::AutoPurgeChoice;
use gaze_lens::cli::init::prompter::FakePrompter;
use gaze_lens::cli::init::{InitArgs, InitScope, SourceKind};

const PRINT_ONLY_USER_CONFIG: &str = "/tmp/gaze-lens-init-print-only-user.toml";
const PRINT_ONLY_PROJECT_CONFIG: &str = "/tmp/gaze-lens-init-print-only-project.toml";

#[test]
fn guided_sqlite_user_scope_yields_expected_plan() {
    let env = InitEnv::test_with_home("/tmp/fake-home", "/tmp/fake-cwd", None, None);
    let args = InitArgs::default_for_test();
    // Flow order (interactive): name → kind → source_path (sqlite) →
    // scope → MCP confirm → AGENTS.md confirm.
    let mut p = FakePrompter::new()
        .with_text("dev")
        .with_select(2) // kind = sqlite
        .with_text("/tmp/x.db")
        .with_select(0) // scope = User
        .with_confirm(true) // MCP
        .with_confirm(true); // AGENTS.md
    let plan = run_guided(&args, &mut p, &env).expect("plan");
    assert_eq!(plan.profile_section.name, "dev");
    assert!(matches!(
        plan.profile_section.source_kind,
        SourceKind::Sqlite
    ));
    assert!(matches!(plan.profile_scope, InitScope::User));
    assert_eq!(plan.mcp_targets.len(), 1);
    assert!(plan.agents_md.is_some());
}

#[test]
fn non_interactive_sqlite_skips_prompts() {
    let env = InitEnv::test_with_home("/tmp/fake-home", "/tmp/fake-cwd", None, None);
    let mut args = InitArgs::default_for_test();
    args.non_interactive = true;
    args.profile = Some("ci".into());
    args.source_kind = Some(SourceKind::Sqlite);
    args.source_path = Some("/tmp/ci.db".into());
    args.scope = Some(InitScope::User);
    args.no_mcp_config = true;
    args.no_agents_md = true;
    // CB9: empty strict FakePrompter — non-interactive code path makes ZERO
    // prompter calls. If any prompt fires, ScriptExhausted errors out.
    let mut p = FakePrompter::new();
    let plan = run_guided(&args, &mut p, &env).expect("plan");
    assert_eq!(plan.profile_section.name, "ci");
    assert!(plan.mcp_targets.is_empty());
    assert!(plan.agents_md.is_none());
}

#[test]
fn non_interactive_mysql_missing_port_names_field() {
    let env = InitEnv::test_with_home("/tmp/fake-home", "/tmp/fake-cwd", None, None);
    let mut args = InitArgs::default_for_test();
    args.non_interactive = true;
    args.profile = Some("ci".into());
    args.source_kind = Some(SourceKind::Mysql);
    args.source_host = Some("db.example.invalid".into());
    args.source_database = Some("app".into());
    args.source_username = Some("app_user".into());
    args.source_password_env = Some("GAZE_LENS_DB_PASSWORD".into());
    args.scope = Some(InitScope::User);
    args.no_mcp_config = true;
    args.no_agents_md = true;
    let mut p = FakePrompter::new();

    let err = run_guided(&args, &mut p, &env).expect_err("missing port must error");
    let detail = err.to_string();
    assert!(
        detail.contains("missing required field: mysql port (--source-port)"),
        "{detail}"
    );
    assert!(
        !detail.contains("requires all required inputs as flags"),
        "{detail}"
    );
}

#[test]
fn auto_purge_destructive_confirm_present() {
    // Directive 11 + CB-r2-1: prompt template "This deletes snapshot files
    // older than {N} days. Continue?". `last_prompt` is `#[doc(hidden)] pub`
    // so this integration test in tests/*.rs can read the field directly.
    let env = InitEnv::test_with_home("/tmp/fake-home", "/tmp/fake-cwd", None, None);
    let mut args = InitArgs::default_for_test();
    args.scope = Some(InitScope::ProjectAutoPurge);
    args.profile = Some("p".into());
    args.source_kind = Some(SourceKind::Sqlite);
    args.source_path = Some("/tmp/x.db".into());
    // Strict FakePrompter scripted with one .with_confirm(false).
    let mut p = FakePrompter::new().with_confirm(false);
    let _result = run_guided(&args, &mut p, &env);
    assert!(
        p.last_prompt
            .as_deref()
            .map(|s| s.contains("deletes snapshot files older than"))
            .unwrap_or(false),
        "destructive-confirm prompt template must contain literal substring; got: {:?}",
        p.last_prompt,
    );
}

#[test]
fn project_auto_purge_in_non_interactive_mode_yields_purge_without_prompting() {
    // CB1 + non_interactive: clap-level consent IS the consent; no extra
    // destructive prompt. Empty strict FakePrompter must not be touched.
    let env = InitEnv::test_with_home("/tmp/fake-home", "/tmp/fake-cwd", None, None);
    let mut args = InitArgs::default_for_test();
    args.non_interactive = true;
    args.profile = Some("p".into());
    args.source_kind = Some(SourceKind::Sqlite);
    args.source_path = Some("/tmp/x.db".into());
    args.scope = Some(InitScope::ProjectAutoPurge);
    args.no_mcp_config = true;
    args.no_agents_md = true;
    let mut p = FakePrompter::new();
    let plan = run_guided(&args, &mut p, &env).expect("plan");
    assert!(matches!(
        plan.profile_section.auto_purge,
        AutoPurgeChoice::Purge
    ));
}

#[test]
fn profile_scope_prompt_explains_each_scope_impact() {
    let env = InitEnv::test_with_home("/tmp/fake-home", "/tmp/fake-cwd", None, None);
    let mut args = InitArgs::default_for_test();
    args.profile = Some("p".into());
    args.source_kind = Some(SourceKind::Sqlite);
    args.source_path = Some("/tmp/x.db".into());
    args.no_mcp_config = true;
    args.no_agents_md = true;
    let mut p = FakePrompter::new().with_select(0);
    let _plan = run_guided(&args, &mut p, &env).expect("plan");
    let choices = p.last_select_choices.expect("scope choices");
    let joined = choices.join("\n");
    assert!(joined.contains("user - local-only config in ~/.gaze-lens/profiles.toml"));
    assert!(joined.contains("not committed to repo"));
    assert!(joined.contains("project - shared project config in .gaze-lens.toml"));
    assert!(joined.contains("secrets still come from env/keyring"));
    assert!(joined.contains("project-auto-purge - same as project"));
    assert!(joined.contains("automatic deletion of old raw replay snapshot files"));
}

#[test]
fn ssh_log_non_interactive_renders_host_and_path() {
    let env = InitEnv::test_with_home("/tmp/fake-home", "/tmp/fake-cwd", None, None);
    let mut args = InitArgs::default_for_test();
    args.non_interactive = true;
    args.profile = Some("p".into());
    args.source_kind = Some(SourceKind::SshLog);
    args.source_host = Some("deploy.example.com".into());
    args.source_path = Some("/var/log/app.log".into());
    args.scope = Some(InitScope::User);
    args.no_mcp_config = true;
    args.no_agents_md = true;
    let mut p = FakePrompter::new();
    let plan = run_guided(&args, &mut p, &env).expect("plan");
    assert!(matches!(
        plan.profile_section.source_kind,
        SourceKind::SshLog
    ));
    assert_eq!(
        plan.profile_section.source_host.as_deref(),
        Some("deploy.example.com")
    );
    assert_eq!(
        plan.profile_section.source_path.as_deref(),
        Some(std::path::Path::new("/var/log/app.log"))
    );
}

#[test]
fn print_only_matches_golden_sqlite() {
    assert_print_only_matches_golden(
        &[
            "--source-kind",
            "sqlite",
            "--source-path",
            "/tmp/demo.sqlite",
        ],
        include_str!("fixtures/init_print_only_sqlite.golden.txt"),
    );
}

#[test]
fn print_only_matches_golden_mysql() {
    assert_print_only_matches_golden(
        &[
            "--source-kind",
            "mysql",
            "--source-host",
            "db.example.invalid",
            "--source-port",
            "3306",
            "--source-database",
            "app",
            "--source-username",
            "app_user",
            "--source-password-env",
            "GAZE_LENS_DB_PASSWORD",
        ],
        include_str!("fixtures/init_print_only_mysql.golden.txt"),
    );
}

#[test]
fn print_only_matches_golden_postgres() {
    assert_print_only_matches_golden(
        &[
            "--source-kind",
            "postgres",
            "--source-host",
            "pg.example.invalid",
            "--source-port",
            "5432",
            "--source-database",
            "app",
            "--source-username",
            "app_user",
            "--source-password-env",
            "GAZE_LENS_DB_PASSWORD",
        ],
        include_str!("fixtures/init_print_only_postgres.golden.txt"),
    );
}

#[test]
fn print_only_matches_golden_ssh_log() {
    assert_print_only_matches_golden(
        &[
            "--source-kind",
            "ssh-log",
            "--source-host",
            "logs.example.invalid",
            "--source-path",
            "/var/log/app.log",
        ],
        include_str!("fixtures/init_print_only_ssh_log.golden.txt"),
    );
}

fn assert_print_only_matches_golden(source_args: &[&str], golden: &str) {
    let mut args = vec![
        "--user-config",
        PRINT_ONLY_USER_CONFIG,
        "--project-config",
        PRINT_ONLY_PROJECT_CONFIG,
        "init",
        "--print-only",
        "--non-interactive",
        "--profile",
        "demo",
    ];
    args.extend_from_slice(source_args);
    args.extend_from_slice(&["--scope", "user", "--no-mcp-config", "--no-agents-md"]);

    let output = Command::cargo_bin("gaze-lens")
        .expect("binary")
        .args(args)
        .output()
        .expect("run init --print-only");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), golden);
    assert!(
        !std::path::Path::new(PRINT_ONLY_USER_CONFIG).exists(),
        "print-only must not write user config"
    );
    assert!(
        !std::path::Path::new(PRINT_ONLY_PROJECT_CONFIG).exists(),
        "print-only must not write project config"
    );
}
