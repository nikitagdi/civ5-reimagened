//! WGSL check through naga: parse, validate, translate every entry point to
//! GLSL ES 3.00 (WebGL2), and check the `downlevel_webgl2_defaults` limits.
//!
//! Limits (wgpu-types 30, `Limits::downlevel_webgl2_defaults`):
//! bind groups 4, sampled textures 16 and samplers 16 per stage, uniform buffers 11 per stage,
//! uniform binding 16 KiB, vertex attributes 16, color outputs 4, inter-stage variables 15,
//! no storage buffers, no storage textures, no compute. Immediates (push constants) do not exist
//! on WebGL2 either.

use std::fmt;

use naga::back::glsl;
use naga::valid::{Capabilities, ValidationFlags, Validator};
use naga::{AddressSpace, Binding, ImageClass, Module, ShaderStage, TypeInner};

/// `max_bind_groups` of the downlevel defaults.
pub const MAX_BIND_GROUPS: u32 = 4;
/// `max_sampled_textures_per_shader_stage`.
pub const MAX_TEXTURES: u32 = 16;
/// `max_samplers_per_shader_stage`.
pub const MAX_SAMPLERS: u32 = 16;
/// `max_uniform_buffers_per_shader_stage` of `downlevel_webgl2_defaults`.
pub const MAX_UNIFORM_BUFFERS: u32 = 11;
/// `max_uniform_buffer_binding_size`: 16 KiB.
pub const MAX_UNIFORM_BYTES: u32 = 16 << 10;
/// `max_vertex_attributes`.
pub const MAX_VERTEX_ATTRIBUTES: u32 = 16;
/// `max_color_attachments`.
pub const MAX_COLOR_OUTPUTS: u32 = 4;
/// `max_inter_stage_shader_variables` of `downlevel_webgl2_defaults`.
pub const MAX_INTER_STAGE: u32 = 15;

/// GLSL ES 3.00 as WebGL2 consumes it.
const GLSL_VERSION: glsl::Version = glsl::Version::Embedded {
    version: 300,
    is_webgl: true,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Violation {
    /// Entry point, or `None` for a problem of the whole module.
    pub entry: Option<String>,
    pub what: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EntryUsage {
    pub name: String,
    pub stage: String,
    pub bind_groups: u32,
    pub textures: u32,
    pub samplers: u32,
    pub uniform_buffers: u32,
    pub max_uniform_bytes: u32,
    pub vertex_attributes: u32,
    pub color_outputs: u32,
    pub inter_stage: u32,
}

/// Result for one WGSL module.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub module: String,
    pub entries: Vec<EntryUsage>,
    pub violations: Vec<Violation>,
}

impl Report {
    pub fn ok(&self) -> bool {
        self.violations.is_empty()
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.ok() {
            writeln!(
                f,
                "{}: ok ({} entry points)",
                self.module,
                self.entries.len()
            )
        } else {
            writeln!(f, "{}: {} violation(s)", self.module, self.violations.len())?;
            for v in &self.violations {
                match &v.entry {
                    Some(e) => writeln!(f, "  [{e}] {}", v.what)?,
                    None => writeln!(f, "  {}", v.what)?,
                }
            }
            Ok(())
        }
    }
}

fn stage_name(s: ShaderStage) -> &'static str {
    match s {
        ShaderStage::Vertex => "vertex",
        ShaderStage::Fragment => "fragment",
        ShaderStage::Compute => "compute",
        _ => "other",
    }
}

/// Number of `@location` bindings in a stage interface (arguments or result): plain bindings and
/// the members of struct types.
fn count_locations(
    module: &Module,
    ty: naga::Handle<naga::Type>,
    binding: &Option<Binding>,
) -> u32 {
    if matches!(binding, Some(Binding::Location { .. })) {
        return 1;
    }
    match &module.types[ty].inner {
        TypeInner::Struct { members, .. } => members
            .iter()
            .filter(|m| matches!(m.binding, Some(Binding::Location { .. })))
            .count() as u32,
        _ => 0,
    }
}

