// UI v2 pass: one instanced quad per primitive, coverage from distance functions, analytic clips.
//
// The instance layout is `ui2_draw::instance` (96 bytes, 12 attributes); that file is the source of truth for
// the offsets, the `KIND_*`, `FILL_*` and `FLAG_*` values and the packing of the three meta words (`words`).
// Positions are device pixels, y down. Colours are straight sRGB; the output is premultiplied and blended in
// the space of the target (gamma space on a plain target, like the browser).
//
// Group 0: frame constants, the transform and clip table (`Rgba32Float`, read with `textureLoad` only, so it
// needs no filtering), the coverage array (R8: package mask pages, then glyph pages), the sampler of both
// arrays, and the image array (RGBA8, premultiplied: the package's colour pages).
//
// The varyings stay within 12 locations (WebGL2 allows 15): the four fill colours travel packed in one
// `vec4<u32>`, the ellipse ratio and the clip radius as bits of the primitive word vector.

struct Frame {
    // (2 / width, 2 / height): pixels to NDC.
    to_ndc: vec2<f32>,
    // 1.0 when the target is sRGB: the output is linearised so that it lands unchanged.
    target_srgb: f32,
    pad: f32,
}

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var tables: texture_2d<f32>;
@group(0) @binding(2) var mask: texture_2d_array<f32>;
@group(0) @binding(3) var mask_smp: sampler;
@group(0) @binding(4) var image: texture_2d_array<f32>;

// Group 1: the external texture of the draw (a game asset or the minimap; straight alpha) and the scale that maps
// a picture's 0..1 onto the stored texture. Draws without one bind a placeholder they never sample.
struct ExtParams {
    uv_scale: vec2<f32>,
    pad: vec2<f32>,
}

@group(1) @binding(0) var ext_tex: texture_2d<f32>;
@group(1) @binding(1) var<uniform> ext: ExtParams;

const KIND_BOX: u32 = 0u;
const KIND_SHADOW: u32 = 1u;
const KIND_IMAGE: u32 = 2u;
const KIND_MASK: u32 = 3u;

const FILL_SOLID: u32 = 0u;
const FILL_LINEAR: u32 = 1u;
const FILL_REPEAT: u32 = 2u;
const FILL_CONIC: u32 = 3u;
const FILL_MODE_MASK: u32 = 3u;
const FLAG_FILL: u32 = 4u;
const FLAG_BORDER: u32 = 8u;
const FLAG_ELLIPSE: u32 = 16u;
const FLAG_TILE: u32 = 32u;
const FLAG_EXT: u32 = 64u;

// Table layout (`ui2_draw::tables`).
const RECORDS_PER_ROW: u32 = 512u;
const CLIP_FIRST_ROW: u32 = 16u;
const CLIPS_PER_ROW: u32 = 128u;

// Distance that never clips.
const FAR: f32 = 1.0e6;
// Anti-aliasing margin around a box, local px.
const AA_MARGIN: f32 = 1.0;
const TAU: f32 = 6.283185307179586;

struct Instance {
    @location(0) rect: vec4<f32>,
    // tl, tr, br, bl
    @location(1) radii: vec4<f32>,
    // top, right, bottom, left
    @location(2) border: vec4<f32>,
    // sprite rectangle (u0, v0, u1, v1), gradient geometry, shadow offset
    @location(3) uv: vec4<f32>,
    // fill colour, gradient stop colours, tint
    @location(4) c0: vec4<f32>,
    @location(5) c1: vec4<f32>,
    @location(6) c2: vec4<f32>,
    @location(7) c3: vec4<f32>,
    @location(8) border_color: vec4<f32>,
    // Gradient stop positions.
    @location(9) stops: vec4<f32>,
    // Shadow sigma and spread, or the tile size; ellipse ratio; unused.
    @location(10) extra: vec4<f32>,
    // kind:8 flags:8 atlas_layer:8 n_stops:8 | xform:16 clip:16 | anchor:16 ext:16
    @location(11) words: vec3<u32>,
}

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    // xy: position relative to the primitive's rectangle, local px (0,0 = its top left corner); zw: sprite uv.
    @location(0) local_uv: vec4<f32>,
    // Rectangle size; tile size of a tiled gradient.
    @location(1) @interpolate(flat) size: vec4<f32>,
    @location(2) @interpolate(flat) radii: vec4<f32>,
    @location(3) @interpolate(flat) border: vec4<f32>,
    // c0..c3 as RGBA8 words.
    @location(4) @interpolate(flat) colors: vec4<u32>,
    @location(5) @interpolate(flat) border_color: vec4<f32>,
    // The first meta word (kind, flags, layer, stops), the ellipse ratio's bits, the clip radius's bits, 0.
    @location(6) @interpolate(flat) prim: vec4<u32>,
    // Distances to the clip rectangle's left, top, right, bottom edges (positive inside), chunk px.
    @location(7) clip_rect: vec4<f32>,
    // Distances to the clip polygon's faces 0-3 and 4-7 (positive inside; FAR for missing faces).
    @location(8) clip_faces0: vec4<f32>,
    @location(9) clip_faces1: vec4<f32>,
    // Gradient geometry (`uv` of the instance), or the shadow's offset, sigma and spread.
    @location(10) @interpolate(flat) grad: vec4<f32>,
    @location(11) @interpolate(flat) stops: vec4<f32>,
}

