//! MCP client config renderers.
//!
//! Three flavors:
//! - **Codex** (`~/.codex/config.toml`): `[mcp_servers.<key>]` toml table.
//! - **Claude Code** (`<cwd>/.mcp.json`): `{"mcpServers": {"<key>": {...}}}`.
//! - **Cursor** (project or user `.cursor/mcp.json`): same shape as Claude Code.
//!
//! Each renderer chooses the entry key at write-time (directive 19): the
//! primary `gaze-lens` key is reused when it already contains the same
//! `command` + `args`; a per-profile suffix is only used for additional
//! profiles when the primary points at another profile.

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

#[derive(Debug, Clone, PartialEq, Eq)]
struct ExistingEntry {
    command: Option<String>,
    args: Vec<String>,
}

impl ExistingEntry {
    fn matches(&self, command: &str, args: &[String]) -> bool {
        self.command.as_deref() == Some(command) && self.args == args
    }

    fn profile_name(&self) -> Option<&str> {
        self.args
            .windows(2)
            .find_map(|pair| (pair[0] == "--profile").then_some(pair[1].as_str()))
    }
}

fn entry_key_for(
    primary: Option<&ExistingEntry>,
    suffix: Option<&ExistingEntry>,
    profile_name: &str,
    command: &str,
    cmd_args: &[String],
    allow_overwrite: bool,
) -> Result<String, RenderError> {
    let suffix_key = format!("{PRIMARY_KEY}-{profile_name}");
    let Some(primary) = primary else {
        return Ok(PRIMARY_KEY.to_string());
    };

    if primary.matches(command, cmd_args) {
        return Ok(PRIMARY_KEY.to_string());
    }

    if primary.profile_name() == Some(profile_name) {
        if allow_overwrite {
            return Ok(PRIMARY_KEY.to_string());
        }
        return Err(RenderError::Collision {
            name: format!("MCP entry `{PRIMARY_KEY}`"),
        });
    }

    if allow_overwrite {
        return Ok(PRIMARY_KEY.to_string());
    }

    match suffix {
        None => Ok(suffix_key),
        Some(entry) if entry.matches(command, cmd_args) => Ok(suffix_key),
        Some(_) => Err(RenderError::Collision {
            name: format!("MCP entry `{suffix_key}`"),
        }),
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

    let suffix_key = format!("{PRIMARY_KEY}-{profile_name}");
    let primary = servers.get(PRIMARY_KEY).and_then(toml_entry);
    let suffix = servers.get(&suffix_key).and_then(toml_entry);
    let final_key = entry_key_for(
        primary.as_ref(),
        suffix.as_ref(),
        profile_name,
        command,
        cmd_args,
        allow_overwrite,
    )?;

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

fn toml_entry(item: &Item) -> Option<ExistingEntry> {
    let table = item.as_table()?;
    let command = table
        .get("command")
        .and_then(|i| i.as_str())
        .map(str::to_string);
    let args = table
        .get("args")
        .and_then(|i| i.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    Some(ExistingEntry { command, args })
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

    let suffix_key = format!("{PRIMARY_KEY}-{profile_name}");
    let primary = servers.get(PRIMARY_KEY).and_then(json_entry);
    let suffix = servers.get(&suffix_key).and_then(json_entry);
    let final_key = entry_key_for(
        primary.as_ref(),
        suffix.as_ref(),
        profile_name,
        command,
        cmd_args,
        allow_overwrite,
    )?;

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

fn json_entry(value: &Value) -> Option<ExistingEntry> {
    let obj = value.as_object()?;
    let command = obj
        .get("command")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let args = obj
        .get("args")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    Some(ExistingEntry { command, args })
}
