//! One authenticated v2 exchange per TLS connection. Readiness only in this slice.
use crate::{
    auth::{Authority, Pinned},
    config::{Checked, bounded_file},
    history::History,
};
use gaze_lens_protocol::{
    Error, Result,
    bounds::{self, FrameBuffer},
    wire::{
        self, Failure, Operation, Prepared, Privacy, ReadinessStatus, ResultBody, Success, Version,
    },
};
use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::TcpListener,
    sync::Semaphore,
    task::JoinSet,
};

/// Frame and connection deadlines. Fixture values exist for tests only; they
/// are deliberately not operator-configurable.
#[derive(Clone, Copy)]
pub(crate) struct Deadlines {
    io: Duration,
    call: Duration,
}
impl Default for Deadlines {
    fn default() -> Self {
        Self {
            io: Duration::from_secs(bounds::IO_SECONDS),
            call: Duration::from_secs(bounds::CALL_SECONDS),
        }
    }
}
#[cfg(test)]
impl Deadlines {
    pub(crate) fn fixture(io: Duration, call: Duration) -> Self {
        Self { io, call }
    }
}
pub async fn run(path: &Path) -> Result<()> {
    let config = crate::config::check(path).await?;
    let listener = TcpListener::bind(config.listen)
        .await
        .map_err(|_| Error::Unavailable)?;
    serve(listener, config).await
}
/// One rejected peer must not end the service, and neither must a temporary
/// descriptor shortage; a socket that can never yield another connection must.
const ACCEPT_BACKOFF: Duration = Duration::from_millis(100);
/// Consecutive resource failures tolerated before the listener is treated as
/// dead. At `ACCEPT_BACKOFF` each this is a bounded stall, not an exit on the
/// first `EMFILE`; `EBADF`-class failures share that uncategorized error kind.
const ACCEPT_FAILURES: u32 = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Accept {
    /// The failure belonged to one discarded peer; accept again immediately.
    Peer,
    /// A process or host resource is exhausted; pause briefly, then retry.
    Resource,
    /// The descriptor is not a usable listening socket any more.
    Dead,
}
pub(crate) fn classify(error: &std::io::Error) -> Accept {
    use std::io::ErrorKind;
    match error.kind() {
        ErrorKind::ConnectionAborted
        | ErrorKind::ConnectionReset
        | ErrorKind::ConnectionRefused
        | ErrorKind::Interrupted
        | ErrorKind::TimedOut
        | ErrorKind::WouldBlock => Accept::Peer,
        ErrorKind::InvalidInput
        | ErrorKind::NotConnected
        | ErrorKind::BrokenPipe
        | ErrorKind::AddrNotAvailable
        | ErrorKind::NotFound => Accept::Dead,
        // EMFILE, ENFILE, ENOBUFS, ENOMEM and EBADF share the uncategorized
        // kind on stable Rust, so the repeat ceiling decides between them.
        _ => Accept::Resource,
    }
}
pub async fn serve(listener: TcpListener, config: Checked) -> Result<()> {
    serve_with(listener, config, Deadlines::default()).await
}
pub(crate) async fn serve_with(
    listener: TcpListener,
    config: Checked,
    deadlines: Deadlines,
) -> Result<()> {
    let authority = Authority::parse(&bounded_file(&config.authority).await?)?;
    let history_path = config.history.clone();
    let history = Arc::new(
        tokio::task::spawn_blocking(move || History::open(&history_path, &authority))
            .await
            .map_err(|_| Error::InternalFailure)??,
    );
    let config = Arc::new(config);
    let permits = Arc::new(Semaphore::new(bounds::DEFAULT_ACTIVE_CALLS));
    let principals = Arc::new(Mutex::new(HashMap::new()));
    let mut tasks = JoinSet::new();
    let mut failures = 0u32;
    loop {
        while tasks.try_join_next().is_some() {}
        tokio::select! {
            result = listener.accept() => {
                let socket = match result {
                    Ok((socket, _)) => {
                        failures = 0;
                        socket
                    }
                    Err(error) => match classify(&error) {
                        Accept::Peer => continue,
                        Accept::Dead => return Err(Error::Unavailable),
                        Accept::Resource => {
                            failures += 1;
                            if failures >= ACCEPT_FAILURES {
                                return Err(Error::Unavailable);
                            }
                            tokio::time::sleep(ACCEPT_BACKOFF).await;
                            continue;
                        }
                    },
                };
                let Ok(permit) = Arc::clone(&permits).try_acquire_owned() else { drop(socket); continue; };
                let config = Arc::clone(&config);
                let principals = Arc::clone(&principals);
                let history = Arc::clone(&history);
                tasks.spawn(async move {
                    let _permit = permit;
                    let _ = tokio::time::timeout(
                        deadlines.call,
                        connection(socket, config, principals, history, deadlines),
                    ).await;
                });
            }
            _ = tasks.join_next(), if !tasks.is_empty() => {}
        }
    }
}
async fn connection(
    socket: tokio::net::TcpStream,
    config: Arc<Checked>,
    principals: Arc<Mutex<HashMap<String, usize>>>,
    history: Arc<History>,
    deadlines: Deadlines,
) -> Result<()> {
    let mut io = tokio::time::timeout(deadlines.io, config.acceptor.accept(socket))
        .await
        .map_err(|_| Error::Timeout)?
        .map_err(|_| Error::Unauthorized)?;
    let p = wire::decode_prepare(&read(&mut io, bounds::PREPARE_BYTES, deadlines).await?)?;
    let pin = authority(&config.authority, &history)
        .await?
        .prepare(&p, now()?)?;
    // No executable values are requested for unimplemented operations.
    let admission = if p.operation == Operation::Readiness {
        PrincipalPermit::acquire(principals, pin.binding().principal.clone())
    } else {
        Err(Error::UnsupportedOperation)
    };
    let _principal = match admission {
        Ok(permit) => permit,
        Err(code) => {
            return finish(
                &mut io,
                &wire::encode_failure(&Failure {
                    version: Version::V2,
                    id: p.id,
                    code,
                })?,
                deadlines,
            )
            .await;
        }
    };
    let prepared = Prepared {
        version: Version::V2,
        privacy: Privacy::ClientGaze,
        id: p.id.clone(),
        operation: p.operation,
        binding: pin.binding().clone(),
    };
    write(&mut io, &wire::encode_prepared(&prepared)?, deadlines).await?;
    let outcome = call(&mut io, &config.authority, &pin, &p.id, &history, deadlines).await;
    let bytes = match outcome {
        Ok(bytes) => bytes,
        Err(code) => wire::encode_failure(&Failure {
            version: Version::V2,
            id: p.id,
            code,
        })?,
    };
    finish(&mut io, &bytes, deadlines).await
}
async fn finish<S: AsyncWrite + Unpin>(
    io: &mut S,
    bytes: &[u8],
    deadlines: Deadlines,
) -> Result<()> {
    write(io, bytes, deadlines).await?;
    tokio::time::timeout(deadlines.io, io.shutdown())
        .await
        .map_err(|_| Error::Timeout)?
        .map_err(|_| Error::Unavailable)
}
async fn call<S: AsyncRead + Unpin>(
    io: &mut S,
    path: &Path,
    pin: &Pinned,
    id: &str,
    history: &History,
    deadlines: Deadlines,
) -> Result<Vec<u8>> {
    let call = wire::decode_call(&read(io, bounds::FRAME_BYTES, deadlines).await?)?;
    // The Call may not change the operation authorized and pinned at Prepare.
    if call.id != id || call.args.operation() != pin.operation() {
        return Err(Error::InvalidRequest);
    }
    if &call.binding != pin.binding() {
        return Err(Error::BindingChanged);
    }
    authority(path, history).await?.revalidate(pin, now()?)?;
    // Readiness is the one implemented operation; the allow-list is in
    // `connection`, which refuses every other operation before Prepared.
    let success = Success {
        id: call.id.clone(),
        operation: pin.operation(),
        binding: pin.binding().clone(),
        result: ResultBody::Readiness {
            status: ReadinessStatus::Configured,
        },
    };
    let bytes = wire::encode_success(&success, &call)?;
    // Last authorization check is the release linearization point.
    authority(path, history).await?.revalidate(pin, now()?)?;
    Ok(bytes)
}
async fn authority(path: &Path, history: &History) -> Result<Authority> {
    let authority =
        Authority::parse(&bounded_file(path).await?).map_err(|_| Error::Unauthorized)?;
    history.validate(&authority)?;
    Ok(authority)
}
fn now() -> Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|x| x.as_secs())
        .map_err(|_| Error::Unavailable)
}
async fn read<S: AsyncRead + Unpin>(
    io: &mut S,
    max: usize,
    deadlines: Deadlines,
) -> Result<Vec<u8>> {
    tokio::time::timeout(deadlines.io, async {
        let mut buffer = FrameBuffer::new(max)?;
        let mut chunk = [0u8; 1024];
        loop {
            let n = io.read(&mut chunk).await.map_err(|_| Error::Unavailable)?;
            if n == 0 {
                return Err(Error::InvalidRequest);
            }
            if buffer.push(&chunk[..n])? {
                return buffer.finish();
            }
        }
    })
    .await
    .map_err(|_| Error::Timeout)?
}
async fn write<S: AsyncWrite + Unpin>(
    io: &mut S,
    bytes: &[u8],
    deadlines: Deadlines,
) -> Result<()> {
    tokio::time::timeout(deadlines.io, async {
        io.write_all(bytes).await.map_err(|_| Error::Unavailable)?;
        io.flush().await.map_err(|_| Error::Unavailable)
    })
    .await
    .map_err(|_| Error::Timeout)?
}
struct PrincipalPermit {
    active: Arc<Mutex<HashMap<String, usize>>>,
    id: String,
}
impl PrincipalPermit {
    fn acquire(active: Arc<Mutex<HashMap<String, usize>>>, id: String) -> Result<Self> {
        {
            // A panic elsewhere must not permanently close admission; the
            // counts behind the lock are plain integers with no invariant that
            // a partial update could break.
            let mut counts = active.lock().unwrap_or_else(PoisonError::into_inner);
            let count = counts.entry(id.clone()).or_default();
            if *count >= bounds::MAX_PRINCIPAL_CALLS {
                return Err(Error::Unavailable);
            }
            *count += 1;
        }
        Ok(Self { active, id })
    }
}
impl Drop for PrincipalPermit {
    fn drop(&mut self) {
        let mut counts = self.active.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(count) = counts.get_mut(&self.id) {
            *count -= 1;
            if *count == 0 {
                counts.remove(&self.id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Accept, Deadlines, TcpListener, classify, serve_with};
    use std::io::{Error, ErrorKind};
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio_rustls::{
        TlsConnector,
        rustls::{self, pki_types::ServerName},
    };

    fn pem(kind: &str, der: &[u8]) -> String {
        use base64::Engine;
        format!(
            "-----BEGIN {kind}-----\n{}\n-----END {kind}-----\n",
            base64::engine::general_purpose::STANDARD.encode(der)
        )
    }
    /// A server whose deadlines are short enough to observe in a test. The
    /// authority is empty: no connection here reaches Prepare.
    async fn server(
        deadlines: Deadlines,
    ) -> (tempfile::TempDir, std::net::SocketAddr, TlsConnector) {
        let directory = tempfile::tempdir().unwrap();
        let certified = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
        let cert = directory.path().join("cert.pem");
        let key = directory.path().join("key.pem");
        let authority = directory.path().join("authority.json");
        let history = directory.path().join("history.json");
        let config = directory.path().join("server.toml");
        std::fs::write(&cert, pem("CERTIFICATE", certified.cert.der())).unwrap();
        std::fs::write(
            &key,
            pem("PRIVATE KEY", &certified.signing_key.serialize_der()),
        )
        .unwrap();
        std::fs::write(
            &authority,
            br#"{"principals":[],"resources":[],"grants":[]}"#,
        )
        .unwrap();
        std::fs::write(&history, br#"{"version":1,"entries":[]}"#).unwrap();
        std::fs::write(
            &config,
            format!(
                "listen = '127.0.0.1:0'\ncertificate = '{}'\nprivate-key = '{}'\nauthority = '{}'\nhistory = '{}'\n",
                cert.display(),
                key.display(),
                authority.display(),
                history.display()
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
            for file in [&cert, &key, &authority, &history, &config] {
                std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o600)).unwrap();
            }
        }
        let checked = crate::config::check(&config).await.unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(serve_with(listener, checked, deadlines));
        let mut roots = rustls::RootCertStore::empty();
        roots.add(certified.cert.der().clone()).unwrap();
        let client = rustls::ClientConfig::builder_with_provider(std::sync::Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
        (
            directory,
            address,
            TlsConnector::from(std::sync::Arc::new(client)),
        )
    }
    fn deadlines() -> Deadlines {
        Deadlines::fixture(Duration::from_millis(250), Duration::from_secs(30))
    }

    #[tokio::test]
    async fn a_silent_handshake_closes_without_a_failure_frame() {
        let (_directory, address, _connector) = server(deadlines()).await;
        let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
        let mut bytes = vec![];
        let closed = tokio::time::timeout(Duration::from_secs(5), socket.read_to_end(&mut bytes))
            .await
            .is_ok();
        assert!(closed, "the handshake deadline must close the connection");
        assert!(bytes.is_empty(), "a closed handshake sends no frame");
    }
    #[tokio::test]
    async fn an_unterminated_frame_closes_without_a_failure_frame() {
        let (_directory, address, connector) = server(deadlines()).await;
        let socket = tokio::net::TcpStream::connect(address).await.unwrap();
        let mut tls = connector
            .connect(ServerName::try_from("localhost").unwrap(), socket)
            .await
            .unwrap();
        // A frame that never ends in a newline: only the deadline ends it.
        tls.write_all(br#"{"version":"gaze-lens-source/2""#)
            .await
            .unwrap();
        let mut bytes = vec![];
        let closed = tokio::time::timeout(Duration::from_secs(5), tls.read_to_end(&mut bytes))
            .await
            .is_ok();
        assert!(closed, "the frame deadline must close the connection");
        assert!(bytes.is_empty(), "a timed-out frame sends no failure");
    }

    #[test]
    fn a_rejected_peer_never_classifies_as_a_dead_listener() {
        for kind in [
            ErrorKind::ConnectionAborted,
            ErrorKind::ConnectionReset,
            ErrorKind::Interrupted,
        ] {
            assert_eq!(classify(&Error::from(kind)), Accept::Peer);
        }
        // EMFILE and ENFILE are uncategorized; they must pause, not exit.
        assert_eq!(
            classify(&Error::from_raw_os_error(24)),
            Accept::Resource,
            "EMFILE"
        );
        assert_eq!(
            classify(&Error::from_raw_os_error(23)),
            Accept::Resource,
            "ENFILE"
        );
        assert_eq!(
            classify(&Error::from(ErrorKind::InvalidInput)),
            Accept::Dead
        );
    }
}
