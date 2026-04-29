use std::path::{Path, PathBuf};

use rusqlite::Connection;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

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
            "SELECT call_id, tool_name, redacted_args_json, snapshot_ref, purged_at_ms
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
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<i64>>(4)?,
            ))
        })
        .map_err(|err| LensError::ReplayUnavailable {
            lens_session_id: lens_session_id.to_string(),
            detail: err.to_string(),
        })?;

    let mut calls = Vec::new();
    for row in rows {
        let (call_id, tool_name, redacted_args_json, snapshot_ref, purged_at_ms) =
            row.map_err(|err| LensError::ReplayUnavailable {
                lens_session_id: lens_session_id.to_string(),
                detail: err.to_string(),
            })?;
        // Tombstone-aware: BEFORE attempting any file IO, check purged_at_ms.
        // D3 truthfulness: the manifest row is the audit-of-record; the snapshot
        // file is gone but the purge event is recorded.
        if let Some(purged_at_ms) = purged_at_ms {
            return Err(LensError::SnapshotPurged {
                lens_session_id: lens_session_id.to_string(),
                purged_at_ms,
                purged_at_iso8601: format_iso8601_ms(purged_at_ms),
                retention_days_repr: "unknown (read from active profile)".to_string(),
            });
        }
        let snapshot_ref = match snapshot_ref {
            Some(p) => PathBuf::from(p),
            None => {
                // Defensive: snapshot_ref NULL but purged_at_ms also NULL is
                // a malformed row; treat as replay-unavailable rather than crash.
                return Err(LensError::ReplayUnavailable {
                    lens_session_id: lens_session_id.to_string(),
                    detail: format!(
                        "call {call_id} has no snapshot_ref and no purged_at_ms tombstone"
                    ),
                });
            }
        };
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
        let redacted_args: RedactedToolArgs =
            serde_json::from_str(&redacted_args_json).map_err(|err| {
                LensError::ReplayUnavailable {
                    lens_session_id: lens_session_id.to_string(),
                    detail: err.to_string(),
                }
            })?;
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

fn format_iso8601_ms(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let nanos = (ms.rem_euclid(1000) as u32) * 1_000_000;
    OffsetDateTime::from_unix_timestamp(secs)
        .ok()
        .and_then(|dt| dt.replace_nanosecond(nanos).ok())
        .and_then(|dt| dt.format(&Rfc3339).ok())
        .unwrap_or_else(|| format!("epoch_ms={ms}"))
}

fn restore_tokens(gaze_session: &gaze::Session, input: &str) -> Result<String, LensError> {
    let mut restored = input.to_string();
    let mut tokens = gaze_session.tokens();
    tokens.sort_by_key(|token| std::cmp::Reverse(token.len()));
    for token in tokens {
        let raw =
            gaze_session
                .restore_strict(&token)
                .map_err(|err| LensError::ReplayUnavailable {
                    lens_session_id: "unknown".to_string(),
                    detail: err.to_string(),
                })?;
        restored = restored.replace(&token, &raw);
    }
    Ok(restored)
}
