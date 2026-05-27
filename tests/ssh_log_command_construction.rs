use std::time::Duration;

use gaze_lens::source::log::ssh_log::{HARD_CAP_LINES, SshLogCaps, SshLogSource, tail_argv};
use gaze_lens::source::ssh_tunnel::validate_ssh_path;

fn caps() -> SshLogCaps {
    SshLogCaps {
        line_bytes: 8 * 1024,
        bytes: 1024 * 1024,
        timeout: Duration::from_secs(30),
    }
}

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

#[test]
fn test_tail_argv_double_dash_present() {
    let source =
        SshLogSource::new("p1", "app-prod", "/var/log/app.log", caps()).expect("valid source");

    assert_eq!(
        source.tail_argv(500),
        vec![
            "ssh",
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=10",
            "--",
            "app-prod",
            "tail",
            "-n",
            "500",
            "--",
            "/var/log/app.log"
        ]
    );
}

#[test]
fn test_remote_command_starts_with_tail_keyword() {
    let argv = tail_argv("ok-host", "/var/log/x.log", 100);
    // After ssh's "--" + host, the next argv element is the remote command's program name.
    // Must be "tail", not "--" (which would make the remote shell execute `--`).
    let host_idx = argv
        .iter()
        .position(|arg| arg == "ok-host")
        .expect("host in argv");
    assert_eq!(
        argv[host_idx + 1],
        "tail",
        "argv[host+1] must be 'tail' (the remote command), not '{}' - otherwise remote shell receives `-- tail -n N -- path` and fails",
        argv[host_idx + 1]
    );
}

#[test]
fn test_tail_no_shell_string_interpolation() {
    assert!(SshLogSource::new("p1", "host; rm", "/var/log/app.log", caps()).is_err());
    assert!(SshLogSource::new("p1", "host", "/var/log/app;rm.log", caps()).is_err());
}

#[test]
fn test_grep_uses_local_regex() {
    let argv = tail_argv("app-prod", "/var/log/app.log", 10_000);
    assert_eq!(argv[0], "ssh");
    assert_eq!(argv[7], "tail");
    assert!(!argv.iter().any(|arg| arg == "grep"));
    assert!(!argv.iter().any(|arg| arg == "awk"));
    assert!(!argv.iter().any(|arg| arg == "sed"));
    assert!(!argv.iter().any(|arg| arg == "alice@example.com"));
}

#[test]
fn test_lines_capped_fits_u32() {
    let source =
        SshLogSource::new("p1", "app-prod", "/var/log/app.log", caps()).expect("valid source");
    let argv = source.tail_argv(usize::MAX);

    assert_eq!(argv[9], HARD_CAP_LINES.to_string());
    let capped = argv[9].parse::<u32>().expect("u32 lines cap");
    assert_eq!(capped, HARD_CAP_LINES as u32);
}
