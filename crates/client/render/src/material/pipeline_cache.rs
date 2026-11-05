//! The cache of render pipelines. A pipeline is made at the first use of
//! its key; a failure is logged once and the call is skipped: no panic, no stand-in.
//!
//! **Key:** (family, entries, `override` values, kind of pass, target format, depth format,
//! number of samples, layout of the vertices, layout of the instances, blend and depth state).
//! The bind group layouts are not in the key: a family has one set of them.

use std::collections::HashMap;

use super::desc::{BlendMode, InstanceLayout, PassKind, RenderState, VertexLayout};
use super::families::{Selection, ShaderFamily, Skip, report_once};
use crate::shaders::catalog;

/// What a call asks the cache for.
pub struct PipelineRequest<'a> {
    pub family: &'a ShaderFamily,
    pub selection: &'a Selection,
    pub pass: PassKind,
    pub state: RenderState,
    /// Format of the colour target.
    pub target: wgpu::TextureFormat,
    /// Format of the depth target, if the pass has one.
    pub depth: Option<wgpu::TextureFormat>,
    /// Samples of the target (1 or 4).
    pub samples: u32,
    pub vertex: &'a VertexLayout,
    pub instance: Option<&'a InstanceLayout>,
    /// Groups 0.. of the family.
    pub layouts: &'a [&'a wgpu::BindGroupLayout],
}

/// The key of a pipeline.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PipelineKey {
    pub family: &'static str,
    pub vs: &'static str,
    pub fs: &'static str,
    pub variant: String,
    pub pass: PassKind,
    pub target: wgpu::TextureFormat,
    pub depth: Option<wgpu::TextureFormat>,
    pub samples: u32,
    pub vertex: VertexLayout,
    pub instance: Option<InstanceLayout>,
    pub state: RenderState,
}

impl PipelineKey {
    pub fn of(req: &PipelineRequest<'_>) -> Self {
        Self {
            family: req.family.name,
            vs: req.selection.vs,
            fs: req.selection.fs,
            variant: req.selection.variant.key(),
            pass: req.pass,
            target: req.target,
            depth: req.depth,
            samples: req.samples,
            vertex: req.vertex.clone(),
            instance: req.instance.cloned(),
            state: req.state,
        }
    }
}

/// How many pipelines were made, asked for again, and refused.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CacheStats {
    pub created: u32,
    pub hits: u32,
    pub failed: u32,
}

#[derive(Default)]
pub struct PipelineCache {
    modules: HashMap<&'static str, wgpu::ShaderModule>,
    pipelines: HashMap<PipelineKey, Result<wgpu::RenderPipeline, Skip>>,
    stats: CacheStats,
    /// The fragment of the class mask (`common/mask.wgsl`), made with the first mask pipeline.
    mask: Option<crate::frame::mask::MaskShader>,
}

