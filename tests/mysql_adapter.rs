use gaze_lens::errors::LensError;
use gaze_lens::profile::{Profile, SourceSpec};
use gaze_lens::session::maintenance::AutoPurge;
use gaze_lens::source::db::mysql::MysqlSource;

fn mysql_profile(readonly_required: bool, password_env: &str) -> Profile {
    Profile {
        name: "prod".to_string(),
        policy: None,
        schema_allowlist: None,
        snapshot_retention_days: None,
        auto_purge: AutoPurge::Off,
        source: SourceSpec::Mysql {
            host: "127.0.0.1".to_string(),
            port: 3306,
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
async fn refuses_mysql_profile_without_readonly_required() {
    let profile = mysql_profile(false, "GAZE_LENS_MYSQL_TEST_PASSWORD_UNUSED");

    let err = MysqlSource::connect(&profile, 100)
        .await
        .expect_err("readonly gate should fail before connecting");

    assert!(matches!(err, LensError::Profile { .. }));
}

#[tokio::test]
async fn refuses_missing_password_env_before_connecting() {
    unsafe {
        std::env::remove_var("GAZE_LENS_MYSQL_TEST_PASSWORD_MISSING");
    }
    let profile = mysql_profile(true, "GAZE_LENS_MYSQL_TEST_PASSWORD_MISSING");

    let err = MysqlSource::connect(&profile, 100)
        .await
        .expect_err("missing env should fail before connecting");

    assert!(matches!(
        err,
        LensError::ProfileEnvMissing { ref env } if env == "GAZE_LENS_MYSQL_TEST_PASSWORD_MISSING"
    ));
}

// MySQL end-to-end coverage is intentionally behind an opt-in feature because
// the standard test run must not require Docker or a local database. The test
// exercises read-only setup, schema introspection, canned query execution,
// NULL preservation, and explicit unsupported-type conversion failures.
#[cfg(feature = "integration-mysql")]
mod integration {
    // Filled in when the project enables a concrete MySQL testcontainer stack.
}
