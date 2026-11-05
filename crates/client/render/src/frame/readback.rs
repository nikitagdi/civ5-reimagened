//! Reading a frame back, for `golden` and the tests.
//!
//! `Renderer::read_frame(what)` asks for a copy of the next frame that is rendered; the returned
//! [`Readback`] gives the [`Image`] through `poll()` once the GPU has delivered it. Nothing waits:
//! on the web `poll` is the same call and the result arrives from the event loop. Natively
//! [`Readback::wait`] blocks for it.

use crate::perf::{Buf, Cluster, DeviceTrack};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use super::targets::ResourceId;

/// What to read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameSource {
    /// The world without the UI: `Scene`, `Rgba8Unorm`.
    World,
    /// The finished frame with the UI, as it goes to the surface.
    WithUi,
    /// The class mask (`R8Uint`, one byte per pixel); runs the mask pass for the frame.
    ClassMask,
    /// A target of the pool by name (not a multisampled one).
    Target(ResourceId),
}

/// The pixels of a read frame, rows tightly packed (no padding).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub size: (u32, u32),
    pub format: wgpu::TextureFormat,
    pub bytes: Vec<u8>,
}

impl Image {
    pub fn bytes_per_pixel(&self) -> usize {
        self.format.block_copy_size(None).unwrap_or(4) as usize
    }

    /// The bytes of the pixel at (`x`, `y`) from the top left.
    pub fn pixel(&self, x: u32, y: u32) -> &[u8] {
        let bpp = self.bytes_per_pixel();
        let at = (y as usize * self.size.0 as usize + x as usize) * bpp;
        &self.bytes[at..at + bpp]
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReadbackError {
    #[error("the frame cannot be read: {0}")]
    Unavailable(String),
    #[error("the GPU did not deliver the frame: {0}")]
    Gpu(String),
}

const WAITING: u8 = 0;
const MAPPING: u8 = 1;
const MAPPED: u8 = 2;
const MAP_FAILED: u8 = 3;

/// A buffer the frame was copied to, and how to unpack it.
struct Copied {
    buffer: Buf,
    size: (u32, u32),
    format: wgpu::TextureFormat,
    padded_row: u32,
    row: u32,
    mapped: Arc<AtomicU8>,
}

enum Stage {
    /// Waiting for a frame to be encoded.
    Waiting,
    /// The copy is in a command buffer that has not been submitted yet.
    Encoded(Copied),
    /// `map_async` was asked for.
    Mapping(Copied),
    Done(Option<Image>),
    Failed(ReadbackError),
}

struct Shared {
    what: FrameSource,
    stage: RefCell<Stage>,
    device: wgpu::Device,
}

/// A frame on its way. Cloning gives another handle to the same frame; the image is given once.
#[derive(Clone)]
pub struct Readback {
    shared: Rc<Shared>,
}

/// The renderer's side of a request.
pub(crate) struct Request {
    shared: Rc<Shared>,
}

impl Readback {
    pub(crate) fn new(device: &wgpu::Device, what: FrameSource) -> (Readback, Request) {
        let shared = Rc::new(Shared {
            what,
            stage: RefCell::new(Stage::Waiting),
            device: device.clone(),
        });
        (
            Readback {
                shared: Rc::clone(&shared),
            },
            Request { shared },
        )
    }

    pub fn source(&self) -> FrameSource {
        self.shared.what
    }

    /// `Ok(Some(image))` once, when the frame has arrived; `Ok(None)` while it is on its way (or
    /// after the image was taken); an error if it cannot be read.
    pub fn poll(&mut self) -> Result<Option<Image>, ReadbackError> {
        let _ = self.shared.device.poll(wgpu::PollType::Poll);
        let mut stage = self.shared.stage.borrow_mut();
        let result = match &*stage {
            Stage::Failed(e) => return Err(e.clone()),
            Stage::Done(_) => {
                return match std::mem::replace(&mut *stage, Stage::Done(None)) {
                    Stage::Done(image) => Ok(image),
                    _ => Ok(None),
                };
            }
            Stage::Mapping(copied) => match copied.mapped.load(Ordering::Acquire) {
                MAPPED => Ok(Some(unpack(copied))),
                MAP_FAILED => Err(ReadbackError::Gpu("the buffer could not be mapped".into())),
                _ => return Ok(None),
            },
            _ => return Ok(None),
        };
        match &result {
            Ok(_) => *stage = Stage::Done(None),
            Err(e) => *stage = Stage::Failed(e.clone()),
        }
        result
    }

    /// Blocks until the frame is there (native only: the browser must not wait). The next frame
    /// has to be rendered first, which is the caller's business.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn wait(mut self) -> Result<Image, ReadbackError> {
        loop {
            if let Some(image) = self.poll()? {
                return Ok(image);
            }
            if matches!(
                &*self.shared.stage.borrow_mut(),
                Stage::Waiting | Stage::Encoded(_)
            ) {
                return Err(ReadbackError::Unavailable(
                    "no frame was rendered after the request".into(),
                ));
            }
            let _ = self.shared.device.poll(wgpu::PollType::wait_indefinitely());
        }
    }
}

fn unpack(c: &Copied) -> Image {
    let range = c
        .buffer
        .get_mapped_range(..)
        .expect("a mapped readback buffer");
    let mut bytes = Vec::with_capacity(c.row as usize * c.size.1 as usize);
    for y in 0..c.size.1 as usize {
        let start = y * c.padded_row as usize;
        bytes.extend_from_slice(&range[start..start + c.row as usize]);
    }
    drop(range);
    c.buffer.unmap();
    Image {
        size: c.size,
        format: c.format,
        bytes,
    }
}

impl Request {
    pub(crate) fn what(&self) -> FrameSource {
        self.shared.what
    }

