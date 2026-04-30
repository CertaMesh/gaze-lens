use std::any::Any;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use gaze_lens::cli::init::flow::{InitEnv, run_guided};
use gaze_lens::cli::init::plan::PlannedSecret;
use gaze_lens::cli::init::prompter::FakePrompter;
use gaze_lens::cli::init::{
    InitArgs, InitScope, SecretBackendChoice, SourceKind, run_with_prompter_for_test,
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

#[test]
fn flow_keyring_branch_prompts_password_and_captures_via_planned_secret() {
    let env = InitEnv::test_with_home("/tmp/fake-home", "/tmp/fake-cwd", None, None);
    let mut args = InitArgs::default_for_test();
    args.profile = Some("prod".into());
    args.source_kind = Some(SourceKind::Postgres);
    args.source_host = Some("db".into());
    args.source_port = Some(5432);
    args.source_database = Some("app".into());
    args.source_username = Some("ro".into());
    args.secret_backend = SecretBackendChoice::Keyring;
    args.scope = Some(InitScope::User);
    args.no_mcp_config = true;
    args.no_agents_md = true;
    let mut p = FakePrompter::new()
        .with_text("gaze-lens")
        .with_text("prod")
        .with_confirm(true)
        .with_password("hunter2-flow");

    let plan = run_guided(&args, &mut p, &env).expect("plan");
    match plan.profile_section.source_secret {
        Some(PlannedSecret::Keyring {
            service,
            account,
            write_value: Some(value),
        }) => {
            assert_eq!(service, "gaze-lens");
            assert_eq!(account, "prod");
            assert_eq!(value.as_str(), "hunter2-flow");
        }
        other => panic!("expected keyring write secret, got {other:?}"),
    }
}

#[test]
fn flow_keyring_branch_with_no_keyring_write_skips_password_prompt() {
    let env = InitEnv::test_with_home("/tmp/fake-home", "/tmp/fake-cwd", None, None);
    let mut args = InitArgs::default_for_test();
    args.non_interactive = true;
    args.profile = Some("prod".into());
    args.source_kind = Some(SourceKind::Mysql);
    args.source_host = Some("db".into());
    args.source_port = Some(3306);
    args.source_database = Some("app".into());
    args.source_username = Some("ro".into());
    args.secret_backend = SecretBackendChoice::Keyring;
    args.source_password_keyring_service = Some("gaze-lens".into());
    args.source_password_keyring_account = Some("prod".into());
    args.no_keyring_write = true;
    args.scope = Some(InitScope::User);
    args.no_mcp_config = true;
    args.no_agents_md = true;
    let mut p = FakePrompter::new();

    let plan = run_guided(&args, &mut p, &env).expect("plan");
    match plan.profile_section.source_secret {
        Some(PlannedSecret::Keyring {
            service,
            account,
            write_value: None,
        }) => {
            assert_eq!(service, "gaze-lens");
            assert_eq!(account, "prod");
        }
        other => panic!("expected keyring metadata without write, got {other:?}"),
    }
}

#[test]
fn smoke_check_with_keyring_and_write_value_reports_secret_ok() {
    install_builder();
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let cwd = dir.path().join("cwd");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&cwd).unwrap();
    let service = "gaze-lens-smoke";
    let account = format!("prod-{}", ulid::Ulid::new());
    let env = InitEnv::test_with_home(home, cwd, None, None);
    let mut args = InitArgs::default_for_test();
    args.profile = Some("prod".into());
    args.source_kind = Some(SourceKind::Postgres);
    args.source_host = Some("db".into());
    args.source_port = Some(5432);
    args.source_database = Some("app".into());
    args.source_username = Some("ro".into());
    args.secret_backend = SecretBackendChoice::Keyring;
    args.source_password_keyring_service = Some(service.into());
    args.source_password_keyring_account = Some(account.clone());
    args.scope = Some(InitScope::User);
    args.no_mcp_config = true;
    args.no_agents_md = true;
    args.smoke_check = true;
    let mut p = FakePrompter::new()
        .with_confirm(true)
        .with_password("smoke-secret");
    let mut out = Vec::new();

    let result = run_with_prompter_for_test(&args, &env, &mut p, &mut out);

    let stdout = String::from_utf8(out).expect("utf8 output");
    assert!(
        stdout.contains(&format!(
            "secret: ok (keyring service={service} account={account})"
        )),
        "{stdout}"
    );
    assert!(!stdout.contains("smoke-secret"), "{stdout}");
    assert!(
        result.is_err(),
        "test uses a fake DB host; only the smoke-check secret line is in scope"
    );
}

#[test]
fn smoke_check_with_keyring_no_keyring_write_reports_not_found() {
    install_builder();
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let cwd = dir.path().join("cwd");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&cwd).unwrap();
    let service = "gaze-lens-smoke";
    let account = format!("missing-{}", ulid::Ulid::new());
    let env = InitEnv::test_with_home(home, cwd, None, None);
    let mut args = InitArgs::default_for_test();
    args.profile = Some("prod".into());
    args.source_kind = Some(SourceKind::Postgres);
    args.source_host = Some("db".into());
    args.source_port = Some(5432);
    args.source_database = Some("app".into());
    args.source_username = Some("ro".into());
    args.secret_backend = SecretBackendChoice::Keyring;
    args.source_password_keyring_service = Some(service.into());
    args.source_password_keyring_account = Some(account.clone());
    args.no_keyring_write = true;
    args.scope = Some(InitScope::User);
    args.no_mcp_config = true;
    args.no_agents_md = true;
    args.smoke_check = true;
    let mut p = FakePrompter::new();
    let mut out = Vec::new();

    let err =
        run_with_prompter_for_test(&args, &env, &mut p, &mut out).expect_err("missing keyring");

    let stdout = String::from_utf8(out).expect("utf8 output");
    assert!(
        stdout.contains(&format!(
            "secret: NOT FOUND (keyring service={service} account={account})"
        )),
        "{stdout}"
    );
    assert!(!stdout.contains("smoke-secret"), "{stdout}");
    assert!(err.to_string().contains("keyring entry not found"), "{err}");
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
