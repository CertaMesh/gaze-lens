//! TOML-edit-based profile-file renderer.
//!
//! `render_profile_toml(existing, section, allow_overwrite)` returns the new
//! file contents. It preserves any unrelated `[[profiles]]` entries verbatim
//! (including their `auto_purge` lines per CB2) and either appends `section`
//! as a new entry, replaces an existing entry of the same name (when
//! `allow_overwrite`), or errors with `Collision`.
//!
//! Wire format (CB3): source `kind` is snake_case (`mysql`, `postgres`,
//! `sqlite`, `ssh_log`). CLI dash-spelling (`ssh-log`) is exclusive to the
//! flag layer.
//!
//! Parse errors carry an explicit `(line, column)` in `Display` (MS3) so the
//! test bar is decoupled from `toml_edit`'s upstream literal format.

use std::path::PathBuf;

use thiserror::Error;
use toml_edit::{Array, ArrayOfTables, DocumentMut, Item, Table};

use crate::cli::init::SourceKind;
use crate::cli::init::plan::{AutoPurgeChoice, ProfileSection};

#[derive(Debug, Error)]
pub enum RenderError {
    #[error(
        "profile `{name}` already exists; rerun with --allow-overwrite or pick a different name"
    )]
    Collision { name: String },
    /// Directive 14 + MS3: `Display` interpolates path + (line, column)
    /// explicitly via `toml_edit::TomlError::span()` mapped through the same
    /// algorithm as `src/profile.rs::line_column`. The test bar
    /// (`malformed_existing_toml_reports_path_and_position`) asserts the
    /// literal substrings `"line "` and `"column "` — gaze-lens owns those
    /// tokens, so toml_edit patch bumps cannot break the test.
    #[error(
        "malformed existing toml at {} at line {line}, column {column}: {source_msg}",
        .path.display(),
    )]
    Parse {
        path: PathBuf,
        line: usize,
        column: usize,
        source_msg: String,
    },
}

/// Mirrors `src/profile.rs::line_column`. Colocated to avoid leaking the
/// helper across modules; if `src/profile.rs` ever exposes a public
/// `line_column` we should switch to it.
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

pub fn render_profile_toml(
    existing: Option<&str>,
    section: &ProfileSection,
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

    let profiles = doc
        .entry("profiles")
        .or_insert_with(|| Item::ArrayOfTables(ArrayOfTables::new()));
    let array = profiles
        .as_array_of_tables_mut()
        .ok_or_else(|| RenderError::Parse {
            path: PathBuf::new(),
            line: 0,
            column: 0,
            source_msg: "profiles is not an array of tables".into(),
        })?;

    let mut existing_idx = None;
    for (i, t) in array.iter().enumerate() {
        if t.get("name").and_then(|v| v.as_str()) == Some(section.name.as_str()) {
            existing_idx = Some(i);
            break;
        }
    }
    if let Some(i) = existing_idx {
        if !allow_overwrite {
            return Err(RenderError::Collision {
                name: section.name.clone(),
            });
        }
        *array.get_mut(i).unwrap() = build_profile_table(section);
    } else {
        array.push(build_profile_table(section));
    }
    Ok(doc.to_string())
}

fn build_profile_table(s: &ProfileSection) -> Table {
    let mut t = Table::new();
    t.insert("name", toml_edit::value(s.name.as_str()));
    if let Some(p) = &s.policy_path {
        t.insert("policy", toml_edit::value(p.to_string_lossy().into_owned()));
    }
    if !s.schema_allowlist.is_empty() {
        let mut arr = Array::new();
        for c in &s.schema_allowlist {
            arr.push(c.as_str());
        }
        t.insert("schema_allowlist", toml_edit::value(arr));
    }
    if let Some(d) = s.snapshot_retention_days {
        t.insert("snapshot_retention_days", toml_edit::value(d as i64));
    }
    // CB2: enum string, never bool. Off omitted entirely (default).
    match s.auto_purge {
        AutoPurgeChoice::Off => {}
        AutoPurgeChoice::Warn => {
            t.insert("auto_purge", toml_edit::value("warn"));
        }
        AutoPurgeChoice::Purge => {
            t.insert("auto_purge", toml_edit::value("purge"));
        }
    }
    let mut src = Table::new();
    // CB3: snake_case. Distinct from CLI dash-spelling `ssh-log`.
    src.insert(
        "kind",
        toml_edit::value(source_kind_str_toml(s.source_kind)),
    );
    if let Some(h) = &s.source_host {
        src.insert("host", toml_edit::value(h.as_str()));
    }
    if let Some(p) = s.source_port {
        src.insert("port", toml_edit::value(p as i64));
    }
    if let Some(d) = &s.source_database {
        src.insert("database", toml_edit::value(d.as_str()));
    }
    if let Some(u) = &s.source_username {
        src.insert("username", toml_edit::value(u.as_str()));
    }
    if let Some(env) = &s.source_password_env {
        src.insert("password_env", toml_edit::value(env.as_str()));
    }
    if let Some(h) = &s.source_ssh_host {
        src.insert("ssh_host", toml_edit::value(h.as_str()));
    }
    if let Some(p) = s.source_local_port {
        src.insert("local_port", toml_edit::value(p as i64));
    }
    if let Some(p) = &s.source_path {
        src.insert("path", toml_edit::value(p.to_string_lossy().into_owned()));
    }
    if !s.source_json_text_columns.is_empty() && matches!(s.source_kind, SourceKind::Sqlite) {
        let mut arr = Array::new();
        for c in &s.source_json_text_columns {
            arr.push(c.as_str());
        }
        src.insert("json_text_columns", toml_edit::value(arr));
    }
    if matches!(s.source_kind, SourceKind::Mysql | SourceKind::Postgres) {
        src.insert("readonly_required", toml_edit::value(true));
    }
    t.insert("source", Item::Table(src));
    t
}

fn source_kind_str_toml(k: SourceKind) -> &'static str {
    match k {
        SourceKind::Mysql => "mysql",
        SourceKind::Postgres => "postgres",
        SourceKind::Sqlite => "sqlite",
        SourceKind::SshLog => "ssh_log",
    }
}
