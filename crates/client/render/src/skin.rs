//! The bone texture: skin matrices of every animated instance in one
//! `Rgba32Float` texture, read by the vertex shader with `textureLoad` (`common/skin.wgsl`). No
//! storage buffers (WebGL2), and no 128-matrix palette in a uniform block.
//!
//! One bone is a 3 x 4 matrix of three texels, one per row of the matrix (the last column is the
//! translation). A row of the texture holds [`BONES_PER_ROW`] bones, so a bone slot `s` lies at
//! texel `((s % 512) * 3, s / 512)`. An instance owns a range of consecutive slots and hands the
//! first one to the shader as `bone_base`.
//!
//! [`BoneStore`] is the CPU half (slots, the shadow copy, the upload plan) and needs no GPU;
//! [`BoneTexture`] adds the texture. Writes only touch the shadow copy and mark the slots dirty;
//! [`BoneTexture::flush`] sends at most [`UPLOAD_LIMIT_BYTES`] a frame and keeps the rest for the
//! next ones.

use crate::perf::{Cluster, DeviceTrack, Tex};
use std::collections::BTreeMap;
use std::ops::Range;

use glam::Mat4;

use crate::anim::BoneSlots;

/// Texels of one bone: the three rows of its matrix.
pub const TEXELS_PER_BONE: u32 = 3;
/// Bones in a row of the texture.
pub const BONES_PER_ROW: u32 = 512;
/// Width of the texture.
// re: 1536 texels, 512 bones a row
pub const TEXTURE_WIDTH: u32 = BONES_PER_ROW * TEXELS_PER_BONE;
/// Highest the texture grows: the limit of WebGL2 (`downlevel_webgl2_defaults`).
pub const MAX_ROWS: u32 = 2048;
/// Height of the texture at creation; it doubles when the slots no longer fit.
pub const INITIAL_ROWS: u32 = 16;
/// Slots of the texture at its highest.
pub const MAX_SLOTS: u32 = BONES_PER_ROW * MAX_ROWS;
/// Bytes of one bone: 12 `f32`.
pub const BYTES_PER_BONE: usize = 48;
/// What [`BoneTexture::flush`] sends in one frame; the rest waits for the next one.
// SIMPLIFIED: spec 4.12 says "512 КиБ (настраивается)"; the setting is `BoneTexture::set_upload_limit`
pub const UPLOAD_LIMIT_BYTES: usize = 512 * 1024;

/// A bone as the texture holds it: the rows of the 3 x 4 matrix, one after the other.
pub type Bone = [f32; 12];

/// The identity bone, for the rigid parts of a model.
pub const IDENTITY_BONE: Bone = [
    1.0, 0.0, 0.0, 0.0, //
    0.0, 1.0, 0.0, 0.0, //
    0.0, 0.0, 1.0, 0.0,
];

/// The texel of the first row of the bone in `slot`: `(x, y)`; the next two are at `x + 1`, `x + 2`.
/// Mirrored by `bone_texel` of `skin.wgsl`.
pub fn bone_texel(slot: u32) -> (u32, u32) {
    (
        (slot % BONES_PER_ROW) * TEXELS_PER_BONE,
        slot / BONES_PER_ROW,
    )
}

/// Rows of the texture that hold `slots` slots.
pub fn rows_for(slots: u32) -> u32 {
    slots.div_ceil(BONES_PER_ROW)
}

/// The skin matrix `m` (column vectors, `glam`) as a bone: row `i` of the matrix is texel `i`.
pub fn pack_bone(m: &Mat4) -> Bone {
    let c = m.to_cols_array_2d();
    std::array::from_fn(|k| c[k % 4][k / 4])
}

/// A part of a row that has to go to the GPU: bones `lo..hi` of `row`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Upload {
    pub row: u32,
    pub lo: u32,
    pub hi: u32,
}

