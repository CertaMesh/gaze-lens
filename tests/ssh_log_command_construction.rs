use gaze_lens::source::ssh_tunnel::validate_ssh_path;

#[test]
fn test_validate_ssh_path_rejects_empty() {
    assert!(validate_ssh_path("").is_err());
}

#[test]
fn test_validate_ssh_path_rejects_metacharacters() {
    for ch in [';', '|', '&', '`', '$', '\n', '\r', '\0'] {
        let path = format!("/var/log/app{ch}.log");
        assert!(
            validate_ssh_path(&path).is_err(),
            "expected {path:?} to be rejected"
        );
    }
}

#[test]
fn test_validate_ssh_path_rejects_glob() {
    for ch in ['*', '?', '[', ']'] {
        let path = format!("/var/log/app{ch}.log");
        assert!(
            validate_ssh_path(&path).is_err(),
            "expected {path:?} to be rejected"
        );
    }
}

#[test]
fn test_validate_ssh_path_rejects_traversal() {
    assert!(validate_ssh_path("/var/log/../etc/passwd").is_err());
}

#[test]
fn test_validate_ssh_path_rejects_too_long() {
    let path = format!("/{}", "a".repeat(4096));
    assert!(validate_ssh_path(&path).is_err());
}

#[test]
fn test_validate_ssh_path_accepts_valid() {
    for path in [
        "/var/log/app.log",
        "/srv/myapp/logs/access.log",
        "/tmp/test_2026-04-26.log",
    ] {
        assert!(
            validate_ssh_path(path).is_ok(),
            "expected {path:?} to be accepted"
        );
    }
}

#[test]
fn test_validate_ssh_path_rejects_non_ascii() {
    assert!(validate_ssh_path("/var/log/日本語.log").is_err());
}
