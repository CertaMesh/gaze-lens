use gaze_lens::cli::init::SourceKind;
use gaze_lens::cli::init::plan::{AutoPurgeChoice, ProfileSection};
use gaze_lens::cli::init::profile_writer::{RenderError, render_profile_toml};

fn section(name: &str, kind: SourceKind) -> ProfileSection {
    ProfileSection {
        name: name.into(),
        source_kind: kind,
        source_path: Some("/tmp/x.db".into()),
        source_host: None,
        source_port: None,
        source_database: None,
        source_username: None,
        source_password_env: None,
        source_ssh_host: None,
        source_local_port: None,
        source_json_text_columns: Vec::new(),
        policy_path: None,
        schema_allowlist: vec!["id".into()],
        snapshot_retention_days: None,
        auto_purge: AutoPurgeChoice::Off,
    }
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
