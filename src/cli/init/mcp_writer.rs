//! MCP client config renderers.
//!
//! Three flavors:
//! - **Codex** (`~/.codex/config.toml`): `[mcp_servers.<key>]` toml table.
//! - **Claude Code** (`<cwd>/.mcp.json`): `{"mcpServers": {"<key>": {...}}}`.
//! - **Cursor** (project or user `.cursor/mcp.json`): same shape as Claude Code.
//!
//! Each renderer chooses the entry key at write-time (directive 19): if no
//! existing `gaze-lens` key, use it; otherwise suffix to `gaze-lens-<profile>`.
//! Existing entries with the SAME profile name + same command/args are no-ops
//! (caller's `would_write` byte-compare skips the write).

use std::path::PathBuf;

use serde_json::{Map, Value};
use toml_edit::{Array, DocumentMut, Item, Table};

use crate::cli::init::profile_writer::RenderError;

const PRIMARY_KEY: &str = "gaze-lens";

fn line_column_from_input(input: &str, byte_index: usize) -> (usize, usize) {
    let mut line = 1usize;
    let mut column = 1usize;
    for (index, ch) in input.char_indices() {
        if index >= byte_index {
            break;
        }
        if ch == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }
    (line, column)
}

fn entry_key_for(existing_keys: &[&str], profile_name: &str) -> String {
    let suffix_key = format!("{PRIMARY_KEY}-{profile_name}");
    if !existing_keys.contains(&PRIMARY_KEY) {
        PRIMARY_KEY.to_string()
    } else if !existing_keys.iter().any(|k| *k == suffix_key) {
        suffix_key
    } else {
        // Both occupied → operator must rerun with --allow-overwrite.
        suffix_key
    }
}

/// Codex config.toml writer. `[mcp_servers.<key>] command = ..., args = [...]`.
pub fn render_codex_toml(
    existing: Option<&str>,
    profile_name: &str,
    command: &str,
    cmd_args: &[String],
    allow_overwrite: bool,
) -> Result<String, RenderError> {
    let mut doc: DocumentMut = match existing {
        Some(s) => s.parse().map_err(|e: toml_edit::TomlError| {
            let (line, column) = match e.span() {
                Some(span) => line_column_from_input(s, span.start),
                None => (0, 0),
            };
            RenderError::Parse {
                path: PathBuf::new(),
                line,
                column,
                source_msg: e.message().to_string(),
            }
        })?,
        None => DocumentMut::new(),
    };

    if doc.get("mcp_servers").is_none() {
        doc.insert("mcp_servers", Item::Table(Table::new()));
    }
    let servers = doc
        .get_mut("mcp_servers")
        .and_then(|i| i.as_table_mut())
        .ok_or_else(|| RenderError::Parse {
            path: PathBuf::new(),
            line: 0,
            column: 0,
            source_msg: "mcp_servers is not a table".into(),
        })?;

    let existing_keys: Vec<&str> = servers.iter().map(|(k, _)| k).collect();
    let suffix_key = format!("{PRIMARY_KEY}-{profile_name}");
    let key = entry_key_for(&existing_keys, profile_name);

    // If the chosen key already exists and we're not allowed to overwrite,
    // and the existing entry is for a DIFFERENT profile (collision), error.
    if servers.contains_key(&key) && !allow_overwrite {
        // Compare existing args[2] with profile_name. If matches, treat as
        // no-op (will be dropped by would_write); if differs, collision.
        let existing_profile = servers
            .get(&key)
            .and_then(|i| i.as_table())
            .and_then(|t| t.get("args"))
            .and_then(|i| i.as_array())
            .and_then(|a| a.iter().nth(2))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if existing_profile != profile_name {
            return Err(RenderError::Collision {
                name: format!("codex MCP entry `{key}`"),
            });
        }
        // Same profile name → caller's would_write will detect no-op via byte
        // compare. We still rewrite the table here (idempotent).
    }
    // Belt-and-braces: if PRIMARY_KEY exists for a DIFFERENT profile, choose
    // the suffixed key instead (directive 19 dispatch at write-time).
    let final_key = if servers.contains_key(PRIMARY_KEY)
        && servers
            .get(PRIMARY_KEY)
            .and_then(|i| i.as_table())
            .and_then(|t| t.get("args"))
            .and_then(|i| i.as_array())
            .and_then(|a| a.iter().nth(2))
            .and_then(|v| v.as_str())
            != Some(profile_name)
    {
        suffix_key
    } else {
        key
    };

    let mut entry = Table::new();
    entry.insert("command", toml_edit::value(command));
    let mut arr = Array::new();
    for a in cmd_args {
        arr.push(a.as_str());
    }
    entry.insert("args", toml_edit::value(arr));
    servers.insert(&final_key, Item::Table(entry));

    Ok(doc.to_string())
}

