// IceShader of the original, the ice sheets on the sea (world-water): `RenderSceneVS` and
// `PS_Ice`. Hand ports in the convention of shader-re 4.8.1 (see river.wgsl): each entry of the
// original is a body `fn <entry>_body(inp: In) -> Out` with the registers of the original as member
// names, and a `@vertex` / `@fragment` entry that only reads its inputs, calls the body and does
// the `texkill` discard.
//
// Needs sm3.wgsl. Group 0: binding 0 the constants of the vertex shader (one register or matrix per
// member, in the order of the registers), binding 1 those of the pixel shader (one `vec4` per
// register c0..c17, named by the CTAB; the CTAB types float, float2, float3 sit in the first
// components), then the textures of the samplers in pairs (texture `S`, sampler `S_smp`).
//
// The ice is a floe mesh: a quad of the bounding box (`g_vBBoxMin` / `g_vBBoxMax` in the xy of the
// world) with the height from the vertex (`v1.w` scaled), and a normal packed in `v1.xyz`. The pixel
// shader blends three projections (along x, along y and along z of the normal) of a diffuse map and
// of a LEAN map for the light: each projection is one lobe of the sun's specular, the sky of the
// cube map and the diffuse of the sun; above the sea level the colour is lit, below it the ice is
// tinted and made thinner with the depth.
//
// LEAN: `g_Lean1` and `g_Lean1_Side` are the first maps (the slopes in xy and the second moments in
// zw), `g_Lean2_Side` the second map of `ice_bump_sides.lean`: the original reads its `w`, ours has
// the value in `x` (a one channel texture, `LEAN_CROSS` of formats.toml). Every read is rounded
// to the grid of 16 bits as the oracle does.

// Register `k` of a `row_major float4x4`: `[j][k]` of the WGSL matrix is component j of register k.
fn row_major_reg(m: mat4x4<f32>, k: u32) -> vec4<f32> {
    return vec4<f32>(m[0][k], m[1][k], m[2][k], m[3][k]);
}

struct IceVsConsts {
    // c0..c3
    g_ViewProjMatrix: mat4x4<f32>,
    // c4 (xy)
    g_vBBoxMin: vec4<f32>,
    // c5 (xy)
    g_vBBoxMax: vec4<f32>,
    // c6 (x)
    g_fHeightScale: vec4<f32>,
    // c7 (x)
    g_fHeightBias: vec4<f32>,
    // c8..c11
    g_OverlayProjection: mat4x4<f32>,
    // c12 (xyz)
    g_vEye: vec4<f32>,
    // c13
    f4FOWBaseScale: vec4<f32>,
}

struct IcePsConsts {
    g_vEye: vec4<f32>,
    pad1: vec4<f32>,
    g_vSunDir: vec4<f32>,
    g_vSpecSunDir: vec4<f32>,
    g_fLeanGradientScale: vec4<f32>,
    g_fDiffuseRepeat: vec4<f32>,
    g_fBumpRepeat: vec4<f32>,
    g_fSpecularPower: vec4<f32>,
    g_vSubSurfaceColor: vec4<f32>,
    g_fSeaLevel: vec4<f32>,
    g_vSideDiffuseRepeat: vec4<f32>,
    g_vSideBumpRepeat: vec4<f32>,
    g_vUnderwaterTint1: vec4<f32>,
    g_vUnderwaterTint2: vec4<f32>,
    g_fUnderwaterColorRamp: vec4<f32>,
    g_fUnderwaterAlphaRamp: vec4<f32>,
    g_FogColor: vec4<f32>,
    g_FogConsts: vec4<f32>,
}

@group(0) @binding(0) var<uniform> ice_vs: IceVsConsts;
@group(0) @binding(1) var<uniform> ice_ps: IcePsConsts;