impl PipelineCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn stats(&self) -> CacheStats {
        self.stats
    }

    /// The pipelines in the cache, failures not counted.
    pub fn len(&self) -> usize {
        self.pipelines.values().filter(|p| p.is_ok()).count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The pipeline for `req`, made if it is new. `Err` means the call is not drawn.
    pub fn get(
        &mut self,
        device: &wgpu::Device,
        req: &PipelineRequest<'_>,
    ) -> Result<wgpu::RenderPipeline, Skip> {
        let key = PipelineKey::of(req);
        if let Some(found) = self.pipelines.get(&key) {
            self.stats.hits += 1;
            return found.clone();
        }
        let made = self.make(device, req);
        match &made {
            Ok(_) => self.stats.created += 1,
            Err(why) => {
                self.stats.failed += 1;
                report_once(&format!("{key:?}"), why);
            }
        }
        self.pipelines.insert(key, made.clone());
        made
    }

    /// The class mask pipeline of a call: the vertex stage and the depth
    /// state of `req` (a main-pass request), the fragment `common/mask.wgsl` writing `class` into
    /// the `R8Uint` mask target, one sample, the mask's own depth. The colour target, the samples
    /// and the blend of `req` are not used.
    pub fn get_mask(
        &mut self,
        device: &wgpu::Device,
        req: &PipelineRequest<'_>,
        class: u32,
    ) -> Result<wgpu::RenderPipeline, Skip> {
        use crate::frame::mask::MASK_FORMAT;
        let mut key = PipelineKey::of(req);
        key.pass = PassKind::Mask;
        key.fs = "fs_mask";
        key.target = MASK_FORMAT;
        key.depth = Some(crate::frame::DEPTH_FORMAT);
        key.samples = 1;
        key.variant = format!("{} class {class}", key.variant);
        if let Some(found) = self.pipelines.get(&key) {
            self.stats.hits += 1;
            return found.clone();
        }
        let made = self.make_mask(device, req, class);
        match &made {
            Ok(_) => self.stats.created += 1,
            Err(why) => {
                self.stats.failed += 1;
                report_once(&format!("{key:?}"), why);
            }
        }
        self.pipelines.insert(key, made.clone());
        made
    }

    fn make_mask(
        &mut self,
        device: &wgpu::Device,
        req: &PipelineRequest<'_>,
        class: u32,
    ) -> Result<wgpu::RenderPipeline, Skip> {
        let (module, source) = self.module(device, req.family)?;
        if !source.contains(&format!("fn {}(", req.selection.vs)) {
            return Err(Skip::MissingEntry {
                module: req.family.module,
                entry: req.selection.vs,
            });
        }
        req.selection
            .variant
            .check(&source)
            .map_err(|e| Skip::Pipeline(e.to_string()))?;
        let vertex_attrs = req.vertex.wgpu_attributes();
        let instance_attrs = match req.instance {
            Some(i) => i
                .attributes(req.vertex)
                .map_err(|e| Skip::Layout(e.to_string()))?,
            None => Vec::new(),
        };
        let mut buffers = Vec::new();
        if !req.vertex.is_empty() {
            buffers.push(Some(wgpu::VertexBufferLayout {
                array_stride: req.vertex.stride(),
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &vertex_attrs,
            }));
        }
        if let Some(i) = req.instance {
            buffers.push(Some(wgpu::VertexBufferLayout {
                array_stride: i.stride(),
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &instance_attrs,
            }));
        }
        let shader = self
            .mask
            .get_or_insert_with(|| crate::frame::mask::MaskShader::new(device));
        let constants = req.selection.variant.constants();
        let layouts: Vec<Option<&wgpu::BindGroupLayout>> =
            req.layouts.iter().map(|l| Some(*l)).collect();
        let desc = crate::frame::mask::MaskPipelineDesc {
            label: req.family.name,
            vertex: &module,
            vertex_entry: req.selection.vs,
            vertex_buffers: &buffers,
            bind_group_layouts: &layouts,
            cull: req.state.cull,
            topology: wgpu::PrimitiveTopology::TriangleList,
            class,
        };
        let depth = crate::frame::mask::MaskDepth {
            compare: req.state.depth_compare,
            write: req.state.depth_write,
        };
        guarded(device, || {
            shader.pipeline_with_constants(device, &desc, depth, &constants)
        })
        .map_err(Skip::Pipeline)
    }

    fn module(
        &mut self,
        device: &wgpu::Device,
        family: &ShaderFamily,
    ) -> Result<(wgpu::ShaderModule, String), Skip> {
        let Some(desc) = catalog().iter().find(|m| m.name == family.module) else {
            return Err(Skip::Pipeline(format!(
                "the catalog has no module `{}`",
                family.module
            )));
        };
        let source = desc.source();
        if let Some(m) = self.modules.get(family.module) {
            return Ok((m.clone(), source));
        }
        let module = guarded(device, || {
            crate::shaders::create_module(device, family.module, &source)
        })
        .map_err(Skip::Pipeline)?;
        self.modules.insert(family.module, module.clone());
        Ok((module, source))
    }

    fn make(
        &mut self,
        device: &wgpu::Device,
        req: &PipelineRequest<'_>,
    ) -> Result<wgpu::RenderPipeline, Skip> {
        let (module, source) = self.module(device, req.family)?;
        for entry in [req.selection.vs, req.selection.fs] {
            if !source.contains(&format!("fn {entry}(")) {
                return Err(Skip::MissingEntry {
                    module: req.family.module,
                    entry,
                });
            }
        }
        req.selection
            .variant
            .check(&source)
            .map_err(|e| Skip::Pipeline(e.to_string()))?;
        if !matches!(req.samples, 1 | 2 | 4 | 8) {
            return Err(Skip::Pipeline(format!("{} samples", req.samples)));
        }
        let vertex_attrs = req.vertex.wgpu_attributes();
        let instance_attrs = match req.instance {
            Some(i) => i
                .attributes(req.vertex)
                .map_err(|e| Skip::Layout(e.to_string()))?,
            None => Vec::new(),
        };
        let mut buffers = Vec::new();
        if !req.vertex.is_empty() {
            buffers.push(Some(wgpu::VertexBufferLayout {
                array_stride: req.vertex.stride(),
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &vertex_attrs,
            }));
        }
        if let Some(i) = req.instance {
            buffers.push(Some(wgpu::VertexBufferLayout {
                array_stride: i.stride(),
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &instance_attrs,
            }));
        }
        let constants = req.selection.variant.constants();
        let layouts: Vec<Option<&wgpu::BindGroupLayout>> =
            req.layouts.iter().map(|l| Some(*l)).collect();
        guarded(device, || {
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(req.family.name),
                bind_group_layouts: &layouts,
                immediate_size: 0,
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(req.family.name),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some(req.selection.vs),
                    compilation_options: wgpu::PipelineCompilationOptions {
                        constants: &constants,
                        ..Default::default()
                    },
                    buffers: &buffers,
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: req.state.cull,
                    ..Default::default()
                },
                depth_stencil: req.depth.map(|format| wgpu::DepthStencilState {
                    format,
                    depth_write_enabled: Some(req.state.depth_write),
                    depth_compare: Some(req.state.depth_compare),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: wgpu::MultisampleState {
                    count: req.samples,
                    mask: !0,
                    alpha_to_coverage_enabled: false,
                },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(req.selection.fs),
                    compilation_options: wgpu::PipelineCompilationOptions {
                        constants: &constants,
                        ..Default::default()
                    },
                    targets: &[Some(wgpu::ColorTargetState {
                        format: req.target,
                        blend: match req.state.blend {
                            BlendMode::Opaque => None,
                            BlendMode::Alpha => Some(wgpu::BlendState::ALPHA_BLENDING),
                            BlendMode::FxPremultiplied => Some(fx_premultiplied_blend()),
                            BlendMode::GodRay => Some(god_ray_blend()),
                            BlendMode::UnitCode => Some(unit_code_blend()),
                        },
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        })
        .map_err(Skip::Pipeline)
    }
}

/// Runs `make` in a validation error scope and gives the error as text instead of the device's
/// handler (which logs it and goes on with an invalid object). Natively the scope answers at
/// once; on the web the answer is a promise that cannot be waited for, so there the checks above
/// are what stops a bad pipeline and the handler logs the rest.
#[cfg(not(target_arch = "wasm32"))]
fn guarded<T>(device: &wgpu::Device, make: impl FnOnce() -> T) -> Result<T, String> {
    use std::future::Future;
    use std::task::{Context, Poll, Waker};
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let made = make();
    let mut answer = std::pin::pin!(scope.pop());
    let mut cx = Context::from_waker(Waker::noop());
    match answer.as_mut().poll(&mut cx) {
        Poll::Ready(Some(e)) => Err(e.to_string()),
        Poll::Ready(None) | Poll::Pending => Ok(made),
    }
}

#[cfg(target_arch = "wasm32")]
fn guarded<T>(_device: &wgpu::Device, make: impl FnOnce() -> T) -> Result<T, String> {
    Ok(make())
}

/// `BlendMode::FxPremultiplied`: `rgb + target * a`, in colour and in alpha.
fn fx_premultiplied_blend() -> wgpu::BlendState {
    let component = wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::SrcAlpha,
        operation: wgpu::BlendOperation::Add,
    };
    wgpu::BlendState {
        color: component,
        alpha: component,
    }
}

// SIMPLIFIED(SIMP-exe-1033): the shader sqrt goes into the Rgba8Unorm gamma-space target unchanged.
/// `BlendMode::GodRay`: colour `src + dst * a`, alpha `dst * a` (the exe's `ZERO`, `SRCALPHA` alpha).
pub(crate) fn god_ray_blend() -> wgpu::BlendState {
    wgpu::BlendState {
        color: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::SrcAlpha,
            operation: wgpu::BlendOperation::Add,
        },
        alpha: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::Zero,
            dst_factor: wgpu::BlendFactor::SrcAlpha,
            operation: wgpu::BlendOperation::Add,
        },
    }
}

/// `BlendMode::UnitCode`: `rgb * a + target * (1 - a)`, alpha `a + target`.
fn unit_code_blend() -> wgpu::BlendState {
    wgpu::BlendState {
        color: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::SrcAlpha,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        },
        alpha: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu_test::{Gpu, gpu};
    use crate::material::MaterialDesc;
    use crate::material::families::{FAMILIES, resolve_in};
    use crate::shaders::Variant;

    fn probe_family() -> ShaderFamily {
        fn select(d: &MaterialDesc, pass: PassKind) -> Option<Selection> {
            (pass == PassKind::Main).then(|| Selection {
                vs: "vs_fullscreen",
                fs: "fs_override",
                variant: Variant::new().with_flag("PROBE_ON", d.has(MaterialDesc::SKINNED)),
            })
        }
        ShaderFamily {
            name: "probe",
            packages: &["ProbeShader"],
            module: "common_probe",
            select,
        }
    }

    fn request<'a>(
        family: &'a ShaderFamily,
        selection: &'a Selection,
        vertex: &'a VertexLayout,
        samples: u32,
    ) -> PipelineRequest<'a> {
        PipelineRequest {
            family,
            selection,
            pass: PassKind::Main,
            state: RenderState::OPAQUE,
            target: wgpu::TextureFormat::Rgba8Unorm,
            depth: Some(wgpu::TextureFormat::Depth24PlusStencil8),
            samples,
            vertex,
            instance: None,
            layouts: &[],
        }
    }

    fn desc(flags: u16) -> MaterialDesc {
        MaterialDesc {
            package: "ProbeShader".into(),
            flags,
            ..MaterialDesc::default()
        }
    }

    #[test]
    fn the_key_tells_the_sample_counts_the_variants_and_the_passes_apart() {
        let family = probe_family();
        let table = [family];
        let (f, s) = resolve_in(&table, &desc(0), PassKind::Main).unwrap();
        let v = VertexLayout::empty();
        let key = |samples| PipelineKey::of(&request(f, &s, &v, samples));
        assert_ne!(key(1), key(4), "the number of samples is in the key");
        assert_eq!(key(4), key(4));
        // Another `override` value is another pipeline.
        let (_, skinned) =
            resolve_in(&table, &desc(MaterialDesc::SKINNED), PassKind::Main).unwrap();
        assert_ne!(
            PipelineKey::of(&request(f, &s, &v, 4)),
            PipelineKey::of(&request(f, &skinned, &v, 4))
        );
        // So are the pass kind, the target and the state.
        let base = request(f, &s, &v, 4);
        let mut zfail = request(f, &s, &v, 4);
        zfail.pass = PassKind::ZFail;
        zfail.state = RenderState::of_material(&desc(0), PassKind::ZFail);
        assert_ne!(PipelineKey::of(&base), PipelineKey::of(&zfail));
        let mut other_target = request(f, &s, &v, 4);
        other_target.target = wgpu::TextureFormat::R8Uint;
        assert_ne!(PipelineKey::of(&base), PipelineKey::of(&other_target));
    }

    #[test]
    fn a_pipeline_is_made_once_per_key_and_a_sample_count_makes_another() {
        let Some(gpu) = gpu(
            "a_pipeline_is_made_once_per_key_and_a_sample_count_makes_another",
            wgpu::Backends::PRIMARY,
            wgpu::Features::empty(),
        ) else {
            return;
        };
        let family = probe_family();
        let table = [family];
        let (f, s) = resolve_in(&table, &desc(0), PassKind::Main).unwrap();
        let v = VertexLayout::empty();
        let mut cache = PipelineCache::new();
        let get = |cache: &mut PipelineCache, samples| {
            cache.get(&gpu.device, &request(f, &s, &v, samples))
        };
        get(&mut cache, 4).expect("4 samples");
        get(&mut cache, 4).expect("again");
        get(&mut cache, 1).expect("1 sample");
        assert_eq!(
            cache.stats(),
            CacheStats {
                created: 2,
                hits: 1,
                failed: 0
            }
        );
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn a_pipeline_that_cannot_be_made_is_skipped_once_without_a_panic() {
        let Some(gpu) = gpu(
            "a_pipeline_that_cannot_be_made_is_skipped_once_without_a_panic",
            wgpu::Backends::PRIMARY,
            wgpu::Features::empty(),
        ) else {
            return;
        };
        let family = probe_family();
        let v = VertexLayout::empty();
        let mut cache = PipelineCache::new();
        // An entry the module does not have.
        let missing = Selection {
            vs: "vs_nothing",
            fs: "fs_override",
            variant: Variant::new(),
        };
        let r = cache.get(&gpu.device, &request(&family, &missing, &v, 4));
        assert!(
            matches!(
                r,
                Err(Skip::MissingEntry {
                    entry: "vs_nothing",
                    ..
                })
            ),
            "{r:?}"
        );
        // An `override` the module does not declare.
        let typo = Selection {
            vs: "vs_fullscreen",
            fs: "fs_override",
            variant: Variant::new().with("NOT_THERE", 1.0),
        };
        assert!(matches!(
            cache.get(&gpu.device, &request(&family, &typo, &v, 4)),
            Err(Skip::Pipeline(_))
        ));
        // A pipeline the device refuses (3 samples): caught by the error scope, not a panic.
        let ok = Selection {
            vs: "vs_fullscreen",
            fs: "fs_override",
            variant: Variant::new(),
        };
        assert!(matches!(
            cache.get(&gpu.device, &request(&family, &ok, &v, 3)),
            Err(Skip::Pipeline(_))
        ));
        // Asking again is a hit, not a second attempt.
        let before = cache.stats();
        let _ = cache.get(&gpu.device, &request(&family, &missing, &v, 4));
        assert_eq!(cache.stats().hits, before.hits + 1);
        assert_eq!(cache.stats().created, 0);
        assert!(cache.is_empty());
    }

    /// The unknown package never reaches the cache: no pipeline, no stand-in.
    #[test]
    fn an_unknown_package_makes_no_pipeline() {
        let Some(gpu) = gpu(
            "an_unknown_package_makes_no_pipeline",
            wgpu::Backends::PRIMARY,
            wgpu::Features::empty(),
        ) else {
            return;
        };
        let cache = PipelineCache::new();
        let d = MaterialDesc {
            package: "NoSuchShader".into(),
            ..MaterialDesc::default()
        };
        let r = resolve_in(FAMILIES, &d, PassKind::Main);
        assert_eq!(r.unwrap_err(), Skip::UnknownPackage("NoSuchShader".into()));
        let _ = &gpu as &Gpu;
        assert!(cache.is_empty() && cache.stats().created == 0);
    }
}
