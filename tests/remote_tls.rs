//! Hermetic encrypted transport and actual stdio privacy boundary.
use gaze_lens::profile::SecretSpec;
use gaze_lens::source::remote::{RemoteConfig, RemoteMcpSource};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

struct Fixture {
    dir: tempfile::TempDir,
    endpoint: String,
    config: RemoteConfig,
    service: tokio::process::Child,
    token: String,
    keylog: PathBuf,
}
impl Fixture {
    async fn start() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
        std::fs::write(
            root.join("cert.pem"),
            pem("CERTIFICATE", cert.cert.der().as_ref()),
        )
        .unwrap();
        std::fs::write(
            root.join("key.pem"),
            pem("PRIVATE KEY", &cert.signing_key.serialize_der()),
        )
        .unwrap();
        std::fs::write(
            root.join("app.log"),
            "ERROR customer bob@example.com\nINFO complete\nunfinished alice@example",
        )
        .unwrap();
        let token = "c".repeat(64);
        write_grants(root, &token, true);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = listener.local_addr().unwrap().to_string();
        drop(listener);
        let service_config = format!(
            "listen = {endpoint:?}\ncertificate = {:?}\nprivate_key = {:?}\ngrants = {:?}\n[resources]\napp = {:?}\n",
            root.join("cert.pem"),
            root.join("key.pem"),
            root.join("grants.json"),
            root.join("app.log")
        );
        std::fs::write(root.join("service.toml"), service_config).unwrap();
        let keylog = root.join("tls-secrets");
        let mut service = tokio::process::Command::new(env!("CARGO_BIN_EXE_gaze-lens"))
            .args(["serve", "--remote-service-config"])
            .arg(root.join("service.toml"))
            .env("HOME", root)
            .env_remove("GAZE_LENS_PROJECT_CONFIG")
            .env_remove("GAZE_LENS_USER_CONFIG")
            .env("RUST_LOG", "trace")
            .env("GAZE_LENS_VERBOSE_ERRORS", "1")
            .env("SSLKEYLOGFILE", &keylog)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let var = format!("GAZE_REMOTE_TEST_{}", ulid::Ulid::new());
        // SAFETY: unique fixture variable, never modified after publication.
        unsafe { std::env::set_var(&var, &token) };
        let config = RemoteConfig {
            endpoint: endpoint.clone(),
            server_name: "localhost".into(),
            trust_root: root.join("cert.pem"),
            resource: "app".into(),
            secret: SecretSpec::Env { var },
        };
        let started = std::time::Instant::now();
        while tokio::net::TcpStream::connect(&endpoint).await.is_err() {
            if let Some(status) = service.try_wait().unwrap() {
                use tokio::io::AsyncReadExt;
                let mut stderr = String::new();
                service
                    .stderr
                    .take()
                    .unwrap()
                    .read_to_string(&mut stderr)
                    .await
                    .unwrap();
                panic!("service exited {status}: {stderr}");
            }
            if started.elapsed() >= Duration::from_secs(10) {
                service.kill().await.unwrap();
                let output = service.wait_with_output().await.unwrap();
                panic!(
                    "service startup hung: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        Self {
            dir,
            endpoint,
            config,
            service,
            token,
            keylog,
        }
    }
    async fn stop(mut self) {
        self.service.kill().await.unwrap();
        let result = self.service.wait_with_output().await.unwrap();
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(!stderr.contains(&self.token));
        assert!(!stderr.contains("bob@example.com"));
        assert!(result.stdout.is_empty());
        assert!(!self.keylog.exists());
        assert!(!self.dir.path().join(".gaze-lens/manifest.sqlite").exists());
    }
}
fn write_grants(root: &Path, token: &str, enabled: bool) {
    std::fs::write(root.join("grants.json"),serde_json::to_vec(&json!({"grants":[{"sha256":format!("{:x}",Sha256::digest(token.as_bytes())),"enabled":enabled,"expires_unix":u64::MAX,"operation":"log_tail","resource":"app"}]})).unwrap()).unwrap();
}
#[tokio::test]
async fn encrypted_service_authentication_trust_and_complete_lines() {
    let fixture = Fixture::start().await;
    let source = RemoteMcpSource::new(fixture.config.clone()).unwrap();
    let result = source.tail(10).await.unwrap();
    let gaze_lens::source::SourceOutput::TextWithTruncation { text, truncated_at } = result else {
        panic!("text source")
    };
    assert_eq!(text, "ERROR customer bob@example.com\nINFO complete");
    assert!(!truncated_at.is_empty());
    let mut wrong = fixture.config.clone();
    wrong.server_name = "wrong.example".into();
    assert!(RemoteMcpSource::new(wrong).unwrap().tail(1).await.is_err());
    let other = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    std::fs::write(
        fixture.dir.path().join("other.pem"),
        pem("CERTIFICATE", other.cert.der().as_ref()),
    )
    .unwrap();
    let mut wrong = fixture.config.clone();
    wrong.trust_root = fixture.dir.path().join("other.pem");
    assert!(RemoteMcpSource::new(wrong).unwrap().tail(1).await.is_err());
    let mut held = Vec::new();
    for _ in 0..8 {
        held.push(
            tokio::net::TcpStream::connect(&fixture.endpoint)
                .await
                .unwrap(),
        );
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
    let mut overflow = tokio::net::TcpStream::connect(&fixture.endpoint)
        .await
        .unwrap();
    use tokio::io::AsyncReadExt;
    let mut byte = [0u8; 1];
    let closed = tokio::time::timeout(Duration::from_secs(2), overflow.read(&mut byte))
        .await
        .unwrap();
    assert!(
        matches!(closed, Ok(0) | Err(_)),
        "admission overflow must close immediately"
    );
    drop(held);
    drop(overflow);
    tokio::time::sleep(Duration::from_millis(50)).await;
    write_grants(fixture.dir.path(), &fixture.token, false);
    assert!(source.tail(1).await.is_err());
    write_grants(fixture.dir.path(), &fixture.token, true);
    assert!(source.tail(1).await.is_ok());
    let mut wrong = fixture.config.clone();
    wrong.resource = "ungranted".into();
    assert!(RemoteMcpSource::new(wrong).unwrap().tail(1).await.is_err());
    fixture.stop().await;
}
async fn send(stdin: &mut tokio::process::ChildStdin, value: Value) {
    let mut bytes = serde_json::to_vec(&value).unwrap();
    bytes.push(b'\n');
    stdin.write_all(&bytes).await.unwrap();
    stdin.flush().await.unwrap();
}
async fn receive(stdout: &mut BufReader<tokio::process::ChildStdout>) -> Value {
    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(15), stdout.read_line(&mut line))
        .await
        .unwrap()
        .unwrap();
    serde_json::from_str(&line).unwrap()
}
#[tokio::test]
async fn actual_stdio_proxy_redacts_audits_and_replays() {
    let fixture = Fixture::start().await;
    let root = fixture.dir.path();
    std::fs::write(root.join("empty.toml"), "").unwrap();
    std::fs::write(
        root.join("policy.toml"),
        "[policy]\ndefault_action = 'tokenize'\n[policy.database]\n",
    )
    .unwrap();
    let config = format!(
        "[[profiles]]\nname = 'remote'\npolicy = {:?}\n[profiles.source]\nkind = 'remote_mcp_log'\nendpoint = {:?}\nserver_name = 'localhost'\ntrust_root = {:?}\nresource = 'app'\n[profiles.source.secret]\ntype = 'env'\nvar = 'REMOTE_CREDENTIAL'\n",
        root.join("policy.toml"),
        fixture.endpoint,
        fixture.config.trust_root
    );
    std::fs::write(root.join("profiles.toml"), config).unwrap();
    let manifest = root.join("manifest.sqlite");
    let snapshots = root.join("snapshots");
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_gaze-lens"))
        .arg("--project-config")
        .arg(root.join("profiles.toml"))
        .arg("--user-config")
        .arg(root.join("empty.toml"))
        .args(["serve", "--profile", "remote", "--manifest"])
        .arg(&manifest)
        .arg("--snapshot-dir")
        .arg(&snapshots)
        .args(["--log", "trace"])
        .env("REMOTE_CREDENTIAL", &fixture.token)
        .env("RUST_LOG", "trace")
        .env("GAZE_LENS_VERBOSE_ERRORS", "1")
        .env("SSLKEYLOGFILE", &fixture.keylog)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    send(&mut stdin,json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"fixture","version":"1"}}})).await;
    let ready = receive(&mut stdout).await;
    assert!(ready.get("result").is_some(), "{ready}");
    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
    )
    .await;
    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}),
    )
    .await;
    let inventory = receive(&mut stdout).await;
    assert_eq!(inventory["result"]["tools"].as_array().unwrap().len(), 5);
    send(&mut stdin,json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"log_tail","arguments":{"profile":"remote","lines":10}}})).await;
    let output = receive(&mut stdout).await;
    let rendered = output.to_string();
    assert!(rendered.contains("Email_1"), "{rendered}");
    assert!(!rendered.contains("bob@example.com"));
    assert!(!rendered.contains(&fixture.token));
    assert!(!rendered.contains("alice@example"));
    drop(stdin);
    child.kill().await.unwrap();
    let result = child.wait_with_output().await.unwrap();
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(!stderr.contains("bob@example.com"));
    assert!(!stderr.contains(&fixture.token));
    let connection = rusqlite::Connection::open(&manifest).unwrap();
    let (session, status): (String, String) = connection
        .query_row(
            "SELECT lens_session_id,status FROM calls LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(status, "ok");
    drop(connection);
    assert!(
        !String::from_utf8_lossy(&std::fs::read(&manifest).unwrap()).contains("bob@example.com")
    );
    let replay = tokio::process::Command::new(env!("CARGO_BIN_EXE_gaze-lens"))
        .env("HOME", root)
        .env("GAZE_LENS_PROJECT_CONFIG", root.join("empty.toml"))
        .env("GAZE_LENS_USER_CONFIG", root.join("empty.toml"))
        .args(["replay", &session, "--manifest"])
        .arg(&manifest)
        .output()
        .await
        .unwrap();
    assert!(
        replay.status.success(),
        "{}",
        String::from_utf8_lossy(&replay.stderr)
    );
    let replayed: Value = serde_json::from_slice(&replay.stdout).unwrap();
    assert_eq!(replayed["calls"].as_array().unwrap().len(), 1);
    let payload: Value =
        serde_json::from_str(output["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    let snapshot = std::fs::read(payload["snapshot_ref"]["path"].as_str().unwrap()).unwrap();
    let restored = gaze::Session::import(gaze::SensitiveSnapshot::from(snapshot))
        .unwrap()
        .restore_strict_text(payload["clean"]["Text"]["text"].as_str().unwrap())
        .unwrap();
    assert!(restored.contains("bob@example.com"));
    fixture.stop().await;
}

fn pem(label: &str, bytes: &[u8]) -> String {
    use base64::Engine;
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    let lines = encoded
        .as_bytes()
        .chunks(64)
        .map(|b| std::str::from_utf8(b).unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    format!("-----BEGIN {label}-----\n{lines}\n-----END {label}-----\n")
}

#[test]
fn unsafe_remote_policies_and_missing_production_model_reject_before_serving() {
    for (mode, policy, production) in [
        ("no_policy", "[policy.database]\n", false),
        (
            "mixed",
            "[policy]\ndefault_action='tokenize'\n[policy.database]\n",
            false,
        ),
        ("implicit_preserve", "[policy.database]\n", false),
        (
            "explicit_preserve",
            "[policy]\ndefault_action='preserve'\n[policy.database]\n",
            false,
        ),
        (
            "override_preserve",
            "[policy]\ndefault_action='tokenize'\n[[policy.database.columns]]\ncolumn='email'\nclass='email'\naction='preserve'\n",
            false,
        ),
        (
            "override_generalize",
            "[policy]\ndefault_action='tokenize'\n[[policy.database.columns]]\ncolumn='email'\nclass='email'\naction='generalize'\n",
            false,
        ),
        (
            "missing_model",
            "[ner]\nmodel_dir='/nonexistent/gaze-remote-fixture-model'\n[policy]\ndefault_action='tokenize'\n[policy.database]\n",
            true,
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("empty.toml"), "").unwrap();
        std::fs::write(root.join("policy.toml"), policy).unwrap();
        let config = format!(
            "[[profiles]]\nname='remote'\nproduction={production}\npolicy={:?}\n[profiles.source]\nkind='remote_mcp_log'\nendpoint='127.0.0.1:1'\nserver_name='localhost'\ntrust_root='absent.pem'\nresource='app'\n[profiles.source.secret]\ntype='env'\nvar='FIXTURE_CREDENTIAL'\n",
            root.join("policy.toml")
        );
        let config = if mode == "no_policy" {
            config
                .lines()
                .filter(|line| !line.starts_with("policy="))
                .collect::<Vec<_>>()
                .join("\n")
        } else if mode == "mixed" {
            format!(
                "{config}\n[[profiles]]\nname='direct'\n[profiles.source]\nkind='local_log'\npath='unused'\n"
            )
        } else {
            config
        };
        std::fs::write(root.join("profiles.toml"), config).unwrap();
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_gaze-lens"))
            .arg("--project-config")
            .arg(root.join("profiles.toml"))
            .arg("--user-config")
            .arg(root.join("empty.toml"))
            .args(["serve", "--manifest"])
            .arg(root.join("manifest.sqlite"))
            .arg("--snapshot-dir")
            .arg(root.join("snapshots"))
            .env("HOME", root)
            .env("RUST_LOG", "trace")
            .env("GAZE_LENS_VERBOSE_ERRORS", "1")
            .env("FIXTURE_CREDENTIAL", "e".repeat(64))
            .output()
            .unwrap();
        assert!(!output.status.success(), "{mode}");
        assert!(output.stdout.is_empty(), "{mode}");
        assert!(!root.join("manifest.sqlite").exists(), "{mode}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!stderr.contains(&"e".repeat(64)));
        assert!(!stderr.contains("bob@example.com"));
        if mode == "mixed" {
            assert!(stderr.contains("dedicated process"), "{stderr}");
        } else if mode != "missing_model" {
            assert!(stderr.contains("default_action"), "{mode}: {stderr}");
        } else {
            assert!(stderr.to_lowercase().contains("model"), "{stderr}");
        }
    }
}

async fn raw_service_call(
    fixture: &Fixture,
    token: Option<&str>,
    operation: &str,
    lines: usize,
) -> Vec<u8> {
    use rustls::pki_types::{CertificateDer, ServerName, pem::PemObject};
    use std::sync::Arc;
    use tokio::io::AsyncReadExt;
    use tokio_rustls::{TlsConnector, rustls};
    let mut roots = rustls::RootCertStore::empty();
    for cert in CertificateDer::pem_slice_iter(&std::fs::read(&fixture.config.trust_root).unwrap())
    {
        roots.add(cert.unwrap()).unwrap();
    }
    let tls = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_root_certificates(roots)
    .with_no_client_auth();
    let tcp = tokio::net::TcpStream::connect(&fixture.endpoint)
        .await
        .unwrap();
    let stream = TlsConnector::from(Arc::new(tls))
        .connect(ServerName::try_from("localhost").unwrap(), tcp)
        .await
        .unwrap();
    let mut io = BufReader::new(stream);
    async fn frame<S: tokio::io::AsyncWrite + Unpin>(stream: &mut S, value: Value) {
        let mut bytes = serde_json::to_vec(&value).unwrap();
        bytes.push(b'\n');
        stream.write_all(&bytes).await.unwrap();
        stream.flush().await.unwrap();
    }
    frame(io.get_mut(),json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"gaze-lens","version":"1"}}})).await;
    let mut response = String::new();
    io.read_line(&mut response).await.unwrap();
    assert!(response.contains("serverInfo"));
    frame(
        io.get_mut(),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
    )
    .await;
    frame(
        io.get_mut(),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}),
    )
    .await;
    response.clear();
    io.read_line(&mut response).await.unwrap();
    assert!(response.contains("log_tail"));
    let mut request = json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":operation,"arguments":{"resource":"app","lines":lines}}});
    if let Some(token) = token {
        request["params"]["_meta"] = json!({"gaze-lens/token":token});
    }
    frame(io.get_mut(), request).await;
    let _ = io.get_mut().shutdown().await;
    let mut result = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), io.read_to_end(&mut result))
        .await
        .unwrap();
    result
}