@group(0) @binding(2) var g_Cube: texture_cube<f32>;
@group(0) @binding(3) var g_Cube_smp: sampler;
@group(0) @binding(4) var g_Diffuse: texture_2d<f32>;
@group(0) @binding(5) var g_Diffuse_smp: sampler;
@group(0) @binding(6) var g_Lean1: texture_2d<f32>;
@group(0) @binding(7) var g_Lean1_smp: sampler;
@group(0) @binding(8) var g_Diffuse_Side: texture_2d<f32>;
@group(0) @binding(9) var g_Diffuse_Side_smp: sampler;
@group(0) @binding(10) var g_Lean1_Side: texture_2d<f32>;
@group(0) @binding(11) var g_Lean1_Side_smp: sampler;
@group(0) @binding(12) var g_Lean2_Side: texture_2d<f32>;
@group(0) @binding(13) var g_Lean2_Side_smp: sampler;
@group(0) @binding(14) var g_FogRamp: texture_2d<f32>;
@group(0) @binding(15) var g_FogRamp_smp: sampler;
@group(0) @binding(16) var FOWMask: texture_2d<f32>;
@group(0) @binding(17) var FOWMask_smp: sampler;
@group(0) @binding(18) var FOWBaseLayer: texture_2d<f32>;
@group(0) @binding(19) var FOWBaseLayer_smp: sampler;
@group(0) @binding(20) var FOWColorKey: texture_3d<f32>;
@group(0) @binding(21) var FOWColorKey_smp: sampler;

// ---- RenderSceneVS ----

struct IceVsIn {
    // dcl_texcoord v0 (xy: the corner in [-1, 1]), dcl_texcoord1 v1 (xyz: the packed normal, w: the
    // height).
    v0: vec4<f32>,
    v1: vec4<f32>,
}

struct IceVsOut {
    // The normal.
    o0: vec3<f32>,
    // The point in the world.
    o1: vec3<f32>,
    // The vector to the eye.
    o2: vec3<f32>,
    // The clip position divided by w (xy).
    o3: vec2<f32>,
    // The overlay maps' coordinates.
    o4: vec2<f32>,
    // The fog of war's base layer.
    o5: vec2<f32>,
    o6: vec4<f32>,
}

// re: IceShader::RenderSceneVS (sha1 39668cf48ead)
fn renderscenevs_body(inp: IceVsIn) -> IceVsOut {
    let c = ice_vs;
    var o: IceVsOut;
    // def c14 = (0.5, 2, -1, -0.5)
    o.o0 = inp.v1.xyz * 2.0 + vec3<f32>(-1.0);
    var r0x = inp.v0.x * 0.5 + 0.5;
    let r1xy = c.g_vBBoxMin.xy;
    let r0y = c.g_vBBoxMax.x - r1xy.x;
    let r0z = c.g_vBBoxMax.y - r1xy.y;
    let p_x = r0x * r0y + c.g_vBBoxMin.x;
    r0x = inp.v0.y * 0.5 + 0.5;
    let p_y = r0x * r0z + c.g_vBBoxMin.y;
    let hs = c.g_fHeightScale.x;
    let p_z = inp.v1.w * hs + c.g_fHeightBias.x;
    let p = vec3<f32>(p_x, p_y, p_z);
    o.o2 = -p + c.g_vEye.xyz;
    let m = c.g_ViewProjMatrix;
    var r0 = p.y * row_major_reg(m, 1u);
    r0 = p.x * row_major_reg(m, 0u) + r0;
    r0 = p.z * row_major_reg(m, 2u) + r0;
    r0 = r0 + row_major_reg(m, 3u);
    let rw = d3d_rcp(r0.w);
    o.o3 = r0.xy * rw;
    o.o6 = r0;
    let fow = (p.xy + c.f4FOWBaseScale.zw) * c.f4FOWBaseScale.xy;
    o.o5 = fow;
    let om = c.g_OverlayProjection;
    var q = p.y * row_major_reg(om, 1u).xy;
    o.o1 = p;
    q = p.x * row_major_reg(om, 0u).xy + q;
    q = p.z * row_major_reg(om, 2u).xy + q;
    q = q + row_major_reg(om, 3u).xy;
    o.o4 = q * vec2<f32>(0.5, -0.5) + vec2<f32>(-0.5);
    return o;
}

// ---- PS_Ice ----

struct IcePsIn {
    // dcl_texcoord v0.xyz: the normal. v1.xyz: the point in the world. v2.xyz: the vector to the
    // eye. v3.y: the clip y divided by w. v4.xy: the overlay maps. v5.xy: the fog of war's base.
    v0: vec3<f32>,
    v1: vec3<f32>,
    v2: vec3<f32>,
    v3: vec2<f32>,
    v4: vec2<f32>,
    v5: vec2<f32>,
}

