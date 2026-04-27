use assert_cmd::Command;
use rusqlite::Connection;

#[test]
fn replay_restores_whole_session_args() {
    let temp = tempfile::tempdir().expect("tempdir");
    let db = temp.path().join("fixture.sqlite");
    let project = temp.path().join("project.toml");
    let manifest = temp.path().join("manifest.sqlite");
    let snapshots = temp.path().join("snapshots");
    seed_sqlite(&db);
    write_profile(&project, &db);

    let mut query = Command::cargo_bin("gaze-lens").expect("binary");
    let query = query
        .args([
            "--project-config",
            project.to_str().expect("project path"),
            "query",
            "--profile",
            "local",
            "--manifest",
            manifest.to_str().expect("manifest path"),
            "--snapshot-dir",
            snapshots.to_str().expect("snapshot path"),
            "--table",
            "users",
            "--column",
            "email",
            "--limit",
            "1",
        ])
        .output()
        .expect("query");
    assert!(query.status.success(), "stderr: {}", stderr(&query));

    let session_id = Connection::open(&manifest)
        .expect("manifest")
        .query_row("SELECT lens_session_id FROM sessions LIMIT 1", [], |row| {
            row.get::<_, String>(0)
        })
        .expect("lens session");

    let mut replay = Command::cargo_bin("gaze-lens").expect("binary");
    let replay = replay
        .args([
            "replay",
            "--manifest",
            manifest.to_str().expect("manifest path"),
            &session_id,
        ])
        .output()
        .expect("replay");

    assert!(replay.status.success(), "stderr: {}", stderr(&replay));
    let stdout = stdout(&replay);
    assert!(stdout.contains(&session_id));
    assert!(stdout.contains("\\\"table\\\":\\\"users\\\""));
    assert!(stdout.contains("\\\"email\\\""));
}

#[test]
fn replay_rejects_call_id_selector_as_v1x_candidate() {
    let mut cmd = Command::cargo_bin("gaze-lens").expect("binary");
    let output = cmd
        .args([
            "replay",
            "--manifest",
            "/tmp/does-not-matter.sqlite",
            "--call-id",
            "01TEST",
            "01ARZ3NDEKTSV4RRFFQ69G5FAV",
        ])
        .output()
        .expect("replay");

    assert!(!output.status.success(), "stdout: {}", stdout(&output));
    assert!(stderr(&output).contains("not in v1; tracked as v1.x candidate"));
}

fn seed_sqlite(path: &std::path::Path) {
    let conn = Connection::open(path).expect("sqlite");
    conn.execute_batch(
        r#"
        CREATE TABLE users (email TEXT NOT NULL);
        INSERT INTO users (email) VALUES ('alice@example.com');
        "#,
    )
    .expect("seed");
}

fn write_profile(path: &std::path::Path, db: &std::path::Path) {
    let policy = path.with_file_name("policy.toml");
    std::fs::write(
        &policy,
        r#"
        [policy.database]

        [[policy.database.columns]]
        column = "email"
        class = "email"
        action = "tokenize"
        "#,
    )
    .expect("policy");
    std::fs::write(
        path,
        format!(
            r#"
            [[profiles]]
            name = "local"
            policy = "{}"
            schema_allowlist = ["users", "email"]
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
