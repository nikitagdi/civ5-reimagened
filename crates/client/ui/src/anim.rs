//! `AnimControl` and its four classes: `AlphaAnim`, `SlideAnim`, `FlipAnim`, `ScrollAnim`.
//! Time is in seconds. Animations of hidden branches do not run (`ControlBase::Update`).

use glam::Vec2;

use crate::state::{LuaArg, State};
use crate::tree::*;

impl State {
    fn anim(&self, id: ControlId) -> Option<&Anim> {
        match &self.tree.get(id)?.kind {
            Kind::Anim(a) => Some(a),
            _ => None,
        }
    }

    fn anim_mut(&mut self, id: ControlId) -> Option<&mut Anim> {
        match &mut self.tree.get_mut(id)?.kind {
            Kind::Anim(a) => Some(a),
            _ => None,
        }
    }

    /// The easing of `AnimControl::Update` and `GetFunctionOut`.
    fn anim_ease(a: &Anim, x: f32) -> f32 {
        match a.function {
            1 => x.powf(a.power),
            2 => x.powf(1.0 / a.power),
            _ => x,
        }
    }

    /// `AnimControl::GetFunctionOut`.
    pub fn anim_function_out(&self, id: ControlId, progress: f32) -> f32 {
        let Some(a) = self.anim(id) else {
            return progress;
        };
        let p = if a.reversing {
            1.0 - progress
        } else {
            progress
        };
        Self::anim_ease(a, p)
    }

    /// `UpdateAnim(progress)` of the class.
    pub fn anim_update(&mut self, id: ControlId, p: f32) {
        let Some(a) = self.anim(id) else { return };
        match a.variant {
            AnimVariant::Alpha => {
                let v = (1.0 - p) * a.alpha_start + a.alpha_end * p;
                self.tree[id].alpha = v;
            }
            AnimVariant::Slide => {
                let o = a.slide_begin * (1.0 - p) + a.slide_end * p;
                self.set_offset(id, o.x, o.y);
            }
            AnimVariant::Flip => {
                let frame = (a.flip_frames as f32 * p).round() as i32;
                self.flip_set_frame(id, frame);
            }
            AnimVariant::Scroll => {
                // SIMPLIFIED(SIMP-uic-6): the scroll position is a shift of the source window; the
                // exe feeds it to a shader constant that is not read.
                if let Some(a) = self.anim_mut(id)
                    && let Some(q) = &mut a.quad
                {
                    q.uv_offset.x = p;
                }
            }
        }
    }

    /// `FlipAnim::SetFrame`: the source window moves by `StepSize` over `Columns`.
    pub fn flip_set_frame(&mut self, id: ControlId, frame: i32) {
        let Some(a) = self.anim_mut(id) else { return };
        let cols = a.flip_columns.max(1);
        let step = a.flip_step;
        if let Some(q) = &mut a.quad {
            q.src_x = step.x * (frame % cols) as f32;
            q.src_y = step.y * (frame / cols) as f32;
            q.normalize();
        }
    }

    /// `AnimControl::Update(dt)` for one control (the caller walks the children).
    pub fn anim_tick(&mut self, id: ControlId, dt: f32) {
        let Some(a) = self.anim_mut(id) else { return };
        if a.stopped {
            return;
        }
        if a.pause_remaining > 0.0 {
            a.pause_remaining -= dt;
            return;
        }
        a.progress += dt * a.speed;
        let reversing = a.reversing;
        if a.progress < 1.0 {
            let v = if reversing {
                1.0 - a.progress
            } else {
                a.progress
            };
            let v = Self::anim_ease(a, v);
            self.anim_update(id, v);
            return;
        }
        let end = if reversing { 0.0 } else { 1.0 };
        let end = Self::anim_ease(a, end);
        self.anim_update(id, end);
        let a = self.anim_mut(id).expect("anim");
        match a.cycle {
            2 | 3 => {
                a.progress = 0.0;
                if !reversing {
                    a.reversing = true;
                    a.pause_remaining = a.end_pause;
                } else if a.cycle == 3 {
                    a.progress = 1.0;
                    a.reversing = false;
                    a.stopped = true;
                } else {
                    a.reversing = false;
                    a.pause_remaining = a.pause;
                }
            }
            1 => {
                a.progress = 0.0;
                let pr = if a.end_pause > 0.0 {
                    a.end_pause
                } else {
                    a.pause
                };
                self.anim_update(id, end);
                self.anim_mut(id).expect("anim").pause_remaining = pr;
            }
            _ => {
                a.progress = 1.0;
                a.reversing = false;
                a.stopped = true;
            }
        }
        // vf_49: the end-of-run callback.
        if let Some(cb) = self.anim(id).and_then(|a| a.callback.clone()) {
            let owner = self.tree[id].owner;
            self.queue_call(owner, &cb, vec![LuaArg::Control(id)]);
        }
    }

    pub fn anim_play(&mut self, id: ControlId) {
        if let Some(a) = self.anim_mut(id) {
            a.stopped = false;
        }
    }

    pub fn anim_stop(&mut self, id: ControlId) {
        if let Some(a) = self.anim_mut(id) {
            a.stopped = true;
        }
    }

    /// `AnimControl::Reverse`.
    pub fn anim_reverse(&mut self, id: ControlId) {
        if let Some(a) = self.anim_mut(id) {
            a.reversing = !a.reversing;
            a.stopped = false;
            a.progress = 1.0 - a.progress;
        }
    }

    /// `AnimControl::SetToBeginning`.
    pub fn anim_to_beginning(&mut self, id: ControlId) {
        if let Some(a) = self.anim_mut(id) {
            a.progress = 0.0;
            a.reversing = false;
            a.stopped = true;
            a.pause_remaining = a.pause;
        }
        self.anim_update(id, 0.0);
    }

    /// `AnimControl::SetToEnd`.
    pub fn anim_to_end(&mut self, id: ControlId) {
        if let Some(a) = self.anim_mut(id) {
            a.progress = 1.0;
            a.stopped = true;
            if a.cycle == 2 {
                a.reversing = true;
            }
            a.pause_remaining = a.end_pause;
        }
        self.anim_update(id, 1.0);
    }

    pub fn anim_offset_for(&self, id: ControlId) -> Vec2 {
        self.tree[id].offset
    }
}