struct IcePsOut {
    oc0: vec4<f32>,
    kill: bool,
}

// A LEAN map as the original stores it, 16 bits per channel unorm; ours is half float, so the value
// read is rounded to the grid of 16 bits (as in ocean.wgsl, and the oracle of shader-re does too).
fn ice_lean_1(uv: vec2<f32>) -> vec4<f32> {
    return round(textureSample(g_Lean1, g_Lean1_smp, uv) * 65535.0) / 65535.0;
}

fn ice_lean_1_side(uv: vec2<f32>) -> vec4<f32> {
    return round(textureSample(g_Lean1_Side, g_Lean1_Side_smp, uv) * 65535.0) / 65535.0;
}

// The original reads `.w` of the second map of the sides; ours has the value in `.x`.
fn ice_lean_cross_side(uv: vec2<f32>) -> f32 {
    return round(textureSample(g_Lean2_Side, g_Lean2_Side_smp, uv).x * 65535.0) / 65535.0;
}

// One lobe of the specular light: `h` the half vector in the slope space minus the mean slope,
// `cov` (xx, yy, xy) the covariance of the LEAN map, `eps` the roughness of the light. Gives the
// density, 0 where the determinant is not positive or the exponent is 16 or more.
fn ice_lobe(h: vec2<f32>, cov: vec3<f32>, eps: f32) -> f32 {
    let a = eps + cov.x;
    let b = eps + cov.y;
    var det = cov.z * cov.z;
    det = a * b - det;
    var q = h.x * h.x;
    var t = h.y * b;
    // dp2add: the scalar is both components, so the product counts twice.
    t = h.x * -cov.z + h.x * -cov.z + t;
    t = h.y * t;
    q = q * b + t;
    q = q * 0.5;
    let rdet = d3d_rcp(det);
    let ex = q * rdet;
    let flag_det = select(0.0, 1.0, -det >= 0.0);
    let flag_ex = select(0.0, 1.0, q * rdet + -16.0 >= 0.0);
    let flag = flag_ex + flag_det;
    var d = exp2(ex * -1.44269502);
    d = d3d_rsq(det) * d;
    return select(0.0, d, -flag >= 0.0);
}

