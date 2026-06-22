use gaze_lens::cli::serve::{ServeArgs, prepare_session_for_test};
use gaze_lens::frontend::mcp::McpFrontend;
use gaze_lens::session::{CleanOutput, ToolCall};
use gaze_lens::source::ToolArgs;
use rmcp::model::CallToolRequestParam;
use rmcp::{ClientHandler, ServiceExt};

#[derive(Clone, Default)]
struct TestClient;

impl ClientHandler for TestClient {}

#[tokio::test]
async fn local_log_tail_routes_through_redaction_and_manifest() {
    let temp = tempfile::tempdir().expect("tempdir");
    let log_path = temp.path().join("app.log");
    tokio::fs::write(
        &log_path,
        "INFO booted\nERROR customer bob@example.com failed checkout\n",
    )
    .await
    .expect("write log");
    let profile_path = temp.path().join("profiles.toml");
    let policy_path = temp.path().join("policy.toml");
    std::fs::write(
        &policy_path,
        r#"
        [policy]
        default_action = "tokenize"

        [policy.database]
        "#,
    )
    .expect("write policy");
    std::fs::write(
        &profile_path,
        format!(
            r#"
            [[profiles]]
            name = "dev-log"
            policy = "{}"
            source = {{ kind = "local_log", path = "{}" }}
            "#,
            policy_path.display(),
            log_path.display()
        ),
    )
    .expect("write profile");
    let manifest = temp.path().join("manifest.sqlite");
    let snapshot_dir = temp.path().join("snapshots");
    let prepared = prepare_session_for_test(
        ServeArgs {
            profile: Vec::new(),
            manifest: manifest.clone(),
            snapshot_dir,
            print_discovery: false,
        },
        Some(&profile_path),
        None,
    )
    .expect("prepare session");

    let result = prepared
        .session
        .dispatch_tool(ToolCall {
            call_id: ulid::Ulid::new().to_string(),
            tool_name: "log_tail".to_string(),
            args: ToolArgs(serde_json::json!({
                "profile": "dev-log",
                "lines": 10
            })),
        })
        .await
        .expect("dispatch");

    let CleanOutput::Text { text, .. } = result.clean else {
        panic!("expected text output");
    };
    assert!(text.contains("ERROR customer"));
    assert!(!text.contains("bob@example.com"));
    assert!(text.contains("Email_1"), "{text}");

    let connection = rusqlite::Connection::open(manifest).expect("manifest");
    let call_count: u32 = connection
        .query_row("SELECT COUNT(*) FROM calls", [], |row| row.get(0))
        .expect("call count");
    assert_eq!(call_count, 1);
}

#[tokio::test]
async fn local_log_keyword_grep_matches_redacted_tokens_over_mcp() {
    let temp = tempfile::tempdir().expect("tempdir");
    let log_path = temp.path().join("app.log");
    tokio::fs::write(
        &log_path,
        "INFO booted\nERROR customer bob@example.com failed checkout\n",
    )
    .await
    .expect("write log");
    let profile_path = temp.path().join("profiles.toml");
    let policy_path = temp.path().join("policy.toml");
    std::fs::write(
        &policy_path,
        r#"
        [policy]
        default_action = "tokenize"

        [policy.database]
        "#,
    )
    .expect("write policy");
    std::fs::write(
        &profile_path,
        format!(
            r#"
            [[profiles]]
            name = "dev-log"
            policy = "{}"
            source = {{ kind = "local_log", path = "{}" }}
            "#,
            policy_path.display(),
            log_path.display()
        ),
    )
    .expect("write profile");
    let manifest = temp.path().join("manifest.sqlite");
    let prepared = prepare_session_for_test(
        ServeArgs {
            profile: Vec::new(),
            manifest: manifest.clone(),
            snapshot_dir: temp.path().join("snapshots"),
            print_discovery: false,
        },
        Some(&profile_path),
        None,
    )
    .expect("prepare session");

    let (server_transport, client_transport) = tokio::io::duplex(4096);
    let server = McpFrontend::with_session(prepared.session);
    let server_handle = tokio::spawn(async move {
        let running = server.serve(server_transport).await.expect("server");
        running.waiting().await.expect("server wait");
    });
    let client = TestClient.serve(client_transport).await.expect("client");

    let tail = client
        .call_tool(CallToolRequestParam {
            name: "log_tail".into(),
            arguments: serde_json::json!({"profile": "dev-log", "lines": 10})
                .as_object()
                .cloned(),
        })
        .await
        .expect("log_tail");
    let tail_text = tool_result_text(&tail);
    assert!(!tail_text.contains("bob@example.com"), "{tail_text}");
    let token = first_gaze_token(&tail_text).to_string();

    let grep = client
        .call_tool(CallToolRequestParam {
            name: "log_grep".into(),
            arguments: serde_json::json!({
                "profile": "dev-log",
                "pattern": token,
                "mode": "keyword",
                "limit": 5,
                "refresh": true
            })
            .as_object()
            .cloned(),
        })
        .await
        .expect("log_grep keyword");
    let grep_text = tool_result_text(&grep);
    assert!(grep_text.contains("ERROR customer"), "{grep_text}");
    assert!(grep_text.contains("Email_1"), "{grep_text}");
    assert!(!grep_text.contains("bob@example.com"), "{grep_text}");
    assert!(!grep_text.contains("INFO booted"), "{grep_text}");

    let connection = rusqlite::Connection::open(manifest).expect("manifest");
    let call_count: u32 = connection
        .query_row("SELECT COUNT(*) FROM calls", [], |row| row.get(0))
        .expect("call count");
    assert_eq!(call_count, 2);

    client.cancel().await.expect("client cancel");
    server_handle.await.expect("server task");
}

fn tool_result_text(result: &rmcp::model::CallToolResult) -> String {
    result
        .content
        .first()
        .and_then(|content| content.raw.as_text())
        .map(|text| text.text.as_str())
        .expect("text result")
        .to_string()
}

fn first_gaze_token(text: &str) -> &str {
    let start = text.find('<').expect("token start");
    let end = text[start..].find('>').expect("token end");
    &text[start..=start + end]
}
