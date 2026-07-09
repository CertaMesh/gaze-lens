use gaze_lens::cli::init::SourceKind;
use gaze_lens::cli::init::plan::{AutoPurgeChoice, CredentialClass, PlannedSecret, ProfileSection};
use gaze_lens::cli::init::profile_writer::{RenderError, render_profile_toml};

fn section(name: &str, kind: SourceKind) -> ProfileSection {
    ProfileSection {
        name: name.into(),
        production: false,
        source_kind: kind,
        source_path: Some("/tmp/x.db".into()),
        source_host: None,
        source_port: None,
        source_database: None,
        source_username: None,
        source_password_env: None,
        source_secret: None,
        source_ssh_host: None,
        source_local_port: None,
        source_json_text_columns: Vec::new(),
        policy_path: None,
        schema_allowlist: vec!["id".into()],
        snapshot_retention_days: None,
        discovered_from_ssh_host: None,
        discovered_from_path: None,
        discovered_at: None,
        discovered_ssh_host_key_fingerprint: None,
        credential_class: CredentialClass::ManuallyEntered,
        auto_purge: AutoPurgeChoice::Off,
    }
}

#[test]
fn production_true_is_rendered() {
    let mut s = section("prod", SourceKind::Sqlite);
    s.production = true;

    let out = render_profile_toml(None, &s, false).unwrap();
    assert!(out.contains("production = true"), "got: {out}");
}

