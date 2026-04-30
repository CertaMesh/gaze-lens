use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use gaze::{Action, DefaultRule};
use gaze_lens::errors::LensError;
use gaze_lens::frontend::mcp::McpFrontend;
use gaze_lens::session::{Session, SourceBuilder, SourceClass};
use gaze_lens::source::{FakeSource, FakeSourceAdapter, SourceOutput, ToolArgs};
use gaze_lens::value::{LensRow, LensValue};
use rmcp::model::ErrorCode;

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

fn pipeline() -> gaze::Pipeline {
    gaze::Pipeline::builder()
        .rule(DefaultRule::new(Action::Preserve))
        .build()
        .expect("pipeline")
}

fn session(temp: &tempfile::TempDir) -> Arc<Session> {
    Arc::new(
        Session::new_for_multi_profile(
            &policy(),
            &temp.path().join("manifest.sqlite"),
            &temp.path().join("snapshots"),
        )
        .expect("session"),
    )
}

fn query_args(profile: &str) -> serde_json::Value {
    serde_json::json!({
        "profile": profile,
        "table": "users",
        "columns": ["email"],
        "limit": 1
    })
}

fn count_calls(temp: &tempfile::TempDir) -> i64 {
    let conn = rusqlite::Connection::open(temp.path().join("manifest.sqlite")).expect("manifest");
    conn.query_row("SELECT COUNT(*) FROM calls", [], |row| row.get(0))
        .expect("count")
}

fn register_db_profile(session: &Session, profile: &str, touched: Arc<AtomicUsize>) {
    session
        .register_pipeline(profile, Arc::new(pipeline()))
        .expect("pipeline");
    session.register_fake_source_for_profile(
        SourceClass::Database,
        profile,
        Box::new(CountingRowsSource { touched }),
    );
}

fn assert_error_code(err: &rmcp::model::ErrorData, code: ErrorCode) {
    assert_eq!(err.code, code, "message: {}", err.message);
}

