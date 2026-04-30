use assert_cmd::Command;
use gaze_lens::cli::check::{CheckArgs, run_with_writer_for_test, validate_secret};
use gaze_lens::errors::LensError;
use gaze_lens::frontend::mcp::McpFrontend;
use gaze_lens::profile::{Profile, SecretSpec, SourceSpec};
use gaze_lens::session::maintenance::AutoPurge;
use keyring::credential::{Credential, CredentialApi, CredentialBuilderApi, CredentialPersistence};
use rusqlite::Connection;
use std::any::Any;
use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::sync::{Mutex, OnceLock};

#[test]
fn trust_report_json_shape_is_stable() {
    use gaze_lens::cli::check_trust::{REPORT_VERSION, TrustReport};

    let report = TrustReport::stub_for_test("prod");
    let v: serde_json::Value = serde_json::to_value(&report).expect("json");
    assert_eq!(v["report_version"], REPORT_VERSION);
    for key in [
        "profile",
        "input_surface",
        "process_surface",
        "output_surface",
        "at_rest_surface",
        "handoff_surface",
    ] {
        assert!(v.get(key).is_some(), "missing top-level key `{key}`: {v}");
    }
    assert_eq!(v["input_surface"]["raw_sql"], "disabled (v1 lock, D5)");
    assert_eq!(v["input_surface"]["query_mode"], "canned-structured");
    assert!(v["process_surface"].get("profile_under_review").is_some());
    assert!(v["process_surface"].get("serve_default_scope").is_some());
}

#[test]
fn collect_input_surface_lists_locked_mcp_tools_from_const() {
    let profile = sqlite_profile("local", std::path::PathBuf::from("fixture.sqlite"));

    let surface = gaze_lens::cli::check_trust::collect_input_surface(&profile);

    assert_eq!(surface.mcp_tools, McpFrontend::public_tool_names());
    assert_eq!(
        surface.cli_subcommands,
        ["init", "query", "replay", "check", "serve", "demo"]
    );
}

#[test]
fn secret_locator_keyring_redacts_value() {
    let profile = keyring_profile("prod", "gaze-lens-prod", "readonly");

    let locator = gaze_lens::cli::check_trust::secret_locator(&profile.source);

    assert_eq!(locator.backend, "keyring");
    assert_eq!(locator.identity, "service=gaze-lens-prod account=readonly");
}

#[test]
fn secret_locator_no_leak_fuzzy() {
    let sentinel = random_hex_32_bytes();
    let env = format!("GAZE_LENS_TRUST_REPORT_SECRET_{}", ulid::Ulid::new());
    unsafe {
        std::env::set_var(&env, &sentinel);
    }
    let profile = postgres_env_profile("prod", &env);

    let report = gaze_lens::cli::check_trust::build_report(
        &profile,
        std::path::Path::new("/proc/self/mem"),
        std::path::Path::new("/tmp/gaze-lens-snapshots"),
        None,
    )
    .expect("report");
    let json = serde_json::to_string(&report).expect("json");
    let debug = format!("{report:?}");

    assert!(!json.contains(&sentinel), "{json}");
    assert!(!debug.contains(&sentinel), "{debug}");
    assert_eq!(report.at_rest_surface.secret_backend.backend, "env");
    assert_eq!(
        report.at_rest_surface.secret_backend.identity,
        format!("var={env}")
    );

    let err = std::fs::read_to_string("/proc/self/mem").expect_err("force error");
    let err = LensError::Internal {
        detail: format!("read manifest: {err}"),
    };
    assert!(!err.to_string().contains(&sentinel), "{err}");
}

#[test]
fn secret_locator_sqlite_says_not_required() {
    let profile = sqlite_profile("local", std::path::PathBuf::from("fixture.sqlite"));

    let locator = gaze_lens::cli::check_trust::secret_locator(&profile.source);

    assert_eq!(locator.backend, "none");
    assert_eq!(locator.identity, "not required");
}

