use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection};

use crate::errors::{sanitize_error, LensError};

use super::{RedactedToolArgs, ResultSummary, ToolCall};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SnapshotRef {
    pub path: PathBuf,
}

pub trait ManifestStore: Send + Sync {
    fn begin_call(&self, call: &ToolCall, redacted_args: &RedactedToolArgs)
        -> Result<(), LensError>;
    fn finish_call(
        &self,
        call_id: &str,
        summary: &ResultSummary,
        snapshot_ref: &SnapshotRef,
    ) -> Result<(), LensError>;
    fn fail_call(&self, call_id: &str, err: &LensError) -> Result<(), LensError>;
}

pub struct ManifestWriter {
    path: PathBuf,
    lens_session_id: ulid::Ulid,
    conn: Mutex<Connection>,
}

impl ManifestWriter {
    pub fn new(
        path: &Path,
        lens_session_id: ulid::Ulid,
        gaze_audit_session_id: &str,
    ) -> Result<Self, LensError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|err| LensError::ManifestBeginFailed {
                call_id: "session".to_string(),
                detail: err.to_string(),
                path: Some(path.to_path_buf()),
            })?;
        }
        let conn = Connection::open(path).map_err(|err| LensError::ManifestBeginFailed {
            call_id: "session".to_string(),
            detail: err.to_string(),
            path: Some(path.to_path_buf()),
        })?;
        initialize_schema(&conn).map_err(|err| LensError::ManifestBeginFailed {
            call_id: "session".to_string(),
            detail: err.to_string(),
            path: Some(path.to_path_buf()),
        })?;
        conn.execute(
            "INSERT OR IGNORE INTO sessions (lens_session_id, gaze_audit_session_id, created_at_ms)
             VALUES (?1, ?2, ?3)",
            params![
                lens_session_id.to_string(),
                gaze_audit_session_id,
                current_epoch_ms()
            ],
        )
        .map_err(|err| LensError::ManifestBeginFailed {
            call_id: "session".to_string(),
            detail: err.to_string(),
            path: Some(path.to_path_buf()),
        })?;

        Ok(Self {
            path: path.to_path_buf(),
            lens_session_id,
            conn: Mutex::new(conn),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl ManifestStore for ManifestWriter {
    fn begin_call(
        &self,
        call: &ToolCall,
        redacted_args: &RedactedToolArgs,
    ) -> Result<(), LensError> {
        let redacted_args_json =
            serde_json::to_string(redacted_args).map_err(|err| LensError::ManifestBeginFailed {
                call_id: call.call_id.clone(),
                detail: err.to_string(),
                path: Some(self.path.clone()),
            })?;
        self.conn
            .lock()
            .expect("manifest connection lock")
            .execute(
                "INSERT INTO calls (
                    call_id, lens_session_id, tool_name, redacted_args_json, status,
                    started_at_ms
                ) VALUES (?1, ?2, ?3, ?4, 'in-progress', ?5)",
                params![
                    call.call_id,
                    self.lens_session_id.to_string(),
                    call.tool_name,
                    redacted_args_json,
                    current_epoch_ms()
                ],
            )
            .map(|_| ())
            .map_err(|err| LensError::ManifestBeginFailed {
                call_id: call.call_id.clone(),
                detail: err.to_string(),
                path: Some(self.path.clone()),
            })
    }

    fn finish_call(
        &self,
        call_id: &str,
        summary: &ResultSummary,
        snapshot_ref: &SnapshotRef,
    ) -> Result<(), LensError> {
        let summary_json =
            serde_json::to_string(summary).map_err(|err| LensError::ManifestFinishFailed {
                call_id: call_id.to_string(),
                detail: err.to_string(),
                path: Some(self.path.clone()),
            })?;
        let updated = self
            .conn
            .lock()
            .expect("manifest connection lock")
            .execute(
                "UPDATE calls
                 SET status = 'ok', result_summary = ?2, snapshot_ref = ?3, finished_at_ms = ?4
                 WHERE call_id = ?1",
                params![
                    call_id,
                    summary_json,
                    snapshot_ref.path.to_string_lossy(),
                    current_epoch_ms()
                ],
            )
            .map_err(|err| LensError::ManifestFinishFailed {
                call_id: call_id.to_string(),
                detail: err.to_string(),
                path: Some(self.path.clone()),
            })?;
        if updated == 1 {
            Ok(())
        } else {
            Err(LensError::ManifestFinishFailed {
                call_id: call_id.to_string(),
                detail: "call not found".to_string(),
                path: Some(self.path.clone()),
            })
        }
    }

    fn fail_call(&self, call_id: &str, err: &LensError) -> Result<(), LensError> {
        let safe = sanitize_error(err);
        self.conn
            .lock()
            .expect("manifest connection lock")
            .execute(
                "UPDATE calls
                 SET status = 'error', result_summary = ?2, finished_at_ms = ?3
                 WHERE call_id = ?1",
                params![call_id, safe, current_epoch_ms()],
            )
            .map(|_| ())
            .map_err(|err| LensError::ManifestFinishFailed {
                call_id: call_id.to_string(),
                detail: err.to_string(),
                path: Some(self.path.clone()),
            })
    }
}

pub fn current_epoch_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

pub fn initialize_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS sessions (
            lens_session_id TEXT PRIMARY KEY,
            gaze_audit_session_id TEXT NOT NULL,
            created_at_ms INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS calls (
            call_id TEXT PRIMARY KEY,
            lens_session_id TEXT NOT NULL,
            tool_name TEXT NOT NULL,
            redacted_args_json TEXT NOT NULL,
            status TEXT NOT NULL,
            result_summary TEXT,
            snapshot_ref TEXT,
            started_at_ms INTEGER NOT NULL,
            finished_at_ms INTEGER
        );
        "#,
    )
}
