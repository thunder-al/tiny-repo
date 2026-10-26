//! Repository names, tags, references and upload ids, with the grammar of the OCI
//! distribution spec.

use super::{Digest, ParseError};
use std::str::FromStr;

/// `[a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*(\/[a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*)*`,
/// at most 255 characters.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RepoName(String);

impl FromStr for RepoName {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.len() <= 255 && s.split('/').all(is_path_component) {
            Ok(Self(s.to_owned()))
        } else {
            Err(ParseError::new("repository name", s))
        }
    }
}

/// Runs of `[a-z0-9]` joined by `.`, `_`, `__` or any number of `-`.
fn is_path_component(c: &str) -> bool {
    let alnum = |b: &u8| b.is_ascii_lowercase() || b.is_ascii_digit();
    let bytes = c.as_bytes();

    // Everything between the alnum runs is a separator; any other byte fails the match.
    bytes.first().is_some_and(alnum)
        && bytes.last().is_some_and(alnum)
        && bytes
            .split(alnum)
            .filter(|sep| !sep.is_empty())
            .all(|sep| matches!(sep, b"." | b"_" | b"__") || sep.iter().all(|&b| b == b'-'))
}

/// `[a-zA-Z0-9_][a-zA-Z0-9._-]{0,127}`
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Tag(String);

impl FromStr for Tag {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let word = |b: &u8| b.is_ascii_alphanumeric() || *b == b'_';

        let ok = match s.as_bytes() {
            [first, rest @ ..] => {
                word(first)
                    && rest.len() < 128
                    && rest.iter().all(|b| word(b) || b"._-".contains(b))
            }
            [] => false,
        };

        if ok {
            Ok(Self(s.to_owned()))
        } else {
            Err(ParseError::new("tag", s))
        }
    }
}

/// The `<reference>` of `/v2/<name>/manifests/<reference>`. Tags cannot contain `:`,
/// so the two forms never overlap.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Reference {
    Tag(Tag),
    Digest(Digest),
}

impl FromStr for Reference {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.contains(':') {
            s.parse().map(Reference::Digest)
        } else {
            s.parse().map(Reference::Tag)
        }
    }
}

/// Id of a blob upload session. It ends up in URLs and storage keys, so it is
/// restricted to `[A-Za-z0-9_-]{1,128}`.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct UploadId(String);

impl FromStr for UploadId {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let ok = (1..=128).contains(&s.len())
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');

        if ok {
            Ok(Self(s.to_owned()))
        } else {
            Err(ParseError::new("upload id", s))
        }
    }
}

super::display_as_str!(RepoName, Tag, UploadId);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_names() {
        let valid = [
            "a",
            "library/nginx",
            "thunder-al/tiny-repo",
            "my.org/sub_dir/img__x",
            "a--b---c/0",
            &"a".repeat(255),
        ];
        for s in valid {
            assert!(s.parse::<RepoName>().is_ok(), "{s:?} should be valid");
        }

        let invalid = [
            "",
            "Library/nginx",
            "/a",
            "a/",
            "a//b",
            "-a",
            "a-",
            "a..b",
            "a___b",
            "a._b",
            "a b",
            "a:b",
            &"a".repeat(256),
        ];
        for s in invalid {
            assert!(s.parse::<RepoName>().is_err(), "{s:?} should be invalid");
        }
    }

    #[test]
    fn tags() {
        for s in ["latest", "v1.2.3", "_x", "A-b_c.d", &"a".repeat(128)] {
            assert!(s.parse::<Tag>().is_ok(), "{s:?} should be valid");
        }
        for s in ["", ".x", "-x", "a:b", "a/b", &"a".repeat(129)] {
            assert!(s.parse::<Tag>().is_err(), "{s:?} should be invalid");
        }
    }

    #[test]
    fn reference_picks_form_by_colon() {
        let digest = "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

        assert!(matches!("v1".parse(), Ok(Reference::Tag(_))));
        assert!(matches!(digest.parse(), Ok(Reference::Digest(_))));
        assert!("sha256:nothex".parse::<Reference>().is_err());
    }

    #[test]
    fn upload_ids() {
        assert!("3f1c-AB_9".parse::<UploadId>().is_ok());
        for s in ["", "../etc", "a/b", "a.b", &"a".repeat(129)] {
            assert!(s.parse::<UploadId>().is_err(), "{s:?} should be invalid");
        }
    }
}
