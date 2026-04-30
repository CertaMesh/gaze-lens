use gaze_lens::cli::init::flow::{InitEnv, run_guided};
use gaze_lens::cli::init::plan::PlannedSecret;
use gaze_lens::cli::init::prompter::FakePrompter;
use gaze_lens::cli::init::{InitArgs, InitScope, SecretBackendChoice, SourceKind};

#[test]
fn flow_keyring_branch_prompts_password_and_captures_via_planned_secret() {
    let env = InitEnv::test_with_home("/tmp/fake-home", "/tmp/fake-cwd", None, None);
    let mut args = InitArgs::default_for_test();
    args.profile = Some("prod".into());
    args.source_kind = Some(SourceKind::Postgres);
    args.source_host = Some("db".into());
    args.source_port = Some(5432);
    args.source_database = Some("app".into());
    args.source_username = Some("ro".into());
    args.secret_backend = SecretBackendChoice::Keyring;
    args.scope = Some(InitScope::User);
    args.no_mcp_config = true;
    args.no_agents_md = true;
    let mut p = FakePrompter::new()
        .with_text("gaze-lens")
        .with_text("prod")
        .with_confirm(true)
        .with_password("hunter2-flow");

    let plan = run_guided(&args, &mut p, &env).expect("plan");
    match plan.profile_section.source_secret {
        Some(PlannedSecret::Keyring {
            service,
            account,
            write_value: Some(value),
        }) => {
            assert_eq!(service, "gaze-lens");
            assert_eq!(account, "prod");
            assert_eq!(value.as_str(), "hunter2-flow");
        }
        other => panic!("expected keyring write secret, got {other:?}"),
    }
}

#[test]
fn flow_keyring_branch_with_no_keyring_write_skips_password_prompt() {
    let env = InitEnv::test_with_home("/tmp/fake-home", "/tmp/fake-cwd", None, None);
    let mut args = InitArgs::default_for_test();
    args.non_interactive = true;
    args.profile = Some("prod".into());
    args.source_kind = Some(SourceKind::Mysql);
    args.source_host = Some("db".into());
    args.source_port = Some(3306);
    args.source_database = Some("app".into());
    args.source_username = Some("ro".into());
    args.secret_backend = SecretBackendChoice::Keyring;
    args.source_password_keyring_service = Some("gaze-lens".into());
    args.source_password_keyring_account = Some("prod".into());
    args.no_keyring_write = true;
    args.scope = Some(InitScope::User);
    args.no_mcp_config = true;
    args.no_agents_md = true;
    let mut p = FakePrompter::new();

    let plan = run_guided(&args, &mut p, &env).expect("plan");
    match plan.profile_section.source_secret {
        Some(PlannedSecret::Keyring {
            service,
            account,
            write_value: None,
        }) => {
            assert_eq!(service, "gaze-lens");
            assert_eq!(account, "prod");
        }
        other => panic!("expected keyring metadata without write, got {other:?}"),
    }
}
