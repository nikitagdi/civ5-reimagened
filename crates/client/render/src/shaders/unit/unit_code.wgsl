// The `_CODE` pixel stages of the skinned units: `UnitShader::UnitSkinnedPS_CODE` (the colour pass)
// and `UnitSkinnedPS_CODE_Shadow` (the caster of the shadow map). These are the ones the D3D11
// build draws the skinned units with in every capture of S1-S4 (their DX11 twins). Hand port in
// the convention of shader-re 4.8.1.
//
// Needs frame.wgsl, sm3.wgsl and unit/unit.wgsl (its resources and `unit_ps_body`).

// ---- UnitSkinnedPS_CODE ----

struct UnitPsCodeIn {
    // dcl_texcoord v0, v1, v2.xyz, v3.yz: as `UnitPsIn`.
    v0: vec4<f32>,
    v1: vec4<f32>,
    v2: vec3<f32>,
    v3: vec3<f32>,
    // `TeamColorS` (c4), from the instance (data move `instance:team_color`), as `UnitSkinnedPS`.
    instance_team_color: vec4<f32>,
    // `AlphaValuesS` (c5): (alpha, alpha to, weight), from the instance (data move
    // `instance:alpha_values`).
    instance_alpha_values: vec3<f32>,
}

// The alpha of the `_CODE` stages: `lerp(AlphaValues.x, AlphaValues.y, AlphaValues.z)`, a third of
// it under the water (`def c, 1, 0.33`).
fn unit_code_alpha(height: f32, a: vec3<f32>) -> f32 {
    let above = height - frame_globals.fWaterHeight;
    let water = select(0.330000013, 1.0, above >= 0.0);
    let alpha = a.z * (a.y - a.x) + a.x;
    return alpha * water;
}

// re: UnitShader::UnitSkinnedPS_CODE (sha1 b42daf79f3f8)
// The code of `UnitSkinnedPS` (`UnitPS`) and the alpha of the instance in `oC0.w`.
fn unit_skinned_ps_code_body(inp: UnitPsCodeIn) -> UnitPsOut {
    var b: UnitPsIn;
    b.v0 = inp.v0;
    b.v1 = inp.v1;
    b.v2 = inp.v2;
    b.v3 = inp.v3;
    b.instance_team_color = inp.instance_team_color;
    var o = unit_ps_body(b);
    o.oc0.w = unit_code_alpha(inp.v2.z, inp.instance_alpha_values);
    return o;
}

// ---- UnitSkinnedPS_CODE_Shadow ----

struct UnitShadowPsCodeIn {
    // dcl_texcoord v0.z: the height of the point (`UnitSkinnedVS_Shadow` o1.z).
    v0: vec3<f32>,
    // `AlphaValuesS` (c5), from the instance.
    instance_alpha_values: vec3<f32>,
}

// re: UnitShader::UnitSkinnedPS_CODE_Shadow (sha1 a3ef2ead9aba)
// White, with the alpha of the instance (a third of it under the water).
fn unit_skinned_shadow_ps_code_body(inp: UnitShadowPsCodeIn) -> UnitPsOut {
    var o: UnitPsOut;
    o.oc0 = vec4<f32>(vec3<f32>(1.0), unit_code_alpha(inp.v0.z, inp.instance_alpha_values));
    return o;
}

// ---- entry points ----

struct UnitCodeVaryings {
    @builtin(position) clip: vec4<f32>,
    @location(0) v0: vec4<f32>,
    @location(1) v1: vec4<f32>,
    @location(2) v2: vec3<f32>,
    @location(3) v3: vec3<f32>,
    // The colour of the team (xyz) and the alpha of the instance (w).
    @location(4) @interpolate(flat) color: vec4<f32>,
}

@fragment
fn unit_skinned_ps_code(in: UnitCodeVaryings) -> @location(0) vec4<f32> {
    var b: UnitPsCodeIn;
    b.v0 = in.v0;
    b.v1 = in.v1;
    b.v2 = in.v2;
    b.v3 = in.v3;
    b.instance_team_color = vec4<f32>(in.color.xyz, 1.0);
    b.instance_alpha_values = vec3<f32>(in.color.w, in.color.w, 0.0);
    return unit_skinned_ps_code_body(b).oc0;
}

struct UnitShadowCodeVaryings {
    @builtin(position) clip: vec4<f32>,
    @location(0) v0: vec3<f32>,
}

@fragment
fn unit_skinned_shadow_ps_code(in: UnitShadowCodeVaryings) -> @location(0) vec4<f32> {
    var b: UnitShadowPsCodeIn;
    b.v0 = in.v0;
    b.instance_alpha_values = vec3<f32>(1.0, 1.0, 0.0);
    return unit_skinned_shadow_ps_code_body(b).oc0;
}
