#[cfg(not(unix))]
compile_error!(
    "gaze-lens v0.1 is Unix-only. Snapshot file privacy (0600/0700) is enforced via Unix file modes; no equivalent guarantee on non-Unix platforms. See SPEC.md §threat-model."
);

use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gaze::{Action, ClassRule, CleanDocument, DefaultRule, RawDocument};
use gaze_recognizers::RegexDetector;

use crate::errors::LensError;
use crate::manifest::gaze_mcp_adapter::GazeMcpManifestAdapter;
use crate::mcp::auth::LensAuthHook;
use crate::mcp::tools::list_tables::ListTablesTool;
use crate::mcp::tools::log_grep::LogGrepTool;
use crate::mcp::tools::log_tail::LogTailTool;
use crate::mcp::tools::query::QueryTool;
use crate::mcp::tools::schema::SchemaTool;
use crate::policy::{ColumnAction, ColumnActionPolicy};
use crate::source::db::TableSchema;
use crate::source::db::schema::SchemaTokenizer;
use crate::source::{FakeSource, FakeSourceAdapter, Source, SourceOutput, ToolArgs};
use crate::value::{LensRow, LensValue, LowerError, gaze_value_to_json};

pub mod maintenance;
pub mod manifest;
pub mod restore;

use manifest::{LensManifestStore, ManifestWriter, SnapshotRef};

#[derive(Clone)]
pub struct Session {
    inner: Arc<SessionInner>,
}

struct SessionInner {
    lens_session_id: ulid::Ulid,
    gaze_session: gaze::Session,
    pipeline_mode: Mutex<PipelineMode>,
    column_action_mode: Mutex<ColumnActionMode>,
    manifest: Arc<dyn LensManifestStore>,
    core_summaries: Arc<Mutex<HashMap<String, ResultSummary>>>,
    snapshot_dir: PathBuf,
    sources: Mutex<HashMap<(SourceClass, String), Arc<LazySource>>>,
    legacy_sources: Mutex<HashMap<String, Arc<dyn Source>>>,
    schema_tokenizer: SchemaTokenizer,
    caps: OutputCaps,
}

pub(crate) enum PipelineMode {
    SingleProfile {
        name: String,
        pipeline: Arc<gaze::Pipeline>,
    },
    MultiProfile(HashMap<String, Arc<gaze::Pipeline>>),
}

#[derive(Clone)]
enum ColumnActionMode {
    SingleProfile {
        name: String,
        policy: ColumnActionPolicy,
    },
    MultiProfile(HashMap<String, ColumnActionPolicy>),
}

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
#[doc(hidden)]
pub enum SourceClass {
    Database,
    Log,
}

impl SourceClass {
    fn as_str(self) -> &'static str {
        match self {
            SourceClass::Database => "database",
            SourceClass::Log => "log",
        }
    }
}

pub(crate) fn tool_kind(tool: &str) -> Option<SourceClass> {
    match tool {
        "query" | "schema" | "list_tables" => Some(SourceClass::Database),
        "log_tail" | "log_grep" => Some(SourceClass::Log),
        _ => None,
    }
}

#[doc(hidden)]
pub type SourceBuilder = Arc<
    dyn Fn() -> Pin<Box<dyn Future<Output = Result<Arc<dyn Source>, LensError>> + Send>>
        + Send
        + Sync,
>;

#[doc(hidden)]
pub struct LazySource {
    cell: tokio::sync::OnceCell<Arc<dyn Source>>,
    builder: SourceBuilder,
}

impl LazySource {
    #[doc(hidden)]
    pub fn new(builder: SourceBuilder) -> Self {
        Self {
            cell: tokio::sync::OnceCell::new(),
            builder,
        }
    }

    fn ready(source: Arc<dyn Source>) -> Self {
        let cell = tokio::sync::OnceCell::new();
        if cell.set(source.clone()).is_err() {
            unreachable!("new once cell accepts value");
        }
        Self {
            cell,
            builder: Arc::new(move || {
                let source = source.clone();
                Box::pin(async move { Ok(source) })
            }),
        }
    }

