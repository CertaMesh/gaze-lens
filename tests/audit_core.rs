use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use gaze_lens::errors::LensError;
use gaze_lens::session::manifest::{ManifestStore, ManifestWriter, SnapshotRef};
use gaze_lens::session::{
    CleanOutput, OutputCaps, RedactedToolArgs, ResultSummary, Session, ToolCall, TruncatedAt,
};
use gaze_lens::source::{FakeSource, SourceOutput, ToolArgs};
use gaze_lens::value::{LensRow, LensValue};
use rusqlite::Connection;

fn policy(scope: gaze::SessionScope) -> gaze::Policy {
    let mut policy = gaze::Policy::default();
    policy.session.scope = scope;
    policy.rulepacks.bundled = vec!["core".to_string()];
    policy
}

fn call(args: serde_json::Value) -> ToolCall {
    ToolCall {
        call_id: ulid::Ulid::new().to_string(),
        tool_name: "fake".to_string(),
        args: ToolArgs(args),
    }
}

fn row_with_email(email: &str) -> LensRow {
    BTreeMap::from([("email".to_string(), LensValue::String(email.to_string()))])
}

struct RecordingManifest {
    events: Arc<Mutex<Vec<&'static str>>>,
    fail_begin: bool,
    fail_finish: bool,
}

impl ManifestStore for RecordingManifest {
    fn begin_call(
        &self,
        call: &ToolCall,
        _redacted_args: &RedactedToolArgs,
    ) -> Result<(), LensError> {
        self.events.lock().expect("events").push("begin");
        if self.fail_begin {
            return Err(LensError::ManifestBeginFailed {
                call_id: call.call_id.clone(),
                detail: "begin failed near alice@example.com".to_string(),
                path: None,
            });
        }
        Ok(())
    }

    fn finish_call(
        &self,
        call_id: &str,
        _summary: &ResultSummary,
        _snapshot_ref: &SnapshotRef,
    ) -> Result<(), LensError> {
        self.events.lock().expect("events").push("finish");
        if self.fail_finish {
            return Err(LensError::ManifestFinishFailed {
                call_id: call_id.to_string(),
                detail: "finish failed near alice@example.com".to_string(),
                path: None,
            });
        }
        Ok(())
    }

    fn fail_call(&self, _call_id: &str, _err: &LensError) -> Result<(), LensError> {
        self.events.lock().expect("events").push("fail");
        Ok(())
    }
}

struct RecordingSource {
    events: Arc<Mutex<Vec<&'static str>>>,
    rows: Vec<LensRow>,
}

#[async_trait]
impl FakeSource for RecordingSource {
    async fn invoke(&self, _args: &ToolArgs) -> Result<SourceOutput, LensError> {
        self.events.lock().expect("events").push("source");
        Ok(SourceOutput::Rows(self.rows.clone()))
    }
}

struct TextSource {
    text: String,
}

#[async_trait]
impl FakeSource for TextSource {
    async fn invoke(&self, _args: &ToolArgs) -> Result<SourceOutput, LensError> {
        Ok(SourceOutput::Text(self.text.clone()))
    }
}

struct TruncatedTextSource {
    text: String,
    truncated_at: Vec<TruncatedAt>,
}

#[async_trait]
impl FakeSource for TruncatedTextSource {
    async fn invoke(&self, _args: &ToolArgs) -> Result<SourceOutput, LensError> {
        Ok(SourceOutput::TextWithTruncation {
            text: self.text.clone(),
            truncated_at: self.truncated_at.clone(),
        })
    }
}

struct SlowSource;

#[async_trait]
impl FakeSource for SlowSource {
    async fn invoke(&self, _args: &ToolArgs) -> Result<SourceOutput, LensError> {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        Ok(SourceOutput::Rows(vec![row_with_email(
            "alice@example.com",
        )]))
    }
}

fn manifest_summary(path: &std::path::Path) -> serde_json::Value {
    let summary: String = Connection::open(path)
        .expect("manifest")
        .query_row("SELECT result_summary FROM calls", [], |row| row.get(0))
        .expect("summary");
    serde_json::from_str(&summary).expect("summary json")
}

