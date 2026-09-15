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

// Serde's ordinary struct derives also accept positional JSON arrays. Every
// protocol object must instead be a map, including nested records and bindings.
macro_rules! closed_object {
    ($(#[$attr:meta])* $vis:vis struct $name:ident {
        $($(#[$field_attr:meta])* $field_vis:vis $field:ident: $ty:ty),* $(,)?
    }) => {
        $(#[$attr])*
        #[derive(serde::Serialize)]
        $vis struct $name { $($(#[$field_attr])* $field_vis $field: $ty),* }
        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
                let raw = <Box<serde_json::value::RawValue> as serde::Deserialize>::deserialize(d)?;
                if !raw.get().starts_with('{') {
                    return Err(serde::de::Error::custom("invalid_request"));
                }
                #[derive(serde::Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Fields { $($(#[$field_attr])* $field: $ty),* }
                let Fields { $($field),* } = serde_json::from_str(raw.get()).map_err(|_| serde::de::Error::custom("invalid_request"))?;
                Ok(Self { $($field),* })
            }
        }
    };
}
pub(crate) use closed_object;
