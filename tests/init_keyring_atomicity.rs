use std::any::Any;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use gaze_lens::cli::init::batch::RealBatchWriter;
use gaze_lens::cli::init::flow::{InitEnv, run_guided};
use gaze_lens::cli::init::prompter::FakePrompter;
use gaze_lens::cli::init::{
    InitArgs, InitScope, SecretBackendChoice, SourceKind, commit_plan_for_test,
};
use keyring::credential::{Credential, CredentialApi, CredentialBuilderApi, CredentialPersistence};

#[derive(Default)]
struct Store {
    secrets: HashMap<(String, String), Vec<u8>>,
}

static STORE: OnceLock<Mutex<Store>> = OnceLock::new();

fn store() -> &'static Mutex<Store> {
    STORE.get_or_init(|| Mutex::new(Store::default()))
}

fn install_builder() {
    keyring::set_default_credential_builder(Box::new(PersistentBuilder));
}

fn set_secret(service: &str, account: &str, value: &str) {
    store().lock().expect("store").secrets.insert(
        (service.to_string(), account.to_string()),
        value.as_bytes().to_vec(),
    );
}

fn get_secret(service: &str, account: &str) -> Option<String> {
    store()
        .lock()
        .expect("store")
        .secrets
        .get(&(service.to_string(), account.to_string()))
        .map(|bytes| String::from_utf8(bytes.clone()).expect("utf8"))
}

fn keyring_args(dir: &tempfile::TempDir) -> (InitArgs, InitEnv) {
    let cwd = dir.path().join("cwd");
    let home = dir.path().join("home");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    let mut args = InitArgs::default_for_test();
    args.profile = Some("prod".into());
    args.source_kind = Some(SourceKind::Postgres);
    args.source_host = Some("db".into());
    args.source_port = Some(5432);
    args.source_database = Some("app".into());
    args.source_username = Some("ro".into());
    args.secret_backend = SecretBackendChoice::Keyring;
    args.source_password_keyring_service = Some("gaze-lens-test".into());
    args.source_password_keyring_account = Some(format!("prod-{}", ulid::Ulid::new()));
    args.scope = Some(InitScope::User);
    args.no_mcp_config = true;
    args.no_agents_md = true;
    (
        args,
        InitEnv::test_with_home(home.clone(), cwd.clone(), None, None),
    )
}

#[test]
fn commit_plan_for_test_with_keyring_writes_entry_and_no_password_on_disk() {
    install_builder();
    let dir = tempfile::tempdir().unwrap();
    let (args, env) = keyring_args(&dir);
    let service = args.source_password_keyring_service.clone().unwrap();
    let account = args.source_password_keyring_account.clone().unwrap();
    let mut p = FakePrompter::new()
        .with_confirm(true)
        .with_password("hunter2-disk-test");
    let plan = run_guided(&args, &mut p, &env).expect("plan");
    let mut writer = RealBatchWriter;

    commit_plan_for_test(&args, &plan, &mut writer).expect("commit");

    assert_eq!(
        get_secret(&service, &account).as_deref(),
        Some("hunter2-disk-test")
    );
    let profile_bytes = std::fs::read_to_string(&plan.profile_path).expect("profile");
    assert!(
        !profile_bytes.contains("hunter2-disk-test"),
        "{profile_bytes}"
    );
    assert!(!profile_bytes.contains("password ="), "{profile_bytes}");
}

#[test]
fn existing_keyring_entry_without_allow_overwrite_rejects_before_file_write() {
    install_builder();
    let dir = tempfile::tempdir().unwrap();
    let (args, env) = keyring_args(&dir);
    let service = args.source_password_keyring_service.clone().unwrap();
    let account = args.source_password_keyring_account.clone().unwrap();
    set_secret(&service, &account, "old-value");
    let mut p = FakePrompter::new()
        .with_confirm(true)
        .with_password("new-value");
    let plan = run_guided(&args, &mut p, &env).expect("plan");
    let mut writer = RealBatchWriter;

    let err = commit_plan_for_test(&args, &plan, &mut writer).expect_err("reject overwrite");

    assert!(err.to_string().contains("--allow-overwrite"), "{err}");
    assert_eq!(get_secret(&service, &account).as_deref(), Some("old-value"));
    assert!(
        !plan.profile_path.exists(),
        "profile file must not be written"
    );
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
        store()
            .lock()
            .expect("store")
            .secrets
            .get(&self.key())
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
