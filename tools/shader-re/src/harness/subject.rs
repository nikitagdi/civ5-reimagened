//! A module under test: what the harness reads out of its WGSL, and how it
//! fills its constant buffers from the SM3 registers by CTAB name.
//!
//! **The body contract** (render-core 4.7: "the entry only reads inputs and calls the body"). A
//! module under test, the oracle or a hand-written one, has a function
//!
//! ```text
//! fn <entry>_body(inp: In) -> Out
//! ```
//!
//! with one struct argument and a struct result. The members are named after the original's
//! registers:
//!
//! | stage | `In` members | `Out` members |
//! |---|---|---|
//! | pixel | `v<N>` (`f32`, `vec2..4<f32>`), `vpos` (`vec2` or `vec4<f32>`), `vface` (`f32`) | `oc<N>`, `odepth` (`f32`), `kill` (`bool`) |
//! | vertex | `v<N>` | `o<N>` (the position is the `o<N>` the original declared as `dcl_position`) |
//!
//! A hand module may leave out inputs it does not read and narrow the types; it may not leave
//! out an output the oracle has. Two kinds of extra `In` member exist, both filled from the
//! status entry's `data_moves`: `instance_<attr>` (a constant the original took from its
//! constants and we take from the instance, `to = "instance:<attr>"`) and the bone-palette
//! members `bone_base`, `bone_base_next`, `bone_blend` (set to 0, `to = "bone_texture"`).
//!
//! Resources are module-level globals named like the CTAB constants: a sampler `S` is a texture
//! `S` and a sampler `S_smp` (or `S_sampler`); constants are members of a `var<uniform>` struct
//! named like the CTAB constant, or the uniform variables themselves.

use naga::{ArraySize, Handle, Module, ScalarKind, Type, TypeInner};

use super::HarnessError;
use super::random::Registers;
use crate::oracle::{ConstBlock, Oracle};
use crate::scan::Stage;
use crate::sm3::ctab::{Ctab, CtabConstant, ParamClass, RegisterSet};

/// Shape of a body member.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    Scalar,
    Vector(u32),
    Matrix { cols: u32, rows: u32 },
}

/// A member of the `In` or `Out` struct.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field {
    pub name: String,
    pub kind: ScalarKind,
    pub shape: Shape,
}

impl Field {
    /// Number of `vec4` slots the input takes: one per matrix column, else one.
    pub fn slots(&self) -> usize {
        match self.shape {
            Shape::Matrix { cols, .. } => cols as usize,
            _ => 1,
        }
    }

    /// Components compared (a matrix is not an output).
    pub fn comps(&self) -> usize {
        match self.shape {
            Shape::Scalar => 1,
            Shape::Vector(n) => n as usize,
            Shape::Matrix { cols, rows } => (cols * rows) as usize,
        }
    }

