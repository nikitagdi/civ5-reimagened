// Gamma of the original: the frame is not sRGB, the shaders do
// their own pow/sqrt. Terrain: pow(x, 2.2) in, pow(x, 1 / 2.2) out; models: the inputs squared, the
// result sqrt (gamma 2.0). Needs sm3.wgsl.
//
// re: research approach 1.4 (gamma of terrain and models).

const TERRAIN_GAMMA: f32 = 2.2;

fn terrain_to_linear(c: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(
        d3d_pow(c.x, TERRAIN_GAMMA),
        d3d_pow(c.y, TERRAIN_GAMMA),
        d3d_pow(c.z, TERRAIN_GAMMA),
    );
}

fn terrain_to_gamma(c: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(
        d3d_pow(c.x, 1.0 / TERRAIN_GAMMA),
        d3d_pow(c.y, 1.0 / TERRAIN_GAMMA),
        d3d_pow(c.z, 1.0 / TERRAIN_GAMMA),
    );
}

fn model_to_linear(c: vec3<f32>) -> vec3<f32> {
    return c * c;
}

fn model_to_gamma(c: vec3<f32>) -> vec3<f32> {
    return sqrt(max(c, vec3<f32>(0.0)));
}