#[tokio::test]
async fn real_service_denies_invalid_grants_and_enforces_text_bounds() {
    let fixture = Fixture::start().await;
    for token in [
        None,
        Some("wrong"),
        Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
    ] {
        assert!(
            raw_service_call(&fixture, token, "log_tail", 1)
                .await
                .is_empty()
        );
    }
    assert!(
        raw_service_call(&fixture, Some(&fixture.token), "log_grep", 1)
            .await
            .is_empty()
    );
    assert!(
        raw_service_call(&fixture, Some(&fixture.token), "log_tail", 1001)
            .await
            .is_empty()
    );
    for (field, value) in [("expires_unix", json!(0)), ("operation", json!("log_grep"))] {
        write_grants(fixture.dir.path(), &fixture.token, true);
        let path = fixture.dir.path().join("grants.json");
        let mut grants: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        grants["grants"][0][field] = value;
        std::fs::write(path, serde_json::to_vec(&grants).unwrap()).unwrap();
        assert!(
            raw_service_call(&fixture, Some(&fixture.token), "log_tail", 1)
                .await
                .is_empty()
        );
    }
    write_grants(fixture.dir.path(), &fixture.token, true);
    let source = RemoteMcpSource::new(fixture.config.clone()).unwrap();
    let path = fixture.dir.path().join("app.log");
    for (data, requested, max_lines, max_bytes, expected) in [
        (
            "line\n".repeat(1001),
            1000,
            1000,
            128 * 1024,
            gaze_lens::session::TruncatedAt::Rows,
        ),
        (
            ("x".repeat(4000) + "\n").repeat(100),
            1000,
            100,
            128 * 1024,
            gaze_lens::session::TruncatedAt::Bytes,
        ),
        (
            "x".repeat(8193) + "\nsafe\n",
            10,
            1,
            4,
            gaze_lens::session::TruncatedAt::LineBytes,
        ),
        (
            "x".repeat(2 * 1024 * 1024),
            10,
            0,
            0,
            gaze_lens::session::TruncatedAt::Bytes,
        ),
    ] {
        std::fs::write(&path, data).unwrap();
        let gaze_lens::source::SourceOutput::TextWithTruncation { text, truncated_at } =
            source.tail(requested).await.unwrap()
        else {
            panic!("bounded text")
        };
        assert!(text.lines().count() <= max_lines);
        assert!(text.len() <= max_bytes);
        assert!(truncated_at.contains(&expected));
        assert!(text.lines().all(|line| line.len() <= 8192));
        if expected == gaze_lens::session::TruncatedAt::LineBytes {
            assert_eq!(text, "safe");
        }
    }
    fixture.stop().await;
}
