//! Atlas pages: rasterized pictures packed on shelves into pages of at most 1024 × 1024.
//!
//! Pages are grouped by format and scale, in this order: mask pages (`A8`, the alpha of the picture) at scale
//! 1 and 2, then color pages (`Rgba8`, premultiplied alpha) at scale 1 and 2; the runtime finds a mask's texture
//! layer by counting the `A8` pages before it. Inside a group the pictures are placed in the order of their
//! names (each name is one picture), left to right on shelves as tall as their tallest picture; a picture that
//! does not fit the shelf starts a new one, one that does not fit the page starts a new page. Every sprite has a
//! 1 px transparent margin (its rectangle includes it). Pictures with the same pixels share one sprite. A page
//! is as large as what it holds. The result is a pure function of the pictures.

use std::collections::BTreeMap;

use ui2_package::sections::blobs::{BlobRef, BlobSection};
use ui2_package::{Page, PageFormat, Sprite};

use crate::assets::Raster;
use crate::errors::{ConvError, Place, codes};
use crate::write::{Picture, Pictures};

/// The largest page side.
pub const PAGE_MAX: u32 = 1024;

struct Placed {
    page: u16,
    rect: [u16; 4],
}

struct Group {
    format: PageFormat,
    scale: u8,
    /// Finished pages: size and pixels.
    pages: Vec<([u32; 2], Vec<u8>)>,
}

/// Bytes per pixel of a format.
fn bpp(f: PageFormat) -> usize {
    match f {
        PageFormat::A8 => 1,
        PageFormat::Rgba8 => 4,
    }
}

/// Straight RGBA to the page format: the alpha of a mask, premultiplied color otherwise.
fn convert(rgba: &[u8], format: PageFormat) -> Vec<u8> {
    match format {
        PageFormat::A8 => rgba.chunks_exact(4).map(|p| p[3]).collect(),
        PageFormat::Rgba8 => rgba
            .chunks_exact(4)
            .flat_map(|p| {
                let a = u32::from(p[3]);
                let m = |c: u8| ((u32::from(c) * a + 127) / 255) as u8;
                [m(p[0]), m(p[1]), m(p[2]), p[3]]
            })
            .collect(),
    }
}

/// Per item: page, x, y; and the page sizes.
type Shelves = (Vec<(usize, u32, u32)>, Vec<[u32; 2]>);

/// Shelf packing of one group: `items` are (w, h) with the margin. Returns per item (page, x, y) and the page
/// sizes.
fn shelves(items: &[(u32, u32)]) -> Result<Shelves, String> {
    let mut out = Vec::new();
    let mut pages: Vec<[u32; 2]> = Vec::new();
    let (mut x, mut y, mut shelf_h) = (0u32, 0u32, 0u32);
    for &(w, h) in items {
        if w > PAGE_MAX || h > PAGE_MAX {
            return Err(format!(
                "a picture of {w} x {h} device pixels does not fit a page"
            ));
        }
        if pages.is_empty() {
            pages.push([0, 0]);
        }
        if x + w > PAGE_MAX {
            y += shelf_h;
            x = 0;
            shelf_h = 0;
        }
        if y + h > PAGE_MAX {
            pages.push([0, 0]);
            x = 0;
            y = 0;
            shelf_h = 0;
        }
        let p = pages.len() - 1;
        out.push((p, x, y));
        x += w;
        shelf_h = shelf_h.max(h);
        pages[p] = [pages[p][0].max(x), pages[p][1].max(y + h)];
    }
    Ok((out, pages))
}

