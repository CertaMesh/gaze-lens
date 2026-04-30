use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use clap::Args;
use gaze::{Action, ClassRule, DefaultRule, PiiClass, Pipeline, SensitiveSnapshot};
use gaze_recognizers::RegexDetector;

use crate::errors::LensError;
use crate::policy::PolicyFile;
use crate::session::{Session, ToolCall};
use crate::source::{FakeSource, SourceOutput, ToolArgs};
use crate::value::{LensRow, LensValue};

#[derive(Debug, Args)]
pub struct DemoArgs {}

#[derive(Debug)]
pub struct DemoOutcome {
    pub tokenized_section: String,
    pub restored_section: String,
    pub manifest_path: PathBuf,
    pub snapshot_dir: PathBuf,
    pub snapshot_path: PathBuf,
    pub lens_session_id: String,
}

pub async fn run(_args: DemoArgs) -> Result<(), LensError> {
    let temp = tempfile::tempdir().map_err(|err| LensError::Internal {
        detail: format!("demo tempdir: {err}"),
    })?;
    let outcome = run_with_workdir(temp.path()).await?;
    println!("=== Tokenized output (what an agent sees) ===");
    println!("{}", outcome.tokenized_section);
    println!();
    println!("=== Restored output (what `gaze-lens replay` would show on a real session) ===");
    println!("{}", outcome.restored_section);
    println!();
    println!(
        "(demo state lived in a tempdir and was cleaned up; nothing was written to ~/.gaze-lens/)"
    );
    drop(temp);
    Ok(())
}

#[doc(hidden)]
pub async fn run_with_workdir(workdir: &Path) -> Result<DemoOutcome, LensError> {
    let manifest_path = workdir.join("manifest.sqlite");
    let snapshot_dir = workdir.join("snapshots");

    let policy_file = PolicyFile::from_toml("[policy.database]\n").map_err(policy_err)?;
    let policy = policy_file.to_gaze_policy().map_err(policy_err)?;
    let pipeline = build_demo_pipeline()?;

    let session = Arc::new(Session::new_with_pipeline_for_profile(
        &policy,
        pipeline,
        "demo",
        &manifest_path,
        &snapshot_dir,
    )?);
    session.register_fake_source_for_profile(
        crate::session::SourceClass::Database,
        "demo",
        Box::new(DemoSource),
    );

    let lens_session_id = session.lens_session_id().to_string();
    let call_id = ulid::Ulid::new().to_string();
    let args = serde_json::json!({
        "profile": "demo",
        "table": "users",
        "limit": 5,
    });
    let result = session
        .dispatch_tool(ToolCall {
            call_id: call_id.clone(),
            tool_name: "query".to_string(),
            args: ToolArgs(args),
        })
        .await?;

    let tokenized_section =
        serde_json::to_string_pretty(&result.clean).map_err(|err| LensError::Internal {
            detail: format!("serialize tokenized: {err}"),
        })?;

    let snapshot_path = result.snapshot_ref.path.clone();
    let snapshot_bytes = std::fs::read(&snapshot_path).map_err(|err| LensError::Internal {
        detail: format!("read snapshot {}: {err}", snapshot_path.display()),
    })?;
    let gaze_session =
        gaze::Session::import(SensitiveSnapshot::from(snapshot_bytes)).map_err(|err| {
            LensError::Internal {
                detail: format!("import snapshot: {err}"),
            }
        })?;
    let restored_section = restore_tokens(&gaze_session, &tokenized_section)?;

    Ok(DemoOutcome {
        tokenized_section,
        restored_section,
        manifest_path,
        snapshot_dir,
        snapshot_path,
        lens_session_id,
    })
}

fn build_demo_pipeline() -> Result<Pipeline, LensError> {
    let phone = PiiClass::custom("phone");
    let ssn = PiiClass::custom("ssn");
    let email_detector = RegexDetector::emails().map_err(redaction_err)?;
    let phone_detector =
        RegexDetector::new(r"\b\d{3}-\d{3}-\d{4}\b", phone.clone()).map_err(redaction_err)?;
    let ssn_detector =
        RegexDetector::new(r"\b\d{3}-\d{2}-\d{4}\b", ssn.clone()).map_err(redaction_err)?;
    Pipeline::builder()
        .detector(email_detector)
        .detector(phone_detector)
        .detector(ssn_detector)
        .rule(ClassRule::new(PiiClass::Email, Action::Tokenize))
        .rule(ClassRule::new(phone, Action::Tokenize))
        .rule(ClassRule::new(ssn, Action::Tokenize))
        .rule(DefaultRule::new(Action::Preserve))
        .build()
        .map_err(redaction_err)
}

fn restore_tokens(gaze_session: &gaze::Session, input: &str) -> Result<String, LensError> {
    let mut restored = input.to_string();
    let mut tokens = gaze_session.tokens();
    tokens.sort_by_key(|token| std::cmp::Reverse(token.len()));
    for token in tokens {
        let raw = gaze_session
            .restore_strict(&token)
            .map_err(|err| LensError::Internal {
                detail: format!("restore token: {err}"),
            })?;
        restored = restored.replace(&token, &raw);
    }
    Ok(restored)
}

fn redaction_err<E: std::fmt::Display>(err: E) -> LensError {
    LensError::RedactionFailed {
        detail: err.to_string(),
    }
}

fn policy_err(err: crate::policy::PolicyError) -> LensError {
    LensError::Profile {
        detail: err.to_string(),
    }
}

struct DemoSource;

#[async_trait]
impl FakeSource for DemoSource {
    async fn invoke(&self, _args: &ToolArgs) -> Result<SourceOutput, LensError> {
        Ok(SourceOutput::Rows(seed_rows()))
    }
}

fn seed_rows() -> Vec<LensRow> {
    let mut row1: LensRow = BTreeMap::new();
    row1.insert("id".to_string(), LensValue::I64(1));
    row1.insert(
        "email".to_string(),
        LensValue::String("alice@example.com".to_string()),
    );
    row1.insert(
        "phone".to_string(),
        LensValue::String("555-123-4567".to_string()),
    );
    row1.insert(
        "note".to_string(),
        LensValue::String("primary contact".to_string()),
    );

    let mut row2: LensRow = BTreeMap::new();
    row2.insert("id".to_string(), LensValue::I64(2));
    row2.insert(
        "email".to_string(),
        LensValue::String("bob@beta.io".to_string()),
    );
    row2.insert(
        "phone".to_string(),
        LensValue::String("555-987-6543".to_string()),
    );
    row2.insert(
        "note".to_string(),
        LensValue::String("ssn on file: 123-45-6789".to_string()),
    );

    let mut row3: LensRow = BTreeMap::new();
    row3.insert("id".to_string(), LensValue::I64(3));
    row3.insert(
        "email".to_string(),
        LensValue::String("carol@example.net".to_string()),
    );
    row3.insert("phone".to_string(), LensValue::Null);
    row3.insert(
        "note".to_string(),
        LensValue::String("archived".to_string()),
    );

    vec![row1, row2, row3]
}
