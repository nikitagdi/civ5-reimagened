//! Blob content hash.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// 32-byte content hash; text form is 64 lowercase hex digits.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ContentHash(pub [u8; 32]);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseHashError {
    #[error("hash is {0} characters long, expected 64")]
    BadLength(usize),
    /// Only `0-9` and `a-f` are allowed.
    #[error("invalid character {0:?} in hash")]
    BadChar(char),
}

impl ContentHash {
    pub const HEX_LEN: usize = 64;

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl From<[u8; 32]> for ContentHash {
    fn from(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

impl fmt::Display for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for b in self.0 {
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ContentHash({self})")
    }
}

fn nibble(c: char) -> Result<u8, ParseHashError> {
    match c {
        '0'..='9' => Ok(c as u8 - b'0'),
        'a'..='f' => Ok(c as u8 - b'a' + 10),
        _ => Err(ParseHashError::BadChar(c)),
    }
}

impl FromStr for ContentHash {
    type Err = ParseHashError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let len = s.chars().count();
        if len != Self::HEX_LEN {
            return Err(ParseHashError::BadLength(len));
        }
        let mut out = [0u8; 32];
        let mut chars = s.chars();
        for byte in &mut out {
            let (Some(hi), Some(lo)) = (chars.next(), chars.next()) else {
                return Err(ParseHashError::BadLength(len));
            };
            *byte = (nibble(hi)? << 4) | nibble(lo)?;
        }
        Ok(Self(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let mut bytes = [0u8; 32];
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = (i * 8) as u8;
        }
        let h = ContentHash(bytes);
        let s = h.to_string();
        assert_eq!(s.len(), 64);
        assert_eq!(s, s.to_lowercase());
        assert_eq!(s.parse::<ContentHash>().unwrap(), h);
    }

    #[test]
    fn rejects_bad_input() {
        assert_eq!(
            "ab".parse::<ContentHash>(),
            Err(ParseHashError::BadLength(2))
        );
        let upper = "A".repeat(64);
        assert_eq!(
            upper.parse::<ContentHash>(),
            Err(ParseHashError::BadChar('A'))
        );
        let bad = "g".repeat(64);
        assert_eq!(
            bad.parse::<ContentHash>(),
            Err(ParseHashError::BadChar('g'))
        );
    }
}
