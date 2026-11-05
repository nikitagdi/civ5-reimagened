//! Modal stack and the popup queue (`UIManager::PushModal`, `PopModal`, `QueuePopup`,
//! `DequeuePopup`). The bodies are in the dump; only the deferred removal of `PopModal` is
//! done at once.

use crate::flags::*;
use crate::state::{Pending, State};
use crate::tree::*;

impl State {
    /// `UIManager::PushModal(control, pass)`: the control is flagged modal and, like a popup,
    /// hidden in the tree; the modal list is where it is updated, drawn and given input.
    pub fn push_modal(&mut self, id: ControlId, pass: bool) {
        if !self.alive(id) {
            return;
        }
        let n = &mut self.tree[id];
        n.flags |= MODAL | MOUSE_OVER_HIDDEN;
        if pass {
            n.flags |= MODAL_PASS;
        } else {
            n.flags &= !MODAL_PASS;
        }
        self.modals.retain(|&m| m != id);
        self.modals.push(id);
        if matches!(self.tree[id].kind, Kind::Context(_)) {
            // The pushed context is shown (its show/hide handler runs): a child LuaContext declared `Hidden="1"` (MPList's
            // ConfirmKick) stayed hidden, so the modal drew nothing and swallowed every click (live report 2026-10-30, the
            // in-game kick froze the UI). The Lua takes it down itself (`PopModal` then `ContextPtr:SetHide(true)`).
            // SIMPLIFIED(SIMP-uic-1): read off the scripts' use of PushModal / PopModal, not off the exe's body.
            self.pending.push(Pending::SetHide {
                ctx: id,
                hide: false,
            });
        }
    }

    /// `UIManager::PopModal`.
    pub fn pop_modal(&mut self, id: ControlId) {
        let Some(i) = self.modals.iter().position(|&m| m == id) else {
            return;
        };
        self.modals.remove(i);
        if !self.alive(id) {
            return;
        }
        if matches!(self.tree[id].kind, Kind::Context(_)) {
            self.pending.push(Pending::ShowHide {
                ctx: id,
                hide: true,
            });
        }
        self.tree[id].flags &= !(MODAL | MOUSE_OVER_HIDDEN);
    }

    /// `UIManager::IsModal`.
    pub fn is_modal(&self, id: ControlId) -> bool {
        self.modals.contains(&id)
    }

    /// `UIManager::IsTopModal`.
    pub fn is_top_modal(&self, id: ControlId) -> bool {
        self.modals.last() == Some(&id)
    }

    /// `UIManager::QueuePopup(ctx, priority)`: the list is ordered by priority, the head is shown.
    pub fn queue_popup(&mut self, ctx: ControlId, priority: u32) {
        // SIMPLIFIED(SIMP-uic-1): the body is transcribed; the meaning of the special
        // priorities (9999 keeps the head's priority, 10000 is 0) is a guess from the code.
        let mut prio = priority;
        if prio == 10_000 {
            prio = 0;
        }
        tracing::debug!(target: "ui.popup", "QueuePopup {} prio {} queue {:?}", self.tree[ctx].id, priority, self.popups.iter().map(|p| (self.tree[p.0].id.clone(), p.1)).collect::<Vec<_>>());
        let head_prio = self.popups.first().map(|p| p.1);
        let mut insert_at: Option<usize> = None;
        let mut existing: Option<usize> = None;
        if prio == 9999 {
            existing = self.popups.iter().position(|p| p.0 == ctx);
            if let Some(h) = head_prio {
                prio = h;
            }
            insert_at = Some(0);
        } else {
            for (i, p) in self.popups.iter().enumerate() {
                if insert_at.is_none() && p.1 <= prio {
                    insert_at = Some(i);
                }
                if p.0 == ctx {
                    existing = Some(i);
                    if prio < p.1 {
                        return;
                    }
                    break;
                }
            }
        }
        let at = insert_at.unwrap_or(self.popups.len());
        self.tree[ctx].flags |= 0x2000_0000;
        let was_head = at == 0;
        self.popups.insert(at, (ctx, prio));
        if was_head {
            if let Some(cur) = self.popup_current
                && self.alive(cur)
            {
                self.tree[cur].flags &= !MOUSE_OVER_HIDDEN;
                self.pending_hide(cur, true);
            }
            self.popup_current = Some(ctx);
            self.tree[ctx].flags |= MOUSE_OVER_HIDDEN;
            self.pending_hide(ctx, false);
        }
        if let Some(e) = existing {
            // The old entry moved by the insert when it was behind the new one.
            let idx = if e >= at { e + 1 } else { e };
            self.popups.remove(idx);
        }
    }

    /// `UIManager::DequeuePopup`.
    pub fn dequeue_popup(&mut self, ctx: ControlId) {
        if let Some(i) = self.popups.iter().position(|p| p.0 == ctx) {
            if self.alive(ctx) {
                self.tree[ctx].flags &= !0x2000_0000;
            }
            if self.popup_current == Some(ctx) {
                self.popup_current = None;
            }
            if self.alive(ctx) {
                self.tree[ctx].flags &= !MOUSE_OVER_HIDDEN;
            }
            self.pending_hide(ctx, true);
            self.popups.remove(i);
        }
        if self.popup_current.is_none()
            && let Some(&(head, _)) = self.popups.first()
        {
            self.popup_current = Some(head);
            if self.alive(head) {
                self.tree[head].flags |= MOUSE_OVER_HIDDEN;
            }
            self.pending_hide(head, false);
        }
    }

    /// `SetHide` of a control from the state layer: contexts need the Lua side, so it is queued.
    pub fn pending_hide(&mut self, id: ControlId, hide: bool) {
        if !self.alive(id) {
            return;
        }
        if matches!(self.tree[id].kind, Kind::Context(_)) {
            self.pending.push(Pending::SetHide { ctx: id, hide });
        } else if hide {
            self.tree[id].flags |= HIDDEN;
        } else {
            self.tree[id].flags &= !HIDDEN;
        }
    }
}
