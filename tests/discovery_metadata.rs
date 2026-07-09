use std::collections::HashMap;
use std::process::{Command, Stdio};
use std::sync::Arc;

use assert_cmd::cargo::CommandCargoExt;
use async_trait::async_trait;
use gaze_lens::frontend::mcp::McpFrontend;
use gaze_lens::session::manifest::{LensManifestStore, SnapshotRef};
use gaze_lens::session::{
    OutputCaps, RedactedToolArgs, ResultSummary, Session, SourceClass, ToolCall,
};
use gaze_lens::source::db::query::CannedQuery;
use gaze_lens::source::db::{ColumnInfo, DbKind, DbSource, TableSchema};
use gaze_lens::source::{DbSourceWrapper, SchemaPresentation};
use gaze_lens::value::LensRow;
use rusqlite::Connection;

fn policy() -> gaze::Policy {
    let mut policy = gaze::Policy::default();
    policy.session.scope = gaze::SessionScope::Conversation;
    policy.rulepacks.bundled = vec!["core".to_string()];
    policy
}

fn session_for_source(source: ScopeDbSource, presentation: SchemaPresentation) -> Arc<Session> {
    let temp = tempfile::tempdir().expect("tempdir");
    let snapshot_dir = temp.path().to_path_buf();
    let session = Arc::new(
        Session::new_with_manifest_for_tests(
            &policy(),
            Arc::new(NoopManifest),
            &snapshot_dir,
            OutputCaps::default(),
        )
        .expect("session"),
    );
    session.register_source_for_profile(
        SourceClass::Database,
        "test",
        Arc::new(DbSourceWrapper::with_schema_presentation(
            Arc::new(source),
            presentation,
        )),
    );
    std::mem::forget(temp);
    session
}

async fn call(frontend: &McpFrontend, tool: &str, args: serde_json::Value) -> serde_json::Value {
    frontend
        .call_tool_json(tool, args)
        .await
        .expect("tool call")
}

#[tokio::test]
async fn schema_response_includes_discovery_metadata() {
    let frontend = McpFrontend::with_session(session_for_source(
        ScopeDbSource::with_columns("users", &[("id", true), ("email", true), ("secret", false)]),
        SchemaPresentation::Raw,
    ));

    let response = call(
        &frontend,
        "schema",
        serde_json::json!({"profile": "test", "table": "users"}),
    )
    .await;
    let discovery = &response["clean"]["observability"]["discovery"];

    assert_eq!(discovery["profile"], "test");
    assert_eq!(discovery["source_class"], "database");
    assert_eq!(
        discovery["allowed_columns"],
        serde_json::json!(["email", "id"])
    );
    assert_hash(&discovery["schema_hash"]);
}

#[tokio::test]
async fn list_tables_response_includes_discovery_metadata_without_allowed_columns() {
    let frontend = McpFrontend::with_session(session_for_source(
        ScopeDbSource::with_tables(&["orders", "users"]),
        SchemaPresentation::Raw,
    ));

    let response = call(
        &frontend,
        "list_tables",
        serde_json::json!({"profile": "test"}),
    )
    .await;
    let clean = &response["clean"];
    let discovery = &clean["observability"]["discovery"];

    assert_eq!(clean["Text"]["text"], "[\"orders\",\"users\"]");
    assert_eq!(discovery["profile"], "test");
    assert_eq!(discovery["source_class"], "database");
    assert!(discovery.get("allowed_columns").is_none());
    assert_hash(&discovery["schema_hash"]);
}

