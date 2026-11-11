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

// `KIND_SHADOW` (1) is not drawn yet.
const KIND_BOX: u32 = 0u;
const KIND_IMAGE: u32 = 2u;
const KIND_MASK: u32 = 3u;

const FLAG_FILL: u32 = 4u;
const FLAG_BORDER: u32 = 8u;

// Table layout (`ui2_draw::tables`).
const RECORDS_PER_ROW: u32 = 512u;
const CLIP_FIRST_ROW: u32 = 16u;
const CLIPS_PER_ROW: u32 = 128u;

// Distance that never clips.
const FAR: f32 = 1.0e6;
// Anti-aliasing margin around a box, local px.
const AA_MARGIN: f32 = 1.0;

struct Instance {
    @location(0) rect: vec4<f32>,
    // tl, tr, br, bl
    @location(1) radii: vec4<f32>,
    // top, right, bottom, left
    @location(2) border: vec4<f32>,
    // sprite rectangle (u0, v0, u1, v1) or gradient points
    @location(3) uv: vec4<f32>,
    @location(4) c0: vec4<f32>,
    // Stop colours and tint: read by the gradient and image modes.
    @location(5) c1: vec4<f32>,
    @location(6) c2: vec4<f32>,
    @location(7) c3: vec4<f32>,
    @location(8) border_color: vec4<f32>,
    // Gradient stop positions; shadow sigma, spread, ellipse ratio, repeat period.
    @location(9) stops: vec4<f32>,
    @location(10) extra: vec4<f32>,
    // kind:8 flags:8 atlas_layer:8 n_stops:8 | xform:16 clip:16 | anchor:16 ext:16
    @location(11) words: vec3<u32>,
}

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    // Position in the primitive's rectangle, local px (0,0 = its top left corner).
    @location(0) local: vec2<f32>,
    @location(1) @interpolate(flat) size: vec2<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) @interpolate(flat) radii: vec4<f32>,
    @location(4) @interpolate(flat) border: vec4<f32>,
    @location(5) @interpolate(flat) fill: vec4<f32>,
    @location(6) @interpolate(flat) border_color: vec4<f32>,
    // kind, flags, atlas layer, 0
    @location(7) @interpolate(flat) prim: vec4<u32>,
    // Distances to the clip rectangle's left, top, right, bottom edges (positive inside), chunk px.
    @location(8) clip_rect: vec4<f32>,
    // Distances to the clip polygon's faces 0-3 and 4-7 (positive inside; FAR for missing faces).
    @location(9) clip_faces0: vec4<f32>,
    @location(10) clip_faces1: vec4<f32>,
    @location(11) @interpolate(flat) clip_radius: f32,
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