    fn scalar_name(&self) -> &'static str {
        match self.kind {
            ScalarKind::Sint => "i32",
            ScalarKind::Uint => "u32",
            ScalarKind::Bool => "bool",
            _ => "f32",
        }
    }

    /// WGSL text of the member type.
    pub fn wgsl_type(&self) -> String {
        let s = self.scalar_name();
        match self.shape {
            Shape::Scalar => s.to_string(),
            Shape::Vector(n) => format!("vec{n}<{s}>"),
            Shape::Matrix { cols, rows } => format!("mat{cols}x{rows}<{s}>"),
        }
    }

    /// The member built from `vec4<f32>` slots (`slot(k)` is the text of slot `k`).
    pub fn from_slots(&self, slot: &dyn Fn(usize) -> String) -> String {
        let comp = |k: usize, n: u32| {
            let sw = &"xyzw"[..n as usize];
            format!("{}.{sw}", slot(k))
        };
        match (self.kind, self.shape) {
            (ScalarKind::Float, Shape::Scalar) => comp(0, 1),
            (ScalarKind::Float, Shape::Vector(n)) => comp(0, n),
            (ScalarKind::Float, Shape::Matrix { cols, rows }) => {
                let cs: Vec<String> = (0..cols as usize).map(|k| comp(k, rows)).collect();
                format!("mat{cols}x{rows}<f32>({})", cs.join(", "))
            }
            (ScalarKind::Bool, _) => format!("({} != 0.0)", comp(0, 1)),
            (_, Shape::Scalar) => format!("{}({})", self.scalar_name(), comp(0, 1)),
            (_, Shape::Vector(n)) => format!("{}({})", self.wgsl_type(), comp(0, n)),
            (_, Shape::Matrix { .. }) => unreachable!("integer matrices do not exist in WGSL"),
        }
    }

    /// The value `v` of this member as a `vec4<f32>` (outputs).
    pub fn to_vec4(&self, v: &str) -> Result<String, HarnessError> {
        Ok(match (self.kind, self.shape) {
            (ScalarKind::Float, Shape::Scalar) => format!("vec4<f32>({v}, 0.0, 0.0, 0.0)"),
            (ScalarKind::Float, Shape::Vector(2)) => format!("vec4<f32>({v}, 0.0, 0.0)"),
            (ScalarKind::Float, Shape::Vector(3)) => format!("vec4<f32>({v}, 0.0)"),
            (ScalarKind::Float, Shape::Vector(_)) => v.to_string(),
            (ScalarKind::Bool, Shape::Scalar) => {
                format!("vec4<f32>(select(0.0, 1.0, {v}), 0.0, 0.0, 0.0)")
            }
            (ScalarKind::Sint | ScalarKind::Uint, Shape::Scalar) => {
                format!("vec4<f32>(f32({v}), 0.0, 0.0, 0.0)")
            }
            _ => {
                return Err(HarnessError::Contract(format!(
                    "output `{}` has the type {}, which the harness cannot read back",
                    self.name,
                    self.wgsl_type()
                )));
            }
        })
    }
}

/// Where the constants of a subject come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConstSource {
    /// The oracle's `OracleConsts` block (SR-D5): registers as they are.
    Oracle(ConstBlock),
    /// Members named like the CTAB constants (hand-written modules).
    ByCtab,
}

/// A module the harness can run: the oracle or a hand-written one.
#[derive(Clone, Debug)]
pub struct Subject {
    pub label: String,
    pub wgsl: String,
    pub stage: Stage,
    /// Name of the body function.
    pub body: String,
    /// Name of its `In` struct type.
    pub in_type: String,
    pub consts: ConstSource,
    pub input: Vec<Field>,
    pub output: Vec<Field>,
}

fn kind_ok(k: ScalarKind) -> bool {
    matches!(
        k,
        ScalarKind::Float | ScalarKind::Sint | ScalarKind::Uint | ScalarKind::Bool
    )
}

fn field_of(module: &Module, name: &str, ty: Handle<Type>) -> Result<Field, HarnessError> {
    let bad = |why: &str| HarnessError::Contract(format!("member `{name}`: {why}"));
    Ok(match &module.types[ty].inner {
        TypeInner::Scalar(s) if kind_ok(s.kind) => Field {
            name: name.to_string(),
            kind: s.kind,
            shape: Shape::Scalar,
        },
        TypeInner::Vector { size, scalar } if kind_ok(scalar.kind) => Field {
            name: name.to_string(),
            kind: scalar.kind,
            shape: Shape::Vector(*size as u32),
        },
        TypeInner::Matrix {
            columns,
            rows,
            scalar,
        } if scalar.kind == ScalarKind::Float => Field {
            name: name.to_string(),
            kind: scalar.kind,
            shape: Shape::Matrix {
                cols: *columns as u32,
                rows: *rows as u32,
            },
        },
        _ => return Err(bad("type is not a scalar, a vector or a float matrix")),
    })
}

