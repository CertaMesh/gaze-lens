use assert_cmd::Command;

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
