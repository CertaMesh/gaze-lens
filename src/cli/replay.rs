use std::path::PathBuf;

use clap::Args;

use crate::errors::LensError;
use crate::profile::load_profile;
use crate::session::restore::restore_whole_session;

#[derive(Debug, Args)]
pub struct ReplayArgs {
    pub session_ulid: String,
    #[arg(long, default_value = "default")]
    pub profile: String,
    #[arg(
        long,
        env = "GAZE_LENS_MANIFEST",
        default_value = "~/.gaze-lens/manifest.sqlite"
    )]
    pub manifest: PathBuf,
    #[arg(long)]
    pub call_id: Option<String>,
}

pub fn run(args: ReplayArgs) -> Result<(), LensError> {
    if args.call_id.is_some() {
        return Err(LensError::FeatureDeferred(
            "per-call replay is not in v1; tracked as v1.x candidate".to_string(),
        ));
    }
    let manifest = shellexpand::full(&args.manifest.to_string_lossy())
        .map(|path| PathBuf::from(path.into_owned()))
        .map_err(|err| LensError::Profile {
            detail: err.to_string(),
        })?;
    // Resolve active profile to surface the concrete retention policy in the
    // SnapshotPurged error message. A missing/unconfigured retention is
    // reported as `0` (i.e. "no retention configured at replay time"), which
    // is honest about the fact that the active profile no longer specifies
    // a policy that explains the purge.
    let retention_days = load_profile(&args.profile, None, None)
        .ok()
        .and_then(|p| p.snapshot_retention_days)
        .unwrap_or(0);
    let restored = restore_whole_session(&manifest, &args.session_ulid, retention_days)?;
    let json = serde_json::to_string_pretty(&restored).map_err(|err| LensError::Internal {
        detail: err.to_string(),
    })?;
    println!("{json}");
    Ok(())
}
