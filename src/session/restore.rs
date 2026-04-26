use std::path::{Path, PathBuf};

use rusqlite::Connection;

use crate::errors::LensError;

use super::RedactedToolArgs;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RestoredSession {
    pub lens_session_id: String,
    pub calls: Vec<RestoredCall>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RestoredCall {
    pub call_id: String,
    pub tool_name: String,
    pub redacted_args_json: String,
    pub restored_args_json: String,
    pub snapshot_ref: PathBuf,
}

pub fn restore_whole_session(
    manifest_path: &Path,
    lens_session_id: &str,
) -> Result<RestoredSession, LensError> {
    let conn = Connection::open(manifest_path).map_err(|err| LensError::ReplayUnavailable {
        lens_session_id: lens_session_id.to_string(),
        detail: err.to_string(),
    })?;
    let mut stmt = conn
        .prepare(
            "SELECT call_id, tool_name, redacted_args_json, snapshot_ref
             FROM calls
             WHERE lens_session_id = ?1 AND status = 'ok'
             ORDER BY started_at_ms ASC",
        )
        .map_err(|err| LensError::ReplayUnavailable {
            lens_session_id: lens_session_id.to_string(),
            detail: err.to_string(),
        })?;
    let rows = stmt
        .query_map([lens_session_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(|err| LensError::ReplayUnavailable {
            lens_session_id: lens_session_id.to_string(),
            detail: err.to_string(),
        })?;

    let mut calls = Vec::new();
    for row in rows {
        let (call_id, tool_name, redacted_args_json, snapshot_ref) =
            row.map_err(|err| LensError::ReplayUnavailable {
                lens_session_id: lens_session_id.to_string(),
                detail: err.to_string(),
            })?;
        let snapshot_ref = PathBuf::from(snapshot_ref);
        let snapshot_bytes =
            std::fs::read(&snapshot_ref).map_err(|err| LensError::ReplayUnavailable {
                lens_session_id: lens_session_id.to_string(),
                detail: err.to_string(),
            })?;
        let gaze_session = gaze::Session::import(gaze::SensitiveSnapshot::from(snapshot_bytes))
            .map_err(|err| LensError::ReplayUnavailable {
                lens_session_id: lens_session_id.to_string(),
                detail: err.to_string(),
            })?;
        let redacted_args: RedactedToolArgs = serde_json::from_str(&redacted_args_json).map_err(
            |err| LensError::ReplayUnavailable {
                lens_session_id: lens_session_id.to_string(),
                detail: err.to_string(),
            },
        )?;
        let restored_args_json = restore_tokens(&gaze_session, &redacted_args.json)?;
        calls.push(RestoredCall {
            call_id,
            tool_name,
            redacted_args_json,
            restored_args_json,
            snapshot_ref,
        });
    }

    Ok(RestoredSession {
        lens_session_id: lens_session_id.to_string(),
        calls,
    })
}

fn restore_tokens(gaze_session: &gaze::Session, input: &str) -> Result<String, LensError> {
    let mut restored = input.to_string();
    let mut tokens = gaze_session.tokens();
    tokens.sort_by_key(|token| std::cmp::Reverse(token.len()));
    for token in tokens {
        let raw = gaze_session
            .restore_strict(&token)
            .map_err(|err| LensError::ReplayUnavailable {
                lens_session_id: "unknown".to_string(),
                detail: err.to_string(),
            })?;
        restored = restored.replace(&token, &raw);
    }
    Ok(restored)
}
