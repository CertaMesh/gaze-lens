use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use clap::Parser;
use gaze_lens::cli::serve::{
    ServeArgs, loaded_profiles_banner, prepare_session_for_test, run, run_frontend_until_shutdown,
};
use gaze_lens::cli::{Cli, Cmd};
use gaze_lens::errors::LensError;
use gaze_lens::frontend::mcp::McpFrontend;
use gaze_lens::frontend::{Frontend, FrontendError, ShutdownToken};
use gaze_lens::session::Session;
use rmcp::model::ErrorCode;

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
    assert_eq!(args.profile, vec!["prod".to_string()]);
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
async fn test_serve_mysql_profile_defers_password_until_first_call() {
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
            profile: vec!["prod".to_string()],
            manifest: temp.path().join("manifest.sqlite"),
            snapshot_dir: temp.path().join("snapshots"),
        },
        Some(&project_config),
        None,
    )
    .await
    .expect_err("stdio frontend exits in test harness");

    assert!(!matches!(err, LensError::ProfileEnvMissing { .. }));
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

#[test]
fn test_serve_rejects_malformed_policy_file_at_startup() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project_config = temp.path().join("profiles.toml");
    let policy = temp.path().join("policy.toml");
    std::fs::write(&policy, "not valid toml =").expect("policy");
    write_profiles(&project_config, &[("dev", &policy)]);

    let err = match prepare_session_for_test(serve_args(&temp, &[]), Some(&project_config), None) {
        Ok(_) => panic!("malformed policy must fail startup"),
        Err(err) => err,
    };
    let msg = err.to_string();
    assert!(
        msg.contains("failed to parse") || msg.contains("TOML"),
        "{msg}"
    );
}

#[test]
fn test_serve_rejects_malformed_profile_toml_with_position() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project_config = temp.path().join("profiles.toml");
    std::fs::write(
        &project_config,
        r#"
        [[profiles]
        name = "dev"
        "#,
    )
    .expect("profile");

    let err = match prepare_session_for_test(serve_args(&temp, &[]), Some(&project_config), None) {
        Ok(_) => panic!("malformed profile TOML must fail startup"),
        Err(err) => err,
    };
    let msg = err.to_string();
    assert!(msg.contains(&project_config.display().to_string()), "{msg}");
    assert!(msg.contains("line "), "{msg}");
    assert!(msg.contains("column "), "{msg}");
}

#[test]
fn test_serve_rejects_invalid_configured_profile_names() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project_config = temp.path().join("profiles.toml");
    std::fs::write(
        &project_config,
        r#"
        [[profiles]]
        name = "Bad"
        source = { kind = "sqlite", path = "/tmp/unused.sqlite", readonly_required = true }
        "#,
    )
    .expect("profile");

    let err = match prepare_session_for_test(serve_args(&temp, &[]), Some(&project_config), None) {
        Ok(_) => panic!("invalid configured name must fail startup"),
        Err(err) => err,
    };
    let msg = err.to_string();
    assert!(msg.contains("invalid profile name `Bad`"), "{msg}");
    assert!(msg.contains("^[a-z0-9][a-z0-9_-]{0,63}$"), "{msg}");
}

#[test]
fn test_serve_startup_banner_lists_loaded_profiles() {
    let names = vec!["dev".to_string(), "prod".to_string(), "staging".to_string()];
    assert_eq!(
        loaded_profiles_banner(&names),
        "gaze-lens serve: loaded profiles: [dev, prod, staging]"
    );
}

#[tokio::test]
async fn test_serve_restrict_list_single_profile_exposes_only_that_profile() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project_config = temp.path().join("profiles.toml");
    let policy = temp.path().join("policy.toml");
    std::fs::write(&policy, "[policy.database]\n").expect("policy");
    write_profiles(&project_config, &[("dev", &policy), ("prod", &policy)]);

    let prepared =
        prepare_session_for_test(serve_args(&temp, &["dev"]), Some(&project_config), None)
            .expect("prepared");
    assert_eq!(prepared.loaded_profiles, vec!["dev"]);

    let err = McpFrontend::with_session(prepared.session)
        .call_tool_result_for_test("query", query_args("prod"))
        .await
        .expect_err("prod restricted out");
    assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
    assert!(err.message.contains("loaded: [\"dev\"]"), "{}", err.message);
}

#[test]
fn test_serve_restrict_list_multiple_profiles_exposes_both() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project_config = temp.path().join("profiles.toml");
    let policy = temp.path().join("policy.toml");
    std::fs::write(&policy, "[policy.database]\n").expect("policy");
    write_profiles(
        &project_config,
        &[("dev", &policy), ("prod", &policy), ("staging", &policy)],
    );

    let prepared = prepare_session_for_test(
        serve_args(&temp, &["prod", "staging"]),
        Some(&project_config),
        None,
    )
    .expect("prepared");
    assert_eq!(prepared.loaded_profiles, vec!["prod", "staging"]);
}

#[test]
fn test_serve_without_restrict_list_exposes_all_profiles() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project_config = temp.path().join("profiles.toml");
    let policy = temp.path().join("policy.toml");
    std::fs::write(&policy, "[policy.database]\n").expect("policy");
    write_profiles(
        &project_config,
        &[("dev", &policy), ("prod", &policy), ("staging", &policy)],
    );

    let prepared = prepare_session_for_test(serve_args(&temp, &[]), Some(&project_config), None)
        .expect("prepared");
    assert_eq!(prepared.loaded_profiles, vec!["dev", "prod", "staging"]);
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

fn serve_args(temp: &tempfile::TempDir, profiles: &[&str]) -> ServeArgs {
    ServeArgs {
        profile: profiles.iter().map(|name| name.to_string()).collect(),
        manifest: temp.path().join("manifest.sqlite"),
        snapshot_dir: temp.path().join("snapshots"),
    }
}

fn write_profiles(path: &std::path::Path, profiles: &[(&str, &std::path::Path)]) {
    let mut toml = String::new();
    for (name, policy) in profiles {
        toml.push_str(&format!(
            r#"
            [[profiles]]
            name = "{name}"
            policy = "{}"
            source = {{ kind = "sqlite", path = "/tmp/unused.sqlite", readonly_required = true }}
            "#,
            policy.display()
        ));
    }
    std::fs::write(path, toml).expect("profiles");
}

fn query_args(profile: &str) -> serde_json::Value {
    serde_json::json!({
        "profile": profile,
        "table": "users",
        "columns": ["email"],
        "limit": 1
    })
}
