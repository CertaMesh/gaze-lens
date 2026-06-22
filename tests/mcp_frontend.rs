use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use gaze_lens::frontend::mcp::McpFrontend;
use gaze_lens::session::manifest::{LensManifestStore, SnapshotRef};
use gaze_lens::session::{OutputCaps, RedactedToolArgs, ResultSummary, Session, ToolCall};
use gaze_lens::source::{FakeSource, InMemoryFakeSource, SourceOutput, ToolArgs};
use gaze_lens::value::LensValue;
use tracing_subscriber::prelude::*;

fn policy() -> gaze::Policy {
    let mut policy = gaze::Policy::default();
    policy.session.scope = gaze::SessionScope::Conversation;
    policy.rulepacks.bundled = vec!["core".to_string()];
    policy
}

fn session_with_manifest(manifest: Arc<dyn LensManifestStore>) -> Session {
    let temp = tempfile::tempdir().expect("tempdir");
    let snapshot_dir = temp.path().to_path_buf();
    let session = Session::new_with_manifest_for_tests(
        &policy(),
        manifest,
        &snapshot_dir,
        OutputCaps {
            rows: 5,
            ..OutputCaps::default()
        },
    )
    .expect("session");
    std::mem::forget(temp);
    session
}

#[test]
fn test_public_tool_set() {
    assert_eq!(
        McpFrontend::public_tool_names(),
        vec!["query", "schema", "list_tables", "log_tail", "log_grep"]
    );
}

#[test]
fn test_every_tool_requires_profile() {
    let frontend = McpFrontend::new();
    for tool in frontend.list_all_tools() {
        let schema = tool.input_schema.as_ref();
        let required = schema
            .get("required")
            .and_then(|value| value.as_array())
            .expect("required array");
        assert!(
            required
                .iter()
                .any(|value| value.as_str() == Some("profile")),
            "tool {} missing required profile",
            tool.name
        );
        let properties = schema
            .get("properties")
            .and_then(|value| value.as_object())
            .expect("properties");
        let profile = properties.get("profile").expect("profile property");
        assert_eq!(
            profile.get("pattern").and_then(|value| value.as_str()),
            Some(r"^[a-z0-9][a-z0-9_-]{0,63}$")
        );
    }
}

#[test]
fn test_schema_tool_descriptions_explain_tokenized_presentation() {
    let frontend = McpFrontend::new();
    let tools = frontend.list_all_tools();
    let schema = tools
        .iter()
        .find(|tool| tool.name == "schema")
        .expect("schema tool");
    let list_tables = tools
        .iter()
        .find(|tool| tool.name == "list_tables")
        .expect("list_tables tool");
    let schema_description = schema.description.as_deref().expect("schema description");
    let list_description = list_tables
        .description
        .as_deref()
        .expect("list_tables description");

    for description in [schema_description, list_description] {
        assert!(
            description.contains("schema_tokenize = true"),
            "{description}"
        );
        assert!(
            description.contains("schema_allowlist only keeps selected"),
            "{description}"
        );
        assert!(description.contains("presentation"), "{description}");
        assert!(
            description.contains("restarting/reloading the MCP server"),
            "{description}"
        );
    }

    let schema_properties = schema
        .input_schema
        .as_ref()
        .get("properties")
        .and_then(|value| value.as_object())
        .expect("schema properties");
    let table_description = schema_properties["table"]["description"]
        .as_str()
        .expect("table description");
    assert!(
        table_description.contains("raw table names"),
        "{table_description}"
    );
}

#[test]
fn test_log_grep_served_schema_documents_keyword_token_pattern() {
    let frontend = McpFrontend::new();
    let tools = frontend.list_all_tools();
    let log_grep = tools
        .iter()
        .find(|tool| tool.name == "log_grep")
        .expect("log_grep tool");
    let properties = log_grep
        .input_schema
        .as_ref()
        .get("properties")
        .and_then(|value| value.as_object())
        .expect("log_grep properties");

    let pattern_description = properties["pattern"]["description"]
        .as_str()
        .expect("pattern description");
    assert!(pattern_description.contains("RAW log text"));
    assert!(pattern_description.contains("presence/absence oracle"));
    assert!(pattern_description.contains("keyword mode"));
    assert!(pattern_description.contains("complete `<hash:Name_N>` token"));
    assert!(pattern_description.contains("Email_1"));
    assert!(pattern_description.contains("0 hits"));

    let mode_description = properties["mode"]["description"]
        .as_str()
        .expect("mode description");
    assert!(mode_description.contains("`regex` (default)"));
    assert!(mode_description.contains("`keyword`"));
    assert!(mode_description.contains("complete `<hash:Name_N>` token"));
}

