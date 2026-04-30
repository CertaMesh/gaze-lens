//! PR 6 — snapshot retention TTL with `purged_at_ms` tombstone.
//!
//! Tests cover the AutoPurge {Off, Warn, Purge} sweep behavior, the
//! profile-merge cap (project-opt-in for destructive operations), the
//! v2→v3 schema migration, and the honest replay error path.

use std::path::{Path, PathBuf};

use gaze_lens::cli::retention::apply_retention_policy;
use gaze_lens::cli::serve::apply_multi_profile_retention;
use gaze_lens::errors::LensError;
use gaze_lens::profile::{Profile, SourceSpec};
use gaze_lens::session::maintenance::{AutoPurge, ManifestMaintenance};
use gaze_lens::session::manifest::initialize_schema;
use gaze_lens::session::restore::restore_whole_session;
use rusqlite::{Connection, params};

const MS_PER_DAY: i64 = 86_400_000;

fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

/// Construct a ULID whose 48-bit timestamp is exactly `ms_since_epoch`.
fn ulid_at_ms(ms_since_epoch: i64) -> ulid::Ulid {
    let ms = ms_since_epoch.max(0) as u64;
    ulid::Ulid::from_parts(ms, 0)
}

/// Initialise a fresh manifest at v3, plus an empty snapshot directory.
struct Fixture {
    _temp: tempfile::TempDir,
    manifest: PathBuf,
    snapshots: PathBuf,
    /// Override `.last_retention_warn` so each test runs in isolation
    /// without touching the shared `~/.gaze-lens/` state.
    warn_state: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("tempdir");
        let manifest = temp.path().join("manifest.sqlite");
        let snapshots = temp.path().join("snapshots");
        let warn_state = temp.path().join(".last_retention_warn");
        std::fs::create_dir_all(&snapshots).expect("snapshot dir");
        let conn = Connection::open(&manifest).expect("open manifest");
        initialize_schema(&conn).expect("init schema");
        Self {
            _temp: temp,
            manifest,
            snapshots,
            warn_state,
        }
    }

    fn open_maintenance(&self) -> ManifestMaintenance {
        ManifestMaintenance::open(&self.manifest, &self.snapshots)
            .expect("open mm")
            .with_warn_state_path(self.warn_state.clone())
    }

    fn insert_call(
        &self,
        call_id: &str,
        lens_session_id: &str,
        snapshot_path: Option<&Path>,
        status: &str,
        started_at_ms: i64,
        purged_at_ms: Option<i64>,
    ) {
        // Sessions row first (referential consistency, even though manifest
        // doesn't enforce FK).
        let conn = Connection::open(&self.manifest).expect("open manifest");
        conn.execute(
            "INSERT OR IGNORE INTO sessions (lens_session_id, gaze_audit_session_id, created_at_ms)
             VALUES (?1, ?2, ?3)",
            params![lens_session_id, "audit-stub", started_at_ms],
        )
        .expect("insert session");
        conn.execute(
            "INSERT INTO calls (
                call_id, lens_session_id, tool_name, redacted_args_json, status,
                snapshot_ref, started_at_ms, purged_at_ms
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                call_id,
                lens_session_id,
                "query",
                "{\"json\":\"\"}",
                status,
                snapshot_path.map(|p| p.to_string_lossy().to_string()),
                started_at_ms,
                purged_at_ms,
            ],
        )
        .expect("insert call");
    }

    fn write_snapshot_file(&self, lens_session_id: &str) -> PathBuf {
        let path = self.snapshots.join(format!("{lens_session_id}.snap"));
        std::fs::write(&path, b"stub").expect("write snapshot");
        path
    }

    fn read_call(&self, call_id: &str) -> (Option<String>, Option<i64>) {
        let conn = Connection::open(&self.manifest).expect("open manifest");
        conn.query_row(
            "SELECT snapshot_ref, purged_at_ms FROM calls WHERE call_id = ?1",
            params![call_id],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                ))
            },
        )
        .expect("read call")
    }
}