// re: IceShader::PS_Ice (sha1 1dc27fc552ef)
fn ps_ice_body(inp: IcePsIn) -> IcePsOut {
    var o: IcePsOut;
    let k = ice_ps;
    // def c1 = (0.5, -16, -1.44269502, 1.44269502)
    // def c19 = (-1, -0, 1, 2), def c20 = (0.3, 0.59, 0.11, 0.454545468)
    let fow = textureSample(FOWMask, FOWMask_smp, inp.v4);
    o.kill = fow.x == 0.0;
    let n = d3d_nrm(vec4<f32>(inp.v0, 0.0)).xyz;
    let e = d3d_nrm(vec4<f32>(inp.v2, 0.0)).xyz;
    let v1 = inp.v1;
    // The weights of the three projections: the fourth powers of the normal, normalised.
    let r3 = abs(n) * abs(n);
    let r3_q = vec3<f32>(r3.x * r3.x, r3.y * r3.y, r3.z * r3.z);
    var sum = r3_q.y + r3_q.x;
    sum = r3.z * r3.z + sum;
    sum = d3d_rcp(sum);
    let wt = sum * r3_q;

    // The diffuse of the three projections.
    let sdr = k.g_vSideDiffuseRepeat.xy;
    let d_a = textureSample(g_Diffuse_Side, g_Diffuse_Side_smp, vec2<f32>(sdr.x * v1.y, sdr.y * v1.z));
    let d_b = textureSample(g_Diffuse_Side, g_Diffuse_Side_smp, vec2<f32>(sdr.x * v1.x, sdr.y * v1.z));
    var diff = wt.y * d_b.xyz;
    diff = d_a.xyz * wt.x + diff;
    let d_c = textureSample(
        g_Diffuse,
        g_Diffuse_smp,
        vec2<f32>(k.g_fDiffuseRepeat.x * v1.x, k.g_fDiffuseRepeat.x * v1.y),
    );
    diff = d_c.xyz * wt.z + diff;

    let sun = k.g_vSunDir.xyz;
    let spec = k.g_vSpecSunDir.xyz;
    let gs = k.g_fLeanGradientScale.x;
    let gs2 = gs * gs;
    let sbr = k.g_vSideBumpRepeat.xy;

    // The vectors of the three projections.
    let va = vec3<f32>(-n.z, n.y, n.x);
    let vb = vec3<f32>(n.x, n.z, -n.y);
    let vc = vec3<f32>(-n.y, n.x, n.z);
    // Projection A (along x): the first side.
    let uv_a = vec2<f32>(sbr.x * v1.y, sbr.y * v1.z);
    let e_ay = dot(va, e);
    let e_aw = dot(vc, e);
    let e_az = dot(n, e);
    let sun_az = dot(va, sun);
    let sun_aw = dot(vc, sun);
    let sun_ax = dot(n, sun);
    let sp_ay = dot(va, spec);
    let sp_aw = dot(vc, spec);
    let sp_az = dot(n, spec);
    let ca = ice_lean_cross_side(uv_a);
    let la = ice_lean_1_side(uv_a);
    let sl_a = (la.xy * 2.0 + vec2<f32>(-1.0)) * gs;
    let nl_a = d3d_nrm(vec4<f32>(sl_a, 1.0, 0.0)).xyz;
    let mom_a = vec3<f32>(la.z, la.w, ca * 2.0 + -1.0);
    let cov_a = mom_a * gs2 - vec3<f32>(sl_a.x * sl_a.x, sl_a.y * sl_a.y, sl_a.y * sl_a.x);
    let ndl_a = saturate(dot(vec3<f32>(sun_az, sun_aw, sun_ax), nl_a));
    let spd_a = saturate(dot(vec3<f32>(sp_ay, sp_aw, sp_az), nl_a));
    let hv_a = vec3<f32>(e_ay, e_aw, e_az) + vec3<f32>(sp_ay, sp_aw, sp_az);
    let rz_a = d3d_rcp(hv_a.z);
    let h_a = hv_a.xy * rz_a - sl_a;
    let eps = d3d_rcp(k.g_fSpecularPower.x);
    let dens_a = ice_lobe(h_a, cov_a, eps);
    var fr_a = 1.0 - (nl_a.x * e_ay + nl_a.z * e_az + nl_a.y * e_aw);
    var fr_a2 = fr_a * fr_a;
    fr_a2 = fr_a2 * fr_a2;
    fr_a = fr_a * fr_a2;
    let cube_a = textureSample(
        g_Cube,
        g_Cube_smp,
        vec3<f32>(
            dot(nl_a, vec3<f32>(-n.z, -n.y, n.x)),
            dot(vec3<f32>(nl_a.y, nl_a.x, nl_a.z), vec3<f32>(n.x, n.y, n.y)),
            dot(nl_a, vec3<f32>(n.x, n.z, n.z)),
        ),
    );
    var c_a = diff * (cube_a.xyz + ndl_a);
    let k_a = saturate(dens_a * fr_a);
    c_a = k_a * spd_a + c_a;

    // Projection B (along y): the second side.
    let uv_b = vec2<f32>(sbr.x * v1.x, sbr.y * v1.z);
    let vb2 = vec3<f32>(n.z, n.y, -n.x);
    let e_bx = dot(vec3<f32>(-n.x, n.z, n.y), e);
    let sun_by = dot(vec3<f32>(-n.x, n.z, n.y), sun);
    let sp_bx = dot(vec3<f32>(-n.x, n.z, n.y), spec);
    let cb = ice_lean_cross_side(uv_b);
    let lb = ice_lean_1_side(uv_b);
    let sl_b = (lb.xy * 2.0 + vec2<f32>(-1.0)) * gs;
    let nl_b = d3d_nrm(vec4<f32>(sl_b, 1.0, 0.0)).xyz;
    let mom_b = vec3<f32>(lb.z, lb.w, cb * 2.0 + -1.0);
    let cov_b = mom_b * gs2 - vec3<f32>(sl_b.x * sl_b.x, sl_b.y * sl_b.y, sl_b.y * sl_b.x);
    let ndl_b = saturate(dot(vec3<f32>(sun_by, sun_aw, sun_ax), nl_b));
    let spd_b = saturate(dot(vec3<f32>(sp_bx, sp_aw, sp_az), nl_b));
    let hv_b = vec3<f32>(e_bx + sp_bx, e_aw + sp_aw, e_az + sp_az);
    let rz_b = d3d_rcp(hv_b.z);
    let h_b = hv_b.xy * rz_b - sl_b;
    let dens_b = ice_lobe(h_b, cov_b, eps);
    var fr_b = 1.0 - (nl_b.x * e_bx + nl_b.z * e_az + nl_b.y * e_aw);
    var fr_b2 = fr_b * fr_b;
    fr_b2 = fr_b2 * fr_b2;
    fr_b = fr_b * fr_b2;
    let cube_b = textureSample(
        g_Cube,
        g_Cube_smp,
        vec3<f32>(
            dot(nl_b, vec3<f32>(-n.x, -n.y, n.x)),
            dot(vec3<f32>(nl_b.y, nl_b.z, nl_b.x), n),
            dot(nl_b, vec3<f32>(n.y, n.z, n.z)),
        ),
    );
    var c_b = diff * (cube_b.xyz + ndl_b);
    let k_b = saturate(dens_b * fr_b);
    c_b = k_b * spd_b + c_b;

    // Projection C (along z): the top, the map `g_Lean1` (its `w` is both the second moment and,
    // turned to [-1, 1], the cross term, as in the original).
    let uv_c = k.g_fBumpRepeat.x * v1.xy;
    let e_c = vec3<f32>(dot(vb2, e), dot(vb, e), e_az);
    let sun_c = vec3<f32>(dot(vb2, sun), dot(vb, sun), sun_ax);
    let sp_c = vec3<f32>(dot(vb2, spec), dot(vb, spec), sp_az);
    let lc = ice_lean_1(uv_c);
    let sl_c = (lc.xy * 2.0 + vec2<f32>(-1.0)) * gs;
    let nl_c = d3d_nrm(vec4<f32>(sl_c, 1.0, 0.0)).xyz;
    let mom_c = lc.zww * vec3<f32>(1.0, 1.0, 2.0) + vec3<f32>(0.0, 0.0, -1.0);
    let cov_c = mom_c * gs2 - vec3<f32>(sl_c.x * sl_c.x, sl_c.y * sl_c.y, sl_c.y * sl_c.x);
    let ndl_c = saturate(dot(sun_c, nl_c));
    let spd_c = saturate(dot(sp_c, nl_c));
    let hv_c = e_c + sp_c;
    let rz_c = d3d_rcp(hv_c.z);
    let h_c = hv_c.xy * rz_c - sl_c;
    let dens_c = ice_lobe(h_c, cov_c, eps);
    var fr_c = 1.0 - dot(nl_c, e_c);
    var fr_c2 = fr_c * fr_c;
    fr_c2 = fr_c2 * fr_c2;
    fr_c = fr_c * fr_c2;
    let cube_c = textureSample(
        g_Cube,
        g_Cube_smp,
        vec3<f32>(
            dot(vec3<f32>(nl_c.y, nl_c.z, nl_c.x), vec3<f32>(n.x, n.x, n.z)),
            dot(vec3<f32>(nl_c.x, nl_c.z, nl_c.y), vec3<f32>(n.y, n.y, n.z)),
            dot(nl_c, vec3<f32>(-n.x, -n.y, n.z)),
        ),
    );
    var c_c = diff * (cube_c.xyz + ndl_c);
    let k_c = saturate(dens_c * fr_c);
    c_c = k_c * spd_c + c_c;

    // The three projections blended, then the sub-surface colour toward the light.
    let m_b = wt.y * c_b;
    var blend = c_a * wt.x + m_b;
    blend = c_c * wt.z + blend;
    var luma = dot(diff, vec3<f32>(0.300000012, 0.589999974, 0.109999999));
    luma = -luma + 1.0;
    let sub = luma * k.g_vSubSurfaceColor.xyz;
    let ndl = saturate(sun_ax);
    let cube_n = textureSample(g_Cube, g_Cube_smp, n);
    let lit = sub * (cube_n.xyz + ndl) + blend;

    // The distance fog.
    let ramp = textureSample(
        g_FogRamp,
        g_FogRamp_smp,
        vec2<f32>(inp.v3.y * 0.5 + 0.5, k.g_FogConsts.w),
    );
    let d = v1 - k.g_vEye.xyz;
    var dist2 = dot(d, d);
    dist2 = dist2 * k.g_FogConsts.y;
    var ht = d.z * k.g_FogConsts.x;
    ht = ht * 1.44269502;
    ht = exp2(ht);
    ht = -ht + 1.0;
    var fog = ht * dist2;
    fog = d3d_rcp(d.z) * fog;
    fog = fog * 1.44269502;
    fog = saturate(exp2(fog));
    fog = -fog + 1.0;
    let fog_amount = fog * ramp.x;

    // Above the sea level the lit ice, below it the tinted ice that fades with the depth.
    let above = v1.z - k.g_fSeaLevel.x;
    let is_above = above >= 0.0;
    let alpha = select(0.0, 1.0, is_above);
    let depth = k.g_fSeaLevel.x - v1.z;
    let ramp_c = saturate(depth * k.g_fUnderwaterColorRamp.x);
    let t1 = k.g_vUnderwaterTint1.xyz;
    let t2 = k.g_vUnderwaterTint2.xyz;
    let tint = ramp_c * (-t1 + t2) + t1;
    let under_col = lit * tint;
    let rz = d3d_rcp(-e.z);
    let dep = depth * rz;
    let ev = dep * e;
    var len = dot(ev, ev);
    len = d3d_rsq(len);
    len = d3d_rcp(len);
    var att = -len * k.g_fUnderwaterAlphaRamp.x;
    att = att * 1.44269502;
    att = exp2(att);
    var ez = e.z * e.z;
    ez = ez * ez;
    ez = e.z * -ez + 1.0;
    ez = -ez + 1.0;
    att = att * ez;
    var under = att * under_col;
    let keep = ramp.x * -fog + 1.0;
    under = keep * under;
    let r1 = select(vec4<f32>(under, 0.0), vec4<f32>(lit, fog_amount), is_above);

    // Gamma 1 / 2.2, the fog of war, the colour key and the distance fog.
    let g = vec3<f32>(d3d_log(r1.x), d3d_log(r1.y), d3d_log(r1.z)) * 0.454545468;
    let col = vec3<f32>(exp2(g.x), exp2(g.y), exp2(g.z));
    let base = textureSample(FOWBaseLayer, FOWBaseLayer_smp, inp.v5);
    let seen = fow.x * (col - base.xyz) + base.xyz;
    let key = textureSample(FOWColorKey, FOWColorKey_smp, seen);
    let amt = fow.z * -fow.x + fow.x;
    let fowd = amt * (key.xyz - seen) + seen;
    let to_fog = -fowd + k.g_FogColor.xyz;
    o.oc0 = vec4<f32>(r1.w * to_fog + fowd, alpha);
    return o;
}

