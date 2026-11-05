//! Instance data: permanent sets for what stays, a frame arena for what
//! changes every frame.
//!
//! - [`InstanceSet`] is for static instances (forest, buildings, landmarks): the buffer lives
//!   between frames and only the chunks that changed are written again (as `replace_set` of B1 did,
//!   but without a new buffer for every change).
//! - [`FrameArena`] is for live instances (animation, selection): one big buffer a frame, the
//!   batches are pieces of it.
//!
//! A draw binds a piece with `set_vertex_buffer(1, buffer.slice(offset..))` and draws instances
//! `0..count`. A first instance other than 0 and a `base_vertex` do not exist in WebGL2, so the
//! helpers here have no parameter for them ([`draw_indexed_instances`]).

use crate::perf::PassCount;
use crate::perf::{Buf, Cluster, DeviceTrack};
use std::ops::Range;

/// The vertex buffer slot of instance data, as in `static.wgsl` and the instanced shaders of
/// the original (the mesh has slot 0).
pub const INSTANCE_SLOT: u32 = 1;

/// Granularity of an [`InstanceSet`] upload, in bytes (a whole number of instances is used).
pub const CHUNK_BYTES: usize = 4096;

/// Where `wgpu` wants buffer writes and vertex buffer offsets aligned.
const ALIGN: u64 = wgpu::COPY_BUFFER_ALIGNMENT;

/// Binds `instances` to slot 1 and draws `index_count` indices of the bound index buffer for
/// `instance_count` instances: first instance 0 and `base_vertex` 0, always.
pub fn draw_indexed_instances(
    pass: &mut wgpu::RenderPass<'_>,
    instances: wgpu::BufferSlice<'_>,
    index_count: u32,
    instance_count: u32,
) {
    pass.set_vertex_buffer(INSTANCE_SLOT, instances);
    pass.draw_indexed_c(0..index_count, 0, 0..instance_count);
}

/// The byte ranges of `new` that differ from `shadow` (what the buffer holds), in pieces of
/// `chunk` bytes, neighbouring pieces joined. `new` past the end of `shadow` always differs.
pub fn changed_chunks(shadow: &[u8], new: &[u8], chunk: usize) -> Vec<Range<usize>> {
    let chunk = chunk.max(1);
    let mut out: Vec<Range<usize>> = Vec::new();
    let mut at = 0;
    while at < new.len() {
        let end = (at + chunk).min(new.len());
        if shadow.get(at..end) != Some(&new[at..end]) {
            match out.last_mut() {
                Some(last) if last.end == at => last.end = end,
                _ => out.push(at..end),
            }
        }
        at = end;
    }
    out
}

fn round_up(n: u64, to: u64) -> u64 {
    n.div_ceil(to) * to
}

/// A buffer of instances that lives between frames.
pub struct InstanceSet {
    label: String,
    buffer: Buf,
    capacity: u64,
    stride: usize,
    chunk: usize,
    /// What the buffer holds, to find the chunks to write.
    shadow: Vec<u8>,
    count: u32,
    last_upload: usize,
}

impl InstanceSet {
    /// A set of instances `stride` bytes each (a multiple of 4).
    pub fn new(device: &wgpu::Device, label: &str, stride: usize) -> Self {
        Self::with_chunk_instances(device, label, stride, (CHUNK_BYTES / stride.max(1)).max(1))
    }

    /// As [`Self::new`], with `chunk_instances` instances per upload piece.
    pub fn with_chunk_instances(
        device: &wgpu::Device,
        label: &str,
        stride: usize,
        chunk_instances: usize,
    ) -> Self {
        assert!(
            stride > 0 && (stride as u64).is_multiple_of(ALIGN),
            "instance stride {stride} is not a multiple of {ALIGN}"
        );
        let chunk = chunk_instances.max(1) * stride;
        Self {
            label: label.to_owned(),
            buffer: Self::make_buffer(device, label, chunk as u64),
            capacity: chunk as u64,
            stride,
            chunk,
            shadow: Vec::new(),
            count: 0,
            last_upload: 0,
        }
    }

    fn make_buffer(device: &wgpu::Device, label: &str, size: u64) -> Buf {
        device.buf(
            &wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
            Cluster::Models,
        )
    }

