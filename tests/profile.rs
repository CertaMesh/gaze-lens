use gaze_lens::errors::LensError;
use gaze_lens::profile::{SecretSpec, SourceSpec, load_profile, load_profiles};

fn write(path: &std::path::Path, input: &str) {
    std::fs::write(path, input).expect("write config");
}

#[test]
fn two_file_merge_with_pii_policy_collision_project_wins() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("project.toml");
    let user = temp.path().join("user.toml");
    write(
        &project,
        r#"
            [[profiles]]
            name = "prod"
            policy = "project-policy.toml"
            schema_tokenize = true
            schema_allowlist = ["email"]
            source = { kind = "mysql", host = "project-db", port = 3306, database = "project_db", username = "project_user", password_env = "PROJECT_DB_PASSWORD" }
        "#,
    );
    write(
        &user,
        r#"
            [[profiles]]
            name = "prod"
            policy = "user-policy.toml"
            schema_allowlist = ["phone"]
            source = { kind = "mysql", host = "user-db", port = 3307, database = "user_db", username = "user_user", password_env = "USER_DB_PASSWORD" }
        "#,
    );

    let profile = load_profile("prod", Some(&project), Some(&user)).expect("profile");

    assert_eq!(
        profile.policy.as_deref(),
        Some(std::path::Path::new("project-policy.toml"))
    );
    assert_eq!(profile.schema_tokenize, Some(true));
    assert!(profile.schema_tokenize());
    assert_eq!(profile.schema_allowlist, Some(vec!["email".to_string()]));
    match profile.source {
        SourceSpec::Mysql {
            database,
            username,
            password_env,
            ..
        } => {
            assert_eq!(database, "project_db");
            assert_eq!(username, "project_user");
            assert_eq!(password_env.as_deref(), Some("PROJECT_DB_PASSWORD"));
        }
        _ => panic!("expected mysql"),
    }
}

#[test]
fn schema_tokenize_omitted_defaults_to_raw_even_with_allowlist() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("project.toml");
    write(
        &project,
        r#"
            [[profiles]]
            name = "prod"
            schema_allowlist = ["email"]
            source = { kind = "sqlite", path = "/tmp/app.sqlite" }
        "#,
    );

    let profile = load_profile("prod", Some(&project), None).expect("profile");

    assert_eq!(profile.schema_tokenize, None);
    assert!(!profile.schema_tokenize());
    assert_eq!(profile.schema_allowlist, Some(vec!["email".to_string()]));
}

#[test]
fn project_schema_tokenize_false_overrides_user_true() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("project.toml");
    let user = temp.path().join("user.toml");
    write(
        &project,
        r#"
            [[profiles]]
            name = "prod"
            schema_tokenize = false
            source = { kind = "sqlite", path = "/project/app.sqlite" }
        "#,
    );
    write(
        &user,
        r#"
            [[profiles]]
            name = "prod"
            schema_tokenize = true
            source = { kind = "sqlite", path = "/user/app.sqlite" }
        "#,
    );

    let profile = load_profile("prod", Some(&project), Some(&user)).expect("profile");

    assert_eq!(profile.schema_tokenize, Some(false));
    assert!(!profile.schema_tokenize());
}

#[test]
fn two_file_merge_with_transport_collision_user_wins() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("project.toml");
    let user = temp.path().join("user.toml");
    write(
        &project,
        r#"
            [[profiles]]
            name = "prod"
            source = { kind = "mysql", host = "project-db", port = 3306, database = "project_db", username = "project_user", password_env = "PROJECT_DB_PASSWORD", ssh_host = "project-bastion", local_port = 13306 }
        "#,
    );
    write(
        &user,
        r#"
            [[profiles]]
            name = "prod"
            source = { kind = "mysql", host = "127.0.0.1", port = 14406, database = "user_db", username = "user_user", password_env = "USER_DB_PASSWORD", ssh_host = "user-bastion", local_port = 14406 }
        "#,
    );

    let profile = load_profile("prod", Some(&project), Some(&user)).expect("profile");

    match profile.source {
        SourceSpec::Mysql {
            host,
            port,
            ssh_host,
            local_port,
            database,
            username,
            password_env,
            ..
        } => {
            assert_eq!(host, "127.0.0.1");
            assert_eq!(port, 14406);
            assert_eq!(ssh_host.as_deref(), Some("user-bastion"));
            assert_eq!(local_port, Some(14406));
            assert_eq!(database, "project_db");
            assert_eq!(username, "project_user");
            assert_eq!(password_env.as_deref(), Some("PROJECT_DB_PASSWORD"));
        }
        _ => panic!("expected mysql"),
    }
}