impl Upload {
    pub fn bytes(&self) -> usize {
        (self.hi - self.lo) as usize * BYTES_PER_BONE
    }
}

/// Ranges of free slots, ordered and never touching (they are joined), and the first-fit search.
struct SlotAllocator {
    free: Vec<Range<u32>>,
}

impl SlotAllocator {
    fn new() -> Self {
        Self {
            free: std::iter::once(0..MAX_SLOTS).collect(),
        }
    }

    fn alloc(&mut self, slots: u32) -> Option<Range<u32>> {
        if slots == 0 {
            return None;
        }
        let i = self.free.iter().position(|r| r.len() >= slots as usize)?;
        let start = self.free[i].start;
        if self.free[i].len() == slots as usize {
            self.free.remove(i);
        } else {
            self.free[i].start += slots;
        }
        Some(start..start + slots)
    }

    /// `false` when `range` is not an allocated range: out of bounds or overlapping free slots.
    fn free(&mut self, range: Range<u32>) -> bool {
        if range.is_empty() || range.end > MAX_SLOTS {
            return false;
        }
        let at = self.free.partition_point(|r| r.end <= range.start);
        if self.free.get(at).is_some_and(|r| r.start < range.end) {
            return false;
        }
        // Join with the neighbours that touch.
        let joins_before = at > 0 && self.free[at - 1].end == range.start;
        let joins_after = self.free.get(at).is_some_and(|r| r.start == range.end);
        match (joins_before, joins_after) {
            (true, true) => {
                self.free[at - 1].end = self.free[at].end;
                self.free.remove(at);
            }
            (true, false) => self.free[at - 1].end = range.end,
            (false, true) => self.free[at].start = range.start,
            (false, false) => self.free.insert(at, range),
        }
        true
    }

    fn used(&self) -> u32 {
        MAX_SLOTS - self.free.iter().map(|r| r.len() as u32).sum::<u32>()
    }

    /// One past the highest slot in use.
    fn high_water(&self) -> u32 {
        match self.free.last() {
            Some(r) if r.end == MAX_SLOTS => r.start,
            _ => MAX_SLOTS,
        }
    }
}

/// The slots, the shadow copy of their contents and what is still to send.
pub struct BoneStore {
    slots: SlotAllocator,
    /// Contents by slot; as long as the highest slot written.
    shadow: Vec<Bone>,
    /// Row to the bones `lo..hi` of it that changed.
    dirty: BTreeMap<u32, (u32, u32)>,
    upload_limit: usize,
}

impl Default for BoneStore {
    fn default() -> Self {
        Self::new()
    }
}

impl BoneStore {
    pub fn new() -> Self {
        Self {
            slots: SlotAllocator::new(),
            shadow: Vec::new(),
            dirty: BTreeMap::new(),
            upload_limit: UPLOAD_LIMIT_BYTES,
        }
    }

    /// `slots` consecutive bones, or `None` for zero or when the texture is full.
    pub fn alloc(&mut self, slots: u32) -> Option<Range<u32>> {
        self.slots.alloc(slots)
    }

    /// Gives a range back. Ranges that were not allocated are ignored and counted as `false`.
    pub fn free(&mut self, range: Range<u32>) -> bool {
        if range.is_empty() {
            return false;
        }
        // Writes that were not sent yet are of no use to anybody now.
        let ok = self.slots.free(range.clone());
        if ok {
            self.forget(range);
        }
        ok
    }

    fn forget(&mut self, range: Range<u32>) {
        for row in range.start / BONES_PER_ROW..=(range.end - 1) / BONES_PER_ROW {
            let (row_lo, row_hi) = (row * BONES_PER_ROW, (row + 1) * BONES_PER_ROW);
            let (a, b) = (
                range.start.max(row_lo) - row_lo,
                range.end.min(row_hi) - row_lo,
            );
            let Some(&(lo, hi)) = self.dirty.get(&row) else {
                continue;
            };
            // Only a dirty span that is wholly inside the range goes; a partial overlap stays, so
            // the neighbour's bones in it are still sent.
            if a <= lo && hi <= b {
                self.dirty.remove(&row);
            }
        }
    }

