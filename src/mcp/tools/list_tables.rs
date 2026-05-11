use std::sync::Arc;

use async_trait::async_trait;
use gaze_mcp_core::{Tool, ToolCtx, ToolDescriptor, ToolError, ToolResponse};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::session::Session;

use super::{invoke_session_tool, schema_for};

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct ListTablesArgs {
    #[schemars(
        description = "Configured profile name selecting the source to dispatch. Required. Pattern: ^[a-z0-9][a-z0-9_-]{0,63}$.",
        regex(pattern = r"^[a-z0-9][a-z0-9_-]{0,63}$")
    )]
    pub profile: String,
}

pub struct ListTablesTool {
    session: Arc<Session>,
    descriptor: ToolDescriptor,
}

impl ListTablesTool {
    pub fn new(session: Arc<Session>) -> Self {
        Self {
            session,
            descriptor: ToolDescriptor::agent("list_tables", schema_for::<ListTablesArgs>())
                .with_description("List table names. Names are raw by default; schema_tokenize = true tokenizes presentation output, schema_allowlist only keeps selected names raw for presentation, and profile edits require restarting/reloading the MCP server."),
        }
    }
}

#[async_trait]
impl Tool for ListTablesTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    async fn invoke(&self, ctx: &ToolCtx<'_>) -> Result<ToolResponse, ToolError> {
        invoke_session_tool(&self.session, "list_tables", ctx).await
    }
}
