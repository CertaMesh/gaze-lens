//! Production policy renderer used by `gaze-lens init --production`.

use std::path::{Path, PathBuf};

use thiserror::Error;
use toml_edit::{DocumentMut, Item, Table};

use crate::policy::PolicyFile;

#[derive(Debug, Error)]
pub enum PolicyWriteError {
    #[error(
        "production policy {} is below the fail-closed floor; rerun with --allow-policy-overwrite to add `[ner].model_dir` and require default tokenization",
        .path.display(),
    )]
    AlreadyBelowFloorNeedsConsent { path: PathBuf },
    #[error(
        "malformed production policy {} at line {line}, column {column}: {source_msg}",
        .path.display(),
    )]
    Parse {
        path: PathBuf,
        line: usize,
        column: usize,
        source_msg: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyWriteOutcome {
    pub bytes: Option<Vec<u8>>,
    pub unchanged: bool,
}

pub fn render_production_policy(
    existing: Option<&str>,
    model_dir: &Path,
    allow_overwrite: bool,
) -> Result<PolicyWriteOutcome, PolicyWriteError> {
    render_production_policy_for_path(existing, model_dir, allow_overwrite, Path::new(""))
}

pub(crate) fn render_production_policy_for_path(
    existing: Option<&str>,
    model_dir: &Path,
    allow_overwrite: bool,
    path: &Path,
) -> Result<PolicyWriteOutcome, PolicyWriteError> {
    match existing {
        None => {
            let rendered = render_fresh_policy(model_dir);
            parse_policy(&rendered, path)?;
            Ok(PolicyWriteOutcome {
                bytes: Some(rendered.into_bytes()),
                unchanged: false,
            })
        }
        Some(input) => render_existing_policy(input, model_dir, allow_overwrite, path),
    }
}

fn render_existing_policy(
    input: &str,
    model_dir: &Path,
    allow_overwrite: bool,
    path: &Path,
) -> Result<PolicyWriteOutcome, PolicyWriteError> {
    let mut doc: DocumentMut = input
        .parse()
        .map_err(|err: toml_edit::TomlError| toml_parse_error(input, path, err))?;

    if policy_is_at_floor(&doc) {
        parse_policy(input, path)?;
        return Ok(PolicyWriteOutcome {
            bytes: None,
            unchanged: true,
        });
    }
    if !allow_overwrite {
        return Err(PolicyWriteError::AlreadyBelowFloorNeedsConsent {
            path: path.to_path_buf(),
        });
    }

    ensure_ner_model_dir(&mut doc, model_dir, path)?;
    ensure_default_action(&mut doc, path)?;
    ensure_database_policy(&mut doc, path)?;

    let rendered = doc.to_string();
    parse_policy(&rendered, path)?;
    Ok(PolicyWriteOutcome {
        bytes: Some(rendered.into_bytes()),
        unchanged: false,
    })
}

fn render_fresh_policy(model_dir: &Path) -> String {
    format!(
        "[ner]\nmodel_dir = {}\n\n[policy]\ndefault_action = \"tokenize\"\n\n[policy.database]\n# Add column-specific rules here, e.g.:\n# columns = [{{ column = \"email\", class = \"email\", action = \"tokenize\" }}]\n",
        toml_basic_string(&model_dir.to_string_lossy()),
    )
}

fn policy_is_at_floor(doc: &DocumentMut) -> bool {
    let model_dir_configured = doc
        .get("ner")
        .and_then(Item::as_table)
        .and_then(|table| table.get("model_dir"))
        .and_then(Item::as_str)
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false);
    let default_action_ok = doc
        .get("policy")
        .and_then(Item::as_table)
        .and_then(|table| table.get("default_action"))
        .and_then(Item::as_str)
        .map(|action| matches!(action, "tokenize" | "redact"))
        .unwrap_or(false);
    model_dir_configured && default_action_ok
}

fn ensure_ner_model_dir(
    doc: &mut DocumentMut,
    model_dir: &Path,
    path: &Path,
) -> Result<(), PolicyWriteError> {
    let ner = ensure_root_table(doc, "ner", path)?;
    let has_model_dir = ner
        .get("model_dir")
        .and_then(Item::as_str)
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false);
    if !has_model_dir {
        ner.insert(
            "model_dir",
            toml_edit::value(model_dir.to_string_lossy().into_owned()),
        );
    }
    Ok(())
}

fn ensure_default_action(doc: &mut DocumentMut, path: &Path) -> Result<(), PolicyWriteError> {
    let policy = ensure_root_table(doc, "policy", path)?;
    let current = policy.get("default_action").and_then(Item::as_str);
    if current.is_none() || current == Some("preserve") {
        policy.insert("default_action", toml_edit::value("tokenize"));
    }
    Ok(())
}

