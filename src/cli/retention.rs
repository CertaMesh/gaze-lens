use std::path::Path;

use crate::errors::LensError;
use crate::profile::Profile;
use crate::session::maintenance::{AutoPurge, ManifestMaintenance, SweepReport};

/// Apply the profile's snapshot retention policy by sweeping the manifest at rest
/// BEFORE constructing a session. No-op when `snapshot_retention_days` is `None`
/// or `auto_purge` is [`AutoPurge::Off`].
///
/// Stderr behavior:
/// - `AutoPurge::Off`: silent (no scan, no message).
/// - `AutoPurge::Warn`: per-day-suppressed warning emitted by
///   [`ManifestMaintenance::sweep_expired_snapshots`].
/// - `AutoPurge::Purge`: `info!` line with purged count when non-empty.
pub fn apply_retention_policy(
    profile: &Profile,
    manifest_path: &Path,
    snapshot_dir: &Path,
) -> Result<(), LensError> {
    let Some(retention_days) = profile.snapshot_retention_days else {
        return Ok(());
    };
    if matches!(profile.auto_purge, AutoPurge::Off) {
        return Ok(());
    }
    let maintenance = ManifestMaintenance::open(manifest_path, snapshot_dir)?;
    let report = maintenance.sweep_expired_snapshots(retention_days, profile.auto_purge)?;
    if matches!(profile.auto_purge, AutoPurge::Purge) && !report.is_empty() {
        report_purged(&report, retention_days);
    }
    Ok(())
}

fn report_purged(report: &SweepReport, retention_days: u32) {
    let n = report.purged.len();
    let failed = report.failed.len();
    eprintln!(
        "gaze-lens: purged {n} expired snapshot(s) (retention: {retention_days} days){extra}",
        extra = if failed > 0 {
            format!("; {failed} failed")
        } else {
            String::new()
        }
    );
    tracing::info!(
        target = "gaze_lens::retention",
        purged = n,
        failed = failed,
        retention_days,
        "snapshot retention sweep purged expired entries"
    );
}