#[test]
fn test_frontend_served_schema_documents_every_local_argument_field() {
    let frontend = McpFrontend::new();
    let tools = frontend.list_all_tools();

    for tool_name in ["schema", "list_tables", "log_tail", "log_grep"] {
        let tool = tools
            .iter()
            .find(|tool| tool.name == tool_name)
            .unwrap_or_else(|| panic!("missing tool {tool_name}"));
        let properties = tool
            .input_schema
            .as_ref()
            .get("properties")
            .and_then(|value| value.as_object())
            .unwrap_or_else(|| panic!("missing properties for {tool_name}"));

        for (field_name, field_schema) in properties {
            let description = field_schema
                .get("description")
                .and_then(|value| value.as_str())
                .unwrap_or("");
            assert!(
                !description.trim().is_empty(),
                "{tool_name}.{field_name} missing description"
            );
        }
    }
}

#[tokio::test]
async fn test_log_tail_and_grep_dispatch_through_source() {
    let manifest = Arc::new(RecordingManifest::default());
    let session = session_with_manifest(manifest.clone());
    session.register_fake_source(
        "log_tail",
        Box::new(LogSourceFake {
            lines: vec![
                "INFO boot ok".to_string(),
                "ERROR alice@example.com failed".to_string(),
            ],
            mode: LogMode::Tail,
        }),
    );
    session.register_fake_source(
        "log_grep",
        Box::new(LogSourceFake {
            lines: vec![
                "INFO boot ok".to_string(),
                "ERROR alice@example.com failed".to_string(),
            ],
            mode: LogMode::Grep,
        }),
    );
    let frontend = McpFrontend::with_session(Arc::new(session));

    let tail = frontend
        .call_tool_json("log_tail", serde_json::json!({"lines": 10}))
        .await
        .expect("log_tail");
    let tail_text = text_output(&tail);
    assert!(tail_text.contains("INFO boot ok"));
    assert!(!tail_text.contains("alice@example.com"));

    let grep = frontend
        .call_tool_json(
            "log_grep",
            serde_json::json!({"pattern": "alice@example.com", "level": "ERROR", "limit": 5}),
        )
        .await
        .expect("log_grep");
    let grep_text = text_output(&grep);
    assert!(grep_text.contains("ERROR"));
    assert!(!grep_text.contains("alice@example.com"));

    assert_eq!(
        manifest.statuses.lock().expect("statuses").as_slice(),
        ["begin", "finish", "begin", "finish"]
    );
    let redacted_args = manifest.redacted_args.lock().expect("redacted args");
    assert!(
        redacted_args
            .iter()
            .all(|args| !args.contains("alice@example.com"))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn test_production_log_grep_regex_warning_is_server_log_only() {
    let (production_regex, production_warnings, production_manifest_args) =
        call_log_grep_with_warning_capture(true, None).await;
    assert!(
        production_warnings.iter().any(|warning| {
            warning.contains("production log_grep regex mode")
                && warning.contains("raw-text presence/absence oracle")
                && warning.contains("mode=\"keyword\"")
        }),
        "{production_warnings:?}"
    );

    let (non_production_regex, non_production_warnings, _non_production_manifest_args) =
        call_log_grep_with_warning_capture(false, None).await;
    assert!(
        non_production_warnings
            .iter()
            .all(|warning| !warning.contains("production log_grep regex mode")),
        "{non_production_warnings:?}"
    );

    let (_production_keyword, keyword_warnings, _keyword_manifest_args) =
        call_log_grep_with_warning_capture(true, Some("keyword")).await;
    assert!(
        keyword_warnings
            .iter()
            .all(|warning| !warning.contains("production log_grep regex mode")),
        "{keyword_warnings:?}"
    );

    for response in [&production_regex, &non_production_regex] {
        let text = text_output(response);
        assert!(text.contains("ERROR <"), "{text}");
        assert!(text.contains("Email_1"), "{text}");
        assert!(!text.contains("alice@example.com"), "{text}");
    }
    let serialized_response = serde_json::to_string(&production_regex).expect("response json");
    assert!(!serialized_response.contains("production log_grep regex mode"));
    assert!(!serialized_response.contains("raw-text presence/absence oracle"));
    assert!(!serialized_response.contains("mode=\"keyword\""));
    for redacted_args in production_manifest_args {
        assert!(!redacted_args.contains("production log_grep regex mode"));
        assert!(!redacted_args.contains("raw-text presence/absence oracle"));
        assert!(!redacted_args.contains("mode=\"keyword\""));
    }
}

#[tokio::test]
async fn test_query_e2e_pseudonymized() {
    let manifest = Arc::new(RecordingManifest::default());
    let session = session_with_manifest(manifest.clone());
    session.register_fake_source(
        "query",
        Box::new(InMemoryFakeSource::rows(vec![BTreeMap::from([(
            "email".to_string(),
            LensValue::String("alice@example.com".to_string()),
        )])])),
    );
    let frontend = McpFrontend::with_session(Arc::new(session));

    let result = frontend
        .call_tool_json(
            "query",
            serde_json::json!({
                "table": "users",
                "columns": ["email"],
                "limit": 1
            }),
        )
        .await
        .expect("query");

    let rows = result["clean"]["Rows"]["rows"]
        .as_array()
        .or_else(|| result["clean"]["rows"].as_array())
        .expect("rows");
    let encoded = serde_json::to_string(rows).expect("json");
    assert!(!encoded.contains("alice@example.com"));
    assert!(encoded.contains("<"));
    let redacted_args = manifest
        .redacted_args
        .lock()
        .expect("redacted args")
        .last()
        .cloned()
        .expect("args");
    assert!(!redacted_args.contains("alice@example.com"));
    assert_eq!(
        manifest.statuses.lock().expect("statuses").as_slice(),
        ["begin", "finish"]
    );
}

async fn call_log_grep_with_warning_capture(
    production: bool,
    mode: Option<&str>,
) -> (serde_json::Value, Vec<String>, Vec<String>) {
    let manifest = Arc::new(RecordingManifest::default());
    let session = session_with_manifest(manifest.clone());
    session
        .register_profile_production("test", production)
        .expect("register production flag");
    session.register_fake_source(
        "log_grep",
        Box::new(LogSourceFake {
            lines: vec![
                "INFO boot ok".to_string(),
                "ERROR alice@example.com failed".to_string(),
            ],
            mode: LogMode::Grep,
        }),
    );
    let frontend = McpFrontend::with_session(Arc::new(session));

    let warnings = Arc::new(Mutex::new(Vec::new()));
    let layer = WarningCaptureLayer {
        warnings: Arc::clone(&warnings),
    };
    let subscriber = tracing_subscriber::registry().with(layer);
    let _guard = tracing::subscriber::set_default(subscriber);

    let mut args = serde_json::json!({"pattern": "ERROR"});
    if let Some(mode) = mode {
        args.as_object_mut().expect("args object").insert(
            "mode".to_string(),
            serde_json::Value::String(mode.to_string()),
        );
    }

    let response = frontend
        .call_tool_json("log_grep", args)
        .await
        .expect("log_grep");
    drop(_guard);

    let captured = warnings.lock().expect("warnings").clone();
    let redacted_args = manifest
        .redacted_args
        .lock()
        .expect("redacted args")
        .clone();
    (response, captured, redacted_args)
}

struct WarningCaptureLayer {
    warnings: Arc<Mutex<Vec<String>>>,
}

impl<S> tracing_subscriber::Layer<S> for WarningCaptureLayer
where
    S: tracing::Subscriber,
{
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        if *event.metadata().level() != tracing::Level::WARN {
            return;
        }
        let mut visitor = WarningVisitor::default();
        event.record(&mut visitor);
        self.warnings.lock().expect("warnings").push(format!(
            "{} {}",
            event.metadata().target(),
            visitor.fields.join(" ")
        ));
    }
}

#[derive(Default)]
struct WarningVisitor {
    fields: Vec<String>,
}

impl tracing::field::Visit for WarningVisitor {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.fields.push(format!("{}={value}", field.name()));
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn fmt::Debug) {
        self.fields.push(format!("{}={value:?}", field.name()));
    }
}

