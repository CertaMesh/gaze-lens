use std::path::PathBuf;

use clap::Args;

use crate::errors::LensError;
use crate::session::restore::restore_whole_session;

#[derive(Debug, Args)]
pub struct ReplayArgs {
    pub session_ulid: String,
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
    let restored = restore_whole_session(&manifest, &args.session_ulid)?;
    let json = serde_json::to_string_pretty(&restored).map_err(|err| LensError::Internal {
        detail: err.to_string(),
    })?;
    println!("{json}");
    Ok(())
}
