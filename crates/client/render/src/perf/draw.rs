//! Draw statistics per pass (AC-H4). The render code calls `set_pipeline_c`, `set_bind_group_c`,
//! `draw_c` and `draw_indexed_c` of [`PassCount`] instead of the wgpu methods of a render pass; they
//! forward and bump global counters (single relaxed adds). The frame loop reads the counters around
//! each step and puts the difference into the pass row.

use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};

/// What a pass (or a whole frame) did on the command side.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DrawCounts {
    pub draw_calls: u32,
    pub triangles: u64,
    pub instances: u64,
    pub pipeline_sets: u32,
    pub bind_group_sets: u32,
}

impl DrawCounts {
    /// `self - before`, for counters that only grow.
    pub fn since(self, before: DrawCounts) -> DrawCounts {
        DrawCounts {
            draw_calls: self.draw_calls.wrapping_sub(before.draw_calls),
            triangles: self.triangles.wrapping_sub(before.triangles),
            instances: self.instances.wrapping_sub(before.instances),
            pipeline_sets: self.pipeline_sets.wrapping_sub(before.pipeline_sets),
            bind_group_sets: self.bind_group_sets.wrapping_sub(before.bind_group_sets),
        }
    }

    pub fn add(&mut self, o: DrawCounts) {
        self.draw_calls += o.draw_calls;
        self.triangles += o.triangles;
        self.instances += o.instances;
        self.pipeline_sets += o.pipeline_sets;
        self.bind_group_sets += o.bind_group_sets;
    }
}

static CALLS: AtomicU64 = AtomicU64::new(0);
static TRIS: AtomicU64 = AtomicU64::new(0);
static INSTANCES: AtomicU64 = AtomicU64::new(0);
static PIPELINES: AtomicU64 = AtomicU64::new(0);
static BIND_GROUPS: AtomicU64 = AtomicU64::new(0);

/// The totals since the start; take one before and one after a piece of work and use `since`.
pub fn counts() -> DrawCounts {
    DrawCounts {
        draw_calls: CALLS.load(Ordering::Relaxed) as u32,
        triangles: TRIS.load(Ordering::Relaxed),
        instances: INSTANCES.load(Ordering::Relaxed),
        pipeline_sets: PIPELINES.load(Ordering::Relaxed) as u32,
        bind_group_sets: BIND_GROUPS.load(Ordering::Relaxed) as u32,
    }
}

fn bump(call: u64, tris: u64, inst: u64) {
    CALLS.fetch_add(call, Ordering::Relaxed);
    TRIS.fetch_add(tris, Ordering::Relaxed);
    INSTANCES.fetch_add(inst, Ordering::Relaxed);
}

/// Drop-in for the counted methods of `wgpu::RenderPass`.
pub trait PassCount {
    fn set_pipeline_c(&mut self, pipeline: &wgpu::RenderPipeline);
    fn set_bind_group_c<'a, BG>(&mut self, index: u32, bind_group: BG, offsets: &[u32])
    where
        Option<&'a wgpu::BindGroup>: From<BG>;
    fn draw_c(&mut self, vertices: Range<u32>, instances: Range<u32>);
    fn draw_indexed_c(&mut self, indices: Range<u32>, base_vertex: i32, instances: Range<u32>);
}

impl PassCount for wgpu::RenderPass<'_> {
    fn set_pipeline_c(&mut self, pipeline: &wgpu::RenderPipeline) {
        PIPELINES.fetch_add(1, Ordering::Relaxed);
        self.set_pipeline(pipeline);
    }

    fn set_bind_group_c<'a, BG>(&mut self, index: u32, bind_group: BG, offsets: &[u32])
    where
        Option<&'a wgpu::BindGroup>: From<BG>,
    {
        BIND_GROUPS.fetch_add(1, Ordering::Relaxed);
        self.set_bind_group(index, bind_group, offsets);
    }

    fn draw_c(&mut self, vertices: Range<u32>, instances: Range<u32>) {
        let n = u64::from(instances.end.saturating_sub(instances.start));
        let v = u64::from(vertices.end.saturating_sub(vertices.start));
        bump(1, v / 3 * n, n);
        self.draw(vertices, instances);
    }

    fn draw_indexed_c(&mut self, indices: Range<u32>, base_vertex: i32, instances: Range<u32>) {
        let n = u64::from(instances.end.saturating_sub(instances.start));
        let i = u64::from(indices.end.saturating_sub(indices.start));
        bump(1, i / 3 * n, n);
        self.draw_indexed(indices, base_vertex, instances);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn since_subtracts() {
        let a = DrawCounts {
            draw_calls: 5,
            triangles: 100,
            instances: 9,
            pipeline_sets: 3,
            bind_group_sets: 7,
        };
        let b = DrawCounts {
            draw_calls: 2,
            triangles: 40,
            instances: 4,
            pipeline_sets: 1,
            bind_group_sets: 3,
        };
        let d = a.since(b);
        assert_eq!(d.draw_calls, 3);
        assert_eq!(d.triangles, 60);
        assert_eq!(d.bind_group_sets, 4);
    }
}
