//! zlib-compatible CRC-32 (used for stripped member-name hashes).

const TABLE: [u32; 256] = {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
            k += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
};

pub fn crc32(data: &[u8]) -> u32 {
    let mut c = !0u32;
    for &b in data {
        c = TABLE[((c ^ b as u32) & 0xff) as usize] ^ (c >> 8);
    }
    !c
}

/// Undoes the CRC update of `suffix`: given the register `state` after `prefix + suffix`
/// (without the final inversion, i.e. [`crate::name_hash`]), returns the register after `prefix`.
/// The update is a bijection on the register, so this is exact.
pub fn unwind(mut state: u32, suffix: &[u8]) -> u32 {
    for &b in suffix.iter().rev() {
        // The top byte of TABLE[i] is unique per i, so it names the table entry used.
        let i = TABLE_BY_TOP[(state >> 24) as usize] as usize;
        state = ((state ^ TABLE[i]) << 8) | ((i as u32) ^ u32::from(b));
    }
    state
}

/// `TABLE_BY_TOP[TABLE[i] >> 24] = i`.
const TABLE_BY_TOP: [u8; 256] = {
    let mut t = [0u8; 256];
    let mut i = 0;
    while i < 256 {
        t[(TABLE[i] >> 24) as usize] = i as u8;
        i += 1;
    }
    t
};

#[cfg(test)]
mod tests {
    #[test]
    fn unwind_inverts_the_update() {
        let state = |s: &[u8]| !super::crc32(s);
        let full = state(b"C:\\art\\units\\Catapult_DIFF.tga");
        assert_eq!(
            super::unwind(full, b"\\Catapult_DIFF.tga"),
            state(b"C:\\art\\units")
        );
        assert_eq!(super::unwind(full, b""), full);
        assert_eq!(
            super::unwind(full, b"C:\\art\\units\\Catapult_DIFF.tga"),
            !0
        );
    }

    #[test]
    fn matches_zlib() {
        assert_eq!(super::crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(super::crc32(b""), 0);
    }
}