#[tokio::test]
async fn source_init_failure_retries_on_next_call() {
    let temp = tempfile::tempdir().expect("tempdir");
    let session = session(&temp);
    session
        .register_pipeline("dev", Arc::new(pipeline()))
        .expect("pipeline");
    let attempts = Arc::new(AtomicUsize::new(0));
    let builder: SourceBuilder = Arc::new({
        let attempts = attempts.clone();
        move || {
            let attempts = attempts.clone();
            Box::pin(async move {
                let attempt = attempts.fetch_add(1, Ordering::SeqCst);
                if attempt == 0 {
                    return Err(LensError::SourceError {
                        source_name: "db".to_string(),
                        detail: "first connect failed with raw-password=secret".to_string(),
                        sql: None,
                        stderr: None,
                    });
                }
                Ok(
                    Arc::new(FakeSourceAdapter::new(Box::new(CountingRowsSource {
                        touched: Arc::new(AtomicUsize::new(0)),
                    }))) as Arc<dyn gaze_lens::source::Source>,
                )
            })
        }
    });
    session.register_source_lazy(SourceClass::Database, "dev", builder);
    let frontend = McpFrontend::with_session(session);

    let first = frontend
        .call_tool_result_for_test("query", query_args("dev"))
        .await
        .expect_err("first call fails");
    assert_error_code(&first, ErrorCode::INTERNAL_ERROR);
    assert!(
        !first.message.contains("raw-password=secret"),
        "{}",
        first.message
    );

    frontend
        .call_tool_result_for_test("query", query_args("dev"))
        .await
        .expect("second call retries and succeeds");
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn unknown_profile_is_invalid_params_without_manifest_or_source_touch() {
    let temp = tempfile::tempdir().expect("tempdir");
    let session = session(&temp);
    let touched = Arc::new(AtomicUsize::new(0));
    register_db_profile(&session, "dev", touched.clone());
    let frontend = McpFrontend::with_session(session);

    let err = frontend
        .call_tool_result_for_test("query", query_args("missing"))
        .await
        .expect_err("unknown profile");
    assert_error_code(&err, ErrorCode::INVALID_PARAMS);
    assert!(err.message.contains("missing"), "{}", err.message);
    assert!(err.message.contains("dev"), "{}", err.message);
    assert_eq!(touched.load(Ordering::SeqCst), 0);
    assert_eq!(count_calls(&temp), 0);
}

#[tokio::test]
async fn empty_profile_and_non_object_args_are_invalid_params() {
    let temp = tempfile::tempdir().expect("tempdir");
    let session = session(&temp);
    register_db_profile(&session, "dev", Arc::new(AtomicUsize::new(0)));
    let frontend = McpFrontend::with_session(session);

    let empty = frontend
        .call_tool_result_for_test("query", query_args(""))
        .await
        .expect_err("empty profile");
    assert_error_code(&empty, ErrorCode::INVALID_PARAMS);
    assert!(
        empty.message.contains("profile required"),
        "{}",
        empty.message
    );

    let non_object = frontend
        .call_tool_result_for_test("query", serde_json::json!("dev"))
        .await
        .expect_err("non-object args");
    assert_error_code(&non_object, ErrorCode::INVALID_PARAMS);
    assert!(
        non_object.message.contains("args must be a JSON object"),
        "{}",
        non_object.message
    );
    assert_eq!(count_calls(&temp), 0);
}

#[tokio::test]
async fn source_class_mismatch_is_invalid_params() {
    let temp = tempfile::tempdir().expect("tempdir");
    let session = session(&temp);
    session
        .register_pipeline("logs", Arc::new(pipeline()))
        .expect("pipeline");
    session.register_fake_source_for_profile(
        SourceClass::Log,
        "logs",
        Box::new(CountingRowsSource {
            touched: Arc::new(AtomicUsize::new(0)),
        }),
    );
    let frontend = McpFrontend::with_session(session);

    let err = frontend
        .call_tool_result_for_test("query", query_args("logs"))
        .await
        .expect_err("class mismatch");
    assert_error_code(&err, ErrorCode::INVALID_PARAMS);
    assert!(err.message.contains("logs"), "{}", err.message);
    assert!(err.message.contains("log source"), "{}", err.message);
    assert!(err.message.contains("database profile"), "{}", err.message);
}

#[tokio::test]
async fn source_failure_is_sanitized_internal_error() {
    let temp = tempfile::tempdir().expect("tempdir");
    let session = session(&temp);
    session
        .register_pipeline("dev", Arc::new(pipeline()))
        .expect("pipeline");
    session.register_fake_source_for_profile(
        SourceClass::Database,
        "dev",
        Box::new(FailingSource {
            detail: "driver dumped raw token password=super-secret".to_string(),
        }),
    );
    let frontend = McpFrontend::with_session(session);

    let err = frontend
        .call_tool_result_for_test("query", query_args("dev"))
        .await
        .expect_err("source failure");
    assert_error_code(&err, ErrorCode::INTERNAL_ERROR);
    assert_eq!(err.message, "SourceError: source failed");
    assert!(
        !err.message.contains("super-secret"),
        "raw source detail leaked: {}",
        err.message
    );
}

struct CountingRowsSource {
    touched: Arc<AtomicUsize>,
}

#[async_trait]
impl FakeSource for CountingRowsSource {
    async fn invoke(&self, _args: &ToolArgs) -> Result<SourceOutput, LensError> {
        self.touched.fetch_add(1, Ordering::SeqCst);
        let mut row: LensRow = BTreeMap::new();
        row.insert(
            "email".to_string(),
            LensValue::String("alice@example.com".to_string()),
        );
        Ok(SourceOutput::Rows(vec![row]))
    }
}

struct FailingSource {
    detail: String,
}

#[async_trait]
impl FakeSource for FailingSource {
    async fn invoke(&self, _args: &ToolArgs) -> Result<SourceOutput, LensError> {
        Err(LensError::SourceError {
            source_name: "db".to_string(),
            detail: self.detail.clone(),
            sql: None,
            stderr: None,
        })
    }
}
