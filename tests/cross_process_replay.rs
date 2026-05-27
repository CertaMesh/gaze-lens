use std::process::Command;

#[test]
fn cross_process_replay_restores_seeded_canary() {
    let temp = tempfile::tempdir().expect("tempdir");
    let manifest = temp.path().join("manifest.sqlite");
    let snapshot_dir = temp.path().join("snapshots");

    let seed = Command::new(env!("CARGO"))
        .args([
            "run",
            "--quiet",
            "--example",
            "replay-fixture",
            "--",
            "seed",
            "--manifest",
        ])
        .arg(&manifest)
        .arg("--snapshot-dir")
        .arg(&snapshot_dir)
        .output()
        .expect("seed process");
    assert!(
        seed.status.success(),
        "seed failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&seed.stdout),
        String::from_utf8_lossy(&seed.stderr)
    );
    let stdout = String::from_utf8(seed.stdout).expect("seed stdout");
    let lens_session = stdout
        .lines()
        .find_map(|line| line.strip_prefix("SEEDED: "))
        .expect("seeded lens session")
        .trim()
        .to_string();

    let restore = Command::new(env!("CARGO"))
        .args([
            "run",
            "--quiet",
            "--example",
            "replay-fixture",
            "--",
            "restore",
            "--manifest",
        ])
        .arg(&manifest)
        .arg("--lens-session")
        .arg(&lens_session)
        .output()
        .expect("restore process");
    assert!(
        restore.status.success(),
        "restore failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&restore.stdout),
        String::from_utf8_lossy(&restore.stderr)
    );
    let stdout = String::from_utf8(restore.stdout).expect("restore stdout");
    assert!(stdout.contains("RESTORED: alice.replay@example.com"));
}
