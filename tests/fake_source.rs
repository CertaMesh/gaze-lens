use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use gaze_lens::errors::LensError;
use gaze_lens::session::ToolCall;
use gaze_lens::source::db::query::CannedQuery;
use gaze_lens::source::db::{ColumnInfo, DbKind, DbSource, TableSchema};
use gaze_lens::source::{
    DbSourceWrapper, FakeSource, InMemoryFakeSource, SchemaPresentation, Source, SourceOutput,
    ToolArgs,
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
        SourceOutput::Text(_)
        | SourceOutput::SchemaText(_)
        | SourceOutput::TextWithTruncation { .. } => {
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
        SourceOutput::Rows(_)
        | SourceOutput::SchemaText(_)
        | SourceOutput::TextWithTruncation { .. } => {
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
                "profile": "test",
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
        SourceOutput::Text(_)
        | SourceOutput::SchemaText(_)
        | SourceOutput::TextWithTruncation { .. } => {
            panic!("expected rows")
        }
    }
}

#[tokio::test]
async fn db_source_wrapper_defaults_schema_metadata_to_raw() {
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
        SourceOutput::SchemaText(text) => {
            assert!(!text.contains("<COL_"));
            assert!(text.contains("email_address"));
        }
        SourceOutput::Rows(_) | SourceOutput::Text(_) | SourceOutput::TextWithTruncation { .. } => {
            panic!("expected schema text")
        }
    }
}

#[tokio::test]
async fn db_source_wrapper_defaults_list_tables_to_raw() {
    let source = DbSourceWrapper::new(Arc::new(RecordingDbSource));

    let output = source
        .dispatch(&ToolCall {
            call_id: "call-1".to_string(),
            tool_name: "list_tables".to_string(),
            args: ToolArgs(serde_json::json!({})),
        })
        .await
        .expect("dispatch");

    match output {
        SourceOutput::SchemaText(text) => {
            assert_eq!(text, "[\"users\"]");
        }
        SourceOutput::Rows(_) | SourceOutput::Text(_) | SourceOutput::TextWithTruncation { .. } => {
            panic!("expected schema text")
        }
    }
}

#[tokio::test]
async fn db_source_wrapper_tokenizes_schema_metadata_when_enabled() {
    let source = DbSourceWrapper::with_schema_presentation(
        Arc::new(RecordingDbSource),
        SchemaPresentation::Tokenized { allowlist: None },
    );

    let output = source
        .dispatch(&ToolCall {
            call_id: "call-1".to_string(),
            tool_name: "schema".to_string(),
            args: ToolArgs(serde_json::json!({"table": "users"})),
        })
        .await
        .expect("dispatch");

    match output {
        SourceOutput::SchemaText(text) => {
            assert!(text.contains("<COL_"));
            assert!(!text.contains("email_address"));
        }
        SourceOutput::Rows(_) | SourceOutput::Text(_) | SourceOutput::TextWithTruncation { .. } => {
            panic!("expected schema text")
        }
    }
}

#[tokio::test]
async fn db_source_wrapper_keeps_allowlisted_schema_metadata_raw_when_tokenized() {
    let source = DbSourceWrapper::with_schema_presentation(
        Arc::new(RecordingDbSource),
        SchemaPresentation::Tokenized {
            allowlist: Some(vec!["users".to_string(), "email_address".to_string()]),
        },
    );
    let output = source
        .dispatch(&ToolCall {
            call_id: "call-1".to_string(),
            tool_name: "schema".to_string(),
            args: ToolArgs(serde_json::json!({"table": "users"})),
        })
        .await
        .expect("dispatch");

    match output {
        SourceOutput::SchemaText(text) => {
            assert!(text.contains("\"table_token\":\"users\""), "{text}");
            assert!(text.contains("\"name_token\":\"email_address\""), "{text}");
            assert!(!text.contains("<TABLE_"), "{text}");
        }
        SourceOutput::Rows(_) | SourceOutput::Text(_) | SourceOutput::TextWithTruncation { .. } => {
            panic!("expected schema text")
        }
    }
}

#[tokio::test]
async fn db_source_wrapper_tokenizes_list_tables_when_enabled() {
    let source = DbSourceWrapper::with_schema_presentation(
        Arc::new(RecordingDbSource),
        SchemaPresentation::Tokenized { allowlist: None },
    );

    let output = source
        .dispatch(&ToolCall {
            call_id: "call-1".to_string(),
            tool_name: "list_tables".to_string(),
            args: ToolArgs(serde_json::json!({})),
        })
        .await
        .expect("dispatch");

    match output {
        SourceOutput::SchemaText(text) => {
            assert_eq!(text, "[\"<TABLE_001>\"]");
        }
        SourceOutput::Rows(_) | SourceOutput::Text(_) | SourceOutput::TextWithTruncation { .. } => {
            panic!("expected schema text")
        }
    }
}

#[tokio::test]
async fn db_source_wrapper_keeps_allowlisted_table_names_raw_when_tokenized() {
    let source = DbSourceWrapper::with_schema_presentation(
        Arc::new(RecordingDbSource),
        SchemaPresentation::Tokenized {
            allowlist: Some(vec!["users".to_string()]),
        },
    );
    let output = source
        .dispatch(&ToolCall {
            call_id: "call-1".to_string(),
            tool_name: "list_tables".to_string(),
            args: ToolArgs(serde_json::json!({})),
        })
        .await
        .expect("dispatch");

    match output {
        SourceOutput::SchemaText(text) => {
            assert_eq!(text, "[\"users\"]");
        }
        SourceOutput::Rows(_) | SourceOutput::Text(_) | SourceOutput::TextWithTruncation { .. } => {
            panic!("expected schema text")
        }
    }
}

#[tokio::test]
async fn raw_schema_presentation_does_not_authorize_disallowed_query_columns() {
    let source = DbSourceWrapper::with_schema_presentation(
        Arc::new(RestrictedDbSource),
        SchemaPresentation::Raw,
    );

    let err = source
        .dispatch(&ToolCall {
            call_id: "call-1".to_string(),
            tool_name: "query".to_string(),
            args: ToolArgs(serde_json::json!({
                "profile": "test",
                "table": "users",
                "columns": ["password"],
                "limit": 1
            })),
        })
        .await
        .expect_err("query must reject disallowed column");

    assert!(err.to_string().contains("not allowed by schema policy"));
}

struct RecordingDbSource;

struct RestrictedDbSource;

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
            columns: vec![
                ColumnInfo {
                    name: "email".to_string(),
                    name_token: "email".to_string(),
                    data_type: "varchar".to_string(),
                    nullable: false,
                    allowed: true,
                },
                ColumnInfo {
                    name: "email_address".to_string(),
                    name_token: "email_address".to_string(),
                    data_type: "varchar".to_string(),
                    nullable: false,
                    allowed: true,
                },
            ],
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

#[async_trait]
impl DbSource for RestrictedDbSource {
    fn kind(&self) -> DbKind {
        DbKind::Sqlite
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
            columns: vec![
                ColumnInfo {
                    name: "email".to_string(),
                    name_token: "email".to_string(),
                    data_type: "varchar".to_string(),
                    nullable: false,
                    allowed: true,
                },
                ColumnInfo {
                    name: "password".to_string(),
                    name_token: "password".to_string(),
                    data_type: "varchar".to_string(),
                    nullable: false,
                    allowed: false,
                },
            ],
            limit_cap: Some(10),
        })
    }

    async fn query(&self, _query: &CannedQuery) -> Result<Vec<LensRow>, LensError> {
        Ok(Vec::new())
    }
}