#[tokio::test]
async fn begin_happens_before_source_access() {
    let temp = tempfile::tempdir().expect("tempdir");
    let events = Arc::new(Mutex::new(Vec::new()));
    let manifest = Arc::new(RecordingManifest {
        events: events.clone(),
        fail_begin: false,
        fail_finish: false,
    });
    let session = Session::new_with_manifest_for_tests(
        &policy(gaze::SessionScope::Conversation),
        manifest,
        temp.path(),
        OutputCaps::default(),
    )
    .expect("session");
    session.register_fake_source(
        "fake",
        Box::new(RecordingSource {
            events: events.clone(),
            rows: vec![row_with_email("alice@example.com")],
        }),
    );

    session
        .dispatch_tool(call(serde_json::json!({"email": "alice@example.com"})))
        .await
        .expect("dispatch");

    assert_eq!(
        events.lock().expect("events").as_slice(),
        ["begin", "source", "finish"]
    );
}

#[tokio::test]
async fn begin_failure_prevents_source_access() {
    let temp = tempfile::tempdir().expect("tempdir");
    let events = Arc::new(Mutex::new(Vec::new()));
    let manifest = Arc::new(RecordingManifest {
        events: events.clone(),
        fail_begin: true,
        fail_finish: false,
    });
    let session = Session::new_with_manifest_for_tests(
        &policy(gaze::SessionScope::Conversation),
        manifest,
        temp.path(),
        OutputCaps::default(),
    )
    .expect("session");
    session.register_fake_source(
        "fake",
        Box::new(RecordingSource {
            events: events.clone(),
            rows: vec![row_with_email("alice@example.com")],
        }),
    );

    let err = session
        .dispatch_tool(call(serde_json::json!({"email": "alice@example.com"})))
        .await
        .expect_err("dispatch should fail closed");

    assert!(matches!(err, LensError::ManifestBeginFailed { .. }));
    assert_eq!(events.lock().expect("events").as_slice(), ["begin"]);
}

#[tokio::test]
async fn finish_failure_returns_error_without_tool_result() {
    let temp = tempfile::tempdir().expect("tempdir");
    let events = Arc::new(Mutex::new(Vec::new()));
    let manifest = Arc::new(RecordingManifest {
        events: events.clone(),
        fail_begin: false,
        fail_finish: true,
    });
    let session = Session::new_with_manifest_for_tests(
        &policy(gaze::SessionScope::Conversation),
        manifest,
        temp.path(),
        OutputCaps::default(),
    )
    .expect("session");
    session.register_fake_source(
        "fake",
        Box::new(RecordingSource {
            events: events.clone(),
            rows: vec![row_with_email("alice@example.com")],
        }),
    );

    let err = session
        .dispatch_tool(call(serde_json::json!({"email": "alice@example.com"})))
        .await
        .expect_err("finish failure must fail closed");

    assert!(matches!(err, LensError::ManifestFinishFailed { .. }));
    assert_eq!(
        events.lock().expect("events").as_slice(),
        ["begin", "source", "finish"]
    );
}

#[tokio::test]
async fn raw_args_are_redacted_before_manifest_write() {
    let temp = tempfile::tempdir().expect("tempdir");
    let manifest_path = temp.path().join("manifest.sqlite");
    let snapshot_dir = temp.path().join("snapshots");
    let session = Session::new(
        &policy(gaze::SessionScope::Conversation),
        &manifest_path,
        &snapshot_dir,
    )
    .expect("session");
    session.register_fake_source(
        "fake",
        Box::new(RecordingSource {
            events: Arc::new(Mutex::new(Vec::new())),
            rows: vec![row_with_email("bob@example.com")],
        }),
    );

    session
        .dispatch_tool(call(serde_json::json!({"grep": "alice@example.com"})))
        .await
        .expect("dispatch");

    let stored: String = Connection::open(&manifest_path)
        .expect("manifest")
        .query_row("SELECT redacted_args_json FROM calls", [], |row| row.get(0))
        .expect("redacted args");
    assert!(!stored.contains("alice@example.com"));
    assert!(stored.contains("<"));
    assert!(stored.contains(">"));
}

