//! Raw service entry. This module never constructs a Session or McpFrontend.
use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rustls::pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tokio_rustls::{TlsAcceptor, rustls};

use super::{Result, bounded_file, failure, wire};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceConfig {
    pub listen: std::net::SocketAddr,
    pub certificate: PathBuf,
    pub private_key: PathBuf,
    pub grants: PathBuf,
    pub resources: HashMap<String, PathBuf>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Grants {
    grants: Vec<Grant>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Grant {
    sha256: String,
    enabled: bool,
    expires_unix: u64,
    operation: String,
    resource: String,
}

fn authorize(path: &Path, token: &str, resource: &str) -> Result<()> {
    let bytes = bounded_file(path, 64 * 1024)?;
    let grants: Grants = serde_json::from_value(wire::parse(&bytes)?).map_err(|_| failure())?;
    if grants.grants.len() > 128 {
        return Err(failure());
    }
    let digest = Sha256::digest(token.as_bytes());
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| failure())?
        .as_secs();
    let mut allowed = false;
    let mut seen = HashSet::new();
    for grant in grants.grants {
        if !wire::valid_token(&grant.sha256)
            || !wire::valid_name(&grant.resource)
            || grant.operation != "log_tail"
        {
            return Err(failure());
        }
        let mut expected = [0u8; 32];
        for (i, byte) in expected.iter_mut().enumerate() {
            *byte =
                u8::from_str_radix(&grant.sha256[i * 2..i * 2 + 2], 16).map_err(|_| failure())?;
        }
        if !seen.insert(expected) {
            return Err(failure());
        }
        let matches = bool::from(digest.as_slice().ct_eq(&expected));
        allowed |=
            matches && grant.enabled && grant.expires_unix > now && grant.resource == resource;
    }
    if allowed { Ok(()) } else { Err(failure()) }
}

fn tail(path: &Path, lines: usize) -> Result<wire::LogBody> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| failure())?;
    let meta = file.metadata().map_err(|_| failure())?;
    if !meta.is_file() {
        return Err(failure());
    }
    let start = meta.len().saturating_sub(wire::SCAN_BYTES as u64);
    file.seek(SeekFrom::Start(start)).map_err(|_| failure())?;
    let mut bytes = Vec::new();
    file.take(meta.len() - start)
        .read_to_end(&mut bytes)
        .map_err(|_| failure())?;
    finish_tail(bytes, start, lines)
}

fn finish_tail(mut bytes: Vec<u8>, start: u64, lines: usize) -> Result<wire::LogBody> {
    let mut truncated = Vec::new();
    if start > 0 {
        truncated.push(wire::Truncation::Bytes);
        let boundary = bytes
            .iter()
            .position(|b| *b == b'\n')
            .map_or(bytes.len(), |i| i + 1);
        bytes.drain(..boundary);
    }
    let end = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
    if end < bytes.len() {
        if !truncated.contains(&wire::Truncation::Bytes) {
            truncated.push(wire::Truncation::Bytes);
        }
        bytes.truncate(end);
    }
    // Concurrent appends beyond the stat boundary are excluded by the read cap.
    // A short read after rotation/truncation is safe only at a newline boundary.
    let text = std::str::from_utf8(&bytes).map_err(|_| failure())?;
    let mut output = Vec::new();
    let mut total = 0;
    for line in text.lines().rev() {
        if output.len() == lines {
            if !truncated.contains(&wire::Truncation::Lines) {
                truncated.push(wire::Truncation::Lines);
            }
            break;
        }
        if line.len() > wire::LINE_BYTES {
            if !truncated.contains(&wire::Truncation::LineBytes) {
                truncated.push(wire::Truncation::LineBytes);
            }
            continue;
        }
        if line.contains(['\r', '\0']) {
            return Err(failure());
        }
        if total + line.len() + 1 > wire::RESULT_BYTES {
            if !truncated.contains(&wire::Truncation::Bytes) {
                truncated.push(wire::Truncation::Bytes);
            }
            break;
        }
        total += line.len() + 1;
        output.push(line.to_owned());
    }
    output.reverse();
    Ok(wire::LogBody {
        lines: output,
        truncated,
    })
}

