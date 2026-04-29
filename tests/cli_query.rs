use assert_cmd::Command;
use rusqlite::Connection;

#[test]
fn query_routes_through_session_and_preserves_null_vs_empty() {
    let temp = tempfile::tempdir().expect("tempdir");
    let db = temp.path().join("fixture.sqlite");
    let project = temp.path().join("project.toml");
    let manifest = temp.path().join("manifest.sqlite");
    let snapshots = temp.path().join("snapshots");
    seed_sqlite(&db);
    write_profile(&project, &db);

    let mut cmd = Command::cargo_bin("gaze-lens").expect("binary");
    let output = cmd
        .args([
            "--project-config",
            project.to_str().expect("project path"),
            "--user-config",
            temp.path()
                .join("missing.toml")
                .to_str()
                .expect("user path"),
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
            "--column",
            "nickname",
            "--column",
            "empty_text",
            "--limit",
            "1",
        ])
        .output()
        .expect("run query");

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let stdout = stdout(&output);
    assert!(!stdout.contains("alice@example.com"));
    assert!(stdout.contains("Email_1"));
    assert!(stdout.contains("\"nickname\":\"Null\""));
    assert!(stdout.contains("\"empty_text\":\"\""));
    assert!(manifest.exists());
    assert!(snapshots.read_dir().expect("snapshots").next().is_some());

    let conn = Connection::open(manifest).expect("manifest");
    let calls: u32 = conn
        .query_row(
            "SELECT COUNT(*) FROM calls WHERE tool_name = 'query'",
            [],
            |row| row.get(0),
        )
        .expect("call count");
    assert_eq!(calls, 1);
}

#[test]
fn query_rejects_unknown_table() {
    let temp = tempfile::tempdir().expect("tempdir");
    let db = temp.path().join("fixture.sqlite");
    let project = temp.path().join("project.toml");
    seed_sqlite(&db);
    write_profile(&project, &db);

    let mut cmd = Command::cargo_bin("gaze-lens").expect("binary");
    let output = cmd
        .args([
            "--project-config",
            project.to_str().expect("project path"),
            "query",
            "--profile",
            "local",
            "--manifest",
            temp.path()
                .join("manifest.sqlite")
                .to_str()
                .expect("manifest"),
            "--snapshot-dir",
            temp.path().join("snapshots").to_str().expect("snapshots"),
            "--table",
            "missing",
        ])
        .output()
        .expect("run query");

    assert!(!output.status.success(), "stdout: {}", stdout(&output));
    assert!(stderr(&output).contains("SourceError: source failed"));
    assert!(!stderr(&output).contains("missing"));
}

fn seed_sqlite(path: &std::path::Path) {
    let conn = Connection::open(path).expect("sqlite");
    conn.execute_batch(
        r#"
        CREATE TABLE users (
            id INTEGER PRIMARY KEY,
            email TEXT NOT NULL,
            nickname TEXT NULL,
            empty_text TEXT NOT NULL
        );
        INSERT INTO users (email, nickname, empty_text)
        VALUES ('alice@example.com', NULL, '');
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
            schema_allowlist = ["users", "email", "nickname", "empty_text"]
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