#[test]
fn source_transport_omits_secret_for_postgres() {
    let sentinel = random_hex_32_bytes();
    let env = format!("GAZE_LENS_TRUST_REPORT_TRANSPORT_{}", ulid::Ulid::new());
    unsafe {
        std::env::set_var(&env, &sentinel);
    }
    let profile = postgres_env_profile("prod", &env);

    let transport = gaze_lens::cli::check_trust::source_transport(&profile.source);
    for key in [
        "host",
        "port",
        "database",
        "username",
        "ssh_host",
        "local_port",
        "readonly_required",
    ] {
        assert!(
            transport.get(key).is_some(),
            "missing `{key}` in {transport}"
        );
    }
    let json = serde_json::to_string(&transport).expect("json");
    assert!(!json.contains("password"), "{json}");
    assert!(!json.contains("secret"), "{json}");
    assert!(!json.contains(&sentinel), "{json}");
}

#[test]
fn sqlite_json_text_policy_surfaced_for_sqlite_profiles() {
    let profile = Profile {
        source: SourceSpec::Sqlite {
            path: std::path::PathBuf::from("fixture.sqlite"),
            readonly_required: true,
            json_text_columns: vec!["events.payload".into(), "audit.context".into()],
        },
        ..sqlite_profile("local", std::path::PathBuf::from("fixture.sqlite"))
    };
    let postgres = postgres_env_profile("prod", "GAZE_LENS_TRUST_REPORT_UNUSED");

    assert_eq!(
        gaze_lens::cli::check_trust::collect_input_surface(&profile).sqlite_json_text_policy,
        Some(vec!["events.payload".into(), "audit.context".into()])
    );
    assert_eq!(
        gaze_lens::cli::check_trust::collect_input_surface(&postgres).sqlite_json_text_policy,
        None
    );
}

#[test]
fn collect_handoff_surface_lists_six_residual_risks() {
    let handoff = gaze_lens::cli::check_trust::collect_handoff_surface();
    let ids: Vec<_> = handoff.residual_risks.iter().map(|risk| risk.id).collect();

    assert_eq!(handoff.residual_risks.len(), 6);
    for id in [
        "disk_encryption",
        "db_user_privileges",
        "ssh_auth",
        "backup_exclusion",
        "cross_profile_correlation",
        "binary_attestation",
    ] {
        assert!(ids.contains(&id), "missing {id}: {ids:?}");
    }
    assert!(
        handoff
            .residual_risks
            .iter()
            .all(|risk| !risk.mitigation.is_empty())
    );
}

#[test]
fn inspect_path_reports_mode_when_file_exists() {
    let temp = tempfile::NamedTempFile::new().expect("tempfile");
    std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o600)).expect("chmod");

    let artifact = gaze_lens::cli::check_trust::inspect_path(temp.path(), 0o600);

    assert!(artifact.exists);
    assert_eq!(artifact.mode_ok, Some(true));
    assert_eq!(artifact.expected_mode, "0600");
}

#[test]
fn inspect_path_reports_mode_mismatch() {
    let temp = tempfile::NamedTempFile::new().expect("tempfile");
    std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o644)).expect("chmod");

    let artifact = gaze_lens::cli::check_trust::inspect_path(temp.path(), 0o600);

    assert!(artifact.exists);
    assert_eq!(artifact.mode_ok, Some(false));
    assert_eq!(artifact.expected_mode, "0600");
}

#[test]
fn inspect_path_handles_not_yet_materialized() {
    let temp = tempfile::tempdir().expect("tempdir");
    let missing = temp.path().join("missing.sqlite");

    let artifact = gaze_lens::cli::check_trust::inspect_path(&missing, 0o600);

    assert!(!artifact.exists);
    assert_eq!(artifact.mode_ok, None);
    assert_eq!(artifact.expected_mode, "0600");
}

#[test]
fn recognizer_pack_default_empty_when_policy_unset() {
    let pack = gaze_lens::cli::check_trust::recognizer_pack_from_parsed(None, None, None);

    assert!(pack.default_empty);
    assert_eq!(pack.source, "default-empty");
    assert_eq!(pack.policy_path, None);
    assert_eq!(pack.policy_sha256, None);
    assert_eq!(pack.recognizer_keys, ["database"]);
    assert!(pack.recognizer_classes.is_empty());
}

