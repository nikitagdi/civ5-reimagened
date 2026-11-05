// The vertices of a page of the terrain, made on the GPU (render-opt GB): what `build_cell_vertices`
// (the normals of `normals_block`, the blend vectors of `CalcBlendVector`) and
// `TerrainBaker::composite_vertices` / `cache_vertices` make on the CPU, in the same layout, written
// into the vertex buffer the passes of the bake read. One invocation a vertex of the
// `(w + 2) x (h + 2)` grid of the cell.
//
// Not a port of an entry of the original (no `re:`): the CPU code it follows is. Not in the catalog
// (no WebGL2 form: a compute stage); the bake keeps the CPU vertices where there is no compute.
//
// The CPU sends per page only the raw heights and ids around the cell (as the CPU reads them: wrapped
// and clamped) and the per-column and per-row values (clip, place in the page, world X / Y), so that
// everything that is not arithmetic on the heights is the CPU's own numbers.

struct Params {
    // W, H (vertices), the u32 offset of the columns, of the rows.
    grid: vec4<u32>,
    // The u32 offset of the window of taps, of the heights of the vertices; the vec4 offset of the
    // composite vertices and of the cache vertices in `out`.
    at: vec4<u32>,
    // The projections of the cell (`TextureSet::ids`, at most eight), the projections of the table.
    counts: vec4<u32>,
    slots: array<vec4<u32>, 2>,
    // `terrain_scale` x, y, z and `HEIGHT_TO_UNIT`.
    scale: vec4<f32>,
}

struct Table {
    // Per terrain id: `Projections::first(id) | Projections::mode(id) << 16`.
    ids: array<vec4<u32>, 64>,
    // `BlendKernel::weights`, tap `y * 5 + x`.
    kernel: array<vec4<f32>, 7>,
    // `f32::from(w) / 255.0` of a weight byte.
    lut: array<vec4<f32>, 64>,
    // The kernels of `normal_of_taps`.
    smooth_k: array<vec4<f32>, 2>,
    deriv_k: array<vec4<f32>, 2>,
    // RANGE_SCALE, EXP_SCALE, WEIGHT_NORM, FLAT_NORMAL_Z.
    normal: vec4<f32>,
    // WEIGHT_SCALE, MIN_WEIGHT_SUM (as f32), 0, 0.
    weight: vec4<f32>,
}

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<uniform> table: Table;
@group(0) @binding(2) var<storage, read> inputs: array<u32>;
@group(0) @binding(3) var<storage, read_write> verts_out: array<vec4<f32>>;

// `asset_format::f32_to_f16_bits`, bit for bit (round to nearest even).
fn f32_to_f16_bits(v: f32) -> u32 {
    let bits = bitcast<u32>(v);
    let sign = (bits >> 16u) & 0x8000u;
    let ex = i32((bits >> 23u) & 0xffu);
    let man = bits & 0x007fffffu;
    if ex == 0xff {
        return sign | 0x7c00u | select(0u, 0x0200u, man != 0u);
    }
    let e = ex - 127 + 15;
    if e >= 0x1f {
        return sign | 0x7c00u;
    }
    if e <= 0 {
        if e < -10 {
            return sign;
        }
        let m = man | 0x00800000u;
        let shift = u32(14 - e);
        var h = m >> shift;
        let rem = m & ((1u << shift) - 1u);
        let halfway = 1u << (shift - 1u);
        if rem > halfway || (rem == halfway && (h & 1u) == 1u) {
            h += 1u;
        }
        return sign | h;
    }
    var h = sign | (u32(e) << 10u) | (man >> 13u);
    let rem = man & 0x1fffu;
    if rem > 0x1000u || (rem == 0x1000u && (h & 1u) == 1u) {
        h += 1u;
    }
    return h;
}

// `asset_format::f16_bits_to_f32`, bit for bit (no flush of the subnormal halves).
fn f16_bits_to_f32(h: u32) -> f32 {
    let sign = (h & 0x8000u) << 16u;
    let ex = (h >> 10u) & 0x1fu;
    let man = h & 0x03ffu;
    if ex == 0u {
        if man == 0u {
            return bitcast<f32>(sign);
        }
        let lz = countLeadingZeros(man) - 21u;
        let e = 127u - 15u + 1u - lz;
        return bitcast<f32>(sign | (e << 23u) | (((man << lz) & 0x03ffu) << 13u));
    }
    if ex == 0x1fu {
        return bitcast<f32>(sign | 0x7f800000u | (man << 13u));
    }
    return bitcast<f32>(sign | ((ex + 112u) << 23u) | (man << 13u));
}