#[tokio::test]
async fn password_env_is_resolved_at_connection_time_not_load_time() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("project.toml");
    write(
        &project,
        r#"
            [[profiles]]
            name = "prod"
            source = { kind = "mysql", host = "db", port = 3306, database = "app", username = "app", password_env = "GAZE_LENS_PROFILE_TEST_PASSWORD" }
        "#,
    );

    unsafe {
        std::env::remove_var("GAZE_LENS_PROFILE_TEST_PASSWORD");
    }
    let profile = load_profile("prod", Some(&project), None).expect("load without env");
    unsafe {
        std::env::set_var("GAZE_LENS_PROFILE_TEST_PASSWORD", "secret-at-connect");
    }

    assert_eq!(
        profile.resolve_password().await.expect("password").as_str(),
        "secret-at-connect"
    );
}

#[tokio::test]
async fn missing_env_var_reports_env_name_without_raw_value() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("project.toml");
    write(
        &project,
        r#"
            [[profiles]]
            name = "prod"
            source = { kind = "mysql", host = "db", port = 3306, database = "app", username = "app", password_env = "GAZE_LENS_PROFILE_TEST_MISSING" }
        "#,
    );
    unsafe {
        std::env::remove_var("GAZE_LENS_PROFILE_TEST_MISSING");
    }

    let profile = load_profile("prod", Some(&project), None).expect("profile");
    let err = profile.resolve_password().await.expect_err("missing env");

    assert!(matches!(
        err,
        LensError::ProfileEnvMissing { ref env } if env == "GAZE_LENS_PROFILE_TEST_MISSING"
    ));
}

#[test]
fn malformed_toml_returns_line_col() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join(".gaze-lens.toml");
    write(
        &project,
        r#"
            [[profiles]
            name = "prod"
        "#,
    );

    let err = load_profiles(Some(&project), None).expect_err("malformed toml");
    let message = err.to_string();

    assert!(message.contains("project profile config"), "{message}");
    assert!(
        message.contains(&project.display().to_string()),
        "{message}"
    );
    assert!(message.contains("line "), "{message}");
    assert!(message.contains("column "), "{message}");
}

#[test]
fn missing_required_field_names_field_and_profile() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("project.toml");
    write(
        &project,
        r#"
            [[profiles]]
            name = "prod"
        "#,
    );

    let err = load_profiles(Some(&project), None).expect_err("missing source");
    let message = err.to_string();

    assert!(message.contains("source"), "{message}");
    assert!(message.contains("prod"), "{message}");
}

#[test]
fn explicit_nonexistent_project_config_errors() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("missing-project.toml");

    let err = load_profiles(Some(&project), None).expect_err("missing project config");
    assert!(matches!(
        err,
        LensError::ProfileNotFound { ref label, ref path }
            if label == "project profile config" && path == &project
    ));
}

#[test]
fn ssh_log_profile_parses_host_and_exact_path() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("project.toml");
    write(
        &project,
        r#"
            [[profiles]]
            name = "prod-log"
            source = { kind = "ssh_log", host = "app-prod", path = "/var/log/app.log" }
        "#,
    );

    let profile = load_profile("prod-log", Some(&project), None).expect("profile");

    match profile.source {
        SourceSpec::SshLog { host, path } => {
            assert_eq!(host, "app-prod");
            assert_eq!(path, "/var/log/app.log");
        }
        _ => panic!("expected ssh_log"),
    }
}

