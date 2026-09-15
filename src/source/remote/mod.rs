//! First-party TLS log source. All returned text still requires local Gaze.
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use rustls::pki_types::{CertificateDer, ServerName, pem::PemObject};
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio_rustls::{TlsConnector, rustls};
use zeroize::Zeroizing;

use crate::errors::LensError;
use crate::profile::SecretSpec;
use crate::session::ToolCall;
use crate::source::{Source, SourceOutput};

pub mod service;
#[cfg(test)]
pub(crate) mod test_support;
pub mod watchdog;
mod wire;

pub(crate) type Result<T> = std::result::Result<T, LensError>;
pub(crate) fn failure() -> LensError {
    LensError::SourceError {
        source_name: "remote log".into(),
        detail: "remote operation rejected".into(),
        sql: None,
        stderr: None,
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RemoteConfig {
    pub endpoint: String,
    pub server_name: String,
    pub trust_root: PathBuf,
    pub resource: String,
    pub secret: SecretSpec,
}
impl RemoteConfig {
    pub fn validate(&self) -> Result<()> {
        if self.endpoint.parse::<std::net::SocketAddr>().is_err()
            || ServerName::try_from(self.server_name.clone()).is_err()
            || self.trust_root.as_os_str().is_empty()
            || !wire::valid_name(&self.resource)
        {
            return Err(failure());
        }
        Ok(())
    }
    async fn token(&self) -> Result<Zeroizing<String>> {
        let token = match &self.secret {
            SecretSpec::Env { var } => Zeroizing::new(std::env::var(var).map_err(|_| failure())?),
            SecretSpec::Keyring { service, account } => {
                let service = service.clone();
                let account = account.clone();
                local_blocking(move || {
                    keyring::Entry::new(&service, &account)
                        .and_then(|e| e.get_password())
                        .map(Zeroizing::new)
                        .map_err(|_| failure())
                })
                .await?
            }
        };
        if !wire::valid_token(&token) {
            return Err(failure());
        }
        Ok(token)
    }
}

// Timed-out filesystem/keyring calls cannot be cancelled by Tokio. Their
// actual blocking job owns this single non-queuing budget until it exits.
async fn local_blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    static BUDGET: std::sync::OnceLock<Arc<tokio::sync::Semaphore>> = std::sync::OnceLock::new();
    blocking_on(
        Arc::clone(BUDGET.get_or_init(|| Arc::new(tokio::sync::Semaphore::new(1)))),
        work,
    )
    .await
}
async fn blocking_on<T: Send + 'static>(
    budget: Arc<tokio::sync::Semaphore>,
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    let permit = budget.try_acquire_owned().map_err(|_| failure())?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        work()
    })
    .await
    .map_err(|_| failure())?
}

pub(crate) fn client_tls(bytes: &[u8]) -> Result<rustls::ClientConfig> {
    let mut roots = rustls::RootCertStore::empty();
    for cert in CertificateDer::pem_slice_iter(bytes) {
        roots
            .add(cert.map_err(|_| failure())?)
            .map_err(|_| failure())?;
    }
    if roots.is_empty() {
        return Err(failure());
    }
    // Explicit provider, normal hostname verification, and the default NoKeyLog.
    rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .map_err(|_| failure())
        .map(|b| b.with_root_certificates(roots).with_no_client_auth())
}

pub(crate) fn bounded_file(path: &Path, cap: usize) -> Result<Vec<u8>> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| failure())?;
    if !file.metadata().map_err(|_| failure())?.is_file() {
        return Err(failure());
    }
    let mut bytes = Vec::new();
    file.take((cap + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| failure())?;
    if bytes.len() > cap {
        return Err(failure());
    }
    Ok(bytes)
}