struct Xform {
    m: mat2x2<f32>,
    t: vec2<f32>,
    visible: f32,
}

// Transform record `r`: two texels, `(a, b, c, d)` and `(tx, ty, visible, 0)`.
fn record(r: u32) -> Xform {
    let row = i32(r / RECORDS_PER_ROW);
    let x = i32(2u * (r % RECORDS_PER_ROW));
    let t0 = textureLoad(tables, vec2<i32>(x, row), 0);
    let t1 = textureLoad(tables, vec2<i32>(x + 1, row), 0);
    return Xform(mat2x2<f32>(t0.xy, t0.zw), t1.xy, t1.z);
}

fn identity() -> Xform {
    return Xform(mat2x2<f32>(vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0)), vec2<f32>(0.0), 1.0);
}

fn clip_texel(k: u32, i: i32) -> vec4<f32> {
    let row = i32(CLIP_FIRST_ROW + k / CLIPS_PER_ROW);
    let x = i32(8u * (k % CLIPS_PER_ROW));
    return textureLoad(tables, vec2<i32>(x + i, row), 0);
}

fn face(f: vec3<f32>, p: vec2<f32>, n: u32, i: u32) -> f32 {
    return select(FAR, dot(f.xy, p) + f.z, i < n);
}

// Two triangles: (0,0) (1,0) (0,1), (0,1) (1,0) (1,1).
fn quad_corner(vi: u32) -> vec2<f32> {
    let i = vi % 6u;
    let x = select(0.0, 1.0, i == 1u || i == 4u || i == 5u);
    let y = select(0.0, 1.0, i == 2u || i == 3u || i == 5u);
    return vec2<f32>(x, y);
}

fn pack_color(c: vec4<f32>) -> u32 {
    let b = vec4<u32>(round(clamp(c, vec4<f32>(0.0), vec4<f32>(1.0)) * 255.0));
    return b.x | (b.y << 8u) | (b.z << 16u) | (b.w << 24u);
}

fn unpack_color(w: u32) -> vec4<f32> {
    return vec4<f32>(
        f32(w & 255u),
        f32((w >> 8u) & 255u),
        f32((w >> 16u) & 255u),
        f32(w >> 24u),
    ) / 255.0;
}