#[tokio::test]
async fn schema_hash_is_stable_and_changes_when_scope_changes() {
    let frontend = McpFrontend::with_session(session_for_source(
        ScopeDbSource::with_columns("users", &[("id", true), ("email", true)]),
        SchemaPresentation::Raw,
    ));

    let first = call(
        &frontend,
        "schema",
        serde_json::json!({"profile": "test", "table": "users"}),
    )
    .await;
    let second = call(
        &frontend,
        "schema",
        serde_json::json!({"profile": "test", "table": "users"}),
    )
    .await;
    assert_eq!(
        first["clean"]["observability"]["discovery"]["schema_hash"],
        second["clean"]["observability"]["discovery"]["schema_hash"]
    );

    let changed_frontend = McpFrontend::with_session(session_for_source(
        ScopeDbSource::with_columns("users", &[("id", true), ("email", true), ("status", true)]),
        SchemaPresentation::Raw,
    ));
    let changed = call(
        &changed_frontend,
        "schema",
        serde_json::json!({"profile": "test", "table": "users"}),
    )
    .await;
    assert_ne!(
        first["clean"]["observability"]["discovery"]["schema_hash"],
        changed["clean"]["observability"]["discovery"]["schema_hash"]
    );

    let table_frontend = McpFrontend::with_session(session_for_source(
        ScopeDbSource::with_tables(&["users"]),
        SchemaPresentation::Raw,
    ));
    let table_first = call(
        &table_frontend,
        "list_tables",
        serde_json::json!({"profile": "test"}),
    )
    .await;
    let table_changed_frontend = McpFrontend::with_session(session_for_source(
        ScopeDbSource::with_tables(&["orders", "users"]),
        SchemaPresentation::Raw,
    ));
    let table_changed = call(
        &table_changed_frontend,
        "list_tables",
        serde_json::json!({"profile": "test"}),
    )
    .await;
    assert_ne!(
        table_first["clean"]["observability"]["discovery"]["schema_hash"],
        table_changed["clean"]["observability"]["discovery"]["schema_hash"]
    );
}

#[tokio::test]
async fn observability_is_additive_to_existing_clean_text_consumers() {
    let frontend = McpFrontend::with_session(session_for_source(
        ScopeDbSource::with_columns("users", &[("id", true), ("email", true)]),
        SchemaPresentation::Raw,
    ));

    let response = call(
        &frontend,
        "schema",
        serde_json::json!({"profile": "test", "table": "users"}),
    )
    .await;

    assert!(response["clean"]["Text"]["text"].as_str().is_some());
    assert!(response["clean"]["observability"]["discovery"].is_object());
}

#[tokio::test]
async fn schema_tokenized_profiles_do_not_leak_raw_discovery_labels() {
    let frontend = McpFrontend::with_session(session_for_source(
        ScopeDbSource::with_columns(
            "customers_private",
            &[
                ("id", true),
                ("email_address", true),
                ("internal_note", false),
            ],
        ),
        SchemaPresentation::Tokenized { allowlist: None },
    ));

    let response = call(
        &frontend,
        "schema",
        serde_json::json!({"profile": "test", "table": "customers_private"}),
    )
    .await;
    let encoded = serde_json::to_string(&response).expect("response json");

    assert!(!encoded.contains("customers_private"), "{encoded}");
    assert!(!encoded.contains("email_address"), "{encoded}");
    assert!(!encoded.contains("internal_note"), "{encoded}");
    assert_eq!(
        response["clean"]["observability"]["discovery"]["allowed_columns"],
        serde_json::json!(["<COL_001>", "id"])
    );
}

