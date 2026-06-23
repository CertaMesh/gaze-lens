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
    pub restore_telemetry_summary: RestoreTelemetrySummary,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RestoredCall {
    pub call_id: String,
    pub tool_name: String,
    pub redacted_args_json: String,
    pub restored_args_json: String,
    pub snapshot_ref: PathBuf,
    pub restore_telemetry: gaze::RestoreTelemetry,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RestoreTelemetrySummary {
    pub success_calls: u64,
    pub partial_calls: u64,
    pub failed_calls: u64,
    pub unknown_token_count: u64,
    pub manifest_bypass_count: u64,
    pub fresh_pii_detected_count: u64,
}

impl RestoreTelemetrySummary {
    fn record(&mut self, telemetry: &gaze::RestoreTelemetry) {
        match telemetry.restore_decision {
            gaze::RestoreDecision::Success => self.success_calls += 1,
            gaze::RestoreDecision::Partial => self.partial_calls += 1,
            gaze::RestoreDecision::Failed => self.failed_calls += 1,
            _ => self.failed_calls += 1,
        }
        self.unknown_token_count += telemetry.unknown_token_count;
        self.manifest_bypass_count += telemetry.manifest_bypass_count;
        self.fresh_pii_detected_count += telemetry.fresh_pii_detected_count;
    }
}

/// Replay every successful tool call in a session.
///
/// `retention_days` is the active profile's `snapshot_retention_days` value
/// (or `0` when the profile has no retention configured). It is propagated
/// into [`LensError::SnapshotPurged`] when a tombstoned row is encountered,
/// so the operator sees the concrete policy that retired the mappings
/// rather than a placeholder.
pub fn restore_whole_session(
    manifest_path: &Path,
    lens_session_id: &str,
    retention_days: u32,
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
    let mut restore_telemetry_summary = RestoreTelemetrySummary::default();
    let pipeline = super::default_pipeline()?;
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
                retention_days,
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
        let (restored_args_json, restore_telemetry) = restore_tokens(
            lens_session_id,
            &pipeline,
            &gaze_session,
            &redacted_args.json,
        )?;
        restore_telemetry_summary.record(&restore_telemetry);
        calls.push(RestoredCall {
            call_id,
            tool_name,
            redacted_args_json,
            restored_args_json,
            snapshot_ref,
            restore_telemetry,
        });
    }

    Ok(RestoredSession {
        lens_session_id: lens_session_id.to_string(),
        calls,
        restore_telemetry_summary,
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

fn restore_tokens(
    lens_session_id: &str,
    pipeline: &gaze::Pipeline,
    gaze_session: &gaze::Session,
    input: &str,
) -> Result<(String, gaze::RestoreTelemetry), LensError> {
    let (restored, telemetry) = pipeline
        .restore_with_policy_telemetry(gaze_session, input, gaze::RestorePolicy::Strict)
        .map_err(|err| LensError::ReplayUnavailable {
            lens_session_id: lens_session_id.to_string(),
            detail: err.to_string(),
        })?;
    Ok((restored.text, telemetry))
}
