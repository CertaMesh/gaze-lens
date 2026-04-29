use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::{Connection, params};
use serde::Deserialize;

use crate::errors::LensError;

use super::manifest::{current_epoch_ms, initialize_schema};

const MS_PER_DAY: i64 = 86_400_000;
const WARN_SUPPRESSION_MS: i64 = 24 * 3_600_000;
const WARN_STATE_FILENAME: &str = ".last_retention_warn";

/// Three-state destructive-action policy for the snapshot retention sweep.
///
/// `Off` is the default and skips the sweep entirely. `Warn` performs a
/// read-only scan and emits a per-day-suppressed warning. `Purge` performs
/// a destructive sweep that tombstones manifest rows and removes files.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AutoPurge {
    #[default]
    Off,
    Warn,
    Purge,
}

impl AutoPurge {
    /// Ordering rank for the project/user merge cap: `Off < Warn < Purge`.
    /// The merged value is `min(project, user)` — user can opt down to a less
    /// destructive mode, but never escalate above what project has authorized.
    pub fn rank(self) -> u8 {
        match self {
            AutoPurge::Off => 0,
            AutoPurge::Warn => 1,
            AutoPurge::Purge => 2,
        }
    }

    /// Returns the less destructive of the two values (`min(self, other)`).
    pub fn cap_with(self, other: AutoPurge) -> AutoPurge {
        if self.rank() <= other.rank() {
            self
        } else {
            other
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            AutoPurge::Off => "off",
            AutoPurge::Warn => "warn",
            AutoPurge::Purge => "purge",
        }
    }
}

/// Snapshot retention sweeper. Operates on the manifest at rest, separate
/// from `Session::new_with_pipeline`. Construct via [`Self::open`] then
/// call [`Self::sweep_expired_snapshots`] before constructing a session.
pub struct ManifestMaintenance {
    manifest_path: PathBuf,
    snapshot_dir: PathBuf,
    warn_state_path: PathBuf,
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
    /// Whether the Warn-mode warning was actually emitted this invocation.
    /// `false` when suppressed by the per-day touch-file or when the mode
    /// is not `Warn`.
    pub warning_emitted: bool,
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
        let warn_state_path = default_warn_state_path(manifest_path);
        if !manifest_path.exists() {
            return Ok(Self {
                manifest_path: manifest_path.to_path_buf(),
                snapshot_dir: snapshot_dir.to_path_buf(),
                warn_state_path,
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
            warn_state_path,
            conn: Some(Mutex::new(conn)),
        })
    }

    /// Override the warn-state path for tests so production state at
    /// `~/.gaze-lens/.last_retention_warn` is never touched.
    pub fn with_warn_state_path(mut self, path: PathBuf) -> Self {
        self.warn_state_path = path;
        self
    }

    pub fn manifest_path(&self) -> &Path {
        &self.manifest_path
    }

    pub fn snapshot_dir(&self) -> &Path {
        &self.snapshot_dir
    }

    pub fn warn_state_path(&self) -> &Path {
        &self.warn_state_path
    }

    pub fn sweep_expired_snapshots(
        &self,
        retention_days: u32,
        mode: AutoPurge,
    ) -> Result<SweepReport, LensError> {
        if matches!(mode, AutoPurge::Off) {
            return Ok(SweepReport::default());
        }
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

            match mode {
                AutoPurge::Off => unreachable!("Off short-circuits earlier"),
                AutoPurge::Warn => {
                    report.would_purge.push(entry);
                }
                AutoPurge::Purge => {
                    // Best-effort remove_file (ENOENT = already gone, treat as purged).
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
            }
        }

        if matches!(mode, AutoPurge::Warn) && !report.would_purge.is_empty() {
            report.warning_emitted = self.maybe_emit_warning(&report, retention_days, now_ms);
        }

        Ok(report)
    }

    /// Emit a single per-day-suppressed warning to stderr.
    /// Returns `true` if the warning was actually printed.
    fn maybe_emit_warning(&self, report: &SweepReport, retention_days: u32, now_ms: i64) -> bool {
        let last_ms = read_last_warn_ms(&self.warn_state_path);
        if let Some(last) = last_ms
            && now_ms.saturating_sub(last) < WARN_SUPPRESSION_MS
        {
            tracing::debug!(
                target = "gaze_lens::retention",
                would_purge = report.would_purge.len(),
                suppressed = true,
                retention_days,
                "warn-mode warning suppressed (within 24h of last emission)"
            );
            return false;
        }
        let n = report.would_purge.len();
        let oldest = report
            .oldest_would_purge()
            .map(|e| format!("{} (age {} days)", e.lens_session_id, e.age_days))
            .unwrap_or_else(|| "<unknown>".to_string());
        eprintln!(
            "gaze-lens: warning — {n} snapshot(s) older than {retention_days} days are eligible \
             for purge but auto_purge = warn (no destructive action taken). Oldest: {oldest}. \
             Set `auto_purge = \"purge\"` in the project profile to enable destructive cleanup."
        );
        write_last_warn_ms(&self.warn_state_path, now_ms);
        true
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

fn default_warn_state_path(manifest_path: &Path) -> PathBuf {
    manifest_path
        .parent()
        .map(|p| p.join(WARN_STATE_FILENAME))
        .unwrap_or_else(|| PathBuf::from(WARN_STATE_FILENAME))
}

fn read_last_warn_ms(path: &Path) -> Option<i64> {
    let bytes = std::fs::read_to_string(path).ok()?;
    bytes.trim().parse::<i64>().ok()
}

fn write_last_warn_ms(path: &Path, now_ms: i64) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, now_ms.to_string());
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
        let report = mm.sweep_expired_snapshots(7, AutoPurge::Purge).unwrap();
        assert!(report.is_empty());
    }

    #[test]
    fn auto_purge_cap_with_user_below_project() {
        assert_eq!(AutoPurge::Purge.cap_with(AutoPurge::Warn), AutoPurge::Warn);
        assert_eq!(AutoPurge::Warn.cap_with(AutoPurge::Off), AutoPurge::Off);
    }

    #[test]
    fn auto_purge_cap_with_user_above_project_caps_at_project() {
        assert_eq!(AutoPurge::Off.cap_with(AutoPurge::Purge), AutoPurge::Off);
        assert_eq!(AutoPurge::Warn.cap_with(AutoPurge::Purge), AutoPurge::Warn);
    }

    #[test]
    fn auto_purge_default_is_off() {
        assert_eq!(AutoPurge::default(), AutoPurge::Off);
    }
}
