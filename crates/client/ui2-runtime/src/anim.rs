//! Transitions: paint properties that move from the old record's value to the new one over time.
//!
//! When the style pass selects another record for a node (a hover, a class, a state), each transition of the
//! new record whose property changed starts an animation: from the value shown now (the old record's, or
//! where a running animation of that property is) to the new record's, with the record's duration, delay
//! and cubic Bezier. Only paint properties transition (`color`, `background-color`, the border colors,
//! `outline-color`, `opacity`, `transform`, the colors of `box-shadow`; gradient stops switch at once); the
//! profile does not let geometry transition, so a
//! transition never lays anything out.
//!
//! Every frame with running animations ([`Animator::tick`]) builds, for each animated node, its record with
//! the current values (paint reads it instead of the selected record) and queues the node's chunk for paint.
//! A finished animation leaves the node on its selected record and costs nothing more; `animating` in the
//! frame output is true only while an animation runs. The clock is the frame time the host passes.

use std::collections::HashMap;

use ui2_package::PropId;
use ui2_package::style::{ColorV, Len, StyleRecord, TFn};

use crate::arena::{Arena, Dirty, NodeId};
use crate::style::{StyleIndex, node_record, tokens};

/// A value being animated.
#[derive(Debug, Clone, PartialEq)]
enum AVal {
    /// A color in RGBA (resolved under the theme when the animation started).
    Color([f32; 4]),
    /// A number (`opacity`).
    Num(f32),
    /// A transform list.
    Transform(Vec<TFn>),
    /// The colors of the shadows.
    Shadows(Vec<[f32; 4]>),
}

#[derive(Debug, Clone)]
struct Anim {
    node: NodeId,
    prop: PropId,
    from: AVal,
    to: AVal,
    /// The frame time it starts at (the delay included), ms.
    start: f64,
    dur: f64,
    ease: [f32; 4],
}

/// What reading a record needs.
pub(crate) struct AnimCtx<'a> {
    pub pkg: &'a crate::Pkg,
    pub index: &'a StyleIndex,
    pub theme: u16,
}

/// The running transitions of a runtime and the records they show.
#[derive(Debug, Default)]
pub(crate) struct Animator {
    anims: Vec<Anim>,
    shown: HashMap<NodeId, StyleRecord>,
    /// The time of the last tick, while no transition started since: a tick at the same time (a frozen
    /// scene, a clock that stands still) has nothing new to paint.
    painted_at: Option<f64>,
}

fn rgba(c: [u8; 4]) -> [f32; 4] {
    c.map(f32::from)
}

fn to_u8(c: [f32; 4]) -> [u8; 4] {
    c.map(|v| v.round().clamp(0.0, 255.0) as u8)
}

/// The value of an animatable property in a record; `None` for properties that do not transition.
fn value(cx: &AnimCtx<'_>, r: &StyleRecord, prop: PropId) -> Option<AVal> {
    let color = |c: ColorV| AVal::Color(rgba(tokens::resolve(cx.pkg.tokens(), cx.theme, c)));
    Some(match prop {
        PropId::Color => color(r.color),
        PropId::BackgroundColor => color(r.background_color),
        PropId::BorderTopColor => color(r.border_top_color),
        PropId::BorderRightColor => color(r.border_right_color),
        PropId::BorderBottomColor => color(r.border_bottom_color),
        PropId::BorderLeftColor => color(r.border_left_color),
        PropId::OutlineColor => color(r.outline_color),
        PropId::Opacity => AVal::Num(r.opacity),
        PropId::Transform => AVal::Transform(r.transform.clone()),
        PropId::BoxShadow => AVal::Shadows(
            r.box_shadow
                .iter()
                .map(|s| rgba(tokens::resolve(cx.pkg.tokens(), cx.theme, s.color)))
                .collect(),
        ),
        _ => return None,
    })
}

