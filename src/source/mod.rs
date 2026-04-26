use async_trait::async_trait;

use crate::errors::LensError;
use crate::value::LensRow;

pub mod db;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ToolArgs(pub serde_json::Value);

#[derive(Debug, Clone, PartialEq)]
pub enum SourceOutput {
    Rows(Vec<LensRow>),
    Text(String),
}

#[async_trait]
pub trait FakeSource: Send + Sync {
    async fn invoke(&self, args: &ToolArgs) -> Result<SourceOutput, LensError>;
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
