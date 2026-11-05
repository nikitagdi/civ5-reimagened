//! Pixel-shader wrapper: a full-screen triangle over a 64x64 grid. Every
//! pixel reads its inputs from `harness_in` (slot `s` of pixel `p` is at `s * 4096 + p`), calls
//! the body and writes the outputs to `harness_out` the same way.
//!
//! The spec reads the inputs from `Rgba32Float` textures and writes the outputs to render
//! targets; storage buffers carry the same `f32` bits without a format in between and without
//! the four-targets limit.

use std::fmt::Write;

use super::subject::{Field, Subject};
use super::{GRID, HarnessError, InputPlan, PIXELS, feed_expr};

/// The WGSL text appended to a pixel-shader module.
pub(super) fn wrapper(
    s: &Subject,
    plan: &InputPlan<'_>,
    outs: &[(&Field, usize)],
) -> Result<String, HarnessError> {
    let mut t = String::new();
    t.push_str(
        "@vertex\nfn harness_vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {\n\
         \x20   let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));\n\
         \x20   return vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);\n}\n",
    );
    t.push_str("@fragment\nfn harness_fs(@builtin(position) pos: vec4<f32>) {\n");
    let _ = writeln!(t, "    let idx = u32(pos.y) * {GRID}u + u32(pos.x);");
    let _ = writeln!(t, "    var b: {};", s.in_type);
    for (f, feed) in &plan.feeds {
        let _ = writeln!(t, "    b.{} = {};", f.name, feed_expr(f, feed, "idx"));
    }
    let _ = writeln!(t, "    let r = {}(b);", s.body);
    for (f, k) in outs {
        let _ = writeln!(
            t,
            "    harness_out[{k}u * {PIXELS}u + idx] = {};",
            f.to_vec4(&format!("r.{}", f.name))?
        );
    }
    // The outputs are stored first: a killed pixel still shows what the body computed.
    if outs.iter().any(|(f, _)| f.name == "kill") {
        t.push_str("    if (r.kill) { discard; }\n");
    }
    t.push_str("}\n");
    Ok(t)
}
