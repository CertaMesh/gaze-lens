use std::sync::Arc;

use async_trait::async_trait;
use gaze_mcp_core::{Tool, ToolCtx, ToolDescriptor, ToolError, ToolResponse};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::session::Session;

use super::{invoke_session_tool, schema_for};

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct SchemaArgs {
    #[schemars(
        description = "Configured profile name selecting the source to dispatch. Required. Pattern: ^[a-z0-9][a-z0-9_-]{0,63}$.",
        regex(pattern = r"^[a-z0-9][a-z0-9_-]{0,63}$")
    )]
    pub profile: String,
    #[schemars(
        description = "Raw configured table name to inspect. Even when schema_tokenize = true changes presentation output, requests still use raw table names from the profile policy."
    )]
    pub table: String,
}

pub struct SchemaTool {
    session: Arc<Session>,
    descriptor: ToolDescriptor,
}

impl SchemaTool {
    pub fn new(session: Arc<Session>) -> Self {
        Self {
            session,
            descriptor: ToolDescriptor::agent("schema", schema_for::<SchemaArgs>())
                .with_description("Describe one raw configured table schema. Table/column labels are raw by default; schema_tokenize = true tokenizes presentation output, schema_allowlist only keeps selected labels raw for presentation, and profile edits require restarting/reloading the MCP server."),
        }
    }
}

#[async_trait]
impl Tool for SchemaTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    async fn invoke(&self, ctx: &ToolCtx<'_>) -> Result<ToolResponse, ToolError> {
        invoke_session_tool(&self.session, "schema", ctx).await
    }
}