@vertex
fn vs(@builtin(vertex_index) vi: u32, inst: Instance) -> VsOut {
    let kind = inst.words.x & 0xffu;
    let flags = (inst.words.x >> 8u) & 0xffu;
    let xform_id = inst.words.y & 0xffffu;
    let clip_id = inst.words.y >> 16u;
    let anchor_id = inst.words.z & 0xffffu;

    let corner = quad_corner(vi);
    // The quad, relative to the rectangle: boxes get a margin for the soft edge, shadows their offset, spread
    // and three standard deviations of blur; masks and pictures are their texels exactly.
    var lo = vec2<f32>(0.0);
    var hi = inst.rect.zw;
    if kind == KIND_BOX {
        lo = vec2<f32>(-AA_MARGIN);
        hi = inst.rect.zw + vec2<f32>(AA_MARGIN);
    } else if kind == KIND_SHADOW {
        let grow = inst.extra.y + 3.0 * inst.extra.x + AA_MARGIN;
        lo = inst.uv.xy - vec2<f32>(grow);
        hi = inst.rect.zw + inst.uv.xy + vec2<f32>(grow);
    }
    let local = mix(lo, hi, corner);
    let p = inst.rect.xy + local;

    var element = identity();
    if xform_id != 0u {
        element = record(xform_id);
    }
    let chunk = element.m * p + element.t;
    var anchor = identity();
    if anchor_id != 0u {
        anchor = record(anchor_id);
    }
    let screen = anchor.m * chunk + anchor.t;

    var o: VsOut;
    o.pos = vec4<f32>(screen.x * frame.to_ndc.x - 1.0, 1.0 - screen.y * frame.to_ndc.y, 0.0, 1.0);
    if anchor.visible < 0.5 {
        // A hidden anchor: zero area, off screen.
        o.pos = vec4<f32>(-2.0, -2.0, 0.0, 1.0);
    }
    o.local_uv = vec4<f32>(local, mix(inst.uv.xy, inst.uv.zw, corner));
    o.size = vec4<f32>(inst.rect.zw, inst.extra.xy);
    o.radii = inst.radii;
    o.border = inst.border;
    o.colors = vec4<u32>(pack_color(inst.c0), pack_color(inst.c1), pack_color(inst.c2), pack_color(inst.c3));
    o.border_color = inst.border_color;
    let ratio = select(1.0, inst.extra.z, (flags & FLAG_ELLIPSE) != 0u && inst.extra.z > 0.0);
    o.grad = inst.uv;
    if kind == KIND_SHADOW {
        o.grad = vec4<f32>(inst.uv.xy, inst.extra.xy);
    }
    o.stops = inst.stops;

    o.clip_rect = vec4<f32>(FAR);
    o.clip_faces0 = vec4<f32>(FAR);
    o.clip_faces1 = vec4<f32>(FAR);
    var clip_radius = 0.0;
    if clip_id != 0u {
        let r = clip_texel(clip_id, 0);
        let head = clip_texel(clip_id, 1);
        o.clip_rect = vec4<f32>(chunk - r.xy, r.xy + r.zw - chunk);
        clip_radius = head.x;
        let n = u32(head.y);
        if n > 0u {
            // Face i is floats 3i..3i+3 of texels 2-7.
            let t2 = clip_texel(clip_id, 2);
            let t3 = clip_texel(clip_id, 3);
            let t4 = clip_texel(clip_id, 4);
            let t5 = clip_texel(clip_id, 5);
            let t6 = clip_texel(clip_id, 6);
            let t7 = clip_texel(clip_id, 7);
            o.clip_faces0 = vec4<f32>(
                face(t2.xyz, chunk, n, 0u),
                face(vec3<f32>(t2.w, t3.xy), chunk, n, 1u),
                face(vec3<f32>(t3.zw, t4.x), chunk, n, 2u),
                face(t4.yzw, chunk, n, 3u),
            );
            o.clip_faces1 = vec4<f32>(
                face(t5.xyz, chunk, n, 4u),
                face(vec3<f32>(t5.w, t6.xy), chunk, n, 5u),
                face(vec3<f32>(t6.zw, t7.x), chunk, n, 6u),
                face(t7.yzw, chunk, n, 7u),
            );
        }
    }
    o.prim = vec4<u32>(inst.words.x, bitcast<u32>(ratio), bitcast<u32>(clip_radius), 0u);
    return o;
}

// Coverage of a signed distance (negative inside) for a pixel of `px` units.
fn inside(d: f32, px: f32) -> f32 {
    return clamp(0.5 - d / px, 0.0, 1.0);
}

// Radius of the corner of the quadrant of `p` (relative to the centre, y down): tl, tr, br, bl.
fn corner_radius(p: vec2<f32>, r: vec4<f32>) -> f32 {
    let top = select(r.x, r.y, p.x > 0.0);
    let bottom = select(r.w, r.z, p.x > 0.0);
    return select(top, bottom, p.y > 0.0);
}