    async fn get(&self) -> Result<Arc<dyn Source>, LensError> {
        self.cell
            .get_or_try_init(|| (self.builder)())
            .await
            .cloned()
    }
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
        Self::new_with_pipeline(policy, default_pipeline()?, manifest_path, snapshot_dir)
    }

    pub fn new_with_pipeline(
        policy: &gaze::Policy,
        pipeline: gaze::Pipeline,
        manifest_path: &Path,
        snapshot_dir: &Path,
    ) -> Result<Self, LensError> {
        Self::new_with_pipeline_for_profile(
            policy,
            pipeline,
            "default",
            manifest_path,
            snapshot_dir,
        )
    }

    pub fn new_with_pipeline_for_profile(
        policy: &gaze::Policy,
        pipeline: gaze::Pipeline,
        profile_name: impl Into<String>,
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
        let profile_name = profile_name.into();
        Ok(Self::from_parts(
            lens_session_id,
            gaze_session,
            PipelineMode::SingleProfile {
                name: profile_name.clone(),
                pipeline: Arc::new(pipeline),
            },
            ColumnActionMode::SingleProfile {
                name: profile_name,
                policy: ColumnActionPolicy::default(),
            },
            Arc::new(manifest),
            snapshot_dir.to_path_buf(),
            OutputCaps::default(),
        ))
    }

    pub fn new_for_multi_profile(
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
            PipelineMode::MultiProfile(HashMap::new()),
            ColumnActionMode::MultiProfile(HashMap::new()),
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
        pipeline_mode: PipelineMode,
        column_action_mode: ColumnActionMode,
        manifest: Arc<dyn LensManifestStore>,
        snapshot_dir: PathBuf,
        caps: OutputCaps,
    ) -> Self {
        Self {
            inner: Arc::new(SessionInner {
                lens_session_id,
                gaze_session,
                pipeline_mode: Mutex::new(pipeline_mode),
                column_action_mode: Mutex::new(column_action_mode),
                manifest,
                core_summaries: Arc::new(Mutex::new(HashMap::new())),
                snapshot_dir,
                sources: Mutex::new(HashMap::new()),
                legacy_sources: Mutex::new(HashMap::new()),
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

    pub async fn dispatch_tool(&self, mut call: ToolCall) -> Result<ToolResult, LensError> {
        let profile_name = self.extract_profile(&mut call)?;
        let pipeline = self.pipeline_for(&profile_name)?;
        let registry = self.core_tool_registry()?;
        let auth = LensAuthHook;
        let manifest = GazeMcpManifestAdapter::new(
            self.inner.manifest.clone(),
            &self.inner.snapshot_dir,
            self.inner.lens_session_id,
            &self.inner.gaze_session,
            self.inner.core_summaries.clone(),
        );
        let session_id_policy = gaze_mcp_core::SessionIdPolicy::default_strict();
        let envelope = gaze_mcp_core::PiiEnvelope::new(
            &registry,
            &auth,
            &manifest,
            &pipeline,
            &self.inner.gaze_session,
            &[],
            &session_id_policy,
        );
        let response = envelope
            .dispatch(
                &gaze_mcp_core::Principal::new("lens-agent"),
                &call.tool_name,
                call.args.0,
                None,
            )
            .await
            .map_err(dispatch_error_to_lens_error)?;
        let mut clean: CleanOutput =
            serde_json::from_value(response.payload).map_err(|err| LensError::Internal {
                detail: err.to_string(),
            })?;
        if matches!(call.tool_name.as_str(), "schema" | "list_tables") {
            clean.restore_gaze_tokens(&self.inner.gaze_session)?;
        }
        Ok(ToolResult {
            clean,
            snapshot_ref: SnapshotRef {
                path: self
                    .inner
                    .snapshot_dir
                    .join(format!("{}.snap", self.inner.lens_session_id)),
            },
        })
    }

    fn core_tool_registry(&self) -> Result<gaze_mcp_core::ToolRegistry, LensError> {
        let session = Arc::new(self.clone());
        let mut registry = gaze_mcp_core::ToolRegistry::new();
        registry
            .register(QueryTool::new(session.clone()))
            .map_err(tool_registry_error)?;
        registry
            .register(SchemaTool::new(session.clone()))
            .map_err(tool_registry_error)?;
        registry
            .register(ListTablesTool::new(session.clone()))
            .map_err(tool_registry_error)?;
        registry
            .register(LogTailTool::new(session.clone()))
            .map_err(tool_registry_error)?;
        registry
            .register(LogGrepTool::new(session))
            .map_err(tool_registry_error)?;
        Ok(registry)
    }

    pub(crate) async fn invoke_core_tool(
        &self,
        tool_name: &str,
        call_id: ulid::Ulid,
        args: serde_json::Value,
    ) -> Result<CleanOutput, LensError> {
        let mut call = ToolCall {
            call_id: call_id.to_string(),
            tool_name: tool_name.to_string(),
            args: ToolArgs(args),
        };
        let profile_name = self.extract_profile(&mut call)?;
        let pipeline = self.pipeline_for(&profile_name)?;
        let column_actions = self.column_actions_for(&profile_name)?;
        let raw_result = tokio::time::timeout(
            self.inner.caps.timeout,
            self.invoke_source(&call, &profile_name),
        )
        .await
        .map_err(|_| {
            operation_timeout("source dispatch", &call.tool_name, self.inner.caps.timeout)
        })??;
        let clean = tokio::time::timeout(self.inner.caps.timeout, async {
            self.redact_result(raw_result, &pipeline, &column_actions)
        })
        .await
        .map_err(|_| operation_timeout("redaction", &call.tool_name, self.inner.caps.timeout))??;
        self.inner
            .core_summaries
            .lock()
            .expect("core summary map lock")
            .insert(call.call_id.clone(), clean.summary());
        Ok(clean)
    }

    pub fn register_source(&self, name: impl Into<String>, source: Arc<dyn Source>) {
        self.inner
            .legacy_sources
            .lock()
            .expect("legacy source map lock")
            .insert(name.into(), source);
    }

    pub fn register_source_for_profile(
        &self,
        class: SourceClass,
        profile_name: impl Into<String>,
        source: Arc<dyn Source>,
    ) {
        self.inner.sources.lock().expect("source map lock").insert(
            (class, profile_name.into()),
            Arc::new(LazySource::ready(source)),
        );
    }

    pub fn register_source_lazy(
        &self,
        class: SourceClass,
        profile_name: impl Into<String>,
        builder: SourceBuilder,
    ) {
        self.inner.sources.lock().expect("source map lock").insert(
            (class, profile_name.into()),
            Arc::new(LazySource::new(builder)),
        );
    }

    pub fn register_fake_source(&self, name: &str, source: Box<dyn FakeSource>) {
        self.register_source(name, Arc::new(FakeSourceAdapter::new(source)));
    }

    #[doc(hidden)]
    pub fn register_fake_source_for_profile(
        &self,
        class: SourceClass,
        profile_name: &str,
        source: Box<dyn FakeSource>,
    ) {
        self.register_source_for_profile(
            class,
            profile_name,
            Arc::new(FakeSourceAdapter::new(source)),
        );
    }

    pub fn register_pipeline(
        &self,
        profile_name: impl Into<String>,
        pipeline: Arc<gaze::Pipeline>,
    ) -> Result<(), LensError> {
        let mut mode = self.inner.pipeline_mode.lock().expect("pipeline mode lock");
        match &mut *mode {
            PipelineMode::SingleProfile { .. } => Err(LensError::Internal {
                detail: "cannot register profile pipeline on single-profile session".to_string(),
            }),
            PipelineMode::MultiProfile(pipelines) => {
                pipelines.insert(profile_name.into(), pipeline);
                Ok(())
            }
        }
    }

    pub fn register_column_action_policy(
        &self,
        profile_name: impl Into<String>,
        policy: ColumnActionPolicy,
    ) -> Result<(), LensError> {
        let profile_name = profile_name.into();
        let mut mode = self
            .inner
            .column_action_mode
            .lock()
            .expect("column action mode lock");
        match &mut *mode {
            ColumnActionMode::SingleProfile {
                name,
                policy: target,
            } => {
                if *name != profile_name {
                    return Err(LensError::Internal {
                        detail: "cannot register different profile column policy on single-profile session".to_string(),
                    });
                }
                *target = policy;
                Ok(())
            }
            ColumnActionMode::MultiProfile(policies) => {
                policies.insert(profile_name, policy);
                Ok(())
            }
        }
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

    fn extract_profile(&self, call: &mut ToolCall) -> Result<String, LensError> {
        let mut mode = self.inner.pipeline_mode.lock().expect("pipeline mode lock");
        match &mut *mode {
            PipelineMode::SingleProfile { name, .. } => {
                if !call.args.0.is_object() {
                    return Err(LensError::Profile {
                        detail: "args must be a JSON object".to_string(),
                    });
                }
                let object = call.args.0.as_object_mut().expect("checked object");
                match object.get("profile").and_then(|value| value.as_str()) {
                    Some(profile) if profile == name => {}
                    Some(profile) => {
                        return Err(LensError::ProfileMismatch {
                            profile: profile.to_string(),
                            tool: call.tool_name.clone(),
                            required: name.clone(),
                            actual: "different".to_string(),
                        });
                    }
                    None => {
                        object.insert(
                            "profile".to_string(),
                            serde_json::Value::String(name.clone()),
                        );
                    }
                }
                Ok(name.clone())
            }
            PipelineMode::MultiProfile(pipelines) => {
                if !call.args.0.is_object() {
                    return Err(LensError::Profile {
                        detail: "args must be a JSON object".to_string(),
                    });
                }
                let profile = call
                    .args
                    .0
                    .get("profile")
                    .and_then(|value| value.as_str())
                    .ok_or_else(|| LensError::Profile {
                        detail: "profile required".to_string(),
                    })?;
                if profile.is_empty() {
                    return Err(LensError::Profile {
                        detail: "profile required".to_string(),
                    });
                }
                if pipelines.contains_key(profile) {
                    Ok(profile.to_string())
                } else {
                    let mut loaded = pipelines.keys().cloned().collect::<Vec<_>>();
                    loaded.sort();
                    Err(LensError::ProfileUnknown {
                        profile: profile.to_string(),
                        loaded,
                    })
                }
            }
        }
    }

    fn pipeline_for(&self, profile: &str) -> Result<Arc<gaze::Pipeline>, LensError> {
        let mode = self.inner.pipeline_mode.lock().expect("pipeline mode lock");
        match &*mode {
            PipelineMode::SingleProfile { name, pipeline } if name == profile => {
                Ok(pipeline.clone())
            }
            PipelineMode::SingleProfile { name, .. } => Err(LensError::ProfileMismatch {
                profile: profile.to_string(),
                tool: "unknown".to_string(),
                required: name.clone(),
                actual: "different".to_string(),
            }),
            PipelineMode::MultiProfile(pipelines) => {
                pipelines
                    .get(profile)
                    .cloned()
                    .ok_or_else(|| LensError::ProfileUnknown {
                        profile: profile.to_string(),
                        loaded: {
                            let mut loaded = pipelines.keys().cloned().collect::<Vec<_>>();
                            loaded.sort();
                            loaded
                        },
                    })
            }
        }
    }

    fn column_actions_for(&self, profile: &str) -> Result<ColumnActionPolicy, LensError> {
        let mode = self
            .inner
            .column_action_mode
            .lock()
            .expect("column action mode lock");
        match &*mode {
            ColumnActionMode::SingleProfile { name, policy } if name == profile => {
                Ok(policy.clone())
            }
            ColumnActionMode::SingleProfile { name, .. } => Err(LensError::ProfileMismatch {
                profile: profile.to_string(),
                tool: "unknown".to_string(),
                required: name.clone(),
                actual: "different".to_string(),
            }),
            ColumnActionMode::MultiProfile(policies) => {
                Ok(policies.get(profile).cloned().unwrap_or_default())
            }
        }
    }

    async fn invoke_source(
        &self,
        call: &ToolCall,
        profile_name: &str,
    ) -> Result<SourceOutput, LensError> {
        let legacy_source = {
            self.inner
                .legacy_sources
                .lock()
                .expect("legacy source map lock")
                .get(&call.tool_name)
                .cloned()
        };
        if let Some(source) = legacy_source {
            return source.dispatch(call).await;
        }
        let class = tool_kind(&call.tool_name).ok_or_else(|| LensError::SourceError {
            source_name: call.tool_name.clone(),
            detail: "unknown source".to_string(),
            sql: None,
            stderr: None,
        })?;
        let source = self
            .lookup_source(class, profile_name, &call.tool_name)?
            .get()
            .await?;
        source.dispatch(call).await
    }

    fn lookup_source(
        &self,
        class: SourceClass,
        profile_name: &str,
        tool_name: &str,
    ) -> Result<Arc<LazySource>, LensError> {
        let sources = self.inner.sources.lock().expect("source map lock");
        if let Some(source) = sources.get(&(class, profile_name.to_string())) {
            return Ok(source.clone());
        }
        let actual = sources
            .keys()
            .find(|(_, name)| name == profile_name)
            .map(|(actual, _)| *actual);
        match actual {
            Some(actual) => Err(LensError::ProfileMismatch {
                profile: profile_name.to_string(),
                tool: tool_name.to_string(),
                required: class.as_str().to_string(),
                actual: actual.as_str().to_string(),
            }),
            None => Err(LensError::ProfileUnknown {
                profile: profile_name.to_string(),
                loaded: {
                    let mut loaded = sources
                        .keys()
                        .map(|(_, name)| name.clone())
                        .collect::<Vec<_>>();
                    loaded.sort();
                    loaded.dedup();
                    loaded
                },
            }),
        }
    }

    fn redact_result(
        &self,
        output: SourceOutput,
        pipeline: &gaze::Pipeline,
        column_actions: &ColumnActionPolicy,
    ) -> Result<CleanOutput, LensError> {
        match output {
            SourceOutput::Rows(rows) => self.redact_rows(rows, pipeline, column_actions),
            SourceOutput::Text(text) => self.redact_text_output(text, Vec::new(), pipeline),
            SourceOutput::SchemaText(text) => self.schema_text_output(text, Vec::new()),
            SourceOutput::TextWithTruncation { text, truncated_at } => {
                self.redact_text_output(text, truncated_at, pipeline)
            }
        }
    }

    fn redact_rows(
        &self,
        rows: Vec<LensRow>,
        pipeline: &gaze::Pipeline,
        column_actions: &ColumnActionPolicy,
    ) -> Result<CleanOutput, LensError> {
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
                if let Some(column_action) = column_actions.action_for(key) {
                    apply_column_action(value, &self.inner.gaze_session, column_action)?;
                }
                if !matches!(
                    value,
                    LensValue::String(_) | LensValue::Json(serde_json::Value::String(_))
                ) {
                    value.redact_with(&self.inner.gaze_session, pipeline)?;
                }
                if let Some(lowered) = value.lower_for_redaction()? {
                    raw_fields.insert(key.clone(), lowered);
                }
            }
            let redacted = pipeline
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
                _ => {
                    return Err(LensError::RedactionFailed {
                        detail: "structured rows produced unsupported output".to_string(),
                    });
                }
            };
            let mut out = serde_json::Map::new();
            for (key, value) in redacted_row {
                if let Some(redacted) = redacted_fields.get(&key) {
                    let cell = gaze_value_to_json(redacted)?;
                    insert_capped_cell(
                        &mut out,
                        key,
                        cell,
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

    fn redact_text_output(
        &self,
        text: String,
        mut truncated_at: Vec<TruncatedAt>,
        pipeline: &gaze::Pipeline,
    ) -> Result<CleanOutput, LensError> {
        let (capped, truncated) = cap_string(text, self.inner.caps.bytes);
        if truncated {
            push_truncation(&mut truncated_at, TruncatedAt::Bytes);
        }
        let clean = pipeline
            .redact(&self.inner.gaze_session, RawDocument::Text(capped))
            .map_err(|err| LensError::RedactionFailed {
                detail: err.to_string(),
            })?;
        match clean {
            CleanDocument::Text(text) => Ok(CleanOutput::Text { text, truncated_at }),
            CleanDocument::Structured(_) => Err(LensError::RedactionFailed {
                detail: "text output produced structured output".to_string(),
            }),
            _ => Err(LensError::RedactionFailed {
                detail: "text output produced unsupported output".to_string(),
            }),
        }
    }

    fn schema_text_output(
        &self,
        text: String,
        mut truncated_at: Vec<TruncatedAt>,
    ) -> Result<CleanOutput, LensError> {
        let (text, truncated) = cap_string(text, self.inner.caps.bytes);
        if truncated {
            push_truncation(&mut truncated_at, TruncatedAt::Bytes);
        }
        Ok(CleanOutput::Text { text, truncated_at })
    }

    #[doc(hidden)]
    pub fn new_with_manifest_for_tests(
        policy: &gaze::Policy,
        manifest: Arc<dyn LensManifestStore>,
        snapshot_dir: &Path,
        caps: OutputCaps,
    ) -> Result<Self, LensError> {
        let lens_session_id = ulid::Ulid::new();
        let gaze_session = Self::build_gaze_session(&lens_session_id, policy)?;
        Ok(Self::from_parts(
            lens_session_id,
            gaze_session,
            PipelineMode::SingleProfile {
                name: "test".to_string(),
                pipeline: Arc::new(default_pipeline()?),
            },
            ColumnActionMode::SingleProfile {
                name: "test".to_string(),
                policy: ColumnActionPolicy::default(),
            },
            manifest,
            snapshot_dir.to_path_buf(),
            caps,
        ))
    }
}

impl CleanOutput {
    fn restore_gaze_tokens(&mut self, session: &gaze::Session) -> Result<(), LensError> {
        match self {
            CleanOutput::Rows { rows, .. } => {
                for row in rows {
                    for value in row.values_mut() {
                        restore_gaze_tokens_in_value(session, value)?;
                    }
                }
            }
            CleanOutput::Text { text, .. } => {
                *text = restore_gaze_tokens_in_string(session, text)?;
            }
        }
        Ok(())
    }

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

fn restore_gaze_tokens_in_value(
    session: &gaze::Session,
    value: &mut serde_json::Value,
) -> Result<(), LensError> {
    match value {
        serde_json::Value::String(text) => {
            *text = restore_gaze_tokens_in_string(session, text)?;
        }
        serde_json::Value::Array(values) => {
            for value in values {
                restore_gaze_tokens_in_value(session, value)?;
            }
        }
        serde_json::Value::Object(values) => {
            for value in values.values_mut() {
                restore_gaze_tokens_in_value(session, value)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn restore_gaze_tokens_in_string(session: &gaze::Session, text: &str) -> Result<String, LensError> {
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0usize;
    for token in gaze::token_shape::pattern().find_iter(text) {
        if !session.contains_token(token.as_str()) {
            continue;
        }
        out.push_str(&text[cursor..token.start()]);
        out.push_str(&session.restore_strict(token.as_str()).map_err(|err| {
            LensError::RedactionFailed {
                detail: err.to_string(),
            }
        })?);
        cursor = token.end();
    }
    out.push_str(&text[cursor..]);
    Ok(out)
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

fn apply_column_action(
    value: &mut LensValue,
    gaze_session: &gaze::Session,
    column_action: &ColumnAction,
) -> Result<(), LensError> {
    match value {
        LensValue::String(text) => {
            *text = apply_action_to_text(text, gaze_session, column_action)?;
        }
        LensValue::Json(json) => {
            apply_column_action_to_json(json, gaze_session, column_action)?;
        }
        _ => {
            value.lower_for_redaction()?;
        }
    }
    Ok(())
}

fn apply_column_action_to_json(
    value: &mut serde_json::Value,
    gaze_session: &gaze::Session,
    column_action: &ColumnAction,
) -> Result<(), LensError> {
    match value {
        serde_json::Value::String(text) => {
            *text = apply_action_to_text(text, gaze_session, column_action)?;
        }
        serde_json::Value::Array(values) => {
            for value in values {
                apply_column_action_to_json(value, gaze_session, column_action)?;
            }
        }
        serde_json::Value::Object(values) => {
            for value in values.values_mut() {
                apply_column_action_to_json(value, gaze_session, column_action)?;
            }
        }
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {}
    }
    Ok(())
}

fn apply_action_to_text(
    text: &str,
    gaze_session: &gaze::Session,
    column_action: &ColumnAction,
) -> Result<String, LensError> {
    Ok(match column_action.action {
        Action::Tokenize => gaze_session
            .tokenize(&column_action.class, text)
            .map_err(|err| LensError::RedactionFailed {
                detail: err.to_string(),
            })?,
        Action::Redact => "[REDACTED]".to_string(),
        Action::FormatPreserve => gaze_session
            .format_preserving_fake(&column_action.class, text)
            .map_err(|err| LensError::RedactionFailed {
                detail: err.to_string(),
            })?,
        Action::Generalize => generalize_column_value(&column_action.class),
        Action::Preserve => text.to_string(),
        _ => {
            return Err(LensError::RedactionFailed {
                detail: "unsupported column action".to_string(),
            });
        }
    })
}

fn generalize_column_value(class: &gaze::PiiClass) -> String {
    match class {
        gaze::PiiClass::Email => "[EMAIL]".to_string(),
        gaze::PiiClass::Name => "[NAME]".to_string(),
        gaze::PiiClass::Location => "[LOCATION]".to_string(),
        gaze::PiiClass::Organization => "[ORGANIZATION]".to_string(),
        gaze::PiiClass::Custom(name) => format!("[{}]", name.to_ascii_uppercase()),
    }
}

fn dispatch_error_to_lens_error(err: gaze_mcp_core::DispatchError) -> LensError {
    match err {
        gaze_mcp_core::DispatchError::ToolError(tool_err) => tool_error_to_lens_error(tool_err),
        gaze_mcp_core::DispatchError::Redaction(detail) => LensError::RedactionFailed { detail },
        gaze_mcp_core::DispatchError::UnknownTool(tool) => LensError::SourceError {
            source_name: tool,
            detail: "unknown source".to_string(),
            sql: None,
            stderr: None,
        },
        gaze_mcp_core::DispatchError::SessionId(err) => LensError::Profile {
            detail: err.to_string(),
        },
        gaze_mcp_core::DispatchError::Auth(err) => LensError::Internal {
            detail: err.to_string(),
        },
        gaze_mcp_core::DispatchError::Manifest(err) => manifest_error_to_lens_error(err),
        gaze_mcp_core::DispatchError::ResponseSerialization(err) => LensError::Internal {
            detail: err.to_string(),
        },
        _ => LensError::Internal {
            detail: "unknown gaze-mcp-core dispatch error".to_string(),
        },
    }
}

fn tool_error_to_lens_error(err: gaze_mcp_core::ToolError) -> LensError {
    match err {
        gaze_mcp_core::ToolError::InvalidArgs(detail)
        | gaze_mcp_core::ToolError::NotFound(detail) => LensError::Profile { detail },
        gaze_mcp_core::ToolError::Internal(err) => boxed_error_to_lens_error(err),
        _ => LensError::Internal {
            detail: "unknown gaze-mcp-core tool error".to_string(),
        },
    }
}

fn manifest_error_to_lens_error(err: gaze_mcp_core::ManifestError) -> LensError {
    match err {
        gaze_mcp_core::ManifestError::Backend(err) => boxed_error_to_lens_error(err),
        other => LensError::Internal {
            detail: other.to_string(),
        },
    }
}

fn boxed_error_to_lens_error(err: Box<dyn std::error::Error + Send + Sync>) -> LensError {
    match err.downcast::<LensError>() {
        Ok(err) => *err,
        Err(err) => LensError::Internal {
            detail: err.to_string(),
        },
    }
}

fn tool_registry_error(err: gaze_mcp_core::ToolRegistryError) -> LensError {
    LensError::Internal {
        detail: err.to_string(),
    }
}

fn operation_timeout(phase: &str, operation: &str, timeout: Duration) -> LensError {
    LensError::OperationTimeout {
        phase: phase.to_string(),
        operation: operation.to_string(),
        timeout_secs: timeout.as_secs(),
        context: None,
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

pub(crate) fn persist_snapshot(
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
