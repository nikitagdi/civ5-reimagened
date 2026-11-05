// Exact semantics of the Shader Model 3 instructions where WGSL differs (render-core spec 4.7,
// shader-re spec 4.6). One text for the products and for the oracle of `shader-re`: the function
// bodies are those of `oracle::HELPERS` (a test of `render` compares them).
//
// re: the shader-re table of the oracle; not taken from one shader of the game.
// Non-finite values are made on purpose: they are D3D9's answers for rcp(0), rsq(0), log(0).
// The D3D9 multiplication rule: 0 times anything (also infinity) is 0, not NaN. So
// d3d_pow(0, 0) = 1 and d3d_nrm(0) = the zero vector (decision of the review of R1.13-R1.16).

// D3D9 semantics that WGSL builtins do not give.
fn d3d_inf(sign_of: f32) -> f32 {
    // +inf, or -inf when `sign_of` has its sign bit set (so that rcp(-0) = -inf).
    return bitcast<f32>(0x7f800000u | (bitcast<u32>(sign_of) & 0x80000000u));
}
fn d3d_rcp(x: f32) -> f32 {
    return select(1.0 / x, d3d_inf(x), x == 0.0);
}
fn d3d_rsq(x: f32) -> f32 {
    return select(inverseSqrt(abs(x)), d3d_inf(0.0), x == 0.0);
}
fn d3d_log(x: f32) -> f32 {
    return select(log2(abs(x)), -d3d_inf(0.0), x == 0.0);
}
fn d3d_pow(x: f32, y: f32) -> f32 {
    // D3D9: 0 times anything is 0, so pow(x, 0) = exp2(0) = 1 for every x, also pow(0, 0).
    return exp2(select(y * d3d_log(x), 0.0, y == 0.0));
}
fn d3d_nrm(v: vec4<f32>) -> vec4<f32> {
    // D3D9: 0 times anything is 0, so nrm(0) is the zero vector, not 0 * inf = NaN.
    return select(v * d3d_rsq(dot(v.xyz, v.xyz)), vec4<f32>(0.0), v == vec4<f32>(0.0));
}
fn d3d_cmp(a: vec4<f32>, b: vec4<f32>, c: vec4<f32>) -> vec4<f32> {
    return select(c, b, a >= vec4<f32>(0.0));
}
