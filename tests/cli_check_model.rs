use std::path::Path;

use gaze_lens::cli::check::{CheckArgs, run_with_verifier_for_test};
use gaze_lens::cli::check_trust::TrustFormat;
use gaze_lens::cli::init::model_fetch::FakeVerifier;

#[tokio::test]
async fn production_check_reports_model_error_before_source_validation() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("project.toml");
    let policy = temp.path().join("policy.toml");
    let db = temp.path().join("missing.sqlite");
    let model_dir = temp.path().join("models").join("kiji");
    write_production_profile(&project, &db, &policy, &model_dir);
    let verifier = FakeVerifier::err("bundle missing");
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();

    let err = run_with_verifier_for_test(
        check_args(false),
        Some(&project),
        None,
        &mut stdout,
        &mut stderr,
        &verifier,
    )
    .await
    .expect_err("model verification should fail before source validation");

    let stdout = String::from_utf8(stdout).expect("stdout utf8");
    let stderr = String::from_utf8(stderr).expect("stderr utf8");
    assert!(stdout.contains("profile: ok"), "{stdout}");
    assert!(stdout.contains("policy: ok"), "{stdout}");
    assert!(!stdout.contains("source:"), "{stdout}");
    assert!(stderr.contains("model: NOT PROVISIONED"), "{stderr}");
    assert!(
        stderr.contains("gaze-lens init --production --fetch-model"),
        "{stderr}"
    );
    assert!(err.to_string().contains("production NER model"), "{err}");
}

#[tokio::test]
async fn production_check_reports_model_ok_before_source_validation() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("project.toml");
    let policy = temp.path().join("policy.toml");
    let db = temp.path().join("missing.sqlite");
    let model_dir = temp.path().join("models").join("kiji");
    write_production_profile(&project, &db, &policy, &model_dir);
    let verifier = FakeVerifier::ok();
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();

    let _err = run_with_verifier_for_test(
        check_args(false),
        Some(&project),
        None,
        &mut stdout,
        &mut stderr,
        &verifier,
    )
    .await
    .expect_err("missing sqlite source should fail after model verification");

    let stdout = String::from_utf8(stdout).expect("stdout utf8");
    assert!(stdout.contains("model: ok ("), "{stdout}");
    assert!(!stdout.contains("pipeline: ok"), "{stdout}");
}

#[tokio::test]
async fn explain_risk_skips_production_model_verification() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("project.toml");
    let policy = temp.path().join("policy.toml");
    let db = temp.path().join("missing.sqlite");
    let model_dir = temp.path().join("models").join("kiji");
    write_production_profile(&project, &db, &policy, &model_dir);
    let verifier = FakeVerifier::err("bundle missing");
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();

    run_with_verifier_for_test(
        check_args(true),
        Some(&project),
        None,
        &mut stdout,
        &mut stderr,
        &verifier,
    )
    .await
    .expect("explain-risk stays local-only");

    let stdout = String::from_utf8(stdout).expect("stdout utf8");
    let stderr = String::from_utf8(stderr).expect("stderr utf8");
    assert!(
        stdout.contains("model: skipped (--explain-risk local-only)"),
        "{stdout}"
    );
    assert!(stdout.contains("source: skipped (--explain-risk local-only)"));
    assert!(
        stdout.contains("pipeline: skipped (--explain-risk local-only)"),
        "{stdout}"
    );
    assert!(!stdout.contains("pipeline: ok"), "{stdout}");
    assert!(!stderr.contains("NOT PROVISIONED"), "{stderr}");
}

#[tokio::test]
async fn non_production_bad_model_fails_before_source_validation() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("project.toml");
    let policy = temp.path().join("policy.toml");
    let db = temp.path().join("missing.sqlite");
    let model_dir = temp.path().join("bad-model");
    write_profile(&project, "dev", false, &db, &policy, &model_dir);
    let verifier = FakeVerifier::ok();
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();

    let err = run_with_verifier_for_test(
        check_args_for("dev", false),
        Some(&project),
        None,
        &mut stdout,
        &mut stderr,
        &verifier,
    )
    .await
    .expect_err("bad non-production model should fail before source validation");

    let stdout = String::from_utf8(stdout).expect("stdout utf8");
    let stderr = String::from_utf8(stderr).expect("stderr utf8");
    assert!(stdout.contains("profile: ok"), "{stdout}");
    assert!(!stdout.contains("policy: ok"), "{stdout}");
    assert!(!stdout.contains("source:"), "{stdout}");
    assert!(!stderr.contains("source failed"), "{stderr}");
    assert!(
        err.to_string().contains("failed to build policy pipeline"),
        "{err}"
    );
}

#[tokio::test]
async fn explain_risk_marks_pipeline_skipped_for_unbuildable_non_production_pipeline() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("project.toml");
    let policy = temp.path().join("policy.toml");
    let db = temp.path().join("missing.sqlite");
    let model_dir = temp.path().join("bad-model");
    write_profile(&project, "dev", false, &db, &policy, &model_dir);
    let verifier = FakeVerifier::ok();
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();

    run_with_verifier_for_test(
        check_args_for("dev", true),
        Some(&project),
        None,
        &mut stdout,
        &mut stderr,
        &verifier,
    )
    .await
    .expect("explain-risk reports local risk without building pipeline");

    let stdout = String::from_utf8(stdout).expect("stdout utf8");
    let stderr = String::from_utf8(stderr).expect("stderr utf8");
    assert!(
        stdout.contains("pipeline: skipped (--explain-risk local-only)"),
        "{stdout}"
    );
    assert!(!stdout.contains("pipeline: ok"), "{stdout}");
    assert!(!stderr.contains("source failed"), "{stderr}");
}

fn check_args(explain_risk: bool) -> CheckArgs {
    check_args_for("prod", explain_risk)
}

fn check_args_for(profile: &str, explain_risk: bool) -> CheckArgs {
    CheckArgs {
        profile: profile.into(),
        explain_risk,
        format: TrustFormat::Text,
    }
}

fn write_production_profile(
    profile_path: &Path,
    db_path: &Path,
    policy_path: &Path,
    model_dir: &Path,
) {
    write_profile(profile_path, "prod", true, db_path, policy_path, model_dir);
}

fn write_profile(
    profile_path: &Path,
    profile_name: &str,
    production: bool,
    db_path: &Path,
    policy_path: &Path,
    model_dir: &Path,
) {
    std::fs::write(
        policy_path,
        format!(
            r#"
            [ner]
            model_dir = "{}"

            [policy]
            default_action = "tokenize"

            [policy.database]
            "#,
            model_dir.display()
        ),
    )
    .expect("policy");
    std::fs::write(
        profile_path,
        format!(
            r#"
            [[profiles]]
            name = "{profile_name}"
            production = {production}
            policy = "{}"
            source = {{ kind = "sqlite", path = "{}", readonly_required = true }}
            "#,
            policy_path.display(),
            db_path.display()
        ),
    )
    .expect("profile");
}
