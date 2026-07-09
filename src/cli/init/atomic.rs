//! Atomic write + perm helpers for the guided init flow.
//!
//! - `atomic_write` stages a `<target>.tmp.<pid>` file, fsyncs it, renames over
//!   the target, then fsyncs the parent dir (Linux/macOS — best-effort on macOS).
//! - `would_write` byte-compares against an existing file so commit_plan can
//!   skip writes when content is unchanged (CB7).
//! - `create_dir_0700_if_missing` creates a 0o700 dir for gaze-lens-owned paths
//!   (~/.gaze-lens/) only.
//! - `assert_dir_0700_or_warn` is read-only — used on third-party dot-dirs
//!   (~/.codex/, ~/.cursor/) so an operator-set 0o755 mode is left alone (CB8).
//!
//! Unix-only for v0.2.1 (directive 15). Windows tracked for v0.2.2.

#[cfg(not(unix))]
compile_error!("gaze-lens v0.2.1 supports Unix only; Windows tracked for v0.2.2");

use std::fs::{File, OpenOptions, Permissions};
use std::io::{ErrorKind, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

use crate::errors::LensError;

/// Atomically write `contents` to `dest` with mode `0o600`.
///
/// Strategy: create `<dest>.tmp.<pid>` with 0o600, write + flush + fsync, then
/// rename onto `dest`, then fsync the parent dir. On any failure, remove the
/// temp file before returning so we never leave `.tmp.` orphans.
pub fn atomic_write(dest: &Path, contents: &[u8]) -> Result<(), LensError> {
    let parent = dest.parent().ok_or_else(|| LensError::Profile {
        detail: format!("dest has no parent: {}", dest.display()),
    })?;
    let file_name = dest
        .file_name()
        .ok_or_else(|| LensError::Profile {
            detail: format!("dest has no file name: {}", dest.display()),
        })?
        .to_string_lossy()
        .into_owned();
    let tmp = parent.join(format!("{file_name}.tmp.{}", std::process::id()));

    // Write tmp file. On any failure, remove tmp and bail.
    let write_result: std::io::Result<()> = (|| {
        let mut f: File = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(contents)?;
        f.flush()?;
        f.sync_all()?;
        Ok(())
    })();
    if let Err(err) = write_result {
        let _ = std::fs::remove_file(&tmp);
        return Err(LensError::Profile {
            detail: format!("failed to stage {}: {err}", tmp.display()),
        });
    }

    // Rename onto dest. On failure, remove tmp.
    if let Err(err) = std::fs::rename(&tmp, dest) {
        let _ = std::fs::remove_file(&tmp);
        return Err(LensError::Profile {
            detail: format!(
                "failed to rename {} -> {}: {err}",
                tmp.display(),
                dest.display()
            ),
        });
    }

    // fsync parent dir (best-effort on macOS).
    if let Ok(dir) = File::open(parent) {
        let _ = dir.sync_all();
    }
    Ok(())
}

/// Returns true iff writing `new` to `dest` would change the file's bytes.
/// Missing file → true (write needed); other read errors fail closed.
pub fn would_write(dest: &Path, new: &[u8]) -> Result<bool, LensError> {
    match std::fs::read(dest) {
        Ok(existing) => Ok(existing != new),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(true),
        Err(err) => Err(LensError::Profile {
            detail: format!(
                "failed to read existing destination {} before atomic write: {err}",
                dest.display()
            ),
        }),
    }
}

/// Create `dir` with mode `0o700` if it doesn't exist. Idempotent; if the dir
/// already exists, perms are NOT modified — the caller is responsible for
/// confirming via `assert_dir_0700_or_warn` if they care.
///
/// Use this ONLY on gaze-lens-owned paths (~/.gaze-lens/). Third-party dot-dirs
/// must use `assert_dir_0700_or_warn` (read-only, never chmod).
pub fn create_dir_0700_if_missing(dir: &Path) -> Result<(), LensError> {
    if dir.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(dir).map_err(|err| LensError::Profile {
        detail: format!("failed to create {}: {err}", dir.display()),
    })?;
    std::fs::set_permissions(dir, Permissions::from_mode(0o700)).map_err(|err| {
        LensError::Profile {
            detail: format!("failed to chmod 0700 {}: {err}", dir.display()),
        }
    })?;
    Ok(())
}

/// Read-only check on third-party dot-dirs (~/.codex/, ~/.cursor/). If the dir
/// is not 0o700, log a tracing warning but DO NOT modify it. Operator-set modes
/// must be respected (CB8).
pub fn assert_dir_0700_or_warn(dir: &Path) -> Result<(), LensError> {
    let meta = std::fs::metadata(dir).map_err(|err| LensError::Profile {
        detail: format!("failed to stat {}: {err}", dir.display()),
    })?;
    let mode = meta.permissions().mode() & 0o777;
    if mode != 0o700 {
        tracing::warn!(
            target = "gaze_lens::init::atomic",
            dir = %dir.display(),
            mode = format!("{mode:#o}"),
            "third-party dir mode is not 0o700; leaving as set by operator"
        );
    }
    Ok(())
}
