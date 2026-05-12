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
        invoke_session_tool(&self.session, "log_grep", ctx).await
    }
}
