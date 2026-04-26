use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::Args;

use crate::errors::LensError;
use crate::frontend::mcp::McpFrontend;
use crate::frontend::{Frontend, ShutdownToken};
use crate::profile::load_profile;
use crate::session::Session;

#[derive(Debug, Args)]
pub struct ServeArgs {
    #[arg(long, default_value = "default")]
    pub profile: String,
    #[arg(
        long,
        env = "GAZE_LENS_MANIFEST",
        default_value = "~/.gaze-lens/manifest.sqlite"
    )]
    pub manifest: PathBuf,
    #[arg(
        long,
        env = "GAZE_LENS_SNAPSHOT_DIR",
        default_value = "~/.gaze-lens/snapshots"
    )]
    pub snapshot_dir: PathBuf,
}

pub async fn run(
    args: ServeArgs,
    project_config: Option<&Path>,
    user_config: Option<&Path>,
) -> Result<(), LensError> {
    let profile = load_profile(&args.profile, project_config, user_config)?;
    let manifest = expand_path(&args.manifest)?;
    let snapshot_dir = expand_path(&args.snapshot_dir)?;
    let policy = default_policy();
    let session = Arc::new(Session::new(&policy, &manifest, &snapshot_dir)?);

    // Source construction lands as adapters are wired into profile runtime.
    // PR2a keeps the MCP transport on the shared Session audit path.
    let _profile_name = profile.name;
    McpFrontend::new()
        .serve(session, ShutdownToken)
        .await
        .map_err(|err| LensError::FrontendError {
            frontend: "mcp".to_string(),
            detail: err.to_string(),
        })
}

fn default_policy() -> gaze::Policy {
    gaze::Policy {
        session: gaze::SessionPolicy {
            scope: gaze::SessionScope::Conversation,
            ttl_secs: None,
        },
        detectors: Vec::new(),
        dictionaries: Vec::new(),
        rules: Vec::new(),
        ner: None,
        rulepacks: gaze::RulepackPolicy {
            bundled: vec!["core".to_string()],
            paths: Vec::new(),
        },
        locale: None,
    }
}

fn expand_path(path: &Path) -> Result<PathBuf, LensError> {
    shellexpand::full(&path.to_string_lossy())
        .map(|path| PathBuf::from(path.into_owned()))
        .map_err(|err| LensError::Profile {
            detail: err.to_string(),
        })
}