pub struct RemoteMcpSource {
    config: RemoteConfig,
}
impl RemoteMcpSource {
    pub fn new(config: RemoteConfig) -> Result<Self> {
        config.validate()?;
        Ok(Self { config })
    }
    pub async fn tail(&self, lines: usize) -> Result<SourceOutput> {
        if !(1..=wire::MAX_LINES).contains(&lines) {
            return Err(failure());
        }
        tokio::time::timeout(Duration::from_secs(30), self.exchange(lines))
            .await
            .map_err(|_| failure())?
    }
    async fn exchange(&self, lines: usize) -> Result<SourceOutput> {
        let token = self.config.token().await?;
        let path = self.config.trust_root.clone();
        let bytes = local_blocking(move || bounded_file(&path, 64 * 1024)).await?;
        let tls = client_tls(&bytes)?;
        let name = ServerName::try_from(self.config.server_name.clone()).map_err(|_| failure())?;
        let stream = tokio::time::timeout(Duration::from_secs(5), async {
            let tcp = tokio::net::TcpStream::connect(&self.config.endpoint)
                .await
                .map_err(|_| failure())?;
            TlsConnector::from(Arc::new(tls))
                .connect(name, tcp)
                .await
                .map_err(|_| failure())
        })
        .await
        .map_err(|_| failure())??;
        let mut io = BufReader::new(stream);
        wire::write(io.get_mut(), &wire::initialize()).await?;
        wire::expect(&mut io, wire::ready()).await?;
        wire::write(io.get_mut(), &wire::initialized()).await?;
        wire::write(io.get_mut(), &wire::list()).await?;
        wire::expect(&mut io, wire::tools()).await?;
        wire::write(
            io.get_mut(),
            &wire::call(&self.config.resource, lines, &token),
        )
        .await?;
        io.get_mut().shutdown().await.map_err(|_| failure())?;
        let output = wire::decode_response(wire::read(&mut io).await?, lines)?;
        // rustls reports an unclean TCP EOF as an error. Wait for authenticated
        // close_notify, rejecting even delayed extra bytes before invoking Gaze.
        if !io.fill_buf().await.map_err(|_| failure())?.is_empty() {
            return Err(failure());
        }
        io.get_mut().shutdown().await.map_err(|_| failure())?;
        Ok(output.into_output())
    }
}
#[async_trait]
impl Source for RemoteMcpSource {
    async fn dispatch(&self, call: &ToolCall) -> Result<SourceOutput> {
        if call.tool_name != "log_tail" {
            return Err(failure());
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Args {
            profile: String,
            lines: Option<usize>,
        }
        let args: Args = serde_json::from_value(call.args.0.clone()).map_err(|_| failure())?;
        crate::profile::validate_profile_name(&args.profile).map_err(|_| failure())?;
        self.tail(args.lines.unwrap_or(100)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::io::AsyncWriteExt;
    use tokio_rustls::TlsAcceptor;
    fn pem(bytes: &[u8]) -> String {
        use base64::Engine;
        format!(
            "-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----\n",
            base64::engine::general_purpose::STANDARD.encode(bytes)
        )
    }
    #[tokio::test]
    async fn cancelled_local_job_retains_nonqueuing_budget() {
        let budget = Arc::new(tokio::sync::Semaphore::new(1));
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let work = tokio::spawn(blocking_on(Arc::clone(&budget), move || {
            let _ = started_tx.send(());
            release_rx.recv().unwrap();
            Ok(())
        }));
        started_rx.await.unwrap();
        work.abort();
        let _ = work.await;
        for _ in 0..100 {
            assert!(
                blocking_on::<()>(Arc::clone(&budget), || panic!("queued job"))
                    .await
                    .is_err()
            );
        }
        release_tx.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while budget.available_permits() == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(blocking_on(budget, || Ok(())).await.is_ok());
    }
    #[tokio::test]
    async fn hostile_tls_peers_never_return_unvalidated_content() {
        for mode in [
            "instructions",
            "metadata",
            "error",
            "is_error",
            "extra",
            "unclean",
            "utf8",
            "oversize",
            "duplicate",
        ] {
            let fixture = tempfile::tempdir().unwrap();
            let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
            let root = fixture.path().join("root.pem");
            std::fs::write(&root, pem(cert.cert.der().as_ref())).unwrap();
            let tls = rustls::ServerConfig::builder_with_provider(Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(
                vec![cert.cert.der().clone()],
                rustls::pki_types::PrivatePkcs8KeyDer::from(cert.signing_key.serialize_der())
                    .into(),
            )
            .unwrap();
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let endpoint = listener.local_addr().unwrap().to_string();
            let peer = tokio::spawn(async move {
                let (tcp, _) = listener.accept().await.unwrap();
                let stream = TlsAcceptor::from(Arc::new(tls)).accept(tcp).await.unwrap();
                let mut io = BufReader::new(stream);
                wire::expect(&mut io, wire::initialize()).await.unwrap();
                let mut ready = wire::ready();
                if mode == "instructions" {
                    ready["result"]["instructions"] = json!("RAW_ERROR_CANARY bob@example.test");
                }
                wire::write(io.get_mut(), &ready).await.unwrap();
                if mode == "instructions" {
                    return;
                }
                wire::expect(&mut io, wire::initialized()).await.unwrap();
                wire::expect(&mut io, wire::list()).await.unwrap();
                let mut tools = wire::tools();
                if mode == "metadata" {
                    tools["result"]["_meta"] = json!({"raw":"RAW_ERROR_CANARY"});
                }
                wire::write(io.get_mut(), &tools).await.unwrap();
                if mode == "metadata" {
                    return;
                }
                let _ = wire::read(&mut io).await.unwrap();
                assert!(io.fill_buf().await.unwrap().is_empty());
                let mut result = wire::response(wire::LogBody {
                    lines: vec!["bob@example.test".into()],
                    truncated: vec![],
                });
                match mode {
                    "error" => {
                        result = json!({"jsonrpc":"2.0","id":3,"error":{"code":-1,"message":"RAW_ERROR_CANARY"}})
                    }
                    "is_error" => result["result"]["isError"] = json!(true),
                    "utf8" => {
                        let _ = io.get_mut().write_all(b"\xff\n").await;
                        return;
                    }
                    "oversize" => {
                        let _ = io
                            .get_mut()
                            .write_all(&vec![b'x'; wire::FRAME_BYTES + 1])
                            .await;
                        return;
                    }
                    "duplicate" => {
                        let _ = io
                            .get_mut()
                            .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":3,\"id\":3}\n")
                            .await;
                        return;
                    }
                    _ => {}
                }
                wire::write(io.get_mut(), &result).await.unwrap();
                if mode == "extra" {
                    tokio::time::sleep(Duration::from_millis(30)).await;
                    let _ = wire::write(io.get_mut(), &json!({"raw":"RAW_ERROR_CANARY"})).await;
                }
                if mode != "unclean" {
                    let _ = io.get_mut().shutdown().await;
                }
            });
            let var = format!("GAZE_HOSTILE_{}", ulid::Ulid::new());
            // SAFETY: unique test environment variable; never overwritten.
            unsafe { std::env::set_var(&var, "a".repeat(64)) };
            let source = RemoteMcpSource::new(RemoteConfig {
                endpoint,
                server_name: "localhost".into(),
                trust_root: root,
                resource: "app".into(),
                secret: SecretSpec::Env { var },
            })
            .unwrap();
            let err = source.tail(1).await.expect_err(mode);
            assert!(!format!("{err:?}").contains("CANARY"));
            assert!(!format!("{err:?}").contains("bob@example.test"));
            peer.await.unwrap();
        }
    }
}