/// Writes an animated value into a record.
fn apply(r: &mut StyleRecord, prop: PropId, v: &AVal) {
    let lit = |c: &[f32; 4]| ColorV::Lit(to_u8(*c));
    match (prop, v) {
        (PropId::Color, AVal::Color(c)) => r.color = lit(c),
        (PropId::BackgroundColor, AVal::Color(c)) => r.background_color = lit(c),
        (PropId::BorderTopColor, AVal::Color(c)) => r.border_top_color = lit(c),
        (PropId::BorderRightColor, AVal::Color(c)) => r.border_right_color = lit(c),
        (PropId::BorderBottomColor, AVal::Color(c)) => r.border_bottom_color = lit(c),
        (PropId::BorderLeftColor, AVal::Color(c)) => r.border_left_color = lit(c),
        (PropId::OutlineColor, AVal::Color(c)) => r.outline_color = lit(c),
        (PropId::Opacity, AVal::Num(o)) => r.opacity = o.clamp(0.0, 1.0),
        (PropId::Transform, AVal::Transform(t)) => r.transform = t.clone(),
        (PropId::BoxShadow, AVal::Shadows(cs)) => {
            for (s, c) in r.box_shadow.iter_mut().zip(cs) {
                s.color = lit(c);
            }
        }
        _ => {}
    }
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn lerp_len(a: Len, b: Len, t: f32) -> Len {
    Len {
        px: lerp(a.px, b.px, t),
        pct: lerp(a.pct, b.pct, t),
    }
}

/// Colors interpolate premultiplied, as in CSS.
fn lerp_color(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    let (aa, ba) = (a[3] / 255.0, b[3] / 255.0);
    let alpha = lerp(aa, ba, t);
    if alpha <= 0.0 {
        return [0.0; 4];
    }
    let ch = |i: usize| lerp(a[i] * aa, b[i] * ba, t) / alpha;
    [ch(0), ch(1), ch(2), alpha * 255.0]
}

/// The identity function of the kind of `f` (missing functions of a list interpolate from or to it).
fn identity(f: &TFn) -> TFn {
    match f {
        TFn::Translate(..) => TFn::Translate(Len::ZERO, Len::ZERO),
        TFn::Rotate(_) => TFn::Rotate(0.0),
        TFn::Scale(..) => TFn::Scale(1.0, 1.0),
    }
}

fn lerp_transform(a: &[TFn], b: &[TFn], t: f32) -> Vec<TFn> {
    let pad = |x: &[TFn], other: &[TFn]| -> Vec<TFn> {
        if x.is_empty() {
            other.iter().map(identity).collect()
        } else {
            x.to_vec()
        }
    };
    let (a, b) = (pad(a, b), pad(b, a));
    let same = a.len() == b.len()
        && a.iter()
            .zip(&b)
            .all(|(x, y)| std::mem::discriminant(x) == std::mem::discriminant(y));
    if !same {
        // SIMPLIFIED(SIMP-280): transform lists whose functions differ in kind or count are not blended: the value jumps from the old list to the new one at the midpoint of the transition (a browser interpolates the matrices).
        return if t < 0.5 { a } else { b };
    }
    a.iter()
        .zip(&b)
        .map(|(x, y)| match (x, y) {
            (TFn::Translate(x0, y0), TFn::Translate(x1, y1)) => {
                TFn::Translate(lerp_len(*x0, *x1, t), lerp_len(*y0, *y1, t))
            }
            (TFn::Rotate(a0), TFn::Rotate(a1)) => TFn::Rotate(lerp(*a0, *a1, t)),
            (TFn::Scale(x0, y0), TFn::Scale(x1, y1)) => {
                TFn::Scale(lerp(*x0, *x1, t), lerp(*y0, *y1, t))
            }
            _ => *y,
        })
        .collect()
}

fn interpolate(a: &AVal, b: &AVal, t: f32) -> AVal {
    match (a, b) {
        (AVal::Color(x), AVal::Color(y)) => AVal::Color(lerp_color(*x, *y, t)),
        (AVal::Num(x), AVal::Num(y)) => AVal::Num(lerp(*x, *y, t)),
        (AVal::Transform(x), AVal::Transform(y)) => AVal::Transform(lerp_transform(x, y, t)),
        (AVal::Shadows(x), AVal::Shadows(y)) if x.len() == y.len() => AVal::Shadows(
            x.iter()
                .zip(y)
                .map(|(c, d)| lerp_color(*c, *d, t))
                .collect(),
        ),
        _ => {
            if t < 0.5 {
                a.clone()
            } else {
                b.clone()
            }
        }
    }
}

/// A CSS cubic Bezier timing function at progress `x` (0 to 1).
pub fn ease(curve: [f32; 4], x: f32) -> f32 {
    let [x1, y1, x2, y2] = curve.map(f64::from);
    let x = f64::from(x.clamp(0.0, 1.0));
    let bez = |a: f64, b: f64, s: f64| {
        let u = 1.0 - s;
        3.0 * u * u * s * a + 3.0 * u * s * s * b + s * s * s
    };
    // The curve's x is monotonic on [0, 1]: bisection finds the parameter.
    let (mut lo, mut hi) = (0.0f64, 1.0f64);
    for _ in 0..40 {
        let mid = (lo + hi) / 2.0;
        if bez(x1, x2, mid) < x {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    bez(y1, y2, (lo + hi) / 2.0) as f32
}

impl Anim {
    fn progress(&self, now: f64) -> f32 {
        if now <= self.start {
            return 0.0;
        }
        if self.dur <= 0.0 {
            return 1.0;
        }
        ((now - self.start) / self.dur).clamp(0.0, 1.0) as f32
    }

    fn at(&self, now: f64) -> AVal {
        interpolate(&self.from, &self.to, ease(self.ease, self.progress(now)))
    }

    fn done(&self, now: f64) -> bool {
        now >= self.start + self.dur
    }
}

impl Animator {
    /// Whether a transition runs (or a node still shows an animated record).
    pub fn is_idle(&self) -> bool {
        self.anims.is_empty() && self.shown.is_empty()
    }

    /// Records were selected again: the next tick builds the animated records anew even at the same time.
    pub fn restyled(&mut self) {
        self.painted_at = None;
    }

    /// The record a node is painted with while it is animated.
    pub fn record(&self, id: NodeId) -> Option<&StyleRecord> {
        self.shown.get(&id)
    }

    /// Starts the transitions of nodes whose record changed: `changes` holds each node with its old record.
    pub fn start(
        &mut self,
        now: f64,
        arena: &Arena,
        cx: &AnimCtx<'_>,
        changes: Vec<(NodeId, StyleRecord)>,
    ) {
        for (id, old) in changes {
            let Some(node) = arena.get(id) else {
                continue;
            };
            let new = node_record(cx.index, cx.pkg.styles(), node);
            for t in &new.transitions {
                let Some(to) = value(cx, new, t.prop) else {
                    continue;
                };
                let running = self
                    .anims
                    .iter()
                    .position(|a| a.node == id && a.prop == t.prop);
                let from = match running {
                    Some(i) => self.anims[i].at(now),
                    None => match value(cx, &old, t.prop) {
                        Some(v) => v,
                        None => continue,
                    },
                };
                if let Some(i) = running {
                    self.anims.swap_remove(i);
                }
                if from == to || t.dur_ms <= 0.0 {
                    continue;
                }
                self.painted_at = None;
                self.anims.push(Anim {
                    node: id,
                    prop: t.prop,
                    from,
                    to,
                    start: now + f64::from(t.delay_ms.max(0.0)),
                    dur: f64::from(t.dur_ms),
                    ease: t.ease,
                });
            }
        }
    }

    /// The nodes of `changes` take their selected records at once (a frozen scene): their running transitions end
    /// and none starts.
    pub fn settle(&mut self, changes: &[(NodeId, StyleRecord)]) {
        let before = self.anims.len();
        self.anims
            .retain(|a| !changes.iter().any(|(id, _)| *id == a.node));
        if self.anims.len() != before {
            self.painted_at = None;
        }
    }

    /// Moves the animations to `now`: the animated records are built again and their chunks queued for paint;
    /// finished animations end (their nodes are painted once more with their selected record). Returns
    /// whether an animation still runs. Nothing is done without animations.
    pub fn tick(&mut self, now: f64, arena: &mut Arena, cx: &AnimCtx<'_>) -> bool {
        if self.is_idle() {
            return false;
        }
        self.anims.retain(|a| arena.contains(a.node));
        if self.painted_at == Some(now) {
            return !self.anims.is_empty();
        }
        self.painted_at = Some(now);
        let mut shown: HashMap<NodeId, StyleRecord> = HashMap::new();
        for a in &self.anims {
            let Some(node) = arena.get(a.node) else {
                continue;
            };
            let r = shown
                .entry(a.node)
                .or_insert_with(|| node_record(cx.index, cx.pkg.styles(), node).clone());
            apply(r, a.prop, &a.at(now));
        }
        for id in self.shown.keys().chain(shown.keys()) {
            arena.mark(*id, Dirty::PAINT);
        }
        self.anims.retain(|a| !a.done(now));
        // A node whose animations all ended this frame is painted with its last values now and with its
        // record (which they equal) from the next frame on.
        self.shown = shown;
        if self.anims.is_empty() {
            let ended: Vec<NodeId> = self.shown.keys().copied().collect();
            for id in ended {
                arena.mark(id, Dirty::PAINT);
            }
            self.shown.clear();
        }
        !self.anims.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bezier_timing_follows_css() {
        let linear = [0.0, 0.0, 1.0, 1.0];
        assert!((ease(linear, 0.25) - 0.25).abs() < 1e-4);
        let css_ease = [0.25, 0.1, 0.25, 1.0];
        // `ease` at half time is about 0.8024 of the way.
        assert!((ease(css_ease, 0.5) - 0.8024).abs() < 1e-3);
        assert!(ease(css_ease, 0.0).abs() < 1e-6);
        assert!((ease(css_ease, 1.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn colors_interpolate_premultiplied_and_transforms_by_function() {
        let c = lerp_color([255.0, 0.0, 0.0, 255.0], [0.0, 0.0, 255.0, 0.0], 0.5);
        assert_eq!(to_u8(c), [255, 0, 0, 128]);
        let t = lerp_transform(&[], &[TFn::Rotate(90.0), TFn::Scale(2.0, 2.0)], 0.5);
        assert_eq!(t, vec![TFn::Rotate(45.0), TFn::Scale(1.5, 1.5)]);
    }
}