    pub fn stride(&self) -> usize {
        self.stride
    }

    /// Instances in the set.
    pub fn len(&self) -> u32 {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Bytes the last [`Self::update`] wrote.
    pub fn last_upload_bytes(&self) -> usize {
        self.last_upload
    }

    pub fn buffer(&self) -> &wgpu::Buffer {
        &self.buffer
    }

    /// Makes the set hold `data`, a whole number of instances. Only the chunks that differ from
    /// what the buffer holds are written; a set that outgrows its buffer gets a bigger one
    /// (twice the size) and is written whole. Returns the bytes written.
    pub fn update(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, data: &[u8]) -> usize {
        assert!(
            data.len().is_multiple_of(self.stride),
            "{} bytes are not whole instances of {}",
            data.len(),
            self.stride
        );
        let need = round_up(data.len() as u64, self.chunk as u64).max(self.chunk as u64);
        if need > self.capacity {
            self.capacity = need.next_power_of_two().max(need);
            self.buffer = Self::make_buffer(device, &self.label, self.capacity);
            self.shadow.clear();
        }
        let mut written = 0;
        for range in changed_chunks(&self.shadow, data, self.chunk) {
            // The tail chunk may be shorter than 4 bytes' multiple only if the stride is not;
            // the stride is a multiple of 4, so every piece is.
            queue.write_buffer(&self.buffer, range.start as u64, &data[range.clone()]);
            written += range.len();
        }
        self.shadow.clear();
        self.shadow.extend_from_slice(data);
        self.count = (data.len() / self.stride) as u32;
        self.last_upload = written;
        written
    }

    /// Instances `range` as a vertex buffer slice for `set_vertex_buffer(1, ...)`.
    pub fn slice(&self, range: Range<u32>) -> wgpu::BufferSlice<'_> {
        let s = self.stride as u64;
        self.buffer
            .slice(u64::from(range.start) * s..u64::from(range.end) * s)
    }

    /// All instances.
    pub fn all(&self) -> wgpu::BufferSlice<'_> {
        self.slice(0..self.count)
    }
}

/// Where a piece lies in the arena: page, byte offset, byte length.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArenaSlice {
    pub page: u32,
    pub offset: u64,
    pub len: u64,
}

/// The arithmetic of the arena without a GPU: pages of bytes, pieces placed one after the other.
#[derive(Clone, Debug)]
pub struct ArenaLayout {
    page_bytes: u64,
    /// (capacity, used) of each page.
    pages: Vec<(u64, u64)>,
}

impl ArenaLayout {
    pub fn new(page_bytes: u64) -> Self {
        Self {
            page_bytes: round_up(page_bytes.max(ALIGN), ALIGN),
            pages: Vec::new(),
        }
    }

    /// Places `len` bytes: after the last piece of the last page if they fit, else in a new page
    /// (as big as the piece needs, at least a page). Every offset is a multiple of 4.
    pub fn alloc(&mut self, len: u64) -> ArenaSlice {
        let len = round_up(len, ALIGN);
        if let Some((cap, used)) = self.pages.last_mut()
            && *used + len <= *cap
        {
            let offset = *used;
            *used += len;
            return ArenaSlice {
                page: self.pages.len() as u32 - 1,
                offset,
                len,
            };
        }
        self.pages.push((self.page_bytes.max(len), len));
        ArenaSlice {
            page: self.pages.len() as u32 - 1,
            offset: 0,
            len,
        }
    }

    pub fn pages(&self) -> usize {
        self.pages.len()
    }

    /// Bytes placed so far.
    pub fn used(&self) -> u64 {
        self.pages.iter().map(|p| p.1).sum()
    }

    /// Capacity of page `i`.
    pub fn capacity(&self, page: usize) -> u64 {
        self.pages[page].0
    }

    /// A new frame: every piece is gone. If the last frame needed more than one page the layout
    /// has one page for all of it from now on (its size the used bytes rounded up to a power of
    /// two); returns that size when the pages have to be made again.
    pub fn next_frame(&mut self) -> Option<u64> {
        let used = self.used();
        let grow = (self.pages.len() > 1).then(|| used.next_power_of_two().max(self.page_bytes));
        match grow {
            Some(size) => {
                self.page_bytes = size;
                self.pages.clear();
            }
            None => {
                for p in &mut self.pages {
                    p.1 = 0;
                }
            }
        }
        grow
    }
}

