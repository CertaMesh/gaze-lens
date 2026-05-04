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
    SchemaText(String),
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
    schema_presentation: SchemaPresentation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemaPresentation {
    Raw,
    Tokenized { allowlist: Option<Vec<String>> },
}

impl DbSourceWrapper {
    pub fn new(inner: std::sync::Arc<dyn DbSource>) -> Self {
        Self {
            inner,
            schema_tokenizer: SchemaTokenizer::default(),
            schema_presentation: SchemaPresentation::Raw,
        }
    }

    pub fn with_schema_presentation(
        inner: std::sync::Arc<dyn DbSource>,
        schema_presentation: SchemaPresentation,
    ) -> Self {
        Self {
            inner,
            schema_tokenizer: SchemaTokenizer::default(),
            schema_presentation,
        }
    }
}

#[async_trait]
impl Source for DbSourceWrapper {
    async fn dispatch(&self, call: &ToolCall) -> Result<SourceOutput, LensError> {
        match call.tool_name.as_str() {
            "query" => {
                let mut query: CannedQuery =
                    serde_json::from_value(call.args.0.clone()).map_err(|err| {
                        LensError::SourceError {
                            source_name: call.tool_name.clone(),
                            detail: err.to_string(),
                            sql: None,
                            stderr: None,
                        }
                    })?;
                let schema = self.inner.schema(&query.table).await?;
                query
                    .compile_to_sql(&schema)
                    .map_err(|err| LensError::SourceError {
                        source_name: call.tool_name.clone(),
                        detail: err.to_string(),
                        sql: Some("<canned>".to_string()),
                        stderr: None,
                    })?;
                if query.columns.as_ref().is_none_or(Vec::is_empty) {
                    query.columns = Some(
                        schema
                            .columns
                            .iter()
                            .filter(|column| column.allowed)
                            .map(|column| column.name.clone())
                            .collect(),
                    );
                }
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
                let schema = self.present_schema(self.inner.schema(&args.table).await?);
                serde_json::to_string(&schema)
                    .map(SourceOutput::SchemaText)
                    .map_err(|err| {
                        LensError::ConvertError(crate::value::LowerError::Decode {
                            kind: "json",
                            detail: err.to_string(),
                        })
                    })
            }
            "list_tables" => {
                let tables = self.inner.list_tables().await?;
                let tables = self.present_table_names(&tables);
                serde_json::to_string(&tables)
                    .map(SourceOutput::SchemaText)
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

impl DbSourceWrapper {
    fn present_schema(
        &self,
        schema: crate::source::db::TableSchema,
    ) -> crate::source::db::TableSchema {
        match &self.schema_presentation {
            SchemaPresentation::Raw => schema,
            SchemaPresentation::Tokenized { allowlist } => self
                .schema_tokenizer
                .tokenize_table_schema(&schema, allowlist.as_deref()),
        }
    }

    fn present_table_names(&self, tables: &[String]) -> Vec<String> {
        match &self.schema_presentation {
            SchemaPresentation::Raw => tables.to_vec(),
            SchemaPresentation::Tokenized { allowlist } => self
                .schema_tokenizer
                .tokenize_table_names(tables, allowlist.as_deref()),
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

#[doc(hidden)]
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