/// Claude Code `.mcp.json` writer. Key = "mcpServers" object.
pub fn render_claude_code_json(
    existing: Option<&str>,
    profile_name: &str,
    command: &str,
    cmd_args: &[String],
    allow_overwrite: bool,
) -> Result<String, RenderError> {
    render_mcp_json(existing, profile_name, command, cmd_args, allow_overwrite)
}

/// Cursor `mcp.json` writer. Same shape as Claude Code (`mcpServers` root).
pub fn render_cursor_json(
    existing: Option<&str>,
    profile_name: &str,
    command: &str,
    cmd_args: &[String],
    allow_overwrite: bool,
) -> Result<String, RenderError> {
    render_mcp_json(existing, profile_name, command, cmd_args, allow_overwrite)
}

fn render_mcp_json(
    existing: Option<&str>,
    profile_name: &str,
    command: &str,
    cmd_args: &[String],
    allow_overwrite: bool,
) -> Result<String, RenderError> {
    let mut root: Value = match existing {
        Some(s) => serde_json::from_str(s).map_err(|e: serde_json::Error| RenderError::Parse {
            path: PathBuf::new(),
            line: e.line(),
            column: e.column(),
            source_msg: e.to_string(),
        })?,
        None => Value::Object(Map::new()),
    };

    let obj = root.as_object_mut().ok_or_else(|| RenderError::Parse {
        path: PathBuf::new(),
        line: 0,
        column: 0,
        source_msg: "expected JSON object at root".into(),
    })?;
    if !obj.contains_key("mcpServers") {
        obj.insert("mcpServers".into(), Value::Object(Map::new()));
    }
    let servers = obj
        .get_mut("mcpServers")
        .and_then(|v| v.as_object_mut())
        .ok_or_else(|| RenderError::Parse {
            path: PathBuf::new(),
            line: 0,
            column: 0,
            source_msg: "mcpServers is not an object".into(),
        })?;

    let existing_keys: Vec<&str> = servers.keys().map(|s| s.as_str()).collect();
    let suffix_key = format!("{PRIMARY_KEY}-{profile_name}");
    let initial_key = entry_key_for(&existing_keys, profile_name);

    let final_key = if servers.contains_key(PRIMARY_KEY)
        && servers
            .get(PRIMARY_KEY)
            .and_then(|v| v.as_object())
            .and_then(|o| o.get("args"))
            .and_then(|v| v.as_array())
            .and_then(|a| a.get(2))
            .and_then(|v| v.as_str())
            != Some(profile_name)
    {
        suffix_key.clone()
    } else {
        initial_key
    };

    if servers.contains_key(&final_key) && !allow_overwrite {
        let existing_profile = servers
            .get(&final_key)
            .and_then(|v| v.as_object())
            .and_then(|o| o.get("args"))
            .and_then(|v| v.as_array())
            .and_then(|a| a.get(2))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if existing_profile != profile_name {
            return Err(RenderError::Collision {
                name: format!("MCP entry `{final_key}`"),
            });
        }
    }

    let mut entry = Map::new();
    entry.insert("command".into(), Value::String(command.to_string()));
    let args_arr: Vec<Value> = cmd_args.iter().map(|s| Value::String(s.clone())).collect();
    entry.insert("args".into(), Value::Array(args_arr));
    servers.insert(final_key, Value::Object(entry));

    let out = serde_json::to_string_pretty(&root).map_err(|e| RenderError::Parse {
        path: PathBuf::new(),
        line: 0,
        column: 0,
        source_msg: format!("serialize: {e}"),
    })?;
    Ok(out + "\n")
}
