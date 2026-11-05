//! SHA-1 (FIPS 180-4), the block hash of the catalog.
//! Written in-tree next to `sha256.rs`; not used for security, only to name blocks.

/// Streaming SHA-1.
pub struct Sha1 {
    state: [u32; 5],
    buf: [u8; 64],
    buf_len: usize,
    total: u64,
}

impl Default for Sha1 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha1 {
    pub fn new() -> Self {
        Self {
            state: [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0],
            buf: [0; 64],
            buf_len: 0,
            total: 0,
        }
    }

    pub fn update(&mut self, mut data: &[u8]) {
        self.total += data.len() as u64;
        if self.buf_len > 0 {
            let take = (64 - self.buf_len).min(data.len());
            self.buf[self.buf_len..self.buf_len + take].copy_from_slice(&data[..take]);
            self.buf_len += take;
            data = &data[take..];
            if self.buf_len < 64 {
                return;
            }
            let block = self.buf;
            self.compress(&block);
            self.buf_len = 0;
        }
        let mut blocks = data.chunks_exact(64);
        for block in &mut blocks {
            let mut b = [0u8; 64];
            b.copy_from_slice(block);
            self.compress(&b);
        }
        let rest = blocks.remainder();
        self.buf[..rest.len()].copy_from_slice(rest);
        self.buf_len = rest.len();
    }

    /// Lowercase hex digest.
    pub fn finalize_hex(mut self) -> String {
        let bit_len = self.total.wrapping_mul(8);
        let mut pad = vec![0x80u8];
        while (self.buf_len + pad.len()) % 64 != 56 {
            pad.push(0);
        }
        pad.extend_from_slice(&bit_len.to_be_bytes());
        self.update(&pad);
        debug_assert_eq!(self.buf_len, 0);
        self.state.iter().map(|w| format!("{w:08x}")).collect()
    }

    fn compress(&mut self, block: &[u8; 64]) {
        let mut w = [0u32; 80];
        for (i, c) in block.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([c[0], c[1], c[2], c[3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = self.state;
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A827999),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _ => (b ^ c ^ d, 0xCA62C1D6u32),
            };
            let t = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = t;
        }
        for (s, v) in self.state.iter_mut().zip([a, b, c, d, e]) {
            *s = s.wrapping_add(v);
        }
    }
}

/// Hex sha1 of a byte slice.
pub fn sha1_hex(data: &[u8]) -> String {
    let mut h = Sha1::new();
    h.update(data);
    h.finalize_hex()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_vectors() {
        // FIPS 180-4 / RFC 3174 examples.
        assert_eq!(sha1_hex(b""), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert_eq!(sha1_hex(b"abc"), "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(
            sha1_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "84983e441c3bd26ebaae4aa1f95129e5e54670f1"
        );
        let million_a = vec![b'a'; 1_000_000];
        assert_eq!(
            sha1_hex(&million_a),
            "34aa973cd4c4daa4f61eeb2bdbad27316534016f"
        );
    }

    #[test]
    fn streaming_matches_one_shot_across_block_edges() {
        let data: Vec<u8> = (0..1000u32).map(|i| (i * 7 + 3) as u8).collect();
        let want = sha1_hex(&data);
        for chunk in [1usize, 3, 63, 64, 65, 200] {
            let mut h = Sha1::new();
            for c in data.chunks(chunk) {
                h.update(c);
            }
            assert_eq!(h.finalize_hex(), want, "chunk {chunk}");
        }
    }
}