/// Packs the pictures into pages.
pub fn pack(rasters: &[Raster]) -> Result<Pictures, ConvError> {
    let mut sorted: Vec<&Raster> = rasters.iter().collect();
    sorted.sort_by(|a, b| a.path.cmp(&b.path));
    let mut groups: Vec<Group> = Vec::new();
    // Per picture, per scale: the placement.
    let mut placed: Vec<[Option<Placed>; 2]> = (0..sorted.len()).map(|_| [None, None]).collect();
    let mut page_base = 0u16;
    for format in [PageFormat::A8, PageFormat::Rgba8] {
        for (si, scale) in [1u8, 2].into_iter().enumerate() {
            let members: Vec<usize> = (0..sorted.len())
                .filter(|&i| sorted[i].mask == (format == PageFormat::A8))
                .collect();
            if members.is_empty() {
                continue;
            }
            // Distinct pixels in name order.
            let mut distinct: Vec<(u32, u32, Vec<u8>)> = Vec::new();
            let mut by_pixels: BTreeMap<Vec<u8>, usize> = BTreeMap::new();
            let mut slot_of = Vec::new();
            for &i in &members {
                let (w, h, rgba) = &sorted[i].scales[si];
                let px = convert(rgba, format);
                let mut key = px.clone();
                key.extend_from_slice(&w.to_le_bytes());
                let at = *by_pixels.entry(key).or_insert_with(|| {
                    distinct.push((*w, *h, px));
                    distinct.len() - 1
                });
                slot_of.push(at);
            }
            let sizes: Vec<(u32, u32)> = distinct.iter().map(|(w, h, _)| (w + 2, h + 2)).collect();
            let (spots, page_sizes) =
                shelves(&sizes).map_err(|e| ConvError::new(codes::IMAGE, Place::default(), e))?;
            let n = bpp(format);
            let mut pages: Vec<([u32; 2], Vec<u8>)> = page_sizes
                .iter()
                .map(|s| (*s, vec![0u8; (s[0] * s[1]) as usize * n]))
                .collect();
            for (d, &(p, x, y)) in distinct.iter().zip(&spots) {
                let (w, h, px) = d;
                let (pw, buf) = (pages[p].0[0], &mut pages[p].1);
                for row in 0..*h {
                    let src = (row * w) as usize * n;
                    let dst = (((y + 1 + row) * pw) + x + 1) as usize * n;
                    buf[dst..dst + *w as usize * n]
                        .copy_from_slice(&px[src..src + *w as usize * n]);
                }
            }
            for (k, &i) in members.iter().enumerate() {
                let (p, x, y) = spots[slot_of[k]];
                let (w, h, _) = &distinct[slot_of[k]];
                placed[i][si] = Some(Placed {
                    page: page_base + p as u16,
                    rect: [x as u16, y as u16, (w + 2) as u16, (h + 2) as u16],
                });
            }
            page_base += pages.len() as u16;
            groups.push(Group {
                format,
                scale,
                pages,
            });
        }
    }
    let mut out = Pictures::default();
    for g in groups {
        for (size, px) in g.pages {
            out.pages.push((
                Page {
                    format: g.format,
                    scale: g.scale,
                    size: [size[0] as u16, size[1] as u16],
                    blob: BlobRef {
                        section: BlobSection::Imgs,
                        offset: 0,
                        len: 0,
                        raw_len: px.len() as u32,
                        zstd: true,
                    },
                    blake3: *blake3::hash(&px).as_bytes(),
                },
                px,
            ));
        }
    }
    for (r, pl) in sorted.iter().zip(placed) {
        let [Some(a), b] = pl else {
            continue;
        };
        out.imgs.push(Picture {
            path: r.path.clone(),
            size: r.size,
            mask: r.mask,
            x1: Sprite {
                page: a.page,
                rect: a.rect,
            },
            x2: b.map(|b| Sprite {
                page: b.page,
                rect: b.rect,
            }),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raster(path: &str, w: u32, mask: bool, v: u8) -> Raster {
        let px = |w: u32| vec![v; (w * w * 4) as usize];
        Raster {
            path: path.into(),
            size: [w as f32, w as f32],
            mask,
            scales: [(w, w, px(w)), (w * 2, w * 2, px(w * 2))],
        }
    }

    #[test]
    fn atlas_groups_masks_first_and_keeps_margins() {
        let p = pack(&[
            raster("b", 16, true, 200),
            raster("a", 18, true, 100),
            raster("c", 10, false, 255),
        ])
        .expect("pack");
        let formats: Vec<(PageFormat, u8)> = p
            .pages
            .iter()
            .map(|(pg, _)| (pg.format, pg.scale))
            .collect();
        assert_eq!(
            formats,
            [
                (PageFormat::A8, 1),
                (PageFormat::A8, 2),
                (PageFormat::Rgba8, 1),
                (PageFormat::Rgba8, 2)
            ]
        );
        // Name order on the shelf, 1 px margins in the rectangles.
        assert_eq!(p.imgs[0].path, "a");
        assert_eq!(p.imgs[0].x1.rect, [0, 0, 20, 20]);
        assert_eq!(p.imgs[1].x1.rect, [20, 0, 18, 18]);
        assert_eq!(p.imgs[1].x2.map(|s| s.page), Some(1));
        assert_eq!(p.pages[0].0.size, [38, 20]);
        // The margin stays transparent, the inside is the alpha.
        let a8 = &p.pages[0].1;
        assert_eq!(a8[0], 0);
        assert_eq!(a8[38 + 1], 100);
        // Color pages are premultiplied.
        assert_eq!(
            &p.pages[2].1[(12 + 1) * 4..(12 + 1) * 4 + 4],
            &[255, 255, 255, 255]
        );
    }

    #[test]
    fn atlas_shares_equal_pixels_and_wraps_shelves() {
        let mut list = vec![raster("x", 16, true, 7), raster("y", 16, true, 7)];
        for i in 0..80 {
            list.push(raster(&format!("z{i:02}"), 100, true, i as u8 + 1));
        }
        let p = pack(&list).expect("pack");
        let x = p.imgs.iter().find(|i| i.path == "x").expect("x");
        let y = p.imgs.iter().find(|i| i.path == "y").expect("y");
        assert_eq!(x.x1, y.x1);
        assert!(
            p.pages
                .iter()
                .all(|(pg, _)| pg.size[0] <= 1024 && pg.size[1] <= 1024)
        );
        assert!(p.pages.iter().filter(|(pg, _)| pg.scale == 2).count() > 1);
        assert_eq!(pack(&list).expect("again").pages, p.pages);
    }
}
