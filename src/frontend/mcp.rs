use std::sync::Arc;

use async_trait::async_trait;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::tool::Parameters;
use rmcp::model::{CallToolResult, Content, ErrorData};
use rmcp::transport::stdio;
use rmcp::{ServerHandler, ServiceExt, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::errors::{LensError, sanitize_error};
use crate::session::{Session, ToolCall};
use crate::source::ToolArgs;
use crate::source::db::query::CannedQuery;

use super::{Frontend, FrontendError, ShutdownToken};

const PUBLIC_TOOLS: [&str; 5] = ["query", "schema", "list_tables", "log_tail", "log_grep"];

#[derive(Clone)]
pub struct McpFrontend {
    session: Option<Arc<Session>>,
    tool_router: ToolRouter<Self>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct SchemaArgs {
    pub table: String,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct LogTailArgs {
    #[serde(default)]
    pub lines: Option<u32>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct LogGrepArgs {
    pub pattern: String,
    #[serde(default)]
    pub limit: Option<u32>,
}

#[tool_router(router = tool_router)]
impl McpFrontend {
    pub fn new() -> Self {
        Self {
            session: None,
            tool_router: Self::tool_router(),
        }
    }

    pub fn with_session(session: Arc<Session>) -> Self {
        Self {
            session: Some(session),
            tool_router: Self::tool_router(),
        }
    }

    pub fn public_tool_names() -> Vec<&'static str> {
        PUBLIC_TOOLS.to_vec()
    }

    pub async fn call_tool_json(
        &self,
        tool_name: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let result = self
            .dispatch(tool_name, args)
            .await
            .map_err(|err| sanitize_error(&err))?;
        serde_json::to_value(result).map_err(|err| err.to_string())
    }

    #[tool(name = "query", description = "Run a canned structured DB query.")]
    async fn query(
        &self,
        Parameters(args): Parameters<CannedQuery>,
    ) -> Result<CallToolResult, ErrorData> {
        self.to_call_tool_result("query", serde_json::to_value(args))
            .await
    }

    #[tool(name = "schema", description = "Describe one tokenized table schema.")]
    async fn schema(
        &self,
        Parameters(args): Parameters<SchemaArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.to_call_tool_result("schema", serde_json::to_value(args))
            .await
    }

    #[tool(name = "list_tables", description = "List tokenized table names.")]
    async fn list_tables(&self) -> Result<CallToolResult, ErrorData> {
        self.to_call_tool_result("list_tables", Ok(serde_json::json!({})))
            .await
    }

    #[tool(name = "log_tail", description = "Deferred until PR2b.")]
    async fn log_tail(
        &self,
        Parameters(args): Parameters<LogTailArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.to_call_tool_result("log_tail", serde_json::to_value(args))
            .await
    }

    #[tool(name = "log_grep", description = "Deferred until PR2b.")]
    async fn log_grep(
        &self,
        Parameters(args): Parameters<LogGrepArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.to_call_tool_result("log_grep", serde_json::to_value(args))
            .await
    }

    async fn to_call_tool_result(
        &self,
        tool_name: &str,
        args: Result<serde_json::Value, serde_json::Error>,
    ) -> Result<CallToolResult, ErrorData> {
        let args = args.map_err(|err| ErrorData::internal_error(err.to_string(), None))?;
        match self.dispatch(tool_name, args).await {
            Ok(result) => serde_json::to_string(&result)
                .map(|json| CallToolResult::success(vec![Content::text(json)]))
                .map_err(|err| ErrorData::internal_error(err.to_string(), None)),
            Err(err) => Err(ErrorData::internal_error(sanitize_error(&err), None)),
        }
    }

    async fn dispatch(
        &self,
        tool_name: &str,
        args: serde_json::Value,
    ) -> Result<crate::session::ToolResult, LensError> {
        if !PUBLIC_TOOLS.contains(&tool_name) {
            return Err(LensError::FrontendError {
                frontend: "mcp".to_string(),
                detail: format!("unknown tool `{tool_name}`"),
            });
        }
        let session = self
            .session
            .as_ref()
            .ok_or_else(|| LensError::FrontendError {
                frontend: "mcp".to_string(),
                detail: "session not attached".to_string(),
            })?;
        session
            .dispatch_tool(ToolCall {
                call_id: ulid::Ulid::new().to_string(),
                tool_name: tool_name.to_string(),
                args: ToolArgs(args),
            })
            .await
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for McpFrontend {}

impl Default for McpFrontend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Frontend for McpFrontend {
    async fn serve(
        mut self,
        session: Arc<Session>,
        _shutdown: ShutdownToken,
    ) -> Result<(), FrontendError> {
        self.session = Some(session);
        let running = ServiceExt::serve(self, stdio())
            .await
            .map_err(|err| FrontendError::Mcp(err.to_string()))?;
        running
            .waiting()
            .await
            .map_err(|err| FrontendError::Mcp(err.to_string()))?;
        Ok(())
    }
}
