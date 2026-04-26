use std::path::Path;

use gaze_lens::source::ssh_tunnel::{TunnelSpec, close_argv, open_argv, validate_ssh_host};

#[test]
fn test_validate_ssh_host_rejects_dash_prefix() {
    assert!(validate_ssh_host("-oProxyCommand=evil").is_err());
}

#[test]
fn test_validate_ssh_host_rejects_metacharacters() {
    for ch in [";", "|", "&", "`", "$", "\n"] {
        assert!(
            validate_ssh_host(&format!("prod{ch}evil")).is_err(),
            "expected rejection for {ch:?}"
        );
    }
}

#[test]
fn test_validate_ssh_host_accepts_valid() {
    for host in ["prod", "prod-1", "prod.internal", "192.168.1.1"] {
        assert_eq!(validate_ssh_host(host).expect("valid"), host);
    }
}

#[test]
fn test_command_includes_double_dash() {
    let argv = open_argv(&TunnelSpec {
        ssh_host: "prod".to_string(),
        local_port: 13306,
        remote_host: "127.0.0.1".to_string(),
        remote_port: 3306,
    })
    .expect("argv");

    let dash_index = argv.iter().position(|arg| arg == "--").expect("--");
    assert_eq!(argv.get(dash_index + 1).map(String::as_str), Some("prod"));
}

#[test]
fn test_command_includes_double_dash_on_close() {
    let argv = close_argv("prod", Path::new("/tmp/gaze.sock")).expect("argv");

    let dash_index = argv.iter().position(|arg| arg == "--").expect("--");
    assert_eq!(argv.get(dash_index + 1).map(String::as_str), Some("prod"));
}