// Signed distance to a rectangle of half size `half` with per-corner radii, `p` relative to its centre. The
// radii are horizontal; the vertical ones are `ratio` times larger (1: circles). An elliptic corner is a
// circle in a space squeezed by `ratio` vertically; the distance there is divided by the length of its
// gradient in the real space (first-order distance to the ellipse).
fn sd_box(p: vec2<f32>, half: vec2<f32>, radii: vec4<f32>, ratio: f32) -> f32 {
    let ps = vec2<f32>(p.x, p.y / ratio);
    let hs = vec2<f32>(half.x, half.y / ratio);
    let r = corner_radius(p, radii);
    let q = abs(ps) - hs + vec2<f32>(r);
    if q.x > 0.0 && q.y > 0.0 {
        let l = length(q);
        let n = q / l;
        return (l - r) / length(vec2<f32>(n.x, n.y / ratio));
    }
    if q.x > q.y {
        return q.x - r;
    }
    return (q.y - r) * ratio;
}

fn premul(c: vec4<f32>) -> vec4<f32> {
    return vec4<f32>(c.rgb * c.a, c.a);
}

// The premultiplied colour of gradient position `t`: `n` stops at positions `s` (in order) with the straight
// colours `c`, interpolated premultiplied (as the browser does); before the first and after the last stop the
// end colours. Equal positions make a hard stop.
fn stop_color(t: f32, n: u32, s: vec4<f32>, c: vec4<u32>) -> vec4<f32> {
    let p0 = premul(unpack_color(c.x));
    let p1 = premul(unpack_color(c.y));
    if t <= s.x {
        return p0;
    }
    if t <= s.y {
        return mix(p0, p1, (t - s.x) / max(s.y - s.x, 1.0e-6));
    }
    if n <= 2u {
        return p1;
    }
    let p2 = premul(unpack_color(c.z));
    if t <= s.z {
        return mix(p1, p2, (t - s.y) / max(s.z - s.y, 1.0e-6));
    }
    if n <= 3u {
        return p2;
    }
    let p3 = premul(unpack_color(c.w));
    if t <= s.w {
        return mix(p2, p3, (t - s.z) / max(s.w - s.z, 1.0e-6));
    }
    return p3;
}

// The premultiplied fill at this fragment: the solid colour `c0`, or the gradient of the fill mode.
fn fill_color(in: VsOut) -> vec4<f32> {
    let flags = (in.prim.x >> 8u) & 0xffu;
    let mode = flags & FILL_MODE_MASK;
    if mode == FILL_SOLID {
        return premul(unpack_color(in.colors.x));
    }
    var q = in.local_uv.xy;
    let tile = in.size.zw;
    if (flags & FLAG_TILE) != 0u && tile.x > 0.0 && tile.y > 0.0 {
        q = q - tile * floor(q / tile);
    }
    var t = 0.0;
    if mode == FILL_CONIC {
        // Clockwise from the top, y down: atan2(x, -y).
        let v = q - in.grad.xy;
        if dot(v, v) > 1.0e-12 {
            let turns = fract(atan2(v.x, -v.y) / TAU - in.grad.z);
            t = fract(turns / max(in.grad.w, 1.0e-6));
        }
    } else {
        let d = in.grad.zw - in.grad.xy;
        t = dot(q - in.grad.xy, d) / max(dot(d, d), 1.0e-12);
        if mode == FILL_REPEAT {
            t = fract(t);
        }
    }
    let n = in.prim.x >> 24u;
    return stop_color(t, n, in.stops, in.colors);
}

