use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use gaze_lens::errors::LensError;
use gaze_lens::session::ToolCall;
use gaze_lens::source::db::query::CannedQuery;
use gaze_lens::source::db::{ColumnInfo, DbKind, DbSource, TableSchema};
use gaze_lens::source::{
    DbSourceWrapper, FakeSource, InMemoryFakeSource, Source, SourceOutput, ToolArgs,
};
use gaze_lens::value::LensRow;
use gaze_lens::value::LensValue;

#[tokio::test]
async fn in_memory_fake_source_returns_canned_rows() {
    let source = InMemoryFakeSource::rows(vec![BTreeMap::from([(
        "email".to_string(),
        LensValue::String("alice@example.com".to_string()),
    )])]);

    let output = source
        .invoke(&ToolArgs(serde_json::json!({"ignored": true})))
        .await
        .expect("invoke");

    match output {
        SourceOutput::Rows(rows) => assert_eq!(rows.len(), 1),
        SourceOutput::Text(_) | SourceOutput::TextWithTruncation { .. } => {
            panic!("expected rows")
        }
    }
}

#[tokio::test]
async fn in_memory_fake_source_returns_canned_text() {
    let source = InMemoryFakeSource::text("alice@example.com");

    let output = source
        .invoke(&ToolArgs(serde_json::json!({})))
        .await
        .expect("invoke");

    match output {
        SourceOutput::Text(text) => assert_eq!(text, "alice@example.com"),
        SourceOutput::Rows(_) | SourceOutput::TextWithTruncation { .. } => {
            panic!("expected text")
        }
    }
}

#[tokio::test]
async fn db_source_wrapper_routes_query_calls() {
    let source = DbSourceWrapper::new(Arc::new(RecordingDbSource));

    let output = source
        .dispatch(&ToolCall {
            call_id: "call-1".to_string(),
            tool_name: "query".to_string(),
            args: ToolArgs(serde_json::json!({
                "table": "users",
                "columns": ["email"],
                "limit": 1
            })),
        })
        .await
        .expect("dispatch");

    match output {
        SourceOutput::Rows(rows) => {
            assert_eq!(
                rows[0].get("email"),
                Some(&LensValue::String("alice@example.com".to_string()))
            );
        }
        SourceOutput::Text(_) | SourceOutput::TextWithTruncation { .. } => {
            panic!("expected rows")
        }
    }
}

#[tokio::test]
async fn db_source_wrapper_tokenizes_schema_metadata() {
    let source = DbSourceWrapper::new(Arc::new(RecordingDbSource));

    let output = source
        .dispatch(&ToolCall {
            call_id: "call-1".to_string(),
            tool_name: "schema".to_string(),
            args: ToolArgs(serde_json::json!({"table": "users"})),
        })
        .await
        .expect("dispatch");

    match output {
        SourceOutput::Text(text) => {
            assert!(text.contains("<COL_"));
            assert!(!text.contains("email_address"));
        }
        SourceOutput::Rows(_) | SourceOutput::TextWithTruncation { .. } => {
            panic!("expected text")
        }
    }
}

struct RecordingDbSource;

#[async_trait]
impl DbSource for RecordingDbSource {
    fn kind(&self) -> DbKind {
        DbKind::Mysql
    }

    fn profile_name(&self) -> &str {
        "test"
    }

    async fn list_tables(&self) -> Result<Vec<String>, LensError> {
        Ok(vec!["users".to_string()])
    }

    async fn schema(&self, table: &str) -> Result<TableSchema, LensError> {
        Ok(TableSchema {
            table: table.to_string(),
            table_token: table.to_string(),
            columns: vec![ColumnInfo {
                name: "email_address".to_string(),
                name_token: "email_address".to_string(),
                data_type: "varchar".to_string(),
                nullable: false,
                allowed: true,
            }],
            limit_cap: Some(10),
        })
    }

    async fn query(&self, _query: &CannedQuery) -> Result<Vec<LensRow>, LensError> {
        Ok(vec![BTreeMap::from([(
            "email".to_string(),
            LensValue::String("alice@example.com".to_string()),
        )])])
    }
}