fn entry_usage(
    module: &Module,
    info: &naga::valid::ModuleInfo,
    index: usize,
    out: &mut Vec<Violation>,
) -> EntryUsage {
    let ep = &module.entry_points[index];
    let fi = info.get_entry_point(index);
    let mut u = EntryUsage {
        name: ep.name.clone(),
        stage: stage_name(ep.stage).to_string(),
        ..Default::default()
    };
    let mut groups = 0u32;
    let mut bad = |what: String| {
        out.push(Violation {
            entry: Some(ep.name.clone()),
            what,
        })
    };
    if ep.stage == ShaderStage::Compute {
        bad("compute shaders do not exist on WebGL2".into());
    }
    for (handle, var) in module.global_variables.iter() {
        if fi[handle].is_empty() {
            continue;
        }
        let name = var.name.as_deref().unwrap_or("?");
        if let Some(b) = &var.binding {
            groups = groups.max(b.group + 1);
        }
        let inner = &module.types[var.ty].inner;
        match var.space {
            AddressSpace::Uniform => {
                u.uniform_buffers += 1;
                let size = inner.size(module.to_ctx());
                u.max_uniform_bytes = u.max_uniform_bytes.max(size);
                if size > MAX_UNIFORM_BYTES {
                    bad(format!(
                        "uniform buffer `{name}` is {size} bytes, WebGL2 allows {MAX_UNIFORM_BYTES}"
                    ));
                }
            }
            AddressSpace::Storage { .. } => {
                bad(format!("storage buffer `{name}`: WebGL2 has none"));
            }
            AddressSpace::Immediate => {
                bad(format!("immediate data `{name}`: not available on WebGL2"));
            }
            AddressSpace::Handle => {
                let base = match inner {
                    TypeInner::BindingArray { base, .. } => &module.types[*base].inner,
                    other => other,
                };
                match base {
                    TypeInner::Image {
                        class: ImageClass::Storage { .. },
                        ..
                    } => bad(format!("storage texture `{name}`: WebGL2 has none")),
                    TypeInner::Image { .. } => u.textures += 1,
                    TypeInner::Sampler { .. } => u.samplers += 1,
                    _ => {}
                }
            }
            _ => {}
        }
    }
    u.bind_groups = groups;
    if groups > MAX_BIND_GROUPS {
        bad(format!(
            "{groups} bind groups, WebGL2 allows {MAX_BIND_GROUPS}"
        ));
    }
    if u.textures > MAX_TEXTURES {
        bad(format!(
            "{} textures, WebGL2 allows {MAX_TEXTURES}",
            u.textures
        ));
    }
    if u.samplers > MAX_SAMPLERS {
        bad(format!(
            "{} samplers, WebGL2 allows {MAX_SAMPLERS}",
            u.samplers
        ));
    }
    if u.uniform_buffers > MAX_UNIFORM_BUFFERS {
        bad(format!(
            "{} uniform buffers, WebGL2 allows {MAX_UNIFORM_BUFFERS}",
            u.uniform_buffers
        ));
    }
    let f = &ep.function;
    match ep.stage {
        ShaderStage::Vertex => {
            u.vertex_attributes = f
                .arguments
                .iter()
                .map(|a| count_locations(module, a.ty, &a.binding))
                .sum();
            u.inter_stage = f
                .result
                .as_ref()
                .map_or(0, |r| count_locations(module, r.ty, &r.binding));
            if u.vertex_attributes > MAX_VERTEX_ATTRIBUTES {
                bad(format!(
                    "{} vertex attributes, WebGL2 allows {MAX_VERTEX_ATTRIBUTES}",
                    u.vertex_attributes
                ));
            }
        }
        ShaderStage::Fragment => {
            u.inter_stage = f
                .arguments
                .iter()
                .map(|a| count_locations(module, a.ty, &a.binding))
                .sum();
            u.color_outputs = f
                .result
                .as_ref()
                .map_or(0, |r| count_locations(module, r.ty, &r.binding));
            if u.color_outputs > MAX_COLOR_OUTPUTS {
                bad(format!(
                    "{} color outputs, WebGL2 allows {MAX_COLOR_OUTPUTS}",
                    u.color_outputs
                ));
            }
        }
        _ => {}
    }
    if u.inter_stage > MAX_INTER_STAGE {
        bad(format!(
            "{} inter-stage variables, WebGL2 allows {MAX_INTER_STAGE}",
            u.inter_stage
        ));
    }
    u
}