#[test]
fn recognizer_pack_lists_keys_and_classes_from_policy_toml() {
    let temp = tempfile::NamedTempFile::new().expect("tempfile");
    let raw = b"[policy.database]\nemail = true\nphone = true\n[policy.logs]\nip = true\n[ner]\nlocale = \"en\"\n";
    std::fs::write(temp.path(), raw).expect("policy");
    let parsed: toml::Value =
        toml::from_str(std::str::from_utf8(raw).expect("utf8")).expect("toml");

    let pack = gaze_lens::cli::check_trust::recognizer_pack_from_parsed(
        Some(temp.path()),
        Some(&parsed),
        Some(raw),
    );

    assert!(!pack.default_empty);
    assert_eq!(pack.source, "policy-toml");
    assert_eq!(pack.policy_sha256.as_deref().expect("sha").len(), 64);
    assert_eq!(pack.recognizer_keys, ["database", "logs"]);
    assert_eq!(
        pack.recognizer_classes,
        ["database.email", "database.phone", "logs.ip"]
    );
}

#[test]
fn at_rest_surface_uses_passed_manifest_path_not_args_default() {
    let temp = tempfile::tempdir().expect("tempdir");
    let manifest = temp.path().join("manifest.sqlite");
    let snapshot_dir = temp.path().join("snapshots");
    std::fs::create_dir(&snapshot_dir).expect("snapshot dir");
    let profile = sqlite_profile("local", std::path::PathBuf::from("fixture.sqlite"));

    let surface =
        gaze_lens::cli::check_trust::collect_at_rest_surface(&profile, &manifest, &snapshot_dir);

    assert_eq!(surface.manifest.path, manifest.display().to_string());
}

#[test]
fn trust_report_text_lists_all_pillars() {
    let report = gaze_lens::cli::check_trust::TrustReport::stub_for_test("prod");
    let mut out = Vec::new();

    gaze_lens::cli::check_trust::render_text(&report, &mut out).expect("render");
    let text = String::from_utf8(out).expect("utf8");

    for header in [
        "Input surface",
        "Process surface",
        "Output surface",
        "At-rest surface",
        "Operator handoff",
    ] {
        assert!(text.contains(header), "missing {header}: {text}");
    }
    assert!(text.contains("raw_sql: disabled (v1 lock, D5)"), "{text}");
    assert!(text.contains("(see src/session/mod.rs:304)"), "{text}");
}

#[test]
fn trust_report_text_warns_on_default_empty_policy() {
    let report = gaze_lens::cli::check_trust::TrustReport::stub_for_test("prod");
    let mut out = Vec::new();

    gaze_lens::cli::check_trust::render_text(&report, &mut out).expect("render");
    let text = String::from_utf8(out).expect("utf8");

    assert!(text.contains("WARN: no recognizer pack"), "{text}");
}

#[test]
fn trust_report_text_rejects_terminal_escape_in_profile_name() {
    let report = gaze_lens::cli::check_trust::TrustReport::stub_for_test("prod\u{1b}[2K");
    let mut out = Vec::new();

    let err = gaze_lens::cli::check_trust::render_text(&report, &mut out).expect_err("render");
    let text = String::from_utf8(out).expect("utf8");

    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    assert!(!text.contains("\u{1b}[2K"), "{text}");
}

#[test]
fn check_validates_profile_policy_connection_and_pipeline_without_writes() {
    let temp = tempfile::tempdir().expect("tempdir");
    let db = temp.path().join("fixture.sqlite");
    let project = temp.path().join("project.toml");
    let policy = temp.path().join("policy.toml");
    seed_sqlite(&db);
    std::fs::write(&policy, "[policy.database]\n").expect("policy");
    write_profile(&project, &db, &policy);

    let mut cmd = Command::cargo_bin("gaze-lens").expect("binary");
    let output = cmd
        .args([
            "--project-config",
            project.to_str().expect("project path"),
            "check",
            "--profile",
            "local",
        ])
        .output()
        .expect("check");

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let stdout = stdout(&output);
    assert!(stdout.contains("profile: ok"));
    assert!(stdout.contains("policy: ok"));
    assert!(stdout.contains("secret: ok"));
    assert!(stdout.contains("source: ok"));
    assert!(stdout.contains("pipeline: ok"));
    assert!(!temp.path().join("manifest.sqlite").exists());
    assert!(!temp.path().join("snapshots").exists());
}

