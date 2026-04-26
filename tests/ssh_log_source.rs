use std::time::Duration;

use gaze_lens::source::log::ssh_log::{SshLogCaps, SshLogSource, split_and_cap_lines};

fn caps() -> SshLogCaps {
    SshLogCaps {
        line_bytes: 8 * 1024,
        bytes: 1024 * 1024,
        timeout: Duration::from_secs(30),
    }
}

#[test]
fn test_new_validates_host() {
    let err = SshLogSource::new("p1", "-bad", "/var/log/x", caps())
        .expect_err("dash-prefixed host should fail");
    assert!(err.to_string().contains("invalid ssh host"));
}

#[test]
fn test_new_validates_path() {
    let err = SshLogSource::new("p1", "good", "; rm -rf /", caps())
        .expect_err("metacharacter path should fail");
    assert!(err.to_string().contains("invalid ssh path"));
}

#[test]
fn test_split_and_cap_lines() {
    let lines = split_and_cap_lines(b"abcdef\nxy\n\nlast", 3);

    assert_eq!(lines, vec!["abc", "xy", "las"]);
}
