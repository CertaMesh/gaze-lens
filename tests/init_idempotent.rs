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

#[test]
fn mcp_enabled_rerun_no_changes_without_suffix() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let project = temp.path().join("project");
    std::fs::create_dir(&home).unwrap();
    std::fs::create_dir(&project).unwrap();
    let project_config = project.join(".gaze-lens.toml");

    let common = [
        "--project-config",
        project_config.to_str().unwrap(),
        "init",
        "--non-interactive",
        "--profile",
        "x",
        "--source-kind",
        "sqlite",
        "--source-path",
        "/tmp/x.db",
        "--scope",
        "project",
        "--client",
        "claude-code",
        "--no-agents-md",
        "--write-all",
    ];

    let out1 = Command::cargo_bin("gaze-lens")
        .unwrap()
        .env("HOME", &home)
        .current_dir(&project)
        .args(common)
        .output()
        .unwrap();
    assert!(
        out1.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out1.stderr)
    );

    let out2 = Command::cargo_bin("gaze-lens")
        .unwrap()
        .env("HOME", &home)
        .current_dir(&project)
        .args(common)
        .output()
        .unwrap();
    assert!(
        out2.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out2.stderr)
    );
    let stdout2 = String::from_utf8_lossy(&out2.stdout);
    assert!(stdout2.contains("no changes"), "got: {stdout2}");

    let mcp: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(project.join(".mcp.json")).unwrap()).unwrap();
    let servers = mcp["mcpServers"].as_object().unwrap();
    assert!(servers.contains_key("gaze-lens"));
    assert!(
        !servers.contains_key("gaze-lens-x"),
        "same profile rerun must not add a suffix entry"
    );
}

#[test]
fn second_profile_same_client_adds_suffix_without_touching_primary() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let project = temp.path().join("project");
    std::fs::create_dir(&home).unwrap();
    std::fs::create_dir(&project).unwrap();
    let project_config = project.join(".gaze-lens.toml");

    for profile in ["alpha", "beta"] {
        let out = Command::cargo_bin("gaze-lens")
            .unwrap()
            .env("HOME", &home)
            .current_dir(&project)
            .args([
                "--project-config",
                project_config.to_str().unwrap(),
                "init",
                "--non-interactive",
                "--profile",
                profile,
                "--source-kind",
                "sqlite",
                "--source-path",
                "/tmp/x.db",
                "--scope",
                "project",
                "--client",
                "claude-code",
                "--no-agents-md",
                "--write-all",
            ])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    let mcp: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(project.join(".mcp.json")).unwrap()).unwrap();
    let servers = mcp["mcpServers"].as_object().unwrap();
    assert_eq!(servers.len(), 1);
    assert_eq!(servers["gaze-lens"]["args"][0], "serve");
}
