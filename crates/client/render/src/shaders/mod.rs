//! Shader modules of the renderer: the glue of WGSL files, the
//! variants through `override`, the table of vertex semantics, uniform blocks with CTAB names and
//! the catalog `shader-re` and `golden` read.
//!
//! WGSL has no `#include`, so a module is the text of its files joined in the declared order. The
//! join is made at compile time (`include_str!`), without build scripts.

pub mod catalog;
#[cfg(test)]
mod gpu_tests;
#[path = "common/semantics.rs"]
pub mod semantics;
pub mod uniform;
pub mod variant;

pub use catalog::{EntryDesc, ModuleDesc, Part, ReRef, Stage, catalog};
pub use semantics::{MAX_VERTEX_ATTRIBUTES, Semantic, free_texcoord_locations};
pub use uniform::{
    BLOCK_ALIGN, ConstantOverrides, FieldDesc, LayoutError, UniformBlock, WgslType, layout_error,
};
pub use variant::{UnknownOverride, Variant, declared_overrides};

/// A `&'static str` made of WGSL files under `shaders/`, joined in the order given, each followed
/// by a newline: `wgsl_module!("common/frame.wgsl", "common/sm3.wgsl", "unit/unit.wgsl")`.
#[macro_export]
macro_rules! wgsl_module {
    ($($path:literal),+ $(,)?) => {
        concat!($(
            include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/shaders/", $path)),
            "\n"
        ),+)
    };
}

