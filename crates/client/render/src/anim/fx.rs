//! A small multiplicative hasher (the "Fx" scheme of rustc) for the keys of the animation service:
//! small integers and short names that are looked up thousands of times a frame. `std`'s SipHash
//! is built to resist hostile keys, which these are not; this is several times cheaper. No new
//! dependency: the crates that ship the same hasher are only reached through `wgpu`.

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

const SEED: u64 = 0x51_7c_c1_b7_27_22_0a_95;

#[derive(Default, Clone, Copy)]
pub struct FxHasher {
    hash: u64,
}

impl FxHasher {
    #[inline]
    fn add(&mut self, word: u64) {
        self.hash = (self.hash.rotate_left(5) ^ word).wrapping_mul(SEED);
    }
}

impl Hasher for FxHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        let mut chunks = bytes.chunks_exact(8);
        for c in &mut chunks {
            self.add(u64::from_le_bytes([
                c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7],
            ]));
        }
        let rest = chunks.remainder();
        if !rest.is_empty() {
            let mut w = 0u64;
            for (i, b) in rest.iter().enumerate() {
                w |= u64::from(*b) << (8 * i);
            }
            self.add(w);
        }
    }

    #[inline]
    fn write_u8(&mut self, i: u8) {
        self.add(u64::from(i));
    }

    #[inline]
    fn write_u16(&mut self, i: u16) {
        self.add(u64::from(i));
    }

    #[inline]
    fn write_u32(&mut self, i: u32) {
        self.add(u64::from(i));
    }

    #[inline]
    fn write_u64(&mut self, i: u64) {
        self.add(i);
    }

    #[inline]
    fn write_usize(&mut self, i: usize) {
        self.add(i as u64);
    }

    /// The low bits of a product are weak and the table indexes by them: fold the high ones down.
    #[inline]
    fn finish(&self) -> u64 {
        self.hash.rotate_left(26)
    }
}

pub type FxBuild = BuildHasherDefault<FxHasher>;
pub type FxHashMap<K, V> = HashMap<K, V, FxBuild>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::hash::{BuildHasher, Hash};

    fn h<T: Hash>(t: T) -> u64 {
        FxBuild::default().hash_one(t)
    }

    #[test]
    fn equal_keys_hash_equal_and_near_keys_differ() {
        assert_eq!(h((3u32, 4u16)), h((3u32, 4u16)));
        assert_ne!(h((3u32, 4u16)), h((3u32, 5u16)));
        assert_ne!(h("md:warrior"), h("md:warrior_asia"));
        let low: std::collections::HashSet<u64> = (0..64u32).map(|i| h(i) & 63).collect();
        assert!(low.len() > 30, "the low bits are spread: {}", low.len());
    }
}
