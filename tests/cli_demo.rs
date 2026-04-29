use std::os::unix::fs::PermissionsExt;

use assert_cmd::Command;
use rusqlite::Connection;

use gaze_lens::cli::demo;

#[tokio::test]
async fn demo_routes_through_session_dispatch_tool() {
    let temp = tempfile::tempdir().expect("tempdir");
    let outcome = demo::run_with_workdir(temp.path()).await.expect("demo run");

    // D4 chokepoint enforcement: any demo dispatch MUST land a `calls` row in the
    // tempdir manifest with tool_name='query' AND status='ok'. If a future patch
    // shortcuts dispatch_tool to skip the pipeline, no row is written and this
    // test fails — the chokepoint stays guaranteed.
    let conn = Connection::open(&outcome.manifest_path).expect("open temp manifest");
    let count: u32 = conn
        .query_row(
            "SELECT COUNT(*) FROM calls WHERE tool_name = 'query' AND status = 'ok'",
            [],
            |row| row.get(0),
        )
        .expect("calls count");
    assert!(
        count >= 1,
        "expected ≥1 ok query call in demo manifest, got {count}"
    );

    // Tokenized section: contains tokens (Email_*, ssn_*, phone_*) and never
    // contains the seeded raw email — demonstrates redaction worked.
    assert!(
        outcome.tokenized_section.contains(":Email_"),
        "tokenized section missing email token: {}",
        outcome.tokenized_section
    );
    assert!(
        !outcome.tokenized_section.contains("alice@example.com"),
        "tokenized section leaked raw email: {}",
        outcome.tokenized_section
    );
    assert!(
        !outcome.tokenized_section.contains("123-45-6789"),
        "tokenized section leaked raw SSN: {}",
        outcome.tokenized_section
    );

    // Restored section: matches what `gaze-lens replay` produces on a real
    // session — original PII visible.
    assert!(
        outcome.restored_section.contains("alice@example.com"),
        "restored section missing seeded email: {}",
        outcome.restored_section
    );
    assert!(
        outcome.restored_section.contains("bob@beta.io"),
        "restored section missing seeded email: {}",
        outcome.restored_section
    );
    assert!(
        outcome.restored_section.contains("123-45-6789"),
        "restored section missing seeded SSN: {}",
        outcome.restored_section
    );
    assert!(
        outcome.restored_section.contains("555-123-4567"),
        "restored section missing seeded phone: {}",
        outcome.restored_section
    );

    // Snapshot file privacy: 0600 file under 0700 dir before tempdir drop.
    let snap_mode = std::fs::metadata(&outcome.snapshot_path)
        .expect("snapshot stat")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(
        snap_mode, 0o600,
        "snapshot file mode should be 0600, got 0o{snap_mode:o}"
    );
    let dir_mode = std::fs::metadata(&outcome.snapshot_dir)
        .expect("snapshot dir stat")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(
        dir_mode, 0o700,
        "snapshot dir mode should be 0700, got 0o{dir_mode:o}"
    );
}

#[test]
fn demo_writes_no_persistent_state() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fake_home = temp.path();

    let mut cmd = Command::cargo_bin("gaze-lens").expect("binary");
    let output = cmd
        .arg("demo")
        .env("HOME", fake_home)
        .output()
        .expect("run demo");

    assert!(
        output.status.success(),
        "demo did not exit 0; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Tokenized output"),
        "missing tokenized section header in stdout"
    );
    assert!(
        stdout.contains("Restored output"),
        "missing restored section header in stdout"
    );

    // Demo must NOT pollute persistent locations under $HOME/.gaze-lens.
    let persistent_dir = fake_home.join(".gaze-lens");
    assert!(
        !persistent_dir.exists(),
        "demo touched persistent state at {}",
        persistent_dir.display()
    );
}

#[tokio::test]
async fn demo_handles_clean_temp_state() {
    // Two back-to-back invocations on independent tempdirs both succeed;
    // confirms the demo never relies on (or pollutes) shared state and is safe
    // for the README's first-run promise even on a brand-new machine.
    let first_temp = tempfile::tempdir().expect("first tempdir");
    let first = demo::run_with_workdir(first_temp.path())
        .await
        .expect("first demo run");
    assert!(first.tokenized_section.contains(":Email_"));
    assert!(first.restored_section.contains("alice@example.com"));

    let second_temp = tempfile::tempdir().expect("second tempdir");
    let second = demo::run_with_workdir(second_temp.path())
        .await
        .expect("second demo run");
    assert!(second.tokenized_section.contains(":Email_"));
    assert!(second.restored_section.contains("alice@example.com"));

    // Independent gaze sessions → independent ulids.
    assert_ne!(
        first.lens_session_id, second.lens_session_id,
        "two demo runs should produce distinct session ids"
    );
}
