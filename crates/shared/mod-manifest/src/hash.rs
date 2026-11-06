//! xxh64, the same algorithm and constants as `protocol::xxh64`. A private copy: this crate is MIT and must not
//! depend on the AGPL `protocol` crate (D12), and the algorithm is public. Verified against the reference vectors.

const P1: u64 = 0x9E37_79B1_85EB_CA87;
const P2: u64 = 0xC2B2_AE3D_27D4_EB4F;
const P3: u64 = 0x1656_67B1_9E37_79F9;
const P4: u64 = 0x85EB_CA77_C2B2_AE63;
const P5: u64 = 0x27D4_EB2F_1656_67C5;

fn round(acc: u64, lane: u64) -> u64 {
    acc.wrapping_add(lane.wrapping_mul(P2))
        .rotate_left(31)
        .wrapping_mul(P1)
}

fn merge(acc: u64, v: u64) -> u64 {
    (acc ^ round(0, v)).wrapping_mul(P1).wrapping_add(P4)
}

/// First 8 bytes as a little-endian lane; the caller guarantees `b.len() >= 8`.
fn lane64(b: &[u8]) -> u64 {
    let mut a = [0u8; 8];
    a.copy_from_slice(&b[..8]);
    u64::from_le_bytes(a)
}

fn lane32(b: &[u8]) -> u64 {
    let mut a = [0u8; 4];
    a.copy_from_slice(&b[..4]);
    u64::from(u32::from_le_bytes(a))
}

/// xxHash64 of `bytes` with `seed`.
pub(crate) fn xxh64(seed: u64, bytes: &[u8]) -> u64 {
    let mut rest = bytes;
    let mut h = if bytes.len() >= 32 {
        let mut v = [
            seed.wrapping_add(P1).wrapping_add(P2),
            seed.wrapping_add(P2),
            seed,
            seed.wrapping_sub(P1),
        ];
        while rest.len() >= 32 {
            for (i, acc) in v.iter_mut().enumerate() {
                *acc = round(*acc, lane64(&rest[i * 8..]));
            }
            rest = &rest[32..];
        }
        let mut h = v[0]
            .rotate_left(1)
            .wrapping_add(v[1].rotate_left(7))
            .wrapping_add(v[2].rotate_left(12))
            .wrapping_add(v[3].rotate_left(18));
        for x in v {
            h = merge(h, x);
        }
        h
    } else {
        seed.wrapping_add(P5)
    };
    h = h.wrapping_add(bytes.len() as u64);
    while rest.len() >= 8 {
        h = (h ^ round(0, lane64(rest)))
            .rotate_left(27)
            .wrapping_mul(P1)
            .wrapping_add(P4);
        rest = &rest[8..];
    }
    if rest.len() >= 4 {
        h = (h ^ lane32(rest).wrapping_mul(P1))
            .rotate_left(23)
            .wrapping_mul(P2)
            .wrapping_add(P3);
        rest = &rest[4..];
    }
    for &b in rest {
        h = (h ^ u64::from(b).wrapping_mul(P5))
            .rotate_left(11)
            .wrapping_mul(P1);
    }
    h ^= h >> 33;
    h = h.wrapping_mul(P2);
    h ^= h >> 29;
    h = h.wrapping_mul(P3);
    h ^ (h >> 32)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRIME32: u64 = 2_654_435_761;
    const PRIME64: u64 = 11_400_714_785_074_694_797;

    fn sanity(len: usize) -> Vec<u8> {
        let mut state = PRIME32;
        let mut out = Vec::with_capacity(len);
        for _ in 0..len {
            out.push((state >> 56) as u8);
            state = state.wrapping_mul(PRIME64);
        }
        out
    }

    #[test]
    fn reference_vectors() {
        assert_eq!(xxh64(0, b""), 0xEF46_DB37_51D8_E999);
        assert_eq!(xxh64(0, b"abc"), 0x44BC_2CF5_AD77_0999);
        assert_eq!(
            xxh64(0, b"Nobody inspects the spammish repetition"),
            0xFBCE_A83C_8A37_8BF1
        );
        assert_eq!(xxh64(0, &sanity(1)), 0xE934_A84A_DB05_2768);
        assert_eq!(xxh64(PRIME32, b""), 0xAC75_FDA2_929B_17EF);
        assert_eq!(xxh64(PRIME32, &sanity(1)), 0x5014_6076_43A9_B4C3);
        // 1000 bytes: 32-byte stripes plus 8-, 4- and 1-byte tails.
        let b = sanity(1000);
        assert_eq!(xxh64(0, &b), 0x52BD_1358_F22E_9EF7);
        assert_eq!(xxh64(PRIME32, &b), 0x7275_1A24_0801_7E26);
        assert_eq!(xxh64(8, &b), 0xD80A_722A_5A5B_82AD);
    }
}
