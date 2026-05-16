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
use rusqlite::Connection;

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
        Some(&empty_user_config(&temp)),
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

    let err = match prepare_session_for_test(
        serve_args(&temp, &[]),
        Some(&project_config),
        Some(&empty_user_config(&temp)),
    ) {
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

    let err = match prepare_session_for_test(
        serve_args(&temp, &[]),
        Some(&project_config),
        Some(&empty_user_config(&temp)),
    ) {
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

    let err = match prepare_session_for_test(
        serve_args(&temp, &[]),
        Some(&project_config),
        Some(&empty_user_config(&temp)),
    ) {
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

    let prepared = prepare_session_for_test(
        serve_args(&temp, &["dev"]),
        Some(&project_config),
        Some(&empty_user_config(&temp)),
    )
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
        Some(&empty_user_config(&temp)),
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
    let prepared = prepare_session_for_test(
        serve_args(&temp, &[]),
        Some(&project_config),
        Some(&empty_user_config(&temp)),
    )
    .expect("prepared");
    assert_eq!(prepared.loaded_profiles, vec!["dev", "prod", "staging"]);
}

#[tokio::test]
async fn test_serve_without_profiles_starts_empty_mcp_server() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project_config = empty_user_config(&temp);
    let user_config = temp.path().join("empty-user.toml");
    std::fs::write(&user_config, "").expect("empty user profile config");

    let prepared = prepare_session_for_test(
        serve_args(&temp, &[]),
        Some(&project_config),
        Some(&user_config),
    )
    .expect("empty profile set should still start MCP");
    assert!(prepared.loaded_profiles.is_empty());

    let err = McpFrontend::with_session(prepared.session)
        .call_tool_result_for_test("query", query_args("dev"))
        .await
        .expect_err("unknown profile");
    assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
    assert!(err.message.contains("loaded: []"), "{}", err.message);
}

#[tokio::test]
async fn test_serve_profile_reload_applies_schema_allowlist_presentation() {
    let temp = tempfile::tempdir().expect("tempdir");
    let db = temp.path().join("fixture.sqlite");
    let project_config = temp.path().join("profiles.toml");
    let user_config = temp.path().join("user.toml");
    let policy = temp.path().join("policy.toml");
    seed_schema_db(&db);
    std::fs::write(&policy, "[policy.database]\n").expect("policy");
    std::fs::write(&user_config, "").expect("user profile");

    write_schema_profile(&project_config, &policy, &db, false);
    let raw_prepared = prepare_session_for_test(
        serve_args(&temp, &[]),
        Some(&project_config),
        Some(&user_config),
    )
    .expect("raw prepared");
    let raw_frontend = McpFrontend::with_session(raw_prepared.session);
    let raw_list = raw_frontend
        .call_tool_json("list_tables", serde_json::json!({"profile": "local"}))
        .await
        .expect("raw list_tables");
    assert_eq!(
        clean_text(&raw_list),
        "[\"customer_pii\",\"orders_sensitive\"]"
    );

    write_schema_profile(&project_config, &policy, &db, true);
    let tokenized_prepared = prepare_session_for_test(
        serve_args(&temp, &[]),
        Some(&project_config),
        Some(&user_config),
    )
    .expect("tokenized prepared");
    let tokenized_frontend = McpFrontend::with_session(tokenized_prepared.session);

    let list = tokenized_frontend
        .call_tool_json("list_tables", serde_json::json!({"profile": "local"}))
        .await
        .expect("tokenized list_tables");
    assert_eq!(clean_text(&list), "[\"customer_pii\",\"<TABLE_001>\"]");

    let schema = tokenized_frontend
        .call_tool_json(
            "schema",
            serde_json::json!({"profile": "local", "table": "customer_pii"}),
        )
        .await
        .expect("tokenized schema");
    let schema_text = clean_text(&schema);
    assert!(
        schema_text.contains("\"table_token\":\"customer_pii\""),
        "{schema_text}"
    );
    assert!(
        schema_text.contains("\"name_token\":\"id\""),
        "{schema_text}"
    );
    assert!(schema_text.contains("<COL_"), "{schema_text}");
    assert!(!schema_text.contains("email_address"), "{schema_text}");

    tokenized_frontend
        .call_tool_json(
            "query",
            serde_json::json!({
                "profile": "local",
                "table": "customer_pii",
                "columns": ["email_address"],
                "limit": 1
            }),
        )
        .await
        .expect("query still uses raw configured names");
}

fn policy() -> gaze::Policy {
    let mut policy = gaze::Policy::default();
    policy.session.scope = gaze::SessionScope::Conversation;
    policy.rulepacks.bundled = vec!["core".to_string()];
    policy
}

fn serve_args(temp: &tempfile::TempDir, profiles: &[&str]) -> ServeArgs {
    ServeArgs {
        profile: profiles.iter().map(|name| name.to_string()).collect(),
        manifest: temp.path().join("manifest.sqlite"),
        snapshot_dir: temp.path().join("snapshots"),
    }
}

fn empty_user_config(temp: &tempfile::TempDir) -> std::path::PathBuf {
    let path = temp.path().join("user.toml");
    std::fs::write(&path, "").expect("empty user profile config");
    path
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

fn write_schema_profile(
    path: &std::path::Path,
    policy: &std::path::Path,
    db: &std::path::Path,
    tokenize: bool,
) {
    std::fs::write(
        path,
        format!(
            r#"
            [[profiles]]
            name = "local"
            policy = "{}"
            schema_tokenize = {tokenize}
            schema_allowlist = ["customer_pii", "id"]
            source = {{ kind = "sqlite", path = "{}", readonly_required = true }}
            "#,
            policy.display(),
            db.display()
        ),
    )
    .expect("schema profile");
}

fn seed_schema_db(path: &std::path::Path) {
    let conn = Connection::open(path).expect("sqlite");
    conn.execute_batch(
        r#"
        CREATE TABLE customer_pii (
            id INTEGER PRIMARY KEY,
            email_address TEXT
        );
        INSERT INTO customer_pii (email_address) VALUES ('alice@example.com');
        CREATE TABLE orders_sensitive (
            id INTEGER PRIMARY KEY,
            internal_note TEXT
        );
        "#,
    )
    .expect("seed schema db");
}

fn clean_text(result: &serde_json::Value) -> &str {
    result["clean"]["Text"]["text"]
        .as_str()
        .or_else(|| result["clean"]["text"].as_str())
        .expect("clean text")
}

fn query_args(profile: &str) -> serde_json::Value {
    serde_json::json!({
        "profile": profile,
        "table": "users",
        "columns": ["email"],
        "limit": 1
    })
}
