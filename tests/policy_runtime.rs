use std::collections::BTreeMap;

use gaze_lens::cli::serve::runtime_policy;
use gaze_lens::profile::{Profile, SourceSpec};
use gaze_lens::session::maintenance::AutoPurge;
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
            path: "/var/log/app.log".to_string(),
        },
        policy: Some(policy_path),
        discovered_from_ssh_host: None,
        discovered_from_path: None,
        discovered_at: None,
        discovered_ssh_host_key_fingerprint: None,
        credential_class: None,
        schema_tokenize: None,
        schema_allowlist: None,
        production: false,
        snapshot_retention_days: None,
        auto_purge: AutoPurge::Off,
    };

    let (policy, pipeline, column_actions) = runtime_policy(&profile).expect("runtime policy");
    let session = Session::new_with_pipeline(
        &policy,
        pipeline,
        &temp.path().join("manifest.sqlite"),
        &temp.path().join("snapshots"),
    )
    .expect("session");
    session
        .register_column_action_policy("default", column_actions)
        .expect("column action policy");
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

#[tokio::test]
async fn configured_column_actions_apply_without_detector_match() {
    let temp = tempfile::tempdir().expect("tempdir");
    let policy_path = temp.path().join("policy.toml");
    std::fs::write(
        &policy_path,
        r#"
        [policy.database]

        [[policy.database.columns]]
        column = "name"
        class = "name"
        action = "tokenize"

        [[policy.database.columns]]
        column = "password"
        class = "secret"
        action = "redact"
        "#,
    )
    .expect("write policy");
    let profile = Profile {
        name: "test".to_string(),
        source: SourceSpec::SshLog {
            host: "example.test".to_string(),
            path: "/var/log/app.log".to_string(),
        },
        policy: Some(policy_path),
        discovered_from_ssh_host: None,
        discovered_from_path: None,
        discovered_at: None,
        discovered_ssh_host_key_fingerprint: None,
        credential_class: None,
        schema_tokenize: None,
        schema_allowlist: None,
        production: false,
        snapshot_retention_days: None,
        auto_purge: AutoPurge::Off,
    };

    let (policy, pipeline, column_actions) = runtime_policy(&profile).expect("runtime policy");
    let session = Session::new_with_pipeline(
        &policy,
        pipeline,
        &temp.path().join("manifest.sqlite"),
        &temp.path().join("snapshots"),
    )
    .expect("session");
    session
        .register_column_action_policy("default", column_actions)
        .expect("column action policy");
    session.register_fake_source(
        "query",
        Box::new(InMemoryFakeSource::rows(vec![BTreeMap::from([
            (
                "name".to_string(),
                LensValue::String("Sandorian User".to_string()),
            ),
            (
                "password".to_string(),
                LensValue::String("argon2id-secret".to_string()),
            ),
        ])])),
    );

    let result = session
        .dispatch_tool(ToolCall {
            call_id: ulid::Ulid::new().to_string(),
            tool_name: "query".to_string(),
            args: ToolArgs(serde_json::json!({
                "table": "users",
                "columns": ["name", "password"],
                "limit": 1
            })),
        })
        .await
        .expect("dispatch");

    let encoded = serde_json::to_string(&result.clean).expect("json");
    assert!(!encoded.contains("Sandorian User"));
    assert!(encoded.contains("Name_1"));
    assert!(!encoded.contains("argon2id-secret"));
    assert!(encoded.contains("[REDACTED]"));
}

/// #988: a `production = true` profile whose policy configures no NER model must
/// fail closed at the serve/query session-build seam (`runtime_policy`), never
/// silently serve a profile that would pass arbitrary person names through
/// unredacted. The default policy has no `[ner].model_dir`, so this profile is
/// rejected before any source is touched.
#[test]
fn production_profile_without_ner_fails_closed_in_runtime_policy() {
    let profile = Profile {
        name: "prod".to_string(),
        source: SourceSpec::SshLog {
            host: "example.test".to_string(),
            path: "/var/log/app.log".to_string(),
        },
        // No `policy` → default policy (emails only, no NER model).
        policy: None,
        discovered_from_ssh_host: None,
        discovered_from_path: None,
        discovered_at: None,
        discovered_ssh_host_key_fingerprint: None,
        credential_class: None,
        schema_tokenize: None,
        schema_allowlist: None,
        production: true,
        snapshot_retention_days: None,
        auto_purge: AutoPurge::Off,
    };

    let err = runtime_policy(&profile)
        .expect_err("production profile without an NER model must be rejected");
    let msg = err.to_string();
    assert!(msg.contains("prod"), "{msg}");
    assert!(
        msg.contains("ner.model_dir") || msg.contains("production"),
        "{msg}"
    );
}