#[test]
fn serve_print_discovery_lists_db_and_log_profiles_without_starting_mcp() {
    let temp = tempfile::tempdir().expect("tempdir");
    let db = temp.path().join("fixture.sqlite");
    let project = temp.path().join("profiles.toml");
    seed_sqlite(&db);
    write_profiles(&project, &db);

    let output = Command::cargo_bin("gaze-lens")
        .expect("binary")
        .args([
            "--project-config",
            project.to_str().expect("project path"),
            "serve",
            "--print-discovery",
            "--manifest",
            temp.path()
                .join("manifest.sqlite")
                .to_str()
                .expect("manifest path"),
            "--snapshot-dir",
            temp.path()
                .join("snapshots")
                .to_str()
                .expect("snapshot dir"),
        ])
        .stdin(Stdio::null())
        .output()
        .expect("serve --print-discovery");

    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("ProfileUnknown"), "{stderr}");
    assert!(!stderr.contains("ProfileMismatch"), "{stderr}");

    let inventory: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("inventory json");
    let profiles = inventory["profiles"].as_array().expect("profiles array");
    let db_profile = profile_named(profiles, "local-db");
    let log_profile = profile_named(profiles, "app-log");

    assert_eq!(db_profile["source_class"], "database");
    assert_eq!(
        db_profile["supported_tools"],
        serde_json::json!(["query", "schema", "list_tables"])
    );
    assert_eq!(db_profile["scope"]["tables"][0]["name"], "users");
    assert_eq!(
        db_profile["scope"]["tables"][0]["allowed_columns"],
        serde_json::json!(["email", "id", "secret"])
    );
    assert_hash(&db_profile["schema_hash"]);

    assert_eq!(log_profile["source_class"], "log");
    assert_eq!(
        log_profile["supported_tools"],
        serde_json::json!(["log_tail", "log_grep"])
    );
    assert_eq!(log_profile["scope"]["host"], "prod.example");
    assert_eq!(log_profile["scope"]["path"], "/var/log/app.log");
    assert_hash(&log_profile["schema_hash"]);
}

#[test]
fn serve_print_discovery_schema_tokenize_does_not_leak_raw_labels() {
    let temp = tempfile::tempdir().expect("tempdir");
    let db = temp.path().join("fixture.sqlite");
    let project = temp.path().join("profiles.toml");
    seed_tokenized_sqlite(&db);
    write_tokenized_profiles(&project, &db);

    let output = Command::cargo_bin("gaze-lens")
        .expect("binary")
        .args([
            "--project-config",
            project.to_str().expect("project path"),
            "serve",
            "--print-discovery",
            "--manifest",
            temp.path()
                .join("manifest.sqlite")
                .to_str()
                .expect("manifest path"),
            "--snapshot-dir",
            temp.path()
                .join("snapshots")
                .to_str()
                .expect("snapshot dir"),
        ])
        .stdin(Stdio::null())
        .output()
        .expect("serve --print-discovery");

    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let encoded = String::from_utf8(output.stdout).expect("inventory utf8");

    assert!(!encoded.contains("customers_private"), "{encoded}");
    assert!(!encoded.contains("email_address"), "{encoded}");
    assert!(!encoded.contains("internal_note"), "{encoded}");
    assert!(encoded.contains("<TABLE_001>"), "{encoded}");
    assert!(encoded.contains("<COL_001>"), "{encoded}");
    assert!(encoded.contains("<COL_002>"), "{encoded}");

    let inventory: serde_json::Value = serde_json::from_str(&encoded).expect("inventory json");
    let profiles = inventory["profiles"].as_array().expect("profiles array");
    let db_profile = profile_named(profiles, "local-db");

    assert_eq!(db_profile["scope"]["tables"][0]["name"], "<TABLE_001>");
    assert_eq!(
        db_profile["scope"]["tables"][0]["allowed_columns"],
        serde_json::json!(["<COL_001>", "<COL_002>"])
    );
    assert_hash(&db_profile["schema_hash"]);
}

fn profile_named<'a>(profiles: &'a [serde_json::Value], name: &str) -> &'a serde_json::Value {
    profiles
        .iter()
        .find(|profile| profile["name"] == name)
        .unwrap_or_else(|| panic!("missing profile {name}: {profiles:?}"))
}

fn assert_hash(value: &serde_json::Value) {
    let hash = value.as_str().expect("hash string");
    assert_eq!(hash.len(), 64);
    assert!(hash.chars().all(|ch| ch.is_ascii_hexdigit()), "{hash}");
}

fn seed_sqlite(path: &std::path::Path) {
    let conn = Connection::open(path).expect("sqlite");
    conn.execute_batch(
        r#"
        CREATE TABLE users (
            id INTEGER PRIMARY KEY,
            email TEXT,
            secret TEXT
        );
        "#,
    )
    .expect("seed db");
}