fn struct_fields(
    module: &Module,
    what: &str,
    ty: Handle<Type>,
) -> Result<(String, Vec<Field>), HarnessError> {
    let t = &module.types[ty];
    let TypeInner::Struct { members, .. } = &t.inner else {
        return Err(HarnessError::Contract(format!("{what} is not a struct")));
    };
    let name = t
        .name
        .clone()
        .ok_or_else(|| HarnessError::Contract(format!("{what} struct has no name")))?;
    let mut out = Vec::new();
    for m in members {
        let n = m
            .name
            .as_deref()
            .ok_or_else(|| HarnessError::Contract(format!("{what}: a member has no name")))?;
        out.push(field_of(module, n, m.ty)?);
    }
    Ok((name, out))
}

impl Subject {
    /// The oracle as a subject.
    pub fn from_oracle(o: &Oracle) -> Result<Subject, HarnessError> {
        let mut s = Subject::hand(&o.name, &o.wgsl, o.stage, &o.body)?;
        if let Some(cb) = o.consts {
            s.consts = ConstSource::Oracle(cb);
        }
        Ok(s)
    }

    /// A hand-written module: `wgsl` is the whole text (its includes already concatenated),
    /// `body` the name of the body function.
    pub fn hand(
        label: &str,
        wgsl: &str,
        stage: Stage,
        body: &str,
    ) -> Result<Subject, HarnessError> {
        let module = naga::front::wgsl::parse_str(wgsl)
            .map_err(|e| HarnessError::Wgsl(format!("{label}: {}", e.emit_to_string(wgsl))))?;
        let f = module
            .functions
            .iter()
            .map(|(_, f)| f)
            .find(|f| f.name.as_deref() == Some(body))
            .ok_or_else(|| {
                HarnessError::Contract(format!("{label}: there is no function `{body}`"))
            })?;
        let [arg] = f.arguments.as_slice() else {
            return Err(HarnessError::Contract(format!(
                "{label}: `{body}` must take one struct argument"
            )));
        };
        let res = f.result.as_ref().ok_or_else(|| {
            HarnessError::Contract(format!("{label}: `{body}` must return a struct"))
        })?;
        let (in_type, input) =
            struct_fields(&module, &format!("{label}: argument of `{body}`"), arg.ty)?;
        let (_, output) = struct_fields(&module, &format!("{label}: result of `{body}`"), res.ty)?;
        Ok(Subject {
            label: label.to_string(),
            wgsl: wgsl.to_string(),
            stage,
            body: body.to_string(),
            in_type,
            consts: ConstSource::ByCtab,
            input,
            output,
        })
    }
}

// ---- constants by CTAB name ---------------------------------------------------------------

/// What `fill_uniforms_by_ctab` did to one uniform buffer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UboFill {
    pub bytes: Vec<u8>,
    /// Members that got values (CTAB names).
    pub filled: Vec<String>,
    /// Members with no CTAB constant of that name: left zero.
    pub unfilled: Vec<String>,
}

enum View<'a> {
    F(&'a [[f32; 4]]),
    I(&'a [[i32; 4]]),
    B(u32),
}

impl View<'_> {
    /// Bits of component `comp` of register `reg` of the view, if there is one.
    fn bits(&self, reg: usize, comp: usize, kind: ScalarKind) -> Result<u32, String> {
        match (self, kind) {
            (View::F(r), ScalarKind::Float) => Ok(r.get(reg).map_or(0.0f32, |q| q[comp]).to_bits()),
            (View::I(r), ScalarKind::Sint | ScalarKind::Uint) => {
                Ok(r.get(reg).map_or(0, |q| q[comp]) as u32)
            }
            (View::B(v), ScalarKind::Uint | ScalarKind::Sint) => Ok(*v),
            _ => Err("the CTAB register set and the WGSL type do not agree".into()),
        }
    }

    fn skip(&self, regs: usize) -> View<'_> {
        match self {
            View::F(r) => View::F(r.get(regs..).unwrap_or(&[])),
            View::I(r) => View::I(r.get(regs..).unwrap_or(&[])),
            View::B(v) => View::B(*v),
        }
    }

    fn len(&self) -> usize {
        match self {
            View::F(r) => r.len(),
            View::I(r) => r.len(),
            View::B(_) => 1,
        }
    }
}

