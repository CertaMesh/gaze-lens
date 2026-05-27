use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use gaze_mcp_core::manifest::{
    BeginCallContext, CallHandle, FailureReason, ManifestError, SnapshotRef as CoreSnapshotRef,
};

use crate::errors::LensError;
use crate::session::manifest::LensManifestStore;
use crate::session::{RedactedToolArgs, ResultSummary, ToolCall, TruncatedAt, persist_snapshot};
use crate::source::ToolArgs;

pub struct GazeMcpManifestAdapter<'a> {
    inner: Arc<dyn LensManifestStore>,
    snapshot_dir: PathBuf,
    lens_session_id: ulid::Ulid,
    summaries: Arc<Mutex<HashMap<String, ResultSummary>>>,
    gaze_session: &'a gaze::Session,
}

impl<'a> GazeMcpManifestAdapter<'a> {
    pub fn new(
        inner: Arc<dyn LensManifestStore>,
        snapshot_dir: impl AsRef<Path>,
        lens_session_id: ulid::Ulid,
        gaze_session: &'a gaze::Session,
        summaries: Arc<Mutex<HashMap<String, ResultSummary>>>,
    ) -> Self {
        Self {
            inner,
            snapshot_dir: snapshot_dir.as_ref().to_path_buf(),
            lens_session_id,
            summaries,
            gaze_session,
        }
    }
}

#[async_trait]
impl gaze_mcp_core::ManifestStore for GazeMcpManifestAdapter<'_> {
    async fn begin_call(&self, ctx: BeginCallContext<'_>) -> Result<CallHandle, ManifestError> {
        let call = ToolCall {
            call_id: ctx.call_id.to_string(),
            tool_name: ctx.tool_name.to_string(),
            args: ToolArgs(ctx.redacted_args.clone()),
        };
        let redacted_args = RedactedToolArgs {
            json: serde_json::to_string(ctx.redacted_args)
                .map_err(|err| ManifestError::backend(lens_internal(err.to_string())))?,
        };
        self.inner
            .begin_call(&call, &redacted_args)
            .map_err(ManifestError::backend)?;
        Ok(CallHandle::new(ctx.call_id))
    }

    async fn finish_call(
        &self,
        handle: CallHandle,
        snapshot: CoreSnapshotRef,
    ) -> Result<(), ManifestError> {
        let snapshot_ref =
            persist_snapshot(&self.snapshot_dir, self.lens_session_id, self.gaze_session)
                .map_err(ManifestError::backend)?;
        let summary = self
            .summaries
            .lock()
            .expect("core summary map lock")
            .remove(&handle.id().to_string())
            .unwrap_or(ResultSummary {
                rows: 0,
                bytes: snapshot.byte_len,
                truncated_at: Vec::new(),
            });
        self.inner
            .finish_call(&handle.id().to_string(), &summary, &snapshot_ref)
            .map_err(ManifestError::backend)
    }

    async fn fail_call(
        &self,
        handle: CallHandle,
        reason: FailureReason,
    ) -> Result<(), ManifestError> {
        let err = lens_error_from_failure(reason);
        self.summaries
            .lock()
            .expect("core summary map lock")
            .remove(&handle.id().to_string());
        self.inner
            .fail_call(&handle.id().to_string(), &err)
            .map_err(ManifestError::backend)
    }
}

fn lens_error_from_failure(reason: FailureReason) -> LensError {
    match reason {
        FailureReason::ToolError { class: _, message }
            if is_operation_timeout_message(&message) =>
        {
            operation_timeout_from_message(&message)
                .unwrap_or(LensError::Truncated(TruncatedAt::Timeout))
        }
        FailureReason::ToolError { class, message } => lens_internal(format!("{class}: {message}")),
        FailureReason::AuthDenied { reason } => lens_internal(format!("auth denied: {reason}")),
        FailureReason::RedactionFailed { message } => {
            LensError::RedactionFailed { detail: message }
        }
        FailureReason::Other { message } => lens_internal(message),
        _ => lens_internal("unknown gaze-mcp-core failure".to_string()),
    }
}

fn is_timeout_message(message: &str) -> bool {
    message.contains("output truncated at Timeout")
}

fn is_operation_timeout_message(message: &str) -> bool {
    is_timeout_message(message) || message.contains("timeout during ")
}

fn operation_timeout_from_message(message: &str) -> Option<LensError> {
    let start = message.find("timeout during ")?;
    let timeout = &message[start + "timeout during ".len()..];
    let (phase, rest) = timeout.split_once(" for ")?;
    let (operation, rest) = rest.split_once(" after ")?;
    let (timeout_secs, context) = rest.split_once('s')?;
    let timeout_secs = timeout_secs.parse::<u64>().ok()?;
    let context = context
        .strip_prefix(" (")
        .and_then(|context| context.strip_suffix(')'))
        .map(ToString::to_string);
    Some(LensError::OperationTimeout {
        phase: phase.to_string(),
        operation: operation.to_string(),
        timeout_secs,
        context,
    })
}

