use std::sync::Arc;

use async_trait::async_trait;
use gaze_mcp_core::{Tool, ToolCtx, ToolDescriptor, ToolError, ToolResponse};

use crate::session::Session;
use crate::source::db::query::CannedQuery;

use super::{invoke_session_tool, schema_for};

pub struct QueryTool {
    session: Arc<Session>,
    descriptor: ToolDescriptor,
}

impl QueryTool {
    pub fn new(session: Arc<Session>) -> Self {
        Self {
            session,
            descriptor: ToolDescriptor::agent("query", schema_for::<CannedQuery>())
                .with_description("Run a canned structured DB query."),
        }
    }
}

#[async_trait]
impl Tool for QueryTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    async fn invoke(&self, ctx: &ToolCtx<'_>) -> Result<ToolResponse, ToolError> {
        invoke_session_tool(&self.session, "query", ctx).await
    }
}