fn seed_tokenized_sqlite(path: &std::path::Path) {
    let conn = Connection::open(path).expect("sqlite");
    conn.execute_batch(
        r#"
        CREATE TABLE customers_private (
            email_address TEXT,
            internal_note TEXT
        );
        "#,
    )
    .expect("seed tokenized db");
}

fn write_profiles(path: &std::path::Path, db: &std::path::Path) {
    std::fs::write(
        path,
        format!(
            r#"
            [[profiles]]
            name = "local-db"
            source = {{ kind = "sqlite", path = "{}", readonly_required = true }}

            [[profiles]]
            name = "app-log"
            source = {{ kind = "ssh_log", host = "prod.example", path = "/var/log/app.log" }}
            "#,
            db.display()
        ),
    )
    .expect("profiles");
}

fn write_tokenized_profiles(path: &std::path::Path, db: &std::path::Path) {
    std::fs::write(
        path,
        format!(
            r#"
            [[profiles]]
            name = "local-db"
            schema_tokenize = true
            source = {{ kind = "sqlite", path = "{}", readonly_required = true }}
            "#,
            db.display()
        ),
    )
    .expect("tokenized profiles");
}

#[derive(Clone)]
struct ScopeDbSource {
    tables: Vec<String>,
    schemas: HashMap<String, TableSchema>,
}

impl ScopeDbSource {
    fn with_tables(tables: &[&str]) -> Self {
        let schemas = tables
            .iter()
            .map(|table| ((*table).to_string(), schema(table, &[("id", true)])))
            .collect();
        Self {
            tables: tables.iter().map(|table| (*table).to_string()).collect(),
            schemas,
        }
    }

    fn with_columns(table: &str, columns: &[(&str, bool)]) -> Self {
        Self {
            tables: vec![table.to_string()],
            schemas: HashMap::from([(table.to_string(), schema(table, columns))]),
        }
    }
}

#[async_trait]
impl DbSource for ScopeDbSource {
    fn kind(&self) -> DbKind {
        DbKind::Sqlite
    }

    fn profile_name(&self) -> &str {
        "test"
    }

    async fn list_tables(&self) -> Result<Vec<String>, gaze_lens::errors::LensError> {
        Ok(self.tables.clone())
    }

    async fn schema(&self, table: &str) -> Result<TableSchema, gaze_lens::errors::LensError> {
        self.schemas
            .get(table)
            .cloned()
            .ok_or_else(|| gaze_lens::errors::LensError::SourceError {
                source_name: "test".to_string(),
                detail: format!("unknown table {table}"),
                sql: None,
                stderr: None,
            })
    }

    async fn query(
        &self,
        _query: &CannedQuery,
    ) -> Result<Vec<LensRow>, gaze_lens::errors::LensError> {
        Ok(Vec::new())
    }
}

fn schema(table: &str, columns: &[(&str, bool)]) -> TableSchema {
    TableSchema {
        table: table.to_string(),
        table_token: table.to_string(),
        columns: columns
            .iter()
            .map(|(name, allowed)| ColumnInfo {
                name: (*name).to_string(),
                name_token: (*name).to_string(),
                data_type: "text".to_string(),
                nullable: false,
                allowed: *allowed,
            })
            .collect(),
        limit_cap: Some(100),
    }
}

struct NoopManifest;

impl LensManifestStore for NoopManifest {
    fn begin_call(
        &self,
        _call: &ToolCall,
        _redacted_args: &RedactedToolArgs,
    ) -> Result<(), gaze_lens::errors::LensError> {
        Ok(())
    }

    fn finish_call(
        &self,
        _call_id: &str,
        _summary: &ResultSummary,
        _snapshot_ref: &SnapshotRef,
    ) -> Result<(), gaze_lens::errors::LensError> {
        Ok(())
    }

    fn fail_call(
        &self,
        _call_id: &str,
        _error: &gaze_lens::errors::LensError,
    ) -> Result<(), gaze_lens::errors::LensError> {
        Ok(())
    }
}
