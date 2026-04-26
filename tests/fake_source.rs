use std::collections::BTreeMap;

use gaze_lens::source::{FakeSource, InMemoryFakeSource, SourceOutput, ToolArgs};
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
        SourceOutput::Text(_) => panic!("expected rows"),
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
        SourceOutput::Rows(_) => panic!("expected text"),
    }
}
