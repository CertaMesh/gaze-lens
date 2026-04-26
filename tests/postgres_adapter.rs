use gaze_lens::errors::LensError;
use gaze_lens::profile::{Profile, SourceSpec};
use gaze_lens::source::db::postgres::PostgresSource;

fn postgres_profile(readonly_required: bool, password_env: &str) -> Profile {
    Profile {
        name: "prod-pg".to_string(),
        policy: None,
        schema_allowlist: None,
        source: SourceSpec::Postgres {
            host: "127.0.0.1".to_string(),
            port: 5432,
            database: "app".to_string(),
            username: "app".to_string(),
            password_env: password_env.to_string(),
            ssh_host: None,
            local_port: None,
            readonly_required,
        },
    }
}

#[tokio::test]
async fn refuses_postgres_profile_without_readonly_required() {
    let profile = postgres_profile(false, "GAZE_LENS_POSTGRES_TEST_PASSWORD_UNUSED");

    let err = PostgresSource::connect(&profile, 100)
        .await
        .expect_err("readonly gate should fail before connecting");

    assert!(matches!(err, LensError::Profile { .. }));
}

#[tokio::test]
async fn refuses_missing_postgres_password_env_before_connecting() {
    unsafe {
        std::env::remove_var("GAZE_LENS_POSTGRES_TEST_PASSWORD_MISSING");
    }
    let profile = postgres_profile(true, "GAZE_LENS_POSTGRES_TEST_PASSWORD_MISSING");

    let err = PostgresSource::connect(&profile, 100)
        .await
        .expect_err("missing env should fail before connecting");

    assert!(matches!(
        err,
        LensError::ProfileEnvMissing { ref env } if env == "GAZE_LENS_POSTGRES_TEST_PASSWORD_MISSING"
    ));
}

// Postgres end-to-end coverage is intentionally behind an opt-in feature
// because the standard test run must not require Docker or a local database.
// It should exercise supported scalar types, NULL preservation, bytea, uuid,
// json/jsonb, timestamp formatting, and explicit ARRAY rejection.
#[cfg(feature = "integration-postgres")]
mod integration {
    // Filled in when the project enables a concrete Postgres testcontainer stack.
}