/// Translates one entry point to GLSL ES 3.00; the text is dropped, only success matters.
fn glsl_check(module: &Module, info: &naga::valid::ModuleInfo, index: usize) -> Result<(), String> {
    let ep = &module.entry_points[index];
    let (module, info) = if module.overrides.is_empty() {
        (
            std::borrow::Cow::Borrowed(module),
            std::borrow::Cow::Borrowed(info),
        )
    } else {
        // Overrides take their default values; a missing default is reported here.
        naga::back::pipeline_constants::process_overrides(
            module,
            info,
            Some((ep.stage, ep.name.as_str())),
            &Default::default(),
        )
        .map_err(|e| format!("override: {e}"))?
    };
    let options = glsl::Options {
        version: GLSL_VERSION,
        ..Default::default()
    };
    let pipeline = glsl::PipelineOptions {
        shader_stage: ep.stage,
        entry_point: ep.name.clone(),
        multiview: None,
    };
    let mut text = String::new();
    glsl::Writer::new(
        &mut text,
        &module,
        &info,
        &options,
        &pipeline,
        naga::proc::BoundsCheckPolicies::default(),
    )
    .and_then(|mut w| w.write().map(|_| ()))
    .map_err(|e| e.to_string())
}

/// Checks one WGSL module: `name` is only used in the report.
pub fn validate_wgsl(name: &str, source: &str) -> Report {
    let mut report = Report {
        module: name.to_string(),
        ..Default::default()
    };
    let module = match naga::front::wgsl::parse_str(source) {
        Ok(m) => m,
        Err(e) => {
            report.violations.push(Violation {
                entry: None,
                what: format!("wgsl parse: {}", e.emit_to_string(source)),
            });
            return report;
        }
    };
    let info = match Validator::new(ValidationFlags::all(), Capabilities::empty()).validate(&module)
    {
        Ok(i) => i,
        Err(e) => {
            report.violations.push(Violation {
                entry: None,
                what: format!("naga validation: {}", e.emit_to_string(source)),
            });
            return report;
        }
    };
    if module.entry_points.is_empty() {
        report.violations.push(Violation {
            entry: None,
            what: "no entry points".into(),
        });
    }
    for index in 0..module.entry_points.len() {
        let usage = entry_usage(&module, &info, index, &mut report.violations);
        if let Err(e) = glsl_check(&module, &info, index) {
            report.violations.push(Violation {
                entry: Some(usage.name.clone()),
                what: format!("glsl-out (GLSL ES 3.00): {e}"),
            });
        }
        report.entries.push(usage);
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"
struct Globals { mvp: mat4x4<f32>, tint: vec4<f32> }
@group(0) @binding(0) var<uniform> g: Globals;
@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var smp: sampler;

struct VOut { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> }

@vertex
fn vs_main(@location(0) p: vec3<f32>, @location(1) uv: vec2<f32>) -> VOut {
    var o: VOut;
    o.pos = g.mvp * vec4<f32>(p, 1.0);
    o.uv = uv;
    return o;
}

@fragment
fn fs_main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    return textureSample(tex, smp, uv) * g.tint;
}
"#;

    #[test]
    fn valid_module_passes_with_usage() {
        let r = validate_wgsl("good", GOOD);
        assert!(r.ok(), "{r}");
        assert_eq!(r.entries.len(), 2);
        let vs = r.entries.iter().find(|e| e.name == "vs_main").unwrap();
        assert_eq!(
            (vs.vertex_attributes, vs.inter_stage, vs.uniform_buffers),
            (2, 1, 1)
        );
        assert_eq!(vs.max_uniform_bytes, 80);
        let fs = r.entries.iter().find(|e| e.name == "fs_main").unwrap();
        assert_eq!(
            (fs.textures, fs.samplers, fs.color_outputs, fs.bind_groups),
            (1, 1, 1, 2)
        );
    }

    #[test]
    fn syntax_error_is_reported_not_panicked() {
        let r = validate_wgsl("bad", "fn main( {");
        assert!(!r.ok());
        assert!(r.violations[0].what.starts_with("wgsl parse"), "{r}");
    }

    #[test]
    fn type_error_is_a_validation_violation() {
        let src = "@fragment fn fs() -> @location(0) vec4<f32> { return 1.0; }";
        let r = validate_wgsl("typed", src);
        assert!(!r.ok());
        assert!(
            r.violations[0].what.contains("wgsl parse")
                || r.violations[0].what.contains("naga validation")
        );
    }

    #[test]
    fn too_many_textures_are_reported() {
        let mut src = String::new();
        let mut body = String::from("var c = vec4<f32>(0.0);\n");
        for i in 0..17 {
            src += &format!("@group(0) @binding({i}) var t{i}: texture_2d<f32>;\n");
            body += &format!("c += textureLoad(t{i}, vec2<i32>(0), 0);\n");
        }
        src += &format!("@fragment fn fs() -> @location(0) vec4<f32> {{ {body} return c; }}");
        let r = validate_wgsl("tex17", &src);
        assert!(!r.ok());
        assert!(
            r.violations.iter().any(|v| v.what.contains("17 textures")),
            "{r}"
        );
        assert_eq!(r.entries[0].textures, 17);
    }

    #[test]
    fn storage_buffer_and_big_uniform_are_reported() {
        let src = r#"
@group(0) @binding(0) var<storage, read> s: array<vec4<f32>>;
struct Big { v: array<vec4<f32>, 1100> }
@group(0) @binding(1) var<uniform> big: Big;
@fragment fn fs() -> @location(0) vec4<f32> { return s[0] + big.v[1099]; }
"#;
        let r = validate_wgsl("limits", src);
        assert!(
            r.violations
                .iter()
                .any(|v| v.what.contains("storage buffer")),
            "{r}"
        );
        assert!(
            r.violations.iter().any(|v| v.what.contains("17600 bytes")),
            "{r}"
        );
    }

    #[test]
    fn five_color_outputs_and_compute_are_reported() {
        let src = r#"
struct Out {
  @location(0) a: vec4<f32>, @location(1) b: vec4<f32>, @location(2) c: vec4<f32>,
  @location(3) d: vec4<f32>, @location(4) e: vec4<f32>,
}
@fragment fn fs() -> Out { return Out(vec4<f32>(0.0), vec4<f32>(0.0), vec4<f32>(0.0), vec4<f32>(0.0), vec4<f32>(0.0)); }
@compute @workgroup_size(1) fn cs() {}
"#;
        let r = validate_wgsl("mrt5", src);
        assert!(
            r.violations
                .iter()
                .any(|v| v.what.contains("5 color outputs")),
            "{r}"
        );
        assert!(
            r.violations
                .iter()
                .any(|v| v.what.contains("compute shaders")),
            "{r}"
        );
    }

    #[test]
    fn too_many_attributes_and_varyings_are_reported() {
        let args: Vec<String> = (0..17)
            .map(|i| format!("@location({i}) a{i}: vec4<f32>"))
            .collect();
        let sum: Vec<String> = (0..17).map(|i| format!("a{i}")).collect();
        let src = format!(
            "@vertex fn vs({}) -> @builtin(position) vec4<f32> {{ return {}; }}",
            args.join(", "),
            sum.join(" + ")
        );
        let r = validate_wgsl("attrs17", &src);
        assert!(
            r.violations
                .iter()
                .any(|v| v.what.contains("17 vertex attributes")),
            "{r}"
        );
        let members: Vec<String> = (0..16)
            .map(|i| format!("@location({i}) v{i}: vec4<f32>"))
            .collect();
        let src = format!(
            "struct I {{ {} }}\n@fragment fn fs(i: I) -> @location(0) vec4<f32> {{ return i.v0 + i.v15; }}",
            members.join(", ")
        );
        let r = validate_wgsl("vary16", &src);
        assert!(
            r.violations
                .iter()
                .any(|v| v.what.contains("16 inter-stage")),
            "{r}"
        );
    }

    #[test]
    fn overrides_get_their_defaults_for_glsl() {
        let src = r#"
override k: f32 = 2.0;
@fragment fn fs() -> @location(0) vec4<f32> { return vec4<f32>(k); }
"#;
        let r = validate_wgsl("override", src);
        assert!(r.ok(), "{r}");
    }
}
