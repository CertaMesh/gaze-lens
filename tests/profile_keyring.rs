//! Keyring resolver tests install a process-global credential builder. Every
//! test uses a unique service/account pair so parallel test execution cannot
//! collide through that global keyring hook.

use std::any::Any;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use gaze_lens::errors::LensError;
use gaze_lens::profile::{Profile, SecretSpec, SourceSpec};
use gaze_lens::session::maintenance::AutoPurge;
use keyring::credential::{Credential, CredentialApi, CredentialBuilderApi, CredentialPersistence};

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
    let suffix = ulid::Ulid::new().to_string();
    (
        "gaze-lens-test".to_string(),
        format!("{test_name}-{suffix}"),
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

fn profile(service: &str, account: &str) -> Profile {
    Profile {
        name: "prod".to_string(),
        source: SourceSpec::Postgres {
            host: "db".to_string(),
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
        schema_tokenize: None,
        schema_allowlist: None,
        snapshot_retention_days: None,
        auto_purge: AutoPurge::Off,
    }
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
        let mut store = store().lock().expect("store");
        store.secrets.insert(self.key(), secret.to_vec());
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

    fn debug_fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PersistentCredential")
            .field("service", &self.service)
            .field("account", &self.account)
            .finish()
    }
}

#[tokio::test]
async fn keyring_secret_resolved_at_connection_time_not_load_time() {
    install_builder();
    let (service, account) = unique_names("lazy");
    let profile = profile(&service, &account);

    set_secret(&service, &account, "secret-at-connect");

    let password = profile.resolve_password().await.expect("password");
    assert_eq!(password.as_str(), "secret-at-connect");
}

#[tokio::test]
async fn keyring_missing_entry_returns_secret_keyring_missing() {
    install_builder();
    let (service, account) = unique_names("missing");
    let profile = profile(&service, &account);

    let err = profile.resolve_password().await.expect_err("missing");
    assert!(matches!(
        err,
        LensError::SecretKeyringMissing { service: ref s, account: ref a }
            if s == &service && a == &account
    ));
}

#[tokio::test]
async fn keyring_access_denied_returns_secret_keyring_denied() {
    install_builder();
    let (service, account) = unique_names("denied");
    let profile = profile(&service, &account);
    set_fault(&service, &account, Fault::Denied);

    let err = profile.resolve_password().await.expect_err("denied");
    assert!(matches!(
        err,
        LensError::SecretKeyringDenied { service: ref s, account: ref a }
            if s == &service && a == &account
    ));
}

#[tokio::test]
async fn keyring_backend_unavailable_returns_backend_unavailable() {
    install_builder();
    let (service, account) = unique_names("unavailable");
    let profile = profile(&service, &account);
    set_fault(&service, &account, Fault::Unavailable);

    let err = profile.resolve_password().await.expect_err("unavailable");
    assert!(matches!(
        err,
        LensError::SecretBackendUnavailable { backend, .. } if backend == "platform"
    ));
}