fn lens_internal(detail: String) -> LensError {
    LensError::Internal { detail }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use gaze_mcp_core::{
        AuthError, AuthHook, PiiEnvelope, Principal, SessionIdPolicy, Tool, ToolCtx,
        ToolDescriptor, ToolRegistry, ToolResponse,
    };
    use rusqlite::Connection;
    use serde_json::json;

    use super::*;
    use crate::session::manifest::ManifestWriter;

    #[tokio::test]
    async fn adapter_round_trips_begin_finish_to_lens_manifest() {
        let temp = tempfile::tempdir().expect("tempdir");
        let manifest_path = temp.path().join("manifest.sqlite");
        let snapshot_dir = temp.path().join("snapshots");
        let lens_session_id = ulid::Ulid::new();
        let gaze_session =
            gaze::Session::new(gaze::Scope::Conversation(lens_session_id.to_string()))
                .expect("gaze session");
        let writer = ManifestWriter::new(
            &manifest_path,
            lens_session_id,
            gaze_session.audit_session_id(),
        )
        .expect("manifest writer");
        let adapter = GazeMcpManifestAdapter::new(
            Arc::new(writer),
            &snapshot_dir,
            lens_session_id,
            &gaze_session,
            Arc::new(Mutex::new(HashMap::new())),
        );
        let pipeline = gaze::Pipeline::builder()
            .rule(gaze::DefaultRule::new(gaze::Action::Preserve))
            .build()
            .expect("pipeline");
        let mut registry = ToolRegistry::new();
        registry.register(EchoTool::new()).expect("register");
        let auth = AllowAuth;
        let session_id_policy = SessionIdPolicy::default_strict();
        let envelope = PiiEnvelope::new(
            &registry,
            &auth,
            &adapter,
            &pipeline,
            &gaze_session,
            &[],
            &session_id_policy,
        );
        let response = envelope
            .dispatch(
                &Principal::new("lens-agent"),
                "query",
                json!({"profile": "dev", "email": "<hex:Email_1>"}),
                None,
            )
            .await
            .expect("dispatch");

        let conn = Connection::open(&manifest_path).expect("open manifest");
        let (call_id, status, tool_name, snapshot_ref): (String, String, String, String) = conn
            .query_row(
                "SELECT call_id, status, tool_name, snapshot_ref FROM calls",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("row");

        assert!(!call_id.is_empty());
        assert_eq!(status, "ok");
        assert_eq!(tool_name, "query");
        assert_eq!(response.payload["ok"], true);
        assert!(snapshot_ref.ends_with(&format!("{lens_session_id}.snap")));
        assert!(std::path::Path::new(&snapshot_ref).exists());
    }

    #[test]
    fn operation_timeout_message_preserves_phase_context() {
        let err = operation_timeout_from_message(
            "timeout during ssh connect for log_tail after 10s (profile=prod host=app path=/var/log/app.log)",
        )
        .expect("operation timeout");

        assert_eq!(
            crate::errors::sanitize_error(&err),
            "Timeout: phase=ssh connect operation=log_tail timeout_secs=10 context=profile=prod host=app path=/var/log/app.log"
        );
    }

    #[test]
    fn legacy_timeout_message_still_maps_to_truncated_timeout() {
        let err = lens_error_from_failure(FailureReason::ToolError {
            class: "internal".to_string(),
            message: "output truncated at Timeout".to_string(),
        });

        assert!(matches!(err, LensError::Truncated(TruncatedAt::Timeout)));
    }

    struct EchoTool {
        descriptor: ToolDescriptor,
    }

    impl EchoTool {
        fn new() -> Self {
            Self {
                descriptor: ToolDescriptor::agent("query", json!({"type": "object"})),
            }
        }
    }

    #[async_trait]
    impl Tool for EchoTool {
        fn descriptor(&self) -> &ToolDescriptor {
            &self.descriptor
        }

        async fn invoke(
            &self,
            _ctx: &ToolCtx<'_>,
        ) -> Result<ToolResponse, gaze_mcp_core::ToolError> {
            Ok(ToolResponse::json(json!({"ok": true})))
        }
    }

    struct AllowAuth;

    #[async_trait]
    impl AuthHook for AllowAuth {
        async fn authorize_agent(
            &self,
            _principal: &Principal,
            _tool_name: &str,
        ) -> Result<(), AuthError> {
            Ok(())
        }

        async fn authorize_operator(
            &self,
            _principal: &Principal,
            _tool_name: &str,
        ) -> Result<(), AuthError> {
            Ok(())
        }
    }
}