#[test]
fn ephemeral_scope_is_rejected_at_construction() {
    let temp = tempfile::tempdir().expect("tempdir");
    let result = Session::new(
        &policy(gaze::SessionScope::Ephemeral),
        &temp.path().join("manifest.sqlite"),
        &temp.path().join("snapshots"),
    );
    let err = match result {
        Ok(_) => panic!("ephemeral should be rejected"),
        Err(err) => err,
    };

    assert!(matches!(err, LensError::ScopeRejected { .. }));
}

#[tokio::test]
async fn output_caps_truncate_rows_and_manifest_summary_records_it() {
    let temp = tempfile::tempdir().expect("tempdir");
    let manifest_path = temp.path().join("manifest.sqlite");
    let snapshot_dir = temp.path().join("snapshots");
    let lens_id = ulid::Ulid::new();
    let gaze_session =
        gaze::Session::new(gaze::Scope::Conversation(lens_id.to_string())).expect("gaze");
    let manifest = ManifestWriter::new(&manifest_path, lens_id, gaze_session.audit_session_id())
        .expect("manifest");
    let session = Session::new_with_manifest_for_tests(
        &policy(gaze::SessionScope::Conversation),
        Arc::new(manifest),
        &snapshot_dir,
        OutputCaps {
            rows: 5,
            ..OutputCaps::default()
        },
    )
    .expect("session");
    let rows = (0..100)
        .map(|index| row_with_email(&format!("user{index}@example.com")))
        .collect::<Vec<_>>();
    session.register_fake_source(
        "fake",
        Box::new(RecordingSource {
            events: Arc::new(Mutex::new(Vec::new())),
            rows,
        }),
    );

    let result = session
        .dispatch_tool(call(serde_json::json!({"email": "alice@example.com"})))
        .await
        .expect("dispatch");

    match result.clean {
        CleanOutput::Rows { rows, truncated_at } => {
            assert_eq!(rows.len(), 5);
            assert_eq!(truncated_at, vec![TruncatedAt::Rows]);
        }
        CleanOutput::Text { .. } => panic!("expected rows"),
    }

    let summary = manifest_summary(&manifest_path);
    assert_eq!(summary["truncated_at"], serde_json::json!(["Rows"]));
}

#[tokio::test]
async fn output_caps_truncate_total_bytes_and_manifest_summary_records_it() {
    let temp = tempfile::tempdir().expect("tempdir");
    let manifest_path = temp.path().join("manifest.sqlite");
    let snapshot_dir = temp.path().join("snapshots");
    let manifest =
        ManifestWriter::new(&manifest_path, ulid::Ulid::new(), "test-audit").expect("manifest");
    let session = Session::new_with_manifest_for_tests(
        &policy(gaze::SessionScope::Conversation),
        Arc::new(manifest),
        &snapshot_dir,
        OutputCaps {
            bytes: 100,
            ..OutputCaps::default()
        },
    )
    .expect("session");
    let rows = (0..50)
        .map(|index| {
            BTreeMap::from([(
                "value".to_string(),
                LensValue::String(format!("abcdefghij{index}")),
            )])
        })
        .collect::<Vec<_>>();
    session.register_fake_source(
        "fake",
        Box::new(RecordingSource {
            events: Arc::new(Mutex::new(Vec::new())),
            rows,
        }),
    );

    let result = session
        .dispatch_tool(call(serde_json::json!({})))
        .await
        .expect("dispatch");

    match result.clean {
        CleanOutput::Rows { rows, truncated_at } => {
            assert!(rows.len() < 50);
            assert!(truncated_at.contains(&TruncatedAt::Bytes));
        }
        CleanOutput::Text { .. } => panic!("expected rows"),
    }
    let summary = manifest_summary(&manifest_path);
    assert_eq!(summary["truncated_at"], serde_json::json!(["Bytes"]));
}

