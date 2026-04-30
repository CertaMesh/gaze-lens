#[cfg(feature = "integration-keyring")]
#[test]
#[ignore = "requires an unlocked platform keyring"]
fn real_keyring_round_trips_password() {
    let service = "gaze-lens-integration-keyring";
    let account = format!("test-{}", ulid::Ulid::new());
    let entry = keyring::Entry::new(service, &account).expect("create keyring entry");

    entry
        .set_password("integration-secret")
        .expect("write keyring password");
    let password = entry.get_password().expect("read keyring password");
    assert_eq!(password, "integration-secret");

    let _ = entry.delete_credential();
}
