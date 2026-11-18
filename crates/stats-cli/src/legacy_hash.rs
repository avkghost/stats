//! Frozen reimplementation of the checkpoint hash used by earlier versions.
//!
//! Earlier versions hashed bytes with `std::collections::hash_map::DefaultHasher`.
//! Rust does not guarantee that output across releases, so a toolchain change would
//! make every stored value look different and trigger a reset. This module computes
//! the same value (SipHash-1-3, zero keys, `[u8]` hashed as a `u64` length prefix
//! followed by the bytes, little-endian) without depending on the standard library.
//! The unit test below pins it to `DefaultHasher` on the toolchain that wrote the data.

/// Lowercase hex, 16 digits, of the frozen hash of `bytes`.
pub fn hash_bytes(bytes: &[u8]) -> String {
    let mut hasher = Siphash13::new();
    hasher.write(&(bytes.len() as u64).to_le_bytes());
    hasher.write(bytes);
    format!("{:016x}", hasher.finish())
}

struct Siphash13 {
    v0: u64,
    v1: u64,
    v2: u64,
    v3: u64,
    tail: u64,
    ntail: usize,
    length: usize,
}

impl Siphash13 {
    fn new() -> Self {
        let (k0, k1) = (0u64, 0u64);
        Self {
            v0: k0 ^ 0x736f_6d65_7073_6575,
            v1: k1 ^ 0x646f_7261_6e64_6f6d,
            v2: k0 ^ 0x6c79_6765_6e65_7261,
            v3: k1 ^ 0x7465_6462_7974_6573,
            tail: 0,
            ntail: 0,
            length: 0,
        }
    }

    fn round(&mut self) {
        self.v0 = self.v0.wrapping_add(self.v1);
        self.v1 = self.v1.rotate_left(13) ^ self.v0;
        self.v0 = self.v0.rotate_left(32);
        self.v2 = self.v2.wrapping_add(self.v3);
        self.v3 = self.v3.rotate_left(16) ^ self.v2;
        self.v0 = self.v0.wrapping_add(self.v3);
        self.v3 = self.v3.rotate_left(21) ^ self.v0;
        self.v2 = self.v2.wrapping_add(self.v1);
        self.v1 = self.v1.rotate_left(17) ^ self.v2;
        self.v2 = self.v2.rotate_left(32);
    }

    fn compress(&mut self, word: u64) {
        self.v3 ^= word;
        self.round();
        self.v0 ^= word;
    }

    fn write(&mut self, mut message: &[u8]) {
        self.length += message.len();
        if self.ntail != 0 {
            let take = (8 - self.ntail).min(message.len());
            for (offset, byte) in message[..take].iter().enumerate() {
                self.tail |= u64::from(*byte) << (8 * (self.ntail + offset));
            }
            self.ntail += take;
            message = &message[take..];
            if self.ntail < 8 {
                return;
            }
            let word = self.tail;
            self.compress(word);
            self.tail = 0;
            self.ntail = 0;
        }
        let (words, rest) = message.as_chunks::<8>();
        for word in words {
            self.compress(u64::from_le_bytes(*word));
        }
        for (offset, byte) in rest.iter().enumerate() {
            self.tail |= u64::from(*byte) << (8 * offset);
        }
        self.ntail = rest.len();
    }

    fn finish(mut self) -> u64 {
        let block = ((self.length as u64 & 0xff) << 56) | self.tail;
        self.v3 ^= block;
        self.round();
        self.v0 ^= block;
        self.v2 ^= 0xff;
        self.round();
        self.round();
        self.round();
        self.v0 ^ self.v1 ^ self.v2 ^ self.v3
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::hash::{Hash, Hasher};

    fn std_hash(bytes: &[u8]) -> String {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        bytes.hash(&mut hasher);
        format!("{:016x}", hasher.finish())
    }

    #[test]
    fn matches_default_hasher_on_this_toolchain() {
        let long: Vec<u8> = (0..1000u32).map(|value| (value % 251) as u8).collect();
        for sample in [
            &b""[..],
            b"a",
            b"1234567",
            b"12345678",
            b"123456789",
            b"# config\nLogFile=\"/var/log/access.log\"\n",
            &long,
        ] {
            assert_eq!(hash_bytes(sample), std_hash(sample), "len {}", sample.len());
        }
    }

    #[test]
    fn split_writes_match_one_write() {
        // Checkpoints are computed over a whole file, but streaming callers may split it.
        let data: Vec<u8> = (0..257u32).map(|value| value as u8).collect();
        let mut split = Siphash13::new();
        split.write(&(data.len() as u64).to_le_bytes());
        for chunk in data.chunks(13) {
            split.write(chunk);
        }
        assert_eq!(format!("{:016x}", split.finish()), hash_bytes(&data));
    }
}
