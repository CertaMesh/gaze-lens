use std::sync::Arc;

use gaze_mcp_core::{ToolError, ToolResponse};

use crate::errors::{LensError, sanitize_error};
use crate::session::{CleanOutput, Session};

pub mod list_tables;
pub mod log_grep;
pub mod log_tail;
pub mod query;
pub mod schema;

async fn invoke_session_tool(
    session: &Arc<Session>,
    tool_name: &str,
    ctx: &gaze_mcp_core::ToolCtx<'_>,
) -> Result<ToolResponse, ToolError> {
    let clean = session
        .invoke_core_tool(tool_name, ctx.call_id(), ctx.redacted_args().clone())
        .await
        .map_err(lens_error_to_tool_error)?;
    clean_output_response(clean)
}

fn clean_output_response(clean: CleanOutput) -> Result<ToolResponse, ToolError> {
    serde_json::to_value(clean)
        .map(ToolResponse::json)
        .map_err(|err| {
            ToolError::internal(LensError::Internal {
                detail: err.to_string(),
            })
        })
}

fn lens_error_to_tool_error(err: LensError) -> ToolError {
    if err.is_invalid_params() {
        ToolError::InvalidArgs(sanitize_error(&err))
    } else {
        ToolError::internal(err)
    }
}

fn schema_for<T: schemars::JsonSchema>() -> serde_json::Value {
    serde_json::to_value(schemars::schema_for!(T)).expect("schema serializes")
}
