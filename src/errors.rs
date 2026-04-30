use std::path::PathBuf;

use thiserror::Error;

use crate::session::TruncatedAt;
use crate::value::LowerError;

#[derive(Debug, Error)]
pub enum LensError {
    #[error("manifest begin failed for call {call_id}: {detail}")]
    ManifestBeginFailed {
        call_id: String,
        detail: String,
        path: Option<PathBuf>,
    },
    #[error("manifest finish failed for call {call_id}: {detail}")]
    ManifestFinishFailed {
        call_id: String,
        detail: String,
        path: Option<PathBuf>,
    },
    #[error("frontend error from {frontend}: {detail}")]
    FrontendError { frontend: String, detail: String },
    #[error("source error from {source_name}: {detail}")]
    SourceError {
        source_name: String,
        detail: String,
        sql: Option<String>,
        stderr: Option<String>,
    },
    #[error("redaction failed: {detail}")]
    RedactionFailed { detail: String },
    #[error("replay unavailable for {lens_session_id}: {detail}")]
    ReplayUnavailable {
        lens_session_id: String,
        detail: String,
    },
    #[error(
        "snapshot for session {lens_session_id} was purged by retention policy on {purged_at_iso8601} (retention: {retention_days} days). The audit row remains in the manifest; the raw token mappings are not recoverable."
    )]
    SnapshotPurged {
        lens_session_id: String,
        purged_at_ms: i64,
        purged_at_iso8601: String,
        retention_days: u32,
    },
    #[error("scope rejected: {scope}")]
    ScopeRejected { scope: String },
    #[error("conversion failed: {0}")]
    ConvertError(#[from] LowerError),
    #[error("profile environment variable missing: {env}")]
    ProfileEnvMissing { env: String },
    #[error("keyring entry not found: service={service} account={account}")]
    SecretKeyringMissing { service: String, account: String },
    #[error("keyring access denied: service={service} account={account}")]
    SecretKeyringDenied { service: String, account: String },
    #[error("secret backend unavailable ({backend}): {detail}")]
    SecretBackendUnavailable { backend: String, detail: String },
    #[error("profile config not found ({label}): {path}")]
    ProfileNotFound { label: String, path: PathBuf },
    #[error("profile error: {detail}")]
    Profile { detail: String },
    #[error("profile `{profile}` not found; loaded: {loaded:?}")]
    ProfileUnknown {
        profile: String,
        loaded: Vec<String>,
    },
    #[error(
        "profile `{profile}` is a {actual} source; tool `{tool}` requires a {required} profile"
    )]
    ProfileMismatch {
        profile: String,
        tool: String,
        required: String,
        actual: String,
    },
    #[error(
        "batch write partial failure on {failed}: {applied} applied, {pending} pending, {source}",
        failed = .failed.display(),
        applied = .applied.len(),
        pending = .pending.len(),
    )]
    BatchPartial {
        applied: Vec<PathBuf>,
        pending: Vec<PathBuf>,
        failed: PathBuf,
        source: Box<LensError>,
    },
    #[error("feature deferred: {0}")]
    FeatureDeferred(String),
    #[error("output truncated at {0:?}")]
    Truncated(TruncatedAt),
    #[error("internal error: {detail}")]
    Internal { detail: String },
}

impl LensError {
    pub fn is_invalid_params(&self) -> bool {
        matches!(
            self,
            LensError::Profile { .. }
                | LensError::ProfileEnvMissing { .. }
                | LensError::ProfileNotFound { .. }
                | LensError::ProfileUnknown { .. }
                | LensError::ProfileMismatch { .. }
        )
    }
}

pub fn sanitize_error(err: &LensError) -> String {
    match err {
        LensError::ManifestBeginFailed { .. } => {
            "ManifestBeginFailed: manifest begin failed".to_string()
        }
        LensError::ManifestFinishFailed { .. } => {
            "ManifestFinishFailed: manifest finish failed".to_string()
        }
        LensError::FrontendError { .. } => "FrontendError: frontend failed".to_string(),
        LensError::SourceError { .. } => "SourceError: source failed".to_string(),
        LensError::RedactionFailed { .. } => "RedactionFailed: redaction failed".to_string(),
        LensError::ReplayUnavailable { .. } => "ReplayUnavailable: replay unavailable".to_string(),
        LensError::SnapshotPurged { .. } => {
            "SnapshotPurged: snapshot purged by retention policy".to_string()
        }
        LensError::ScopeRejected { .. } => "ScopeRejected: unsupported session scope".to_string(),
        LensError::ConvertError(err) => match err {
            LowerError::Decode { kind, .. } => {
                format!("ConvertError: decode failure for {kind}")
            }
            LowerError::Unsupported(_) => "ConvertError: unsupported source type".to_string(),
        },
        LensError::ProfileEnvMissing { env } => {
            format!("ProfileEnvMissing: missing environment variable {env}")
        }
        LensError::SecretKeyringMissing { service, account } => {
            format!(
                "SecretKeyringMissing: missing keyring entry service={service} account={account}"
            )
        }
        LensError::SecretKeyringDenied { service, account } => {
            format!(
                "SecretKeyringDenied: keyring access denied service={service} account={account}"
            )
        }
        LensError::SecretBackendUnavailable { backend, .. } => {
            format!("SecretBackendUnavailable: {backend} backend unavailable")
        }
        LensError::ProfileNotFound { label, path } => {
            format!("ProfileNotFound: {label} not found: {}", path.display())
        }
        LensError::Profile { detail } => format!("Profile: {detail}"),
        LensError::ProfileUnknown { profile, loaded } => {
            format!("ProfileUnknown: profile `{profile}` not found; loaded: {loaded:?}")
        }
        LensError::ProfileMismatch {
            profile,
            tool,
            required,
            actual,
        } => {
            format!(
                "ProfileMismatch: profile `{profile}` is a {actual} source; tool `{tool}` requires a {required} profile"
            )
        }
        LensError::BatchPartial {
            applied, pending, ..
        } => {
            format!(
                "BatchPartial: batch write failed (applied {}, pending {})",
                applied.len(),
                pending.len()
            )
        }
        LensError::FeatureDeferred(feature) => format!("FeatureDeferred: {feature}"),
        LensError::Truncated(reason) => format!("Truncated: {reason:?}"),
        LensError::Internal { .. } => "Internal: internal error".to_string(),
    }
}
