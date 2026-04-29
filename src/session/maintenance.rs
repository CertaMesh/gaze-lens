use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::{Connection, params};

use crate::errors::LensError;

use super::manifest::{current_epoch_ms, initialize_schema};

const MS_PER_DAY: i64 = 86_400_000;

/// Snapshot retention sweeper. Operates on the manifest at rest, separate
/// from `Session::new_with_pipeline`. Construct via [`Self::open`] then
/// call [`Self::sweep_expired_snapshots`] before constructing a session.
pub struct ManifestMaintenance {
    manifest_path: PathBuf,
    snapshot_dir: PathBuf,
    conn: Option<Mutex<Connection>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpiredEntry {
    pub call_id: String,
    pub lens_session_id: String,
    pub snapshot_path: PathBuf,
    pub ulid_ms: i64,
    pub age_days: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PurgedEntry {
    pub call_id: String,
    pub lens_session_id: String,
    pub snapshot_path: PathBuf,
    pub purged_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailedEntry {
    pub call_id: String,
    pub lens_session_id: String,
    pub snapshot_path: PathBuf,
    pub detail: String,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SweepReport {
    pub would_purge: Vec<ExpiredEntry>,
    pub purged: Vec<PurgedEntry>,
    pub failed: Vec<FailedEntry>,
}

impl SweepReport {
    pub fn is_empty(&self) -> bool {
        self.would_purge.is_empty() && self.purged.is_empty() && self.failed.is_empty()
    }

    pub fn oldest_would_purge(&self) -> Option<&ExpiredEntry> {
        self.would_purge.iter().min_by_key(|e| e.ulid_ms)
    }
}

impl ManifestMaintenance {
    pub fn open(manifest_path: &Path, snapshot_dir: &Path) -> Result<Self, LensError> {
        if !manifest_path.exists() {
            return Ok(Self {
                manifest_path: manifest_path.to_path_buf(),
                snapshot_dir: snapshot_dir.to_path_buf(),
                conn: None,
            });
        }
        let conn = Connection::open(manifest_path).map_err(|err| LensError::Internal {
            detail: format!(
                "manifest maintenance open failed for {}: {err}",
                manifest_path.display()
            ),
        })?;
        initialize_schema(&conn).map_err(|err| LensError::Internal {
            detail: format!(
                "manifest maintenance schema init failed for {}: {err}",
                manifest_path.display()
            ),
        })?;
        Ok(Self {
            manifest_path: manifest_path.to_path_buf(),
            snapshot_dir: snapshot_dir.to_path_buf(),
            conn: Some(Mutex::new(conn)),
        })
    }

    pub fn manifest_path(&self) -> &Path {
        &self.manifest_path
    }

    pub fn snapshot_dir(&self) -> &Path {
        &self.snapshot_dir
    }

    pub fn sweep_expired_snapshots(
        &self,
        retention_days: u32,
        auto_purge: bool,
    ) -> Result<SweepReport, LensError> {
        let Some(conn_mutex) = self.conn.as_ref() else {
            return Ok(SweepReport::default());
        };
        let conn = conn_mutex.lock().expect("manifest maintenance lock");
        let now_ms = current_epoch_ms();
        let retention_ms = (retention_days as i64).saturating_mul(MS_PER_DAY);

        let mut stmt = conn
            .prepare(
                "SELECT call_id, lens_session_id, snapshot_ref
                 FROM calls
                 WHERE status = 'ok'
                   AND snapshot_ref IS NOT NULL
                   AND purged_at_ms IS NULL",
            )
            .map_err(|err| LensError::Internal {
                detail: format!("manifest sweep prepare failed: {err}"),
            })?;
        let mut rows = stmt.query([]).map_err(|err| LensError::Internal {
            detail: format!("manifest sweep query failed: {err}"),
        })?;

        let mut report = SweepReport::default();
        while let Some(row) = rows.next().map_err(|err| LensError::Internal {
            detail: format!("manifest sweep iter failed: {err}"),
        })? {
            let call_id: String = row.get(0).map_err(|err| LensError::Internal {
                detail: format!("manifest sweep row.call_id: {err}"),
            })?;
            let lens_session_id: String = row.get(1).map_err(|err| LensError::Internal {
                detail: format!("manifest sweep row.lens_session_id: {err}"),
            })?;
            let snapshot_ref: String = row.get(2).map_err(|err| LensError::Internal {
                detail: format!("manifest sweep row.snapshot_ref: {err}"),
            })?;

            let ulid_ms = match parse_ulid_ms(&lens_session_id) {
                Some(ms) => ms,
                None => {
                    // Malformed lens_session_id — skip; do not tombstone.
                    tracing::warn!(
                        target = "gaze_lens::maintenance",
                        lens_session_id = %lens_session_id,
                        "skipping sweep candidate with non-ULID lens_session_id"
                    );
                    continue;
                }
            };
            if now_ms.saturating_sub(ulid_ms) <= retention_ms {
                continue;
            }
            let age_days = ((now_ms.saturating_sub(ulid_ms)) / MS_PER_DAY).max(0) as u32;
            let snapshot_path = PathBuf::from(snapshot_ref);
            let entry = ExpiredEntry {
                call_id: call_id.clone(),
                lens_session_id: lens_session_id.clone(),
                snapshot_path: snapshot_path.clone(),
                ulid_ms,
                age_days,
            };

            if !auto_purge {
                report.would_purge.push(entry);
                continue;
            }

            // auto_purge: best-effort remove_file (ENOENT = already gone, treat as purged).
            match std::fs::remove_file(&snapshot_path) {
                Ok(()) => {}
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => {
                    report.failed.push(FailedEntry {
                        call_id,
                        lens_session_id,
                        snapshot_path,
                        detail: err.to_string(),
                    });
                    continue;
                }
            }

            // Tombstone: UPDATE not DELETE — D3 audit-of-record invariant.
            let purged_at_ms = current_epoch_ms();
            conn.execute(
                "UPDATE calls
                 SET purged_at_ms = ?2, snapshot_ref = NULL
                 WHERE call_id = ?1",
                params![&entry.call_id, purged_at_ms],
            )
            .map_err(|err| LensError::Internal {
                detail: format!("manifest tombstone UPDATE failed: {err}"),
            })?;

            report.purged.push(PurgedEntry {
                call_id: entry.call_id,
                lens_session_id: entry.lens_session_id,
                snapshot_path: entry.snapshot_path,
                purged_at_ms,
            });
        }

        Ok(report)
    }
}

fn parse_ulid_ms(s: &str) -> Option<i64> {
    let ulid = ulid::Ulid::from_string(s).ok()?;
    let ms = ulid.timestamp_ms();
    if ms > i64::MAX as u64 {
        return None;
    }
    Some(ms as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ulid_ms_extracts_timestamp() {
        let known_ms: u64 = 1_700_000_000_000;
        let ulid = ulid::Ulid::from_parts(known_ms, 0);
        let s = ulid.to_string();
        assert_eq!(parse_ulid_ms(&s), Some(known_ms as i64));
    }

    #[test]
    fn parse_ulid_ms_rejects_non_ulid() {
        assert_eq!(parse_ulid_ms("not-a-ulid"), None);
    }

    #[test]
    fn open_with_missing_manifest_returns_empty_sweeper() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = dir.path().join("absent.sqlite");
        let snap = dir.path().join("snap");
        let mm = ManifestMaintenance::open(&manifest, &snap).unwrap();
        let report = mm.sweep_expired_snapshots(7, true).unwrap();
        assert!(report.is_empty());
    }
}