fn put(bytes: &mut [u8], off: usize, bits: u32) -> Result<(), String> {
    let dst = bytes
        .get_mut(off..off + 4)
        .ok_or_else(|| format!("write at byte {off} is past the end of the buffer"))?;
    dst.copy_from_slice(&bits.to_le_bytes());
    Ok(())
}

/// Writes constant `c` (registers in `view`) into a WGSL value of type `ty` at `off`.
fn write_ty(
    module: &Module,
    ty: Handle<Type>,
    bytes: &mut [u8],
    off: usize,
    view: &View,
    c: &CtabConstant,
) -> Result<(), String> {
    match &module.types[ty].inner {
        TypeInner::Scalar(s) => put(bytes, off, view.bits(0, 0, s.kind)?),
        TypeInner::Vector { size, scalar } => {
            for k in 0..*size as usize {
                put(bytes, off + 4 * k, view.bits(0, k, scalar.kind)?)?;
            }
            Ok(())
        }
        TypeInner::Matrix {
            columns,
            rows,
            scalar,
        } => {
            let (cols, rows) = (*columns as usize, *rows as usize);
            // Column stride of a WGSL matrix: the alignment of its column vector.
            let stride = if rows == 2 { 8 } else { 16 };
            match c.ty.class {
                // Register k holds column k (D3D9 default, column-major constants).
                ParamClass::MatrixColumns => {
                    for j in 0..cols {
                        for k in 0..rows {
                            put(
                                bytes,
                                off + j * stride + 4 * k,
                                view.bits(j, k, scalar.kind)?,
                            )?;
                        }
                    }
                }
                // Register k holds row k: element (column j, row k) = register k, component j.
                ParamClass::MatrixRows => {
                    for k in 0..rows {
                        for j in 0..cols {
                            put(
                                bytes,
                                off + j * stride + 4 * k,
                                view.bits(k, j, scalar.kind)?,
                            )?;
                        }
                    }
                }
                other => {
                    return Err(format!(
                        "a matrix in WGSL for a CTAB constant of class {}",
                        other.name()
                    ));
                }
            }
            Ok(())
        }
        TypeInner::Array { base, size, stride } => {
            let ArraySize::Constant(n) = size else {
                return Err("array of unknown length".into());
            };
            let n = n.get() as usize;
            // The registers of one element: a whole matrix, else one register.
            let per = (view.len() / n).max(1);
            for e in 0..n {
                write_ty(
                    module,
                    *base,
                    bytes,
                    off + e * *stride as usize,
                    &view.skip(e * per),
                    c,
                )?;
            }
            Ok(())
        }
        _ => Err(
            "only scalars, vectors, matrices and arrays of them can hold a CTAB constant".into(),
        ),
    }
}

fn write_const(
    module: &Module,
    ty: Handle<Type>,
    bytes: &mut [u8],
    off: usize,
    c: &CtabConstant,
    regs: &Registers,
) -> Result<bool, String> {
    let reg = c.reg as usize;
    let count = c.count as usize;
    let view = match c.set {
        RegisterSet::Float4 => View::F(
            regs.c
                .get(reg..)
                .map_or(&[][..], |s| &s[..count.min(s.len())]),
        ),
        RegisterSet::Int4 => {
            View::I(&regs.i[reg.min(regs.i.len())..(reg + count).min(regs.i.len())])
        }
        RegisterSet::Bool => View::B((regs.b >> reg.min(31)) & 1),
        RegisterSet::Sampler => return Ok(false),
    };
    write_ty(module, ty, bytes, off, &view, c)?;
    Ok(true)
}