    /// The request cannot be met.
    pub(crate) fn fail(&self, why: impl Into<String>) {
        *self.shared.stage.borrow_mut() = Stage::Failed(ReadbackError::Unavailable(why.into()));
    }

    /// Encodes the copy of `texture` (`size`, `format`, needing `COPY_SRC`) into `encoder`.
    pub(crate) fn encode(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        texture: &wgpu::Texture,
        size: (u32, u32),
        format: wgpu::TextureFormat,
    ) {
        let Some(bpp) = format.block_copy_size(None) else {
            return self.fail(format!("{format:?} has no plain texel size"));
        };
        let row = size.0 * bpp;
        let padded_row = row.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buffer = device.buf(
            &wgpu::BufferDescriptor {
                label: Some("frame readback"),
                size: u64::from(padded_row) * u64::from(size.1),
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            },
            Cluster::RenderTargets,
        );
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_row),
                    rows_per_image: Some(size.1),
                },
            },
            wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
        );
        *self.shared.stage.borrow_mut() = Stage::Encoded(Copied {
            buffer,
            size,
            format,
            padded_row,
            row,
            mapped: Arc::new(AtomicU8::new(WAITING)),
        });
    }

    /// Asks for the map once the commands that hold the copy were submitted.
    pub(crate) fn submitted(&self) {
        let mut stage = self.shared.stage.borrow_mut();
        let copied = match std::mem::replace(&mut *stage, Stage::Waiting) {
            Stage::Encoded(copied) => copied,
            other => {
                *stage = other;
                return;
            }
        };
        copied.mapped.store(MAPPING, Ordering::Release);
        let flag = Arc::clone(&copied.mapped);
        copied.buffer.map_async(wgpu::MapMode::Read, .., move |r| {
            flag.store(
                if r.is_ok() { MAPPED } else { MAP_FAILED },
                Ordering::Release,
            );
        });
        *stage = Stage::Mapping(copied);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pixel_is_found_by_row_and_column() {
        let image = Image {
            size: (3, 2),
            format: wgpu::TextureFormat::R8Uint,
            bytes: vec![0, 1, 2, 3, 4, 5],
        };
        assert_eq!(image.pixel(2, 0), [2]);
        assert_eq!(image.pixel(0, 1), [3]);
        let rgba = Image {
            size: (2, 1),
            format: wgpu::TextureFormat::Rgba8Unorm,
            bytes: vec![1, 2, 3, 4, 5, 6, 7, 8],
        };
        assert_eq!(rgba.pixel(1, 0), [5, 6, 7, 8]);
    }
}
