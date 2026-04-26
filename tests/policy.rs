use gaze_lens::errors::LensError;
use gaze_lens::policy::{build_pipeline, PolicyFile};
use gaze_lens::session::Session;

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
fn ephemeral_scope_from_policy_is_rejected_by_session_core() {
    let policy = PolicyFile::from_toml(
        r#"
        [session]
        scope = "ephemeral"

        [policy.database]
        "#,
    )
    .expect("policy")
    .to_gaze_policy()
    .expect("gaze policy");
    let temp = tempfile::tempdir().expect("tempdir");

    let result = Session::new(
        &policy,
        &temp.path().join("manifest.sqlite"),
        &temp.path().join("snapshots"),
    );
    let err = match result {
        Ok(_) => panic!("ephemeral session should fail"),
        Err(err) => err,
    };

    assert!(matches!(err, LensError::ScopeRejected { .. }));
}
