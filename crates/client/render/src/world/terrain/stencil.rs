//! The stencil of the terrain: `TerrainStencil`, a grid of bit masks
//! at half the resolution of the field, in pages of 16 x 16. The stages of `world-water` add the
//! coast to it (`AddStencil`), the layout of forests and the objects read it
//! (`SampleWorldPosition`). A page is allocated at the first write.
//!
//! What the bits mean is the business of the writers (R3.4); this file holds the grid.

use glam::Vec2;

use super::sampler::TerrainFrame;

/// Texels per side of a page.
// re: TerrainStencil::StartupGame (App/TerrainStencil.cpp:35-36, `>> 4`)
pub const PAGE_SIZE: u32 = 16;

const PAGE_TEXELS: usize = (PAGE_SIZE * PAGE_SIZE) as usize;

/// The grid of masks.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TerrainStencil {
    width: u32,
    height: u32,
    pages_x: u32,
    pages: Vec<Option<Box<[u8; PAGE_TEXELS]>>>,
    allocated: usize,
}

impl TerrainStencil {
    /// The stencil of a field of `field_width` x `field_height` texels: half of each.
    // re: TerrainStencil::StartupGame (App/TerrainStencil.cpp:32-34, `dims = size >> 1`)
    pub fn new(field_width: u32, field_height: u32) -> Self {
        let (width, height) = (field_width >> 1, field_height >> 1);
        let pages_x = width.div_ceil(PAGE_SIZE);
        let pages_y = height.div_ceil(PAGE_SIZE);
        Self {
            width,
            height,
            pages_x,
            pages: vec![None; (pages_x * pages_y) as usize],
            allocated: 0,
        }
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// Pages written so far.
    pub fn pages_allocated(&self) -> usize {
        self.allocated
    }

    fn slot(&self, x: u32, y: u32) -> (usize, usize) {
        let page = ((y / PAGE_SIZE) * self.pages_x + x / PAGE_SIZE) as usize;
        let texel = ((y % PAGE_SIZE) * PAGE_SIZE + x % PAGE_SIZE) as usize;
        (page, texel)
    }

    fn inside(&self, x: i32, y: i32) -> bool {
        x >= 0 && y >= 0 && (x as u32) < self.width && (y as u32) < self.height
    }

    /// The bits of the stencil texel; 0 outside and in a page never written.
    pub fn get(&self, x: i32, y: i32) -> u8 {
        if !self.inside(x, y) {
            return 0;
        }
        let (p, t) = self.slot(x as u32, y as u32);
        self.pages[p].as_ref().map_or(0, |page| page[t])
    }

    /// Adds `bits` to the texel (allocating its page).
    pub fn or_bits(&mut self, x: i32, y: i32, bits: u8) {
        if !self.inside(x, y) || bits == 0 {
            return;
        }
        let (p, t) = self.slot(x as u32, y as u32);
        if self.pages[p].is_none() {
            self.pages[p] = Some(Box::new([0; PAGE_TEXELS]));
            self.allocated += 1;
        }
        if let Some(page) = self.pages[p].as_mut() {
            page[t] |= bits;
        }
    }

    /// Clears `bits` of the texel; a page never written stays unallocated.
    pub fn clear_bits(&mut self, x: i32, y: i32, bits: u8) {
        if !self.inside(x, y) {
            return;
        }
        let (p, t) = self.slot(x as u32, y as u32);
        if let Some(page) = self.pages[p].as_mut() {
            page[t] &= !bits;
        }
    }

    /// Drops every page.
    // re: TerrainStencil::ClearStencil (App/TerrainStencil.cpp:47-64)
    pub fn clear(&mut self) {
        self.pages.iter_mut().for_each(|p| *p = None);
        self.allocated = 0;
    }

    /// Adds `bits` to the stencil texels whose centres lie inside the triangle (the corners in
    /// stencil texels). The winding does not matter.
    // re: TerrainStencil::AddStencil (App/TerrainStencil.cpp:106-134), the rasterization is ours
    pub fn add_triangle(&mut self, tri: [Vec2; 3], bits: u8) {
        let edge =
            |a: Vec2, b: Vec2, p: Vec2| (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x);
        let area = edge(tri[0], tri[1], tri[2]);
        if area == 0.0 {
            return;
        }
        let sign = area.signum();
        let min = tri[0].min(tri[1]).min(tri[2]);
        let max = tri[0].max(tri[1]).max(tri[2]);
        let (x0, x1) = (min.x.floor() as i32, max.x.ceil() as i32);
        let (y0, y1) = (min.y.floor() as i32, max.y.ceil() as i32);
        for y in y0.max(0)..=y1.min(self.height as i32 - 1) {
            for x in x0.max(0)..=x1.min(self.width as i32 - 1) {
                let p = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
                let inside = edge(tri[0], tri[1], p) * sign >= 0.0
                    && edge(tri[1], tri[2], p) * sign >= 0.0
                    && edge(tri[2], tri[0], p) * sign >= 0.0;
                if inside {
                    self.or_bits(x, y, bits);
                }
            }
        }
    }

    /// The bits of the field texel `(u, v)` under `mask`: the stencil texel is `(u >> 1, v >> 1)`.
    /// A texel outside the stencil gives 0.
    // re: TerrainStencil::SampleWorldPosition (App/TerrainStencil.cpp:137-151)
    pub fn sample_texel(&self, u: i32, v: i32, mask: u8) -> u8 {
        if ((u | v) >> 1) < 0 {
            return 0;
        }
        self.get(u >> 1, v >> 1) & mask
    }

    /// The bits under our `(X, Z)` under `mask`.
    // re: TerrainStencil::SampleWorldPosition (App/TerrainStencil.cpp:137-151)
    pub fn sample_world(&self, frame: &TerrainFrame, p: Vec2, mask: u8) -> u8 {
        let t = frame.world_to_tex(p);
        self.sample_texel(t.x as i32, t.y as i32, mask)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stencil_is_half_the_field_in_pages_of_sixteen() {
        let s = TerrainStencil::new(2560, 1536);
        assert_eq!((s.width(), s.height()), (1280, 768));
        assert_eq!(s.pages_allocated(), 0);
        assert_eq!(s.get(5, 5), 0);
    }

    #[test]
    fn a_page_is_allocated_by_the_first_bit() {
        let mut s = TerrainStencil::new(256, 256);
        s.or_bits(20, 3, 0b10);
        assert_eq!(s.pages_allocated(), 1);
        s.or_bits(21, 4, 0b01);
        assert_eq!(s.pages_allocated(), 1);
        s.or_bits(40, 3, 0b01);
        assert_eq!(s.pages_allocated(), 2);
        assert_eq!(s.get(20, 3), 0b10);
        // Bits add up and can be cleared.
        s.or_bits(20, 3, 0b01);
        assert_eq!(s.get(20, 3), 0b11);
        s.clear_bits(20, 3, 0b10);
        assert_eq!(s.get(20, 3), 0b01);
        // Nothing outside, nothing for clearing a page that is not there.
        s.or_bits(-1, 0, 1);
        s.clear_bits(100, 100, 1);
        assert_eq!(s.pages_allocated(), 2);
        s.clear();
        assert_eq!((s.pages_allocated(), s.get(20, 3)), (0, 0));
    }

    #[test]
    fn the_field_texel_reads_the_stencil_texel_of_half_the_index() {
        let mut s = TerrainStencil::new(256, 256);
        s.or_bits(10, 20, 0b101);
        for (u, v) in [(20, 40), (21, 41)] {
            assert_eq!(s.sample_texel(u, v, 0xff), 0b101);
            assert_eq!(s.sample_texel(u, v, 0b100), 0b100);
            assert_eq!(s.sample_texel(u, v, 0b010), 0);
        }
        assert_eq!(s.sample_texel(22, 40, 0xff), 0);
        assert_eq!(s.sample_texel(-3, 40, 0xff), 0);
        assert_eq!(s.sample_texel(40_000, 40, 0xff), 0);
    }

    #[test]
    fn a_world_point_goes_through_the_frame() {
        let frame = TerrainFrame::new(2, 2, false);
        let mut s = TerrainStencil::new(128, 128);
        s.or_bits(30, 12, 1);
        let p = frame.tex_to_world(Vec2::new(61.5, 25.5));
        assert_eq!(s.sample_world(&frame, p, 1), 1);
        let q = frame.tex_to_world(Vec2::new(65.5, 25.5));
        assert_eq!(s.sample_world(&frame, q, 1), 0);
    }

    #[test]
    fn a_triangle_covers_the_texels_under_it_whatever_the_winding() {
        let tri = [
            Vec2::new(2.0, 2.0),
            Vec2::new(10.0, 2.0),
            Vec2::new(2.0, 10.0),
        ];
        let mut a = TerrainStencil::new(64, 64);
        a.add_triangle(tri, 1);
        let mut b = TerrainStencil::new(64, 64);
        b.add_triangle([tri[0], tri[2], tri[1]], 1);
        let mut count = 0;
        for y in 0..32 {
            for x in 0..32 {
                assert_eq!(a.get(x, y), b.get(x, y), "({x}, {y})");
                count += i32::from(a.get(x, y));
            }
        }
        // About half the 8 x 8 square, minus the texels the hypotenuse cuts.
        assert!((28..=36).contains(&count), "{count}");
        assert_eq!(a.get(3, 3), 1);
        assert_eq!(a.get(9, 9), 0);
        // A degenerate triangle writes nothing.
        let mut c = TerrainStencil::new(64, 64);
        c.add_triangle([Vec2::ZERO, Vec2::new(5.0, 5.0), Vec2::new(10.0, 10.0)], 1);
        assert_eq!(c.pages_allocated(), 0);
    }
}