/// Fills the bytes of the uniform variable `var` of `module` from the registers, by CTAB name:
/// every member whose name is that of a CTAB constant gets that constant's registers in the
/// layout WGSL gives it (`float4` in a `vec4`, a matrix by its class, an array element by
/// element). Members the CTAB does not know stay zero and are listed in `unfilled`.
pub fn fill_uniforms_by_ctab(
    module: &Module,
    var: &naga::GlobalVariable,
    ctab: &Ctab,
    regs: &Registers,
) -> Result<UboFill, HarnessError> {
    let ty = &module.types[var.ty].inner;
    let size = ty.size(module.to_ctx()) as usize;
    let mut fill = UboFill {
        bytes: vec![0; size],
        ..Default::default()
    };
    let var_name = var.name.as_deref().unwrap_or("?");
    let one = |name: &str, ty: Handle<Type>, off: usize, fill: &mut UboFill| match ctab
        .constants
        .iter()
        .find(|c| c.name == name)
    {
        Some(c) => match write_const(module, ty, &mut fill.bytes, off, c, regs) {
            Ok(true) => {
                fill.filled.push(name.to_string());
                Ok(())
            }
            Ok(false) => Ok(()),
            Err(e) => Err(HarnessError::Fill(format!(
                "uniform `{var_name}`, constant `{name}`: {e}"
            ))),
        },
        None => {
            fill.unfilled.push(name.to_string());
            Ok(())
        }
    };
    match ty {
        TypeInner::Struct { members, .. } => {
            for m in members {
                let name = m.name.clone().unwrap_or_default();
                one(&name, m.ty, m.offset as usize, &mut fill)?;
            }
        }
        _ => one(var_name, var.ty, 0, &mut fill)?,
    }
    Ok(fill)
}

/// The bytes of the oracle's `OracleConsts` block (SR-D5): `c`, then `i`, then the `b` mask.
pub fn oracle_uniform_bytes(cb: &ConstBlock, regs: &Registers) -> Vec<u8> {
    let mut bytes = vec![0u8; cb.size()];
    for (k, q) in regs.c.iter().take(cb.float_regs as usize).enumerate() {
        for (j, x) in q.iter().enumerate() {
            let at = k * 16 + j * 4;
            bytes[at..at + 4].copy_from_slice(&x.to_le_bytes());
        }
    }
    if cb.int_regs > 0 {
        for (k, q) in regs.i.iter().take(cb.int_regs as usize).enumerate() {
            for (j, x) in q.iter().enumerate() {
                let at = cb.i_offset() + k * 16 + j * 4;
                bytes[at..at + 4].copy_from_slice(&x.to_le_bytes());
            }
        }
    }
    if cb.bool_mask {
        bytes[cb.b_offset()..cb.b_offset() + 4].copy_from_slice(&regs.b.to_le_bytes());
    }
    bytes
}

/// Values of the `instance_*` members and the bone palette from `data_moves` (4.8).
/// `slots(field)` is what the wrapper loads for the member: one `vec4` per matrix column.
pub fn move_slots(
    f: &Field,
    c: &CtabConstant,
    regs: &Registers,
) -> Result<Vec<[f32; 4]>, HarnessError> {
    let reg = c.reg as usize;
    let get = |k: usize| regs.c.get(reg + k).copied().unwrap_or([0.0; 4]);
    let err = |why: String| HarnessError::Fill(format!("data move of `{}`: {why}", c.name));
    if c.set != RegisterSet::Float4 {
        return Err(err("only float constants can move".into()));
    }
    match f.shape {
        Shape::Scalar | Shape::Vector(_) => Ok(vec![get(0)]),
        Shape::Matrix { cols, rows } => {
            let (cols, rows) = (cols as usize, rows as usize);
            let mut slots = vec![[0f32; 4]; cols];
            match c.ty.class {
                ParamClass::MatrixColumns => {
                    for (j, s) in slots.iter_mut().enumerate() {
                        let r = get(j);
                        s[..rows].copy_from_slice(&r[..rows]);
                    }
                }
                ParamClass::MatrixRows => {
                    for k in 0..rows {
                        let r = get(k);
                        for (j, s) in slots.iter_mut().enumerate() {
                            s[k] = r[j];
                        }
                    }
                }
                other => {
                    return Err(err(format!(
                        "a matrix member for a constant of class {}",
                        other.name()
                    )));
                }
            }
            Ok(slots)
        }
    }
}
