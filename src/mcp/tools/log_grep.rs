use std::sync::Arc;

use async_trait::async_trait;
use gaze_mcp_core::{Tool, ToolCtx, ToolDescriptor, ToolError, ToolResponse};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::session::Session;

use super::{invoke_session_tool, schema_for};

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct LogGrepArgs {
    #[schemars(
        description = "Configured profile name selecting the source to dispatch. Required. Pattern: ^[a-z0-9][a-z0-9_-]{0,63}$.",
        regex(pattern = r"^[a-z0-9][a-z0-9_-]{0,63}$")
    )]
    pub profile: String,
    pub pattern: String,
    #[serde(default)]
    pub level: Option<String>,
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub refresh: Option<bool>,
}

pub struct LogGrepTool {
    session: Arc<Session>,
    descriptor: ToolDescriptor,
}

impl LogGrepTool {
    pub fn new(session: Arc<Session>) -> Self {
        Self {
            session,
            descriptor: ToolDescriptor::agent("log_grep", schema_for::<LogGrepArgs>())
                .with_description("Search a configured SSH log source."),
        }
    }
}

#[async_trait]
impl Tool for LogGrepTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    async fn invoke(&self, ctx: &ToolCtx<'_>) -> Result<ToolResponse, ToolError> {
        log_grep_mode(ctx.redacted_args())?;
        invoke_session_tool(&self.session, "log_grep", ctx).await
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LogGrepMode {
    Regex,
    Keyword,
}

fn log_grep_mode(args: &serde_json::Value) -> Result<LogGrepMode, ToolError> {
    match args.get("mode") {
        None | Some(serde_json::Value::Null) => Ok(LogGrepMode::Regex),
        Some(serde_json::Value::String(mode)) if mode == "regex" => Ok(LogGrepMode::Regex),
        Some(serde_json::Value::String(mode)) if mode == "keyword" => Ok(LogGrepMode::Keyword),
        Some(serde_json::Value::String(mode)) => Err(ToolError::InvalidArgs(format!(
            "invalid log_grep mode `{mode}`; expected `regex` or `keyword`"
        ))),
        Some(_) => Err(ToolError::InvalidArgs(
            "invalid log_grep mode; expected `regex` or `keyword`".to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use gaze_mcp_core::ToolError;
    use serde_json::json;

    use super::*;

    #[test]
    fn log_grep_mode_defaults_to_regex() {
        assert_eq!(
            log_grep_mode(&json!({"pattern": "ERROR"})).expect("mode"),
            LogGrepMode::Regex
        );
    }

    #[test]
    fn log_grep_mode_accepts_explicit_regex() {
        assert_eq!(
            log_grep_mode(&json!({"pattern": "ERROR", "mode": "regex"})).expect("mode"),
            LogGrepMode::Regex
        );
    }

    #[test]
    fn log_grep_mode_rejects_unknown_modes() {
        let err = log_grep_mode(&json!({"pattern": "ERROR", "mode": "substring"}))
            .expect_err("unknown mode");

        assert!(matches!(
            err,
            ToolError::InvalidArgs(message)
                if message.contains("invalid log_grep mode")
                    && message.contains("regex")
                    && message.contains("keyword")
        ));
    }
}