pub async fn run(path: &Path) -> Result<()> {
    super::watchdog::fixed_panic_hook();
    tracing::subscriber::set_global_default(tracing::subscriber::NoSubscriber::default())
        .map_err(|_| failure())?;
    let config: ServiceConfig = toml::from_str(
        std::str::from_utf8(&bounded_file(path, 64 * 1024)?).map_err(|_| failure())?,
    )
    .map_err(|_| failure())?;
    if config.resources.is_empty()
        || config.resources.len() > 128
        || config.resources.keys().any(|k| !wire::valid_name(k))
    {
        return Err(failure());
    }
    let certs = CertificateDer::pem_slice_iter(&bounded_file(&config.certificate, 64 * 1024)?)
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|_| failure())?;
    let key = PrivateKeyDer::from_pem_slice(&bounded_file(&config.private_key, 64 * 1024)?)
        .map_err(|_| failure())?;
    let tls = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|_| failure())?
    .with_no_client_auth()
    .with_single_cert(certs, key)
    .map_err(|_| failure())?;
    // Keep rustls's default NoKeyLog. Never consult SSLKEYLOGFILE.
    let acceptor = TlsAcceptor::from(Arc::new(tls));
    let listener = tokio::net::TcpListener::bind(config.listen)
        .await
        .map_err(|_| failure())?;
    serve(listener, acceptor, Arc::new(config)).await
}
async fn serve(
    listener: tokio::net::TcpListener,
    acceptor: TlsAcceptor,
    config: Arc<ServiceConfig>,
) -> Result<()> {
    let permits = Arc::new(Semaphore::new(8));
    let mut tasks = JoinSet::new();
    loop {
        // Reap completed tasks before admitting more work under sustained load.
        while tasks.try_join_next().is_some() {}
        tokio::select! {
            result=listener.accept()=> {
                let (socket,_)=result.map_err(|_|failure())?;
                let Ok(permit)=Arc::clone(&permits).try_acquire_owned() else { drop(socket); continue; };
                let acceptor=acceptor.clone(); let config=Arc::clone(&config);
                tasks.spawn(async move {
                    let _=tokio::time::timeout(Duration::from_secs(30),connection(socket,acceptor,config,permit)).await;
                });
            },
            _=tasks.join_next(), if !tasks.is_empty()=> {}
        }
    }
}

async fn connection(
    socket: tokio::net::TcpStream,
    acceptor: TlsAcceptor,
    config: Arc<ServiceConfig>,
    permit: tokio::sync::OwnedSemaphorePermit,
) -> Result<()> {
    let stream = tokio::time::timeout(Duration::from_secs(5), acceptor.accept(socket))
        .await
        .map_err(|_| failure())?
        .map_err(|_| failure())?;
    let mut io = BufReader::new(stream);
    wire::expect(&mut io, wire::initialize()).await?;
    wire::write(io.get_mut(), &wire::ready()).await?;
    wire::expect(&mut io, wire::initialized()).await?;
    wire::expect(&mut io, wire::list()).await?;
    wire::write(io.get_mut(), &wire::tools()).await?;
    let call = wire::decode_call(wire::read(&mut io).await?)?;
    // Require the client's authenticated half-close. Extra requests cannot hide
    // behind an otherwise valid call, and requests never share this connection.
    if !io.fill_buf().await.map_err(|_| failure())?.is_empty() {
        return Err(failure());
    }
    let (body, _permit) = tokio::task::spawn_blocking(move || {
        let token = zeroize::Zeroizing::new(call.params.meta.token);
        let body = authorized_tail(&config, &token, &call.params.arguments, tail)?;
        // Keep admission in the actual worker if the outer task is cancelled.
        Ok::<_, crate::errors::LensError>((body, permit))
    })
    .await
    .map_err(|_| failure())??;
    wire::write(io.get_mut(), &wire::response(body)).await?;
    io.get_mut().shutdown().await.map_err(|_| failure())
}

