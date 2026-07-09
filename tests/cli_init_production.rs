use assert_cmd::Command;
use gaze_lens::cli::init::batch::RealBatchWriter;
use gaze_lens::cli::init::flow::{InitEnv, render_preview, run_guided};
use gaze_lens::cli::init::model_fetch::FakeProvisioner;
use gaze_lens::cli::init::plan::FetchIntent;
use gaze_lens::cli::init::prompter::FakePrompter;
use gaze_lens::cli::init::{InitArgs, InitScope, SourceKind, commit_plan_for_test};

fn bin() -> Command {
    Command::cargo_bin("gaze-lens").unwrap()
}

#[test]
fn production_print_only_preview_marks_profile_production() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join(".gaze-lens.toml");

    let output = bin()
        .args([
            "--project-config",
            project.to_str().expect("project path"),
            "init",
            "--print-only",
            "--non-interactive",
            "--profile",
            "prod",
            "--source-kind",
            "sqlite",
            "--source-path",
            "/tmp/prod.db",
            "--scope",
            "project",
            "--production",
            "--no-mcp-config",
            "--no-agents-md",
        ])
        .output()
        .expect("run init");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("production = true"), "stdout: {stdout}");
    assert!(!project.exists(), "print-only must not write profile");
}

#[test]
fn production_flag_plans_policy_and_marks_profile() {
    let temp = tempfile::tempdir().expect("tempdir");
    let home = temp.path().join("home");
    let cwd = temp.path().join("repo");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&cwd).unwrap();
    let model_dir = cwd.join("models").join("kiji");
    let env = InitEnv::test_with_home(&home, &cwd, None, None);
    let mut args = InitArgs::default_for_test();
    args.non_interactive = true;
    args.profile = Some("prod".into());
    args.source_kind = Some(SourceKind::Sqlite);
    args.source_path = Some("/tmp/prod.db".into());
    args.scope = Some(InitScope::Project);
    args.production = true;
    args.model_dir = Some(model_dir.clone());
    args.no_mcp_config = true;
    args.no_agents_md = true;
    let mut p = FakePrompter::new();

    let plan = run_guided(&args, &mut p, &env).expect("plan");

    assert!(plan.profile_section.production);
    assert_eq!(
        plan.profile_section.policy_path.as_deref(),
        Some(cwd.join("gaze-policy.toml").as_path())
    );
    let policy_write = plan.policy_write.as_ref().expect("policy write");
    assert_eq!(policy_write.path, cwd.join("gaze-policy.toml"));
    assert_eq!(policy_write.model_dir, model_dir);
    assert!(plan.fetch_intent.is_none());
    let preview = render_preview(&plan);
    assert!(preview.contains("model: not installed yet"), "{preview}");
}

#[test]
fn production_noninteractive_writes_profile_and_policy_deferred() {
    let temp = tempfile::tempdir().expect("tempdir");
    let cwd = temp.path();
    let project = cwd.join(".gaze-lens.toml");
    let policy = cwd.join("gaze-policy.toml");
    let model_dir = cwd.join("models").join("kiji");

    let output = bin()
        .current_dir(cwd)
        .args([
            "--project-config",
            project.to_str().expect("project path"),
            "init",
            "--non-interactive",
            "--profile",
            "prod",
            "--source-kind",
            "sqlite",
            "--source-path",
            "/tmp/prod.db",
            "--scope",
            "project",
            "--production",
            "--model-dir",
            model_dir.to_str().expect("model path"),
            "--no-mcp-config",
            "--no-agents-md",
        ])
        .output()
        .expect("run init");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("model: not installed yet"),
        "stdout: {stdout}"
    );
    let profile_toml = std::fs::read_to_string(&project).expect("profile");
    assert!(profile_toml.contains("production = true"), "{profile_toml}");
    assert!(profile_toml.contains("policy = "), "{profile_toml}");
    let policy_toml = std::fs::read_to_string(&policy).expect("policy");
    assert!(policy_toml.contains("[ner]"), "{policy_toml}");
    assert!(
        policy_toml.contains("default_action = \"tokenize\""),
        "{policy_toml}"
    );
}

#[test]
fn fake_provision_error_happens_after_config_writes() {
    let temp = tempfile::tempdir().expect("tempdir");
    let home = temp.path().join("home");
    let cwd = temp.path().join("repo");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&cwd).unwrap();
    let model_dir = cwd.join("models").join("kiji");
    let env = InitEnv::test_with_home(&home, &cwd, None, None);
    let mut args = InitArgs::default_for_test();
    args.non_interactive = true;
    args.profile = Some("prod".into());
    args.source_kind = Some(SourceKind::Sqlite);
    args.source_path = Some("/tmp/prod.db".into());
    args.scope = Some(InitScope::Project);
    args.production = true;
    args.model_dir = Some(model_dir.clone());
    args.no_mcp_config = true;
    args.no_agents_md = true;
    let mut p = FakePrompter::new();
    let mut plan = run_guided(&args, &mut p, &env).expect("plan");
    plan.fetch_intent = Some(FetchIntent {
        model_dir: Some(model_dir),
    });
    let fake = FakeProvisioner::err("install failed");
    let mut writer = RealBatchWriter;

    let err =
        commit_plan_for_test(&args, &plan, &mut writer, Some(&fake)).expect_err("provision error");

    assert!(err.to_string().contains("install failed"), "{err}");
    assert!(
        plan.profile_path.exists(),
        "profile should already be written"
    );
    assert!(
        plan.policy_write
            .as_ref()
            .expect("policy write")
            .path
            .exists(),
        "policy should already be written"
    );
}
