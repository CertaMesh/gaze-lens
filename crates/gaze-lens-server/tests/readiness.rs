use gaze_lens_protocol::wire::{self, Args, Call, Empty, Operation, Prepare, Privacy, Version};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_rustls::{
    TlsConnector,
    rustls::{self, pki_types::ServerName},
};

async fn exchange(scenario: &str) {
    let fixture = tempfile::tempdir().unwrap();
    let certified = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    let cert = fixture.path().join("cert.pem");
    let key = fixture.path().join("key.pem");
    let state = fixture.path().join("authority.json");
    let config = fixture.path().join("server.toml");
    let history = fixture.path().join("history.json");
    std::fs::write(&history, br#"{"version":1,"entries":[]}"#).unwrap();
    std::fs::write(&cert, pem("CERTIFICATE", certified.cert.der())).unwrap();
    std::fs::write(
        &key,
        pem("PRIVATE KEY", &certified.signing_key.serialize_der()),
    )
    .unwrap();
    std::fs::write(&state, format!(r#"{{"principals":[{{"id":"{}","generation":"{}","sha256":"ffe054fe7ae0cb6dc65c3af9b61d5209f439851db43d0ba5997337df154668eb"}}],"resources":[{{"alias":"fixture","id":"{}","generation":"{}","class":"database"}}],"grants":[{{"principal":"{}","resource":"{}","operation":"readiness","enabled":true,"expires-unix":4102444800}}]}}"#, "1".repeat(32), "2".repeat(32), "3".repeat(32), "4".repeat(32), "1".repeat(32), "3".repeat(32))).unwrap();
    std::fs::write(
        &config,
        format!(
            "listen = '127.0.0.1:0'\ncertificate = '{}'\nprivate-key = '{}'\nauthority = '{}'\nhistory = '{}'\n",
            cert.display(),
            key.display(),
            state.display(),
            history.display()
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(fixture.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        for file in [&cert, &key, &state, &config, &history] {
            std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
    }
    if scenario == "unsupported_operation" {
        let content = std::fs::read_to_string(&state).unwrap();
        std::fs::write(&state, content.replace("readiness", "query")).unwrap();
    }
    let checked = gaze_lens_server::config::check(&config).await.unwrap();
    assert_eq!(
        std::fs::read(&history).unwrap(),
        br#"{"version":1,"entries":[]}"#
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(gaze_lens_server::service::serve(listener, checked));
    let mut roots = rustls::RootCertStore::empty();
    roots.add(certified.cert.der().clone()).unwrap();
    let client = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_root_certificates(roots)
    .with_no_client_auth();
    if scenario == "aborted_peer" {
        // Peers that vanish before and during TLS must not end the accept loop.
        // The errno classification itself is a unit test; a real ECONNABORTED
        // from accept(2) cannot be forced portably from a client fixture.
        drop(tokio::net::TcpStream::connect(address).await.unwrap());
        let mut junk = tokio::net::TcpStream::connect(address).await.unwrap();
        junk.write_all(b"not-tls").await.unwrap();
        drop(junk);
    }
    let socket = tokio::net::TcpStream::connect(address).await.unwrap();
    let connector = TlsConnector::from(Arc::new(client));
    if scenario == "wrong_cert" {
        assert!(
            connector
                .connect(ServerName::try_from("wrong.invalid").unwrap(), socket)
                .await
                .is_err()
        );
        server.abort();
        let _ = server.await;
        return;
    }
    let mut tls = connector
        .connect(ServerName::try_from("localhost").unwrap(), socket)
        .await
        .unwrap();
    let mut p = Prepare {
        version: Version::V2,
        privacy: Privacy::ClientGaze,
        id: "01ARZ3NDEKTSV4RRFFQ69G5FAV".into(),
        credential: "a".repeat(64),
        resource: "fixture".into(),
        operation: Operation::Readiness,
    };
    if scenario == "wrong_credential" {
        p.credential = "b".repeat(64);
    }
    if scenario == "unsupported_operation" {
        p.operation = Operation::Query;
    }
    if scenario == "call_first" {
        tls.write_all(b"{\"version\":\"gaze-lens-source/2\",\"args\":{}}\n")
            .await
            .unwrap();
    } else {
        tls.write_all(&wire::encode_prepare(&p).unwrap())
            .await
            .unwrap();
    }
    if scenario == "wrong_credential" || scenario == "call_first" {
        let mut bytes = vec![];
        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            tls.read_to_end(&mut bytes),
        )
        .await
        .unwrap();
        assert!(bytes.is_empty());
        server.abort();
        let _ = server.await;
        return;
    }
    let mut bytes = vec![];
    loop {
        let b = tls.read_u8().await.unwrap();
        bytes.push(b);
        if b == b'\n' {
            break;
        }
    }
    if scenario == "unsupported_operation" {
        assert_eq!(
            wire::decode_failure(&bytes, &p.id).unwrap().code,
            gaze_lens_protocol::Error::UnsupportedOperation
        );
        server.abort();
        let _ = server.await;
        return;
    }
    let prepared = wire::decode_prepared(&bytes).unwrap();
    let mut held = None;
    if scenario == "principal_limit" {
        let socket = tokio::net::TcpStream::connect(address).await.unwrap();
        let mut second = connector
            .connect(ServerName::try_from("localhost").unwrap(), socket)
            .await
            .unwrap();
        second
            .write_all(&wire::encode_prepare(&p).unwrap())
            .await
            .unwrap();
        let mut bytes = vec![];
        loop {
            let b = second.read_u8().await.unwrap();
            bytes.push(b);
            if b == b'\n' {
                break;
            }
        }
        assert!(wire::decode_prepared(&bytes).is_ok());
        let socket = tokio::net::TcpStream::connect(address).await.unwrap();
        let mut third = connector
            .connect(ServerName::try_from("localhost").unwrap(), socket)
            .await
            .unwrap();
        third
            .write_all(&wire::encode_prepare(&p).unwrap())
            .await
            .unwrap();
        let mut bytes = vec![];
        loop {
            let b = third.read_u8().await.unwrap();
            bytes.push(b);
            if b == b'\n' {
                break;
            }
        }
        assert_eq!(
            wire::decode_failure(&bytes, &p.id).unwrap().code,
            gaze_lens_protocol::Error::Unavailable
        );
        held = Some(second);
    }
    let mut call = Call {
        id: p.id,
        binding: prepared.binding,
        args: Args::Readiness(Empty {}),
    };
    match scenario {
        "changed_call" => call.binding.resource_generation = "5".repeat(32),
        // A Call may not switch to an operation the Prepare never authorized.
        "changed_operation" => call.args = Args::ListTables(Empty {}),
        "revoke" => {
            let bytes = std::fs::read_to_string(&state).unwrap();
            std::fs::write(&state, bytes.replace("true", "false")).unwrap();
        }
        "rebind" => {
            let bytes = std::fs::read_to_string(&state).unwrap();
            std::fs::write(&state, bytes.replace(&"4".repeat(32), &"5".repeat(32))).unwrap();
        }
        "malformed_authority" => {
            std::fs::write(&state, "{broken").unwrap();
        }
        _ => {}
    }
    tls.write_all(&wire::encode_call(&call).unwrap())
        .await
        .unwrap();
    let mut response = vec![];
    tls.read_to_end(&mut response).await.unwrap();
    match scenario {
        "changed_call" | "rebind" => assert_eq!(
            wire::decode_failure(&response, &call.id).unwrap().code,
            gaze_lens_protocol::Error::BindingChanged
        ),
        "revoke" | "malformed_authority" => assert_eq!(
            wire::decode_failure(&response, &call.id).unwrap().code,
            gaze_lens_protocol::Error::Unauthorized
        ),
        "changed_operation" => assert_eq!(
            wire::decode_failure(&response, &call.id).unwrap().code,
            gaze_lens_protocol::Error::InvalidRequest
        ),
        _ => assert!(wire::decode_success(&response, &call).is_ok()),
    }
    drop(held);
    server.abort();
    let _ = server.await;
    if scenario == "restart" {
        let checked = gaze_lens_server::config::check(&config).await.unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(gaze_lens_server::service::serve(listener, checked));
        let socket = tokio::net::TcpStream::connect(address).await.unwrap();
        let mut tls = connector
            .connect(ServerName::try_from("localhost").unwrap(), socket)
            .await
            .unwrap();
        let p = Prepare {
            version: Version::V2,
            privacy: Privacy::ClientGaze,
            id: call.id.clone(),
            credential: "a".repeat(64),
            resource: "fixture".into(),
            operation: Operation::Readiness,
        };
        tls.write_all(&wire::encode_prepare(&p).unwrap())
            .await
            .unwrap();
        let mut bytes = vec![];
        loop {
            let b = tls.read_u8().await.unwrap();
            bytes.push(b);
            if b == b'\n' {
                break;
            }
        }
        let prepared = wire::decode_prepared(&bytes).unwrap();
        assert!(prepared.binding == call.binding);
        tls.write_all(&wire::encode_call(&call).unwrap())
            .await
            .unwrap();
        let mut bytes = vec![];
        tls.read_to_end(&mut bytes).await.unwrap();
        assert!(wire::decode_success(&bytes, &call).is_ok());
        server.abort();
        let _ = server.await;
    }
}

fn pem(kind: &str, der: &[u8]) -> String {
    use base64::Engine;
    format!(
        "-----BEGIN {kind}-----\n{}\n-----END {kind}-----\n",
        base64::engine::general_purpose::STANDARD.encode(der)
    )
}

#[tokio::test]
async fn tls_prepare_then_bound_readiness_without_source_access() {
    exchange("success").await;
}
#[tokio::test]
async fn altered_call_binding_releases_no_success() {
    exchange("changed_call").await;
}
#[tokio::test]
async fn revoked_grant_after_prepared_releases_no_success() {
    exchange("revoke").await;
}
#[tokio::test]
async fn rebound_resource_after_prepared_releases_no_success() {
    exchange("rebind").await;
}
#[tokio::test]
async fn malformed_authority_after_prepared_releases_no_success() {
    exchange("malformed_authority").await;
}

#[tokio::test]
async fn wrong_credential_receives_no_binding() {
    exchange("wrong_credential").await;
}
#[tokio::test]
async fn call_before_prepare_receives_no_binding() {
    exchange("call_first").await;
}
#[tokio::test]
async fn unimplemented_source_operation_never_requests_values() {
    exchange("unsupported_operation").await;
}

#[tokio::test]
async fn unchanged_tls_restart_preserves_the_enrolled_binding() {
    exchange("restart").await;
}
#[tokio::test]
async fn wrong_certificate_identity_cannot_establish_tls() {
    exchange("wrong_cert").await;
}

#[tokio::test]
async fn third_principal_call_fails_without_waiting_or_prepared() {
    exchange("principal_limit").await;
}
#[tokio::test]
async fn a_disconnected_peer_does_not_end_the_accept_loop() {
    exchange("aborted_peer").await;
}
#[tokio::test]
async fn a_call_for_another_operation_than_the_pin_is_refused() {
    exchange("changed_operation").await;
}
