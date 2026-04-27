use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use clap::Parser;
use gaze_lens::cli::serve::{ServeArgs, run, run_frontend_until_shutdown};
use gaze_lens::cli::{Cli, Cmd};
use gaze_lens::errors::LensError;
use gaze_lens::frontend::{Frontend, FrontendError, ShutdownToken};
use gaze_lens::session::Session;

struct ShutdownObservingFrontend {
    observed: Arc<AtomicBool>,
}

#[async_trait]
impl Frontend for ShutdownObservingFrontend {
    async fn serve(
        self,
        _session: Arc<Session>,
        shutdown: ShutdownToken,
    ) -> Result<(), FrontendError> {
        shutdown.cancelled().await;
        self.observed.store(true, Ordering::SeqCst);
        Ok(())
    }
}

#[test]
fn test_serve_is_available_as_a_subcommand() {
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
    let Cmd::Serve(args) = cli.cmd else {
        panic!("expected serve command");
    };
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

#[tokio::test]
async fn test_serve_shutdown_clean() {
    let temp = tempfile::tempdir().expect("tempdir");
    let session = Arc::new(
        Session::new(
            &policy(),
            &temp.path().join("manifest-token.sqlite"),
            &temp.path().join("snapshots-token"),
        )
        .expect("session"),
    );
    let observed = Arc::new(AtomicBool::new(false));
    run_frontend_until_shutdown(
        ShutdownObservingFrontend {
            observed: observed.clone(),
        },
        session,
        async {},
    )
    .await
    .expect("shutdown path");
    assert!(
        observed.load(Ordering::SeqCst),
        "shutdown signal path did not cancel token"
    );
}

fn policy() -> gaze::Policy {
    gaze::Policy {
        session: gaze::SessionPolicy {
            scope: gaze::SessionScope::Conversation,
            ttl_secs: None,
        },
        detectors: Vec::new(),
        dictionaries: Vec::new(),
        rules: Vec::new(),
        ner: None,
        rulepacks: gaze::RulepackPolicy {
            bundled: vec!["core".to_string()],
            paths: Vec::new(),
        },
        locale: None,
    }
}
