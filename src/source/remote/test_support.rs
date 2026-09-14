//! Test-only real TLS peer for exercising local privacy failures.
use super::*;
use tokio_rustls::TlsAcceptor;

pub(crate) async fn peer(
    error: bool,
) -> (
    RemoteMcpSource,
    tokio::task::JoinHandle<()>,
    tempfile::TempDir,
) {
    use base64::Engine;
    let dir = tempfile::tempdir().unwrap();
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    let root = dir.path().join("root.pem");
    std::fs::write(
        &root,
        format!(
            "-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----\n",
            base64::engine::general_purpose::STANDARD.encode(cert.cert.der())
        ),
    )
    .unwrap();
    let tls = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(
        vec![cert.cert.der().clone()],
        rustls::pki_types::PrivatePkcs8KeyDer::from(cert.signing_key.serialize_der()).into(),
    )
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = listener.local_addr().unwrap().to_string();
    let task = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let stream = TlsAcceptor::from(Arc::new(tls)).accept(tcp).await.unwrap();
        let mut io = BufReader::new(stream);
        wire::expect(&mut io, wire::initialize()).await.unwrap();
        wire::write(io.get_mut(), &wire::ready()).await.unwrap();
        wire::expect(&mut io, wire::initialized()).await.unwrap();
        wire::expect(&mut io, wire::list()).await.unwrap();
        wire::write(io.get_mut(), &wire::tools()).await.unwrap();
        let _ = wire::read(&mut io).await.unwrap();
        assert!(io.fill_buf().await.unwrap().is_empty());
        let response = if error {
            serde_json::json!({"jsonrpc":"2.0","id":3,"error":{"code":-1,"message":"RAW_ERROR_CANARY secret@example.test"}})
        } else {
            wire::response(wire::LogBody {
                lines: vec!["RAW_TEXT_CANARY secret@example.test".into()],
                truncated: vec![],
            })
        };
        wire::write(io.get_mut(), &response).await.unwrap();
        io.get_mut().shutdown().await.unwrap();
    });
    let var = format!("GAZE_PRIVACY_{}", ulid::Ulid::new());
    // SAFETY: unique fixture variable, not overwritten by another test.
    unsafe { std::env::set_var(&var, "d".repeat(64)) };
    let source = RemoteMcpSource::new(RemoteConfig {
        endpoint,
        server_name: "localhost".into(),
        trust_root: root,
        resource: "app".into(),
        secret: SecretSpec::Env { var },
    })
    .unwrap();
    (source, task, dir)
}
