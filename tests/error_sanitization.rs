use std::path::PathBuf;

use gaze_lens::errors::{sanitize_error, LensError};

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
fn manifest_begin_error_drops_snapshot_path() {
    let err = LensError::ManifestBeginFailed {
        call_id: "call-1".to_string(),
        detail: "failed near alice@example.com".to_string(),
        path: Some(PathBuf::from("/tmp/gaze-lens/snapshots/alice@example.com.snap")),
    };

    let sanitized = sanitize_error(&err);
    assert_eq!(
        sanitized,
        "ManifestBeginFailed: manifest begin failed"
    );
    assert_no_canary_leak(&sanitized);
}

#[test]
fn manifest_finish_error_drops_snapshot_path() {
    let err = LensError::ManifestFinishFailed {
        call_id: "call-1".to_string(),
        detail: "failed near alice@example.com".to_string(),
        path: Some(PathBuf::from("/tmp/gaze-lens/snapshots/alice@example.com.snap")),
    };

    let sanitized = sanitize_error(&err);
    assert_eq!(
        sanitized,
        "ManifestFinishFailed: manifest finish failed"
    );
    assert_no_canary_leak(&sanitized);
}
