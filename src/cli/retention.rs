use std::path::Path;

use time::OffsetDateTime;

use crate::errors::LensError;
use crate::profile::Profile;
use crate::session::maintenance::{ManifestMaintenance, SweepReport};

/// Apply the profile's snapshot retention policy by sweeping the manifest at rest
/// BEFORE constructing a session. No-op when `snapshot_retention_days` is `None`.
///
/// Reports to stderr:
/// - `auto_purge = true` and report non-empty: `info!` line with purged count.
/// - `auto_purge = false` and report non-empty: stderr warning, suppressed
///   per-day per-profile via touch-file under the snapshot dir's parent.
pub fn apply_retention_policy(
    profile: &Profile,
    manifest_path: &Path,
    snapshot_dir: &Path,
) -> Result<(), LensError> {
    let Some(retention_days) = profile.snapshot_retention_days else {
        return Ok(());
    };
    let maintenance = ManifestMaintenance::open(manifest_path, snapshot_dir)?;
    let report = maintenance.sweep_expired_snapshots(retention_days, profile.auto_purge)?;
    if report.is_empty() {
        return Ok(());
    }
    if profile.auto_purge {
        report_purged(&report, retention_days);
    } else {
        report_warn_only(&report, retention_days, &profile.name, snapshot_dir);
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

fn report_warn_only(
    report: &SweepReport,
    retention_days: u32,
    profile_name: &str,
    snapshot_dir: &Path,
) {
    let today = today_yyyy_mm_dd();
    let parent = snapshot_dir.parent().unwrap_or(snapshot_dir).to_path_buf();
    let marker = parent.join(format!(".warned-{today}-{profile_name}"));
    let suppress = marker.exists();

    let n = report.would_purge.len();
    let oldest = report.oldest_would_purge();
    if !suppress {
        let oldest_repr = oldest
            .map(|e| format!("{} (age {} days)", e.lens_session_id, e.age_days))
            .unwrap_or_else(|| "<unknown>".to_string());
        eprintln!(
            "gaze-lens: warning — {n} snapshot(s) older than {retention_days} days are eligible for purge \
             but auto_purge is disabled for profile `{profile_name}`. Oldest: {oldest_repr}. \
             Set `auto_purge = true` in the project profile to enable destructive cleanup."
        );
        // Touch marker. Best-effort — failures don't block the CLI.
        let _ = std::fs::create_dir_all(&parent);
        let _ = std::fs::write(&marker, b"");
    }
    tracing::debug!(
        target = "gaze_lens::retention",
        would_purge = n,
        suppressed = suppress,
        retention_days,
        profile = profile_name,
        "snapshot retention sweep produced warn-only report"
    );
}

fn today_yyyy_mm_dd() -> String {
    let d = OffsetDateTime::now_utc().date();
    format!("{:04}-{:02}-{:02}", d.year(), u8::from(d.month()), d.day())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn today_format_is_yyyy_mm_dd() {
        let s = today_yyyy_mm_dd();
        assert_eq!(s.len(), 10);
        assert_eq!(s.chars().nth(4), Some('-'));
        assert_eq!(s.chars().nth(7), Some('-'));
    }
}
