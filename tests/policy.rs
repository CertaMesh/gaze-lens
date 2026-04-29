use gaze_lens::policy::{PolicyFile, build_pipeline};

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