#[test]
fn postgres_profile_parses_database_connection_fields() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("project.toml");
    write(
        &project,
        r#"
            [[profiles]]
            name = "prod-pg"
            source = { kind = "postgres", host = "pg-prod", port = 5432, database = "app", username = "app_user", password_env = "PG_PASSWORD", ssh_host = "bastion", local_port = 15432 }
        "#,
    );

    let profile = load_profile("prod-pg", Some(&project), None).expect("profile");

    match profile.source {
        SourceSpec::Postgres {
            host,
            port,
            database,
            username,
            password_env,
            secret,
            ssh_host,
            local_port,
            readonly_required,
        } => {
            assert_eq!(host, "pg-prod");
            assert_eq!(port, 5432);
            assert_eq!(database, "app");
            assert_eq!(username, "app_user");
            assert_eq!(password_env.as_deref(), Some("PG_PASSWORD"));
            assert_eq!(secret, None);
            assert_eq!(ssh_host.as_deref(), Some("bastion"));
            assert_eq!(local_port, Some(15432));
            assert!(readonly_required);
        }
        _ => panic!("expected postgres"),
    }
}

#[test]
fn legacy_password_env_profile_still_parses() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("project.toml");
    write(
        &project,
        r#"
            [[profiles]]
            name = "legacy"
            source = { kind = "mysql", host = "db", port = 3306, database = "app", username = "ro", password_env = "DB_PASSWORD" }
        "#,
    );

    let profile = load_profile("legacy", Some(&project), None).expect("profile");
    match profile.source {
        SourceSpec::Mysql {
            password_env,
            secret,
            ..
        } => {
            assert_eq!(password_env.as_deref(), Some("DB_PASSWORD"));
            assert_eq!(secret, None);
        }
        _ => panic!("expected mysql"),
    }
}

#[test]
fn keyring_secret_profile_parses() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("project.toml");
    write(
        &project,
        r#"
            [[profiles]]
            name = "prod"
            source = { kind = "postgres", host = "db", port = 5432, database = "app", username = "ro", secret = { type = "keyring", service = "gaze-lens", account = "prod" } }
        "#,
    );

    let profile = load_profile("prod", Some(&project), None).expect("profile");
    match profile.source {
        SourceSpec::Postgres {
            password_env,
            secret,
            ..
        } => {
            assert_eq!(password_env, None);
            assert_eq!(
                secret,
                Some(SecretSpec::Keyring {
                    service: "gaze-lens".into(),
                    account: "prod".into()
                })
            );
        }
        _ => panic!("expected postgres"),
    }
}

#[test]
fn both_password_env_and_secret_set_rejects_at_load() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("project.toml");
    write(
        &project,
        r#"
            [[profiles]]
            name = "prod"
            source = { kind = "mysql", host = "db", port = 3306, database = "app", username = "ro", password_env = "PW", secret = { type = "env", var = "OTHER" } }
        "#,
    );

    let err = load_profile("prod", Some(&project), None).expect_err("both rejected");
    assert!(matches!(
        err,
        LensError::Profile { ref detail }
            if detail.contains("specify exactly one")
                && detail.contains("password_env")
                && detail.contains("secret")
    ));
}

#[test]
fn neither_password_env_nor_secret_rejects_at_load() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("project.toml");
    write(
        &project,
        r#"
            [[profiles]]
            name = "prod"
            source = { kind = "mysql", host = "db", port = 3306, database = "app", username = "ro" }
        "#,
    );

    let err = load_profile("prod", Some(&project), None).expect_err("neither rejected");
    assert!(matches!(
        err,
        LensError::Profile { ref detail } if detail.contains("is required")
    ));
}

#[test]
fn merge_secret_project_wins() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("project.toml");
    let user = temp.path().join("user.toml");
    write(
        &project,
        r#"
            [[profiles]]
            name = "prod"
            source = { kind = "mysql", host = "project-db", port = 3306, database = "project_db", username = "project_user", secret = { type = "keyring", service = "gaze-lens", account = "prod" } }
        "#,
    );
    write(
        &user,
        r#"
            [[profiles]]
            name = "prod"
            source = { kind = "mysql", host = "user-db", port = 3307, database = "user_db", username = "user_user", password_env = "USER_DB_PASSWORD" }
        "#,
    );

    let profile = load_profile("prod", Some(&project), Some(&user)).expect("profile");
    match profile.source {
        SourceSpec::Mysql {
            password_env,
            secret,
            ..
        } => {
            assert_eq!(password_env, None);
            assert_eq!(
                secret,
                Some(SecretSpec::Keyring {
                    service: "gaze-lens".into(),
                    account: "prod".into()
                })
            );
        }
        _ => panic!("expected mysql"),
    }
}