// ---- the entries ----

struct IceVertex {
    @builtin(position) clip: vec4<f32>,
    @location(0) t0: vec3<f32>,
    @location(1) t1: vec3<f32>,
    @location(2) t2: vec3<f32>,
    @location(3) t3: vec2<f32>,
    @location(4) t4: vec2<f32>,
    @location(5) t5: vec2<f32>,
}

@vertex
fn vs_ice(
    @location(0) v0: vec4<f32>,
    @location(1) v1: vec4<f32>,
) -> IceVertex {
    var inp: IceVsIn;
    inp.v0 = v0;
    inp.v1 = v1;
    let r = renderscenevs_body(inp);
    var out: IceVertex;
    out.t0 = r.o0;
    out.t1 = r.o1;
    out.t2 = r.o2;
    out.t3 = r.o3;
    out.t4 = r.o4;
    out.t5 = r.o5;
    out.clip = r.o6;
    return out;
}

@fragment
fn ps_ice(in: IceVertex) -> @location(0) vec4<f32> {
    var inp: IcePsIn;
    inp.v0 = in.t0;
    inp.v1 = in.t1;
    inp.v2 = in.t2;
    inp.v3 = in.t3;
    inp.v4 = in.t4;
    inp.v5 = in.t5;
    let r = ps_ice_body(inp);
    if r.kill {
        discard;
    }
    return r.oc0;
}