#[test]
fn check_without_explain_risk_unchanged_backward_compat() {
    let temp = tempfile::tempdir().expect("tempdir");
    let db = temp.path().join("fixture.sqlite");
    let project = temp.path().join("project.toml");
    let policy = temp.path().join("policy.toml");
    seed_sqlite(&db);
    std::fs::write(&policy, "[policy.database]\n").expect("policy");
    write_profile(&project, &db, &policy);

    let mut cmd = Command::cargo_bin("gaze-lens").expect("binary");
    let output = cmd
        .args([
            "--project-config",
            project.to_str().expect("project path"),
            "check",
            "--profile",
            "local",
        ])
        .output()
        .expect("check");

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert_eq!(
        stdout(&output),
        "profile: ok (local)\npolicy: ok\nsecret: ok (none not required)\nsource: ok\npipeline: ok\n"
    );
}

#[test]
fn check_explain_risk_text_appends_after_status_lines() {
    let temp = tempfile::tempdir().expect("tempdir");
    let db = temp.path().join("fixture.sqlite");
    let project = temp.path().join("project.toml");
    let policy = temp.path().join("policy.toml");
    seed_sqlite(&db);
    std::fs::write(&policy, "[policy.database]\n").expect("policy");
    write_profile(&project, &db, &policy);

    let mut cmd = Command::cargo_bin("gaze-lens").expect("binary");
    let output = cmd
        .args([
            "--project-config",
            project.to_str().expect("project path"),
            "check",
            "--profile",
            "local",
            "--explain-risk",
        ])
        .output()
        .expect("check");

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let stdout = stdout(&output);
    assert!(stdout.starts_with(
        "profile: ok (local)\npolicy: ok\nsecret: skipped (--explain-risk local-only)\nsource: skipped (--explain-risk local-only)\npipeline: ok\n"
    ), "{stdout}");
    assert!(stdout.contains("Input surface"), "{stdout}");
}

#[test]
fn check_explain_risk_json_emits_only_json_on_stdout() {
    let temp = tempfile::tempdir().expect("tempdir");
    let db = temp.path().join("fixture.sqlite");
    let project = temp.path().join("project.toml");
    let policy = temp.path().join("policy.toml");
    seed_sqlite(&db);
    std::fs::write(&policy, "[policy.database]\n").expect("policy");
    write_profile(&project, &db, &policy);

    let mut cmd = Command::cargo_bin("gaze-lens").expect("binary");
    let output = cmd
        .args([
            "--project-config",
            project.to_str().expect("project path"),
            "check",
            "--profile",
            "local",
            "--explain-risk",
            "--format",
            "json",
        ])
        .output()
        .expect("check");

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let v: serde_json::Value = serde_json::from_slice(&output.stdout).expect("json stdout");
    assert_eq!(v["report_version"], 1);
    let stdout = stdout(&output);
    assert!(!stdout.contains("profile: ok"), "{stdout}");
    assert!(stderr(&output).contains("profile: ok (local)"));
}

#[test]
fn check_explain_risk_does_not_open_db_connection() {
    let temp = tempfile::tempdir().expect("tempdir");
    let db = temp.path().join("does-not-exist.sqlite");
    let project = temp.path().join("project.toml");
    let policy = temp.path().join("policy.toml");
    std::fs::write(&policy, "[policy.database]\n").expect("policy");
    write_profile(&project, &db, &policy);

    let mut cmd = Command::cargo_bin("gaze-lens").expect("binary");
    let output = cmd
        .args([
            "--project-config",
            project.to_str().expect("project path"),
            "check",
            "--profile",
            "local",
            "--explain-risk",
            "--format",
            "json",
        ])
        .output()
        .expect("check");

    assert!(output.status.success(), "stderr: {}", stderr(&output));
}

#[test]
fn check_explain_risk_does_not_call_resolve_password() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("project.toml");
    write_keyring_profile(&project, "prod", "missing-service", "missing-account");

    let mut cmd = Command::cargo_bin("gaze-lens").expect("binary");
    let output = cmd
        .args([
            "--project-config",
            project.to_str().expect("project path"),
            "check",
            "--profile",
            "prod",
            "--explain-risk",
            "--format",
            "json",
        ])
        .output()
        .expect("check");

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let v: serde_json::Value = serde_json::from_slice(&output.stdout).expect("json stdout");
    assert_eq!(v["at_rest_surface"]["secret_backend"]["backend"], "keyring");
    assert!(
        v["at_rest_surface"]["secret_backend"]["identity"]
            .as_str()
            .expect("identity")
            .contains("service=missing-service")
    );
}