fn profile_with_retention(retention_days: Option<u32>, auto_purge: AutoPurge) -> Profile {
    Profile {
        name: "retention-test".to_string(),
        source: SourceSpec::Sqlite {
            path: PathBuf::from("/tmp/unused.sqlite"),
            readonly_required: true,
            json_text_columns: Vec::new(),
        },
        policy: None,
        discovered_from_ssh_host: None,
        discovered_from_path: None,
        discovered_at: None,
        discovered_ssh_host_key_fingerprint: None,
        credential_class: None,
        schema_allowlist: None,
        snapshot_retention_days: retention_days,
        auto_purge,
    }
}

fn named_profile_with_retention(
    name: &str,
    retention_days: Option<u32>,
    auto_purge: AutoPurge,
) -> Profile {
    Profile {
        name: name.to_string(),
        source: SourceSpec::Sqlite {
            path: PathBuf::from("/tmp/unused.sqlite"),
            readonly_required: true,
            json_text_columns: Vec::new(),
        },
        policy: None,
        discovered_from_ssh_host: None,
        discovered_from_path: None,
        discovered_at: None,
        discovered_ssh_host_key_fingerprint: None,
        credential_class: None,
        schema_allowlist: None,
        snapshot_retention_days: retention_days,
        auto_purge,
    }
}

#[test]
fn sweep_purge_tombstones_and_removes_files() {
    let fx = Fixture::new();
    let now = now_ms();
    let old_ulid = ulid_at_ms(now - (10 * MS_PER_DAY));
    let id = old_ulid.to_string();
    let snap = fx.write_snapshot_file(&id);
    fx.insert_call(
        "call-old",
        &id,
        Some(&snap),
        "ok",
        now - (10 * MS_PER_DAY),
        None,
    );

    let mm = fx.open_maintenance();
    let report = mm
        .sweep_expired_snapshots(7, AutoPurge::Purge)
        .expect("sweep");
    assert_eq!(report.purged.len(), 1);
    assert!(report.would_purge.is_empty());
    assert!(report.failed.is_empty());

    let (snapshot_ref, purged_at_ms) = fx.read_call("call-old");
    assert!(
        snapshot_ref.is_none(),
        "snapshot_ref should be NULL after tombstone"
    );
    assert!(purged_at_ms.is_some(), "purged_at_ms should be set");
    assert!(!snap.exists(), "snapshot file should be removed");
}

#[test]
fn sweep_purge_idempotent_on_missing_file() {
    // A previous sweep (or operator) already deleted the snapshot file but
    // the manifest row still references it (purged_at_ms NULL). The next
    // sweep must tombstone the row without erroring on the missing file.
    let fx = Fixture::new();
    let now = now_ms();
    let id = ulid_at_ms(now - (10 * MS_PER_DAY)).to_string();
    let snap = fx.snapshots.join(format!("{id}.snap"));
    // Intentionally do NOT create the snapshot file.
    fx.insert_call(
        "call-missing",
        &id,
        Some(&snap),
        "ok",
        now - (10 * MS_PER_DAY),
        None,
    );

    let mm = fx.open_maintenance();
    let report = mm
        .sweep_expired_snapshots(7, AutoPurge::Purge)
        .expect("sweep");
    assert_eq!(report.purged.len(), 1, "row tombstoned despite ENOENT");
    assert!(report.failed.is_empty(), "ENOENT must NOT count as failure");

    let (snapshot_ref, purged_at_ms) = fx.read_call("call-missing");
    assert!(snapshot_ref.is_none());
    assert!(purged_at_ms.is_some());
}