#[test]
fn render_keyring_secret_round_trips_through_loader() {
    let mut s = section("prod", SourceKind::Postgres);
    s.source_host = Some("db".into());
    s.source_port = Some(5432);
    s.source_database = Some("app".into());
    s.source_username = Some("ro".into());
    s.source_path = None;
    s.source_password_env = None;
    s.source_secret = Some(PlannedSecret::Keyring {
        service: "gaze-lens".into(),
        account: "prod".into(),
        write_value: None,
    });

    let out = render_profile_toml(None, &s, false).unwrap();
    assert!(out.contains(r#"type = "keyring""#), "got: {out}");
    assert!(out.contains(r#"service = "gaze-lens""#), "got: {out}");
    assert!(out.contains(r#"account = "prod""#), "got: {out}");

    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("profile.toml");
    std::fs::write(&p, &out).unwrap();
    let profiles = gaze_lens::profile::load_profiles(Some(&p), None).expect("round-trip");
    assert!(profiles.iter().any(|x| x.name == "prod"));
}

#[test]
fn render_secret_env_form_round_trips() {
    let mut s = section("prod", SourceKind::Mysql);
    s.source_host = Some("db".into());
    s.source_port = Some(3306);
    s.source_database = Some("app".into());
    s.source_username = Some("ro".into());
    s.source_path = None;
    s.source_password_env = None;
    s.source_secret = Some(PlannedSecret::Env {
        var: "DB_PW".into(),
    });

    let out = render_profile_toml(None, &s, false).unwrap();
    assert!(out.contains(r#"type = "env""#), "got: {out}");
    assert!(out.contains(r#"var = "DB_PW""#), "got: {out}");

    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("profile.toml");
    std::fs::write(&p, &out).unwrap();
    let profiles = gaze_lens::profile::load_profiles(Some(&p), None).expect("round-trip");
    assert!(profiles.iter().any(|x| x.name == "prod"));
}

#[test]
fn pillar2_byte_scan_no_password_value_for_keyring_or_env() {
    let supplied_password = "hunter2-writer-test";
    let mut s = section("prod", SourceKind::Postgres);
    s.source_host = Some("db".into());
    s.source_port = Some(5432);
    s.source_database = Some("app".into());
    s.source_username = Some("ro".into());
    s.source_path = None;
    s.source_secret = Some(PlannedSecret::Keyring {
        service: "gaze-lens".into(),
        account: "prod".into(),
        write_value: Some(zeroize::Zeroizing::new(supplied_password.to_string())),
    });

    let out = render_profile_toml(None, &s, false).unwrap();
    assert!(!out.contains(supplied_password), "{out}");
    assert!(!out.contains("password ="), "{out}");
}

#[test]
fn provenance_fields_round_trip_through_toml() {
    let mut s = section("prod", SourceKind::Postgres);
    s.source_path = None;
    s.source_host = Some("db".into());
    s.source_port = Some(5432);
    s.source_database = Some("app".into());
    s.source_username = Some("ro".into());
    s.source_secret = Some(PlannedSecret::Keyring {
        service: "gaze-lens".into(),
        account: "prod".into(),
        write_value: None,
    });
    s.discovered_from_ssh_host = Some("deploy@app01".into());
    s.discovered_from_path = Some("/var/www/app/.env".into());
    s.discovered_at = Some(time::OffsetDateTime::from_unix_timestamp(1_777_500_000).unwrap());
    s.discovered_ssh_host_key_fingerprint = Some("SHA256:abc".into());
    s.credential_class = CredentialClass::ManuallyEntered;

    let out = render_profile_toml(None, &s, false).unwrap();
    assert!(out.contains(r#"discovered_from_ssh_host = "deploy@app01""#));
    assert!(out.contains(r#"discovered_from_path = "/var/www/app/.env""#));
    assert!(out.contains("discovered_at"));
    assert!(out.contains(r#"discovered_ssh_host_key_fingerprint = "SHA256:abc""#));
    assert!(out.contains(r#"credential_class = "manually-entered""#));

    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("profile.toml");
    std::fs::write(&p, &out).unwrap();
    let profiles = gaze_lens::profile::load_profiles(Some(&p), None).expect("round-trip");
    let profile = profiles.iter().find(|x| x.name == "prod").unwrap();
    assert_eq!(
        profile.discovered_from_ssh_host.as_deref(),
        Some("deploy@app01")
    );
    assert_eq!(
        profile.credential_class.as_deref(),
        Some("manually-entered")
    );
}

#[test]
fn render_into_empty_yields_minimal_profile_no_password() {
    let out = render_profile_toml(None, &section("dev", SourceKind::Sqlite), false).unwrap();
    assert!(out.contains("[[profiles]]"));
    assert!(out.contains(r#"name = "dev""#));
    assert!(!out.contains("password ="), "must never write `password =`");
    assert!(
        !out.contains("auto_purge"),
        "auto_purge default off → omit (CB2)"
    );
}

#[test]
fn auto_purge_renders_enum_string_round_trips() {
    let mut s = section("p", SourceKind::Sqlite);
    s.auto_purge = AutoPurgeChoice::Purge;
    let out = render_profile_toml(None, &s, false).unwrap();
    assert!(
        out.contains(r#"auto_purge = "purge""#),
        "must render enum string, not bool; got: {out}"
    );
    assert!(!out.contains("auto_purge = true"));

    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("profile.toml");
    std::fs::write(&p, &out).unwrap();
    let profiles = gaze_lens::profile::load_profiles(Some(&p), None).expect("load");
    assert!(profiles.iter().any(|x| x.name == "p"));
}

#[test]
fn source_kind_renders_snake_case_round_trips() {
    for (kind, expected_token) in [
        (SourceKind::Sqlite, r#"kind = "sqlite""#),
        (SourceKind::Mysql, r#"kind = "mysql""#),
        (SourceKind::Postgres, r#"kind = "postgres""#),
        (SourceKind::SshLog, r#"kind = "ssh_log""#),
    ] {
        let mut s = section("x", kind);
        s.source_host = Some("h".into());
        s.source_port = Some(1);
        s.source_database = Some("d".into());
        s.source_username = Some("u".into());
        s.source_password_env = Some("E".into());
        let out = render_profile_toml(None, &s, false).unwrap();
        assert!(out.contains(expected_token), "kind {kind:?}; got: {out}");
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("profile.toml");
        std::fs::write(&p, &out).unwrap();
        gaze_lens::profile::load_profiles(Some(&p), None).expect("round-trip");
    }
}

#[test]
fn collision_without_overwrite_errors() {
    let existing = r#"
[[profiles]]
name = "dev"

[profiles.source]
kind = "sqlite"
path = "/old/x.db"
"#;
    let err = render_profile_toml(Some(existing), &section("dev", SourceKind::Sqlite), false)
        .unwrap_err();
    assert!(matches!(err, RenderError::Collision { .. }));
}

#[test]
fn auto_purge_line_in_existing_preserved_when_unrelated_profile_added() {
    let existing = r#"
[[profiles]]
name = "prod"
auto_purge = "purge"

[profiles.source]
kind = "mysql"
host = "p"
port = 3306
database = "d"
username = "u"
password_env = "E"
"#;
    let new = section("dev", SourceKind::Sqlite);
    let out = render_profile_toml(Some(existing), &new, false).unwrap();
    assert!(
        out.contains(r#"auto_purge = "purge""#),
        "verbatim auto_purge string preservation"
    );
    assert!(out.contains(r#"name = "dev""#));
}

#[test]
fn malformed_existing_toml_reports_path_and_position() {
    // Directive 14 + MS3: path + (line, column) explicit in Display, decoupled
    // from `toml_edit::TomlError::to_string()`'s upstream literal format.
    let existing = "[[profiles]\nname = \"x\"\n";
    let err =
        render_profile_toml(Some(existing), &section("x", SourceKind::Sqlite), false).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("line "), "{msg}");
    assert!(msg.contains("column "), "{msg}");
}
