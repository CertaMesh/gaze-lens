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
