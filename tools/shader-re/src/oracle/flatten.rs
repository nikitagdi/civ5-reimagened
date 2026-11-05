//! Flattening of branches and loops (SR-D4, rows `if` and `rep`).
//!
//! WGSL wants derivatives and `textureSample` in uniform control flow, and SM3 has no side
//! effects but the outputs and `texkill`. So the oracle never branches:
//!
//! - `if` / `else` / `endif` are a stack of execution masks. Both sides are computed; every
//!   register write, `mova` and `texkill` is gated by the current mask (`Emit::guarded`).
//! - `rep i#` is a real `for` loop. The count is `clamp(i#.x, 0, 255)` and comes from the
//!   constant block or a `defi`, so it is uniform. `break` never leaves the loop: it clears
//!   the loop's `alive` mask for the lanes that took it, and later writes in the loop are gated
//!   by that mask.
//!
//! Masks are `bool` locals (`_c1` condition, `_p1` parent, `_m1` branch mask, `_alive1`).

use super::emit::Emit;
use crate::sm3::ir::{Cmp, Inst, Opcode};

#[derive(Debug)]
enum Frame {
    If {
        /// Effective mask when the `if` was reached (`None` = always), as a local.
        parent: Option<String>,
        /// The condition of the `if`, as a local.
        cond: String,
        /// Mask of the side being written now.
        mask: String,
        has_else: bool,
    },
    Loop {
        /// `var` that is cleared for lanes that left the loop.
        alive: String,
    },
}

/// Execution-mask state while walking a body.
#[derive(Debug, Default)]
pub struct Flow {
    frames: Vec<Frame>,
}

impl Flow {
    /// The mask every write is gated by; `None` = always executing.
    ///
    /// The mask of an `if` branch already contains everything outside it as it was when the
    /// `if` was reached; only the `alive` mask of the enclosing loop can have changed since.
    pub fn mask(&self) -> Option<String> {
        let alive = self.frames.iter().rev().find_map(|f| match f {
            Frame::Loop { alive } => Some(alive.as_str()),
            _ => None,
        });
        match (self.frames.last()?, alive) {
            (Frame::Loop { alive }, _) => Some(alive.clone()),
            (Frame::If { mask, .. }, None) => Some(mask.clone()),
            (Frame::If { mask, .. }, Some(a)) => Some(format!("({mask} && {a})")),
        }
    }

    /// Nesting depth of `for` loops (for indentation).
    pub fn depth(&self) -> usize {
        self.frames
            .iter()
            .filter(|f| matches!(f, Frame::Loop { .. }))
            .count()
    }

    /// What is still open at the end of the block, if anything.
    pub fn open(&self) -> Option<&'static str> {
        self.frames.last().map(|f| match f {
            Frame::If { .. } => "`if`",
            Frame::Loop { .. } => "`rep`",
        })
    }

    fn innermost_alive(&self) -> Option<&str> {
        self.frames.iter().rev().find_map(|f| match f {
            Frame::Loop { alive } => Some(alive.as_str()),
            _ => None,
        })
    }
}

fn cmp_op(c: Cmp) -> &'static str {
    match c {
        Cmp::Gt => ">",
        Cmp::Eq => "==",
        Cmp::Ge => ">=",
        Cmp::Lt => "<",
        Cmp::Ne => "!=",
        Cmp::Le => "<=",
    }
}

/// `a <cmp> b` of the first components of the two operands.
fn comparison(e: &Emit<'_>, i: &Inst) -> Result<String, String> {
    let [a, b] = i.src.as_slice() else {
        return Err("comparison without two operands".into());
    };
    let c = i.cmp().ok_or("comparison code is not 1..=6")?;
    Ok(format!("({} {} {})", e.src(a, 1)?, cmp_op(c), e.src(b, 1)?))
}

fn open_if(e: &mut Emit<'_>, cond: String) {
    let eff = e.flow.mask();
    let c = e.tmp("c");
    e.line(format!("let {c} = {cond};"));
    let (parent, mask) = match eff {
        None => (None, c.clone()),
        Some(p) => {
            let pn = e.tmp("p");
            e.line(format!("let {pn} = {p};"));
            let m = e.tmp("m");
            e.line(format!("let {m} = {pn} && {c};"));
            (Some(pn), m)
        }
    };
    e.flow.frames.push(Frame::If {
        parent,
        cond: c,
        mask,
        has_else: false,
    });
}

/// `if`, `else`, `endif`, `rep`, `endrep`, `break*`.
pub fn handle(e: &mut Emit<'_>, i: &Inst) -> Result<(), String> {
    match i.op {
        Opcode::If => {
            let cond = e.bool_reg(i.src.first().ok_or("`if` without operand")?)?;
            open_if(e, cond);
        }
        Opcode::Ifc => {
            let cond = comparison(e, i)?;
            open_if(e, cond);
        }
        Opcode::Else => {
            let m = e.tmp("m");
            let Some(Frame::If {
                parent,
                cond,
                mask,
                has_else,
            }) = e.flow.frames.last_mut()
            else {
                return Err("`else` outside an `if`".into());
            };
            if *has_else {
                return Err("second `else` in one `if`".into());
            }
            let expr = match parent {
                None => format!("!{cond}"),
                Some(p) => format!("{p} && !{cond}"),
            };
            *mask = m.clone();
            *has_else = true;
            e.line(format!("let {m} = {expr};"));
        }
        Opcode::EndIf => {
            if !matches!(e.flow.frames.last(), Some(Frame::If { .. })) {
                return Err("`endif` without an `if`".into());
            }
            e.flow.frames.pop();
        }
        Opcode::Rep => {
            let reg = e.int_reg(i.src.first().ok_or("`rep` without operand")?)?;
            let eff = e.flow.mask().unwrap_or_else(|| "true".to_string());
            let n = e.tmp("n");
            e.line(format!("let {n} = clamp({reg}.x, 0, 255);"));
            let alive = e.tmp("alive");
            e.line(format!("var {alive} = {eff};"));
            let k = e.tmp("k");
            e.line(format!("for (var {k}: i32 = 0; {k} < {n}; {k}++) {{"));
            e.flow.frames.push(Frame::Loop { alive });
        }
        Opcode::EndRep => {
            if !matches!(e.flow.frames.last(), Some(Frame::Loop { .. })) {
                return Err("`endrep` without a `rep` (or an `if` is still open)".into());
            }
            e.flow.frames.pop();
            e.line("}");
        }
        Opcode::Break | Opcode::BreakC => {
            let Some(alive) = e.flow.innermost_alive().map(str::to_string) else {
                return Err("`break` outside a `rep`".into());
            };
            let eff = e.flow.mask().expect("inside a loop there is a mask");
            if i.op == Opcode::Break {
                e.line(format!("{alive} = {alive} && !{eff};"));
            } else {
                let cond = comparison(e, i)?;
                e.line(format!("{alive} = {alive} && !({eff} && {cond});"));
            }
        }
        other => return Err(format!("`{}` is not control flow", other.name())),
    }
    Ok(())
}
