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
    let args = restore_tokens_in_json(ctx.resources().session(), ctx.redacted_args())
        .map_err(ToolError::internal)?;
    let clean = session
        .invoke_core_tool(tool_name, ctx.call_id(), args)
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

fn restore_tokens_in_json(
    session: &gaze::Session,
    value: &serde_json::Value,
) -> Result<serde_json::Value, LensError> {
    match value {
        serde_json::Value::String(text) => Ok(serde_json::Value::String(restore_tokens_in_string(
            session, text,
        )?)),
        serde_json::Value::Array(values) => values
            .iter()
            .map(|value| restore_tokens_in_json(session, value))
            .collect::<Result<Vec<_>, _>>()
            .map(serde_json::Value::Array),
        serde_json::Value::Object(values) => {
            let mut out = serde_json::Map::new();
            for (key, value) in values {
                out.insert(key.clone(), restore_tokens_in_json(session, value)?);
            }
            Ok(serde_json::Value::Object(out))
        }
        other => Ok(other.clone()),
    }
}

fn restore_tokens_in_string(session: &gaze::Session, text: &str) -> Result<String, LensError> {
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