/// One big buffer a frame for the instances that change every frame; batches are pieces of it.
pub struct FrameArena {
    label: String,
    layout: ArenaLayout,
    pages: Vec<Buf>,
}

impl FrameArena {
    pub fn new(label: &str, page_bytes: u64) -> Self {
        Self {
            label: label.to_owned(),
            layout: ArenaLayout::new(page_bytes),
            pages: Vec::new(),
        }
    }

    /// Starts a frame: the pieces of the last one are free. After a frame that needed several
    /// pages the arena goes on with one page big enough for all.
    pub fn begin_frame(&mut self) {
        if self.layout.next_frame().is_some() {
            self.pages.clear();
        }
    }

    /// Copies `data` into the arena and returns where it lies.
    pub fn push(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, data: &[u8]) -> ArenaSlice {
        let piece = self.layout.alloc(data.len() as u64);
        while self.pages.len() < self.layout.pages() {
            let cap = self.layout.capacity(self.pages.len());
            self.pages.push(device.buf(
                &wgpu::BufferDescriptor {
                    label: Some(&self.label),
                    size: cap,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                },
                Cluster::Models,
            ));
        }
        if !data.is_empty() {
            let mut padded;
            let bytes = if (data.len() as u64).is_multiple_of(ALIGN) {
                data
            } else {
                padded = data.to_vec();
                padded.resize(piece.len as usize, 0);
                &padded
            };
            queue.write_buffer(&self.pages[piece.page as usize], piece.offset, bytes);
        }
        ArenaSlice {
            len: data.len() as u64,
            ..piece
        }
    }