// A float through a half and back, as `pack_halves` and `cache_vertices` do.
fn through_half(v: f32) -> f32 {
    return f16_bits_to_f32(f32_to_f16_bits(v));
}

fn smooth_k(i: u32) -> f32 {
    return table.smooth_k[i >> 2u][i & 3u];
}

fn deriv_k(i: u32) -> f32 {
    return table.deriv_k[i >> 2u][i & 3u];
}

@compute @workgroup_size(8, 8, 1)
fn bake_vertices(@builtin(global_invocation_id) gid: vec3<u32>) {
    let w = params.grid.x;
    let h = params.grid.y;
    if gid.x >= w || gid.y >= h {
        return;
    }
    let gx = gid.x;
    let gy = gid.y;
    let ww = w + 4u;
    let window_at = params.at.x;
    let h2u = params.scale.w;
    let scale = params.scale.xyz;

    // `TerrainSampler::normal_of_taps` on the 5 x 5 taps around the vertex: tap (i, j) at the offset
    // (i - 2, j - 2), the window starts two texels before the first vertex.
    let tap = window_at + gy * ww + gx;
    let r32 = i32(inputs[tap + 2u * ww + 3u] & 0xffffu);
    let r12 = i32(inputs[tap + 2u * ww + 1u] & 0xffffu);
    let r23 = i32(inputs[tap + 3u * ww + 2u] & 0xffffu);
    let r21 = i32(inputs[tap + 1u * ww + 2u] & 0xffffu);
    let center = f32(inputs[tap + 2u * ww + 2u] & 0xffffu) * h2u;
    let plane_x = f32((r32 - r12) >> 1u) * h2u + center;
    let plane_y = f32((r23 - r21) >> 1u) * h2u + center;
    let range_scale = table.normal.x;
    let exp_scale = table.normal.y;
    let weight_norm = table.normal.z;
    var sum_x = 0.0;
    var sum_y = 0.0;
    var weight_x = 0.0;
    var weight_y = 0.0;
    for (var i = 0u; i < 5u; i += 1u) {
        for (var j = 0u; j < 5u; j += 1u) {
            let hh = f32(inputs[tap + j * ww + i] & 0xffffu) * h2u;
            let ux = (plane_x - hh) * range_scale;
            let uy = (plane_y - hh) * range_scale;
            let tx = ux * exp_scale * ux;
            let ty = uy * exp_scale * uy;
            let txx = tx * tx;
            let tyy = ty * ty;
            let wx = (tx * txx / 6.0 + (txx * 0.5 + (tx + 1.0))) * weight_norm * deriv_k(i)
                * smooth_k(j);
            let wy = (ty * tyy / 6.0 + (tyy * 0.5 + (ty + 1.0))) * weight_norm * smooth_k(i)
                * deriv_k(j);
            sum_x += hh * wx;
            sum_y += hh * wy;
            weight_x += abs(wx);
            weight_y += abs(wy);
        }
    }
    let slope_x = (sum_x / weight_x) * scale.z;
    let slope_y = (sum_y / weight_y) * scale.z;
    let n0 = vec3<f32>(slope_x * scale.y, scale.x * slope_y, scale.x * scale.y);
    // glam's `normalize`: times the reciprocal of the length.
    let len = sqrt(n0.x * n0.x + n0.y * n0.y + n0.z * n0.z);
    let n = n0 * (1.0 / len);

    // `projection_mode_of_normal`.
    var mode = 0u;
    if !(n.z > table.normal.w) {
        mode = select(1u, 2u, abs(n.y) <= abs(n.x));
    }

    // `blend_of_ids`: the 25 taps of the kernel on the ids around the vertex.
    var blend: array<f32, 64>;
    for (var ky = 0u; ky < 5u; ky += 1u) {
        for (var kx = 0u; kx < 5u; kx += 1u) {
            let t = ky * 5u + kx;
            let id = (inputs[tap + ky * ww + kx] >> 16u) & 0xffu;
            let e = table.ids[id >> 2u][id & 3u];
            var p = e & 0xffffu;
            if mode != 0u && (e >> 16u) == 1u {
                p += mode;
            }
            if p < 64u {
                blend[p] += table.kernel[t >> 2u][t & 3u];
            }
        }
    }
    let count = min(params.counts.y, 64u);
    if count != 0u {
        var sum = 0.0;
        for (var k = 0u; k < count; k += 1u) {
            sum += blend[k];
        }
        if sum > 0.0 {
            let inv = 1.0 / sum;
            for (var k = 0u; k < count; k += 1u) {
                blend[k] *= inv;
            }
        }
    }
    var wts = array<f32, 8>(0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    var tex = array<u32, 8>(0u, 0u, 0u, 0u, 0u, 0u, 0u, 0u);
    let textures = min(params.counts.x, 8u);
    if textures == 0u {
        wts[0] = 1.0;
    } else {
        var sum = 0.0;
        for (var i = 0u; i < textures; i += 1u) {
            let p = params.slots[i >> 2u][i & 3u];
            tex[i] = p & 0xffu;
            var b = 0.0;
            if p < 64u {
                b = blend[p];
            }
            wts[i] = b;
            sum += b;
        }
        if sum < table.weight.y {
            wts[0] = 1.0;
        }
    }
    var bytes = array<u32, 8>(0u, 0u, 0u, 0u, 0u, 0u, 0u, 0u);
    for (var i = 0u; i < 8u; i += 1u) {
        // `(f * 255) as u32 as u8`.
        bytes[i] = u32(max(wts[i] * table.weight.x, 0.0)) & 0xffu;
    }
    let lut = array<f32, 8>(
        table.lut[bytes[0] >> 2u][bytes[0] & 3u],
        table.lut[bytes[1] >> 2u][bytes[1] & 3u],
        table.lut[bytes[2] >> 2u][bytes[2] & 3u],
        table.lut[bytes[3] >> 2u][bytes[3] & 3u],
        table.lut[bytes[4] >> 2u][bytes[4] & 3u],
        table.lut[bytes[5] >> 2u][bytes[5] & 3u],
        table.lut[bytes[6] >> 2u][bytes[6] & 3u],
        table.lut[bytes[7] >> 2u][bytes[7] & 3u],
    );

    // The column and the row: clip, place in the page through a half, world X / Y.
    let col = params.grid.z + gx * 4u;
    let row = params.grid.w + gy * 4u;
    let clip_x = bitcast<f32>(inputs[col]);
    let page_u = bitcast<f32>(inputs[col + 1u]);
    let world_x = bitcast<f32>(inputs[col + 2u]);
    let clip_y = bitcast<f32>(inputs[row]);
    let page_v = bitcast<f32>(inputs[row + 1u]);
    let world_y = bitcast<f32>(inputs[row + 2u]);
    let raw = inputs[params.at.y + gy * w + gx] & 0xffffu;
    let world_z = f32(raw) * h2u * scale.z;

    let v = gy * w + gx;
    let c = params.at.z + v * 6u;
    verts_out[c] = vec4<f32>(clip_x, clip_y, 0.5, 1.0);
    verts_out[c + 1u] = vec4<f32>(world_x, world_y, world_z, 1.0);
    verts_out[c + 2u] = vec4<f32>(lut[0], lut[1], lut[2], lut[3]);
    verts_out[c + 3u] = vec4<f32>(lut[4], lut[5], lut[6], lut[7]);
    verts_out[c + 4u] = vec4<f32>(f32(tex[0]), f32(tex[1]), f32(tex[2]), f32(tex[3]));
    verts_out[c + 5u] = vec4<f32>(f32(tex[4]), f32(tex[5]), f32(tex[6]), f32(tex[7]));
    let k = params.at.w + v * 2u;
    verts_out[k] = vec4<f32>(through_half(n.x), through_half(n.y), through_half(n.z), 0.0);
    verts_out[k + 1u] = vec4<f32>(page_u, page_v, 0.0, 0.0);
}
