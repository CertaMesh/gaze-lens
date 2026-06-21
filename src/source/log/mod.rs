use std::sync::Arc;

use async_trait::async_trait;

use crate::errors::LensError;
use crate::session::ToolCall;
use crate::source::{Source, SourceOutput};

pub mod ssh_log;

use ssh_log::SshLogSource;

pub struct SshLogSourceWrapper {
    inner: Arc<SshLogSource>,
}

impl SshLogSourceWrapper {
    pub fn new(inner: Arc<SshLogSource>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl Source for SshLogSourceWrapper {
    async fn dispatch(&self, call: &ToolCall) -> Result<SourceOutput, LensError> {
        match call.tool_name.as_str() {
            "log_tail" => {
                let args: LogTailArgs = serde_json::from_value(call.args.0.clone())
                    .map_err(|err| source_error(self.inner.profile_name(), err.to_string()))?;
                let lines = args.lines.unwrap_or(100);
                self.inner.tail(lines).await.map(text_output_from_log)
            }
            "log_grep" => {
                let args: LogGrepArgs =
                    serde_json::from_value(call.args.0.clone()).map_err(|_| {
                        source_error(
                            self.inner.profile_name(),
                            "invalid log_grep args".to_string(),
                        )
                    })?;
                let _refresh = args.refresh.unwrap_or(false);
                match args.mode.as_deref().unwrap_or("regex") {
                    "regex" => self
                        .inner
                        .grep(
                            &args.pattern,
                            args.level.as_deref(),
                            args.limit.unwrap_or(100),
                        )
                        .await
                        .map(text_output_from_log),
                    "keyword" => self
                        .inner
                        .grep_window(
                            &args.pattern,
                            args.level.as_deref(),
                            args.limit.unwrap_or(100),
                        )
                        .await
                        .map(text_output_from_log),
                    other => Err(source_error(
                        self.inner.profile_name(),
                        format!("invalid log_grep mode `{other}`; expected `regex` or `keyword`"),
                    )),
                }
            }
            other => Err(source_error(
                self.inner.profile_name(),
                format!("unsupported tool {other} on log source"),
            )),
        }
    }
}

#[derive(serde::Deserialize)]
struct LogTailArgs {
    lines: Option<usize>,
}

#[derive(serde::Deserialize)]
struct LogGrepArgs {
    pattern: String,
    level: Option<String>,
    limit: Option<usize>,
    mode: Option<String>,
    refresh: Option<bool>,
}

fn source_error(profile_name: &str, detail: String) -> LensError {
    LensError::SourceError {
        source_name: profile_name.to_string(),
        detail,
        sql: None,
        stderr: None,
    }
}

fn text_output_from_log(output: ssh_log::SshLogOutput) -> SourceOutput {
    let truncated_at = output.truncated_at.clone();
    SourceOutput::TextWithTruncation {
        text: output.into_text(),
        truncated_at,
    }
}