    /// The piece as a vertex buffer slice for `set_vertex_buffer(1, ...)`.
    pub fn slice(&self, piece: ArenaSlice) -> wgpu::BufferSlice<'_> {
        self.pages[piece.page as usize].slice(piece.offset..piece.offset + piece.len)
    }

    /// Bytes placed this frame.
    pub fn used_bytes(&self) -> u64 {
        self.layout.used()
    }

    pub fn pages(&self) -> usize {
        self.pages.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu_test::{Gpu, gpu, read_texture};
    use wgpu::util::DeviceExt;

    fn rng(start: usize, end: usize) -> Range<usize> {
        start..end
    }

    #[test]
    fn only_the_chunks_that_differ_are_found() {
        let old: Vec<u8> = (0..32u8).collect();
        assert!(changed_chunks(&old, &old, 8).is_empty());
        let mut new = old.clone();
        new[9] = 99;
        assert_eq!(changed_chunks(&old, &new, 8), [rng(8, 16)]);
        // Two changes in neighbouring chunks are one write.
        new[17] = 98;
        new[18] = 97;
        assert_eq!(changed_chunks(&old, &new, 8), [rng(8, 24)]);
        // Far apart: two.
        let mut far = old.clone();
        far[0] = 5;
        far[31] = 5;
        assert_eq!(changed_chunks(&old, &far, 8), [0..8, 24..32]);
        // Growing: everything past the old end is new, also if the bytes are equal.
        let longer: Vec<u8> = (0..40u8).collect();
        assert_eq!(changed_chunks(&old, &longer, 8), [rng(32, 40)]);
        // The last chunk may be short.
        assert_eq!(changed_chunks(&[], &old[..12], 8), [rng(0, 12)]);
        assert!(changed_chunks(&old, &[], 8).is_empty());
    }

    #[test]
    fn arena_pieces_follow_each_other_and_are_aligned() {
        let mut a = ArenaLayout::new(256);
        let p0 = a.alloc(48);
        let p1 = a.alloc(10); // rounded to 12
        let p2 = a.alloc(96);
        assert_eq!((p0.page, p0.offset, p0.len), (0, 0, 48));
        assert_eq!((p1.page, p1.offset, p1.len), (0, 48, 12));
        assert_eq!((p2.page, p2.offset), (0, 60));
        for p in [p0, p1, p2] {
            assert_eq!(p.offset % 4, 0);
        }
        assert_eq!(a.used(), 156);
        // Not a first instance: the offset is the byte offset into the page, and the instance
        // count of every draw starts from 0 (`draw_indexed_instances` has no way to say otherwise).
        // A piece that does not fit opens a new page, as big as it needs.
        let big = a.alloc(1000);
        assert_eq!((big.page, big.offset), (1, 0));
        assert_eq!(a.capacity(1), 1000);
        assert_eq!(a.pages(), 2);
    }

    #[test]
    fn a_busy_frame_makes_the_next_one_a_single_page() {
        let mut a = ArenaLayout::new(64);
        a.alloc(40);
        a.alloc(40);
        a.alloc(40);
        assert_eq!(a.pages(), 3);
        // 120 bytes were used: one page of 128 from now on.
        assert_eq!(a.next_frame(), Some(128));
        assert_eq!((a.pages(), a.used()), (0, 0));
        let x = a.alloc(40);
        let y = a.alloc(40);
        let z = a.alloc(40);
        assert_eq!((x.page, y.page, z.page), (0, 0, 0));
        assert_eq!((x.offset, y.offset, z.offset), (0, 40, 80));
        // The next frame needs no change.
        assert_eq!(a.next_frame(), None);
        assert_eq!(a.used(), 0);
        assert_eq!(a.alloc(8).offset, 0);
    }

    /// Instance data of the GPU tests: a point at `x` pixels of a 4-pixel row, colour (r, g).
    fn instance(pixel: u32, r: f32, g: f32) -> [f32; 4] {
        [(pixel as f32 + 0.5) / 4.0 * 2.0 - 1.0, 0.0, r, g]
    }

    const POINTS_WGSL: &str = r"
struct Out { @builtin(position) pos: vec4<f32>, @location(0) color: vec2<f32> }
@vertex
fn vs(@location(0) inst: vec4<f32>) -> Out {
    var o: Out;
    o.pos = vec4<f32>(inst.xy, 0.0, 1.0);
    o.color = inst.zw;
    return o;
}
@fragment
fn fs(in: Out) -> @location(0) vec4<f32> {
    return vec4<f32>(in.color, 0.0, 1.0);
}
";

    /// Draws the pieces (each with `draw_instances` instances from 0) into a 4x1 target and returns
    /// its pixels.
    fn draw(gpu: &Gpu, pieces: &[(wgpu::BufferSlice<'_>, u32)]) -> Vec<[u8; 4]> {
        let shader = gpu
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("points"),
                source: wgpu::ShaderSource::Wgsl(POINTS_WGSL.into()),
            });
        let layout = gpu
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("points"),
                bind_group_layouts: &[],
                immediate_size: 0,
            });
        let pipeline = gpu
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("points"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    // Slot 0 is the mesh (one index, no attributes), slot 1 the instances.
                    buffers: &[
                        Some(wgpu::VertexBufferLayout {
                            array_stride: 4,
                            step_mode: wgpu::VertexStepMode::Vertex,
                            attributes: &[],
                        }),
                        Some(wgpu::VertexBufferLayout {
                            array_stride: 16,
                            step_mode: wgpu::VertexStepMode::Instance,
                            attributes: &wgpu::vertex_attr_array![0 => Float32x4],
                        }),
                    ],
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::PointList,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            });
        let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("row"),
            size: wgpu::Extent3d {
                width: 4,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let mesh = gpu
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("mesh"),
                contents: &[0u8; 4],
                usage: wgpu::BufferUsages::VERTEX,
            });
        let index = gpu
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("index"),
                contents: bytemuck::cast_slice(&[0u32]),
                usage: wgpu::BufferUsages::INDEX,
            });
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("row"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline_c(&pipeline);
            pass.set_vertex_buffer(0, mesh.slice(..));
            pass.set_index_buffer(index.slice(..), wgpu::IndexFormat::Uint32);
            for (slice, count) in pieces {
                // A point of one index, `count` instances from 0, base vertex 0.
                draw_indexed_instances(&mut pass, *slice, 1, *count);
            }
        }
        gpu.queue.submit([encoder.finish()]);
        read_texture(gpu, &target, 0, 0, (4, 1), 4)
            .chunks_exact(4)
            .map(|p| [p[0], p[1], p[2], p[3]])
            .collect()
    }

    /// Pieces of the frame arena at non-zero offsets draw their own instances with the instance
    /// range starting at 0 (no first instance, no base vertex).
    #[test]
    fn arena_pieces_at_offsets_draw_their_own_instances() {
        let Some(gpu) = gpu(
            "arena_pieces_at_offsets_draw_their_own_instances",
            wgpu::Backends::PRIMARY,
            wgpu::Features::empty(),
        ) else {
            return;
        };
        let mut arena = FrameArena::new("arena", 4096);
        for _frame in 0..2 {
            arena.begin_frame();
            let a = [instance(0, 1.0, 0.0), instance(1, 1.0, 0.0)];
            let b = [instance(2, 0.0, 1.0), instance(3, 0.0, 1.0)];
            let pa = arena.push(&gpu.device, &gpu.queue, bytemuck::cast_slice(&a));
            let pb = arena.push(&gpu.device, &gpu.queue, bytemuck::cast_slice(&b));
            assert_eq!(
                (pa.offset, pb.offset),
                (0, 32),
                "the second piece is not at the start"
            );
            assert_eq!(arena.used_bytes(), 64);
            let px = draw(&gpu, &[(arena.slice(pa), 2), (arena.slice(pb), 2)]);
            assert_eq!(
                px,
                [
                    [255, 0, 0, 255],
                    [255, 0, 0, 255],
                    [0, 255, 0, 255],
                    [0, 255, 0, 255]
                ]
            );
        }
    }

    /// A set written again with one instance changed writes that chunk and no other.
    #[test]
    fn a_set_rewrites_only_the_changed_chunk() {
        let Some(gpu) = gpu(
            "a_set_rewrites_only_the_changed_chunk",
            wgpu::Backends::PRIMARY,
            wgpu::Features::empty(),
        ) else {
            return;
        };
        // One instance per chunk, so a change shows in the number of bytes written.
        let mut set = InstanceSet::with_chunk_instances(&gpu.device, "set", 16, 1);
        let mut data = vec![
            instance(0, 1.0, 0.0),
            instance(1, 1.0, 0.0),
            instance(2, 1.0, 0.0),
            instance(3, 1.0, 0.0),
        ];
        let bytes = |d: &[[f32; 4]]| bytemuck::cast_slice::<_, u8>(d).to_vec();
        assert_eq!(set.update(&gpu.device, &gpu.queue, &bytes(&data)), 64);
        assert_eq!(set.len(), 4);
        let red = [255, 0, 0, 255];
        assert_eq!(draw(&gpu, &[(set.all(), 4)]), [red; 4]);

        // The same data: nothing is written.
        assert_eq!(set.update(&gpu.device, &gpu.queue, &bytes(&data)), 0);
        // One instance changes colour: one chunk of 16 bytes.
        data[2] = instance(2, 0.0, 1.0);
        assert_eq!(set.update(&gpu.device, &gpu.queue, &bytes(&data)), 16);
        assert_eq!(set.last_upload_bytes(), 16);
        assert_eq!(
            draw(&gpu, &[(set.all(), 4)]),
            [red, red, [0, 255, 0, 255], red]
        );
        // A range of the set: instances 2..4, drawn as instances 0..2 of a slice.
        assert_eq!(
            draw(&gpu, &[(set.slice(2..4), 2)]),
            [[0, 0, 0, 255], [0, 0, 0, 255], [0, 255, 0, 255], red]
        );
        // Growing: a bigger buffer, written whole; the data survives.
        let grown: Vec<[f32; 4]> = data
            .iter()
            .cloned()
            .chain([instance(3, 0.0, 1.0)])
            .collect();
        let written = set.update(&gpu.device, &gpu.queue, &bytes(&grown));
        assert_eq!(written, 80);
        assert_eq!(set.len(), 5);
        // Shrinking back writes only what differs (the data of the first four equals).
        assert_eq!(set.update(&gpu.device, &gpu.queue, &bytes(&data)), 0);
        assert_eq!(set.len(), 4);
    }
}
