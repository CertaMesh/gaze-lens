//! AC #3: `--non-interactive` sqlite + `--smoke-check` runs in-process check.
//!
//! Default test runs do NOT pass `--smoke-check`, so the smoke phase is opt-in
//! and never fires unless explicitly requested (directive 17 — avoids
//! parallel-test races on `std::env::set_var`).

use assert_cmd::Command;

#[test]
fn non_interactive_sqlite_init_then_smoke_check_succeeds() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let db = temp.path().join("lens.db");
    // Touch a real (empty) sqlite file so check::run can connect.
    std::fs::write(&db, b"").unwrap();

    let out = Command::cargo_bin("gaze-lens")
        .unwrap()
        .env("HOME", &home)
        .args([
            "init",
            "--non-interactive",
            "--profile",
            "x",
            "--source-kind",
            "sqlite",
            "--source-path",
            db.to_str().unwrap(),
            "--scope",
            "user",
            "--no-mcp-config",
            "--no-agents-md",
            "--write-all",
            "--smoke-check",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("profile: ok"),
        "smoke-check should have run check::run; stdout: {stdout}"
    );
}

#[test]
fn production_sqlite_init_smoke_check_defers_model_and_succeeds() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let repo = temp.path().join("repo");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&repo).unwrap();
    let db = temp.path().join("lens.db");
    std::fs::write(&db, b"").unwrap();
    let model_dir = temp.path().join("models").join("kiji");

    let out = Command::cargo_bin("gaze-lens")
        .unwrap()
        .current_dir(&repo)
        .env("HOME", &home)
        .args([
            "init",
            "--non-interactive",
            "--profile",
            "prod",
            "--source-kind",
            "sqlite",
            "--source-path",
            db.to_str().unwrap(),
            "--scope",
            "project",
            "--production",
            "--model-dir",
            model_dir.to_str().unwrap(),
            "--no-mcp-config",
            "--no-agents-md",
            "--write-all",
            "--smoke-check",
        ])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("model: deferred"), "{stdout}");
    assert!(stdout.contains("source: ok"), "{stdout}");
    assert!(!stdout.contains("pipeline: ok"), "{stdout}");
}