#[test]
fn sweep_warn_emits_warning_no_mutation() {
    let fx = Fixture::new();
    let now = now_ms();
    let old_ulid = ulid_at_ms(now - (10 * MS_PER_DAY));
    let id = old_ulid.to_string();
    let snap = fx.write_snapshot_file(&id);
    fx.insert_call(
        "call-old",
        &id,
        Some(&snap),
        "ok",
        now - (10 * MS_PER_DAY),
        None,
    );

    let mm = fx.open_maintenance();
    let report = mm
        .sweep_expired_snapshots(7, AutoPurge::Warn)
        .expect("sweep");
    assert_eq!(report.would_purge.len(), 1);
    assert!(report.purged.is_empty());
    assert!(
        report.warning_emitted,
        "first warn invocation should emit the warning"
    );
    assert!(
        fx.warn_state.exists(),
        "warn-state file should be touched after emission"
    );

    let (snapshot_ref, purged_at_ms) = fx.read_call("call-old");
    assert!(snapshot_ref.is_some(), "snapshot_ref should remain set");
    assert!(purged_at_ms.is_none(), "purged_at_ms should remain NULL");
    assert!(snap.exists(), "snapshot file should remain on disk");
}

#[test]
fn sweep_warn_per_day_suppression() {
    let fx = Fixture::new();
    let now = now_ms();
    let old_ulid = ulid_at_ms(now - (10 * MS_PER_DAY));
    let id = old_ulid.to_string();
    let snap = fx.write_snapshot_file(&id);
    fx.insert_call(
        "call-old",
        &id,
        Some(&snap),
        "ok",
        now - (10 * MS_PER_DAY),
        None,
    );

    let mm = fx.open_maintenance();
    let first = mm
        .sweep_expired_snapshots(7, AutoPurge::Warn)
        .expect("first sweep");
    assert!(first.warning_emitted, "first invocation must warn");

    let second = mm
        .sweep_expired_snapshots(7, AutoPurge::Warn)
        .expect("second sweep");
    assert_eq!(
        second.would_purge.len(),
        1,
        "scan still finds the expired entry"
    );
    assert!(
        !second.warning_emitted,
        "second invocation within 24h must be suppressed"
    );
}

#[test]
fn sweep_off_is_noop() {
    let fx = Fixture::new();
    let now = now_ms();
    let id = ulid_at_ms(now - (30 * MS_PER_DAY)).to_string();
    let snap = fx.write_snapshot_file(&id);
    fx.insert_call(
        "call-ancient",
        &id,
        Some(&snap),
        "ok",
        now - (30 * MS_PER_DAY),
        None,
    );

    let mm = fx.open_maintenance();
    let report = mm
        .sweep_expired_snapshots(7, AutoPurge::Off)
        .expect("sweep");
    assert!(report.is_empty(), "Off mode produces empty report");
    assert!(report.purged.is_empty());
    assert!(report.would_purge.is_empty());
    assert!(!report.warning_emitted);
    assert!(
        !fx.warn_state.exists(),
        "Off must not touch the warn-state file"
    );

    let (snapshot_ref, purged_at_ms) = fx.read_call("call-ancient");
    assert!(snapshot_ref.is_some());
    assert!(purged_at_ms.is_none());
    assert!(snap.exists());
}

#[test]
fn replay_returns_snapshot_purged_for_tombstoned_row() {
    let fx = Fixture::new();
    let now = now_ms();
    let id = ulid_at_ms(now - (30 * MS_PER_DAY)).to_string();
    let purged_at = now - MS_PER_DAY;
    fx.insert_call(
        "call-tombstoned",
        &id,
        None,
        "ok",
        now - (30 * MS_PER_DAY),
        Some(purged_at),
    );

    let err = restore_whole_session(&fx.manifest, &id, 14)
        .expect_err("replay against tombstoned row should error");
    match err {
        LensError::SnapshotPurged {
            lens_session_id,
            purged_at_ms,
            purged_at_iso8601,
            retention_days,
        } => {
            assert_eq!(lens_session_id, id);
            assert_eq!(purged_at_ms, purged_at);
            assert!(
                purged_at_iso8601.contains('T') && purged_at_iso8601.ends_with('Z'),
                "iso8601 should be RFC3339-shaped: {purged_at_iso8601}"
            );
            assert_eq!(
                retention_days, 14,
                "concrete retention_days must come through to the error"
            );
        }
        other => panic!("expected SnapshotPurged, got {other:?}"),
    }
}

