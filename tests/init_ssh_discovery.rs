use std::path::Path;

use gaze_lens::cli::init::flow::{InitEnv, run_guided};
use gaze_lens::cli::init::plan::{CredentialClass, PlannedSecret};
use gaze_lens::cli::init::profile_writer::render_profile_toml;
use gaze_lens::cli::init::prompter::FakePrompter;
use gaze_lens::cli::init::ssh_exec::{CatOutput, MockSsh};
use gaze_lens::cli::init::{InitArgs, InitScope, SourceKind};
use gaze_lens::errors::LensError;

fn env_with_ssh(ssh: MockSsh) -> InitEnv {
    InitEnv::test_with_home("/tmp/fake-home", "/tmp/fake-cwd", None, None)
        .with_ssh_exec(Box::new(ssh))
}

fn discovery_args() -> InitArgs {
    let mut args = InitArgs::default_for_test();
    args.profile = Some("prod".into());
    args.scope = Some(InitScope::User);
    args.no_mcp_config = true;
    args.no_agents_md = true;
    args.source_kind = Some(SourceKind::Mysql);
    args.discover_ssh_host = Some("deploy@app01".into());
    args.discover_env_path = Some("/var/www/app/.env".into());
    args
}

fn env_bytes() -> Vec<u8> {
    b"DB_HOST=127.0.0.1\nDB_PORT=3306\nDB_DATABASE=app\nDB_USERNAME=prod_user\nDB_PASSWORD=prod-secret\n".to_vec()
}

#[test]
fn discover_path_b_uses_operator_credential_and_drops_discovered_password() {
    let ssh = MockSsh::default()
        .with_response(
            "deploy@app01",
            "/var/www/app/.env",
            Ok(CatOutput {
                bytes: env_bytes(),
                truncated: false,
            }),
        )
        .with_fingerprint("deploy@app01", "SHA256:test");
    let env = env_with_ssh(ssh);
    let args = discovery_args();
    let mut p = FakePrompter::new()
        .with_select(0)
        .with_text("readonly_user")
        .with_password("readonly-secret");

    let plan = run_guided(&args, &mut p, &env).expect("plan");
    let section = &plan.profile_section;
    assert_eq!(section.source_username.as_deref(), Some("readonly_user"));
    assert_eq!(section.credential_class, CredentialClass::ManuallyEntered);
    assert_eq!(
        section.discovered_from_ssh_host.as_deref(),
        Some("deploy@app01")
    );
    assert_eq!(
        section.discovered_ssh_host_key_fingerprint.as_deref(),
        Some("SHA256:test")
    );
    let rendered = render_profile_toml(None, section, false).unwrap();
    assert!(!rendered.contains("prod-secret"), "{rendered}");
    assert!(!format!("{:?}", plan).contains("prod-secret"));
}

#[test]
fn discover_path_a_writes_keyring_value_when_username_typeback_matches() {
    let ssh = MockSsh::default().with_response(
        "deploy@app01",
        "/var/www/app/.env",
        Ok(CatOutput {
            bytes: env_bytes(),
            truncated: false,
        }),
    );
    let env = env_with_ssh(ssh);
    let args = discovery_args();
    let mut p = FakePrompter::new().with_select(1).with_text("prod_user");
    let plan = run_guided(&args, &mut p, &env).expect("plan");
    assert_eq!(
        plan.profile_section.credential_class,
        CredentialClass::ProdRwCloned
    );
    match plan.profile_section.source_secret {
        Some(PlannedSecret::Keyring {
            write_value: Some(value),
            ..
        }) => assert_eq!(value.as_str(), "prod-secret"),
        other => panic!("expected keyring secret with write value: {other:?}"),
    }
}

#[test]
fn discover_path_c_returns_profile_error() {
    let ssh = MockSsh::default().with_response(
        "deploy@app01",
        "/var/www/app/.env",
        Ok(CatOutput {
            bytes: env_bytes(),
            truncated: false,
        }),
    );
    let env = env_with_ssh(ssh);
    let args = discovery_args();
    let mut p = FakePrompter::new().with_select(2);
    let err = run_guided(&args, &mut p, &env).unwrap_err();
    assert!(matches!(err, LensError::Profile { .. }));
    assert!(err.to_string().contains("aborted"));
}

#[test]
fn discover_credential_prompt_keeps_select_labels_short_and_help_outside_items() {
    let ssh = MockSsh::default().with_response(
        "deploy@app01",
        "/var/www/app/.env",
        Ok(CatOutput {
            bytes: env_bytes(),
            truncated: false,
        }),
    );
    let env = env_with_ssh(ssh);
    let args = discovery_args();
    let mut p = FakePrompter::new().with_select(2);
    let _err = run_guided(&args, &mut p, &env).unwrap_err();
    let choices = p.last_select_choices.expect("discovery choices");
    assert_eq!(
        choices,
        vec![
            "Use separate read-only credential",
            "Store discovered production credential",
            "Abort without writing config",
        ]
    );
    assert!(
        choices.iter().all(|choice| choice.len() <= 40),
        "select labels should stay short enough for stable arrow-key repaint: {choices:?}"
    );
    assert!(
        choices.iter().all(|choice| !choice.contains(';')),
        "select labels must not carry long explanatory clauses: {choices:?}"
    );

    let prompt = p.last_prompt.expect("discovery prompt");
    assert!(prompt.contains("keep discovered host/database"));
    assert!(prompt.contains("least-privilege agent access"));
    assert!(prompt.contains("save DB username/password found in remote .env"));
    assert!(prompt.contains("usually too broad"));
    assert!(prompt.contains("stop discovery without writing config"));
    for choice in &choices {
        let duplicate_help_prefix = format!("{choice}:");
        assert!(
            !prompt.contains(&duplicate_help_prefix),
            "prompt help must not repeat select label as a detail heading: {duplicate_help_prefix}"
        );
    }
    assert!(!prompt.contains("prod-secret"));
}

#[test]
fn discover_with_print_only_makes_zero_ssh_calls_in_inner_flow() {
    let ssh = MockSsh::default();
    let recorder = ssh.clone();
    let env = env_with_ssh(ssh);
    let mut args = discovery_args();
    args.print_only = true;
    let mut p = FakePrompter::new();
    let err = run_guided(&args, &mut p, &env).unwrap_err();
    assert!(err.to_string().contains("--print-only"));
    assert_eq!(recorder.call_count(), 0);
}

#[test]
fn discover_path_error_redacts_path_string() {
    let path = Path::new("/var/www/app/.env");
    let ssh = MockSsh::default().with_response(
        "deploy@app01",
        path,
        Err("cat: /var/www/app/.env: Permission denied".into()),
    );
    let env = env_with_ssh(ssh);
    let args = discovery_args();
    let mut p = FakePrompter::new();
    let err = run_guided(&args, &mut p, &env).unwrap_err();
    assert!(!err.to_string().contains("/var/www/app/.env"));
    assert!(err.to_string().contains("<redacted>"));
}
