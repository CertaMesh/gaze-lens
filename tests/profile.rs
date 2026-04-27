use gaze_lens::errors::LensError;
use gaze_lens::profile::{SourceSpec, load_profile, load_profiles};

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
            assert_eq!(password_env, "PROJECT_DB_PASSWORD");
        }
        _ => panic!("expected mysql"),
    }
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
            assert_eq!(password_env, "PROJECT_DB_PASSWORD");
        }
        _ => panic!("expected mysql"),
    }
}

#[test]
fn password_env_is_resolved_at_connection_time_not_load_time() {
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
        profile.resolve_password().expect("password"),
        "secret-at-connect"
    );
}

#[test]
fn missing_env_var_reports_env_name_without_raw_value() {
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
    let err = profile.resolve_password().expect_err("missing env");

    assert!(matches!(
        err,
        LensError::ProfileEnvMissing { ref env } if env == "GAZE_LENS_PROFILE_TEST_MISSING"
    ));
}

#[test]
fn absent_files_load_empty_profiles() {
    let temp = tempfile::tempdir().expect("tempdir");
    let profiles = load_profiles(
        Some(&temp.path().join("missing-project.toml")),
        Some(&temp.path().join("missing-user.toml")),
    )
    .expect("profiles");
    assert!(profiles.is_empty());
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
            ssh_host,
            local_port,
            readonly_required,
        } => {
            assert_eq!(host, "pg-prod");
            assert_eq!(port, 5432);
            assert_eq!(database, "app");
            assert_eq!(username, "app_user");
            assert_eq!(password_env, "PG_PASSWORD");
            assert_eq!(ssh_host.as_deref(), Some("bastion"));
            assert_eq!(local_port, Some(15432));
            assert!(readonly_required);
        }
        _ => panic!("expected postgres"),
    }
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
