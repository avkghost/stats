//! Stable SHA-256 digests for checkpoint prefixes.
//!
//! `std::collections::hash_map::DefaultHasher` is not specified across Rust
//! releases, so prefix states that must survive toolchain upgrades use SHA-256.

use sha2::{Digest, Sha256};
use std::fmt::Write;

/// Lowercase hex SHA-256 of `bytes`.
pub fn prefix_sha256(bytes: &[u8]) -> String {
    let mut digest = PrefixDigest::new();
    digest.update(bytes);
    digest.finish()
}

/// Incremental SHA-256, so a decoder can be streamed through it without
/// holding the decoded content in memory.
#[derive(Clone)]
pub struct PrefixDigest {
    inner: Sha256,
}

impl PrefixDigest {
    pub fn new() -> Self {
        Self {
            inner: Sha256::new(),
        }
    }

    pub fn update(&mut self, bytes: &[u8]) {
        self.inner.update(bytes);
    }

    pub fn finish(self) -> String {
        let digest = self.inner.finalize();
        let mut hex = String::with_capacity(digest.len() * 2);
        for byte in digest.iter() {
            // Writing into a String cannot fail.
            let _ = write!(hex, "{byte:02x}");
        }
        hex
    }
}

impl Default for PrefixDigest {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_standard_sha256_vectors() {
        assert_eq!(
            prefix_sha256(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            prefix_sha256(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn streaming_matches_one_shot() {
        let mut digest = PrefixDigest::new();
        digest.update(b"a");
        digest.update(b"bc");
        assert_eq!(digest.finish(), prefix_sha256(b"abc"));
    }
}
