use clap::Parser;
use gaze_lens::cli::{Cli, Cmd};

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
