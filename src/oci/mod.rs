//! OCI protocol types. Values from the wire are parsed into these at the boundary
//! and passed around as types, so the rest of the code never sees an unchecked string.

mod digest;
mod name;

pub use digest::Digest;
pub(crate) use digest::{decode_hex, encode_hex};
pub use name::{Reference, RepoName, Tag, UploadId};

/// `Display` and `Debug` for a newtype over `String` that shows the inner string.
macro_rules! display_as_str {
    ($($t:ty),*) => {$(
        impl std::fmt::Display for $t {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl std::fmt::Debug for $t {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                std::fmt::Debug::fmt(&self.0, f)
            }
        }
    )*};
}

pub(crate) use display_as_str;

/// Serde as the text form: `Display` out, `FromStr` in. Stored values go through
/// the same parser as values from the wire, so a damaged file fails to load.
macro_rules! serde_as_str {
    ($($t:ty),*) => {$(
        impl serde::Serialize for $t {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.collect_str(self)
            }
        }

        impl<'de> serde::Deserialize<'de> for $t {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let s = String::deserialize(deserializer)?;
                s.parse().map_err(serde::de::Error::custom)
            }
        }
    )*};
}

pub(crate) use serde_as_str;

serde_as_str!(Digest, RepoName, Tag);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid {kind}: {value:?}")]
pub struct ParseError {
    pub kind: &'static str,
    pub value: String,
}

impl ParseError {
    pub(crate) fn new(kind: &'static str, value: &str) -> Self {
        Self {
            kind,
            value: value.to_owned(),
        }
    }
}