// BOX: fill under a border, per-corner radii (or one ellipse ratio), per-side widths (CSS
// `background-clip: border-box`).
fn shade_box(in: VsOut, px: f32) -> vec4<f32> {
    let flags = (in.prim.x >> 8u) & 0xffu;
    let ratio = bitcast<f32>(in.prim.y);
    let size = in.size.xy;
    let local = in.local_uv.xy;
    let half = size * 0.5;
    let outer = inside(sd_box(local - half, half, in.radii, ratio), px);
    var fill = vec4<f32>(0.0);
    if (flags & FLAG_FILL) != 0u {
        fill = fill_color(in) * outer;
    }
    if (flags & FLAG_BORDER) == 0u {
        return fill;
    }
    let b = in.border;
    let inner_min = vec2<f32>(b.w, b.x);
    let inner_size = size - vec2<f32>(b.y + b.w, b.x + b.z);
    var inner = 0.0;
    if inner_size.x > 0.0 && inner_size.y > 0.0 {
        let ihalf = inner_size * 0.5;
        // SIMPLIFIED(SIMP-248): the inner corner of a border is the outer one shrunk by the mean of the adjacent widths (one ellipse ratio for the whole box), not the CSS ellipse of each corner when the widths differ
        let inset = 0.5 * vec4<f32>(b.w + b.x, b.y + b.x, b.y + b.z, b.w + b.z);
        var iradii = max(in.radii - inset, vec4<f32>(0.0));
        var iratio = 1.0;
        if ratio != 1.0 {
            // Horizontal radii shrink by the side widths, vertical ones by the top and bottom widths.
            let bx = 0.5 * (b.y + b.w);
            let by = 0.5 * (b.x + b.z);
            let rmax = max(max(in.radii.x, in.radii.y), max(in.radii.z, in.radii.w));
            iradii = max(in.radii - vec4<f32>(bx), vec4<f32>(0.0));
            iratio = max(rmax * ratio - by, 0.0) / max(rmax - bx, 1.0e-3);
            iratio = clamp(iratio, 1.0e-3, 1.0e3);
        }
        inner = inside(sd_box(local - inner_min - ihalf, ihalf, iradii, iratio), px);
    }
    let ring = clamp(outer - inner, 0.0, 1.0);
    let border = premul(in.border_color) * ring;
    return border + fill * (1.0 - border.a);
}

// The error function, for a pair of values (Abramowitz and Stegun 7.1.27, error below 5e-4).
fn erf2(v: vec2<f32>) -> vec2<f32> {
    let s = sign(v);
    let a = abs(v);
    var r = 1.0 + (0.278393 + (0.230389 + 0.078108 * (a * a)) * a) * a;
    r = r * r;
    return s - s / (r * r);
}

fn gaussian(x: f32, sigma: f32) -> f32 {
    return exp(-(x * x) / (2.0 * sigma * sigma)) / (2.5066282746 * sigma);
}

// The blur along x of the row `y` of a rounded rectangle (`p` relative to its centre).
fn blur_along_x(x: f32, y: f32, sigma: f32, corner: f32, half: vec2<f32>) -> f32 {
    let delta = min(half.y - corner - abs(y), 0.0);
    let curved = half.x - corner + sqrt(max(0.0, corner * corner - delta * delta));
    let integral = 0.5 + 0.5 * erf2((x + vec2<f32>(-curved, curved)) * (0.70710678 / sigma));
    return integral.y - integral.x;
}

// Coverage of a rounded rectangle convolved with a Gaussian: exact along x (erf), four samples along y.
fn blurred_box(p: vec2<f32>, half: vec2<f32>, corner: f32, sigma: f32) -> f32 {
    let low = p.y - half.y;
    let high = p.y + half.y;
    let start = clamp(-3.0 * sigma, low, high);
    let end = clamp(3.0 * sigma, low, high);
    let step = (end - start) / 4.0;
    var y = start + step * 0.5;
    var a = 0.0;
    for (var i = 0; i < 4; i++) {
        a += blur_along_x(p.x, p.y - y, sigma, corner, half) * gaussian(y, sigma) * step;
        y += step;
    }
    return a;
}

// SHADOW: the element box moved by the offset, grown by the spread, blurred; cut out of the element's own box.
fn shade_shadow(in: VsOut, px: f32) -> vec4<f32> {
    let size = in.size.xy;
    let local = in.local_uv.xy;
    let half = size * 0.5;
    let offset = in.grad.xy;
    let sigma = in.grad.z;
    let spread = in.grad.w;
    let shalf = max(half + vec2<f32>(spread), vec2<f32>(0.0));
    // CSS: a rounded corner grows with the spread, a square one stays square.
    let sradii = select(vec4<f32>(0.0), max(in.radii + vec4<f32>(spread), vec4<f32>(0.0)), in.radii > vec4<f32>(0.0));
    let p = local - half - offset;
    var a = 0.0;
    if sigma < 0.25 * px {
        a = inside(sd_box(p, shalf, sradii, 1.0), px);
    } else {
        let corner = min(corner_radius(p, sradii), min(shalf.x, shalf.y));
        a = blurred_box(p, shalf, corner, sigma);
    }
    let own = inside(sd_box(local - half, half, in.radii, 1.0), px);
    return premul(unpack_color(in.colors.x)) * (a * (1.0 - own));
}

