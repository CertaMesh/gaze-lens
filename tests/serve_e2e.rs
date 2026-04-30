use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use gaze_lens::frontend::mcp::McpFrontend;
use gaze_lens::session::Session;
use gaze_lens::source::db::query::CannedQuery;
use gaze_lens::source::db::{ColumnInfo, DbKind, DbSource, TableSchema};
use gaze_lens::source::{DbSourceWrapper, FakeSource, SourceOutput, ToolArgs};
use gaze_lens::value::{LensRow, LensValue};
use rmcp::model::CallToolRequestParam;
use rmcp::{ClientHandler, ServiceExt};

#[derive(Clone, Default)]
struct TestClient;

impl ClientHandler for TestClient {}

#[tokio::test]
async fn mcp_duplex_query_roundtrips_manifest_and_snapshot() {
    let temp = tempfile::tempdir().expect("tempdir");
    let session = Arc::new(
        Session::new(
            &policy(),
            &temp.path().join("manifest.sqlite"),
            &temp.path().join("snapshots"),
        )
        .expect("session"),
    );
    let source = Arc::new(DbSourceWrapper::new(Arc::new(FakeDbSource)));
    for tool_name in ["query", "schema", "list_tables"] {
        session.register_source(tool_name, source.clone());
    }
    session.register_fake_source(
        "log_tail",
        Box::new(FakeLogSource {
            lines: log_lines(),
            mode: LogMode::Tail,
        }),
    );
    session.register_fake_source(
        "log_grep",
        Box::new(FakeLogSource {
            lines: log_lines(),
            mode: LogMode::Grep,
        }),
    );

    let (server_transport, client_transport) = tokio::io::duplex(4096);
    let server = McpFrontend::with_session(session);
    let server_handle = tokio::spawn(async move {
        let running = server.serve(server_transport).await.expect("server");
        running.waiting().await.expect("server wait");
    });
    let client = TestClient.serve(client_transport).await.expect("client");

    let tools = client.list_all_tools().await.expect("list tools");
    let mut tool_names = tools
        .iter()
        .map(|tool| tool.name.as_ref())
        .collect::<Vec<_>>();
    tool_names.sort_unstable();
    assert_eq!(
        tool_names,
        vec!["list_tables", "log_grep", "log_tail", "query", "schema"]
    );

    let result = client
        .call_tool(CallToolRequestParam {
            name: "query".into(),
            arguments: serde_json::json!({
                "profile": "default",
                "table": "users",
                "columns": ["email"],
                "limit": 1
            })
            .as_object()
            .cloned(),
        })
        .await
        .expect("call tool");
    let result_text = result
        .content
        .first()
        .and_then(|content| content.raw.as_text())
        .map(|text| text.text.as_str())
        .expect("text result");
    assert!(!result_text.contains("alice@example.com"));
    assert!(result_text.contains("Email_1"));

    let snapshot_path = snapshot_path(result_text);
    assert!(snapshot_path.exists(), "snapshot should exist");

    let tail = client
        .call_tool(CallToolRequestParam {
            name: "log_tail".into(),
            arguments: serde_json::json!({"profile": "default", "lines": 10})
                .as_object()
                .cloned(),
        })
        .await
        .expect("log_tail");
    let tail_text = tool_result_text(&tail);
    assert!(tail_text.contains("INFO boot"));
    assert!(!tail_text.contains("bob@example.com"));

    let grep = client
        .call_tool(CallToolRequestParam {
            name: "log_grep".into(),
            arguments: serde_json::json!({
                "profile": "default",
                "pattern": "bob@example.com",
                "level": "ERROR",
                "limit": 5
            })
            .as_object()
            .cloned(),
        })
        .await
        .expect("log_grep");
    let grep_text = tool_result_text(&grep);
    assert!(grep_text.contains("ERROR"));
    assert!(!grep_text.contains("bob@example.com"));
    assert!(!grep_text.contains("INFO boot"));

    let manifest = rusqlite::Connection::open(temp.path().join("manifest.sqlite")).expect("db");
    let call_count: u32 = manifest
        .query_row("SELECT COUNT(*) FROM calls", [], |row| row.get(0))
        .expect("call count");
    assert_eq!(call_count, 3);

    client.cancel().await.expect("client cancel");
    server_handle.await.expect("server task");
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

fn snapshot_path(result_text: &str) -> std::path::PathBuf {
    let result: serde_json::Value = serde_json::from_str(result_text).expect("tool result json");
    result["snapshot_ref"]["path"]
        .as_str()
        .expect("snapshot path")
        .into()
}

fn tool_result_text(result: &rmcp::model::CallToolResult) -> String {
    result
        .content
        .first()
        .and_then(|content| content.raw.as_text())
        .map(|text| text.text.as_str())
        .expect("text result")
        .to_string()
}

fn log_lines() -> Vec<String> {
    vec![
        "INFO boot complete".to_string(),
        "ERROR bob@example.com failed checkout".to_string(),
    ]
}

struct FakeDbSource;

#[async_trait]
impl DbSource for FakeDbSource {
    fn kind(&self) -> DbKind {
        DbKind::Mysql
    }

    fn profile_name(&self) -> &str {
        "fake"
    }

    async fn list_tables(&self) -> Result<Vec<String>, gaze_lens::errors::LensError> {
        Ok(vec!["users".to_string()])
    }

    async fn schema(&self, table: &str) -> Result<TableSchema, gaze_lens::errors::LensError> {
        Ok(TableSchema {
            table: table.to_string(),
            table_token: table.to_string(),
            columns: vec![ColumnInfo {
                name: "email".to_string(),
                name_token: "email".to_string(),
                data_type: "varchar".to_string(),
                nullable: false,
                allowed: true,
            }],
            limit_cap: Some(10),
        })
    }

    async fn query(
        &self,
        _query: &CannedQuery,
    ) -> Result<Vec<LensRow>, gaze_lens::errors::LensError> {
        Ok(vec![BTreeMap::from([(
            "email".to_string(),
            LensValue::String("alice@example.com".to_string()),
        )])])
    }
}

struct FakeLogSource {
    lines: Vec<String>,
    mode: LogMode,
}

enum LogMode {
    Tail,
    Grep,
}

#[async_trait]
impl FakeSource for FakeLogSource {
    async fn invoke(&self, args: &ToolArgs) -> Result<SourceOutput, gaze_lens::errors::LensError> {
        let lines = match self.mode {
            LogMode::Tail => self.lines.clone(),
            LogMode::Grep => {
                let pattern = args
                    .0
                    .get("pattern")
                    .and_then(|value| value.as_str())
                    .expect("pattern");
                let level = args.0.get("level").and_then(|value| value.as_str());
                let re = regex::Regex::new(pattern).expect("regex");
                self.lines
                    .iter()
                    .filter(|line| re.is_match(line))
                    .filter(|line| level.is_none_or(|level| line.contains(level)))
                    .cloned()
                    .collect()
            }
        };
        Ok(SourceOutput::Text(lines.join("\n")))
    }
}