#[test]
fn status_not_ok_rows_ignored_by_replay() {
    let fx = Fixture::new();
    let now = now_ms();
    let id = ulid_at_ms(now).to_string();
    fx.insert_call("call-err", &id, None, "error", now, None);

    let restored = restore_whole_session(&fx.manifest, &id, 0)
        .expect("replay should succeed even with only error rows");
    assert!(
        restored.calls.is_empty(),
        "error rows must not appear in restored calls"
    );
}

#[test]
fn default_unlimited_is_no_op() {
    let fx = Fixture::new();
    let now = now_ms();
    let id = ulid_at_ms(now - (365 * MS_PER_DAY)).to_string();
    let snap = fx.write_snapshot_file(&id);
    fx.insert_call(
        "call-ancient",
        &id,
        Some(&snap),
        "ok",
        now - (365 * MS_PER_DAY),
        None,
    );

    // No retention configured → apply_retention_policy is a no-op even with
    // a one-year-old snapshot present. D3 default behaviour preserved.
    let profile = profile_with_retention(None, AutoPurge::Off);
    apply_retention_policy(&profile, &fx.manifest, &fx.snapshots).expect("apply policy");

    let (snapshot_ref, purged_at_ms) = fx.read_call("call-ancient");
    assert!(
        snapshot_ref.is_some(),
        "ancient snapshot must remain referenced"
    );
    assert!(
        purged_at_ms.is_none(),
        "ancient snapshot must not be tombstoned"
    );
    assert!(snap.exists(), "snapshot file must remain on disk");
}

#[test]
fn multi_profile_retention_uses_min_days() {
    let fx = Fixture::new();
    let now = now_ms();
    let id = ulid_at_ms(now - (5 * MS_PER_DAY)).to_string();
    let snap = fx.write_snapshot_file(&id);
    fx.insert_call(
        "call-min",
        &id,
        Some(&snap),
        "ok",
        now - (5 * MS_PER_DAY),
        None,
    );
    let profiles = vec![
        named_profile_with_retention("short", Some(3), AutoPurge::Purge),
        named_profile_with_retention("long", Some(7), AutoPurge::Purge),
    ];

    apply_multi_profile_retention(&profiles, &fx.manifest, &fx.snapshots)
        .expect("multi profile retention");

    let (snapshot_ref, purged_at_ms) = fx.read_call("call-min");
    assert!(
        snapshot_ref.is_none(),
        "MIN(3, 7) should purge 5-day snapshot"
    );
    assert!(purged_at_ms.is_some());
    assert!(!snap.exists());
}

#[test]
fn multi_profile_auto_purge_caps_to_least_destructive() {
    let fx = Fixture::new();
    let now = now_ms();
    let id = ulid_at_ms(now - (5 * MS_PER_DAY)).to_string();
    let snap = fx.write_snapshot_file(&id);
    fx.insert_call(
        "call-and",
        &id,
        Some(&snap),
        "ok",
        now - (5 * MS_PER_DAY),
        None,
    );
    let profiles = vec![
        named_profile_with_retention("purge", Some(3), AutoPurge::Purge),
        named_profile_with_retention("off", Some(7), AutoPurge::Off),
    ];

    apply_multi_profile_retention(&profiles, &fx.manifest, &fx.snapshots)
        .expect("multi profile retention");

    let (snapshot_ref, purged_at_ms) = fx.read_call("call-and");
    assert!(
        snapshot_ref.is_some(),
        "Off in the loaded set must prevent purge"
    );
    assert!(purged_at_ms.is_none());
    assert!(snap.exists());
}

