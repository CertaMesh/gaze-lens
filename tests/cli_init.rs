use assert_cmd::Command;

#[test]
fn init_print_only_writes_nothing() {
    // --print-only renders preview to stdout and exits 0 without writes.
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join(".gaze-lens.toml");
    let user = temp.path().join("profiles.toml");

    let output = Command::cargo_bin("gaze-lens")
        .expect("binary")
        .args([
            "--project-config",
            project.to_str().expect("project path"),
            "--user-config",
            user.to_str().expect("user path"),
            "init",
            "--print-only",
            "--non-interactive",
            "--profile",
            "demo",
            "--source-kind",
            "sqlite",
            "--source-path",
            "/tmp/x.db",
            "--scope",
            "user",
            "--no-mcp-config",
            "--no-agents-md",
        ])
        .output()
        .expect("run init");

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let stdout = stdout(&output);
    assert!(stdout.contains("demo"), "stdout: {stdout}");
    assert!(!project.exists());
    assert!(!user.exists());
}

#[test]
fn init_no_tty_no_flag_exits_one() {
    // Directive 10: stdin OR stdout not a tty → exit 1 before any FS op.
    // assert_cmd pipes stdin by default, so this triggers without extra setup.
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join(".gaze-lens.toml");
    let user = temp.path().join("profiles.toml");

    let output = Command::cargo_bin("gaze-lens")
        .expect("binary")
        .args([
            "--project-config",
            project.to_str().expect("project path"),
            "--user-config",
            user.to_str().expect("user path"),
            "init",
        ])
        .write_stdin("\n")
        .output()
        .expect("run init");

    assert!(!output.status.success());
    let stderr = stderr(&output);
    assert!(
        stderr.contains("not a tty") || stderr.contains("tty"),
        "stderr should cite tty guard: {stderr}"
    );
    assert!(!project.exists());
    assert!(!user.exists());
}

#[test]
fn init_non_interactive_user_scope_writes_profile() {
    let temp = tempfile::tempdir().expect("tempdir");
    let user = temp.path().join("u.toml");

    let output = Command::cargo_bin("gaze-lens")
        .expect("binary")
        .args([
            "--user-config",
            user.to_str().expect("user path"),
            "init",
            "--non-interactive",
            "--profile",
            "x",
            "--source-kind",
            "sqlite",
            "--source-path",
            "/tmp/x.db",
            "--scope",
            "user",
            "--no-mcp-config",
            "--no-agents-md",
        ])
        .output()
        .expect("run init");

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(user.exists(), "user profile written");
    let toml = std::fs::read_to_string(&user).unwrap();
    assert!(toml.contains(r#"name = "x""#), "got: {toml}");
}

#[test]
fn init_non_interactive_local_log_requires_source_path() {
    let temp = tempfile::tempdir().expect("tempdir");
    let user = temp.path().join("u.toml");

    let output = Command::cargo_bin("gaze-lens")
        .expect("binary")
        .args([
            "--user-config",
            user.to_str().expect("user path"),
            "init",
            "--non-interactive",
            "--profile",
            "local",
            "--source-kind",
            "local-log",
            "--scope",
            "user",
            "--no-mcp-config",
            "--no-agents-md",
        ])
        .output()
        .expect("run init");

    assert!(!output.status.success());
    let stderr = stderr(&output);
    assert!(stderr.contains("--source-kind local-log requires --source-path <log-path>"));
    assert!(!user.exists());
}

#[test]
fn init_non_interactive_user_scope_writes_local_log_profile() {
    let temp = tempfile::tempdir().expect("tempdir");
    let user = temp.path().join("u.toml");

    let output = Command::cargo_bin("gaze-lens")
        .expect("binary")
        .args([
            "--user-config",
            user.to_str().expect("user path"),
            "init",
            "--non-interactive",
            "--profile",
            "local",
            "--source-kind",
            "local-log",
            "--source-path",
            "/tmp/app.log",
            "--scope",
            "user",
            "--no-mcp-config",
            "--no-agents-md",
        ])
        .output()
        .expect("run init");

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let toml = std::fs::read_to_string(&user).expect("profile");
    assert!(toml.contains(r#"name = "local""#), "got: {toml}");
    assert!(toml.contains(r#"kind = "local_log""#), "got: {toml}");
    assert!(toml.contains(r#"path = "/tmp/app.log""#), "got: {toml}");
    assert!(!toml.contains("host ="), "got: {toml}");
}

#[test]
fn user_config_override_writes_exact_path() {
    // CB4: explicit --user-config path overrides ~/.gaze-lens/profiles.toml default.
    let dir = tempfile::tempdir().unwrap();
    let custom = dir.path().join("custom_profiles.toml");
    let output = Command::cargo_bin("gaze-lens")
        .unwrap()
        .args([
            "--user-config",
            custom.to_str().unwrap(),
            "init",
            "--non-interactive",
            "--profile",
            "x",
            "--source-kind",
            "sqlite",
            "--source-path",
            "/tmp/x.db",
            "--scope",
            "user",
            "--no-mcp-config",
            "--no-agents-md",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(custom.exists(), "custom path written");
    let toml = std::fs::read_to_string(&custom).unwrap();
    assert!(toml.contains(r#"name = "x""#));
}

fn stdout(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}
