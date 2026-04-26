use std::collections::BTreeMap;

use gaze_lens::cli::serve::runtime_policy;
use gaze_lens::profile::{Profile, SourceSpec};
use gaze_lens::session::{Session, ToolCall};
use gaze_lens::source::{InMemoryFakeSource, ToolArgs};
use gaze_lens::value::LensValue;

#[tokio::test]
async fn serve_runtime_policy_column_rules_reach_session_pipeline() {
    let temp = tempfile::tempdir().expect("tempdir");
    let policy_path = temp.path().join("policy.toml");
    std::fs::write(
        &policy_path,
        r#"
        [policy.database]

        [[policy.database.columns]]
        column = "customer_email"
        class = "email"
        action = "tokenize"
        "#,
    )
    .expect("write policy");
    let profile = Profile {
        name: "test".to_string(),
        source: SourceSpec::SshLog {
            host: "example.test".to_string(),
            path: "/var/log/app.log".into(),
            ssh_host: None,
        },
        policy: Some(policy_path),
        schema_allowlist: None,
    };

    let (policy, pipeline) = runtime_policy(&profile).expect("runtime policy");
    let session = Session::new_with_pipeline(
        &policy,
        pipeline,
        &temp.path().join("manifest.sqlite"),
        &temp.path().join("snapshots"),
    )
    .expect("session");
    session.register_fake_source(
        "query",
        Box::new(InMemoryFakeSource::rows(vec![BTreeMap::from([(
            "customer_email".to_string(),
            LensValue::String("alice@example.com".to_string()),
        )])])),
    );

    let result = session
        .dispatch_tool(ToolCall {
            call_id: ulid::Ulid::new().to_string(),
            tool_name: "query".to_string(),
            args: ToolArgs(serde_json::json!({
                "table": "customers",
                "columns": ["customer_email"],
                "limit": 1
            })),
        })
        .await
        .expect("dispatch");

    let encoded = serde_json::to_string(&result.clean).expect("json");
    assert!(!encoded.contains("alice@example.com"));
    assert!(encoded.contains("Email_1"));
}