#[tokio::test]
async fn output_caps_replace_large_cells_and_manifest_summary_records_it() {
    let temp = tempfile::tempdir().expect("tempdir");
    let manifest_path = temp.path().join("manifest.sqlite");
    let snapshot_dir = temp.path().join("snapshots");
    let manifest =
        ManifestWriter::new(&manifest_path, ulid::Ulid::new(), "test-audit").expect("manifest");
    let session = Session::new_with_manifest_for_tests(
        &policy(gaze::SessionScope::Conversation),
        Arc::new(manifest),
        &snapshot_dir,
        OutputCaps {
            cell_bytes: 20,
            ..OutputCaps::default()
        },
    )
    .expect("session");
    session.register_fake_source(
        "fake",
        Box::new(RecordingSource {
            events: Arc::new(Mutex::new(Vec::new())),
            rows: vec![BTreeMap::from([(
                "value".to_string(),
                LensValue::String("x".repeat(200)),
            )])],
        }),
    );

    let result = session
        .dispatch_tool(call(serde_json::json!({})))
        .await
        .expect("dispatch");

    match result.clean {
        CleanOutput::Rows { rows, truncated_at } => {
            assert_eq!(rows[0]["value"], "<TRUNCATED:cell_bytes>");
            assert!(truncated_at.contains(&TruncatedAt::CellBytes));
        }
        CleanOutput::Text { .. } => panic!("expected rows"),
    }
    let summary = manifest_summary(&manifest_path);
    assert_eq!(summary["truncated_at"], serde_json::json!(["CellBytes"]));
}

#[tokio::test]
async fn output_caps_truncate_text_bytes_and_manifest_summary_records_it() {
    let temp = tempfile::tempdir().expect("tempdir");
    let manifest_path = temp.path().join("manifest.sqlite");
    let snapshot_dir = temp.path().join("snapshots");
    let manifest =
        ManifestWriter::new(&manifest_path, ulid::Ulid::new(), "test-audit").expect("manifest");
    let session = Session::new_with_manifest_for_tests(
        &policy(gaze::SessionScope::Conversation),
        Arc::new(manifest),
        &snapshot_dir,
        OutputCaps {
            bytes: 100,
            ..OutputCaps::default()
        },
    )
    .expect("session");
    session.register_fake_source(
        "fake",
        Box::new(TextSource {
            text: "x".repeat(500),
        }),
    );

    let result = session
        .dispatch_tool(call(serde_json::json!({})))
        .await
        .expect("dispatch");

    match result.clean {
        CleanOutput::Text { text, truncated_at } => {
            assert_eq!(text.len(), 100);
            assert!(truncated_at.contains(&TruncatedAt::Bytes));
        }
        CleanOutput::Rows { .. } => panic!("expected text"),
    }
    let summary = manifest_summary(&manifest_path);
    assert_eq!(summary["truncated_at"], serde_json::json!(["Bytes"]));
}

#[tokio::test]
async fn source_text_byte_truncation_is_recorded_in_manifest_summary() {
    let temp = tempfile::tempdir().expect("tempdir");
    let manifest_path = temp.path().join("manifest.sqlite");
    let snapshot_dir = temp.path().join("snapshots");
    let manifest =
        ManifestWriter::new(&manifest_path, ulid::Ulid::new(), "test-audit").expect("manifest");
    let session = Session::new_with_manifest_for_tests(
        &policy(gaze::SessionScope::Conversation),
        Arc::new(manifest),
        &snapshot_dir,
        OutputCaps {
            bytes: 100,
            ..OutputCaps::default()
        },
    )
    .expect("session");
    session.register_fake_source(
        "fake",
        Box::new(TruncatedTextSource {
            text: "x".repeat(100),
            truncated_at: vec![TruncatedAt::Bytes],
        }),
    );

    let result = session
        .dispatch_tool(call(serde_json::json!({})))
        .await
        .expect("dispatch");

    match result.clean {
        CleanOutput::Text { text, truncated_at } => {
            assert_eq!(text.len(), 100);
            assert!(truncated_at.contains(&TruncatedAt::Bytes));
        }
        CleanOutput::Rows { .. } => panic!("expected text"),
    }
    let summary = manifest_summary(&manifest_path);
    assert_eq!(summary["truncated_at"], serde_json::json!(["Bytes"]));
}