#[test]
fn check_explain_risk_json_fails_loudly_when_pipeline_build_fails() {
    let temp = tempfile::tempdir().expect("tempdir");
    let db = temp.path().join("fixture.sqlite");
    let project = temp.path().join("project.toml");
    let policy = temp.path().join("policy.toml");
    seed_sqlite(&db);
    std::fs::write(
        &policy,
        r#"
        [policy.database]
        [[policy.database.columns]]
        column = "email"
        class = "email"
        action = "explode"
        "#,
    )
    .expect("policy");
    write_profile(&project, &db, &policy);

    let mut cmd = Command::cargo_bin("gaze-lens").expect("binary");
    let output = cmd
        .args([
            "--project-config",
            project.to_str().expect("project path"),
            "check",
            "--profile",
            "local",
            "--explain-risk",
            "--format",
            "json",
        ])
        .output()
        .expect("check");

    assert!(!output.status.success(), "stdout: {}", stdout(&output));
    assert!(output.stdout.is_empty(), "stdout: {}", stdout(&output));
    assert!(stderr(&output).contains("failed to build policy pipeline"));
}

#[tokio::test]
async fn validate_secret_ok_for_keyring_does_not_print_value() {
    install_builder();
    let (service, account) = unique_names("ok");
    set_secret(&service, &account, "hunter2-ok");
    let profile = keyring_profile("prod", &service, &account);

    let meta = validate_secret(&profile).await.expect("secret");
    let display = meta.to_string();
    assert!(display.contains("keyring service="), "{display}");
    assert!(!display.contains("hunter2-ok"), "{display}");
}

#[tokio::test]
async fn validate_secret_missing_returns_keyring_missing() {
    install_builder();
    let (service, account) = unique_names("missing");
    let profile = keyring_profile("prod", &service, &account);

    let err = validate_secret(&profile).await.expect_err("missing");
    assert!(matches!(
        err,
        LensError::SecretKeyringMissing { service: ref s, account: ref a }
            if s == &service && a == &account
    ));
}

#[tokio::test]
async fn check_run_renders_secret_not_found_then_returns_error() {
    install_builder();
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("project.toml");
    let (service, account) = unique_names("run-missing");
    write_keyring_profile(&project, "prod", &service, &account);
    let mut out = Vec::new();

    let err = run_with_writer_for_test(
        CheckArgs {
            profile: "prod".into(),
            explain_risk: false,
            format: gaze_lens::cli::check_trust::TrustFormat::Text,
        },
        Some(&project),
        None,
        &mut out,
    )
    .await
    .expect_err("missing");

    let stdout = String::from_utf8(out).expect("utf8");
    assert!(stdout.contains("secret: NOT FOUND"), "{stdout}");
    assert!(stdout.contains("rerun"), "{stdout}");
    assert!(!stdout.contains("hunter2"), "{stdout}");
    assert!(matches!(err, LensError::SecretKeyringMissing { .. }));
}

#[tokio::test]
async fn check_run_renders_secret_access_denied_then_returns_error() {
    install_builder();
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("project.toml");
    let (service, account) = unique_names("run-denied");
    write_keyring_profile(&project, "prod", &service, &account);
    set_fault(&service, &account, Fault::Denied);
    let mut out = Vec::new();

    let err = run_with_writer_for_test(
        CheckArgs {
            profile: "prod".into(),
            explain_risk: false,
            format: gaze_lens::cli::check_trust::TrustFormat::Text,
        },
        Some(&project),
        None,
        &mut out,
    )
    .await
    .expect_err("denied");

    let stdout = String::from_utf8(out).expect("utf8");
    assert!(stdout.contains("secret: ACCESS DENIED"), "{stdout}");
    assert!(!stdout.contains("hunter2"), "{stdout}");
    assert!(matches!(err, LensError::SecretKeyringDenied { .. }));
}