#[derive(Default)]
struct RecordingManifest {
    statuses: Mutex<Vec<&'static str>>,
    redacted_args: Mutex<Vec<String>>,
}

#[derive(Clone)]
struct LogSourceFake {
    lines: Vec<String>,
    mode: LogMode,
}

#[derive(Clone)]
enum LogMode {
    Tail,
    Grep,
}

#[async_trait]
impl FakeSource for LogSourceFake {
    async fn invoke(&self, args: &ToolArgs) -> Result<SourceOutput, gaze_lens::errors::LensError> {
        let lines = match self.mode {
            LogMode::Tail => self.lines.clone(),
            LogMode::Grep => {
                let pattern = args
                    .0
                    .get("pattern")
                    .and_then(|value| value.as_str())
                    .expect("pattern");
                let level = args.0.get("level").and_then(|value| value.as_str());
                let re = regex::Regex::new(pattern).expect("regex");
                self.lines
                    .iter()
                    .filter(|line| re.is_match(line))
                    .filter(|line| level.is_none_or(|level| line.contains(level)))
                    .cloned()
                    .collect()
            }
        };
        Ok(SourceOutput::Text(lines.join("\n")))
    }
}

fn text_output(result: &serde_json::Value) -> String {
    result["clean"]["Text"]["text"]
        .as_str()
        .or_else(|| result["clean"]["text"].as_str())
        .expect("text")
        .to_string()
}

impl LensManifestStore for RecordingManifest {
    fn begin_call(
        &self,
        _call: &ToolCall,
        redacted_args: &RedactedToolArgs,
    ) -> Result<(), gaze_lens::errors::LensError> {
        self.statuses.lock().expect("statuses").push("begin");
        self.redacted_args
            .lock()
            .expect("args")
            .push(redacted_args.json.clone());
        Ok(())
    }

    fn finish_call(
        &self,
        _call_id: &str,
        _summary: &ResultSummary,
        _snapshot_ref: &SnapshotRef,
    ) -> Result<(), gaze_lens::errors::LensError> {
        self.statuses.lock().expect("statuses").push("finish");
        Ok(())
    }

    fn fail_call(
        &self,
        _call_id: &str,
        _err: &gaze_lens::errors::LensError,
    ) -> Result<(), gaze_lens::errors::LensError> {
        self.statuses.lock().expect("statuses").push("fail");
        Ok(())
    }
}
