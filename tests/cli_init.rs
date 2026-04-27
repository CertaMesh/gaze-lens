use assert_cmd::Command;

#[test]
fn init_print_only_writes_nothing() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join(".gaze-lens.toml");
    let user = temp.path().join("profiles.toml");

    let mut cmd = Command::cargo_bin("gaze-lens").expect("binary");
    let output = cmd
        .args([
            "--project-config",
            project.to_str().expect("project path"),
            "--user-config",
            user.to_str().expect("user path"),
            "init",
            "--print-only",
        ])
        .output()
        .expect("run init");

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let stdout = stdout(&output);
    assert!(stdout.contains("[[profiles]]"));
    assert!(stdout.contains("schema_allowlist"));
    assert!(!project.exists());
    assert!(!user.exists());
}

#[test]
fn init_default_decline_writes_nothing() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join(".gaze-lens.toml");
    let user = temp.path().join("profiles.toml");

    let mut cmd = Command::cargo_bin("gaze-lens").expect("binary");
    let output = cmd
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

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(!project.exists());
    assert!(!user.exists());
}

#[test]
fn init_write_all_creates_files_but_refuses_overwrite() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join(".gaze-lens.toml");
    let user = temp.path().join("profiles.toml");

    let mut first = Command::cargo_bin("gaze-lens").expect("binary");
    let first = first
        .args([
            "--project-config",
            project.to_str().expect("project path"),
            "--user-config",
            user.to_str().expect("user path"),
            "init",
            "--write-all",
        ])
        .output()
        .expect("run init");
    assert!(first.status.success(), "stderr: {}", stderr(&first));
    assert!(project.exists());
    assert!(user.exists());

    let mut second = Command::cargo_bin("gaze-lens").expect("binary");
    let second = second
        .args([
            "--project-config",
            project.to_str().expect("project path"),
            "--user-config",
            user.to_str().expect("user path"),
            "init",
            "--write-all",
        ])
        .output()
        .expect("run init again");
    assert!(!second.status.success(), "stdout: {}", stdout(&second));
    assert!(stderr(&second).contains("already exists"));
}

fn stdout(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}
