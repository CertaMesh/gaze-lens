use std::path::{Path, PathBuf};
use std::sync::Arc;

use assert_cmd::Command;
use async_trait::async_trait;
use gaze_lens::frontend::mcp::McpFrontend;
use gaze_lens::session::manifest::{LensManifestStore, SnapshotRef, initialize_schema};
use gaze_lens::session::restore::restore_whole_session;
use gaze_lens::session::{OutputCaps, RedactedToolArgs, ResultSummary, Session, ToolCall};
use gaze_lens::source::{FakeSource, SourceOutput, ToolArgs};
use rusqlite::{Connection, params};

fn policy() -> gaze::Policy {
    let mut policy = gaze::Policy::default();
    policy.session.scope = gaze::SessionScope::Conversation;
    policy.rulepacks.bundled = vec!["core".to_string()];
    policy
}

#[test]
fn replay_restores_adjacent_and_path_tokens_without_replacing_inserted_raw_text() {
    let fixture = ReplayFixture::new();
    let token_fixture = TokenFixture::new("restore-overlap");
    let redacted = format!(
        r#"{{"path":"{}","adjacent":"{}{}","literal":"/tmp/{}"}}"#,
        token_fixture.path_token,
        token_fixture.path_token,
        token_fixture.email_token,
        token_fixture.email_token
    );
    let expected = format!(
        r#"{{"path":"{}","adjacent":"{}{}","literal":"/tmp/{}"}}"#,
        token_fixture.path_raw,
        token_fixture.path_raw,
        token_fixture.email_raw,
        token_fixture.email_raw
    );
    fixture.insert_ok_call(
        "call-byte-exact",
        &token_fixture.lens_session_id,
        &token_fixture.snapshot_path(&fixture.snapshots),
        &redacted,
        1,
    );

    let restored =
        restore_whole_session(&fixture.manifest, &token_fixture.lens_session_id, 0).unwrap();

    assert_eq!(restored.calls.len(), 1);
    assert_eq!(restored.calls[0].restored_args_json, expected);
    assert_eq!(
        restored.calls[0].restore_telemetry.restore_decision,
        gaze::RestoreDecision::Success
    );
    assert_eq!(restored.restore_telemetry_summary.success_calls, 1);
    assert_eq!(restored.restore_telemetry_summary.partial_calls, 0);
    assert_eq!(restored.restore_telemetry_summary.failed_calls, 0);
    assert_eq!(restored.restore_telemetry_summary.unknown_token_count, 0);
}

