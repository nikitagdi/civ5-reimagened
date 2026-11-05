//! WebSocket frame tests (net-protocol 9, "Тесты wire"): enc 0 and 1, threshold, no inflation, rejections.

use protocol::{ClientMsg, Codec, Postcard, ServerMsg};
use wire::{
    ENC_RAW, ENC_ZSTD, Encoder, GATEWAY_MAX_FRAME, Limits, MAX_RAW, WireError, compress, decode,
    decompress, unframe,
};

/// Deterministic, compressible bytes (a repeating phrase with a counter).
fn text(n: usize) -> Vec<u8> {
    let mut v = Vec::with_capacity(n + 32);
    let mut i = 0u32;
    while v.len() < n {
        v.extend_from_slice(format!("plot {i} grass hills river ").as_bytes());
        i = i.wrapping_add(1) % 64;
    }
    v.truncate(n);
    v
}

/// Deterministic incompressible bytes (xorshift).
fn noise(n: usize) -> Vec<u8> {
    let mut s = 0x9E37_79B9_7F4A_7C15u64;
    (0..n)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            (s >> 24) as u8
        })
        .collect()
}

#[test]
fn enc0_roundtrip() {
    let raw = text(100);
    let frame = Encoder::default().frame(&raw);
    assert_eq!(frame[0], ENC_RAW);
    assert_eq!(&frame[1..], &raw[..]);
    assert_eq!(&*unframe(&frame, &Limits::CLIENT).unwrap(), &raw[..]);
    assert_eq!(&*unframe(&frame, &Limits::GATEWAY).unwrap(), &raw[..]);
}

#[test]
fn enc1_roundtrip() {
    let raw = text(5000);
    let frame = Encoder::default().frame(&raw);
    assert_eq!(frame[0], ENC_ZSTD);
    assert!(frame.len() < raw.len());
    assert_eq!(&*unframe(&frame, &Limits::CLIENT).unwrap(), &raw[..]);
}

#[test]
fn message_roundtrip_both_encodings() {
    let enc = Encoder::default();
    let small = ServerMsg::Pong {
        seq: 1,
        client_ms: 2,
        server_ms: 3,
    };
    let f = enc.frame_msg(&small).unwrap();
    assert_eq!(f[0], ENC_RAW);
    assert_eq!(decode::<ServerMsg>(&f, &Limits::CLIENT).unwrap(), small);

    let big = ServerMsg::Welcome {
        session: 7,
        build: "x".repeat(2000),
    };
    let f = enc.frame_msg(&big).unwrap();
    assert_eq!(f[0], ENC_ZSTD);
    assert_eq!(decode::<ServerMsg>(&f, &Limits::CLIENT).unwrap(), big);
}

#[test]
fn threshold_is_256_bytes() {
    let enc = Encoder::default();
    assert_eq!(enc.threshold, 256);
    // 255 bytes of zeros would pack well, but stay under the threshold.
    assert_eq!(enc.frame(&[0u8; 255])[0], ENC_RAW);
    assert_eq!(enc.frame(&[0u8; 256])[0], ENC_ZSTD);
}

#[test]
fn never_longer_than_raw_plus_one() {
    let enc = Encoder::default();
    for raw in [
        Vec::new(),
        text(10),
        noise(300),
        noise(70_000),
        text(300_000),
        vec![0; 1 << 20],
    ] {
        let frame = enc.frame(&raw);
        assert!(
            frame.len() <= raw.len() + 1,
            "{} > {}",
            frame.len(),
            raw.len()
        );
        assert_eq!(&*unframe(&frame, &Limits::CLIENT).unwrap(), &raw[..]);
    }
    // Incompressible data goes out as enc 0 because the gain is below 10 %.
    assert_eq!(enc.frame(&noise(4096))[0], ENC_RAW);
}

#[test]
fn gain_threshold_follows_min_gain_pct() {
    let raw = text(4000);
    let z = compress(&raw);
    let strict = Encoder {
        threshold: 256,
        min_gain_pct: 99,
    };
    // Needs 99 % gain, which this input cannot give.
    assert!(z.len() * 100 >= raw.len(), "test input compresses too well");
    assert_eq!(strict.frame(&raw)[0], ENC_RAW);
    assert_eq!(Encoder::default().frame(&raw)[0], ENC_ZSTD);
}

#[test]
fn plain_is_enc0_for_any_size() {
    let raw = text(5000);
    let f = Encoder::plain(&raw);
    assert_eq!(f[0], ENC_RAW);
    assert_eq!(&f[1..], &raw[..]);
}

#[test]
fn empty_frame_is_rejected() {
    assert!(matches!(
        unframe(&[], &Limits::CLIENT),
        Err(WireError::Empty)
    ));
}