@vertex
fn vs(@builtin(vertex_index) vi: u32, inst: Instance) -> VsOut {
    let kind = inst.words.x & 0xffu;
    let flags = (inst.words.x >> 8u) & 0xffu;
    let layer = (inst.words.x >> 16u) & 0xffu;
    let xform_id = inst.words.y & 0xffffu;
    let clip_id = inst.words.y >> 16u;
    let anchor_id = inst.words.z & 0xffffu;

    let corner = quad_corner(vi);
    // Boxes get a margin for the soft edge; masks are their texels exactly.
    let margin = select(0.0, AA_MARGIN, kind == KIND_BOX);
    let local = corner * (inst.rect.zw + vec2<f32>(2.0 * margin)) - vec2<f32>(margin);
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
    o.local = local;
    o.size = inst.rect.zw;
    o.uv = mix(inst.uv.xy, inst.uv.zw, corner);
    o.radii = inst.radii;
    o.border = inst.border;
    o.fill = inst.c0;
    o.border_color = inst.border_color;
    o.prim = vec4<u32>(kind, flags, layer, 0u);

    o.clip_rect = vec4<f32>(FAR);
    o.clip_faces0 = vec4<f32>(FAR);
    o.clip_faces1 = vec4<f32>(FAR);
    o.clip_radius = 0.0;
    if clip_id != 0u {
        let r = clip_texel(clip_id, 0);
        let head = clip_texel(clip_id, 1);
        o.clip_rect = vec4<f32>(chunk - r.xy, r.xy + r.zw - chunk);
        o.clip_radius = head.x;
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

// Signed distance to a rectangle of half size `half` with per-corner radii, `p` relative to its centre.
fn sd_rounded(p: vec2<f32>, half: vec2<f32>, radii: vec4<f32>) -> f32 {
    let r = corner_radius(p, radii);
    let q = abs(p) - half + vec2<f32>(r);
    return min(max(q.x, q.y), 0.0) + length(max(q, vec2<f32>(0.0))) - r;
}

fn premul(c: vec4<f32>) -> vec4<f32> {
    return vec4<f32>(c.rgb * c.a, c.a);
}

// The fill colour at this fragment: `FILL_SOLID` (the fill mode bits of `flags`, 0). The gradient modes
// (`FILL_LINEAR`, `FILL_REPEAT`, `FILL_CONIC`) take their points from `uv`, colours from `c0..c3` and stops
// from `stops`; they branch here on `in.prim.y & 3u` once those values reach the fragment stage.
fn fill_color(in: VsOut) -> vec4<f32> {
    return in.fill;
}

// BOX: fill under a border, per-corner radii, per-side widths (CSS `background-clip: border-box`).
fn shade_box(in: VsOut, px: f32) -> vec4<f32> {
    let half = in.size * 0.5;
    let outer = inside(sd_rounded(in.local - half, half, in.radii), px);
    var fill = vec4<f32>(0.0);
    if (in.prim.y & FLAG_FILL) != 0u {
        fill = premul(fill_color(in)) * outer;
    }
    if (in.prim.y & FLAG_BORDER) == 0u {
        return fill;
    }
    let b = in.border;
    let inner_min = vec2<f32>(b.w, b.x);
    let inner_size = in.size - vec2<f32>(b.y + b.w, b.x + b.z);
    var inner = 0.0;
    if inner_size.x > 0.0 && inner_size.y > 0.0 {
        let ihalf = inner_size * 0.5;
        // SIMPLIFIED(SIMP-248): the inner corner of a border is a circle shrunk by the mean of the two adjacent widths, not the CSS ellipse when those widths differ
        let inset = 0.5 * vec4<f32>(b.w + b.x, b.y + b.x, b.y + b.z, b.w + b.z);
        let iradii = max(in.radii - inset, vec4<f32>(0.0));
        inner = inside(sd_rounded(in.local - inner_min - ihalf, ihalf, iradii), px);
    }
    let ring = clamp(outer - inner, 0.0, 1.0);
    let border = premul(in.border_color) * ring;
    return border + fill * (1.0 - border.a);
}

// MASK: coverage of a glyph or an icon times the colour. Glyph coverage already carries the rasterizer's
// gamma, so it is used as it is.
fn shade_mask(in: VsOut) -> vec4<f32> {
    let cov = textureSampleLevel(mask, mask_smp, in.uv, i32(in.prim.z), 0.0).r;
    return premul(in.fill) * cov;
}

// IMAGE: a premultiplied texel of a colour page times the tint (straight colour, its alpha an opacity), cut to
// the rounded rectangle of the instance.
fn shade_image(in: VsOut, px: f32) -> vec4<f32> {
    let s = textureSampleLevel(image, mask_smp, in.uv, i32(in.prim.z), 0.0);
    let tint = in.fill;
    let half = in.size * 0.5;
    let cover = inside(sd_rounded(in.local - half, half, in.radii), px);
    return vec4<f32>(s.rgb * tint.rgb, s.a) * (tint.a * cover);
}

// Coverage of the clip: rounded rectangle and convex polygon, anti-aliased over one pixel of chunk space.
fn clip_coverage(in: VsOut, w_rect: vec4<f32>, w0: vec4<f32>, w1: vec4<f32>) -> f32 {
    let e = in.clip_rect;
    let ex = min(e.x, e.z);
    let ey = min(e.y, e.w);
    let r = in.clip_radius;
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
    let dx = dpdx(in.local);
    let dy = dpdy(in.local);
    // Local px per screen pixel (isotropic estimate; exact for translation, rotation and uniform scale).
    let px = max(sqrt(0.5 * (dot(dx, dx) + dot(dy, dy))), 1.0e-6);
    let w_rect = fwidth(in.clip_rect);
    let w0 = fwidth(in.clip_faces0);
    let w1 = fwidth(in.clip_faces1);

    var c = vec4<f32>(0.0);
    switch in.prim.x {
        case KIND_BOX: {
            c = shade_box(in, px);
        }
        case KIND_MASK: {
            c = shade_mask(in);
        }
        case KIND_IMAGE: {
            c = shade_image(in, px);
        }
        // KIND_SHADOW comes with the shadow task; until then it draws nothing.
        default: {}
    }
    return finish(c * clip_coverage(in, w_rect, w0, w1));
}