    /// Writes `bones` from the first slot of `range` on (as many as fit in the range).
    pub fn write(&mut self, range: Range<u32>, bones: &[Bone]) {
        let n = bones.len().min(range.len());
        if n == 0 {
            return;
        }
        let start = range.start as usize;
        if self.shadow.len() < start + n {
            self.shadow.resize(start + n, [0.0; 12]);
        }
        self.shadow[start..start + n].copy_from_slice(&bones[..n]);
        let end = range.start + n as u32;
        for row in range.start / BONES_PER_ROW..=(end - 1) / BONES_PER_ROW {
            let (row_lo, row_hi) = (row * BONES_PER_ROW, (row + 1) * BONES_PER_ROW);
            let a = range.start.max(row_lo) - row_lo;
            let b = end.min(row_hi) - row_lo;
            self.dirty
                .entry(row)
                .and_modify(|(lo, hi)| {
                    *lo = (*lo).min(a);
                    *hi = (*hi).max(b);
                })
                .or_insert((a, b));
        }
    }

    pub fn set_upload_limit(&mut self, bytes: usize) {
        self.upload_limit = bytes;
    }

    /// What goes to the GPU this frame: dirty parts by row, at most the upload limit in all (but
    /// always one bone, so a limit of zero does not stall). What is not planned stays dirty.
    pub fn plan_upload(&mut self) -> Vec<Upload> {
        let mut left = self.upload_limit.max(BYTES_PER_BONE);
        let mut out = Vec::new();
        let rows: Vec<u32> = self.dirty.keys().copied().collect();
        for row in rows {
            let fit = left / BYTES_PER_BONE;
            if fit == 0 {
                break;
            }
            let (lo, hi) = self.dirty[&row];
            let n = ((hi - lo) as usize).min(fit) as u32;
            out.push(Upload {
                row,
                lo,
                hi: lo + n,
            });
            left -= n as usize * BYTES_PER_BONE;
            if lo + n == hi {
                self.dirty.remove(&row);
            } else {
                self.dirty.insert(row, (lo + n, hi));
            }
        }
        out
    }

    /// The contents of the bones of `upload`, as bytes for the texture.
    pub fn bytes_of(&self, upload: &Upload) -> &[u8] {
        let start = (upload.row * BONES_PER_ROW + upload.lo) as usize;
        let end = (upload.row * BONES_PER_ROW + upload.hi) as usize;
        bytemuck::cast_slice(&self.shadow[start..end])
    }

    /// The bone in `slot` as last written (zeros if never).
    pub fn bone(&self, slot: u32) -> Bone {
        self.shadow.get(slot as usize).copied().unwrap_or([0.0; 12])
    }

    /// Bytes that wait for a flush.
    pub fn pending_bytes(&self) -> usize {
        self.dirty
            .values()
            .map(|&(lo, hi)| (hi - lo) as usize * BYTES_PER_BONE)
            .sum()
    }

    /// Slots in use.
    pub fn used_slots(&self) -> u32 {
        self.slots.used()
    }

    /// Rows of the texture that hold the slots in use (`?stats=1`).
    pub fn used_rows(&self) -> u32 {
        rows_for(self.slots.high_water())
    }
}

/// The store is where `AnimService` bakes the clips: the texture itself
/// belongs to `WorldServices`, which sends the store's changes with [`BoneTexture::flush_from`].
impl BoneSlots for BoneStore {
    fn alloc(&mut self, slots: u32) -> Option<Range<u32>> {
        BoneStore::alloc(self, slots)
    }

    fn free(&mut self, range: Range<u32>) -> bool {
        BoneStore::free(self, range)
    }