// MASK: coverage of a glyph or an icon times the colour. Glyph coverage already carries the rasterizer's
// gamma, so it is used as it is.
fn shade_mask(in: VsOut) -> vec4<f32> {
    let layer = i32((in.prim.x >> 16u) & 0xffu);
    let cov = textureSampleLevel(mask, mask_smp, in.local_uv.zw, layer, 0.0).r;
    return premul(unpack_color(in.colors.x)) * cov;
}

// IMAGE: a premultiplied texel of a colour page, or a straight one of the external texture, times the tint
// (straight colour, its alpha an opacity), cut to the rounded rectangle of the instance.
fn shade_image(in: VsOut, px: f32) -> vec4<f32> {
    let flags = (in.prim.x >> 8u) & 0xffu;
    let layer = i32((in.prim.x >> 16u) & 0xffu);
    var s = vec4<f32>(0.0);
    if (flags & FLAG_EXT) != 0u {
        s = premul(textureSampleLevel(ext_tex, mask_smp, in.local_uv.zw * ext.uv_scale, 0.0));
    } else {
        s = textureSampleLevel(image, mask_smp, in.local_uv.zw, layer, 0.0);
    }
    let tint = unpack_color(in.colors.x);
    let half = in.size.xy * 0.5;
    let cover = inside(sd_box(in.local_uv.xy - half, half, in.radii, 1.0), px);
    return vec4<f32>(s.rgb * tint.rgb, s.a) * (tint.a * cover);
}

// Coverage of the clip: rounded rectangle and convex polygon, anti-aliased over one pixel of chunk space.
fn clip_coverage(in: VsOut, w_rect: vec4<f32>, w0: vec4<f32>, w1: vec4<f32>) -> f32 {
    let e = in.clip_rect;
    let ex = min(e.x, e.z);
    let ey = min(e.y, e.w);
    let r = bitcast<f32>(in.prim.z);
    let q = vec2<f32>(r - ex, r - ey);
    let d = select(min(ex, ey), r - length(q), r > 0.0 && q.x > 0.0 && q.y > 0.0);
    let wr = max(max(w_rect.x, w_rect.y), max(w_rect.z, w_rect.w));
    let rect = clamp(d / max(wr, 1.0e-6) + 0.5, 0.0, 1.0);
    let f0 = clamp(in.clip_faces0 / max(w0, vec4<f32>(1.0e-6)) + 0.5, vec4<f32>(0.0), vec4<f32>(1.0));
    let f1 = clamp(in.clip_faces1 / max(w1, vec4<f32>(1.0e-6)) + 0.5, vec4<f32>(0.0), vec4<f32>(1.0));
    let faces = min(min(min(f0.x, f0.y), min(f0.z, f0.w)), min(min(f1.x, f1.y), min(f1.z, f1.w)));
    return rect * faces;
}

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

// SIMPLIFIED(SIMP-249): the browser blends UI in gamma space; on an sRGB-only surface the colour is linearised here and blends in linear space, so translucent edges differ a little
fn finish(c: vec4<f32>) -> vec4<f32> {
    if frame.target_srgb > 0.5 && c.a > 0.0 {
        return vec4<f32>(srgb_to_linear(c.rgb / c.a) * c.a, c.a);
    }
    return c;
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    // Derivatives first, in uniform control flow.
    let dx = dpdx(in.local_uv.xy);
    let dy = dpdy(in.local_uv.xy);
    // Local px per screen pixel (isotropic estimate; exact for translation, rotation and uniform scale).
    let px = max(sqrt(0.5 * (dot(dx, dx) + dot(dy, dy))), 1.0e-6);
    let w_rect = fwidth(in.clip_rect);
    let w0 = fwidth(in.clip_faces0);
    let w1 = fwidth(in.clip_faces1);

    var c = vec4<f32>(0.0);
    switch in.prim.x & 0xffu {
        case KIND_BOX: {
            c = shade_box(in, px);
        }
        case KIND_SHADOW: {
            c = shade_shadow(in, px);
        }
        case KIND_MASK: {
            c = shade_mask(in);
        }
        case KIND_IMAGE: {
            c = shade_image(in, px);
        }
        default: {}
    }
    return finish(c * clip_coverage(in, w_rect, w0, w1));
}