#[test]
fn unknown_enc_is_rejected() {
    for enc in [2u8, 3, 7, 255] {
        let frame = [enc, 1, 2, 3];
        assert!(
            matches!(unframe(&frame, &Limits::CLIENT), Err(WireError::UnknownEnc(e)) if e == enc),
            "enc {enc}"
        );
    }
}

#[test]
fn gateway_rejects_compressed_frame_from_client() {
    let frame = Encoder::default().frame(&text(5000));
    assert_eq!(frame[0], ENC_ZSTD);
    assert!(matches!(
        unframe(&frame, &Limits::GATEWAY),
        Err(WireError::CompressedNotAllowed)
    ));
}

#[test]
fn gateway_rejects_frame_over_64_kib() {
    let mut frame = vec![ENC_RAW];
    frame.resize(GATEWAY_MAX_FRAME + 1, 0);
    assert!(matches!(
        unframe(&frame, &Limits::GATEWAY),
        Err(WireError::FrameTooLarge { .. })
    ));
    frame.truncate(GATEWAY_MAX_FRAME);
    assert!(unframe(&frame, &Limits::GATEWAY).is_ok());
}

#[test]
fn declared_length_over_limit_is_rejected_before_decoding() {
    // Prefix claims 17 MiB; the body is garbage, so a decoder run would fail differently.
    let mut z = (17u32 << 20).to_le_bytes().to_vec();
    z.extend_from_slice(&[0xFF; 16]);
    assert!(matches!(
        decompress(&z, MAX_RAW),
        Err(WireError::RawTooLarge { declared, max }) if declared == 17 << 20 && max == MAX_RAW
    ));
    let mut frame = vec![ENC_ZSTD];
    frame.extend_from_slice(&z);
    assert!(matches!(
        unframe(&frame, &Limits::CLIENT),
        Err(WireError::RawTooLarge { .. })
    ));
    // u32::MAX must not allocate either.
    let mut z = u32::MAX.to_le_bytes().to_vec();
    z.extend_from_slice(&[0; 8]);
    assert!(matches!(
        decompress(&z, MAX_RAW),
        Err(WireError::RawTooLarge { .. })
    ));
    // A smaller limit with a real block.
    let real = compress(&text(5000));
    assert!(matches!(
        decompress(&real, 4999),
        Err(WireError::RawTooLarge { .. })
    ));
    assert!(decompress(&real, 5000).is_ok());
}

#[test]
fn lying_length_is_an_error() {
    let raw = text(5000);
    let z = compress(&raw);
    for lie in [0u32, 1, 4999, 5001, 100_000] {
        let mut bad = z.clone();
        bad[..4].copy_from_slice(&lie.to_le_bytes());
        assert!(
            decompress(&bad, MAX_RAW).is_err(),
            "declared {lie} for 5000 real bytes"
        );
    }
    // Shorter than real is TargetTooSmall-style, longer is a size mismatch: both errors, never a panic.
    assert_eq!(decompress(&z, MAX_RAW).unwrap(), raw);
}

#[test]
fn missing_prefix_is_an_error() {
    assert!(decompress(&[], MAX_RAW).is_err());
    assert!(decompress(&[1, 0, 0], MAX_RAW).is_err());
}

#[test]
fn garbage_is_an_error_not_a_panic() {
    for z in [
        &[][..],
        &[5, 0, 0, 0][..],
        &[5, 0, 0, 0, 0x28, 0xB5, 0x2F, 0xFD][..],
        &[0u8; 64][..],
    ] {
        assert!(decompress(z, MAX_RAW).is_err());
    }
    // Truncated and corrupted real frames.
    let real = compress(&text(5000));
    assert!(decompress(&real[..real.len() / 2], MAX_RAW).is_err());
    let mut bad = real.clone();
    let mid = bad.len() / 2;
    bad[mid] ^= 0xFF;
    // May fail or decode to other bytes (the checksum usually catches it). Never a panic.
    if let Ok(out) = decompress(&bad, MAX_RAW) {
        assert_ne!(out, text(5000));
    }
}

#[test]
fn compress_decompress_across_sizes() {
    // Sizes around the content-size field widths (flag 1 range is 256..=65791).
    for n in [
        0usize, 1, 255, 256, 257, 4096, 65_791, 65_792, 131_072, 131_073, 400_000,
    ] {
        for raw in [text(n), noise(n)] {
            let z = compress(&raw);
            assert_eq!(decompress(&z, MAX_RAW).unwrap(), raw, "size {n}");
        }
    }
}

#[test]
fn trailing_bytes_after_message_are_rejected() {
    let msg = ClientMsg::Ping {
        seq: 1,
        client_ms: 2,
    };
    let mut body = Postcard.encode(&msg).unwrap();
    body.push(0);
    let frame = Encoder::plain(&body);
    assert!(matches!(
        decode::<ClientMsg>(&frame, &Limits::GATEWAY),
        Err(WireError::Codec(_))
    ));
}
