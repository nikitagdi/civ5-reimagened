//! `InterfaceBuddy`: the popup queue of the exe.
//!
//! Every popup has a type (`ButtonPopupTypes`). At most one popup of a type is open, except for the types
//! registered as a stack. A popup of an open (non-stack) type waits until `ProcessedPopup(type)`, which shows the
//! first waiting popup of that type or closes the type. In the "do not show popups" mode popups go to a delayed
//! list; `process_delayed` shows them in order of arrival: the first if nothing is open, then only stack types.
//! A popup is shown as `Events.SerialEventGameMessagePopup(popupInfo)`.

use std::collections::{BTreeMap, BTreeSet};

use crate::ExeConfig;
use crate::core::{Arg, Family, Fire, Payload};

/// A popup: the `popupInfo` table the listeners get (`Type`, `Data1..`, `Option1..`, `Text`, ...).
#[derive(Debug, Clone, PartialEq)]
pub struct PopupInfo {
    pub ty: i32,
    pub fields: Vec<(String, Arg)>,
}

impl PopupInfo {
    /// A popup from the core: `Type`, `Data<i>` for the ints, `Option<i>` for the bools, `Text`.
    pub fn from_core(ty: i32, ints: &[i32], bools: &[bool], text: &str) -> PopupInfo {
        let mut fields = vec![("Type".to_owned(), Arg::from(ty))];
        for (i, v) in ints.iter().enumerate() {
            fields.push((format!("Data{}", i + 1), Arg::from(*v)));
        }
        for (i, v) in bools.iter().enumerate() {
            fields.push((format!("Option{}", i + 1), Arg::Bool(*v)));
        }
        if !text.is_empty() {
            fields.push(("Text".to_owned(), Arg::Str(text.to_owned())));
        }
        PopupInfo { ty, fields }
    }

    pub fn to_arg(&self) -> Arg {
        Arg::Table(self.fields.clone())
    }

    fn fire(&self) -> Fire {
        Fire {
            family: Family::Events,
            name: "SerialEventGameMessagePopup".to_owned(),
            payload: Payload::Args(vec![self.to_arg()]),
            hook: None,
        }
    }
}

#[derive(Debug, Default)]
pub struct Buddy {
    /// Open popups per type (more than one only for stack types).
    open: BTreeMap<i32, u32>,
    waiting: Vec<PopupInfo>,
    delayed: Vec<PopupInfo>,
    stack: BTreeSet<i32>,
    dont_show: bool,
}

impl Buddy {
    pub fn new(cfg: &ExeConfig) -> Buddy {
        Buddy {
            stack: cfg.stack_popup_types.iter().copied().collect(),
            dont_show: cfg.dont_show_popups,
            ..Buddy::default()
        }
    }

    /// `RegisterPopupType(type, stack)`.
    pub fn register_type(&mut self, ty: i32, stack: bool) {
        if stack {
            self.stack.insert(ty);
        } else {
            self.stack.remove(&ty);
        }
    }

    pub fn set_dont_show(&mut self, v: bool) {
        self.dont_show = v;
    }

    /// `AddPopup`: the events to fire.
    pub fn add(&mut self, p: PopupInfo) -> Vec<Fire> {
        if self.dont_show {
            self.delayed.push(p);
            return Vec::new();
        }
        self.show_or_wait(p)
    }

    fn show_or_wait(&mut self, p: PopupInfo) -> Vec<Fire> {
        let open = self.open.get(&p.ty).copied().unwrap_or(0);
        if open == 0 || self.stack.contains(&p.ty) {
            *self.open.entry(p.ty).or_insert(0) += 1;
            vec![p.fire()]
        } else {
            self.waiting.push(p);
            Vec::new()
        }
    }

    /// `ProcessedPopup(type)`: shows the first waiting popup of the type, or closes one of the type.
    pub fn processed(&mut self, ty: i32) -> Vec<Fire> {
        if let Some(i) = self.waiting.iter().position(|p| p.ty == ty) {
            let p = self.waiting.remove(i);
            // The type stays open: the new popup replaces the one that was processed.
            return vec![p.fire()];
        }
        if let Some(n) = self.open.get_mut(&ty) {
            *n -= 1;
            if *n == 0 {
                self.open.remove(&ty);
            }
        }
        Vec::new()
    }

    /// A Lua popup says it is on the screen (`SerialEventGameMessagePopupShown`): the type counts as open.
    pub fn shown(&mut self, ty: i32) {
        self.open.entry(ty).or_insert(1);
    }

    /// `ProcessDelayedPopups`.
    pub fn process_delayed(&mut self) -> Vec<Fire> {
        let mut fires = Vec::new();
        let delayed = std::mem::take(&mut self.delayed);
        let mut first = true;
        for p in delayed {
            let nothing_open = self.open.is_empty();
            if (first && nothing_open) || self.stack.contains(&p.ty) {
                *self.open.entry(p.ty).or_insert(0) += 1;
                fires.push(p.fire());
            } else {
                self.delayed.push(p);
            }
            first = false;
        }
        fires
    }

    pub fn is_popup_up(&self) -> bool {
        !self.open.is_empty()
    }

    pub fn is_type_open(&self, ty: i32) -> bool {
        self.open.contains_key(&ty)
    }

    pub fn waiting_len(&self) -> usize {
        self.waiting.len()
    }

    pub fn delayed_len(&self) -> usize {
        self.delayed.len()
    }
}
