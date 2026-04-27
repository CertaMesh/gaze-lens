use std::collections::BTreeMap;
use std::sync::Arc;

use gaze_lens::errors::LensError;
use gaze_lens::session::{CleanOutput, Session, ToolCall};
use gaze_lens::source::DbSourceWrapper;
use gaze_lens::source::db::query::CannedQuery;
use gaze_lens::source::db::sqlite::SqliteSource;
use gaze_lens::source::db::{DbSource, TableSchema};
use gaze_lens::value::LensValue;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

#[tokio::test]
async fn sqlite_decodes_runtime_type_not_declared_affinity() {
    let source = sqlite_source_with_json_text_columns(["items.json_text"]).await;

    let rows = source
        .query(&CannedQuery {
            table: "items".to_string(),
            columns: Some(vec![
                "integer_declared_text_value".to_string(),
                "real_value".to_string(),
                "text_value".to_string(),
                "blob_value".to_string(),
                "bool_false".to_string(),
                "bool_true".to_string(),
                "json_text".to_string(),
                "timestamp_text".to_string(),
                "null_value".to_string(),
            ]),
            r#where: None,
            where_combinator: None,
            order_by: None,
            limit: Some(1),
        })
        .await
        .expect("query");

    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(
        row["integer_declared_text_value"],
        LensValue::String("stored as text".to_string())
    );
    assert_eq!(row["real_value"], LensValue::F64(12.5));
    assert_eq!(
        row["text_value"],
        LensValue::String("plain text".to_string())
    );
    assert_eq!(
        row["blob_value"],
        LensValue::Bytes {
            base64: "AQID".to_string(),
            len: 3,
        }
    );
    assert_eq!(row["bool_false"], LensValue::Bool(false));
    assert_eq!(row["bool_true"], LensValue::Bool(true));
    assert_eq!(
        row["json_text"],
        LensValue::Json(serde_json::json!({"email": "alice@example.com"}))
    );
    assert_eq!(
        row["timestamp_text"],
        LensValue::DateTime("2026-04-26T22:30:15Z".to_string())
    );
    assert_eq!(row["null_value"], LensValue::Null);
}

#[tokio::test]
async fn sqlite_schema_uses_declared_types_for_metadata() {
    let source = sqlite_source().await;

    let schema = source.schema("items").await.expect("schema");

    assert_column(&schema, "integer_declared_text_value", "INTEGER", true);
    assert_column(&schema, "bool_true", "BOOLEAN", true);
    assert_column(&schema, "timestamp_text", "TIMESTAMP", true);
}

#[tokio::test]
async fn sqlite_source_can_route_through_session_db_wrapper() {
    let source = sqlite_source_with_json_text_columns(["items.json_text"]).await;
    let temp = tempfile::tempdir().expect("tempdir");
    let session = Arc::new(
        Session::new(
            &policy(),
            &temp.path().join("manifest.sqlite"),
            &temp.path().join("snapshots"),
        )
        .expect("session"),
    );
    let wrapper = Arc::new(DbSourceWrapper::new(Arc::new(source)));
    session.register_source("query", wrapper);

    let result = session
        .dispatch_tool(ToolCall {
            call_id: "sqlite-roundtrip".to_string(),
            tool_name: "query".to_string(),
            args: gaze_lens::source::ToolArgs(serde_json::json!({
                "table": "items",
                "columns": ["json_text"],
                "limit": 1
            })),
        })
        .await
        .expect("dispatch");

    let CleanOutput::Rows { rows, .. } = result.clean else {
        panic!("expected rows");
    };
    let rows: Vec<BTreeMap<String, LensValue>> = serde_json::from_value(serde_json::Value::Array(
        rows.into_iter().map(Into::into).collect(),
    ))
    .expect("rows");
    assert_eq!(rows.len(), 1);
    let LensValue::Json(json) = &rows[0]["json_text"] else {
        panic!("expected json");
    };
    let email = json["email"].as_str().expect("email");
    assert!(email.contains("Email_1"));
    assert!(!email.contains("alice@example.com"));
}

