//! Local validation only. No source connections or probes.
use crate::auth::Authority;
use gaze_lens_protocol::{Error, Result};
use serde::Deserialize;
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio_rustls::{
    TlsAcceptor,
    rustls::{
        self,
        pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject},
    },
};
use zeroize::Zeroizing;

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
    // The PEM key text is wiped when this scope ends; the parsed key already
    // zeroizes itself. Both copies are short-lived server-local material.
    let key_bytes = Zeroizing::new(bounded_file(&config.private_key).await?);
    let certs = CertificateDer::pem_slice_iter(&cert_bytes)
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|_| Error::InvalidRequest)?;
    let key = PrivateKeyDer::from_pem_slice(&key_bytes).map_err(|_| Error::InvalidRequest)?;
    let tls = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    // TLS 1.3 only: the private protocol has no legacy peer to accommodate.
    .with_protocol_versions(&[&rustls::version::TLS13])
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
/// Configuration, certificate, key, authority and history all pass through the
/// one private-file reader. It runs on a blocking worker because its checks are
/// synchronous metadata calls, and it sizes storage from a constant.
pub(crate) async fn bounded_file(path: &Path) -> Result<Vec<u8>> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || crate::private::read(&path))
        .await
        .map_err(|_| Error::InternalFailure)?
}
