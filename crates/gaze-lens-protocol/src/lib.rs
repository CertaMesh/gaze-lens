//! Pure, closed `gaze-lens-source/2` data and framing contract.
//! Raw DTOs are never clean output. TLS, authorization, sources, Gaze and durable
//! release must be supplied by the later server/client phases.
pub mod bounds;
pub mod query;
pub mod value;
pub mod wire;

/// Fixed failures only: no parser, credential, source or driver payloads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Error {
    InvalidRequest,
    Unauthorized,
    UnsupportedVersion,
    UnsupportedOperation,
    Unavailable,
    CapExceeded,
    Timeout,
    BindingChanged,
    InternalFailure,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidRequest => "invalid_request",
            Self::Unauthorized => "unauthorized",
            Self::UnsupportedVersion => "unsupported_version",
            Self::UnsupportedOperation => "unsupported_operation",
            Self::Unavailable => "unavailable",
            Self::CapExceeded => "cap_exceeded",
            Self::Timeout => "timeout",
            Self::BindingChanged => "binding_changed",
            Self::InternalFailure => "internal_failure",
        })
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;
