//! Unpublished phase 2 server work. Source execution is not yet available.
pub mod auth;
pub mod config;
pub mod history;
pub(crate) mod private;
pub mod service;

/// Exact-length lowercase hex: the shape of every opaque identity, generation
/// and digest field in the authority file and the identity history.
pub(crate) fn hex(text: &str, len: usize) -> bool {
    text.len() == len
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
