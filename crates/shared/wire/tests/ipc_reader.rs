//! IPC frame tests (net-protocol 9): byte-by-byte reading, oversized length, EOF in the middle of a frame.

use std::io::{self, Cursor, Read};
use wire::ipc::{FrameReader, MAX_FRAME, write_frame};

/// Hands out at most `chunk` bytes per call.
struct Trickle<R> {
    inner: R,
    chunk: usize,
}

impl<R: Read> Read for Trickle<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = buf.len().min(self.chunk);
        self.inner.read(&mut buf[..n])
    }
}

fn frames(bodies: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::new();
    for b in bodies {
        write_frame(&mut out, b).unwrap();
    }
    out
}

#[test]
fn write_is_length_prefixed_little_endian() {
    let out = frames(&[b"abc"]);
    assert_eq!(out, [3, 0, 0, 0, b'a', b'b', b'c']);
}

#[test]
fn reads_frames_in_order_then_eof() {
    let big = vec![7u8; 100_000];
    let data = frames(&[b"one", b"", &big, b"four"]);
    let mut r = FrameReader::new(Cursor::new(data));
    assert_eq!(r.next().unwrap().unwrap(), b"one");
    assert_eq!(r.next().unwrap().unwrap(), b"");
    assert_eq!(r.next().unwrap().unwrap(), big);
    assert_eq!(r.next().unwrap().unwrap(), b"four");
    assert!(r.next().unwrap().is_none());
    assert!(r.next().unwrap().is_none());
}

#[test]
fn reads_one_byte_at_a_time() {
    let big: Vec<u8> = (0..5000u32).map(|i| (i % 251) as u8).collect();
    let data = frames(&[b"x", &big, b"", b"yz"]);
    let mut r = FrameReader::new(Trickle {
        inner: Cursor::new(data),
        chunk: 1,
    });
    assert_eq!(r.next().unwrap().unwrap(), b"x");
    assert_eq!(r.next().unwrap().unwrap(), big);
    assert_eq!(r.next().unwrap().unwrap(), b"");
    assert_eq!(r.next().unwrap().unwrap(), b"yz");
    assert!(r.next().unwrap().is_none());
}

#[test]
fn reads_with_every_chunk_size() {
    let data = frames(&[b"hello", &[9u8; 300], b"!"]);
    for chunk in 1..=9 {
        let mut r = FrameReader::new(Trickle {
            inner: Cursor::new(data.clone()),
            chunk,
        });
        assert_eq!(r.next().unwrap().unwrap(), b"hello", "chunk {chunk}");
        assert_eq!(r.next().unwrap().unwrap(), vec![9u8; 300], "chunk {chunk}");
        assert_eq!(r.next().unwrap().unwrap(), b"!", "chunk {chunk}");
        assert!(r.next().unwrap().is_none(), "chunk {chunk}");
    }
}

#[test]
fn length_above_max_frame_is_invalid_data() {
    let mut data = ((MAX_FRAME as u32) + 1).to_le_bytes().to_vec();
    data.extend_from_slice(&[0; 8]);
    let mut r = FrameReader::new(Cursor::new(data));
    let err = r.next().unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    // The stream is out of sync: the error stays.
    assert_eq!(r.next().unwrap_err().kind(), io::ErrorKind::InvalidData);
}

#[test]
fn length_u32_max_is_invalid_data() {
    let mut r = FrameReader::new(Cursor::new(u32::MAX.to_le_bytes().to_vec()));
    assert_eq!(r.next().unwrap_err().kind(), io::ErrorKind::InvalidData);
}

#[test]
fn max_frame_exactly_is_accepted() {
    let body = vec![1u8; MAX_FRAME];
    let data = frames(&[&body]);
    let mut r = FrameReader::new(Cursor::new(data));
    assert_eq!(r.next().unwrap().unwrap().len(), MAX_FRAME);
}

#[test]
fn write_rejects_body_over_max_frame() {
    let body = vec![0u8; MAX_FRAME + 1];
    let mut out = Vec::new();
    let err = write_frame(&mut out, &body).unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    assert!(out.is_empty());
}

#[test]
fn eof_inside_header_is_unexpected_eof() {
    for cut in 1..4 {
        let data = frames(&[b"abc"])[..cut].to_vec();
        let mut r = FrameReader::new(Cursor::new(data));
        assert_eq!(
            r.next().unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof,
            "cut {cut}"
        );
    }
}

#[test]
fn eof_inside_body_is_unexpected_eof() {
    let full = frames(&[b"abcdef"]);
    for cut in 4..full.len() {
        let mut r = FrameReader::new(Cursor::new(full[..cut].to_vec()));
        assert_eq!(
            r.next().unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof,
            "cut {cut}"
        );
    }
}

/// Reports `WouldBlock` between bytes, as a non-blocking pipe would; the frame must survive it.
struct Flaky {
    data: Vec<u8>,
    pos: usize,
    block_next: bool,
}

impl Read for Flaky {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.block_next {
            self.block_next = false;
            return Err(io::ErrorKind::WouldBlock.into());
        }
        self.block_next = true;
        if self.pos == self.data.len() {
            return Ok(0);
        }
        buf[0] = self.data[self.pos];
        self.pos += 1;
        Ok(1)
    }
}

#[test]
fn would_block_does_not_lose_the_frame() {
    let mut r = FrameReader::new(Flaky {
        data: frames(&[b"hello", b"!"]),
        pos: 0,
        block_next: true,
    });
    let mut got = Vec::new();
    for _ in 0..200 {
        match r.next() {
            Ok(Some(f)) => got.push(f),
            Ok(None) => break,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
            Err(e) => panic!("{e}"),
        }
    }
    assert_eq!(got, vec![b"hello".to_vec(), b"!".to_vec()]);
}
