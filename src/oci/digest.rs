//! Content digests. Only the algorithms registered in the OCI image spec that are
//! actually in use: `sha256` (required) and `sha512`. `blake3` is registered too,
//! but no client produces it.

use super::ParseError;
use std::{fmt, str::FromStr};

/// `<algorithm>:<lowercase hex>`. The derived order matches the string order, so
/// listings sorted by `Digest` and by its text agree.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Digest {
    Sha256([u8; 32]),
    Sha512([u8; 64]),
}

impl Digest {
    pub fn algorithm(&self) -> &'static str {
        match self {
            Digest::Sha256(_) => "sha256",
            Digest::Sha512(_) => "sha512",
        }
    }

    pub fn bytes(&self) -> &[u8] {
        match self {
            Digest::Sha256(b) => b,
            Digest::Sha512(b) => b,
        }
    }

    /// The encoded part, as used in storage keys (`blobs/<algorithm>/<hex>`).
    pub fn hex(&self) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";

        let mut out = String::with_capacity(self.bytes().len() * 2);
        for b in self.bytes() {
            out.push(char::from(HEX[usize::from(b >> 4)]));
            out.push(char::from(HEX[usize::from(b & 0xf)]));
        }
        out
    }
}

impl FromStr for Digest {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || ParseError::new("digest", s);

        let (algorithm, hex) = s.split_once(':').ok_or_else(err)?;
        match algorithm {
            "sha256" => decode_hex(hex).map(Digest::Sha256).ok_or_else(err),
            "sha512" => decode_hex(hex).map(Digest::Sha512).ok_or_else(err),
            _ => Err(err()),
        }
    }
}

/// Lowercase hex of exactly `N` bytes; the spec forbids uppercase for these algorithms.
fn decode_hex<const N: usize>(hex: &str) -> Option<[u8; N]> {
    fn nibble(c: u8) -> Option<u8> {
        match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            _ => None,
        }
    }

    let hex = hex.as_bytes();
    if hex.len() != N * 2 {
        return None;
    }

    let mut out = [0u8; N];
    for (byte, pair) in out.iter_mut().zip(hex.chunks_exact(2)) {
        *byte = nibble(pair[0])? << 4 | nibble(pair[1])?;
    }
    Some(out)
}

impl fmt::Display for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.algorithm(), self.hex())
    }
}

impl fmt::Debug for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // sha256("") and sha512("")
    const SHA256_EMPTY: &str =
        "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    const SHA512_EMPTY: &str = "sha512:cf83e1357eefb8bdf1542850d66d8007d620e4050b5715dc83f4a921d36ce9ce47d0d13c5d85f2b0ff8318d2877eec2f63b931bd47417a81a538327af927da3e";

    #[test]
    fn round_trips_both_algorithms() {
        for s in [SHA256_EMPTY, SHA512_EMPTY] {
            let d: Digest = s.parse().unwrap_or_else(|e| panic!("{e}"));
            assert_eq!(d.to_string(), s);
        }
    }

    #[test]
    fn rejects_malformed() {
        let upper = SHA256_EMPTY.to_uppercase().replace("SHA256", "sha256");
        let short = &SHA256_EMPTY[..SHA256_EMPTY.len() - 1];
        let long = format!("{SHA256_EMPTY}0");
        let sha512_len_as_256 = SHA512_EMPTY.replace("sha512", "sha256");
        let blake3 = SHA256_EMPTY.replace("sha256", "blake3");

        for s in [
            "",
            "sha256",
            "sha256:",
            short,
            &long,
            &upper,
            &sha512_len_as_256,
            &blake3,
            "md5:d41d8cd98f00b204e9800998ecf8427e",
        ] {
            assert!(s.parse::<Digest>().is_err(), "{s:?} should be rejected");
        }
    }

    #[test]
    fn orders_like_its_text() {
        let a: Result<Vec<Digest>, _> = [SHA512_EMPTY, SHA256_EMPTY]
            .iter()
            .map(|s| s.parse())
            .collect();
        let mut digests = a.unwrap_or_else(|e| panic!("{e}"));
        digests.sort();

        let texts: Vec<String> = digests.iter().map(Digest::to_string).collect();
        let mut sorted = texts.clone();
        sorted.sort();
        assert_eq!(texts, sorted);
    }
}
