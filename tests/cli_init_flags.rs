//! clap-surface and runtime-validate assertions for the new `init` flag matrix.
//!
//! Behavioral tests that exercise the new run path (writing a generated TOML
//! profile via `commit_plan`) live in `tests/init_idempotent.rs`,
//! `tests/init_smoke_check.rs`, and `tests/init_flow.rs`. This file is restricted
//! to assertions that can be made without driving the guided flow.

use assert_cmd::Command;
use gaze_lens::cli::init::{InitArgs, SecretBackendChoice, SourceKind};

fn bin() -> Command {
    Command::cargo_bin("gaze-lens").unwrap()
}

#[test]
fn print_only_with_write_all_is_clap_error() {
    let out = bin()
        .args(["init", "--print-only", "--write-all"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn discover_with_print_only_is_clap_error() {
    let out = bin()
        .args([
            "init",
            "--print-only",
            "--discover-ssh-host",
            "deploy@app01",
            "--discover-env-path",
            "/var/www/app/.env",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn auto_purge_user_combo_is_clap_unknown_arg() {
    // CB1: there is no `--auto-purge` flag on the post-CB1 surface. The
    // destructive consent rides on `--scope project-auto-purge`. So passing
    // a literal `--auto-purge` token is a clap unknown-arg error (exit 2).
    let out = bin()
        .args([
            "init",
            "--non-interactive",
            "--profile",
            "x",
            "--source-kind",
            "sqlite",
            "--source-path",
            "/tmp/x.db",
            "--scope",
            "user",
            "--auto-purge",
        ])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(2),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn project_auto_purge_is_a_valid_scope_value() {
    // Clap accepts `--scope project-auto-purge` at the parse layer. Behavior
    // assertion (auto_purge = "purge" in the rendered TOML) lives in P5/P6
    // tests once commit_plan is wired.
    let out = bin()
        .args([
            "init",
            "--print-only",
            "--non-interactive",
            "--profile",
            "x",
            "--source-kind",
            "sqlite",
            "--source-path",
            "/tmp/x.db",
            "--scope",
            "project-auto-purge",
        ])
        .output()
        .unwrap();
    // print-only path exits 0 in legacy run body too; once P4 lands, this
    // test continues to pass because clap-parse never rejected the value.
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn non_interactive_without_profile_errors_at_runtime() {
    let out = bin()
        .args([
            "init",
            "--non-interactive",
            "--source-kind",
            "sqlite",
            "--source-path",
            "/tmp/x.db",
        ])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(1),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--profile") || stderr.contains("profile name"),
        "stderr: {stderr}"
    );
}

#[test]
fn non_interactive_ssh_log_without_source_host_errors_at_runtime() {
    // CB-r2-3: ssh-log requires --source-host (TOML field `host` per
    // src/profile.rs:70-73). validate() rejects with exit 1.
    let out = bin()
        .args([
            "init",
            "--non-interactive",
            "--profile",
            "p",
            "--source-kind",
            "ssh-log",
            "--source-path",
            "/var/log/app.log",
        ])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(1),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--source-host"),
        "stderr should cite missing --source-host: {stderr}"
    );
}

#[test]
fn source_json_text_columns_flag_accepted_as_csv() {
    // Directive 18: `--source-json-text-columns metadata,payload` parses into
    // Vec<String>. Real semantic check lives in the renderer.
    let out = bin()
        .args([
            "init",
            "--print-only",
            "--non-interactive",
            "--profile",
            "x",
            "--source-kind",
            "sqlite",
            "--source-path",
            "/tmp/x.db",
            "--scope",
            "user",
            "--no-mcp-config",
            "--no-agents-md",
            "--source-json-text-columns",
            "metadata,payload",
        ])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn model_dir_without_production_is_rejected() {
    let out = bin()
        .args([
            "init",
            "--print-only",
            "--non-interactive",
            "--profile",
            "x",
            "--source-kind",
            "sqlite",
            "--source-path",
            "/tmp/x.db",
            "--scope",
            "project",
            "--model-dir",
            "/tmp/gaze-model",
        ])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(2),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn fetch_model_is_not_registered_in_phase_1a() {
    let out = bin()
        .args([
            "init",
            "--print-only",
            "--non-interactive",
            "--profile",
            "x",
            "--source-kind",
            "sqlite",
            "--source-path",
            "/tmp/x.db",
            "--scope",
            "project",
            "--fetch-model",
        ])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(2),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn secret_backend_env_default() {
    let args = InitArgs::default_for_test();
    assert_eq!(args.secret_backend, SecretBackendChoice::Env);
}

#[test]
fn secret_backend_keyring_with_service_account() {
    let mut args = InitArgs::default_for_test();
    args.profile = Some("p".into());
    args.source_kind = Some(SourceKind::Mysql);
    args.secret_backend = SecretBackendChoice::Keyring;
    args.source_password_keyring_service = Some("gaze-lens".into());
    args.source_password_keyring_account = Some("prod".into());
    args.no_keyring_write = true;
    args.validate().expect("valid keyring flags");
}

#[test]
fn validate_rejects_env_backend_with_keyring_flags() {
    let mut args = InitArgs::default_for_test();
    args.source_password_keyring_service = Some("gaze-lens".into());
    let err = args.validate().expect_err("must reject");
    assert!(err.to_string().contains("--secret-backend keyring"));
}

#[test]
fn validate_rejects_keyring_backend_with_env_flag() {
    let mut args = InitArgs::default_for_test();
    args.secret_backend = SecretBackendChoice::Keyring;
    args.source_password_env = Some("DB_PASSWORD".into());
    let err = args.validate().expect_err("must reject");
    assert!(err.to_string().contains("--source-password-env"));
}

#[test]
fn non_interactive_keyring_without_no_keyring_write_rejects() {
    let mut args = InitArgs::default_for_test();
    args.profile = Some("p".into());
    args.source_kind = Some(SourceKind::Mysql);
    args.non_interactive = true;
    args.secret_backend = SecretBackendChoice::Keyring;
    args.source_password_keyring_service = Some("gaze-lens".into());
    args.source_password_keyring_account = Some("prod".into());

    let err = args.validate().expect_err("must reject combo");
    let detail = err.to_string();
    assert!(detail.contains("--non-interactive"), "{detail}");
    assert!(detail.contains("--secret-backend keyring"), "{detail}");
    assert!(detail.contains("--no-keyring-write"), "{detail}");
}

#[test]
fn accept_prod_rw_value_must_match_discover_host() {
    let mut args = InitArgs::default_for_test();
    args.profile = Some("p".into());
    args.non_interactive = true;
    args.discover_ssh_host = Some("deploy@app01".into());
    args.discover_env_path = Some("/var/www/app/.env".into());
    args.accept_prod_rw = Some("deploy@app02".into());
    let err = args.validate().expect_err("must reject typeback mismatch");
    assert!(err.to_string().contains("--accept-prod-rw"));
}

#[test]
fn non_interactive_discovery_requires_accept_prod_rw() {
    let mut args = InitArgs::default_for_test();
    args.profile = Some("p".into());
    args.non_interactive = true;
    args.discover_ssh_host = Some("deploy@app01".into());
    args.discover_env_path = Some("/var/www/app/.env".into());
    let err = args
        .validate()
        .expect_err("must reject noninteractive path b");
    assert!(err.to_string().contains("--accept-prod-rw"));
}