#[tokio::test]
async fn check_run_renders_secret_backend_unavailable_then_returns_error() {
    install_builder();
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("project.toml");
    let (service, account) = unique_names("run-unavailable");
    write_keyring_profile(&project, "prod", &service, &account);
    set_fault(&service, &account, Fault::Unavailable);
    let mut out = Vec::new();

    let err = run_with_writer_for_test(
        CheckArgs {
            profile: "prod".into(),
            explain_risk: false,
            format: gaze_lens::cli::check_trust::TrustFormat::Text,
        },
        Some(&project),
        None,
        &mut out,
    )
    .await
    .expect_err("unavailable");

    let stdout = String::from_utf8(out).expect("utf8");
    assert!(stdout.contains("secret: BACKEND UNAVAILABLE"), "{stdout}");
    assert!(!stdout.contains("hunter2"), "{stdout}");
    assert!(matches!(err, LensError::SecretBackendUnavailable { .. }));
}

#[test]
fn check_reports_invalid_policy() {
    let temp = tempfile::tempdir().expect("tempdir");
    let db = temp.path().join("fixture.sqlite");
    let project = temp.path().join("project.toml");
    let policy = temp.path().join("policy.toml");
    seed_sqlite(&db);
    std::fs::write(&policy, "not valid toml =").expect("policy");
    write_profile(&project, &db, &policy);

    let mut cmd = Command::cargo_bin("gaze-lens").expect("binary");
    let output = cmd
        .args([
            "--project-config",
            project.to_str().expect("project path"),
            "check",
            "--profile",
            "local",
        ])
        .output()
        .expect("check");

    assert!(!output.status.success(), "stdout: {}", stdout(&output));
    assert!(stderr(&output).contains("failed to parse policy"));
}

#[test]
fn malformed_toml_exits_nonzero_with_hint() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join(".gaze-lens.toml");
    std::fs::write(
        &project,
        r#"
            [[profiles]
            name = "local"
        "#,
    )
    .expect("profile");

    let mut cmd = Command::cargo_bin("gaze-lens").expect("binary");
    let output = cmd
        .args([
            "--project-config",
            project.to_str().expect("project path"),
            "check",
            "--profile",
            "local",
        ])
        .output()
        .expect("check");

    let stderr = stderr(&output);
    assert!(!output.status.success(), "stdout: {}", stdout(&output));
    assert!(stderr.contains(&project.display().to_string()), "{stderr}");
    assert!(stderr.contains("line "), "{stderr}");
    assert!(stderr.contains("column "), "{stderr}");
}

fn seed_sqlite(path: &std::path::Path) {
    let conn = Connection::open(path).expect("sqlite");
    conn.execute_batch("CREATE TABLE users (id INTEGER PRIMARY KEY);")
        .expect("seed");
}

fn write_profile(path: &std::path::Path, db: &std::path::Path, policy: &std::path::Path) {
    std::fs::write(
        path,
        format!(
            r#"
            [[profiles]]
            name = "local"
            policy = "{}"
            source = {{ kind = "sqlite", path = "{}", readonly_required = true }}
            "#,
            policy.display(),
            db.display()
        ),
    )
    .expect("profile");
}

fn stdout(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[derive(Debug, Clone)]
enum Fault {
    Denied,
    Unavailable,
}

#[derive(Default)]
struct Store {
    secrets: HashMap<(String, String), Vec<u8>>,
    faults: HashMap<(String, String), Fault>,
}

static STORE: OnceLock<Mutex<Store>> = OnceLock::new();

fn store() -> &'static Mutex<Store> {
    STORE.get_or_init(|| Mutex::new(Store::default()))
}

fn install_builder() {
    keyring::set_default_credential_builder(Box::new(PersistentBuilder));
}

fn unique_names(test_name: &str) -> (String, String) {
    (
        "gaze-lens-check-test".to_string(),
        format!("{test_name}-{}", ulid::Ulid::new()),
    )
}

fn set_secret(service: &str, account: &str, value: &str) {
    store().lock().expect("store").secrets.insert(
        (service.to_string(), account.to_string()),
        value.as_bytes().to_vec(),
    );
}

fn set_fault(service: &str, account: &str, fault: Fault) {
    store()
        .lock()
        .expect("store")
        .faults
        .insert((service.to_string(), account.to_string()), fault);
}

