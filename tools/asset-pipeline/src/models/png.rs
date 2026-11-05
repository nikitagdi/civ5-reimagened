//! Minimal PNG writer: RGBA8, no filtering, zlib stored (uncompressed) blocks.
//!
//! Only for the glTF debug export, so size does not matter and no compression crate is needed.

const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
/// Largest payload of one stored deflate block.
const MAX_STORED: usize = 65535;

fn crc_table() -> [u32; 256] {
    std::array::from_fn(|n| {
        (0..8).fold(n as u32, |c, _| {
            if c & 1 != 0 {
                0xedb8_8320 ^ (c >> 1)
            } else {
                c >> 1
            }
        })
    })
}

/// CRC-32 (IEEE) as used by PNG chunks.
pub fn crc32(data: &[u8]) -> u32 {
    let table = crc_table();
    !data.iter().fold(!0u32, |c, &b| {
        table[((c ^ u32::from(b)) & 0xff) as usize] ^ (c >> 8)
    })
}

/// Adler-32 checksum of the zlib stream.
pub fn adler32(data: &[u8]) -> u32 {
    const MOD: u32 = 65521;
    let (mut a, mut b) = (1u32, 0u32);
    // 5552 bytes keep the sums inside u32 between reductions.
    for chunk in data.chunks(5552) {
        for &x in chunk {
            a += u32::from(x);
            b += a;
        }
        a %= MOD;
        b %= MOD;
    }
    (b << 16) | a
}

fn zlib_stored(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0x01];
    let mut blocks = data.chunks(MAX_STORED).peekable();
    if blocks.peek().is_none() {
        // One empty final block.
        out.extend_from_slice(&[1, 0, 0, 0xff, 0xff]);
    }
    while let Some(block) = blocks.next() {
        let last = blocks.peek().is_none();
        let len = block.len() as u16;
        out.push(u8::from(last));
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes());
        out.extend_from_slice(block);
    }
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let crc = crc32(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

/// Encodes tightly packed RGBA8 rows (`width * height * 4` bytes) as a PNG.
pub fn encode_rgba8(width: u32, height: u32, pixels: &[u8]) -> Result<Vec<u8>, String> {
    let row = width as usize * 4;
    if width == 0 || height == 0 || pixels.len() != row * height as usize {
        return Err(format!(
            "bad image: {width}x{height} with {} bytes",
            pixels.len()
        ));
    }
    let mut raw = Vec::with_capacity((row + 1) * height as usize);
    for line in pixels.chunks_exact(row) {
        raw.push(0); // filter: none
        raw.extend_from_slice(line);
    }
    let mut out = SIGNATURE.to_vec();
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8 bit, RGBA, deflate, adaptive filter, no interlace
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &zlib_stored(&raw));
    chunk(&mut out, b"IEND", &[]);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksums_match_known_values() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
        assert_eq!(crc32(b""), 0);
        assert_eq!(adler32(b"Wikipedia"), 0x11e6_0398);
        assert_eq!(adler32(b""), 1);
        // Long input crosses the reduction interval.
        let long = vec![0xffu8; 100_000];
        let (mut a, mut b) = (1u64, 0u64);
        for &x in &long {
            a = (a + u64::from(x)) % 65521;
            b = (b + a) % 65521;
        }
        assert_eq!(adler32(&long), ((b << 16) | a) as u32);
    }

    /// Walks the chunk list and checks lengths and CRCs.
    fn chunks(png: &[u8]) -> Vec<([u8; 4], Vec<u8>)> {
        assert_eq!(&png[..8], &SIGNATURE);
        let mut at = 8;
        let mut out = Vec::new();
        while at < png.len() {
            let len = u32::from_be_bytes(png[at..at + 4].try_into().unwrap()) as usize;
            let kind: [u8; 4] = png[at + 4..at + 8].try_into().unwrap();
            let data = png[at + 8..at + 8 + len].to_vec();
            let crc = u32::from_be_bytes(png[at + 8 + len..at + 12 + len].try_into().unwrap());
            assert_eq!(crc, crc32(&png[at + 4..at + 8 + len]), "crc of {kind:?}");
            out.push((kind, data));
            at += 12 + len;
        }
        assert_eq!(at, png.len());
        out
    }

    /// Inflates a zlib stream made only of stored blocks.
    fn unstore(z: &[u8]) -> Vec<u8> {
        assert_eq!(z[0] & 0x0f, 8);
        assert_eq!((u16::from(z[0]) << 8 | u16::from(z[1])) % 31, 0);
        let mut at = 2;
        let mut out = Vec::new();
        loop {
            let last = z[at] & 1 == 1;
            assert_eq!(z[at] >> 1, 0, "stored block");
            let len = u16::from_le_bytes([z[at + 1], z[at + 2]]);
            let nlen = u16::from_le_bytes([z[at + 3], z[at + 4]]);
            assert_eq!(len, !nlen);
            out.extend_from_slice(&z[at + 5..at + 5 + usize::from(len)]);
            at += 5 + usize::from(len);
            if last {
                break;
            }
        }
        let adler = u32::from_be_bytes(z[at..at + 4].try_into().unwrap());
        assert_eq!(adler, adler32(&out));
        assert_eq!(at + 4, z.len());
        out
    }

    #[test]
    fn structure_and_pixels_round_trip() {
        // 200 x 100 forces several stored blocks (80 100 raw bytes).
        let (w, h) = (200u32, 100u32);
        let pixels: Vec<u8> = (0..w * h * 4).map(|i| (i * 7 % 251) as u8).collect();
        let png = encode_rgba8(w, h, &pixels).unwrap();
        let cs = chunks(&png);
        let kinds: Vec<_> = cs.iter().map(|(k, _)| *k).collect();
        assert_eq!(kinds, [*b"IHDR", *b"IDAT", *b"IEND"]);
        let ihdr = &cs[0].1;
        assert_eq!(u32::from_be_bytes(ihdr[0..4].try_into().unwrap()), w);
        assert_eq!(u32::from_be_bytes(ihdr[4..8].try_into().unwrap()), h);
        assert_eq!(&ihdr[8..], &[8, 6, 0, 0, 0]);
        let raw = unstore(&cs[1].1);
        assert_eq!(raw.len(), (w as usize * 4 + 1) * h as usize);
        for (y, line) in raw.chunks_exact(w as usize * 4 + 1).enumerate() {
            assert_eq!(line[0], 0);
            assert_eq!(&line[1..], &pixels[y * w as usize * 4..][..w as usize * 4]);
        }
    }

    #[test]
    fn rejects_bad_dimensions() {
        assert!(encode_rgba8(0, 1, &[]).is_err());
        assert!(encode_rgba8(2, 2, &[0; 15]).is_err());
    }
}