fn authorized_tail(
    config: &ServiceConfig,
    token: &str,
    args: &wire::Arguments,
    read: impl FnOnce(&Path, usize) -> Result<wire::LogBody>,
) -> Result<wire::LogBody> {
    authorize(&config.grants, token, &args.resource)?;
    let path = config.resources.get(&args.resource).ok_or_else(failure)?;
    let body = read(path, args.lines)?;
    authorize(&config.grants, token, &args.resource)?;
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    #[test]
    fn tail_only_returns_complete_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log");
        std::fs::write(&path, b"complete@example.test\npartial@example").unwrap();
        let result = tail(&path, 10).unwrap();
        assert_eq!(result.lines, ["complete@example.test"]);
        assert!(result.truncated.contains(&wire::Truncation::Bytes));
        std::fs::write(&path, vec![b'x'; wire::SCAN_BYTES * 3]).unwrap();
        let result = tail(&path, 10).unwrap();
        assert!(result.lines.is_empty());
        assert!(result.truncated.contains(&wire::Truncation::Bytes));
        std::fs::write(
            &path,
            format!("{}\nsafe\n", "x".repeat(wire::LINE_BYTES + 1)),
        )
        .unwrap();
        let result = tail(&path, 10).unwrap();
        assert_eq!(result.lines, ["safe"]);
        assert!(result.truncated.contains(&wire::Truncation::LineBytes));
    }
    #[test]
    fn scan_boundary_and_utf8_are_never_partial() {
        let result = finish_tail(b"fragment\ncomplete\npartial\xff".to_vec(), 20, 10).unwrap();
        assert_eq!(result.lines, ["complete"]);
        assert!(finish_tail(b"invalid\xff\n".to_vec(), 0, 10).is_err());
        let result = finish_tail(b"\xa9partial\nsafe\n".to_vec(), 1, 10).unwrap();
        assert_eq!(result.lines, ["safe"]);
    }
    #[test]
    fn read_bound_excludes_later_appends() {
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(b"first\npartial").unwrap();
        let len = file.metadata().unwrap().len();
        file.write_all(b"@example.test\nappended\n").unwrap();
        file.rewind().unwrap();
        let mut bytes = Vec::new();
        file.take(len).read_to_end(&mut bytes).unwrap();
        let result = finish_tail(bytes, 0, 10).unwrap();
        assert_eq!(result.lines, ["first"]);
    }
    #[test]
    fn symlinks_and_fifo_fail_without_reading() {
        use std::os::unix::ffi::OsStrExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fifo");
        let c = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        // SAFETY: c is a live NUL-terminated test fixture path.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        assert!(tail(&path, 1).is_err());
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(tail(&link, 1).is_err());
    }
    fn grants(path: &Path, token: &str, enabled: bool, expires: u64, resource: &str) {
        std::fs::write(path,serde_json::to_vec(&serde_json::json!({"grants":[{"sha256":format!("{:x}",Sha256::digest(token.as_bytes())),"enabled":enabled,"expires_unix":expires,"operation":"log_tail","resource":resource}]})).unwrap()).unwrap();
    }
    #[test]
    fn revocation_during_read_discards_output_and_invalid_auth_never_reads() {
        let dir = tempfile::tempdir().unwrap();
        let grant_path = dir.path().join("grants");
        let token = "a".repeat(64);
        let config = ServiceConfig {
            listen: "127.0.0.1:1".parse().unwrap(),
            certificate: PathBuf::new(),
            private_key: PathBuf::new(),
            grants: grant_path.clone(),
            resources: HashMap::from([("app".into(), dir.path().join("unused"))]),
        };
        let args = wire::Arguments {
            resource: "app".into(),
            lines: 1,
        };
        grants(&grant_path, &token, true, u64::MAX, "app");
        let result = authorized_tail(&config, &token, &args, |_, _| {
            grants(&grant_path, &token, false, u64::MAX, "app");
            Ok(wire::LogBody {
                lines: vec!["CANARY@example.test".into()],
                truncated: vec![],
            })
        });
        assert!(result.is_err());
        assert!(
            authorized_tail(&config, &token, &args, |_, _| panic!("unauthorized read")).is_err()
        );
    }
    #[test]
    fn equivalent_mixed_case_grant_digests_reject() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("grants");
        let token = "a".repeat(64);
        grants(&path, &token, true, u64::MAX, "app");
        let mut file: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let mut duplicate = file["grants"][0].clone();
        duplicate["sha256"] =
            serde_json::json!(duplicate["sha256"].as_str().unwrap().to_uppercase());
        duplicate["enabled"] = serde_json::json!(false);
        file["grants"].as_array_mut().unwrap().push(duplicate);
        std::fs::write(&path, serde_json::to_vec(&file).unwrap()).unwrap();
        assert!(authorize(&path, &token, "app").is_err());
    }
    #[test]
    fn grants_reload_and_fail_closed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("grants");
        let token = "a".repeat(64);
        grants(&path, &token, true, u64::MAX, "app");
        assert!(authorize(&path, &token, "app").is_ok());
        assert!(authorize(&path, &"b".repeat(64), "app").is_err());
        assert!(authorize(&path, &token, "other").is_err());
        grants(&path, &token, false, u64::MAX, "app");
        assert!(authorize(&path, &token, "app").is_err());
        grants(&path, &token, true, 0, "app");
        assert!(authorize(&path, &token, "app").is_err());
        std::fs::write(&path, b"{\"grants\":[],\"grants\":[]}").unwrap();
        assert!(authorize(&path, &token, "app").is_err());
        std::fs::remove_file(&path).unwrap();
        assert!(authorize(&path, &token, "app").is_err());
    }
}
