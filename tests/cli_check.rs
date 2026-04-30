use assert_cmd::Command;
use gaze_lens::cli::check::{CheckArgs, run_with_writer_for_test, validate_secret};
use gaze_lens::errors::LensError;
use gaze_lens::profile::{Profile, SecretSpec, SourceSpec};
use gaze_lens::session::maintenance::AutoPurge;
use keyring::credential::{Credential, CredentialApi, CredentialBuilderApi, CredentialPersistence};
use rusqlite::Connection;
use std::any::Any;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

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
        schema_allowlist: None,
        snapshot_retention_days: None,
        auto_purge: AutoPurge::Off,
    }
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
