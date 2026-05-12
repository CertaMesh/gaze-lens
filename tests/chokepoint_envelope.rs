use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use gaze_lens::errors::LensError;
use gaze_lens::session::{Session, ToolCall};
use gaze_lens::source::{FakeSource, SourceOutput, ToolArgs};
use gaze_lens::value::LensValue;
use rusqlite::Connection;

fn policy() -> gaze::Policy {
    let mut policy = gaze::Policy::default();
    policy.session.scope = gaze::SessionScope::Conversation;
    policy.rulepacks.bundled = vec!["core".to_string()];
    policy
}

struct ChokepointSource {
    manifest_path: PathBuf,
    events: Arc<Mutex<Vec<&'static str>>>,
}

#[async_trait]
impl FakeSource for ChokepointSource {
    async fn invoke(&self, args: &ToolArgs) -> Result<SourceOutput, LensError> {
        let conn = Connection::open(&self.manifest_path).expect("manifest");
        let (status, redacted_args): (String, String) = conn
            .query_row(
                "SELECT status, redacted_args_json FROM calls WHERE tool_name = 'query'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("started manifest row");

        assert_eq!(status, "in-progress");
        assert!(!redacted_args.contains("alice@example.com"));
        assert!(redacted_args.contains("<"));
        assert!(redacted_args.contains(">"));
        assert!(args.0.to_string().contains("alice@example.com"));
        self.events.lock().expect("events").push("source");

        Ok(SourceOutput::Rows(vec![BTreeMap::from([(
            "email".to_string(),
            LensValue::String("bob@example.com".to_string()),
        )])]))
    }
}

#[tokio::test]
async fn query_dispatch_runs_through_envelope_and_manifest_chokepoint() {
    let temp = tempfile::tempdir().expect("tempdir");
    let manifest_path = temp.path().join("manifest.sqlite");
    let snapshot_dir = temp.path().join("snapshots");
    let events = Arc::new(Mutex::new(Vec::new()));
    let session = Session::new(&policy(), &manifest_path, &snapshot_dir).expect("session");
    session.register_fake_source(
        "query",
        Box::new(ChokepointSource {
            manifest_path: manifest_path.clone(),
            events: events.clone(),
        }),
    );

    let result = session
        .dispatch_tool(ToolCall {
            call_id: ulid::Ulid::new().to_string(),
            tool_name: "query".to_string(),
            args: ToolArgs(serde_json::json!({
                "profile": "default",
                "filter": { "email": "alice@example.com" }
            })),
        })
        .await
        .expect("dispatch");

    assert_eq!(*events.lock().expect("events"), vec!["source"]);
    let clean = serde_json::to_string(&result.clean).expect("clean json");
    assert!(!clean.contains("bob@example.com"));
    assert!(clean.contains("<"));
    assert!(clean.contains(">"));

    let conn = Connection::open(&manifest_path).expect("manifest");
    let (tool_name, status, redacted_args, result_summary, snapshot_ref): (
        String,
        String,
        String,
        String,
        String,
    ) = conn
        .query_row(
            "SELECT tool_name, status, redacted_args_json, result_summary, snapshot_ref FROM calls",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .expect("finished manifest row");
    let summary: serde_json::Value = serde_json::from_str(&result_summary).expect("summary json");

    assert_eq!(tool_name, "query");
    assert_eq!(status, "ok");
    assert!(!redacted_args.contains("alice@example.com"));
    assert!(redacted_args.contains("<"));
    assert!(redacted_args.contains(">"));
    assert_eq!(summary["rows"], 1);
    assert!(std::path::Path::new(&snapshot_ref).exists());
}
