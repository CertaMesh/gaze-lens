use std::path::PathBuf;

use thiserror::Error;

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
    #[error("scope rejected: {scope}")]
    ScopeRejected { scope: String },
    #[error("conversion failed: {0}")]
    ConvertError(#[from] LowerError),
    #[error("internal error: {detail}")]
    Internal { detail: String },
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
        LensError::ReplayUnavailable { .. } => {
            "ReplayUnavailable: replay unavailable".to_string()
        }
        LensError::ScopeRejected { .. } => "ScopeRejected: unsupported session scope".to_string(),
        LensError::ConvertError(err) => match err {
            LowerError::Decode { kind, .. } => {
                format!("ConvertError: decode failure for {kind}")
            }
            LowerError::Unsupported(_) => "ConvertError: unsupported source type".to_string(),
        },
        LensError::Internal { .. } => "Internal: internal error".to_string(),
    }
}
