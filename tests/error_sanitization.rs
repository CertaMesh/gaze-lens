use std::path::PathBuf;

use gaze_lens::errors::{LensError, format_cli_error, sanitize_error};

fn assert_no_canary_leak(output: &str) {
    for leaked in [
        "alice",
        "@example",
        "SELECT",
        "users",
        "/tmp/gaze-lens",
        "password",
        "sudo",
    ] {
        assert!(
            !output.contains(leaked),
            "sanitized error leaked {leaked:?}: {output}"
        );
    }
}

#[test]
fn generic_source_error_drops_pii_shaped_detail() {
    let err = LensError::SourceError {
        source_name: "fake".to_string(),
        detail: "user alice@example.com failed".to_string(),
        sql: None,
        stderr: None,
    };

    let sanitized = sanitize_error(&err);
    assert_eq!(sanitized, "SourceError: source failed");
    assert_no_canary_leak(&sanitized);
}

#[test]
fn sql_source_error_drops_sql_text() {
    let err = LensError::SourceError {
        source_name: "fake-db".to_string(),
        detail: "query failed".to_string(),
        sql: Some("SELECT * FROM users WHERE email = 'alice@example.com'".to_string()),
        stderr: None,
    };

    let sanitized = sanitize_error(&err);
    assert_eq!(sanitized, "SourceError: source failed");
    assert_no_canary_leak(&sanitized);
}

#[test]
fn ssh_source_error_drops_stderr() {
    let err = LensError::SourceError {
        source_name: "fake-ssh".to_string(),
        detail: "remote command failed".to_string(),
        sql: None,
        stderr: Some("sudo: alice's password incorrect".to_string()),
    };

    let sanitized = sanitize_error(&err);
    assert_eq!(sanitized, "SourceError: source failed");
    assert_no_canary_leak(&sanitized);
}

#[test]
fn verbose_source_error_keeps_diagnostics_but_scrubs_credentials() {
    let err = LensError::SourceError {
        source_name: "mysql-prod".to_string(),
        detail: "mysql://readonly:hunter2@db.example.test/app: Access denied for user 'readonly'@'10.0.0.1' (password=secret-value); token='quoted secret value'; Authorization: Bearer bearer-value".to_string(),
        sql: Some("SELECT secret FROM credentials".to_string()),
        stderr: Some("password=stderr-secret".to_string()),
    };

    let verbose = format_cli_error(&err, true);
    assert!(
        verbose.contains("source error from mysql-prod"),
        "{verbose}"
    );
    assert!(verbose.contains("Access denied"), "{verbose}");
    assert!(verbose.contains("[REDACTED]"), "{verbose}");
    for secret in [
        "readonly",
        "hunter2",
        "secret-value",
        "quoted secret value",
        "bearer-value",
        "stderr-secret",
        "SELECT secret",
    ] {
        assert!(
            !verbose.contains(secret),
            "verbose error leaked {secret:?}: {verbose}"
        );
    }
}

#[test]
fn verbose_mode_does_not_expand_non_source_errors() {
    let err = LensError::ManifestBeginFailed {
        call_id: "call-secret".to_string(),
        detail: "password=hunter2".to_string(),
        path: Some(PathBuf::from("/tmp/secret")),
    };

    assert_eq!(
        format_cli_error(&err, true),
        "ManifestBeginFailed: manifest begin failed"
    );
}

#[test]
fn verbose_source_errors_distinguish_failure_causes() {
    let format = |detail: &str| {
        format_cli_error(
            &LensError::SourceError {
                source_name: "mysql-prod".to_string(),
                detail: detail.to_string(),
                sql: None,
                stderr: None,
            },
            true,
        )
    };

    let connection = format("connection refused");
    let table = format("unknown table `missing`");
    let predicate = format("operator `Eq` requires a value");
    assert_ne!(connection, table);
    assert_ne!(table, predicate);
    assert!(connection.contains("connection refused"));
    assert!(table.contains("unknown table"));
    assert!(predicate.contains("requires a value"));
}

#[test]
fn manifest_begin_error_drops_snapshot_path() {
    let err = LensError::ManifestBeginFailed {
        call_id: "call-1".to_string(),
        detail: "failed near alice@example.com".to_string(),
        path: Some(PathBuf::from(
            "/tmp/gaze-lens/snapshots/alice@example.com.snap",
        )),
    };

    let sanitized = sanitize_error(&err);
    assert_eq!(sanitized, "ManifestBeginFailed: manifest begin failed");
    assert_no_canary_leak(&sanitized);
}

#[test]
fn manifest_finish_error_drops_snapshot_path() {
    let err = LensError::ManifestFinishFailed {
        call_id: "call-1".to_string(),
        detail: "failed near alice@example.com".to_string(),
        path: Some(PathBuf::from(
            "/tmp/gaze-lens/snapshots/alice@example.com.snap",
        )),
    };

    let sanitized = sanitize_error(&err);
    assert_eq!(sanitized, "ManifestFinishFailed: manifest finish failed");
    assert_no_canary_leak(&sanitized);
}