fn keyring_profile(name: &str, service: &str, account: &str) -> Profile {
    Profile {
        name: name.to_string(),
        source: SourceSpec::Postgres {
            host: "127.0.0.1".to_string(),
            port: 5432,
            database: "app".to_string(),
            username: "ro".to_string(),
            password_env: None,
            secret: Some(SecretSpec::Keyring {
                service: service.to_string(),
                account: account.to_string(),
            }),
            ssh_host: None,
            local_port: None,
            readonly_required: true,
        },
        policy: None,
        discovered_from_ssh_host: None,
        discovered_from_path: None,
        discovered_at: None,
        discovered_ssh_host_key_fingerprint: None,
        credential_class: None,
        schema_allowlist: None,
        snapshot_retention_days: None,
        auto_purge: AutoPurge::Off,
    }
}

fn sqlite_profile(name: &str, path: std::path::PathBuf) -> Profile {
    Profile {
        name: name.to_string(),
        source: SourceSpec::Sqlite {
            path,
            readonly_required: true,
            json_text_columns: Vec::new(),
        },
        policy: None,
        discovered_from_ssh_host: None,
        discovered_from_path: None,
        discovered_at: None,
        discovered_ssh_host_key_fingerprint: None,
        credential_class: None,
        schema_allowlist: None,
        snapshot_retention_days: None,
        auto_purge: AutoPurge::Off,
    }
}

fn postgres_env_profile(name: &str, env: &str) -> Profile {
    Profile {
        name: name.to_string(),
        source: SourceSpec::Postgres {
            host: "127.0.0.1".to_string(),
            port: 5432,
            database: "app".to_string(),
            username: "ro".to_string(),
            password_env: Some(env.to_string()),
            secret: None,
            ssh_host: Some("db-bastion.example".to_string()),
            local_port: Some(15432),
            readonly_required: true,
        },
        policy: None,
        discovered_from_ssh_host: None,
        discovered_from_path: None,
        discovered_at: None,
        discovered_ssh_host_key_fingerprint: None,
        credential_class: None,
        schema_allowlist: None,
        snapshot_retention_days: None,
        auto_purge: AutoPurge::Off,
    }
}

fn random_hex_32_bytes() -> String {
    format!("{}{}", ulid::Ulid::new(), ulid::Ulid::new())
}

fn write_keyring_profile(path: &std::path::Path, name: &str, service: &str, account: &str) {
    std::fs::write(
        path,
        format!(
            r#"
            [[profiles]]
            name = "{name}"
            source = {{ kind = "postgres", host = "127.0.0.1", port = 5432, database = "app", username = "ro", secret = {{ type = "keyring", service = "{service}", account = "{account}" }} }}
            "#
        ),
    )
    .expect("profile");
}

#[derive(Debug)]
struct PersistentBuilder;

impl CredentialBuilderApi for PersistentBuilder {
    fn build(
        &self,
        _target: Option<&str>,
        service: &str,
        user: &str,
    ) -> keyring::Result<Box<Credential>> {
        Ok(Box::new(PersistentCredential {
            service: service.to_string(),
            account: user.to_string(),
        }))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn persistence(&self) -> CredentialPersistence {
        CredentialPersistence::ProcessOnly
    }
}

#[derive(Debug)]
struct PersistentCredential {
    service: String,
    account: String,
}

impl PersistentCredential {
    fn key(&self) -> (String, String) {
        (self.service.clone(), self.account.clone())
    }
}

impl CredentialApi for PersistentCredential {
    fn set_secret(&self, secret: &[u8]) -> keyring::Result<()> {
        store()
            .lock()
            .expect("store")
            .secrets
            .insert(self.key(), secret.to_vec());
        Ok(())
    }

    fn get_secret(&self) -> keyring::Result<Vec<u8>> {
        let mut store = store().lock().expect("store");
        let key = self.key();
        if let Some(fault) = store.faults.remove(&key) {
            return match fault {
                Fault::Denied => Err(keyring::Error::NoStorageAccess(Box::new(
                    std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied"),
                ))),
                Fault::Unavailable => Err(keyring::Error::PlatformFailure(Box::new(
                    std::io::Error::other("dbus unavailable"),
                ))),
            };
        }
        store
            .secrets
            .get(&key)
            .cloned()
            .ok_or(keyring::Error::NoEntry)
    }

    fn delete_credential(&self) -> keyring::Result<()> {
        store()
            .lock()
            .expect("store")
            .secrets
            .remove(&self.key())
            .map(|_| ())
            .ok_or(keyring::Error::NoEntry)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
