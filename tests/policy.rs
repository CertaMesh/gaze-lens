use gaze_lens::policy::{PolicyFile, build_pipeline};

fn redact_policy_text(policy: &PolicyFile, text: &str) -> String {
    let pipeline = build_pipeline(policy).expect("pipeline");
    let session = gaze::Session::new(gaze::Scope::Conversation(ulid::Ulid::new().to_string()))
        .expect("gaze session");
    match pipeline
        .redact(&session, gaze::RawDocument::Text(text.to_string()))
        .expect("redact")
    {
        gaze::CleanDocument::Text(text) => text,
        other => panic!("expected text output, got {other:?}"),
    }
}

#[test]
fn minimal_policy_toml_builds_pipeline_and_allows_non_production_profiles() {
    let policy = PolicyFile::from_toml(
        r#"
        [connection.staging]
        kind = "mysql"

        [connection.incident]
        kind = "postgres"

        [policy.database]

        [[policy.database.columns]]
        column = "email"
        class = "email"
        action = "tokenize"
        "#,
    )
    .expect("policy");

    assert_eq!(policy.connection.len(), 2);
    build_pipeline(&policy).expect("pipeline");
}

#[test]
fn minimal_policy_preserves_detected_email_without_explicit_rule() {
    let policy = PolicyFile::from_toml("[policy.database]\n").expect("policy");

    let output = redact_policy_text(&policy, "email alice@example.invalid about the incident");

    assert_eq!(output, "email alice@example.invalid about the incident");
}

#[test]
fn explicit_preserve_default_action_keeps_detected_email_byte_identical() {
    let policy = PolicyFile::from_toml(
        r#"
        [policy]
        default_action = "preserve"

        [policy.database]
        "#,
    )
    .expect("policy");

    let output = redact_policy_text(&policy, "email alice@example.invalid about the incident");

    assert_eq!(output, "email alice@example.invalid about the incident");
}

#[test]
fn tokenize_default_action_tokenizes_detected_email_without_explicit_rule() {
    let policy = PolicyFile::from_toml(
        r#"
        [policy]
        default_action = "tokenize"

        [policy.database]
        "#,
    )
    .expect("policy");

    let output = redact_policy_text(&policy, "email alice@example.invalid about the incident");

    assert!(!output.contains("alice@example.invalid"), "{output}");
    assert!(output.starts_with("email <"), "{output}");
    assert!(output.ends_with(":Email_1> about the incident"), "{output}");
}

fn policy_with_scope(scope: &str) -> PolicyFile {
    PolicyFile::from_toml(&format!(
        r#"
        [session]
        scope = "{scope}"

        [policy.database]
        "#,
    ))
    .expect("policy")
}

#[test]
fn conversation_scope_is_accepted_case_insensitively() {
    let policy = policy_with_scope("Conversation")
        .to_gaze_policy()
        .expect("gaze policy");

    assert!(matches!(
        policy.session.scope,
        gaze::SessionScope::Conversation
    ));
}

#[test]
fn persistent_scope_from_policy_is_rejected_explicitly() {
    let err = policy_with_scope("persistent")
        .to_gaze_policy()
        .expect_err("persistent scope should fail");

    assert_eq!(
        err.to_string(),
        "policy.session.scope = \"persistent\" is not supported in v0.1; only \"conversation\" is accepted. See SPEC.md §session-lifecycle."
    );
}
