use gaze_lens_protocol::Error;
use gaze_lens_server::{auth::Authority, history::History};

fn state_text(generation: &str, digest: &str) -> String {
    format!(
        r#"{{"principals":[{{"id":"{}","generation":"{}","sha256":"{}"}}],"resources":[{{"alias":"fixture","id":"{}","generation":"{}","class":"database"}}],"grants":[]}}"#,
        "1".repeat(32),
        generation,
        digest,
        "3".repeat(32),
        "4".repeat(32)
    )
}
fn state(generation: &str, digest: &str) -> Authority {
    Authority::parse(state_text(generation, digest).as_bytes()).unwrap()
}
fn fixture() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.json");
    std::fs::write(&path, br#"{"version":1,"entries":[]}"#).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    (dir, path)
}
#[test]
fn restart_preserves_binding_but_reusing_a_generation_for_rebinding_fails() {
    let (_dir, path) = fixture();
    let a = state(&"2".repeat(32), &"a".repeat(64));
    drop(History::open(&path, &a).unwrap());
    drop(History::open(&path, &a).unwrap());
    let rebound = state(&"2".repeat(32), &"b".repeat(64));
    assert!(matches!(
        History::open(&path, &rebound),
        Err(Error::BindingChanged)
    ));
    let rotated = state(&"5".repeat(32), &"b".repeat(64));
    drop(History::open(&path, &rotated).unwrap());
    assert!(matches!(
        History::open(&path, &a),
        Err(Error::BindingChanged)
    ));
}

#[test]
fn retired_ids_cannot_be_reused_even_with_a_fresh_generation() {
    let (_dir, path) = fixture();
    let original = state(&"2".repeat(32), &"a".repeat(64));
    drop(History::open(&path, &original).unwrap());
    let empty = Authority::parse(br#"{"principals":[],"resources":[],"grants":[]}"#).unwrap();
    drop(History::open(&path, &empty).unwrap());
    let before = std::fs::read(&path).unwrap();
    assert!(matches!(
        History::open(&path, &original),
        Err(Error::BindingChanged)
    ));
    let fresh_generation = state(&"5".repeat(32), &"b".repeat(64));
    assert!(matches!(
        History::open(&path, &fresh_generation),
        Err(Error::BindingChanged)
    ));
    assert_eq!(std::fs::read(&path).unwrap(), before);
}
#[test]
fn resource_remap_requires_new_generation_and_cannot_regress() {
    let (_dir, path) = fixture();
    let text = state_text(&"2".repeat(32), &"a".repeat(64));
    let original = Authority::parse(text.as_bytes()).unwrap();
    drop(History::open(&path, &original).unwrap());
    let remapped = text
        .replace("fixture", "changed")
        .replace("database", "log");
    assert!(matches!(
        History::open(&path, &Authority::parse(remapped.as_bytes()).unwrap()),
        Err(Error::BindingChanged)
    ));
    let remapped = Authority::parse(
        remapped
            .replace(&"4".repeat(32), &"6".repeat(32))
            .as_bytes(),
    )
    .unwrap();
    drop(History::open(&path, &remapped).unwrap());
    assert!(matches!(
        History::open(&path, &original),
        Err(Error::BindingChanged)
    ));
}
#[test]
fn one_writer_and_read_only_check_preserve_the_registry() {
    let (_dir, path) = fixture();
    let original = state(&"2".repeat(32), &"a".repeat(64));
    let empty = std::fs::read(&path).unwrap();
    History::check(&path, &original).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), empty);
    let history = History::open(&path, &original).unwrap();
    assert!(matches!(
        History::open(&path, &original),
        Err(Error::Unavailable)
    ));
    assert!(history.validate(&original).is_ok());
    let rotated = state(&"5".repeat(32), &"b".repeat(64));
    assert_eq!(history.validate(&rotated), Err(Error::BindingChanged));
    let before = std::fs::read(&path).unwrap();
    History::check(&path, &rotated).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), before);
    drop(history);
    std::fs::remove_file(&path).unwrap();
    assert!(matches!(
        History::open(&path, &original),
        Err(Error::Unavailable)
    ));
    assert!(!path.exists());
}
/// 128 principals, the per-file maximum, with a generation set unique to each
/// round. Every round after the first retires 128 entries and enrolls 128 more.
fn rotation(round: usize) -> Authority {
    let principals = (0..128usize)
        .map(|i| {
            format!(
                r#"{{"id":"{:032x}","generation":"{:032x}","sha256":"{:064x}"}}"#,
                0x1000 + i,
                0x900000 + round * 0x1000 + i,
                0x2000 + i
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    Authority::parse(
        format!(r#"{{"principals":[{principals}],"resources":[],"grants":[]}}"#).as_bytes(),
    )
    .unwrap()
}
#[test]
fn the_byte_ceiling_binds_before_the_entry_count_and_writes_nothing() {
    let (_dir, path) = fixture();
    drop(History::open(&path, &rotation(0)).unwrap());
    drop(History::open(&path, &rotation(1)).unwrap());
    let before = std::fs::read(&path).unwrap();
    let entries = String::from_utf8(before.clone())
        .unwrap()
        .matches("\"active\"")
        .count();
    // 256 retained entries are far below the 1,024-entry cap, yet a third
    // rotation would not fit in 64 KiB: the byte ceiling is the real limit.
    assert_eq!(entries, 256);
    assert!(before.len() < 65_536);
    assert!(
        matches!(History::open(&path, &rotation(2)), Err(Error::CapExceeded)),
        "a third rotation must fail closed"
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
}
#[test]
fn malformed_and_oversized_history_is_never_reset() {
    let (_dir, path) = fixture();
    let original = state(&"2".repeat(32), &"a".repeat(64));
    for bytes in [b"{broken".to_vec(), vec![b' '; 65537]] {
        std::fs::write(&path, &bytes).unwrap();
        assert!(History::open(&path, &original).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
}