/// Makes the module on `device`. A failure to compile is reported by the device's error handler
/// (the renderer logs it); callers then skip the draw instead of panicking.
pub fn create_module(device: &wgpu::Device, label: &str, source: &str) -> wgpu::ShaderModule {
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_macro_joins_the_files_as_the_catalog_does() {
        let by_macro = wgsl_module!("common/frame.wgsl", "common/sm3.wgsl");
        let module = catalog().iter().find(|m| m.name == "common_probe").unwrap();
        let by_catalog: String = module.parts[..2]
            .iter()
            .map(|p| format!("{}\n", p.text))
            .collect();
        assert_eq!(by_macro, by_catalog);
        assert!(by_macro.contains("struct ViewGlobals"));
        assert!(by_macro.contains("fn d3d_pow"));
    }

    #[test]
    fn every_module_names_its_entry_points_and_they_are_in_the_text() {
        assert!(!catalog().is_empty());
        for module in catalog() {
            let text = module.source();
            assert!(!module.entries.is_empty(), "{}", module.name);
            for e in module.entries {
                let stage = match e.stage {
                    Stage::Vertex => "@vertex",
                    Stage::Fragment => "@fragment",
                };
                let declared = text.contains(&format!("fn {}(", e.name));
                assert!(declared, "{}: no `fn {}`", module.name, e.name);
                assert!(text.contains(stage), "{}", module.name);
            }
            // Each ported function names the input it comes from.
            for part in module.parts {
                assert!(
                    !part.text.trim().is_empty(),
                    "{}: {} is empty",
                    module.name,
                    part.path
                );
            }
        }
    }

    /// The `fn` items of a WGSL text by name, comments and indentation dropped.
    fn functions(text: &str) -> std::collections::BTreeMap<String, String> {
        let mut out = std::collections::BTreeMap::new();
        let mut current: Option<(String, Vec<String>)> = None;
        for line in text.lines() {
            let t = line.trim();
            if t.starts_with("//") || t.is_empty() {
                continue;
            }
            if let Some(rest) = t.strip_prefix("fn ") {
                let name = rest.split('(').next().unwrap_or("").to_owned();
                current = Some((name, Vec::new()));
            }
            if let Some((_, lines)) = current.as_mut() {
                lines.push(t.to_owned());
            }
            if t == "}"
                && let Some((name, lines)) = current.take()
            {
                out.insert(name, lines.join("\n"));
            }
        }
        out
    }

    /// The `d3d_*` functions of `sm3.wgsl` are the text of `oracle::HELPERS` of `shader-re`
    /// (one source). `ORACLE_HELPERS_FILE` names another copy of
    /// `tools/shader-re/src/oracle/mod.rs`.
    #[test]
    fn sm3_wgsl_has_the_helpers_of_the_oracle() {
        let path = std::env::var("ORACLE_HELPERS_FILE").unwrap_or_else(|_| {
            format!(
                "{}/../../../tools/shader-re/src/oracle/mod.rs",
                env!("CARGO_MANIFEST_DIR")
            )
        });
        let source = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        let start = source
            .find("HELPERS: &str = r#\"")
            .expect("HELPERS in the oracle")
            + "HELPERS: &str = r#\"".len();
        let end = start + source[start..].find("\"#;").expect("end of HELPERS");
        let oracle = functions(&source[start..end]);
        let ours = functions(include_str!("common/sm3.wgsl"));
        assert!(oracle.len() >= 7, "{:?}", oracle.keys());
        for (name, body) in &oracle {
            assert_eq!(ours.get(name), Some(body), "{name} differs from the oracle");
        }
        assert_eq!(
            oracle.keys().collect::<Vec<_>>(),
            ours.keys().collect::<Vec<_>>()
        );
    }

    /// The shaders must pass the checks `shader-re validate` makes, in the sense of the parser and
    /// the validator (the limits of WebGL2 are checked there).
    #[test]
    fn every_module_parses_and_validates() {
        for module in catalog() {
            let text = module.source();
            let parsed = naga::front::wgsl::parse_str(&text)
                .unwrap_or_else(|e| panic!("{}: {}", module.name, e.emit_to_string(&text)));
            naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::empty(),
            )
            .validate(&parsed)
            .unwrap_or_else(|e| panic!("{}: {}", module.name, e.emit_to_string(&text)));
            let mut entries: Vec<&str> = parsed
                .entry_points
                .iter()
                .map(|e| e.name.as_str())
                .collect();
            let mut want: Vec<&str> = module.entries.iter().map(|e| e.name).collect();
            entries.sort_unstable();
            want.sort_unstable();
            assert_eq!(entries, want, "{}: entry points of the text", module.name);
        }
    }

    /// `frame.wgsl` is the layout of `ViewGlobals`.
    #[test]
    fn frame_wgsl_has_the_size_of_view_globals() {
        let text = wgsl_module!("common/frame.wgsl");
        let module = naga::front::wgsl::parse_str(text).unwrap();
        let (_, ty) = module
            .types
            .iter()
            .find(|(_, t)| t.name.as_deref() == Some("ViewGlobals"))
            .expect("ViewGlobals");
        let naga::TypeInner::Struct { span, .. } = &ty.inner else {
            panic!("a struct");
        };
        assert_eq!(
            *span as usize,
            std::mem::size_of::<crate::frame::ViewGlobals>()
        );
    }

    /// `frame.wgsl` also holds `FrameGlobals`, field for field the uniform block.
    #[test]
    fn frame_wgsl_has_the_layout_of_frame_globals() {
        use crate::shaders::UniformBlock;
        let text = wgsl_module!("common/frame.wgsl");
        let module = naga::front::wgsl::parse_str(text).unwrap();
        let (_, ty) = module
            .types
            .iter()
            .find(|(_, t)| t.name.as_deref() == Some("FrameGlobals"))
            .expect("FrameGlobals");
        let naga::TypeInner::Struct { members, span } = &ty.inner else {
            panic!("a struct");
        };
        assert_eq!(
            *span as usize,
            std::mem::size_of::<crate::lighting::FrameGlobals>()
        );
        let wgsl: Vec<(&str, u32)> = members
            .iter()
            .map(|m| (m.name.as_deref().unwrap(), m.offset))
            .collect();
        let rust: Vec<(&str, u32)> = crate::lighting::FrameGlobals::FIELDS
            .iter()
            .map(|f| (f.name, f.offset as u32))
            .collect();
        assert_eq!(wgsl, rust);
    }
}
