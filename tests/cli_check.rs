use assert_cmd::Command;
use rusqlite::Connection;

#[test]
fn check_validates_profile_policy_connection_and_pipeline_without_writes() {
    let temp = tempfile::tempdir().expect("tempdir");
    let db = temp.path().join("fixture.sqlite");
    let project = temp.path().join("project.toml");
    let policy = temp.path().join("policy.toml");
    seed_sqlite(&db);
    std::fs::write(&policy, "[policy.database]\n").expect("policy");
    write_profile(&project, &db, &policy);

    let mut cmd = Command::cargo_bin("gaze-lens").expect("binary");
    let output = cmd
        .args([
            "--project-config",
            project.to_str().expect("project path"),
            "check",
            "--profile",
            "local",
        ])
        .output()
        .expect("check");

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let stdout = stdout(&output);
    assert!(stdout.contains("profile: ok"));
    assert!(stdout.contains("policy: ok"));
    assert!(stdout.contains("source: ok"));
    assert!(stdout.contains("pipeline: ok"));
    assert!(!temp.path().join("manifest.sqlite").exists());
    assert!(!temp.path().join("snapshots").exists());
}

#[test]
fn check_reports_invalid_policy() {
    let temp = tempfile::tempdir().expect("tempdir");
    let db = temp.path().join("fixture.sqlite");
    let project = temp.path().join("project.toml");
    let policy = temp.path().join("policy.toml");
    seed_sqlite(&db);
    std::fs::write(&policy, "not valid toml =").expect("policy");
    write_profile(&project, &db, &policy);

    let mut cmd = Command::cargo_bin("gaze-lens").expect("binary");
    let output = cmd
        .args([
            "--project-config",
            project.to_str().expect("project path"),
            "check",
            "--profile",
            "local",
        ])
        .output()
        .expect("check");

    assert!(!output.status.success(), "stdout: {}", stdout(&output));
    assert!(stderr(&output).contains("failed to parse policy"));
}

fn seed_sqlite(path: &std::path::Path) {
    let conn = Connection::open(path).expect("sqlite");
    conn.execute_batch("CREATE TABLE users (id INTEGER PRIMARY KEY);")
        .expect("seed");
}

fn write_profile(path: &std::path::Path, db: &std::path::Path, policy: &std::path::Path) {
    std::fs::write(
        path,
        format!(
            r#"
            [[profiles]]
            name = "local"
            policy = "{}"
            source = {{ kind = "sqlite", path = "{}", readonly_required = true }}
            "#,
            policy.display(),
            db.display()
        ),
    )
    .expect("profile");
}

fn stdout(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}
