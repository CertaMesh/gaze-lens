use assert_cmd::Command;
use rusqlite::Connection;

#[test]
fn cli_error_path_sanitizes_pii_in_where_ast() {
    let temp = tempfile::tempdir().expect("tempdir");
    let db = temp.path().join("fixture.sqlite");
    let project = temp.path().join("project.toml");
    let manifest = temp.path().join("manifest.sqlite");
    let snapshots = temp.path().join("snapshots");
    seed_sqlite(&db);
    write_profile(&project, &db);

    let where_json = r#"[{"col":"alice@example.com","op":"eq","val":"secret@example.com"}]"#;
    let mut cmd = Command::cargo_bin("gaze-lens").expect("binary");
    let output = cmd
        .args([
            "--project-config",
            project.to_str().expect("project path"),
            "query",
            "--profile",
            "local",
            "--manifest",
            manifest.to_str().expect("manifest path"),
            "--snapshot-dir",
            snapshots.to_str().expect("snapshot dir"),
            "--table",
            "users",
            "--where-json",
            where_json,
        ])
        .output()
        .expect("query");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "stdout: {}", stdout(&output));
    assert!(!stderr.contains("alice@example.com"), "{stderr}");
    assert!(!stderr.contains("secret@example.com"), "{stderr}");
    assert!(stderr.contains("SourceError: source failed"), "{stderr}");
}

fn seed_sqlite(path: &std::path::Path) {
    let conn = Connection::open(path).expect("sqlite");
    conn.execute_batch("CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT);")
        .expect("seed");
}

fn write_profile(path: &std::path::Path, db: &std::path::Path) {
    std::fs::write(
        path,
        format!(
            r#"
            [[profiles]]
            name = "local"
            source = {{ kind = "sqlite", path = "{}", readonly_required = true }}
            "#,
            db.display()
        ),
    )
    .expect("profile");
}

fn stdout(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}
