use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use gaze::{Action, ClassRule, DefaultRule};
use gaze_lens::errors::LensError;
use gaze_lens::session::restore::restore_whole_session;
use gaze_lens::session::{Session, SourceBuilder, SourceClass, ToolCall};
use gaze_lens::source::{FakeSource, FakeSourceAdapter, SourceOutput, ToolArgs};
use gaze_lens::value::{LensRow, LensValue};
use gaze_recognizers::RegexDetector;

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
        .detector(RegexDetector::emails().expect("email detector"))
        .rule(ClassRule::new(gaze::PiiClass::Email, Action::Tokenize))
        .rule(DefaultRule::new(Action::Preserve))
        .build()
        .expect("pipeline")
}

fn call(tool: &str, profile: &str) -> ToolCall {
    ToolCall {
        call_id: ulid::Ulid::new().to_string(),
        tool_name: tool.to_string(),
        args: ToolArgs(serde_json::json!({
            "profile": profile,
            "table": "users",
            "columns": ["email"],
            "limit": 1
        })),
    }
}

fn call_with_filter(tool: &str, profile: &str, email: &str) -> ToolCall {
    ToolCall {
        call_id: ulid::Ulid::new().to_string(),
        tool_name: tool.to_string(),
        args: ToolArgs(serde_json::json!({
            "profile": profile,
            "table": "users",
            "columns": ["email"],
            "where": [{"col": "email", "op": "eq", "val": email}],
            "limit": 1
        })),
    }
}

#[tokio::test]
async fn shared_gaze_session_yields_consistent_tokens_across_profiles() {
    let temp = tempfile::tempdir().expect("tempdir");
    let session = Arc::new(
        Session::new_for_multi_profile(
            &policy(),
            &temp.path().join("manifest.sqlite"),
            &temp.path().join("snapshots"),
        )
        .expect("session"),
    );
    session
        .register_pipeline("a", Arc::new(pipeline()))
        .expect("pipeline a");
    session
        .register_pipeline("b", Arc::new(pipeline()))
        .expect("pipeline b");
    session.register_fake_source_for_profile(
        SourceClass::Database,
        "a",
        Box::new(RowsSource("alice@example.com")),
    );
    session.register_fake_source_for_profile(
        SourceClass::Database,
        "b",
        Box::new(RowsSource("alice@example.com")),
    );

    let a = session.dispatch_tool(call("query", "a")).await.expect("a");
    let b = session.dispatch_tool(call("query", "b")).await.expect("b");
    let encoded_a = serde_json::to_string(&a.clean).expect("json a");
    let encoded_b = serde_json::to_string(&b.clean).expect("json b");

    assert!(encoded_a.contains("Email_1"), "{encoded_a}");
    assert_eq!(encoded_a, encoded_b);
    assert_eq!(a.snapshot_ref.path, b.snapshot_ref.path);
}

#[tokio::test]
async fn concurrent_first_calls_across_db_tools_invoke_builder_once() {
    let temp = tempfile::tempdir().expect("tempdir");
    let session = Arc::new(
        Session::new_for_multi_profile(
            &policy(),
            &temp.path().join("manifest.sqlite"),
            &temp.path().join("snapshots"),
        )
        .expect("session"),
    );
    session
        .register_pipeline("a", Arc::new(pipeline()))
        .expect("pipeline");
    let counter = Arc::new(AtomicUsize::new(0));
    let builder: SourceBuilder = Arc::new({
        let counter = counter.clone();
        move || {
            let counter = counter.clone();
            Box::pin(async move {
                counter.fetch_add(1, Ordering::SeqCst);
                let source = FakeSourceAdapter::new(Box::new(RowsSource("alice@example.com")));
                Ok(Arc::new(source) as Arc<dyn gaze_lens::source::Source>)
            })
        }
    });
    session.register_source_lazy(SourceClass::Database, "a", builder);

    let h1 = tokio::spawn({
        let session = session.clone();
        async move { session.dispatch_tool(call("query", "a")).await }
    });
    let h2 = tokio::spawn({
        let session = session.clone();
        async move { session.dispatch_tool(call("schema", "a")).await }
    });
    let h3 = tokio::spawn({
        let session = session.clone();
        async move { session.dispatch_tool(call("list_tables", "a")).await }
    });

    let _ = tokio::try_join!(h1, h2, h3).expect("join");
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn whole_session_replay_restores_calls_across_profiles() {
    let temp = tempfile::tempdir().expect("tempdir");
    let manifest = temp.path().join("manifest.sqlite");
    let snapshots = temp.path().join("snapshots");
    let session = Arc::new(
        Session::new_for_multi_profile(&policy(), &manifest, &snapshots).expect("session"),
    );
    session
        .register_pipeline("a", Arc::new(pipeline()))
        .expect("pipeline a");
    session
        .register_pipeline("b", Arc::new(pipeline()))
        .expect("pipeline b");
    session.register_fake_source_for_profile(
        SourceClass::Database,
        "a",
        Box::new(RowsSource("alice@example.com")),
    );
    session.register_fake_source_for_profile(
        SourceClass::Database,
        "b",
        Box::new(RowsSource("bob@example.com")),
    );

    session
        .dispatch_tool(call_with_filter("query", "a", "alice@example.com"))
        .await
        .expect("profile a");
    session
        .dispatch_tool(call_with_filter("query", "b", "bob@example.com"))
        .await
        .expect("profile b");

    let restored = restore_whole_session(&manifest, &session.lens_session_id().to_string(), 0)
        .expect("whole-session replay");
    assert_eq!(restored.calls.len(), 2);
    let restored_args = restored
        .calls
        .iter()
        .map(|call| call.restored_args_json.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        restored_args.contains("alice@example.com"),
        "{restored_args}"
    );
    assert!(restored_args.contains("bob@example.com"), "{restored_args}");
    let snapshot_count = std::fs::read_dir(&snapshots)
        .expect("snapshots")
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "snap"))
        .count();
    assert_eq!(snapshot_count, 1, "one shared session snapshot");
}

struct RowsSource(&'static str);

#[async_trait]
impl FakeSource for RowsSource {
    async fn invoke(&self, _args: &ToolArgs) -> Result<SourceOutput, LensError> {
        let mut row: LensRow = BTreeMap::new();
        row.insert("email".to_string(), LensValue::String(self.0.to_string()));
        Ok(SourceOutput::Rows(vec![row]))
    }
}
