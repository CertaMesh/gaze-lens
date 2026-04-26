use std::collections::BTreeMap;
use std::sync::Arc;

use gaze_lens::frontend::mcp::McpFrontend;
use gaze_lens::session::manifest::{ManifestStore, SnapshotRef};
use gaze_lens::session::{OutputCaps, RedactedToolArgs, ResultSummary, Session, ToolCall};
use gaze_lens::source::InMemoryFakeSource;
use gaze_lens::value::LensValue;

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

fn session_with_manifest(manifest: Arc<dyn ManifestStore>) -> Session {
    let temp = tempfile::tempdir().expect("tempdir");
    let snapshot_dir = temp.path().to_path_buf();
    let session = Session::new_with_manifest_for_tests(
        &policy(),
        manifest,
        &snapshot_dir,
        OutputCaps {
            rows: 5,
            ..OutputCaps::default()
        },
    )
    .expect("session");
    std::mem::forget(temp);
    session
}

#[test]
fn test_public_tool_set() {
    assert_eq!(
        McpFrontend::public_tool_names(),
        vec!["query", "schema", "list_tables", "log_tail", "log_grep"]
    );
}

#[tokio::test]
async fn test_log_tail_stub_returns_deferred() {
    let manifest = Arc::new(RecordingManifest::default());
    let session = Arc::new(session_with_manifest(manifest.clone()));
    let frontend = McpFrontend::with_session(session);

    let err = frontend
        .call_tool_json("log_tail", serde_json::json!({"lines": 10}))
        .await
        .expect_err("deferred");

    assert!(err.contains("FeatureDeferred"));
    assert_eq!(
        manifest.statuses.lock().expect("statuses").as_slice(),
        ["begin", "fail"]
    );
}

#[tokio::test]
async fn test_query_e2e_pseudonymized() {
    let manifest = Arc::new(RecordingManifest::default());
    let session = session_with_manifest(manifest.clone());
    session.register_fake_source(
        "query",
        Box::new(InMemoryFakeSource::rows(vec![BTreeMap::from([(
            "email".to_string(),
            LensValue::String("alice@example.com".to_string()),
        )])])),
    );
    let frontend = McpFrontend::with_session(Arc::new(session));

    let result = frontend
        .call_tool_json(
            "query",
            serde_json::json!({
                "table": "users",
                "columns": ["email"],
                "limit": 1
            }),
        )
        .await
        .expect("query");

    let rows = result["clean"]["Rows"]["rows"]
        .as_array()
        .or_else(|| result["clean"]["rows"].as_array())
        .expect("rows");
    let encoded = serde_json::to_string(rows).expect("json");
    assert!(!encoded.contains("alice@example.com"));
    assert!(encoded.contains("<"));
    let redacted_args = manifest
        .redacted_args
        .lock()
        .expect("redacted args")
        .last()
        .cloned()
        .expect("args");
    assert!(!redacted_args.contains("alice@example.com"));
    assert_eq!(
        manifest.statuses.lock().expect("statuses").as_slice(),
        ["begin", "finish"]
    );
}

#[derive(Default)]
struct RecordingManifest {
    statuses: std::sync::Mutex<Vec<&'static str>>,
    redacted_args: std::sync::Mutex<Vec<String>>,
}

impl ManifestStore for RecordingManifest {
    fn begin_call(
        &self,
        _call: &ToolCall,
        redacted_args: &RedactedToolArgs,
    ) -> Result<(), gaze_lens::errors::LensError> {
        self.statuses.lock().expect("statuses").push("begin");
        self.redacted_args
            .lock()
            .expect("args")
            .push(redacted_args.json.clone());
        Ok(())
    }

    fn finish_call(
        &self,
        _call_id: &str,
        _summary: &ResultSummary,
        _snapshot_ref: &SnapshotRef,
    ) -> Result<(), gaze_lens::errors::LensError> {
        self.statuses.lock().expect("statuses").push("finish");
        Ok(())
    }

    fn fail_call(
        &self,
        _call_id: &str,
        _err: &gaze_lens::errors::LensError,
    ) -> Result<(), gaze_lens::errors::LensError> {
        self.statuses.lock().expect("statuses").push("fail");
        Ok(())
    }
}
