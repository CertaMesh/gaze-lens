use gaze_lens::session::{OutputCaps, Session};
use gaze_lens::source::db::{ColumnInfo, TableSchema};
use std::sync::Arc;

fn policy() -> gaze::Policy {
    gaze::Policy {
        session: gaze::SessionPolicy {
            scope: gaze::SessionScope::Conversation,
            ttl_secs: None,
        },
        detectors: Vec::new(),
        dictionaries: Vec::new(),
        rules: Vec::new(),
        ner: None,
        rulepacks: gaze::RulepackPolicy {
            bundled: vec!["core".to_string()],
            paths: Vec::new(),
        },
        locale: None,
    }
}

fn session() -> Session {
    let temp = tempfile::tempdir().expect("tempdir");
    let snapshot_dir = temp.path().to_path_buf();
    let session = Session::new_with_manifest_for_tests(
        &policy(),
        Arc::new(NoopManifest),
        &snapshot_dir,
        OutputCaps::default(),
    )
    .expect("session");
    std::mem::forget(temp);
    session
}

fn schema() -> TableSchema {
    TableSchema {
        table: "users_pii".to_string(),
        table_token: "users_pii".to_string(),
        limit_cap: Some(100),
        columns: vec![
            col("id"),
            col("created_at"),
            col("email"),
            col("customer_internal_ref"),
        ],
    }
}

fn col(name: &str) -> ColumnInfo {
    ColumnInfo {
        name: name.to_string(),
        name_token: name.to_string(),
        data_type: "varchar".to_string(),
        nullable: false,
        allowed: true,
    }
}

#[test]
fn test_default_allowlist_passes_id_created_at() {
    let session = session();

    let tokenized = session.tokenize_schema_metadata(&schema(), None);

    assert_eq!(tokenized.columns[0].name_token, "id");
    assert_eq!(tokenized.columns[1].name_token, "created_at");
    assert_eq!(tokenized.columns[2].name_token, "<COL_001>");
    assert_eq!(tokenized.columns[3].name_token, "<COL_002>");
}

#[test]
fn test_session_stable_tokens() {
    let session = session();

    let first = session.tokenize_schema_metadata(&schema(), None);
    let second = session.tokenize_schema_metadata(&schema(), None);

    assert_eq!(first.columns[2].name_token, second.columns[2].name_token);
    assert_eq!(first.table_token, second.table_token);
}

#[test]
fn test_per_profile_allowlist() {
    let session = session();
    let allowlist = vec!["email".to_string()];

    let tokenized = session.tokenize_schema_metadata(&schema(), Some(&allowlist));

    assert_eq!(tokenized.columns[2].name_token, "email");
    assert_eq!(tokenized.columns[3].name_token, "<COL_001>");
}

#[test]
fn test_table_name_tokenized_unless_allowlisted() {
    let session = session();

    let tokenized = session.tokenize_schema_metadata(&schema(), None);
    let allowed = session.tokenize_schema_metadata(&schema(), Some(&["users_pii".to_string()]));

    assert_eq!(tokenized.table_token, "<TABLE_001>");
    assert_eq!(allowed.table_token, "users_pii");
}

struct NoopManifest;

impl gaze_lens::session::manifest::ManifestStore for NoopManifest {
    fn begin_call(
        &self,
        _call: &gaze_lens::session::ToolCall,
        _redacted_args: &gaze_lens::session::RedactedToolArgs,
    ) -> Result<(), gaze_lens::errors::LensError> {
        Ok(())
    }

    fn finish_call(
        &self,
        _call_id: &str,
        _summary: &gaze_lens::session::ResultSummary,
        _snapshot_ref: &gaze_lens::session::manifest::SnapshotRef,
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