    fn write(&mut self, range: Range<u32>, bones: &[Bone]) {
        BoneStore::write(self, range, bones);
    }

    fn used_slots(&self) -> u32 {
        BoneStore::used_slots(self)
    }

    fn capacity(&self) -> u32 {
        MAX_SLOTS
    }
}

/// The texture, its store and the number of its creation.
pub struct BoneTexture {
    store: BoneStore,
    texture: Tex,
    view: wgpu::TextureView,
    rows: u32,
    generation: u64,
    uploaded_last: usize,
}

fn create(device: &wgpu::Device, rows: u32) -> (Tex, wgpu::TextureView) {
    let texture = device.tex(
        &wgpu::TextureDescriptor {
            label: Some("BoneTexture"),
            size: wgpu::Extent3d {
                width: TEXTURE_WIDTH,
                height: rows,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba32Float,
            // COPY_SRC: growing copies the old rows; the tests read the texture back.
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        },
        Cluster::Animation,
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

impl BoneTexture {
    pub fn new(device: &wgpu::Device) -> Self {
        let (texture, view) = create(device, INITIAL_ROWS);
        Self {
            store: BoneStore::new(),
            texture,
            view,
            rows: INITIAL_ROWS,
            generation: 1,
            uploaded_last: 0,
        }
    }

    /// The layout of the bind group that holds the texture, for `skin.wgsl`: binding 14 of group 1,
    /// read by the vertex stage with `textureLoad`.
    pub fn layout_entry() -> wgpu::BindGroupLayoutEntry {
        wgpu::BindGroupLayoutEntry {
            binding: BONE_TEXTURE_BINDING,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        }
    }

    pub fn alloc(&mut self, slots: u32) -> Option<Range<u32>> {
        self.store.alloc(slots)
    }

    pub fn free(&mut self, range: Range<u32>) -> bool {
        self.store.free(range)
    }

    pub fn write(&mut self, range: Range<u32>, bones: &[Bone]) {
        self.store.write(range, bones);
    }

    pub fn store(&self) -> &BoneStore {
        &self.store
    }

    pub fn set_upload_limit(&mut self, bytes: usize) {
        self.store.set_upload_limit(bytes);
    }

    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    pub fn texture(&self) -> &wgpu::Texture {
        &self.texture
    }

    pub fn rows(&self) -> u32 {
        self.rows
    }

    /// Grows each time the texture is made again; a bind group made of [`Self::view`] is then stale.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Bytes the last [`Self::flush`] sent.
    pub fn uploaded_last(&self) -> usize {
        self.uploaded_last
    }

    /// Makes room for the slots in use (a bigger texture keeps the rows it has), then sends this
    /// frame's share of the changes. Call once a frame, before the passes.
    pub fn flush(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let mut store = std::mem::take(&mut self.store);
        self.flush_from(&mut store, device, queue);
        self.store = store;
    }

    /// [`Self::flush`] for a store that lives elsewhere (the one inside `AnimService`): the texture
    /// is the GPU half of that store.
    pub fn flush_from(
        &mut self,
        store: &mut BoneStore,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) {
        let need = store.used_rows().min(MAX_ROWS);
        if need > self.rows {
            self.grow(device, queue, need);
        }
        let mut sent = 0;
        for up in store.plan_upload() {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: up.lo * TEXELS_PER_BONE,
                        y: up.row,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                store.bytes_of(&up),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: None,
                    rows_per_image: None,
                },
                wgpu::Extent3d {
                    width: (up.hi - up.lo) * TEXELS_PER_BONE,
                    height: 1,
                    depth_or_array_layers: 1,
                },
            );
            sent += up.bytes();
        }
        self.uploaded_last = sent;
    }

    fn grow(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, need: u32) {
        let rows = need.next_power_of_two().clamp(INITIAL_ROWS, MAX_ROWS);
        let (texture, view) = create(device, rows);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("BoneTexture grow"),
        });
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: TEXTURE_WIDTH,
                height: self.rows,
                depth_or_array_layers: 1,
            },
        );
        queue.submit([encoder.finish()]);
        self.texture = texture;
        self.view = view;
        self.rows = rows;
        self.generation += 1;
    }
}