#[tokio::test]
async fn output_caps_timeout_records_manifest_without_raw_values() {
    let temp = tempfile::tempdir().expect("tempdir");
    let manifest_path = temp.path().join("manifest.sqlite");
    let snapshot_dir = temp.path().join("snapshots");
    let manifest =
        ManifestWriter::new(&manifest_path, ulid::Ulid::new(), "test-audit").expect("manifest");
    let session = Session::new_with_manifest_for_tests(
        &policy(gaze::SessionScope::Conversation),
        Arc::new(manifest),
        &snapshot_dir,
        OutputCaps {
            timeout: std::time::Duration::from_millis(100),
            ..OutputCaps::default()
        },
    )
    .expect("session");
    session.register_fake_source("fake", Box::new(SlowSource));

    let err = session
        .dispatch_tool(call(serde_json::json!({"email": "arg@example.com"})))
        .await
        .expect_err("dispatch should timeout");

    assert!(matches!(err, LensError::Truncated(TruncatedAt::Timeout)));
    let summary = manifest_summary(&manifest_path);
    assert_eq!(summary["truncated_at"], serde_json::json!(["Timeout"]));
    let stored: String = Connection::open(&manifest_path)
        .expect("manifest")
        .query_row(
            "SELECT redacted_args_json || COALESCE(result_summary, '') FROM calls",
            [],
            |row| row.get(0),
        )
        .expect("stored");
    assert!(!stored.contains("arg@example.com"));
    assert!(!stored.contains("alice@example.com"));
}

#[tokio::test]
async fn dispatch_with_nested_json_args_redacted() {
    let temp = tempfile::tempdir().expect("tempdir");
    let manifest_path = temp.path().join("manifest.sqlite");
    let snapshot_dir = temp.path().join("snapshots");
    let session = Session::new(
        &policy(gaze::SessionScope::Conversation),
        &manifest_path,
        &snapshot_dir,
    )
    .expect("session");
    session.register_fake_source(
        "fake",
        Box::new(RecordingSource {
            events: Arc::new(Mutex::new(Vec::new())),
            rows: vec![row_with_email("bob@example.com")],
        }),
    );

    session
        .dispatch_tool(call(serde_json::json!({
            "filter": { "email": "alice@example.com" }
        })))
        .await
        .expect("dispatch");

    let stored: String = Connection::open(&manifest_path)
        .expect("manifest")
        .query_row("SELECT redacted_args_json FROM calls", [], |row| row.get(0))
        .expect("redacted args");
    assert!(!stored.contains("alice@example.com"));
    assert!(stored.contains("<"));
    assert!(stored.contains(">"));
}

#[tokio::test]
async fn dispatch_with_bytes_preserves_base64_metadata() {
    let temp = tempfile::tempdir().expect("tempdir");
    let manifest_path = temp.path().join("manifest.sqlite");
    let snapshot_dir = temp.path().join("snapshots");
    let session = Session::new(
        &policy(gaze::SessionScope::Conversation),
        &manifest_path,
        &snapshot_dir,
    )
    .expect("session");
    session.register_fake_source(
        "fake",
        Box::new(RecordingSource {
            events: Arc::new(Mutex::new(Vec::new())),
            rows: vec![BTreeMap::from([(
                "payload".to_string(),
                LensValue::Bytes {
                    base64: "aGVsbG8=".to_string(),
                    len: 5,
                },
            )])],
        }),
    );

    let result = session
        .dispatch_tool(call(serde_json::json!({})))
        .await
        .expect("dispatch");

    match result.clean {
        CleanOutput::Rows { rows, .. } => {
            assert_eq!(rows[0]["payload"]["type"], "bytes");
            assert_eq!(rows[0]["payload"]["base64"], "aGVsbG8=");
            assert_eq!(rows[0]["payload"]["len"], 5);
        }
        CleanOutput::Text { .. } => panic!("expected rows"),
    }
}

#[cfg(unix)]
#[tokio::test]
async fn snapshot_file_and_dir_permissions_are_private() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().expect("tempdir");
    let manifest_path = temp.path().join("manifest.sqlite");
    let snapshot_dir = temp.path().join("snapshots");
    let session = Session::new(
        &policy(gaze::SessionScope::Conversation),
        &manifest_path,
        &snapshot_dir,
    )
    .expect("session");
    session.register_fake_source(
        "fake",
        Box::new(RecordingSource {
            events: Arc::new(Mutex::new(Vec::new())),
            rows: vec![row_with_email("alice@example.com")],
        }),
    );

    let result = session
        .dispatch_tool(call(serde_json::json!({"email": "alice@example.com"})))
        .await
        .expect("dispatch");

    assert_eq!(
        std::fs::metadata(&snapshot_dir)
            .expect("snapshot dir")
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(&result.snapshot_ref.path)
            .expect("snapshot")
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}
