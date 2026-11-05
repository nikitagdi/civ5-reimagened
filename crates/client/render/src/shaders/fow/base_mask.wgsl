// The cloud layers in the class mask pass: `BasePS` without its colour. The
// pixels where the clouds show (the mask's cloud channel over 0.01, as in `BasePS`) get the class
// bit `FOG_BIT` (0x40). Used with `base.wgsl`, whose vertex stage, constants and textures it shares.

override MASK_CLASS: u32 = 0u;

@fragment
fn fogbase_mask_ps(in: FowBaseVertexOut) -> @location(0) u32 {
    let m = textureSample(FOWBaseMask, FOWBaseMask_smp, in.mask_uv);
    if !((m.y + -0.01) >= 0.0) {
        discard;
    }
    return MASK_CLASS;
}
