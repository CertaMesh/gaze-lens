use gaze_lens::cli::serve::{ServeArgs, run};

#[tokio::test]
async fn test_serve_installs_tracing_before_session_prepare_errors() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project_config = temp.path().join("profiles.toml");
    std::fs::write(&project_config, "[[profiles]\n").expect("malformed profile");

    let err = run(
        ServeArgs {
            profile: Vec::new(),
            manifest: temp.path().join("manifest.sqlite"),
            snapshot_dir: temp.path().join("snapshots"),
            print_discovery: false,
            log: Some("warn".to_string()),
        },
        Some(&project_config),
        Some(&empty_user_config(&temp)),
    )
    .await
    .expect_err("malformed profile should fail session preparation");

    let msg = err.to_string();
    assert!(msg.contains("line "), "{msg}");
    assert!(
        tracing::subscriber::set_global_default(tracing_subscriber::registry()).is_err(),
        "serve did not install a global tracing subscriber before preparing the session"
    );
}

fn empty_user_config(temp: &tempfile::TempDir) -> std::path::PathBuf {
    let path = temp.path().join("user.toml");
    std::fs::write(&path, "").expect("empty user profile config");
    path
}
