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
fn test_new_accepts_user_at_host() {
    // Todo #504: the SSH log host may carry an explicit login user (`user@host`),
    // mirroring `--discover-ssh-host` and the runtime `validate_ssh_login_host`
    // path that actually builds the ssh argv. Construction must not reject it.
    let source = SshLogSource::new("p1", "ploi@94.237.89.225", "/var/log/x", caps())
        .expect("user@host should be accepted");
    assert!(
        source
            .tail_argv(1)
            .iter()
            .any(|a| a == "ploi@94.237.89.225"),
        "host must reach the ssh argv verbatim: {:?}",
        source.tail_argv(1)
    );
}

#[test]
fn test_split_and_cap_lines() {
    let lines = split_and_cap_lines(b"abcdef\nxy\n\nlast", 3);

    assert_eq!(lines, vec!["abc", "xy", "las"]);
}