#[test]
fn validate_profile_bytes_rejects_password_literal_line_form() {
    let bytes = br#"
        [[profiles]]
        name = "p"
        [profiles.source]
        kind = "mysql"
        host = "db"
        port = 3306
        database = "app"
        username = "ro"
        password = "leak"
    "#;

    let err =
        gaze_lens::profile::validate_profile_bytes(bytes, std::path::Path::new("rendered.toml"))
            .expect_err("must reject");
    assert!(matches!(
        err,
        LensError::Profile { ref detail } if detail.contains("password")
    ));
}

#[test]
fn validate_profile_bytes_rejects_password_inline_table_form() {
    let bytes = br#"
        [[profiles]]
        name = "p"
        source = { kind = "mysql", host = "db", port = 3306, database = "app", username = "ro", password = "leak" }
    "#;

    let err =
        gaze_lens::profile::validate_profile_bytes(bytes, std::path::Path::new("rendered.toml"))
            .expect_err("must reject inline-table password literal");
    assert!(matches!(
        err,
        LensError::Profile { ref detail } if detail.contains("password")
    ));
}

#[test]
fn validate_profile_bytes_allows_password_env_keyword() {
    let bytes = br#"
        [[profiles]]
        name = "p"
        source = { kind = "mysql", host = "db", port = 3306, database = "app", username = "ro", password_env = "PW" }
    "#;

    gaze_lens::profile::validate_profile_bytes(bytes, std::path::Path::new("rendered.toml"))
        .expect("password_env is allowed");
}

#[test]
fn sqlite_profile_parses_path_and_readonly_default() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("project.toml");
    write(
        &project,
        r#"
            [[profiles]]
            name = "prod-sqlite"
            source = { kind = "sqlite", path = "/srv/app/prod.sqlite" }
        "#,
    );

    let profile = load_profile("prod-sqlite", Some(&project), None).expect("profile");

    match profile.source {
        SourceSpec::Sqlite {
            path,
            readonly_required,
            json_text_columns,
        } => {
            assert_eq!(path, std::path::PathBuf::from("/srv/app/prod.sqlite"));
            assert!(readonly_required);
            assert!(json_text_columns.is_empty());
        }
        _ => panic!("expected sqlite"),
    }
}

#[test]
fn sqlite_profile_parses_json_text_columns() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("project.toml");
    write(
        &project,
        r#"
            [[profiles]]
            name = "prod-sqlite"
            source = { kind = "sqlite", path = "/srv/app/prod.sqlite", json_text_columns = ["users.preferences"] }
        "#,
    );

    let profile = load_profile("prod-sqlite", Some(&project), None).expect("profile");

    match profile.source {
        SourceSpec::Sqlite {
            json_text_columns, ..
        } => {
            assert_eq!(json_text_columns, vec!["users.preferences"]);
        }
        _ => panic!("expected sqlite"),
    }
}

#[test]
fn ssh_log_two_file_merge_user_transport_wins() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("project.toml");
    let user = temp.path().join("user.toml");
    write(
        &project,
        r#"
            [[profiles]]
            name = "prod-log"
            source = { kind = "ssh_log", host = "project-host", path = "/var/log/project.log" }
        "#,
    );
    write(
        &user,
        r#"
            [[profiles]]
            name = "prod-log"
            source = { kind = "ssh_log", host = "user-host", path = "/var/log/user.log" }
        "#,
    );

    let profile = load_profile("prod-log", Some(&project), Some(&user)).expect("profile");

    match profile.source {
        SourceSpec::SshLog { host, path } => {
            assert_eq!(host, "user-host");
            assert_eq!(path, "/var/log/user.log");
        }
        _ => panic!("expected ssh_log"),
    }
}
