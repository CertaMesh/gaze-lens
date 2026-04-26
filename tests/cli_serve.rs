use clap::Parser;
use gaze_lens::cli::serve::{ServeArgs, run};
use gaze_lens::cli::{Cli, Cmd};
use gaze_lens::errors::LensError;

#[test]
fn test_serve_is_the_only_subcommand() {
    let cli = Cli::parse_from(["gaze-lens", "serve"]);
    assert!(matches!(cli.cmd, Cmd::Serve(_)));
}

#[test]
fn test_serve_parses_profile_and_paths() {
    let cli = Cli::parse_from([
        "gaze-lens",
        "--project-config",
        "project.toml",
        "--user-config",
        "user.toml",
        "serve",
        "--profile",
        "prod",
        "--manifest",
        "/tmp/manifest.sqlite",
        "--snapshot-dir",
        "/tmp/snapshots",
    ]);
    let Cmd::Serve(args) = cli.cmd;
    assert_eq!(args.profile, "prod");
    assert_eq!(
        cli.project_config.as_deref(),
        Some(std::path::Path::new("project.toml"))
    );
    assert_eq!(
        cli.user_config.as_deref(),
        Some(std::path::Path::new("user.toml"))
    );
}

#[tokio::test]
async fn test_serve_mysql_profile_resolves_password_before_frontend() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project_config = temp.path().join("project.toml");
    let missing_env = format!("GAZE_LENS_TEST_MISSING_{}", ulid::Ulid::new());
    std::fs::write(
        &project_config,
        format!(
            r#"
[[profiles]]
name = "prod"
schema_allowlist = ["id"]

[profiles.source]
kind = "mysql"
host = "127.0.0.1"
port = 3306
database = "app"
username = "readonly"
password_env = "{missing_env}"
readonly_required = true
"#
        ),
    )
    .expect("write profile");

    let err = run(
        ServeArgs {
            profile: "prod".to_string(),
            manifest: temp.path().join("manifest.sqlite"),
            snapshot_dir: temp.path().join("snapshots"),
        },
        Some(&project_config),
        None,
    )
    .await
    .expect_err("missing password env");

    assert!(matches!(err, LensError::ProfileEnvMissing { env } if env == missing_env));
}

#[cfg(unix)]
#[tokio::test]
async fn test_serve_shutdown_clean() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project_config = temp.path().join("project.toml");
    std::fs::write(
        &project_config,
        r#"
[[profiles]]
name = "logs"

[profiles.source]
kind = "ssh_log"
host = "example.test"
path = "/var/log/app.log"
"#,
    )
    .expect("write profile");

    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_gaze-lens"))
        .arg("--project-config")
        .arg(&project_config)
        .arg("--user-config")
        .arg(temp.path().join("missing-user.toml"))
        .arg("serve")
        .arg("--profile")
        .arg("logs")
        .arg("--manifest")
        .arg(temp.path().join("manifest.sqlite"))
        .arg("--snapshot-dir")
        .arg(temp.path().join("snapshots"))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn serve");

    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    let kill_status = std::process::Command::new("kill")
        .arg("-TERM")
        .arg(child.id().to_string())
        .status()
        .expect("send sigterm");
    assert!(kill_status.success(), "kill failed with {kill_status}");

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.try_wait().expect("try_wait") {
            break status;
        }
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            panic!("serve did not exit after SIGTERM");
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    };

    assert!(status.success(), "serve exited with {status}");
}
