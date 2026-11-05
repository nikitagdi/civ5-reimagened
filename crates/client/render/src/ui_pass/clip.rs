//! Clip rectangle to hardware scissor.

use draw_list::ClipRect;

/// Scissor `[x, y, w, h]` for `clip` on a target of `size`: the minimum is floored, the maximum is
/// ceiled and the result is cut to the target. `None` when nothing of it is left.
// SIMPLIFIED(SIMP-uir-1): the exe clips in the shader on a fractional border (up to half a pixel
// of difference at the edge); this is a pixel-aligned hardware scissor.
pub(crate) fn scissor(clip: ClipRect, size: (u32, u32)) -> Option<[u32; 4]> {
    let (w, h) = (size.0 as f32, size.1 as f32);
    let x0 = clip.min.x.floor().clamp(0.0, w);
    let y0 = clip.min.y.floor().clamp(0.0, h);
    let x1 = clip.max.x.ceil().clamp(0.0, w);
    let y1 = clip.max.y.ceil().clamp(0.0, h);
    if x1 <= x0 || y1 <= y0 || x0.is_nan() || y0.is_nan() || x1.is_nan() || y1.is_nan() {
        return None;
    }
    Some([x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32])
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec2;

    fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> ClipRect {
        ClipRect {
            min: Vec2::new(x0, y0),
            max: Vec2::new(x1, y1),
        }
    }

    #[test]
    fn rounds_outwards() {
        assert_eq!(
            scissor(rect(10.2, 5.7, 20.1, 9.01), (100, 100)),
            Some([10, 5, 11, 5])
        );
    }

    #[test]
    fn no_clip_is_the_whole_target() {
        assert_eq!(
            scissor(ClipRect::NONE, (1280, 720)),
            Some([0, 0, 1280, 720])
        );
    }

    #[test]
    fn is_cut_to_the_target() {
        assert_eq!(
            scissor(rect(-5.0, -5.0, 150.0, 40.5), (100, 50)),
            Some([0, 0, 100, 41])
        );
    }

    #[test]
    fn outside_or_empty_is_none() {
        assert_eq!(scissor(rect(120.0, 0.0, 200.0, 10.0), (100, 100)), None);
        assert_eq!(scissor(rect(-20.0, 0.0, -1.0, 10.0), (100, 100)), None);
        assert_eq!(scissor(rect(10.0, 10.0, 10.0, 20.0), (100, 100)), None);
        assert_eq!(
            scissor(rect(10.3, 10.0, 10.6, 20.0), (100, 100)),
            Some([10, 10, 1, 10])
        );
    }
}