#[test]
fn multi_profile_retention_none_does_not_lower_minimum() {
    let fx = Fixture::new();
    let now = now_ms();
    let id = ulid_at_ms(now - MS_PER_DAY).to_string();
    let snap = fx.write_snapshot_file(&id);
    fx.insert_call("call-none", &id, Some(&snap), "ok", now - MS_PER_DAY, None);
    let profiles = vec![
        named_profile_with_retention("unlimited", None, AutoPurge::Purge),
        named_profile_with_retention("seven", Some(7), AutoPurge::Purge),
    ];

    apply_multi_profile_retention(&profiles, &fx.manifest, &fx.snapshots)
        .expect("multi profile retention");

    let (snapshot_ref, purged_at_ms) = fx.read_call("call-none");
    assert!(
        snapshot_ref.is_some(),
        "None must behave as unlimited, leaving MIN at 7 days"
    );
    assert!(purged_at_ms.is_none());
    assert!(snap.exists());
}

#[test]
fn merge_cap_truth_table_when_both_files_define_profile() {
    use gaze_lens::profile::load_profiles;
    use std::fs::write;

    fn project_user_pair(
        project_auto_purge: &str,
        user_auto_purge: &str,
    ) -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let project = dir.path().join("project.toml");
        let user = dir.path().join("user.toml");
        write(
            &project,
            format!(
                r#"
[[profiles]]
name = "p"
auto_purge = "{project_auto_purge}"
[profiles.source]
kind = "sqlite"
path = "/tmp/x.sqlite"
"#
            ),
        )
        .expect("write project");
        write(
            &user,
            format!(
                r#"
[[profiles]]
name = "p"
auto_purge = "{user_auto_purge}"
[profiles.source]
kind = "sqlite"
path = "/tmp/x.sqlite"
"#
            ),
        )
        .expect("write user");
        (dir, project, user)
    }

    // Truth table for `merged = min(project, user)` over Off < Warn < Purge.
    // User can downgrade; user CANNOT escalate above project-authorized value.
    let cases = [
        ("off", "off", AutoPurge::Off),
        ("off", "warn", AutoPurge::Off),
        ("off", "purge", AutoPurge::Off),
        ("warn", "off", AutoPurge::Off),
        ("warn", "warn", AutoPurge::Warn),
        ("warn", "purge", AutoPurge::Warn),
        ("purge", "off", AutoPurge::Off),
        ("purge", "warn", AutoPurge::Warn),
        ("purge", "purge", AutoPurge::Purge),
    ];
    for (project, user, expected) in cases {
        let (_dir, project_path, user_path) = project_user_pair(project, user);
        let profiles = load_profiles(Some(&project_path), Some(&user_path)).expect("load profiles");
        let profile = profiles
            .into_iter()
            .find(|p| p.name == "p")
            .expect("profile p");
        assert_eq!(
            profile.auto_purge, expected,
            "project={project} user={user} -> expected {expected:?}"
        );
    }
}

#[test]
fn auto_purge_user_only_profile_downgrades_to_off() {
    use gaze_lens::profile::{MergeWarningKind, load_profiles_with_warnings};
    use std::fs::write;

    let dir = tempfile::tempdir().expect("tempdir");
    let project = dir.path().join("project.toml");
    let user = dir.path().join("user.toml");
    // Project file is empty (no profiles array).
    write(&project, "").expect("write empty project");
    write(
        &user,
        r#"
[[profiles]]
name = "user-only"
auto_purge = "purge"
[profiles.source]
kind = "sqlite"
path = "/tmp/x.sqlite"
"#,
    )
    .expect("write user");

    let (profiles, warnings) =
        load_profiles_with_warnings(Some(&project), Some(&user)).expect("load profiles");
    let profile = profiles
        .into_iter()
        .find(|p| p.name == "user-only")
        .expect("profile user-only");
    assert_eq!(
        profile.auto_purge,
        AutoPurge::Off,
        "user-only profile must be forced to Off (project opt-in required)"
    );

    // The merge step must emit exactly one warning naming the offending
    // profile and explaining that the destructive purge was downgraded.
    // `MergeWarning::message()` is the same string `load_profiles` writes to
    // stderr, so asserting on it covers the operator-visible warning without
    // needing stderr capture in tests.
    assert_eq!(
        warnings.len(),
        1,
        "expected exactly one user-only downgrade warning, got: {warnings:?}"
    );
    let warning = &warnings[0];
    assert_eq!(
        warning.profile, "user-only",
        "warning must name the downgraded profile"
    );
    assert!(
        matches!(
            warning.kind,
            MergeWarningKind::UserOnlyAutoPurgeDowngrade {
                requested: AutoPurge::Purge
            }
        ),
        "warning must record the originally-requested auto_purge mode, got: {:?}",
        warning.kind
    );
    let message = warning.message();
    assert!(
        message.contains("user-only"),
        "operator-facing message must name the profile: {message}"
    );
    assert!(
        message.contains("auto_purge"),
        "operator-facing message must mention auto_purge: {message}"
    );
    assert!(
        message.contains("project-level opt-in"),
        "operator-facing message must explain destructive-purge opt-in requirement: {message}"
    );
    assert!(
        message.contains("\"off\""),
        "operator-facing message must state the forced auto_purge=off result: {message}"
    );
}

