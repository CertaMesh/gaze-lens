use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gaze::{Action, ClassRule, CleanDocument, DefaultRule, RawDocument};
use gaze_recognizers::RegexDetector;

use crate::errors::LensError;
use crate::source::db::TableSchema;
use crate::source::db::schema::SchemaTokenizer;
use crate::source::{FakeSource, SourceOutput, ToolArgs};
use crate::value::{LensRow, LensValue, LowerError};

pub mod manifest;
pub mod restore;

use manifest::{ManifestStore, ManifestWriter, SnapshotRef};

#[derive(Clone)]
pub struct Session {
    inner: Arc<SessionInner>,
}

struct SessionInner {
    lens_session_id: ulid::Ulid,
    gaze_session: gaze::Session,
    pipeline: Arc<gaze::Pipeline>,
    manifest: Arc<dyn ManifestStore>,
    snapshot_dir: PathBuf,
    sources: Mutex<HashMap<String, Arc<dyn FakeSource>>>,
    schema_tokenizer: SchemaTokenizer,
    caps: OutputCaps,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ToolCall {
    pub call_id: String,
    pub tool_name: String,
    pub args: ToolArgs,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RedactedToolArgs {
    pub json: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ToolResult {
    pub clean: CleanOutput,
    pub snapshot_ref: SnapshotRef,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum CleanOutput {
    Rows {
        rows: Vec<serde_json::Map<String, serde_json::Value>>,
        truncated_at: Vec<TruncatedAt>,
    },
    Text {
        text: String,
        truncated_at: Vec<TruncatedAt>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ResultSummary {
    pub rows: u32,
    pub bytes: u64,
    pub truncated_at: Vec<TruncatedAt>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TruncatedAt {
    Rows,
    Bytes,
    CellBytes,
    LineBytes,
    Timeout,
}

#[derive(Debug, Clone, Copy)]
pub struct OutputCaps {
    pub rows: usize,
    pub bytes: usize,
    pub cell_bytes: usize,
    pub line_bytes: usize,
    pub timeout: Duration,
}

impl Default for OutputCaps {
    fn default() -> Self {
        Self {
            rows: 1000,
            bytes: 1024 * 1024,
            cell_bytes: 32 * 1024,
            line_bytes: 8 * 1024,
            timeout: Duration::from_secs(30),
        }
    }
}

impl Session {
    pub fn new(
        policy: &gaze::Policy,
        manifest_path: &Path,
        snapshot_dir: &Path,
    ) -> Result<Self, LensError> {
        let lens_session_id = ulid::Ulid::new();
        let gaze_session = Self::build_gaze_session(&lens_session_id, policy)?;
        let manifest = ManifestWriter::new(
            manifest_path,
            lens_session_id,
            gaze_session.audit_session_id(),
        )?;
        Ok(Self::from_parts(
            lens_session_id,
            gaze_session,
            default_pipeline()?,
            Arc::new(manifest),
            snapshot_dir.to_path_buf(),
            OutputCaps::default(),
        ))
    }

    pub fn lens_session_id(&self) -> ulid::Ulid {
        self.inner.lens_session_id
    }

    fn from_parts(
        lens_session_id: ulid::Ulid,
        gaze_session: gaze::Session,
        pipeline: gaze::Pipeline,
        manifest: Arc<dyn ManifestStore>,
        snapshot_dir: PathBuf,
        caps: OutputCaps,
    ) -> Self {
        Self {
            inner: Arc::new(SessionInner {
                lens_session_id,
                gaze_session,
                pipeline: Arc::new(pipeline),
                manifest,
                snapshot_dir,
                sources: Mutex::new(HashMap::new()),
                schema_tokenizer: SchemaTokenizer::default(),
                caps,
            }),
        }
    }

    fn build_gaze_session(
        lens_id: &ulid::Ulid,
        policy: &gaze::Policy,
    ) -> Result<gaze::Session, LensError> {
        if matches!(policy.session.scope, gaze::SessionScope::Ephemeral) {
            return Err(LensError::ScopeRejected {
                scope: "ephemeral".to_string(),
            });
        }
        gaze::Session::new(gaze::Scope::Conversation(lens_id.to_string())).map_err(|err| {
            LensError::Internal {
                detail: err.to_string(),
            }
        })
    }

    pub async fn dispatch_tool(&self, call: ToolCall) -> Result<ToolResult, LensError> {
        let redacted_args = self.redact_args(&call.args)?;
        self.inner.manifest.begin_call(&call, &redacted_args)?;

        let raw_result =
            match tokio::time::timeout(self.inner.caps.timeout, self.invoke_source(&call)).await {
                Ok(result) => result.inspect_err(|err| {
                    let _ = self.inner.manifest.fail_call(&call.call_id, err);
                })?,
                Err(_) => {
                    let err = LensError::Truncated(TruncatedAt::Timeout);
                    self.finish_truncated_call(&call.call_id, TruncatedAt::Timeout)?;
                    return Err(err);
                }
            };
        let clean = match tokio::time::timeout(self.inner.caps.timeout, async {
            self.redact_result(raw_result)
        })
        .await
        {
            Ok(result) => result?,
            Err(_) => {
                let err = LensError::Truncated(TruncatedAt::Timeout);
                self.finish_truncated_call(&call.call_id, TruncatedAt::Timeout)?;
                return Err(err);
            }
        };
        let snapshot_ref = self.persist_snapshot()?;
        let summary = clean.summary();
        self.inner
            .manifest
            .finish_call(&call.call_id, &summary, &snapshot_ref)?;
        Ok(ToolResult {
            clean,
            snapshot_ref,
        })
    }

    pub fn register_fake_source(&mut self, name: &str, source: Box<dyn FakeSource>) {
        self.inner
            .sources
            .lock()
            .expect("source map lock")
            .insert(name.to_string(), Arc::from(source));
    }

    pub fn tokenize_schema_metadata(
        &self,
        schema: &TableSchema,
        profile_allowlist: Option<&[String]>,
    ) -> TableSchema {
        self.inner
            .schema_tokenizer
            .tokenize_table_schema(schema, profile_allowlist)
    }

    pub fn tokenize_table_names(
        &self,
        tables: &[String],
        profile_allowlist: Option<&[String]>,
    ) -> Vec<String> {
        self.inner
            .schema_tokenizer
            .tokenize_table_names(tables, profile_allowlist)
    }

    fn redact_args(&self, args: &ToolArgs) -> Result<RedactedToolArgs, LensError> {
        let raw = serde_json::to_string(&args.0).map_err(|err| LensError::RedactionFailed {
            detail: err.to_string(),
        })?;
        let clean = self
            .inner
            .pipeline
            .redact(&self.inner.gaze_session, RawDocument::Text(raw))
            .map_err(|err| LensError::RedactionFailed {
                detail: err.to_string(),
            })?;
        match clean {
            CleanDocument::Text(json) => Ok(RedactedToolArgs { json }),
            CleanDocument::Structured(_) => Err(LensError::RedactionFailed {
                detail: "text args produced structured output".to_string(),
            }),
        }
    }

    async fn invoke_source(&self, call: &ToolCall) -> Result<SourceOutput, LensError> {
        if matches!(call.tool_name.as_str(), "log_tail" | "log_grep") {
            return Err(LensError::FeatureDeferred(format!(
                "{} in PR2b",
                call.tool_name
            )));
        }
        let source = self
            .inner
            .sources
            .lock()
            .expect("source map lock")
            .get(&call.tool_name)
            .cloned()
            .ok_or_else(|| LensError::SourceError {
                source_name: call.tool_name.clone(),
                detail: "unknown fake source".to_string(),
                sql: None,
                stderr: None,
            })?;
        source.invoke(&call.args).await
    }

    fn redact_result(&self, output: SourceOutput) -> Result<CleanOutput, LensError> {
        match output {
            SourceOutput::Rows(rows) => self.redact_rows(rows),
            SourceOutput::Text(text) => self.redact_text_output(text),
        }
    }

    fn redact_rows(&self, rows: Vec<LensRow>) -> Result<CleanOutput, LensError> {
        let mut truncated_at = Vec::new();
        if rows.len() > self.inner.caps.rows {
            truncated_at.push(TruncatedAt::Rows);
        }
        let mut clean_rows = Vec::new();
        let mut total_bytes = 0usize;
        for row in rows.into_iter().take(self.inner.caps.rows) {
            let mut raw_fields = std::collections::BTreeMap::new();
            let mut redacted_row = row;
            for (key, value) in &mut redacted_row {
                value.redact_with(&self.inner.gaze_session, &self.inner.pipeline)?;
                if let Some(lowered) = value.lower_for_redaction()? {
                    raw_fields.insert(key.clone(), lowered);
                }
            }
            let redacted = self
                .inner
                .pipeline
                .redact(
                    &self.inner.gaze_session,
                    RawDocument::Structured(raw_fields),
                )
                .map_err(|err| LensError::RedactionFailed {
                    detail: err.to_string(),
                })?;
            let redacted_fields = match redacted {
                CleanDocument::Structured(fields) => fields,
                CleanDocument::Text(_) => {
                    return Err(LensError::RedactionFailed {
                        detail: "structured rows produced text output".to_string(),
                    });
                }
            };
            let mut out = serde_json::Map::new();
            for (key, value) in redacted_row {
                if let Some(redacted) = redacted_fields.get(&key) {
                    insert_capped_cell(
                        &mut out,
                        key,
                        gaze_value_to_json(redacted.clone()),
                        &mut truncated_at,
                        self.inner.caps.cell_bytes,
                    )?;
                } else {
                    let cell = lens_value_to_json(value)?;
                    insert_capped_cell(
                        &mut out,
                        key,
                        cell,
                        &mut truncated_at,
                        self.inner.caps.cell_bytes,
                    )?;
                }
            }
            let row_bytes = serde_json::to_vec(&out)
                .map_err(|err| {
                    LensError::ConvertError(LowerError::Decode {
                        kind: "json",
                        detail: err.to_string(),
                    })
                })?
                .len();
            if total_bytes.saturating_add(row_bytes) > self.inner.caps.bytes {
                push_truncation(&mut truncated_at, TruncatedAt::Bytes);
                break;
            }
            total_bytes += row_bytes;
            clean_rows.push(out);
        }
        Ok(CleanOutput::Rows {
            rows: clean_rows,
            truncated_at,
        })
    }

    fn redact_text_output(&self, text: String) -> Result<CleanOutput, LensError> {
        let mut truncated_at = Vec::new();
        let (capped, truncated) = cap_string(text, self.inner.caps.line_bytes);
        if truncated {
            truncated_at.push(TruncatedAt::LineBytes);
        }
        let clean = self
            .inner
            .pipeline
            .redact(&self.inner.gaze_session, RawDocument::Text(capped))
            .map_err(|err| LensError::RedactionFailed {
                detail: err.to_string(),
            })?;
        match clean {
            CleanDocument::Text(text) => Ok(CleanOutput::Text { text, truncated_at }),
            CleanDocument::Structured(_) => Err(LensError::RedactionFailed {
                detail: "text output produced structured output".to_string(),
            }),
        }
    }

    fn persist_snapshot(&self) -> Result<SnapshotRef, LensError> {
        persist_snapshot(
            &self.inner.snapshot_dir,
            self.inner.lens_session_id,
            &self.inner.gaze_session,
        )
    }

    fn finish_truncated_call(&self, call_id: &str, reason: TruncatedAt) -> Result<(), LensError> {
        let snapshot_ref = self.persist_snapshot()?;
        let summary = ResultSummary {
            rows: 0,
            bytes: 0,
            truncated_at: vec![reason],
        };
        self.inner
            .manifest
            .finish_call(call_id, &summary, &snapshot_ref)
    }

    #[doc(hidden)]
    pub fn new_with_manifest_for_tests(
        policy: &gaze::Policy,
        manifest: Arc<dyn ManifestStore>,
        snapshot_dir: &Path,
        caps: OutputCaps,
    ) -> Result<Self, LensError> {
        let lens_session_id = ulid::Ulid::new();
        let gaze_session = Self::build_gaze_session(&lens_session_id, policy)?;
        Ok(Self::from_parts(
            lens_session_id,
            gaze_session,
            default_pipeline()?,
            manifest,
            snapshot_dir.to_path_buf(),
            caps,
        ))
    }
}

impl CleanOutput {
    pub fn summary(&self) -> ResultSummary {
        match self {
            CleanOutput::Rows { rows, truncated_at } => ResultSummary {
                rows: rows.len().min(u32::MAX as usize) as u32,
                bytes: serde_json::to_vec(rows)
                    .map(|bytes| bytes.len())
                    .unwrap_or(0)
                    .min(u64::MAX as usize) as u64,
                truncated_at: truncated_at.clone(),
            },
            CleanOutput::Text { text, truncated_at } => ResultSummary {
                rows: 0,
                bytes: text.len() as u64,
                truncated_at: truncated_at.clone(),
            },
        }
    }
}

fn insert_capped_cell(
    row: &mut serde_json::Map<String, serde_json::Value>,
    key: String,
    cell: serde_json::Value,
    truncated_at: &mut Vec<TruncatedAt>,
    max_bytes: usize,
) -> Result<(), LensError> {
    let cell_bytes = serde_json::to_vec(&cell)
        .map_err(|err| {
            LensError::ConvertError(LowerError::Decode {
                kind: "json",
                detail: err.to_string(),
            })
        })?
        .len();
    if cell_bytes > max_bytes {
        push_truncation(truncated_at, TruncatedAt::CellBytes);
        row.insert(
            key,
            serde_json::Value::String("<TRUNCATED:cell_bytes>".to_string()),
        );
    } else {
        row.insert(key, cell);
    }
    Ok(())
}

fn push_truncation(truncated_at: &mut Vec<TruncatedAt>, reason: TruncatedAt) {
    if !truncated_at.contains(&reason) {
        truncated_at.push(reason);
    }
}

fn lens_value_to_json(value: LensValue) -> Result<serde_json::Value, LensError> {
    match value {
        LensValue::Bytes { base64, len } => Ok(serde_json::json!({
            "type": "bytes",
            "base64": base64,
            "len": len,
        })),
        value => serde_json::to_value(value).map_err(|err| {
            LensError::ConvertError(LowerError::Decode {
                kind: "json",
                detail: err.to_string(),
            })
        }),
    }
}

fn gaze_value_to_json(value: gaze::Value) -> serde_json::Value {
    match value {
        gaze::Value::String(value) => serde_json::Value::String(value),
        gaze::Value::I64(value) => serde_json::json!(value),
    }
}

fn default_pipeline() -> Result<gaze::Pipeline, LensError> {
    gaze::Pipeline::builder()
        .detector(
            RegexDetector::emails().map_err(|err| LensError::RedactionFailed {
                detail: err.to_string(),
            })?,
        )
        .rule(ClassRule::new(gaze::PiiClass::Email, Action::Tokenize))
        .rule(DefaultRule::new(Action::Preserve))
        .build()
        .map_err(|err| LensError::RedactionFailed {
            detail: err.to_string(),
        })
}

fn persist_snapshot(
    snapshot_dir: &Path,
    lens_session_id: ulid::Ulid,
    gaze_session: &gaze::Session,
) -> Result<SnapshotRef, LensError> {
    std::fs::create_dir_all(snapshot_dir).map_err(|err| LensError::ManifestFinishFailed {
        call_id: "snapshot".to_string(),
        detail: err.to_string(),
        path: Some(snapshot_dir.to_path_buf()),
    })?;
    set_dir_private(snapshot_dir)?;
    let path = snapshot_dir.join(format!("{lens_session_id}.snap"));
    let bytes = gaze_session
        .export()
        .map_err(|err| LensError::ManifestFinishFailed {
            call_id: "snapshot".to_string(),
            detail: err.to_string(),
            path: Some(path.clone()),
        })?
        .into_bytes();
    write_private_file(&path, &bytes)?;
    Ok(SnapshotRef { path })
}

fn cap_string(value: String, max_bytes: usize) -> (String, bool) {
    if value.len() <= max_bytes {
        return (value, false);
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    (value[..end].to_string(), true)
}

#[cfg(unix)]
fn set_dir_private(path: &Path) -> Result<(), LensError> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).map_err(|err| {
        LensError::ManifestFinishFailed {
            call_id: "snapshot".to_string(),
            detail: err.to_string(),
            path: Some(path.to_path_buf()),
        }
    })
}

#[cfg(not(unix))]
fn set_dir_private(_path: &Path) -> Result<(), LensError> {
    Ok(())
}

#[cfg(unix)]
fn write_private_file(path: &Path, bytes: &[u8]) -> Result<(), LensError> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .map_err(|err| LensError::ManifestFinishFailed {
            call_id: "snapshot".to_string(),
            detail: err.to_string(),
            path: Some(path.to_path_buf()),
        })?;
    file.write_all(bytes)
        .map_err(|err| LensError::ManifestFinishFailed {
            call_id: "snapshot".to_string(),
            detail: err.to_string(),
            path: Some(path.to_path_buf()),
        })
}

#[cfg(not(unix))]
fn write_private_file(path: &Path, bytes: &[u8]) -> Result<(), LensError> {
    std::fs::write(path, bytes).map_err(|err| LensError::ManifestFinishFailed {
        call_id: "snapshot".to_string(),
        detail: err.to_string(),
        path: Some(path.to_path_buf()),
    })
}
