// The entries the skinned units of `world-objects` draw with: the `_CODE` stages of the original
// (`UnitSkinnedPS_CODE`, `UnitSkinnedPS_CODE_Shadow`, ported in `unit/unit_code.wgsl`), which the
// D3D11 build draws every skinned unit of S1-S4 with (S1a ev 313, 580, 2095).
//
// The bodies are the original's. What the entries add: the instance gives the team colour and the
// alpha (`AlphaValuesS` = (alpha, alpha, 0): the instance has one alpha, the lerp is then that
// alpha), the fog of war of `world-objects` (`skin_fow_*`, as `unit_skinned_ps`), and the cut at the
// water plane in the reflection pass. The main pass blends as S1a does (`BlendMode::UnitCode`).
//
// Needs frame.wgsl, sm3.wgsl, skin.wgsl, skin/common.wgsl, unit/unit.wgsl,
// skin/unit_skinned.wgsl (the vertex outputs) and unit/unit_code.wgsl (the bodies).

fn unit_code_ps_impl(in: UnitSkinnedVertexOut, reflection: bool) -> vec4<f32> {
    var b: UnitPsCodeIn;
    b.v0 = in.v0;
    b.v1 = in.v1;
    b.v2 = in.v2;
    b.v3 = in.v3;
    // The colour of the team as the instance gives it: gamma values 0..1.
    b.instance_team_color = vec4<f32>(in.color.xyz, 1.0);
    b.instance_alpha_values = vec3<f32>(in.color.w, in.color.w, 0.0);
    let r = unit_skinned_ps_code_body(b);
    let m = skin_fow_mask(in.v2.xy);
    if (m.x < FOW_UNREVEALED || (reflection && reflection_clipped(in.v2))) {
        discard;
    }
    // Never dimmed under the fog (as unit_skinned_ps).
    return vec4<f32>(r.oc0.xyz, r.oc0.w);
}

@fragment
fn unit_code_ps(in: UnitSkinnedVertexOut) -> @location(0) vec4<f32> {
    return unit_code_ps_impl(in, false);
}

@fragment
fn unit_code_reflect_ps(in: UnitSkinnedVertexOut) -> @location(0) vec4<f32> {
    return unit_code_ps_impl(in, true);
}

// The caster: white, the alpha of the instance (a third of it under the water). The blur reads the
// alpha of the raw map (`shadow/blur.wgsl`).
@fragment
fn unit_code_shadow_ps(in: UnitSkinnedShadowVertexOut) -> @location(0) vec4<f32> {
    var b: UnitShadowPsCodeIn;
    b.v0 = in.v0;
    b.instance_alpha_values = vec3<f32>(in.alpha, in.alpha, 0.0);
    return unit_skinned_shadow_ps_code_body(b).oc0;
}
