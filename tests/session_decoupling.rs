use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use gaze_lens::errors::LensError;
use gaze_lens::session::{Session, ToolCall};
use gaze_lens::source::{FakeSource, SourceOutput, ToolArgs};
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

struct FrontendHandle;

impl Drop for FrontendHandle {
    fn drop(&mut self) {}
}

struct DirectSource {
    calls: Arc<Mutex<usize>>,
}

#[async_trait]
impl FakeSource for DirectSource {
    async fn invoke(&self, _args: &ToolArgs) -> Result<SourceOutput, LensError> {
        *self.calls.lock().expect("calls") += 1;
        Ok(SourceOutput::Rows(vec![BTreeMap::from([(
            "email".to_string(),
            LensValue::String("alice@example.com".to_string()),
        )])]))
    }
}

#[tokio::test]
async fn session_constructs_and_dispatches_without_frontend() {
    let temp = tempfile::tempdir().expect("tempdir");
    let calls = Arc::new(Mutex::new(0));
    let session = Session::new(
        &policy(),
        &temp.path().join("manifest.sqlite"),
        &temp.path().join("snapshots"),
    )
    .expect("session");
    session.register_fake_source(
        "fake",
        Box::new(DirectSource {
            calls: calls.clone(),
        }),
    );

    session
        .dispatch_tool(ToolCall {
            call_id: ulid::Ulid::new().to_string(),
            tool_name: "fake".to_string(),
            args: ToolArgs(serde_json::json!({"email": "alice@example.com"})),
        })
        .await
        .expect("dispatch");

    assert_eq!(*calls.lock().expect("calls"), 1);
}

#[tokio::test]
async fn dropping_frontend_handle_does_not_drop_session_state() {
    let temp = tempfile::tempdir().expect("tempdir");
    let calls = Arc::new(Mutex::new(0));
    let session = Session::new(
        &policy(),
        &temp.path().join("manifest.sqlite"),
        &temp.path().join("snapshots"),
    )
    .expect("session");
    session.register_fake_source(
        "fake",
        Box::new(DirectSource {
            calls: calls.clone(),
        }),
    );
    let frontend = FrontendHandle;
    drop(frontend);

    session
        .dispatch_tool(ToolCall {
            call_id: ulid::Ulid::new().to_string(),
            tool_name: "fake".to_string(),
            args: ToolArgs(serde_json::json!({"email": "alice@example.com"})),
        })
        .await
        .expect("dispatch after frontend drop");

    assert_eq!(*calls.lock().expect("calls"), 1);
}

#[test]
fn session_constructs_multiple_ways_without_frontend_imports() {
    let temp = tempfile::tempdir().expect("tempdir");
    let one = Session::new(
        &policy(),
        &temp.path().join("one.sqlite"),
        &temp.path().join("one-snaps"),
    );
    let two = Session::new(
        &policy(),
        &temp.path().join("two.sqlite"),
        &temp.path().join("two-snaps"),
    );

    assert!(one.is_ok());
    assert!(two.is_ok());
}