/// The texture with a store of its own as the home of the clips (the tests; the world keeps the
/// store in `AnimService` and the texture in `WorldServices`).
impl BoneSlots for BoneTexture {
    fn alloc(&mut self, slots: u32) -> Option<Range<u32>> {
        self.store.alloc(slots)
    }

    fn free(&mut self, range: Range<u32>) -> bool {
        self.store.free(range)
    }

    fn write(&mut self, range: Range<u32>, bones: &[Bone]) {
        self.store.write(range, bones);
    }

    fn used_slots(&self) -> u32 {
        self.store.used_slots()
    }

    fn capacity(&self) -> u32 {
        MAX_SLOTS
    }
}

/// Binding of the bone texture in group 1; `bone_tex` of `skin.wgsl`.
pub const BONE_TEXTURE_BINDING: u32 = 14;

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    fn bone(v: f32) -> Bone {
        [v; 12]
    }

    #[test]
    fn addresses_at_the_borders_of_rows_and_of_the_texture() {
        assert_eq!(TEXTURE_WIDTH, 1536);
        assert_eq!(bone_texel(0), (0, 0));
        assert_eq!(bone_texel(1), (3, 0));
        assert_eq!(
            bone_texel(511),
            (1533, 0),
            "the last bone of a row ends at the width"
        );
        assert_eq!(bone_texel(512), (0, 1));
        assert_eq!(bone_texel(513), (3, 1));
        assert_eq!(bone_texel(MAX_SLOTS - 1), (1533, MAX_ROWS - 1));
        assert_eq!(bone_texel(MAX_SLOTS - 1).0 + TEXELS_PER_BONE, TEXTURE_WIDTH);
        assert_eq!(rows_for(0), 0);
        assert_eq!(rows_for(1), 1);
        assert_eq!(rows_for(512), 1);
        assert_eq!(rows_for(513), 2);
        assert_eq!(rows_for(MAX_SLOTS), MAX_ROWS);
    }

    #[test]
    fn a_matrix_packs_into_its_rows() {
        let m = Mat4::from_translation(Vec3::new(7.0, 8.0, 9.0))
            * Mat4::from_scale(Vec3::new(2.0, 3.0, 4.0));
        let b = pack_bone(&m);
        // Row 0 is (sx, 0, 0, tx), and so on.
        assert_eq!(b[0..4], [2.0, 0.0, 0.0, 7.0]);
        assert_eq!(b[4..8], [0.0, 3.0, 0.0, 8.0]);
        assert_eq!(b[8..12], [0.0, 0.0, 4.0, 9.0]);
        assert_eq!(pack_bone(&Mat4::IDENTITY), IDENTITY_BONE);
        // Applying the rows to (p, 1) is the matrix applied to p.
        let p = Vec3::new(1.0, -2.0, 0.5);
        let by_rows = |b: &Bone| {
            Vec3::new(
                b[0] * p.x + b[1] * p.y + b[2] * p.z + b[3],
                b[4] * p.x + b[5] * p.y + b[6] * p.z + b[7],
                b[8] * p.x + b[9] * p.y + b[10] * p.z + b[11],
            )
        };
        assert!(by_rows(&b).abs_diff_eq(m.transform_point3(p), 1e-6));
    }

    #[test]
    fn alloc_and_free_leave_nothing_behind() {
        let mut s = BoneStore::new();
        let a = s.alloc(100).unwrap();
        let b = s.alloc(50).unwrap();
        let c = s.alloc(400).unwrap();
        assert_eq!(
            (a.clone(), b.clone(), c.clone()),
            (0..100, 100..150, 150..550)
        );
        assert_eq!(s.used_slots(), 550);
        assert_eq!(s.used_rows(), 2);
        // Freed in the middle first, then the neighbours: the holes join.
        assert!(s.free(b.clone()));
        assert!(!s.free(b.clone()), "a second free is refused");
        assert!(s.free(a));
        assert!(s.free(c));
        assert_eq!(s.used_slots(), 0);
        assert_eq!(s.used_rows(), 0);
        // All of it is one range again: the whole texture fits.
        assert_eq!(s.alloc(MAX_SLOTS), Some(0..MAX_SLOTS));
        assert_eq!(s.alloc(1), None, "full");
        assert!(!s.free(0..MAX_SLOTS + 1));
        assert!(s.free(0..MAX_SLOTS));
        assert_eq!(s.alloc(0), None);
    }

    #[test]
    fn a_hole_is_reused_before_the_texture_grows() {
        let mut s = BoneStore::new();
        let a = s.alloc(10).unwrap();
        let _b = s.alloc(10).unwrap();
        s.free(a);
        assert_eq!(s.alloc(4), Some(0..4));
        assert_eq!(s.alloc(6), Some(4..10));
        assert_eq!(
            s.alloc(1),
            Some(20..21),
            "the hole is full, the next goes after"
        );
        // Churn does not leak: 1000 rounds of alloc and free end where they began.
        let mut ranges = Vec::new();
        for round in 0..1000u32 {
            ranges.push(s.alloc(1 + round % 37).unwrap());
            if round % 3 == 2 {
                let r = ranges.remove((round as usize * 7) % ranges.len());
                assert!(s.free(r));
            }
        }
        for r in ranges {
            assert!(s.free(r));
        }
        assert_eq!(s.used_slots(), 21);
    }

    #[test]
    fn a_write_marks_the_rows_it_touches() {
        let mut s = BoneStore::new();
        let r = s.alloc(600).unwrap();
        s.write(r.clone(), &vec![bone(1.0); 600]);
        // Row 0 whole, row 1 the first 88.
        assert_eq!(s.pending_bytes(), 600 * BYTES_PER_BONE);
        let plan = s.plan_upload();
        assert_eq!(
            plan,
            [
                Upload {
                    row: 0,
                    lo: 0,
                    hi: 512
                },
                Upload {
                    row: 1,
                    lo: 0,
                    hi: 88
                }
            ]
        );
        assert_eq!(s.pending_bytes(), 0);
        assert_eq!(s.bytes_of(&plan[1]).len(), 88 * BYTES_PER_BONE);
        assert_eq!(s.bone(599), bone(1.0));
        // A second write widens only its own span.
        s.write(10..12, &[bone(2.0), bone(3.0)]);
        assert_eq!(
            s.plan_upload(),
            [Upload {
                row: 0,
                lo: 10,
                hi: 12
            }]
        );
        assert_eq!(s.bone(11), bone(3.0));
        // More bones than the range holds: the extra are dropped.
        let tiny = s.alloc(2).unwrap();
        s.write(tiny.clone(), &[bone(5.0); 5]);
        assert_eq!(s.pending_bytes(), 2 * BYTES_PER_BONE);
    }

    #[test]
    fn the_upload_limit_holds_and_the_rest_waits_for_the_next_frame() {
        let mut s = BoneStore::new();
        let slots = 3 * 512 + 100;
        let r = s.alloc(slots).unwrap();
        s.write(r, &vec![bone(1.0); slots as usize]);
        let limit = 700 * BYTES_PER_BONE;
        s.set_upload_limit(limit);
        let mut frames = 0;
        let mut sent = 0;
        while s.pending_bytes() > 0 {
            let plan = s.plan_upload();
            let bytes: usize = plan.iter().map(Upload::bytes).sum();
            assert!(bytes <= limit, "frame {frames}: {bytes} > {limit}");
            assert!(bytes > 0);
            sent += bytes;
            frames += 1;
            assert!(frames < 100);
        }
        assert_eq!(
            sent,
            slots as usize * BYTES_PER_BONE,
            "everything arrives once"
        );
        assert_eq!(frames, 3, "1636 bones at 700 a frame");
        // The real limit is 512 KiB: 10922 bones.
        let mut s = BoneStore::new();
        let r = s.alloc(20_000).unwrap();
        s.write(r, &vec![bone(1.0); 20_000]);
        let first: usize = s.plan_upload().iter().map(Upload::bytes).sum();
        assert!(first <= UPLOAD_LIMIT_BYTES && first > UPLOAD_LIMIT_BYTES - BYTES_PER_BONE);
        // A limit of zero still moves.
        s.set_upload_limit(0);
        assert_eq!(
            s.plan_upload().iter().map(Upload::bytes).sum::<usize>(),
            BYTES_PER_BONE
        );
    }

    #[test]
    fn a_freed_range_drops_its_unsent_writes() {
        let mut s = BoneStore::new();
        let a = s.alloc(10).unwrap();
        s.write(a.clone(), &[bone(1.0); 10]);
        assert!(s.free(a));
        assert_eq!(s.pending_bytes(), 0);
        // A neighbour's write in the same row survives the free of another range.
        let a = s.alloc(10).unwrap();
        let b = s.alloc(10).unwrap();
        s.write(b.clone(), &[bone(2.0); 10]);
        s.write(a.clone(), &[bone(1.0); 10]);
        assert!(s.free(a));
        assert_eq!(
            s.plan_upload(),
            [Upload {
                row: 0,
                lo: 0,
                hi: 20
            }]
        );
        assert_eq!(s.bone(b.start), bone(2.0));
    }
}