#[test]
fn replay_aggregates_failed_strict_restore_telemetry_for_unknown_tokens() {
    let fixture = ReplayFixture::new();
    let token_fixture = TokenFixture::new("restore-unknown");
    let success_redacted = format!(r#"{{"email":"{}"}}"#, token_fixture.email_token);
    let unknown_token = format!("<{}:Email_999>", token_fixture.session.session_hex());
    let failed_redacted = format!(
        r#"{{"known":"{}","unknown":"{}"}}"#,
        token_fixture.email_token, unknown_token
    );
    let snapshot = token_fixture.snapshot_path(&fixture.snapshots);
    fixture.insert_ok_call(
        "call-success",
        &token_fixture.lens_session_id,
        &snapshot,
        &success_redacted,
        1,
    );
    fixture.insert_ok_call(
        "call-failed",
        &token_fixture.lens_session_id,
        &snapshot,
        &failed_redacted,
        2,
    );

    let restored =
        restore_whole_session(&fixture.manifest, &token_fixture.lens_session_id, 0).unwrap();

    assert_eq!(restored.calls.len(), 2);
    assert_eq!(
        restored.calls[0].restore_telemetry.restore_decision,
        gaze::RestoreDecision::Success
    );
    assert_eq!(
        restored.calls[1].restore_telemetry.restore_decision,
        gaze::RestoreDecision::Failed
    );
    assert_eq!(restored.calls[1].restore_telemetry.unknown_token_count, 1);
    assert_eq!(restored.calls[1].restore_telemetry.manifest_bypass_count, 1);
    assert_eq!(restored.restore_telemetry_summary.success_calls, 1);
    assert_eq!(restored.restore_telemetry_summary.partial_calls, 0);
    assert_eq!(restored.restore_telemetry_summary.failed_calls, 1);
    assert_eq!(restored.restore_telemetry_summary.unknown_token_count, 1);
    assert_eq!(restored.restore_telemetry_summary.manifest_bypass_count, 1);
    assert_eq!(
        restored.restore_telemetry_summary.fresh_pii_detected_count,
        0
    );
}

#[test]
fn cli_replay_output_includes_restore_telemetry() {
    let fixture = ReplayFixture::new();
    let token_fixture = TokenFixture::new("restore-cli");
    let redacted = format!(r#"{{"email":"{}"}}"#, token_fixture.email_token);
    fixture.insert_ok_call(
        "call-cli",
        &token_fixture.lens_session_id,
        &token_fixture.snapshot_path(&fixture.snapshots),
        &redacted,
        1,
    );

    let mut replay = Command::cargo_bin("gaze-lens").expect("binary");
    let output = replay
        .args([
            "replay",
            "--manifest",
            fixture.manifest.to_str().expect("manifest path"),
            &token_fixture.lens_session_id,
        ])
        .output()
        .expect("replay");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains(r#""restore_telemetry""#), "{stdout}");
    assert!(
        stdout.contains(r#""restore_telemetry_summary""#),
        "{stdout}"
    );
    assert!(
        stdout.contains(r#""restore_decision": "success""#),
        "{stdout}"
    );
    assert!(stdout.contains(r#""unknown_token_count": 0"#), "{stdout}");
}

#[tokio::test]
async fn inbound_agent_args_keep_unknown_token_literals_leniently() {
    let temp = tempfile::tempdir().expect("tempdir");
    let session = Session::new_with_manifest_for_tests(
        &policy(),
        Arc::new(NoopManifest),
        &temp.path().join("snapshots"),
        OutputCaps::default(),
    )
    .expect("session");
    session.register_fake_source("log_grep", Box::new(EchoPattern));
    let frontend = McpFrontend::with_session(Arc::new(session));
    let unknown_token = "<deadbeef:Email_999>";

    let result = frontend
        .call_tool_json(
            "log_grep",
            serde_json::json!({
                "pattern": unknown_token,
                "limit": 5
            }),
        )
        .await
        .expect("log_grep should accept unknown token-shaped literals");

    let text = result["clean"]["Text"]["text"]
        .as_str()
        .or_else(|| result["clean"]["text"].as_str())
        .expect("text output");
    assert_eq!(text, unknown_token);
}

struct ReplayFixture {
    _temp: tempfile::TempDir,
    manifest: PathBuf,
    snapshots: PathBuf,
}

impl ReplayFixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("tempdir");
        let manifest = temp.path().join("manifest.sqlite");
        let snapshots = temp.path().join("snapshots");
        std::fs::create_dir_all(&snapshots).expect("snapshot dir");
        let conn = Connection::open(&manifest).expect("open manifest");
        initialize_schema(&conn).expect("init schema");
        Self {
            _temp: temp,
            manifest,
            snapshots,
        }
    }

    fn insert_ok_call(
        &self,
        call_id: &str,
        lens_session_id: &str,
        snapshot_path: &Path,
        redacted_args_json: &str,
        started_at_ms: i64,
    ) {
        let conn = Connection::open(&self.manifest).expect("open manifest");
        conn.execute(
            "INSERT OR IGNORE INTO sessions (lens_session_id, gaze_audit_session_id, created_at_ms)
             VALUES (?1, ?2, ?3)",
            params![lens_session_id, "audit-stub", started_at_ms],
        )
        .expect("insert session");
        let redacted_args = serde_json::to_string(&RedactedToolArgs {
            json: redacted_args_json.to_string(),
        })
        .expect("redacted args json");
        conn.execute(
            "INSERT INTO calls (
                call_id, lens_session_id, tool_name, redacted_args_json, status,
                snapshot_ref, started_at_ms
            ) VALUES (?1, ?2, ?3, ?4, 'ok', ?5, ?6)",
            params![
                call_id,
                lens_session_id,
                "query",
                redacted_args,
                snapshot_path.to_string_lossy().to_string(),
                started_at_ms,
            ],
        )
        .expect("insert call");
    }
}

struct TokenFixture {
    lens_session_id: String,
    session: gaze::Session,
    email_raw: String,
    email_token: String,
    path_raw: String,
    path_token: String,
}

impl TokenFixture {
    fn new(lens_session_id: &str) -> Self {
        let session = gaze::Session::new(gaze::Scope::Conversation(lens_session_id.to_string()))
            .expect("gaze session");
        let email_raw = "bob@example.com".to_string();
        let email_token = session
            .tokenize(&gaze::PiiClass::Email, &email_raw)
            .expect("email token");
        let path_raw = format!("/var/log/{email_token}/audit.log");
        let path_token = session
            .tokenize(&gaze::PiiClass::custom("path"), &path_raw)
            .expect("path token");
        Self {
            lens_session_id: lens_session_id.to_string(),
            session,
            email_raw,
            email_token,
            path_raw,
            path_token,
        }
    }

    fn snapshot_path(&self, snapshot_dir: &Path) -> PathBuf {
        let path = snapshot_dir.join(format!("{}.snap", self.lens_session_id));
        let bytes = self.session.export().expect("export snapshot").into_bytes();
        std::fs::write(&path, bytes).expect("write snapshot");
        path
    }
}

struct EchoPattern;

#[async_trait]
impl FakeSource for EchoPattern {
    async fn invoke(&self, args: &ToolArgs) -> Result<SourceOutput, gaze_lens::errors::LensError> {
        Ok(SourceOutput::Text(
            args.0["pattern"].as_str().expect("pattern").to_string(),
        ))
    }
}

struct NoopManifest;

impl LensManifestStore for NoopManifest {
    fn begin_call(
        &self,
        _call: &ToolCall,
        _redacted_args: &RedactedToolArgs,
    ) -> Result<(), gaze_lens::errors::LensError> {
        Ok(())
    }

    fn finish_call(
        &self,
        _call_id: &str,
        _summary: &ResultSummary,
        _snapshot_ref: &SnapshotRef,
    ) -> Result<(), gaze_lens::errors::LensError> {
        Ok(())
    }

    fn fail_call(
        &self,
        _call_id: &str,
        _err: &gaze_lens::errors::LensError,
    ) -> Result<(), gaze_lens::errors::LensError> {
        Ok(())
    }
}
