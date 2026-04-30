#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;

use assert_cmd::Command;
use gaze_lens::cli::init::atomic::{
    assert_dir_0700_or_warn, atomic_write, create_dir_0700_if_missing, would_write,
};

#[test]
fn atomic_write_creates_0600_file_in_0700_parent() {
    let dir = tempfile::tempdir().unwrap();
    let parent = dir.path().join("nested/child");
    create_dir_0700_if_missing(&parent).expect("dir");
    let target = parent.join("profile.toml");
    atomic_write(&target, b"hello\n").expect("write");
    let dir_mode = std::fs::metadata(&parent).unwrap().permissions().mode() & 0o777;
    let file_mode = std::fs::metadata(&target).unwrap().permissions().mode() & 0o777;
    assert_eq!(dir_mode, 0o700);
    assert_eq!(file_mode, 0o600);
    assert_eq!(std::fs::read(&target).unwrap(), b"hello\n");
}

#[test]
fn atomic_write_leaves_no_tmp_orphans_on_failure() {
    let dir = tempfile::tempdir().unwrap();
    let parent = dir.path().join("p");
    create_dir_0700_if_missing(&parent).unwrap();
    let target = parent.join("x.toml");
    // Make parent read-only to induce a failure mid-write.
    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o500)).unwrap();
    let _ = atomic_write(&target, b"x");
    // Restore so we can read the dir.
    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o700)).unwrap();
    // Non-recursive read of parent — temp files would land here.
    for entry in std::fs::read_dir(&parent).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().to_string_lossy().into_owned();
        assert!(
            !name.contains(".tmp."),
            "orphan: {}",
            entry.path().display()
        );
    }
}

#[test]
fn existing_third_party_dir_mode_not_modified() {
    let dir = tempfile::tempdir().unwrap();
    let third_party = dir.path().join("dot-codex");
    std::fs::create_dir(&third_party).unwrap();
    std::fs::set_permissions(&third_party, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_dir_0700_or_warn(&third_party).expect("must not error");
    let mode = std::fs::metadata(&third_party)
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o755, "third-party dir mode must not be modified");
}

#[test]
fn would_write_returns_false_when_bytes_match() {
    let dir = tempfile::tempdir().unwrap();
    let parent = dir.path().join("d");
    create_dir_0700_if_missing(&parent).unwrap();
    let target = parent.join("a.toml");
    atomic_write(&target, b"same").unwrap();
    assert!(!would_write(&target, b"same"));
    assert!(would_write(&target, b"different"));
}

#[test]
fn create_dir_0700_if_missing_idempotent_when_already_0700() {
    let dir = tempfile::tempdir().unwrap();
    let lens_owned = dir.path().join("lens-owned");
    create_dir_0700_if_missing(&lens_owned).unwrap();
    create_dir_0700_if_missing(&lens_owned).unwrap();
    let mode = std::fs::metadata(&lens_owned).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o700);
}

#[test]
fn malformed_mcp_config_fails_before_profile_write() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let project = dir.path().join("project");
    std::fs::create_dir(&home).unwrap();
    std::fs::create_dir(&project).unwrap();
    let profile = project.join(".gaze-lens.toml");
    std::fs::write(project.join(".mcp.json"), "{ broken json").unwrap();

    let output = Command::cargo_bin("gaze-lens")
        .unwrap()
        .env("HOME", &home)
        .current_dir(&project)
        .args([
            "--project-config",
            profile.to_str().unwrap(),
            "init",
            "--non-interactive",
            "--profile",
            "p",
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

    assert!(!output.status.success());
    assert!(
        !profile.exists(),
        "profile must not be written after MCP validation failure"
    );
}

#[test]
fn malformed_agents_markers_fail_before_profile_write() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let project = dir.path().join("project");
    std::fs::create_dir(&home).unwrap();
    std::fs::create_dir(&project).unwrap();
    let profile = project.join(".gaze-lens.toml");
    std::fs::write(
        project.join("AGENTS.md"),
        "<!-- gaze-lens:init:start -->\nbody\n",
    )
    .unwrap();

    let output = Command::cargo_bin("gaze-lens")
        .unwrap()
        .env("HOME", &home)
        .current_dir(&project)
        .args([
            "--project-config",
            profile.to_str().unwrap(),
            "init",
            "--non-interactive",
            "--profile",
            "p",
            "--source-kind",
            "sqlite",
            "--source-path",
            "/tmp/x.db",
            "--scope",
            "project",
            "--no-mcp-config",
            "--write-all",
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(
        !profile.exists(),
        "profile must not be written after AGENTS marker validation failure"
    );
}

#[test]
fn mcp_same_profile_command_collision_fails_before_profile_write() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let project = dir.path().join("project");
    std::fs::create_dir(&home).unwrap();
    std::fs::create_dir(&project).unwrap();
    let profile = project.join(".gaze-lens.toml");
    std::fs::write(
        project.join(".mcp.json"),
        r#"{"mcpServers":{"gaze-lens":{"command":"/opt/gaze-lens","args":["serve","--profile","p"]}}}"#,
    )
    .unwrap();

    let output = Command::cargo_bin("gaze-lens")
        .unwrap()
        .env("HOME", &home)
        .current_dir(&project)
        .args([
            "--project-config",
            profile.to_str().unwrap(),
            "init",
            "--non-interactive",
            "--profile",
            "p",
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

    assert!(!output.status.success());
    assert!(
        !profile.exists(),
        "profile must not be written after MCP entry collision"
    );
}
