use async_trait::async_trait;

use crate::errors::LensError;
use crate::session::{ToolCall, TruncatedAt};
use crate::source::db::schema::SchemaTokenizer;
use crate::source::db::{DbSource, query::CannedQuery};
use crate::value::LensRow;

pub mod db;
pub mod log;
pub mod ssh_tunnel;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ToolArgs(pub serde_json::Value);

#[derive(Debug, Clone, PartialEq)]
pub enum SourceOutput {
    Rows(Vec<LensRow>),
    Text(String),
    TextWithTruncation {
        text: String,
        truncated_at: Vec<TruncatedAt>,
    },
}

#[async_trait]
pub trait FakeSource: Send + Sync {
    async fn invoke(&self, args: &ToolArgs) -> Result<SourceOutput, LensError>;
}

#[async_trait]
pub trait Source: Send + Sync {
    async fn dispatch(&self, call: &ToolCall) -> Result<SourceOutput, LensError>;
}

pub struct FakeSourceAdapter {
    inner: Box<dyn FakeSource>,
}

impl FakeSourceAdapter {
    pub fn new(inner: Box<dyn FakeSource>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl Source for FakeSourceAdapter {
    async fn dispatch(&self, call: &ToolCall) -> Result<SourceOutput, LensError> {
        self.inner.invoke(&call.args).await
    }
}

pub struct DbSourceWrapper {
    inner: std::sync::Arc<dyn DbSource>,
    schema_tokenizer: SchemaTokenizer,
    schema_allowlist: Option<Vec<String>>,
}

impl DbSourceWrapper {
    pub fn new(inner: std::sync::Arc<dyn DbSource>) -> Self {
        Self {
            inner,
            schema_tokenizer: SchemaTokenizer::default(),
            schema_allowlist: None,
        }
    }

    pub fn with_schema_allowlist(
        inner: std::sync::Arc<dyn DbSource>,
        schema_allowlist: Option<Vec<String>>,
    ) -> Self {
        Self {
            inner,
            schema_tokenizer: SchemaTokenizer::default(),
            schema_allowlist,
        }
    }
}

#[async_trait]
impl Source for DbSourceWrapper {
    async fn dispatch(&self, call: &ToolCall) -> Result<SourceOutput, LensError> {
        match call.tool_name.as_str() {
            "query" => {
                let query: CannedQuery =
                    serde_json::from_value(call.args.0.clone()).map_err(|err| {
                        LensError::SourceError {
                            source_name: call.tool_name.clone(),
                            detail: err.to_string(),
                            sql: None,
                            stderr: None,
                        }
                    })?;
                self.inner.query(&query).await.map(SourceOutput::Rows)
            }
            "schema" => {
                let args: SchemaArgs =
                    serde_json::from_value(call.args.0.clone()).map_err(|err| {
                        LensError::SourceError {
                            source_name: call.tool_name.clone(),
                            detail: err.to_string(),
                            sql: None,
                            stderr: None,
                        }
                    })?;
                let schema = self.inner.schema(&args.table).await?;
                let tokenized = self
                    .schema_tokenizer
                    .tokenize_table_schema(&schema, self.schema_allowlist.as_deref());
                serde_json::to_string(&tokenized)
                    .map(SourceOutput::Text)
                    .map_err(|err| {
                        LensError::ConvertError(crate::value::LowerError::Decode {
                            kind: "json",
                            detail: err.to_string(),
                        })
                    })
            }
            "list_tables" => {
                let tables = self.inner.list_tables().await?;
                let tokenized = self
                    .schema_tokenizer
                    .tokenize_table_names(&tables, self.schema_allowlist.as_deref());
                serde_json::to_string(&tokenized)
                    .map(SourceOutput::Text)
                    .map_err(|err| {
                        LensError::ConvertError(crate::value::LowerError::Decode {
                            kind: "json",
                            detail: err.to_string(),
                        })
                    })
            }
            other => Err(LensError::SourceError {
                source_name: other.to_string(),
                detail: "unsupported db source tool".to_string(),
                sql: None,
                stderr: None,
            }),
        }
    }
}

#[derive(serde::Deserialize)]
struct SchemaArgs {
    table: String,
}

#[derive(Debug, Clone)]
pub struct InMemoryFakeSource {
    output: SourceOutput,
}

impl InMemoryFakeSource {
    pub fn rows(rows: Vec<LensRow>) -> Self {
        Self {
            output: SourceOutput::Rows(rows),
        }
    }

    pub fn text(text: impl Into<String>) -> Self {
        Self {
            output: SourceOutput::Text(text.into()),
        }
    }
}

#[async_trait]
impl FakeSource for InMemoryFakeSource {
    async fn invoke(&self, _args: &ToolArgs) -> Result<SourceOutput, LensError> {
        Ok(self.output.clone())
    }
}

#[cfg(test)]
pub mod test_support {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;

    use super::{FakeSource, SourceOutput, ToolArgs};
    use crate::errors::LensError;
    use crate::value::LensRow;

    #[derive(Clone)]
    pub struct CannedRowsSource {
        rows: Vec<LensRow>,
        events: Arc<Mutex<Vec<&'static str>>>,
    }

    impl CannedRowsSource {
        pub fn new(rows: Vec<LensRow>, events: Arc<Mutex<Vec<&'static str>>>) -> Self {
            Self { rows, events }
        }
    }

    #[async_trait]
    impl FakeSource for CannedRowsSource {
        async fn invoke(&self, _args: &ToolArgs) -> Result<SourceOutput, LensError> {
            self.events.lock().expect("events lock").push("source");
            Ok(SourceOutput::Rows(self.rows.clone()))
        }
    }

    #[derive(Clone)]
    pub struct FailingSource {
        pub detail: String,
        pub events: Arc<Mutex<Vec<&'static str>>>,
    }

    #[async_trait]
    impl FakeSource for FailingSource {
        async fn invoke(&self, _args: &ToolArgs) -> Result<SourceOutput, LensError> {
            self.events.lock().expect("events lock").push("source");
            Err(LensError::SourceError {
                source_name: "fake".to_string(),
                detail: self.detail.clone(),
                sql: None,
                stderr: None,
            })
        }
    }
}