fn ensure_database_policy(doc: &mut DocumentMut, path: &Path) -> Result<(), PolicyWriteError> {
    let policy = ensure_root_table(doc, "policy", path)?;
    let database = policy
        .entry("database")
        .or_insert_with(|| Item::Table(Table::new()));
    if database.as_table().is_none() {
        return Err(PolicyWriteError::Parse {
            path: path.to_path_buf(),
            line: 0,
            column: 0,
            source_msg: "policy.database is not a table".into(),
        });
    }
    Ok(())
}

fn ensure_root_table<'a>(
    doc: &'a mut DocumentMut,
    key: &str,
    path: &Path,
) -> Result<&'a mut Table, PolicyWriteError> {
    let item = doc.entry(key).or_insert_with(|| Item::Table(Table::new()));
    item.as_table_mut().ok_or_else(|| PolicyWriteError::Parse {
        path: path.to_path_buf(),
        line: 0,
        column: 0,
        source_msg: format!("{key} is not a table"),
    })
}

fn parse_policy(input: &str, path: &Path) -> Result<PolicyFile, PolicyWriteError> {
    PolicyFile::from_toml(input).map_err(|err| PolicyWriteError::Parse {
        path: path.to_path_buf(),
        line: 0,
        column: 0,
        source_msg: err.to_string(),
    })
}

fn toml_parse_error(input: &str, path: &Path, err: toml_edit::TomlError) -> PolicyWriteError {
    let (line, column) = err
        .span()
        .map(|span| line_column_from_input(input, span.start))
        .unwrap_or((0, 0));
    PolicyWriteError::Parse {
        path: path.to_path_buf(),
        line,
        column,
        source_msg: err.message().to_string(),
    }
}

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

fn toml_basic_string(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 2);
    out.push('"');
    for ch in input.chars() {
        match ch {
            '\u{08}' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\u{0c}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            ch if ch <= '\u{1f}' => {
                out.push_str(&format!("\\u{:04X}", ch as u32));
            }
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_policy_is_generated_with_ner_and_tokenize() {
        let outcome = render_production_policy(None, Path::new("/models/kiji"), false).unwrap();
        let bytes = outcome.bytes.expect("generated bytes");
        let rendered = String::from_utf8(bytes).unwrap();

        assert!(rendered.contains("[ner]"), "{rendered}");
        assert!(
            rendered.contains("model_dir = \"/models/kiji\""),
            "{rendered}"
        );
        assert!(
            rendered.contains("default_action = \"tokenize\""),
            "{rendered}"
        );
        assert!(rendered.contains("[policy.database]"), "{rendered}");
        PolicyFile::from_toml(&rendered).expect("generated policy parses");
    }

    #[test]
    fn existing_policy_at_floor_is_unchanged() {
        let existing = r#"
[ner]
model_dir = "/existing/model"

[policy]
default_action = "redact"

[policy.database]
"#;

        let outcome =
            render_production_policy(Some(existing), Path::new("/models/kiji"), false).unwrap();

        assert!(outcome.unchanged);
        assert!(outcome.bytes.is_none());
    }

    #[test]
    fn existing_below_floor_without_consent_errors() {
        let existing = "[policy.database]\n";

        let err =
            render_production_policy(Some(existing), Path::new("/models/kiji"), false).unwrap_err();

        assert!(matches!(
            err,
            PolicyWriteError::AlreadyBelowFloorNeedsConsent { .. }
        ));
    }

    #[test]
    fn existing_below_floor_with_consent_merges_additively_preserving_columns() {
        let existing = r#"
# keep this comment
[policy]
default_action = "preserve"

[policy.database]
columns = [{ column = "email", class = "email", action = "tokenize" }]

[session]
ttl_secs = 60
"#;

        let outcome =
            render_production_policy(Some(existing), Path::new("/models/kiji"), true).unwrap();
        let rendered = String::from_utf8(outcome.bytes.expect("merged bytes")).unwrap();

        assert!(rendered.contains("# keep this comment"), "{rendered}");
        assert!(
            rendered.contains("model_dir = \"/models/kiji\""),
            "{rendered}"
        );
        assert!(
            rendered.contains("default_action = \"tokenize\""),
            "{rendered}"
        );
        assert!(
            rendered.contains(
                "columns = [{ column = \"email\", class = \"email\", action = \"tokenize\" }]"
            ),
            "{rendered}"
        );
        assert!(rendered.contains("[session]"), "{rendered}");
        PolicyFile::from_toml(&rendered).expect("merged policy parses");
    }

    #[test]
    fn existing_tokenize_or_redact_is_never_downgraded() {
        for action in ["tokenize", "redact"] {
            let existing = format!(
                r#"
[ner]
model_dir = "/existing/model"

[policy]
default_action = "{action}"

[policy.database]
"#
            );

            let outcome =
                render_production_policy(Some(&existing), Path::new("/models/kiji"), true).unwrap();

            assert!(outcome.unchanged, "{action}");
            assert!(outcome.bytes.is_none(), "{action}");
        }
    }
}