#[test]
fn ulid_timestamp_used_for_age() {
    // Two rows with identical mtimes (write order, not relevant to sweep) but
    // different ULID-embedded timestamps. Sweep must see the OLD-ULID one as
    // expired and the FRESH-ULID one as fresh, independent of file mtime.
    let fx = Fixture::new();
    let now = now_ms();
    let old_id = ulid_at_ms(now - (30 * MS_PER_DAY)).to_string();
    let fresh_id = ulid_at_ms(now - MS_PER_DAY).to_string();
    let old_snap = fx.write_snapshot_file(&old_id);
    let fresh_snap = fx.write_snapshot_file(&fresh_id);
    fx.insert_call("call-old", &old_id, Some(&old_snap), "ok", now, None);
    fx.insert_call("call-fresh", &fresh_id, Some(&fresh_snap), "ok", now, None);

    let mm = fx.open_maintenance();
    let report = mm
        .sweep_expired_snapshots(7, AutoPurge::Warn)
        .expect("sweep");
    assert_eq!(report.would_purge.len(), 1);
    let entry = &report.would_purge[0];
    assert_eq!(entry.lens_session_id, old_id);
    assert!(entry.age_days >= 7);
}

#[test]
fn legacy_v2_manifest_gains_purged_at_ms_column_via_migration() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manifest = dir.path().join("legacy.sqlite");
    {
        let conn = Connection::open(&manifest).expect("open");
        // Synthesize a v0.1.x-shaped v2 manifest: no `purged_at_ms` column,
        // user_version pinned at 2.
        conn.execute_batch(
            r#"
            CREATE TABLE sessions (
                lens_session_id TEXT PRIMARY KEY,
                gaze_audit_session_id TEXT NOT NULL,
                created_at_ms INTEGER NOT NULL
            );
            CREATE TABLE calls (
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
            PRAGMA user_version = 2;
            "#,
        )
        .expect("legacy schema");
        conn.execute(
            "INSERT INTO sessions (lens_session_id, gaze_audit_session_id, created_at_ms)
             VALUES ('legacy-session', 'legacy-audit', 0)",
            [],
        )
        .expect("legacy session row");
        conn.execute(
            "INSERT INTO calls (call_id, lens_session_id, tool_name, redacted_args_json, status, started_at_ms)
             VALUES ('legacy-call', 'legacy-session', 'query', '{}', 'ok', 0)",
            [],
        )
        .expect("legacy call row");
    }

    // Run the migration.
    {
        let conn = Connection::open(&manifest).expect("open for migration");
        initialize_schema(&conn).expect("migrate v2→v3");
        let user_version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("user_version");
        assert_eq!(user_version, 3, "user_version must advance to 3");
        let purged: Option<i64> = conn
            .query_row(
                "SELECT purged_at_ms FROM calls WHERE call_id = 'legacy-call'",
                [],
                |row| row.get(0),
            )
            .expect("select purged_at_ms");
        assert!(
            purged.is_none(),
            "legacy rows must default to NULL for purged_at_ms"
        );
    }
}
