use assert_cmd::Command;
use gaze_lens::cli::init::batch::RealBatchWriter;
use gaze_lens::cli::init::flow::{InitEnv, render_preview, run_guided};
use gaze_lens::cli::init::model_fetch::FakeProvisioner;
use gaze_lens::cli::init::plan::FetchIntent;
use gaze_lens::cli::init::prompter::FakePrompter;
use gaze_lens::cli::init::{InitArgs, InitScope, SourceKind, commit_plan_for_test};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Output;

fn bin() -> Command {
    Command::cargo_bin("gaze-lens").unwrap()
}

fn run_production_init_with_policy(cwd: &Path, project: &Path, allow_overwrite: bool) -> Output {
    let model_dir = cwd.join("models").join("kiji");
    let mut cmd = bin();
    cmd.current_dir(cwd).args([
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
    ]);
    if allow_overwrite {
        cmd.arg("--allow-policy-overwrite");
    }
    cmd.output().expect("run init")
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

#[cfg(unix)]
#[test]
fn production_existing_policy_unreadable_refuses_even_with_policy_overwrite_flag() {
    for allow_overwrite in [false, true] {
        let temp = tempfile::tempdir().expect("tempdir");
        let cwd = temp.path();
        let project = cwd.join(".gaze-lens.toml");
        let policy = cwd.join("gaze-policy.toml");
        let original = "[policy]\ndefault_action = \"preserve\"\n";
        std::fs::write(&policy, original).expect("policy");
        std::fs::set_permissions(&policy, std::fs::Permissions::from_mode(0o000))
            .expect("chmod policy");

        let output = run_production_init_with_policy(cwd, &project, allow_overwrite);

        std::fs::set_permissions(&policy, std::fs::Permissions::from_mode(0o600))
            .expect("restore policy mode");
        assert!(
            !output.status.success(),
            "allow_overwrite={allow_overwrite} stdout: {} stderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("failed to read existing production policy"),
            "allow_overwrite={allow_overwrite} stderr: {stderr}"
        );
        assert!(
            !project.exists(),
            "profile must not be written after unreadable policy"
        );
        assert_eq!(
            std::fs::read_to_string(&policy).expect("policy"),
            original,
            "policy must remain untouched"
        );
    }
}

#[test]
fn production_existing_policy_invalid_utf8_refuses_even_with_policy_overwrite_flag() {
    for allow_overwrite in [false, true] {
        let temp = tempfile::tempdir().expect("tempdir");
        let cwd = temp.path();
        let project = cwd.join(".gaze-lens.toml");
        let policy = cwd.join("gaze-policy.toml");
        let original = b"\xff\xfe[policy]\n";
        std::fs::write(&policy, original).expect("policy");

        let output = run_production_init_with_policy(cwd, &project, allow_overwrite);

        assert!(
            !output.status.success(),
            "allow_overwrite={allow_overwrite} stdout: {} stderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("malformed production policy"),
            "allow_overwrite={allow_overwrite} stderr: {stderr}"
        );
        assert!(
            stderr.contains("not valid UTF-8"),
            "allow_overwrite={allow_overwrite} stderr: {stderr}"
        );
        assert!(
            !project.exists(),
            "profile must not be written after malformed policy"
        );
        assert_eq!(
            std::fs::read(&policy).expect("policy"),
            original,
            "policy must remain untouched"
        );
    }
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
