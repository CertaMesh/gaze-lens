use gaze_lens::cli::init::mcp_writer::{
    LegacyMigration, mcp_json_legacy_migration_prompt, render_claude_code_json,
    render_claude_code_json_with_migration, render_codex_toml, render_cursor_json,
};

const COMMAND: &str = "gaze-lens";

fn args_for(_name: &str) -> Vec<String> {
    vec!["serve".into()]
}

#[test]
fn claude_code_first_profile_uses_primary_key() {
    let out = render_claude_code_json(None, "prod", COMMAND, &args_for("prod"), false).unwrap();
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let servers = v["mcpServers"].as_object().unwrap();
    assert!(servers.contains_key("gaze-lens"));
    assert_eq!(servers["gaze-lens"]["command"], "gaze-lens");
    assert_eq!(servers["gaze-lens"]["args"][0], "serve");
}

#[test]
fn second_profile_reuses_primary_key() {
    let existing = r#"{"mcpServers":{"gaze-lens":{"command":"gaze-lens","args":["serve","--profile","prod"]}}}"#;
    let out =
        render_claude_code_json(Some(existing), "dev", COMMAND, &args_for("dev"), false).unwrap();
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let servers = v["mcpServers"].as_object().unwrap();
    assert_eq!(servers.len(), 1);
    assert_eq!(servers["gaze-lens"]["args"][2], "prod");
}

#[test]
fn same_profile_same_command_args_reuses_primary_key() {
    let existing = r#"{"mcpServers":{"gaze-lens":{"command":"gaze-lens","args":["serve","--profile","dev"]}}}"#;
    let out =
        render_claude_code_json(Some(existing), "dev", COMMAND, &args_for("dev"), false).unwrap();
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let servers = v["mcpServers"].as_object().unwrap();
    assert!(servers.contains_key("gaze-lens"));
    assert_eq!(servers["gaze-lens"]["args"][2], "dev");
}

#[test]
fn same_profile_different_command_collides_without_overwrite() {
    let existing = r#"{"mcpServers":{"gaze-lens":{"command":"/opt/gaze-lens","args":["serve","--profile","dev"]}}}"#;
    let err = render_claude_code_json(Some(existing), "dev", COMMAND, &args_for("dev"), false)
        .unwrap_err();
    assert!(err.to_string().contains("MCP entry `gaze-lens`"));
}

#[test]
fn same_profile_different_command_overwrites_primary_with_allow_overwrite() {
    let existing = r#"{"mcpServers":{"gaze-lens":{"command":"/opt/gaze-lens","args":["serve","--profile","dev"]}}}"#;
    let out =
        render_claude_code_json(Some(existing), "dev", COMMAND, &args_for("dev"), true).unwrap();
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let servers = v["mcpServers"].as_object().unwrap();
    assert_eq!(servers["gaze-lens"]["command"], "gaze-lens");
    assert!(
        !servers.contains_key("gaze-lens-dev"),
        "allow-overwrite for same profile should update primary"
    );
}

#[test]
fn existing_gaze_lens_entry_with_matching_suffix_is_idempotent() {
    let existing = r#"{"mcpServers":{"gaze-lens":{"command":"gaze-lens","args":["serve","--profile","prod"]},"gaze-lens-dev":{"command":"gaze-lens","args":["serve","--profile","dev"]}}}"#;
    let out =
        render_claude_code_json(Some(existing), "dev", COMMAND, &args_for("dev"), false).unwrap();
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let servers = v["mcpServers"].as_object().unwrap();
    assert_eq!(servers.len(), 2);
    assert!(servers.contains_key("gaze-lens-dev"));
    assert_eq!(servers["gaze-lens"]["args"][2], "prod");
}

#[test]
fn legacy_migration_prompt_includes_compliance_note() {
    let existing = r#"{"mcpServers":{"gaze-lens-prod":{"command":"gaze-lens","args":["serve","--profile","prod"]},"gaze-lens-staging":{"command":"gaze-lens","args":["serve","--profile","staging"]}}}"#;
    let prompt = mcp_json_legacy_migration_prompt(Some(existing))
        .unwrap()
        .expect("legacy prompt");
    assert!(
        prompt.contains("Found 2 legacy gaze-lens MCP entries"),
        "{prompt}"
    );
    assert!(
        prompt.contains("v0.2.2 requires a single `gaze-lens` entry"),
        "{prompt}"
    );
    assert!(
        prompt.contains("`profile` argument on every MCP tool call"),
        "{prompt}"
    );
    assert!(prompt.contains("removes the 2 legacy entries"), "{prompt}");
    assert!(prompt.contains("`invalid_params`"), "{prompt}");
    assert!(prompt.contains("default Y"), "{prompt}");
    assert!(!prompt.contains("breaks compliance isolation"), "{prompt}");
}

#[test]
fn confirmed_legacy_migration_removes_suffixed_entries() {
    let existing = r#"{"mcpServers":{"gaze-lens":{"command":"gaze-lens","args":["serve","--profile","prod"]},"gaze-lens-dev":{"command":"gaze-lens","args":["serve","--profile","dev"]}}}"#;
    let out = render_claude_code_json_with_migration(
        Some(existing),
        "dev",
        COMMAND,
        &args_for("dev"),
        false,
        LegacyMigration::Migrate,
    )
    .unwrap();
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let servers = v["mcpServers"].as_object().unwrap();
    assert_eq!(servers.len(), 1);
    assert!(!servers.contains_key("gaze-lens-dev"));
    assert_eq!(servers["gaze-lens"]["args"][0], "serve");
}

#[test]
fn malformed_json_parse_error_includes_position() {
    let existing = r#"{"mcpServers":{"gaze-lens":{"command":"gaze-lens","args":[serve]}}}"#;
    let err =
        render_claude_code_json(Some(existing), "p", COMMAND, &args_for("p"), false).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("line "), "{msg}");
    assert!(msg.contains("column "), "{msg}");
}

#[test]
fn codex_toml_emits_mcp_servers_table() {
    let out = render_codex_toml(None, "prod", COMMAND, &args_for("prod"), false).unwrap();
    assert!(out.contains("[mcp_servers.gaze-lens]"), "{out}");
    assert!(out.contains(r#"command = "gaze-lens""#));
    assert!(out.contains(r#""serve""#));
}

#[test]
fn codex_toml_second_profile_suffixed() {
    let existing = r#"
[mcp_servers.gaze-lens]
command = "gaze-lens"
args = ["serve", "--profile", "prod"]
"#;
    let out = render_codex_toml(Some(existing), "dev", COMMAND, &args_for("dev"), false).unwrap();
    assert!(!out.contains("[mcp_servers.gaze-lens-dev]"), "{out}");
    assert!(out.contains("[mcp_servers.gaze-lens]"), "{out}");
    assert!(out.contains(r#""prod""#), "{out}");
}

#[test]
fn cursor_uses_same_format_as_claude_code() {
    let out = render_cursor_json(None, "p", COMMAND, &args_for("p"), false).unwrap();
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["mcpServers"]["gaze-lens"]["args"][0], "serve");
}
