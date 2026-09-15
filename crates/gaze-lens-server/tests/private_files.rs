//! Every trusted operator file - configuration, certificate, key, authority and
//! identity history - passes the same private-file checks.
#![cfg(unix)]
use gaze_lens_protocol::Error;
use gaze_lens_server::{auth::Authority, history::History};
use std::{os::unix::fs::PermissionsExt, path::PathBuf, process::Command};

struct Fixture {
    directory: tempfile::TempDir,
    config: PathBuf,
    authority: PathBuf,
    history: PathBuf,
}
fn authority_text() -> String {
    format!(
        r#"{{"principals":[{{"id":"{}","generation":"{}","sha256":"{}"}}],"resources":[{{"alias":"fixture","id":"{}","generation":"{}","class":"database"}}],"grants":[]}}"#,
        "1".repeat(32),
        "2".repeat(32),
        "a".repeat(64),
        "3".repeat(32),
        "4".repeat(32)
    )
}
fn fixture() -> Fixture {
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
    std::fs::write(&authority, authority_text()).unwrap();
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
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    for file in [&cert, &key, &authority, &history, &config] {
        std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    Fixture {
        directory,
        config,
        authority,
        history,
    }
}
fn pem(kind: &str, der: &[u8]) -> String {
    use base64::Engine;
    format!(
        "-----BEGIN {kind}-----\n{}\n-----END {kind}-----\n",
        base64::engine::general_purpose::STANDARD.encode(der)
    )
}
async fn configured(fixture: &Fixture) -> bool {
    gaze_lens_server::config::check(&fixture.config)
        .await
        .is_ok()
}
fn enrolled(fixture: &Fixture) -> bool {
    let authority = Authority::parse(authority_text().as_bytes()).unwrap();
    History::open(&fixture.history, &authority).is_ok()
}
/// Replaces a path with a symlink to an equally private file beside it.
fn relink(path: &PathBuf) {
    let target = path.with_extension("real");
    std::fs::rename(path, &target).unwrap();
    std::os::unix::fs::symlink(&target, path).unwrap();
}
fn fifo(path: &PathBuf) {
    std::fs::remove_file(path).unwrap();
    assert!(Command::new("mkfifo").arg(path).status().unwrap().success());
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}

#[tokio::test]
async fn a_group_readable_authority_or_history_is_refused() {
    let fixture = fixture();
    assert!(configured(&fixture).await && enrolled(&fixture));
    let shared = std::fs::Permissions::from_mode(0o644);
    let owner = std::fs::Permissions::from_mode(0o600);
    std::fs::set_permissions(&fixture.authority, shared.clone()).unwrap();
    assert!(!configured(&fixture).await);
    std::fs::set_permissions(&fixture.authority, owner.clone()).unwrap();
    std::fs::set_permissions(&fixture.history, shared).unwrap();
    assert!(!configured(&fixture).await);
    assert!(!enrolled(&fixture));
    std::fs::set_permissions(&fixture.history, owner).unwrap();
    assert!(configured(&fixture).await && enrolled(&fixture));
}
#[tokio::test]
async fn a_symlinked_authority_or_history_is_refused() {
    let replaced = fixture();
    relink(&replaced.authority);
    assert!(!configured(&replaced).await);
    let fixture = fixture();
    relink(&fixture.history);
    assert!(!configured(&fixture).await);
    assert!(!enrolled(&fixture));
}
#[tokio::test]
async fn a_shared_parent_directory_is_refused() {
    let fixture = fixture();
    std::fs::set_permissions(
        fixture.directory.path(),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    assert!(!configured(&fixture).await);
    assert!(!enrolled(&fixture));
    std::fs::set_permissions(
        fixture.directory.path(),
        std::fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    assert!(configured(&fixture).await && enrolled(&fixture));
}
#[tokio::test]
async fn a_fifo_authority_or_history_is_refused() {
    let replaced = fixture();
    fifo(&replaced.authority);
    assert!(!configured(&replaced).await);
    let fixture = fixture();
    fifo(&fixture.history);
    assert!(!configured(&fixture).await);
    assert!(!enrolled(&fixture));
}
#[tokio::test]
async fn the_reader_reports_an_oversized_file_as_a_cap() {
    let fixture = fixture();
    std::fs::write(&fixture.authority, vec![b' '; 65_537]).unwrap();
    std::fs::set_permissions(&fixture.authority, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(matches!(
        gaze_lens_server::config::check(&fixture.config).await,
        Err(Error::CapExceeded)
    ));
}
