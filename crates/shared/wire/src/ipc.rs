//! IPC frames between gateway and worker: `u32 LE` length + body (net-protocol 4.14, NP-D15). No compression.

use std::io::{self, Read, Write};

/// Longest body, bytes.
pub const MAX_FRAME: usize = 16 << 20;

/// Writes one frame and flushes. A body longer than `MAX_FRAME` is `InvalidInput`.
pub fn write_frame(w: &mut impl Write, raw: &[u8]) -> io::Result<()> {
    if raw.len() > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("ipc frame of {} bytes exceeds {MAX_FRAME}", raw.len()),
        ));
    }
    w.write_all(&(raw.len() as u32).to_le_bytes())?;
    w.write_all(raw)?;
    w.flush()
}

/// Reads frames from a stream that may return any number of bytes per call.
/// State survives an error such as `WouldBlock`: the next `next` call continues the same frame.
pub struct FrameReader<R: Read> {
    r: R,
    head: [u8; 4],
    head_have: usize,
    body: Vec<u8>,
    body_have: usize,
    /// Length of the frame being read, once the header is complete.
    len: Option<usize>,
    bad_length: Option<usize>,
}

impl<R: Read> FrameReader<R> {
    pub fn new(r: R) -> Self {
        FrameReader {
            r,
            head: [0; 4],
            head_have: 0,
            body: Vec::new(),
            body_have: 0,
            len: None,
            bad_length: None,
        }
    }

    /// Next frame body. `Ok(None)` is EOF at a frame boundary; EOF inside a frame is `UnexpectedEof`;
    /// a length above `MAX_FRAME` is `InvalidData` (checked before any allocation, and sticky).
    // The name is the spec's API (net-protocol 4.13); it returns `io::Result<Option<_>>`, not an `Iterator` item.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> io::Result<Option<Vec<u8>>> {
        if let Some(len) = self.bad_length {
            return Err(too_long(len));
        }
        while self.len.is_none() {
            match self.r.read(&mut self.head[self.head_have..]) {
                Ok(0) if self.head_have == 0 => return Ok(None),
                Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
                Ok(n) => {
                    self.head_have += n;
                    if self.head_have == self.head.len() {
                        let len = u32::from_le_bytes(self.head) as usize;
                        if len > MAX_FRAME {
                            self.bad_length = Some(len);
                            return Err(too_long(len));
                        }
                        self.len = Some(len);
                        self.body = vec![0; len];
                        self.body_have = 0;
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        let len = self.len.unwrap_or(0);
        while self.body_have < len {
            match self.r.read(&mut self.body[self.body_have..]) {
                Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
                Ok(n) => self.body_have += n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        self.len = None;
        self.head_have = 0;
        self.body_have = 0;
        Ok(Some(std::mem::take(&mut self.body)))
    }

    pub fn into_inner(self) -> R {
        self.r
    }
}

fn too_long(len: usize) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("ipc frame length {len} exceeds {MAX_FRAME}"),
    )
}