#[cfg(test)]
mod gpu_tests {
    use super::*;
    use crate::gpu_test::{gpu, read_texture};

    fn bone(v: f32) -> Bone {
        [v; 12]
    }

    fn floats(bytes: &[u8]) -> Vec<f32> {
        bytemuck::cast_slice(bytes).to_vec()
    }

    #[test]
    fn the_texture_holds_what_was_written_and_keeps_it_when_it_grows() {
        let Some(g) = gpu(
            "bone texture",
            wgpu::Backends::all(),
            wgpu::Features::empty(),
        ) else {
            return;
        };
        let mut t = BoneTexture::new(&g.device);
        assert_eq!((t.rows(), t.generation()), (INITIAL_ROWS, 1));
        let a = t.alloc(600).unwrap();
        let bones: Vec<Bone> = (0..600).map(|i| bone(i as f32)).collect();
        t.write(a.clone(), &bones);
        t.flush(&g.device, &g.queue);
        assert_eq!(t.generation(), 1, "two rows fit");
        // Past the 16 rows: the texture doubles and the old rows are copied.
        let far = t.alloc(MAX_SLOTS / 64 * 3).unwrap();
        let last = far.end - 1;
        t.write(last..far.end, &[bone(-1.0)]);
        t.flush(&g.device, &g.queue);
        assert!(t.rows() > INITIAL_ROWS, "{}", t.rows());
        assert_eq!(t.generation(), 2);
        let data = floats(&read_texture(
            &g,
            t.texture(),
            0,
            0,
            (TEXTURE_WIDTH, t.rows()),
            16,
        ));
        for slot in [0u32, 1, 511, 512, 599] {
            let (x, y) = bone_texel(slot);
            let at = ((y * TEXTURE_WIDTH + x) * 4) as usize;
            assert_eq!(data[at..at + 12], bone(slot as f32), "slot {slot}");
        }
        let (x, y) = bone_texel(last);
        let at = ((y * TEXTURE_WIDTH + x) * 4) as usize;
        assert_eq!(data[at..at + 12], bone(-1.0));
    }
}
