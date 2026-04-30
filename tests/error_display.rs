use assert_cmd::Command;
use gaze_lens::errors::{LensError, sanitize_error};
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

#[test]
fn keyring_error_display_does_not_leak_secret_value() {
    let err = LensError::SecretKeyringMissing {
        service: "gaze-lens".into(),
        account: "prod".into(),
    };

    let s = err.to_string();
    assert!(s.contains("gaze-lens") && s.contains("prod"), "{s}");
    assert!(!s.contains("hunter2"), "{s}");
}

#[test]
fn keyring_error_sanitize_strips_platform_detail() {
    let err = LensError::SecretBackendUnavailable {
        backend: "platform".into(),
        detail: "hunter2-platform-detail".into(),
    };

    let sanitized = sanitize_error(&err);
    assert!(
        sanitized.contains("SecretBackendUnavailable"),
        "{sanitized}"
    );
    assert!(
        !sanitized.contains("hunter2-platform-detail"),
        "{sanitized}"
    );
}

#[test]
fn secret_keyring_denied_display_does_not_leak() {
    let err = LensError::SecretKeyringDenied {
        service: "gaze-lens".into(),
        account: "prod".into(),
    };

    let s = err.to_string();
    assert!(s.contains("gaze-lens") && s.contains("prod"), "{s}");
    assert!(!s.contains("hunter2"), "{s}");
    let sanitized = sanitize_error(&err);
    assert!(sanitized.contains("SecretKeyringDenied"), "{sanitized}");
}

#[test]
fn secret_backend_unavailable_sanitized_display_does_not_leak() {
    let err = LensError::SecretBackendUnavailable {
        backend: "secret-service".into(),
        detail: "hunter2-platform-detail".into(),
    };

    let sanitized = sanitize_error(&err);
    assert!(
        !sanitized.contains("hunter2-platform-detail"),
        "{sanitized}"
    );
    assert!(
        sanitized.contains("SecretBackendUnavailable"),
        "{sanitized}"
    );
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
