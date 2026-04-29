use gaze_lens::cli::init::flow::{InitEnv, run_guided};
use gaze_lens::cli::init::plan::AutoPurgeChoice;
use gaze_lens::cli::init::prompter::FakePrompter;
use gaze_lens::cli::init::{InitArgs, InitScope, SourceKind};

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
