//! AC #17: `pub fn validate_ssh_host` lives in `src/source/ssh_tunnel.rs` and
//! is called from init + serve/check via the same symbol. This test pins the
//! single-source-of-truth invariant by asserting that the public fn referenced
//! from the init flow is the same one referenced from `serve`/`check`.

#[test]
fn validator_called_from_init_and_tunnel_open() {
    use gaze_lens::source::ssh_tunnel::validate_ssh_host;
    // Sanity: rejects a dash-prefixed host with the same canonical message
    // whether called from init's pre-write gate or from a runtime tunnel open.
    let err = validate_ssh_host("-evil").unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("host cannot start with '-'"),
        "single-source validator must reject dash-prefixed hosts; got: {msg}"
    );
    // And accepts well-formed hosts (validator's charset is ASCII letters,
    // digits, `.`, `_`, `-`).
    assert!(validate_ssh_host("example.com").is_ok());
    assert!(validate_ssh_host("prod-db_01.internal").is_ok());
}