#[tokio::test]
async fn sqlite_text_json_default_denies_scalar_looking_text() {
    let source = sqlite_source().await;

    let rows = source
        .query(&CannedQuery {
            table: "items".to_string(),
            columns: Some(vec![
                "json_text".to_string(),
                "scalar_bool_text".to_string(),
                "scalar_number_text".to_string(),
            ]),
            r#where: None,
            where_combinator: None,
            order_by: None,
            limit: Some(1),
        })
        .await
        .expect("query");

    let row = &rows[0];
    assert_eq!(
        row["json_text"],
        LensValue::String(r#"{"email":"alice@example.com"}"#.to_string())
    );
    assert_eq!(
        row["scalar_bool_text"],
        LensValue::String("true".to_string())
    );
    assert_eq!(
        row["scalar_number_text"],
        LensValue::String("123".to_string())
    );
}

#[tokio::test]
async fn sqlite_text_json_allowlist_decodes_objects_and_arrays() {
    let source =
        sqlite_source_with_json_text_columns(["items.json_text", "items.json_array_text"]).await;

    let rows = source
        .query(&CannedQuery {
            table: "items".to_string(),
            columns: Some(vec!["json_text".to_string(), "json_array_text".to_string()]),
            r#where: None,
            where_combinator: None,
            order_by: None,
            limit: Some(1),
        })
        .await
        .expect("query");

    let row = &rows[0];
    assert_eq!(
        row["json_text"],
        LensValue::Json(serde_json::json!({"email": "alice@example.com"}))
    );
    assert_eq!(
        row["json_array_text"],
        LensValue::Json(serde_json::json!(["alpha", 2]))
    );
}

#[tokio::test]
async fn sqlite_text_json_allowlist_rejects_invalid_json_text() {
    let source = sqlite_source_with_json_text_columns(["items.invalid_json_text"]).await;

    let err = source
        .query(&CannedQuery {
            table: "items".to_string(),
            columns: Some(vec!["invalid_json_text".to_string()]),
            r#where: None,
            where_combinator: None,
            order_by: None,
            limit: Some(1),
        })
        .await
        .expect_err("invalid json should reject row");

    assert!(matches!(err, LensError::ConvertError(_)));
}

async fn sqlite_source() -> SqliteSource {
    sqlite_source_with_json_text_columns(std::iter::empty::<&str>()).await
}

async fn sqlite_source_with_json_text_columns(
    json_text_columns: impl IntoIterator<Item = impl Into<String>>,
) -> SqliteSource {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("target.sqlite");
    let options = SqliteConnectOptions::new()
        .filename(&path)
        .create_if_missing(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .expect("sqlite pool");
    setup_schema(&pool).await;
    std::mem::forget(temp);
    SqliteSource::from_pool_for_tests_with_json_text_columns(
        pool,
        "sqlite-test",
        100,
        json_text_columns,
    )
}

async fn setup_schema(pool: &sqlx::SqlitePool) {
    sqlx::query(
        r#"
        CREATE TABLE items (
            integer_declared_text_value INTEGER,
            real_value REAL,
            text_value TEXT,
            blob_value BLOB,
            bool_false BOOLEAN,
            bool_true BOOLEAN,
            json_text TEXT,
            json_array_text TEXT,
            invalid_json_text TEXT,
            scalar_bool_text TEXT,
            scalar_number_text TEXT,
            timestamp_text TIMESTAMP,
            null_value TEXT
        )
        "#,
    )
    .execute(pool)
    .await
    .expect("create table");

    sqlx::query(
        r#"
        INSERT INTO items (
            integer_declared_text_value,
            real_value,
            text_value,
            blob_value,
            bool_false,
            bool_true,
            json_text,
            json_array_text,
            invalid_json_text,
            scalar_bool_text,
            scalar_number_text,
            timestamp_text,
            null_value
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind("stored as text")
    .bind(12.5_f64)
    .bind("plain text")
    .bind(vec![1_u8, 2, 3])
    .bind(0_i64)
    .bind(1_i64)
    .bind(r#"{"email":"alice@example.com"}"#)
    .bind(r#"["alpha",2]"#)
    .bind("not json")
    .bind("true")
    .bind("123")
    .bind("2026-04-26T22:30:15Z")
    .bind(Option::<String>::None)
    .execute(pool)
    .await
    .expect("insert");
}

fn assert_column(schema: &TableSchema, name: &str, data_type: &str, nullable: bool) {
    let column = schema
        .columns
        .iter()
        .find(|column| column.name == name)
        .expect("column");
    assert_eq!(column.data_type, data_type);
    assert_eq!(column.nullable, nullable);
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
