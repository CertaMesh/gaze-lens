//! Local validation only. No source connections or probes.
use crate::auth::Authority;
use gaze_lens_protocol::{Error, Result};
use serde::Deserialize;
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::io::AsyncReadExt;
use tokio_rustls::{
    TlsAcceptor,
    rustls::{
        self,
        pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject},
    },
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct Config {
    listen: SocketAddr,
    certificate: PathBuf,
    private_key: PathBuf,
    authority: PathBuf,
    history: PathBuf,
}
/// Checked TLS material and the path to freshly reloaded authority.
pub struct Checked {
    pub(crate) listen: SocketAddr,
    pub(crate) authority: PathBuf,
    pub(crate) history: PathBuf,
    pub(crate) acceptor: TlsAcceptor,
}
pub async fn check(path: &Path) -> Result<Checked> {
    let bytes = bounded_file(path).await?;
    let config: Config =
        toml::from_str(std::str::from_utf8(&bytes).map_err(|_| Error::InvalidRequest)?)
            .map_err(|_| Error::InvalidRequest)?;
    let authority = Authority::parse(&bounded_file(&config.authority).await?)?;
    let history_path = config.history.clone();
    tokio::task::spawn_blocking(move || crate::history::History::check(&history_path, &authority))
        .await
        .map_err(|_| Error::InternalFailure)??;
    let cert_bytes = bounded_file(&config.certificate).await?;
    let key_bytes = bounded_file(&config.private_key).await?;
    let certs = CertificateDer::pem_slice_iter(&cert_bytes)
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|_| Error::InvalidRequest)?;
    let key = PrivateKeyDer::from_pem_slice(&key_bytes).map_err(|_| Error::InvalidRequest)?;
    let tls = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|_| Error::InvalidRequest)?
    .with_no_client_auth()
    .with_single_cert(certs, key)
    .map_err(|_| Error::InvalidRequest)?;
    Ok(Checked {
        listen: config.listen,
        authority: config.authority,
        history: config.history,
        acceptor: TlsAcceptor::from(Arc::new(tls)),
    })
}
/// Fixed-sized storage is allocated before reading, never from file metadata.
pub(crate) async fn bounded_file(path: &Path) -> Result<Vec<u8>> {
    let link = tokio::fs::symlink_metadata(path)
        .await
        .map_err(|_| Error::Unavailable)?;
    if !link.is_file() {
        return Err(Error::Unavailable);
    }
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|_| Error::Unavailable)?;
    let meta = file.metadata().await.map_err(|_| Error::Unavailable)?;
    if !meta.is_file() {
        return Err(Error::Unavailable);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.mode() & 0o077 != 0 || meta.ino() != link.ino() || meta.dev() != link.dev() {
            return Err(Error::Unavailable);
        }
    }
    #[cfg(not(unix))]
    return Err(Error::Unavailable);
    let mut bytes = vec![0u8; 65537];
    let mut n = 0;
    while n < bytes.len() {
        let got = file
            .read(&mut bytes[n..])
            .await
            .map_err(|_| Error::Unavailable)?;
        if got == 0 {
            break;
        }
        n += got;
    }
    if n > 65536 {
        return Err(Error::CapExceeded);
    }
    bytes.truncate(n);
    Ok(bytes)
}
