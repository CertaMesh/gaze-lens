//! D15 host gate end-to-end. Asserts that a dash-prefixed host on the right
//! flag for each source kind triggers an exit-1 BEFORE any file is written.

use assert_cmd::Command;

fn bin() -> Command {
    Command::cargo_bin("gaze-lens").unwrap()
}

fn args_for_ssh_log(host_arg_value: &str) -> Vec<String> {
    // `=` syntax forces clap to treat the dash-prefixed value as the value of
    // `--source-host`, not a new flag.
    vec![
        "init".into(),
        "--non-interactive".into(),
        "--profile".into(),
        "p".into(),
        "--source-kind".into(),
        "ssh-log".into(),
        format!("--source-host={host_arg_value}"),
        "--source-path".into(),
        "/var/log/app.log".into(),
        "--scope".into(),
        "user".into(),
        "--no-mcp-config".into(),
        "--no-agents-md".into(),
    ]
}

fn args_for_db_tunnel(kind: &str, ssh_host_arg_value: &str) -> Vec<String> {
    vec![
        "init".into(),
        "--non-interactive".into(),
        "--profile".into(),
        "p".into(),
        "--source-kind".into(),
        kind.into(),
        "--source-host".into(),
        "h".into(),
        "--source-port".into(),
        "1".into(),
        "--source-database".into(),
        "d".into(),
        "--source-username".into(),
        "u".into(),
        "--source-password-env".into(),
        "E".into(),
        format!("--source-ssh-host={ssh_host_arg_value}"),
        "--scope".into(),
        "user".into(),
        "--no-mcp-config".into(),
        "--no-agents-md".into(),
    ]
}

fn assert_init_rejects(args: Vec<String>, label: &str, cwd: &std::path::Path) {
    let home = cwd.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let user_cfg = home.join(".gaze-lens").join("profiles.toml");
    let out = bin()
        .current_dir(cwd)
        .env("HOME", &home)
        .args(&args)
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(1),
        "{label}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("host cannot start with '-'") || stderr.contains("invalid ssh host"),
        "{label} stderr: {stderr}"
    );
    assert!(
        !user_cfg.exists(),
        "{label}: must not write profile when host validation fails"
    );
}

#[test]
fn ssh_log_rejects_dash_prefixed_host_via_source_host() {
    // CB-r2-3: ssh-log `host` flows through `--source-host` (TOML `host` per
    // src/profile.rs:70-73), NOT `--source-ssh-host`.
    let temp = tempfile::tempdir().unwrap();
    assert_init_rejects(
        args_for_ssh_log("-evilflag"),
        "ssh-log via --source-host",
        temp.path(),
    );
}

#[test]
fn mysql_tunnel_host_dash_prefix_rejected_via_source_ssh_host() {
    // Directive 13: db-tunnel jump-host flows through `--source-ssh-host`.
    let temp = tempfile::tempdir().unwrap();
    assert_init_rejects(
        args_for_db_tunnel("mysql", "-evilflag"),
        "mysql tunnel via --source-ssh-host",
        temp.path(),
    );
}

#[test]
fn postgres_tunnel_host_dash_prefix_rejected_via_source_ssh_host() {
    let temp = tempfile::tempdir().unwrap();
    assert_init_rejects(
        args_for_db_tunnel("postgres", "-evilflag"),
        "postgres tunnel via --source-ssh-host",
        temp.path(),
    );
}
