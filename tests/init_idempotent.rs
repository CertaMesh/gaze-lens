//! AC #5 + AC #9: re-run identical inputs ⇒ "no changes" + perms verified.
//!
//! MS2 (rev 3): also asserts ~/.gaze-lens/ is mode 0o700 and profiles.toml
//! is mode 0o600 after the first run, so AC #9 has end-to-end coverage in
//! both directions (this positive case + tests/init_atomic.rs negative case
//! `existing_third_party_dir_mode_not_modified`).

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;

use assert_cmd::Command;

#[test]
fn rerun_no_changes() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    std::fs::create_dir(&home).unwrap();
    let user_dot_gaze_lens = home.join(".gaze-lens");

    let common = [
        "init",
        "--non-interactive",
        "--profile",
        "x",
        "--source-kind",
        "sqlite",
        "--source-path",
        "/tmp/x.db",
        "--scope",
        "user",
        "--no-mcp-config",
        "--no-agents-md",
        "--write-all",
    ];

    // First run: create profile + dir.
    let out1 = Command::cargo_bin("gaze-lens")
        .unwrap()
        .env("HOME", &home)
        .args(common)
        .output()
        .unwrap();
    assert!(
        out1.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out1.stderr)
    );

    // MS2: positive 0o700 dir + 0o600 file end-to-end (AC #9 second half).
    let dir_mode = std::fs::metadata(&user_dot_gaze_lens)
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(
        dir_mode, 0o700,
        "AC #9: lens-owned ~/.gaze-lens must be 0o700 after init"
    );
    let profile_file = user_dot_gaze_lens.join("profiles.toml");
    assert!(profile_file.exists(), "user-scope profile written");
    let file_mode = std::fs::metadata(&profile_file)
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(file_mode, 0o600, "AC #9: profile file must be 0o600");

    // Second run: same inputs ⇒ "no changes".
    let out2 = Command::cargo_bin("gaze-lens")
        .unwrap()
        .env("HOME", &home)
        .args(common)
        .output()
        .unwrap();
    assert!(out2.status.success());
    let stdout2 = String::from_utf8_lossy(&out2.stdout);
    assert!(stdout2.contains("no changes"), "got: {stdout2}");
}
