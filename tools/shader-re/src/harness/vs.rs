//! Vertex-shader wrapper: one point per pixel of a 64x64 grid. Vertex `i`
//! reads its attributes from `harness_in` (slot `s` at `s * 4096 + i`), calls the body and
//! passes every output on as a `flat` varying; the fragment stage of the same pixel stores them
//! in `harness_out`. The position the body computes is an output like the others: the point is
//! drawn where the wrapper puts it.
//!
//! The spec passes four outputs per pass through render targets; the device's inter-stage
//! variables take them all at once (the caller checks the limit).

use std::fmt::Write;

use super::subject::{Field, Subject};
use super::{GRID, HarnessError, InputPlan, PIXELS, feed_expr};

pub(super) fn wrapper(
    s: &Subject,
    plan: &InputPlan<'_>,
    outs: &[(&Field, usize)],
) -> Result<String, HarnessError> {
    let mut t = String::new();
    t.push_str("struct HarnessVsOut {\n    @builtin(position) pos: vec4<f32>,\n");
    for (_, k) in outs {
        let _ = writeln!(t, "    @location({k}) @interpolate(flat) o{k}: vec4<f32>,");
    }
    t.push_str("}\n");
    t.push_str("@vertex\nfn harness_vs(@builtin(vertex_index) i: u32) -> HarnessVsOut {\n");
    let _ = writeln!(t, "    var b: {};", s.in_type);
    for (f, feed) in &plan.feeds {
        let _ = writeln!(t, "    b.{} = {};", f.name, feed_expr(f, feed, "i"));
    }
    let _ = writeln!(t, "    let r = {}(b);", s.body);
    t.push_str("    var o: HarnessVsOut;\n");
    let _ = writeln!(
        t,
        "    let px = vec2<f32>(f32(i % {GRID}u), f32(i / {GRID}u));\n\
         \x20   o.pos = vec4<f32>((px.x + 0.5) / {half} - 1.0, 1.0 - (px.y + 0.5) / {half}, 0.0, 1.0);",
        half = GRID as f32 / 2.0
    );
    for (f, k) in outs {
        let _ = writeln!(t, "    o.o{k} = {};", f.to_vec4(&format!("r.{}", f.name))?);
    }
    t.push_str("    return o;\n}\n");
    t.push_str("@fragment\nfn harness_fs(v: HarnessVsOut) {\n");
    let _ = writeln!(t, "    let idx = u32(v.pos.y) * {GRID}u + u32(v.pos.x);");
    for (_, k) in outs {
        let _ = writeln!(t, "    harness_out[{k}u * {PIXELS}u + idx] = v.o{k};");
    }
    t.push_str("}\n");
    Ok(t)
}
