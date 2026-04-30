//! SSH `.env` discovery helpers for setup-time `init` only.
//!
//! This module is intentionally a CLI leaf. It must not be imported from
//! `Session::dispatch_tool` or `McpFrontend`.

use std::collections::BTreeSet;
use std::fmt;
use std::path::Path;

use zeroize::Zeroizing;

use crate::cli::init::SourceKind;
use crate::errors::LensError;
use crate::source::ssh_tunnel::{SshError, remote_argv, validate_ssh_path};

#[derive(Clone)]
pub struct EnvVar {
    pub key: String,
    pub value: Zeroizing<String>,
}

impl fmt::Debug for EnvVar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EnvVar")
            .field("key", &self.key)
            .field("value", &"<redacted>")
            .finish()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiscoveredDbMeta {
    pub kind: Option<SourceKind>,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub database: Option<String>,
    pub username: Option<String>,
}

impl DiscoveredDbMeta {
    pub fn source_port_or_default(&self, kind: SourceKind) -> Option<u16> {
        self.port.or(match kind {
            SourceKind::Mysql => Some(3306),
            SourceKind::Postgres => Some(5432),
            SourceKind::Sqlite | SourceKind::SshLog => None,
        })
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DiscoveryPath {
    #[default]
    HostDbOnly,
    AsIs,
    Abort,
}

pub const DISCOVERY_PATH_CHOICES: &[&str] = &[
    "Host + database only; enter a separate readonly credential",
    "Store discovered production credential as-is",
    "Abort discovery",
];

pub fn parse_env(input: &str) -> Result<Vec<EnvVar>, LensError> {
    let mut vars = Vec::new();
    let mut seen = BTreeSet::new();
    for (line_no, raw_line) in input.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.ends_with('\\') {
            return profile_err(format!(
                "unsupported multiline .env value at line {}",
                line_no + 1
            ));
        }
        let line = line.strip_prefix("export ").unwrap_or(line).trim_start();
        let Some((key, rest)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if !is_env_key(key) {
            continue;
        }
        if !key.starts_with("DB_") {
            continue;
        }
        if !matches!(
            key,
            "DB_CONNECTION" | "DB_HOST" | "DB_PORT" | "DB_DATABASE" | "DB_USERNAME" | "DB_PASSWORD"
        ) {
            continue;
        }
        if !seen.insert(key.to_string()) {
            return profile_err(format!("duplicate .env key: {key}"));
        }
        let value = parse_value(rest.trim_start(), line_no + 1)?;
        reject_unsupported_expansion(&value, line_no + 1)?;
        vars.push(EnvVar {
            key: key.to_string(),
            value: Zeroizing::new(value),
        });
    }
    Ok(vars)
}

pub fn extract_db(
    vars: &mut Vec<EnvVar>,
) -> Result<(DiscoveredDbMeta, Option<Zeroizing<String>>), LensError> {
    let mut meta = DiscoveredDbMeta::default();
    let mut password = None;
    let mut keep = Vec::with_capacity(vars.len());
    for v in vars.drain(..) {
        match v.key.as_str() {
            "DB_CONNECTION" => {
                meta.kind = parse_db_connection(v.value.as_str());
            }
            "DB_HOST" => meta.host = Some(v.value.as_str().to_string()),
            "DB_PORT" => {
                meta.port = Some(v.value.parse::<u16>().map_err(|_| LensError::Profile {
                    detail: format!("DB_PORT '{}' is not a valid u16", v.value.as_str()),
                })?);
            }
            "DB_DATABASE" => meta.database = Some(v.value.as_str().to_string()),
            "DB_USERNAME" => meta.username = Some(v.value.as_str().to_string()),
            "DB_PASSWORD" => {
                password = Some(v.value.clone());
                continue;
            }
            _ => {}
        }
        keep.push(v);
    }
    *vars = keep;
    Ok((meta, password))
}

pub fn validate_env_path(path: &Path) -> Result<&Path, LensError> {
    let path_str = path.to_string_lossy();
    if !path.is_absolute() {
        return Err(LensError::Profile {
            detail: "discovery env path must be absolute".into(),
        });
    }
    validate_ssh_path(&path_str).map_err(|err| LensError::Profile {
        detail: format!("invalid discovery env path: {err}"),
    })?;
    Ok(path)
}

pub fn cat_env_argv(host: &str, path: &Path) -> Result<Vec<String>, LensError> {
    let path_str = path.to_string_lossy();
    remote_argv(host, &["cat"], &path_str).map_err(ssh_to_profile)
}

fn parse_db_connection(value: &str) -> Option<SourceKind> {
    match value {
        "mysql" | "mariadb" => Some(SourceKind::Mysql),
        "pgsql" | "postgres" | "postgresql" => Some(SourceKind::Postgres),
        _ => None,
    }
}

fn parse_value(input: &str, line_no: usize) -> Result<String, LensError> {
    if let Some(rest) = input.strip_prefix('"') {
        parse_double_quoted(rest, line_no)
    } else if let Some(rest) = input.strip_prefix('\'') {
        parse_single_quoted(rest, line_no)
    } else {
        Ok(strip_unquoted_comment(input).trim_end().to_string())
    }
}

fn parse_double_quoted(rest: &str, line_no: usize) -> Result<String, LensError> {
    let mut out = String::new();
    let mut escaped = false;
    for ch in rest.chars() {
        if escaped {
            match ch {
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                other => {
                    out.push('\\');
                    out.push(other);
                }
            }
            escaped = false;
            continue;
        }
        match ch {
            '\\' => escaped = true,
            '"' => return Ok(out),
            other => out.push(other),
        }
    }
    if escaped {
        out.push('\\');
    }
    profile_err(format!(
        "unterminated double-quoted .env value at line {line_no}"
    ))
}

fn parse_single_quoted(rest: &str, line_no: usize) -> Result<String, LensError> {
    let Some(end) = rest.find('\'') else {
        return profile_err(format!(
            "unterminated single-quoted .env value at line {line_no}"
        ));
    };
    let value = &rest[..end];
    if value.contains('\\') {
        return profile_err("single-quoted .env values do not support escape sequences");
    }
    Ok(value.to_string())
}

fn strip_unquoted_comment(input: &str) -> &str {
    input.find(" #").map_or(input, |idx| &input[..idx])
}

fn reject_unsupported_expansion(value: &str, line_no: usize) -> Result<(), LensError> {
    if value.contains("${") || value.contains("$(") || value.contains('`') {
        return profile_err(format!(
            "unsupported shell expansion in .env value at line {line_no}"
        ));
    }
    Ok(())
}

fn is_env_key(key: &str) -> bool {
    let mut chars = key.bytes();
    let Some(first) = chars.next() else {
        return false;
    };
    matches!(first, b'A'..=b'Z' | b'_')
        && chars.all(|byte| matches!(byte, b'A'..=b'Z' | b'0'..=b'9' | b'_'))
}

fn ssh_to_profile(err: SshError) -> LensError {
    LensError::Profile {
        detail: err.to_string(),
    }
}

fn profile_err<T>(detail: impl Into<String>) -> Result<T, LensError> {
    Err(LensError::Profile {
        detail: detail.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_only_returns_db_keys() {
        let vars = parse_env("APP_KEY=x\nDB_HOST=h\nDB_PASSWORD=p\nREDIS_HOST=r\n").unwrap();
        assert_eq!(
            vars.iter().map(|v| v.key.as_str()).collect::<Vec<_>>(),
            vec!["DB_HOST", "DB_PASSWORD"]
        );
    }

    #[test]
    fn parse_double_quoted_with_escaped_quote() {
        let mut vars = parse_env(r#"DB_PASSWORD="p@ss \"quoted\""\n"#).unwrap();
        let (_, pw) = extract_db(&mut vars).unwrap();
        assert_eq!(pw.unwrap().as_str(), r#"p@ss "quoted""#);
    }

    #[test]
    fn parse_single_quoted_rejects_backslash() {
        assert!(parse_env(r"DB_PASSWORD='not\valid'\n").is_err());
    }

    #[test]
    fn parse_unquoted_preserves_inline_hash_without_space() {
        let mut vars = parse_env("DB_PASSWORD=abc#def\n").unwrap();
        let (_, pw) = extract_db(&mut vars).unwrap();
        assert_eq!(pw.unwrap().as_str(), "abc#def");
    }

    #[test]
    fn parse_unquoted_strips_space_hash_comment() {
        let mut vars = parse_env("DB_HOST=a # comment\n").unwrap();
        let (db, _) = extract_db(&mut vars).unwrap();
        assert_eq!(db.host.as_deref(), Some("a"));
    }

    #[test]
    fn parse_duplicate_keys_errors() {
        assert!(matches!(
            parse_env("DB_HOST=a\nDB_HOST=b\n"),
            Err(LensError::Profile { .. })
        ));
    }

    #[test]
    fn parse_invalid_port_errors() {
        let mut vars = parse_env("DB_PORT=nope\n").unwrap();
        assert!(extract_db(&mut vars).is_err());
    }

    #[test]
    fn extract_db_zeroizes_password_in_source_vars() {
        let mut vars = parse_env("DB_HOST=h\nDB_PASSWORD=secret\n").unwrap();
        let (_, pw) = extract_db(&mut vars).unwrap();
        assert!(vars.iter().all(|v| v.key != "DB_PASSWORD"));
        assert_eq!(pw.unwrap().as_str(), "secret");
        assert_eq!(vars.len(), 1);
    }

    #[test]
    fn cat_env_argv_matches_repo_ssh_shape() {
        let argv = cat_env_argv("deploy@app01", Path::new("/var/www/app/.env")).unwrap();
        assert_eq!(
            argv,
            vec![
                "ssh",
                "--",
                "deploy@app01",
                "cat",
                "--",
                "/var/www/app/.env"
            ]
        );
    }
}
