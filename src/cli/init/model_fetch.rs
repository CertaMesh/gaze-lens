//! Local init seams for production NER model provisioning.
//!
//! Phase 1a intentionally ships only traits and fakes. The real installer and
//! verifier arrive with `--fetch-model` in Phase 1b after the shared
//! `gaze-model-setup` release.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use crate::errors::LensError;

const MODEL_PATH_SUFFIX: &[&str] = &["gaze", "models", "kiji-distilbert"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProvisionOutcome {
    AlreadyPresent { model_dir: PathBuf },
    Installed { model_dir: PathBuf },
}

pub trait ModelProvisioner {
    fn provision(&self, model_dir: Option<&Path>) -> Result<ProvisionOutcome, LensError>;
}

pub trait BundleVerifier {
    fn verify(&self, model_dir: &Path) -> Result<(), LensError>;
}

pub fn resolve_model_dir(explicit: Option<&Path>) -> Result<PathBuf, LensError> {
    resolve_model_dir_from_env(
        explicit,
        std::env::var_os("XDG_DATA_HOME").map(PathBuf::from),
        std::env::var_os("HOME").map(PathBuf::from),
    )
}

// TODO(1b): delete in favor of gaze_model_setup::default_kiji_model_dir.
fn resolve_model_dir_from_env(
    explicit: Option<&Path>,
    xdg_data_home: Option<PathBuf>,
    home: Option<PathBuf>,
) -> Result<PathBuf, LensError> {
    if let Some(path) = explicit {
        return Ok(path.to_path_buf());
    }
    if let Some(base) = xdg_data_home.filter(|path| !path.as_os_str().is_empty()) {
        return Ok(append_model_suffix(base));
    }
    let home = home
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or_else(|| LensError::Profile {
            detail: "HOME unset; cannot resolve default model dir".into(),
        })?;
    Ok(append_model_suffix(home.join(".local").join("share")))
}

fn append_model_suffix(mut base: PathBuf) -> PathBuf {
    for component in MODEL_PATH_SUFFIX {
        base.push(component);
    }
    base
}

#[doc(hidden)]
pub struct FakeProvisioner {
    outcome: RefCell<Option<Result<ProvisionOutcome, LensError>>>,
}

impl FakeProvisioner {
    pub fn outcome(outcome: ProvisionOutcome) -> Self {
        Self {
            outcome: RefCell::new(Some(Ok(outcome))),
        }
    }

    pub fn err(detail: impl Into<String>) -> Self {
        Self {
            outcome: RefCell::new(Some(Err(LensError::Profile {
                detail: detail.into(),
            }))),
        }
    }
}

impl ModelProvisioner for FakeProvisioner {
    fn provision(&self, _model_dir: Option<&Path>) -> Result<ProvisionOutcome, LensError> {
        self.outcome.borrow_mut().take().unwrap_or_else(|| {
            Err(LensError::Internal {
                detail: "fake provisioner has no scripted outcome".into(),
            })
        })
    }
}

#[doc(hidden)]
pub struct FakeVerifier {
    result: RefCell<Option<Result<(), LensError>>>,
}

impl FakeVerifier {
    pub fn ok() -> Self {
        Self {
            result: RefCell::new(Some(Ok(()))),
        }
    }

    pub fn err(detail: impl Into<String>) -> Self {
        Self {
            result: RefCell::new(Some(Err(LensError::Profile {
                detail: detail.into(),
            }))),
        }
    }
}

impl BundleVerifier for FakeVerifier {
    fn verify(&self, _model_dir: &Path) -> Result<(), LensError> {
        self.result.borrow_mut().take().unwrap_or_else(|| {
            Err(LensError::Internal {
                detail: "fake verifier has no scripted result".into(),
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_model_dir_uses_xdg_then_home() {
        let xdg = resolve_model_dir_from_env(
            None,
            Some(PathBuf::from("/xdg-data")),
            Some(PathBuf::from("/home/operator")),
        )
        .unwrap();
        assert_eq!(xdg, PathBuf::from("/xdg-data/gaze/models/kiji-distilbert"));

        let home =
            resolve_model_dir_from_env(None, None, Some(PathBuf::from("/home/operator"))).unwrap();
        assert_eq!(
            home,
            PathBuf::from("/home/operator/.local/share/gaze/models/kiji-distilbert")
        );
    }

    #[test]
    fn resolve_model_dir_prefers_explicit_path() {
        let resolved = resolve_model_dir_from_env(
            Some(Path::new("/explicit/model")),
            Some(PathBuf::from("/xdg-data")),
            Some(PathBuf::from("/home/operator")),
        )
        .unwrap();
        assert_eq!(resolved, PathBuf::from("/explicit/model"));
    }

    #[test]
    fn fake_provisioner_returns_scripted_outcome() {
        let expected = ProvisionOutcome::AlreadyPresent {
            model_dir: PathBuf::from("/models/kiji"),
        };
        let fake = FakeProvisioner::outcome(expected.clone());

        let actual = fake.provision(None).unwrap();

        assert_eq!(actual, expected);
    }

    #[test]
    fn fake_verifier_returns_scripted_error() {
        let fake = FakeVerifier::err("bundle invalid");

        let err = fake.verify(Path::new("/models/kiji")).unwrap_err();

        assert!(err.to_string().contains("bundle invalid"));
    }
}
