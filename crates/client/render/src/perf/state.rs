//! A tracker of the state a render pass already has (B3, packet D): the draw loops set the
//! pipeline, the bind groups, the vertex and the index buffers for every call, and most of the time
//! the call before set the same ones. [`PassState`] remembers what was set last and skips a set that
//! changes nothing. Setting a pipeline does not unbind the groups or the buffers in WebGPU, so a
//! skipped set is never visible in the picture; the draws that come out are the same.
//!
//! A tracker lives for one loop (it knows nothing of what the code before the loop set), so make
//! one where the loop starts. The sets still go through the counters of [`PassCount`], so the
//! `pipeline_sets` and `bind_group_sets` of the HUD show the sets that were really made.

use std::ops::Range;

use super::draw::PassCount;

/// The slots of a pass: bind groups 0..4, vertex buffers 0..2.
const GROUPS: usize = 5;
const VERTEX_SLOTS: usize = 3;

type VertexSlot<'a> = Option<(&'a wgpu::Buffer, u64, u64)>;

#[derive(Default)]
pub struct PassState<'a> {
    pipeline: Option<&'a wgpu::RenderPipeline>,
    groups: [Option<&'a wgpu::BindGroup>; GROUPS],
    vertex: [VertexSlot<'a>; VERTEX_SLOTS],
    index: Option<(&'a wgpu::Buffer, u64, u64, wgpu::IndexFormat)>,
    /// Sets made and sets skipped, for the tests and the counters.
    pub made: u32,
    pub skipped: u32,
}

impl<'a> PassState<'a> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Forget everything: the code outside the loop touched the pass.
    pub fn reset(&mut self) {
        *self = Self {
            made: self.made,
            skipped: self.skipped,
            ..Self::default()
        };
    }

    /// The caller set `group` at `index` just before the loop.
    pub fn assume_group(&mut self, index: u32, group: &'a wgpu::BindGroup) {
        self.groups[index as usize] = Some(group);
    }

    pub fn pipeline(
        &mut self,
        pass: &mut wgpu::RenderPass<'_>,
        pipeline: &'a wgpu::RenderPipeline,
    ) {
        if self.pipeline == Some(pipeline) {
            self.skipped += 1;
            return;
        }
        self.made += 1;
        self.pipeline = Some(pipeline);
        pass.set_pipeline_c(pipeline);
    }

    /// A bind group with no dynamic offsets.
    pub fn group(
        &mut self,
        pass: &mut wgpu::RenderPass<'_>,
        index: u32,
        group: &'a wgpu::BindGroup,
    ) {
        let slot = &mut self.groups[index as usize];
        if *slot == Some(group) {
            self.skipped += 1;
            return;
        }
        self.made += 1;
        *slot = Some(group);
        pass.set_bind_group_c(index, group, &[]);
    }

    /// A bind group with dynamic offsets is always set (the offsets are part of the state), and
    /// the slot forgets what it held.
    pub fn group_offsets(
        &mut self,
        pass: &mut wgpu::RenderPass<'_>,
        index: u32,
        group: &wgpu::BindGroup,
        offsets: &[u32],
    ) {
        self.groups[index as usize] = None;
        self.made += 1;
        pass.set_bind_group_c(index, group, offsets);
    }

    /// The bytes `bytes` of `buffer` as the vertex buffer `slot`.
    pub fn vertex(
        &mut self,
        pass: &mut wgpu::RenderPass<'_>,
        slot: u32,
        buffer: &'a wgpu::Buffer,
        bytes: Range<u64>,
    ) {
        let now = Some((buffer, bytes.start, bytes.end));
        if self.vertex[slot as usize] == now {
            self.skipped += 1;
            return;
        }
        self.made += 1;
        self.vertex[slot as usize] = now;
        pass.set_vertex_buffer(slot, buffer.slice(bytes));
    }

    pub fn index(
        &mut self,
        pass: &mut wgpu::RenderPass<'_>,
        buffer: &'a wgpu::Buffer,
        bytes: Range<u64>,
        format: wgpu::IndexFormat,
    ) {
        let now = Some((buffer, bytes.start, bytes.end, format));
        if self.index == now {
            self.skipped += 1;
            return;
        }
        self.made += 1;
        self.index = now;
        pass.set_index_buffer(buffer.slice(bytes), format);
    }
}
