//! The Lua side of the controls: the userdata of a control, its methods (`LuaThunk_<Class>_*`),
//! `ContextPtr`, `UIManager`, `TTManager` and the global `LookUpControl`.
//!
//! A method exists for a control only when its class has it in the exe (EUI tests
//! `ctrl.Method and ...`). The bodies of the thunks are not in the dumps beyond their argument
//! checks, so argument shapes follow the usage in the original Lua.

use std::rc::{Rc, Weak};

use glam::Vec2;
use lua_sys::{
    AnyUserData, Function, Lua, LuaError, LuaValue, MetaMethod, MultiValue, Table, UserData,
    UserDataMethods,
};

use crate::flags::*;
use crate::shared::{CtrlRef, Shared};
use crate::state::{State, crc32};
use crate::text::LocArg;
use crate::tree::*;

type Ret<'l> = lua_sys::Result<MultiValue<'l>>;

// ---- which class has which method ----

const BASE: &[&str] = &[
    "BranchResetAnimation",
    "CalculateVisibilityBox",
    "ChangeParent",
    "DestroyAllChildren",
    "EnableToolTip",
    "GetAlpha",
    "GetConsumeMouseButton",
    "GetConsumeMouseOver",
    "GetConsumeMouseWheel",
    "GetID",
    "GetNumChildren",
    "GetOffsetVal",
    "GetOffsetX",
    "GetOffsetY",
    "GetSize",
    "GetSizeVal",
    "GetSizeX",
    "GetSizeY",
    "GetToolTipString",
    "HasMouseOver",
    "IsDisabled",
    "IsHidden",
    "IsModal",
    "IsToolTipEnabled",
    "LocalizeAndSetToolTip",
    "ReleaseChild",
    "ReprocessAnchoring",
    "SetAlpha",
    "SetAnchor",
    "SetColor",
    "SetConsumeMouseButton",
    "SetConsumeMouseOver",
    "SetConsumeMouseWheel",
    "SetDisabled",
    "SetHide",
    "SetModal",
    "SetNoClip",
    "SetOffset",
    "SetOffsetVal",
    "SetOffsetX",
    "SetOffsetY",
    "SetSize",
    "SetSizeVal",
    "SetSizeX",
    "SetSizeY",
    "SetToolTipCallback",
    "SetToolTipString",
    "SetToolTipType",
    "SortChildren",
];
const BUTTON_BASE: &[&str] = &[
    "ClearCallback",
    "GetVoid1",
    "GetVoid2",
    "IsTrackingLeftMouseButton",
    "IsTrackingMiddleMouseButton",
    "IsTrackingRightMouseButton",
    "RegisterCallback",
    "SetVoid1",
    "SetVoid2",
    "SetVoids",
];
const BUTTON: &[&str] = &[
    "GetText",
    "GetTextControl",
    "LocalizeAndSetText",
    "SetText",
    "SetTexture",
    "SetTextureHandle",
    "SetTextureOffset",
    "SetTextureOffsetVal",
];
const GRID_BUTTON: &[&str] = &[
    "DoAutoSize",
    "GetText",
    "GetTextControl",
    "LocalizeAndSetText",
    "SetText",
    "SetTextureOffset",
    "SetTextureOffsetVal",
];
const TEXT_BUTTON: &[&str] = &["LocalizeAndSetText", "SetText"];
const IMAGE: &[&str] = &[
    "NormalizeTexture",
    "Resize",
    "SetTexture",
    "SetTextureAndResize",
    "SetTextureHandle",
    "SetTextureOffset",
    "SetTextureOffsetVal",
    "SetTextureSizeVal",
    "UnloadTexture",
];
const LABEL: &[&str] = &[
    "GetText",
    "LocalizeAndSetText",
    "SetColorByName",
    "SetFontByName",
    "SetForceIME",
    "SetForceNonIME",
    "SetString",
    "SetText",
    "SetTruncateWidth",
    "SetWrapWidth",
];
const ANIM: &[&str] = &[
    "GetProgress",
    "IncrementPause",
    "IsReversing",
    "IsStopped",
    "Play",
    "RegisterAnimCallback",
    "Reverse",
    "SetPauseTime",
    "SetProgress",
    "SetToBeginning",
    "SetToEnd",
    "Stop",
];
const SLIDE: &[&str] = &["SetBeginVal", "SetEndVal"];
const FLIP: &[&str] = &["SetFrame"];
const ALPHA: &[&str] = &["SetTexture", "SetTextureOffset", "SetTextureOffsetVal"];
const SCROLL_ANIM: &[&str] = &[
    "SetMask",
    "SetTexture",
    "SetTextureAndMask",
    "SetTextureOffset",
    "SetTextureOffsetVal",
];
const STACK: &[&str] = &["CalculateSize", "SetWrapWidth"];
const GRID: &[&str] = &["DoAutoSize", "SetTextureOffset", "SetTextureOffsetVal"];
const SCROLL_PANEL: &[&str] = &[
    "CalculateInternalSize",
    "GetDownButton",
    "GetRatio",
    "GetScrollBar",
    "GetScrollValue",
    "GetUpButton",
    "HideScrollBar",
    "RegisterDownEndCallback",
    "RegisterUpEndCallback",
    "SetScrollValue",
];
const SLIDER: &[&str] = &[
    "GetValue",
    "RegisterSliderCallback",
    "SetAndCall",
    "SetValue",
];
const CHECK: &[&str] = &[
    "GetButton",
    "GetTextButton",
    "IsChecked",
    "RegisterCheckHandler",
    "SetAndCall",
    "SetCheck",
];
const PULL_DOWN: &[&str] = &[
    "BuildEntry",
    "CalculateInternals",
    "ClearEntries",
    "GetButton",
    "GetGrid",
    "GetScrollPanel",
    "GetStack",
    "RegisterSelectionCallback",
];
const EDIT_BOX: &[&str] = &[
    "ClearString",
    "GetText",
    "GetVoid1",
    "GetVoid2",
    "HasFocus",
    "LocalizeAndSetText",
    "RegisterCallback",
    "SetText",
    "SetVoid1",
    "SetVoid2",
    "SetVoids",
    "TakeFocus",
];
const METER: &[&str] = &[
    "SetPercent",
    "SetPercents",
    "SetShadowColor",
    "SetShadowPercent",
];
const TEXTURE_BAR: &[&str] = &[
    "SetPercent",
    "SetPercents",
    "SetShadowColor",
    "SetShadowPercent",
    "SetTexture",
];
const BAR: &[&str] = &["SetBGColor", "SetFGColor", "SetPercent"];
const COLOR_BOX: &[&str] = &["SetColorChannel", "SetColorVal"];
const LINE: &[&str] = &["SetColorVal", "SetEndVal", "SetStartVal", "SetWidth"];
const WORLD_ANCHOR: &[&str] = &["SetHexPosition", "SetWorldPosition", "SetWorldPositionVal"];
const REPLAY_MAP: &[&str] = &["SetMapSize", "SetPlot"];
const CONTEXT: &[&str] = &[
    "BuildInstance",
    "BuildInstanceForControl",
    "BuildInstanceForControlAtIndex",
    "CallParentShowHideHandler",
    "CallShowHideHandler",
    "ClearUpdate",
    "DoDeferredLoad",
    "IsHotLoad",
    "LoadNewContext",
    "LookUpControl",
    "Reload",
    "SetInputHandler",
    "SetPostInit",
    "SetShowHideHandler",
    "SetShutdown",
    "SetUpdate",
];

fn class_lists(kind: &Kind) -> Vec<&'static [&'static str]> {
    let mut v: Vec<&'static [&'static str]> = vec![BASE];
    match kind {
        Kind::Container => {}
        Kind::Button(_) => v.extend([BUTTON_BASE, BUTTON]),
        Kind::TextButton(_) => v.extend([BUTTON_BASE, TEXT_BUTTON]),
        Kind::GridButton(_) => v.extend([BUTTON_BASE, GRID_BUTTON]),
        Kind::CheckBox(_) => v.extend([BUTTON_BASE, CHECK]),
        Kind::RadioButton(_) => v.extend([BUTTON_BASE, CHECK, &["SetCheck"]]),
        Kind::ColorBoxButton(_) => v.extend([BUTTON_BASE, COLOR_BOX]),
        Kind::Slider(_) => v.extend([BUTTON_BASE, SLIDER]),
        Kind::Image(_) => v.push(IMAGE),
        Kind::Label(_) => v.push(LABEL),
        Kind::Anim(a) => {
            v.push(ANIM);
            match a.variant {
                AnimVariant::Slide => v.push(SLIDE),
                AnimVariant::Flip => v.push(FLIP),
                AnimVariant::Alpha => v.extend([ALPHA, &["SetSize", "SetSizeVal"]]),
                AnimVariant::Scroll => v.push(SCROLL_ANIM),
            }
        }
        Kind::Context(_) => v.push(CONTEXT),
        Kind::Stack(_) => v.push(STACK),
        Kind::Grid(_) => v.push(GRID),
        Kind::ScrollPanel(_) => v.push(SCROLL_PANEL),
        Kind::Bar(_) => v.push(BAR),
        Kind::Meter(_) => v.push(METER),
        Kind::TextureBar(_) => v.push(TEXTURE_BAR),
        Kind::ColorBox(_) => v.push(COLOR_BOX),
        Kind::EditBox(_) => v.push(EDIT_BOX),
        Kind::PullDown(_) => v.extend([PULL_DOWN, &["SetDisabled"]]),
        Kind::Line(_) => v.push(LINE),
        Kind::WorldAnchor(_) => v.push(WORLD_ANCHOR),
        Kind::ReplayMap(_) => v.push(REPLAY_MAP),
    }
    v
}

/// The method names, with the one `&'static str` the function cache is keyed by.
fn find_method(kind: &Kind, key: &str) -> Option<&'static str> {
    class_lists(kind)
        .into_iter()
        .flat_map(|l| l.iter())
        .find(|m| **m == key)
        .copied()
}

// ---- the userdata ----

impl UserData for CtrlRef {
    fn add_methods<'lua, M: UserDataMethods<'lua, Self>>(m: &mut M) {
        m.add_meta_method(MetaMethod::Index, |lua, this, key: String| {
            let Some(sh) = this.sh.upgrade() else {
                return Ok(None);
            };
            let name = {
                let st = sh.st.borrow();
                let Some(n) = st.tree.get(this.id) else {
                    // A deleted control: any method call is an error with a clear message.
                    drop(st);
                    let f = lua.create_function(|_, _: MultiValue| -> lua_sys::Result<()> {
                        Err(err("control destroyed"))
                    });
                    return Ok(Some(f));
                };
                find_method(&n.kind, &key)
            };
            match name {
                Some(n) => Ok(Some(sh.method_fn(lua, n)?)),
                None => Ok(None),
            }
        });
        m.add_meta_method(MetaMethod::ToString, |_, this, ()| {
            let Some(sh) = this.sh.upgrade() else {
                return Ok("(gone)".to_owned());
            };
            let st = sh.st.borrow();
            Ok(match st.tree.get(this.id) {
                // Unique per control, as the exe's userdata address is: Lua keys tables by `tostring(control)`
                // (TechButtonInclude's g_buttonItemInfo), and the XML ID ("B1") repeats in every instance.
                Some(n) => format!(
                    "{} Control: {} ({}:{})",
                    n.kind.name(),
                    n.id,
                    this.id.index,
                    this.id.ver
                ),
                None => "(destroyed control)".to_owned(),
            })
        });
    }

    fn type_name() -> &'static str {
        "Control"
    }
}

fn err(msg: impl Into<String>) -> LuaError {
    LuaError::msg(msg)
}

/// Arguments after `self`, with the conversions the thunks use.
struct A<'l> {
    lua: &'l Lua,
    v: Vec<LuaValue<'l>>,
    name: &'static str,
}

impl<'l> A<'l> {
    fn get(&self, i: usize) -> &LuaValue<'l> {
        self.v.get(i).unwrap_or(&LuaValue::Nil)
    }

    fn num(&self, i: usize) -> lua_sys::Result<f64> {
        match self.get(i) {
            LuaValue::Number(n) => Ok(*n),
            LuaValue::String(s) => s
                .to_string_lossy()
                .trim()
                .parse::<f64>()
                .map_err(|_| err(format!("{}: argument {} is not a number", self.name, i + 1))),
            LuaValue::Boolean(b) => Ok(f64::from(u8::from(*b))),
            v => Err(err(format!(
                "{}: argument {} must be a number, got {}",
                self.name,
                i + 1,
                v.type_name()
            ))),
        }
    }

    fn f(&self, i: usize) -> lua_sys::Result<f32> {
        self.num(i).map(|n| n as f32)
    }

    fn opt_f(&self, i: usize) -> Option<f32> {
        self.num(i).ok().map(|n| n as f32)
    }

    fn int(&self, i: usize) -> lua_sys::Result<i32> {
        self.num(i).map(|n| n as i32)
    }

    fn str(&self, i: usize) -> lua_sys::Result<String> {
        match self.get(i) {
            LuaValue::String(s) => Ok(s.to_string_lossy().into_owned()),
            LuaValue::Number(n) => Ok(n.to_string()),
            v => Err(err(format!(
                "{}: argument {} must be a string, got {}",
                self.name,
                i + 1,
                v.type_name()
            ))),
        }
    }

    fn opt_str(&self, i: usize) -> Option<String> {
        match self.get(i) {
            LuaValue::Nil => None,
            _ => self.str(i).ok(),
        }
    }

    fn bool(&self, i: usize) -> bool {
        self.get(i).is_truthy()
    }

    fn func(&self, i: usize) -> lua_sys::Result<Function<'l>> {
        match self.get(i) {
            LuaValue::Function(f) => Ok(f.clone()),
            v => Err(err(format!(
                "{}: argument {} must be a function, got {}",
                self.name,
                i + 1,
                v.type_name()
            ))),
        }
    }

    fn control(&self, sh: &Shared, i: usize) -> lua_sys::Result<ControlId> {
        match self.get(i) {
            LuaValue::UserData(u) => {
                let r = u.borrow::<CtrlRef>()?;
                if sh.st.borrow().alive(r.id) {
                    Ok(r.id)
                } else {
                    Err(err("control destroyed"))
                }
            }
            v => Err(err(format!(
                "{}: argument {} must be a control, got {}",
                self.name,
                i + 1,
                v.type_name()
            ))),
        }
    }

    fn table(&self, i: usize) -> lua_sys::Result<Table<'l>> {
        match self.get(i) {
            LuaValue::Table(t) => Ok(t.clone()),
            v => Err(err(format!(
                "{}: argument {} must be a table, got {}",
                self.name,
                i + 1,
                v.type_name()
            ))),
        }
    }

    /// A field of a vector-like value: a table or a userdata with `x`, `y`, `z`, `w`.
    fn field(&self, v: &LuaValue<'l>, key: &str) -> lua_sys::Result<f32> {
        let r: LuaValue<'l> = match v {
            LuaValue::Table(t) => t.get(key)?,
            LuaValue::UserData(_) => {
                let f = self
                    .lua
                    .load("local v, k = ... return v[k]")
                    .set_name("=field")
                    .into_function()?;
                f.call((v.clone(), key))?
            }
            other => {
                return Err(err(format!(
                    "{}: expected a vector, got {}",
                    self.name,
                    other.type_name()
                )));
            }
        };
        Ok(r.as_f64().unwrap_or(0.0) as f32)
    }

    fn vec2(&self, i: usize) -> lua_sys::Result<Vec2> {
        let v = self.get(i).clone();
        Ok(Vec2::new(self.field(&v, "x")?, self.field(&v, "y")?))
    }

    /// Colours come as a vector with x, y, z, w in 0..1, or as one packed number (ABGR).
    fn color(&self, i: usize) -> lua_sys::Result<[f32; 4]> {
        let v = self.get(i).clone();
        if let LuaValue::Number(n) = v {
            let u = n as i64 as u32;
            return Ok([
                (u & 0xff) as f32 / 255.0,
                ((u >> 8) & 0xff) as f32 / 255.0,
                ((u >> 16) & 0xff) as f32 / 255.0,
                ((u >> 24) & 0xff) as f32 / 255.0,
            ]);
        }
        Ok([
            self.field(&v, "x")?,
            self.field(&v, "y")?,
            self.field(&v, "z")?,
            self.field(&v, "w")?,
        ])
    }

    /// The arguments of `LocalizeAndSet*`: key, then bool, number, string.
    fn loc_args(&self, from: usize) -> lua_sys::Result<Vec<LocArg>> {
        let mut out = Vec::new();
        for i in from..self.v.len() {
            out.push(match &self.v[i] {
                LuaValue::Boolean(b) => LocArg::Bool(*b),
                LuaValue::Number(n) => LocArg::Number(*n),
                LuaValue::String(s) => LocArg::Str(s.to_string_lossy().into_owned()),
                other => {
                    return Err(err(format!(
                        "{}: argument {} has an unsupported type {}",
                        self.name,
                        i + 1,
                        other.type_name()
                    )));
                }
            });
        }
        Ok(out)
    }
}

fn none<'l>() -> Ret<'l> {
    Ok(MultiValue::new())
}

fn one<'l, T: lua_sys::ToLua<'l>>(lua: &'l Lua, v: T) -> Ret<'l> {
    Ok(MultiValue::from_vec(vec![v.to_lua(lua)?]))
}

fn two<'l, T: lua_sys::ToLua<'l>, U: lua_sys::ToLua<'l>>(lua: &'l Lua, a: T, b: U) -> Ret<'l> {
    Ok(MultiValue::from_vec(vec![a.to_lua(lua)?, b.to_lua(lua)?]))
}

fn vector<'l>(lua: &'l Lua, x: f32, y: f32) -> lua_sys::Result<Table<'l>> {
    let t = lua.create_table();
    t.set("x", x)?;
    t.set("y", y)?;
    Ok(t)
}

/// The entry point of every control method.
pub fn call_method<'l>(
    sh: &Rc<Shared>,
    lua: &'l Lua,
    name: &'static str,
    args: MultiValue<'l>,
) -> Ret<'l> {
    let mut v = args.into_vec();
    if v.is_empty() {
        return Err(err(format!("{name} must be called on a control (use ':')")));
    }
    let first = v.remove(0);
    let id = match &first {
        LuaValue::UserData(u) => u.borrow::<CtrlRef>()?.id,
        _ => return Err(err(format!("{name} must be called on a control (use ':')"))),
    };
    if !sh.st.borrow().alive(id) {
        return Err(err(format!("{name}: control destroyed")));
    }
    let a = A { lua, v, name };
    let r = dispatch(sh, lua, id, name, &a);
    sh.flush();
    r
}

/// The text control of a control that has one.
fn text_of(st: &State, id: ControlId) -> Option<ControlId> {
    match &st.tree[id].kind {
        Kind::Label(_) => Some(id),
        Kind::Button(b) => b.text,
        Kind::GridButton(g) => g.text,
        Kind::TextButton(t) => Some(t.label),
        Kind::EditBox(e) => Some(e.label),
        _ => None,
    }
}

fn label_text(st: &State, id: ControlId) -> String {
    match &st.tree[id].kind {
        Kind::Label(l) => l.text.clone(),
        _ => String::new(),
    }
}

/// `SetText` and its relatives for the classes that show text.
fn set_text(st: &mut State, id: ControlId, text: &str) {
    match st.tree[id].kind {
        Kind::Button(_) => {
            let t = match &st.tree[id].kind {
                Kind::Button(b) => b.text,
                _ => None,
            };
            let t = match t {
                Some(t) => t,
                None => {
                    let owner = st.tree[id].owner;
                    let t = st.new_node(Kind::Label(Box::default()), owner);
                    st.tree[t].parent = Some(id);
                    st.tree[t].owner = owner;
                    if st.tree[id].flags & button_flag::TEXT_ANCHOR == 0 {
                        st.tree[t].flags |= H_CENTER | V_CENTER;
                    }
                    if let Kind::Button(b) = &mut st.tree[id].kind {
                        b.text = Some(t);
                    }
                    if st.tree[id].flags & INITIALIZED != 0 {
                        st.initialize_plain(t);
                    }
                    t
                }
            };
            st.tree[id].flags |= button_flag::HAS_TEXT;
            st.label_change(t, |l| l.text = text.to_owned());
        }
        Kind::GridButton(_) => {
            let t = match &st.tree[id].kind {
                Kind::GridButton(g) => g.text,
                _ => None,
            };
            let t = match t {
                Some(t) => t,
                None => {
                    let owner = st.tree[id].owner;
                    let t = st.new_node(Kind::Label(Box::default()), owner);
                    st.tree[t].parent = Some(id);
                    st.tree[t].flags |= H_CENTER | V_CENTER;
                    if let Kind::GridButton(g) = &mut st.tree[id].kind {
                        g.text = Some(t);
                    }
                    if st.tree[id].flags & INITIALIZED != 0 {
                        st.initialize_plain(t);
                    }
                    t
                }
            };
            st.tree[id].flags |= button_flag::HAS_TEXT;
            st.label_change(t, |l| l.text = text.to_owned());
        }
        Kind::TextButton(_) => {
            if let Some(t) = text_of(st, id) {
                st.label_change(t, |l| l.text = text.to_owned());
                // TextButtonControl: the button takes the size of its text.
                let s = st.tree[t].size;
                st.tree[id].size = s;
                st.reprocess_anchoring(id);
            }
        }
        Kind::Label(_) => st.label_change(id, |l| l.text = text.to_owned()),
        Kind::EditBox(_) => {
            if let Kind::EditBox(e) = &mut st.tree[id].kind {
                e.text = text.to_owned();
                e.cursor = text.chars().count();
            }
            if let Some(t) = text_of(st, id) {
                st.label_change(t, |l| l.text = text.to_owned());
            }
        }
        _ => {}
    }
}

fn dispatch<'l>(
    sh: &Rc<Shared>,
    lua: &'l Lua,
    id: ControlId,
    name: &'static str,
    a: &A<'l>,
) -> Ret<'l> {
    let mut guard = sh.st.borrow_mut();
    let st: &mut State = &mut guard;
    match name {
        // ---- ControlBase ----
        "BranchResetAnimation" => st.branch_reset_animation(id),
        "CalculateVisibilityBox" => st.calc_vis_box(id),
        "ChangeParent" => {
            drop(guard);
            let p = a.control(sh, 0)?;
            sh.run(|st| st.change_parent(id, p));
            return none();
        }
        "DestroyAllChildren" => {
            st.release_all_children(id);
            drop(guard);
            sh.sweep_userdata();
            return none();
        }
        "EnableToolTip" => {
            let on = a.bool(0);
            let n = &mut st.tree[id];
            if on {
                if n.flags & HAS_TOOLTIP == 0 && st.tooltips.current != Some(id) {
                    st.tooltips.recheck = true;
                }
                let n = &mut st.tree[id];
                n.flags |= HAS_TOOLTIP;
            } else {
                n.flags &= !HAS_TOOLTIP;
            }
        }
        "IsToolTipEnabled" => return one(lua, st.tree[id].flags & HAS_TOOLTIP != 0),
        "GetAlpha" => return one(lua, st.tree[id].alpha),
        "SetAlpha" => st.tree[id].alpha = a.f(0)?,
        "GetConsumeMouseButton" => return one(lua, st.tree[id].flags & CONSUME_BUTTON != 0),
        "GetConsumeMouseOver" => return one(lua, st.tree[id].flags & CONSUME_OVER != 0),
        "GetConsumeMouseWheel" => return one(lua, st.tree[id].flags & CONSUME_WHEEL != 0),
        "SetConsumeMouseButton" | "SetConsumeMouseOver" | "SetConsumeMouseWheel" => {
            let bit = match name {
                "SetConsumeMouseButton" => CONSUME_BUTTON,
                "SetConsumeMouseOver" => CONSUME_OVER,
                _ => CONSUME_WHEEL,
            };
            let on = a.bool(0);
            let n = &mut st.tree[id];
            if on {
                n.flags |= bit;
            } else {
                n.flags &= !bit;
            }
        }
        "GetID" => return one(lua, st.tree[id].id.clone()),
        "GetNumChildren" => return one(lua, st.tree[id].children.len() as f64),
        "GetOffsetVal" => {
            let o = st.tree[id].offset;
            return two(lua, o.x, o.y);
        }
        "GetOffsetX" => return one(lua, st.tree[id].offset.x),
        "GetOffsetY" => return one(lua, st.tree[id].offset.y),
        "GetSize" => {
            let s = st.tree[id].size;
            return one(lua, vector(lua, s.x, s.y)?);
        }
        "GetSizeVal" => {
            let s = st.tree[id].size;
            return two(lua, s.x, s.y);
        }
        "GetSizeX" => return one(lua, st.tree[id].size.x),
        "GetSizeY" => return one(lua, st.tree[id].size.y),
        "GetToolTipString" => return one(lua, st.tree[id].tt_string.clone().unwrap_or_default()),
        "LocalizeAndSetToolTip" => {
            let key = a.opt_str(0);
            match key {
                None => st.set_tooltip_string(id, None),
                Some(k) => {
                    let args = a.loc_args(1)?;
                    let t = st.localize(&k, &args);
                    st.set_tooltip_string(id, Some(t));
                }
            }
        }
        "SetToolTipString" => {
            let t = a.opt_str(0);
            st.set_tooltip_string(id, t);
        }
        "SetToolTipType" => {
            let t = a.opt_str(0);
            let n = &mut st.tree[id];
            match t {
                Some(t) => {
                    let was = n.flags & HAS_TOOLTIP != 0;
                    n.flags |= HAS_TOOLTIP;
                    n.tt_crc = crc32(&t);
                    if !was && st.tooltips.current != Some(id) {
                        st.tooltips.recheck = true;
                    }
                }
                None => {
                    n.flags &= !HAS_TOOLTIP;
                    n.tt_crc = 0;
                }
            }
            st.refresh_tooltip(id);
        }
        "SetToolTipCallback" => {
            let f = a.func(0)?;
            let key = Rc::new(lua.create_registry_value(f)?);
            st.tree[id].tt_callback = Some(key);
        }
        "HasMouseOver" => return one(lua, st.tree[id].flags & MOUSE_OVER != 0),
        "IsDisabled" => return one(lua, st.tree[id].flags & DISABLED != 0),
        "IsHidden" => return one(lua, st.tree[id].flags & HIDDEN != 0),
        "IsModal" => return one(lua, st.is_modal(id)),
        "ReleaseChild" => {
            drop(guard);
            let c = a.control(sh, 0)?;
            sh.run(|st| st.release_child(id, c));
            sh.sweep_userdata();
            return none();
        }
        "ReprocessAnchoring" => st.reprocess_anchoring(id),
        "SetAnchor" => {
            let s = a.str(0)?;
            st.set_anchor(id, &s);
        }
        "SetColor" => {
            if matches!(st.tree[id].kind, Kind::Label(_)) {
                let c = a.color(0)?;
                let slot = a.int(1).unwrap_or(0).clamp(0, 2) as usize;
                st.label_change(id, |l| {
                    l.colors.colors[slot] = c;
                    l.color_explicit = true;
                });
            } else {
                let c = a.color(0)?;
                st.set_color(id, c);
            }
        }
        "SetDisabled" => {
            let d = a.bool(0);
            st.set_disabled(id, d);
            if let Kind::PullDown(p) = &st.tree[id].kind
                && let Some(m) = p.main
            {
                st.set_disabled(m, d);
            }
        }
        "SetHide" => {
            let h = a.bool(0);
            drop(guard);
            sh.set_hide(id, h);
            return none();
        }
        "SetModal" => {
            let on = a.bool(0);
            let n = &mut st.tree[id];
            if on {
                n.flags |= MODAL;
            } else {
                n.flags &= !MODAL;
            }
        }
        "SetNoClip" => {
            let on = a.bool(0);
            let n = &mut st.tree[id];
            if on {
                n.flags |= NO_CLIP;
            } else {
                n.flags &= !NO_CLIP;
            }
        }
        "SetOffset" => {
            let v = a.vec2(0)?;
            st.set_offset(id, v.x, v.y);
        }
        "SetOffsetVal" => {
            let (x, y) = (a.f(0)?, a.f(1)?);
            st.set_offset(id, x, y);
        }
        "SetOffsetX" => {
            let (x, y) = (a.f(0)?, st.tree[id].offset.y);
            st.set_offset(id, x, y);
        }
        "SetOffsetY" => {
            let (x, y) = (st.tree[id].offset.x, a.f(0)?);
            st.set_offset(id, x, y);
        }
        "SetSize" => {
            let v = a.vec2(0)?;
            st.set_size_val(id, v.x, v.y);
        }
        "SetSizeVal" => {
            let (w, h) = (a.f(0)?, a.f(1)?);
            st.set_size_val(id, w, h);
        }
        "SetSizeX" => {
            let (w, h) = (a.f(0)?, st.tree[id].size.y);
            st.set_size_val(id, w, h);
        }
        "SetSizeY" => {
            let (w, h) = (st.tree[id].size.x, a.f(0)?);
            st.set_size_val(id, w, h);
        }
        "SortChildren" => {
            // SortChildren(fn(a, b) -> bool): a stable sort by the Lua order.
            let f = a.func(0)?;
            let kids = st.tree[id].children.clone();
            drop(guard);
            let mut sorted = kids.clone();
            let mut failure = None;
            // Insertion sort keeps the number of Lua calls small and the order stable.
            for i in 1..sorted.len() {
                let mut j = i;
                while j > 0 {
                    let (l, r) = (
                        sh.push_control(lua, sorted[j])?,
                        sh.push_control(lua, sorted[j - 1])?,
                    );
                    match f.call::<_, bool>((l, r)) {
                        Ok(true) => {
                            sorted.swap(j, j - 1);
                            j -= 1;
                        }
                        Ok(false) => break,
                        Err(e) => {
                            failure = Some(e);
                            break;
                        }
                    }
                }
                if failure.is_some() {
                    break;
                }
            }
            if let Some(e) = failure {
                return Err(e);
            }
            sh.with(|st| {
                if st.alive(id) && st.tree[id].children.len() == sorted.len() {
                    st.tree[id].children = sorted;
                }
            });
            return none();
        }
        _ => {
            return dispatch_class(sh, lua, id, name, a, guard);
        }
    }
    drop(guard);
    none()
}

fn dispatch_class<'l>(
    sh: &Rc<Shared>,
    lua: &'l Lua,
    id: ControlId,
    name: &'static str,
    a: &A<'l>,
    mut guard: std::cell::RefMut<'_, State>,
) -> Ret<'l> {
    let st: &mut State = &mut guard;
    let scr = st.screen();
    match name {
        // ---- ButtonControlBase ----
        "ClearCallback" => {
            let slot = a.int(0)?.clamp(0, 7) as usize;
            if let Some(b) = st.tree[id].kind.button_base_mut() {
                b.callbacks[slot] = None;
            }
        }
        "GetVoid1" | "GetVoid2" => {
            let v = match &st.tree[id].kind {
                Kind::EditBox(e) => {
                    if name == "GetVoid1" {
                        e.void1
                    } else {
                        e.void2
                    }
                }
                k => k
                    .button_base()
                    .map(|b| if name == "GetVoid1" { b.void1 } else { b.void2 })
                    .unwrap_or(0),
            };
            return one(lua, v);
        }
        "SetVoid1" | "SetVoid2" | "SetVoids" => {
            // The exe reads the voids with lua_tointeger: a nil (Lua of the original that sets a void to an undefined
            // global, EconomicGeneralInfo, NotificationLogPopup) is 0, not an error (SIMPLIFIED(SIMP-exe-360)).
            let (x, y) = (a.int(0).unwrap_or(0), a.int(1).unwrap_or(0));
            if let Kind::EditBox(e) = &mut st.tree[id].kind {
                match name {
                    "SetVoid1" => e.void1 = x,
                    "SetVoid2" => e.void2 = x,
                    _ => {
                        e.void1 = x;
                        e.void2 = y;
                    }
                }
            } else if let Some(b) = st.tree[id].kind.button_base_mut() {
                match name {
                    "SetVoid1" => b.void1 = x,
                    "SetVoid2" => b.void2 = x,
                    _ => {
                        b.void1 = x;
                        b.void2 = y;
                    }
                }
            }
        }
        "IsTrackingLeftMouseButton" => {
            // A slider tracks the left button while its shuttle is dragged (`SliderControl` keeps no LEFT_DOWN flag: its
            // `dragging` is the tracking). CityView's queue drag (SwapQueueItem) asks QueueSlider for it.
            let down = match &st.tree[id].kind {
                Kind::Slider(s) => s.dragging,
                _ => st.tree[id].flags & btn::LEFT_DOWN != 0,
            };
            return one(lua, down);
        }
        "IsTrackingRightMouseButton" => return one(lua, st.tree[id].flags & btn::RIGHT_DOWN != 0),
        "IsTrackingMiddleMouseButton" => {
            return one(lua, st.tree[id].flags & btn::MIDDLE_DOWN != 0);
        }
        "RegisterCallback" => {
            if let Kind::EditBox(_) = st.tree[id].kind {
                let f = a.func(0)?;
                let k = Rc::new(lua.create_registry_value(f)?);
                if let Kind::EditBox(e) = &mut st.tree[id].kind {
                    e.callback = Some(k);
                }
            } else {
                // RegisterCallback(Mouse.eLClick, fn): the slot is the value of the Mouse enum.
                let slot = a.int(0)?;
                let f = a.func(1)?;
                let k = Rc::new(lua.create_registry_value(f)?);
                let slot = if (0..8).contains(&slot) {
                    slot as usize
                } else {
                    7
                };
                if let Some(b) = st.tree[id].kind.button_base_mut() {
                    b.callbacks[slot] = Some(k);
                }
            }
        }

        // ---- text ----
        "GetText" => {
            let s = match &st.tree[id].kind {
                Kind::EditBox(e) => e.text.clone(),
                _ => text_of(st, id)
                    .map(|t| label_text(st, t))
                    .unwrap_or_default(),
            };
            return one(lua, s);
        }
        "GetTextControl" | "GetTextButton" => {
            let t = if name == "GetTextButton" {
                match &st.tree[id].kind {
                    Kind::CheckBox(c) | Kind::RadioButton(c) => Some(c.label),
                    _ => None,
                }
            } else {
                // SIMPLIFIED(SIMP-exe-70): a Button/GridButton without a `String` has no text control until the first
                // `GetTextControl`/`SetText`; EUI calls `GetTextControl():SetAnchor` on such headers (CityView), so the
                // call makes an empty label (as the exe's always-present text object would be).
                if text_of(st, id).is_none()
                    && matches!(st.tree[id].kind, Kind::Button(_) | Kind::GridButton(_))
                {
                    set_text(st, id, "");
                    st.tree[id].flags &= !button_flag::HAS_TEXT;
                }
                text_of(st, id)
            };
            drop(guard);
            return match t {
                Some(t) => one(lua, sh.push_control(lua, t)?),
                None => none(),
            };
        }
        "SetText" | "SetString" => {
            let s = a.opt_str(0).unwrap_or_default();
            if let Kind::CheckBox(c) | Kind::RadioButton(c) = &st.tree[id].kind {
                let l = c.label;
                set_text(st, l, &s);
            } else {
                set_text(st, id, &s);
            }
        }
        "LocalizeAndSetText" => {
            let text = match a.opt_str(0) {
                None => String::new(),
                Some(k) => {
                    let args = a.loc_args(1)?;
                    st.localize(&k, &args)
                }
            };
            set_text(st, id, &text);
        }
        "SetColorByName" => {
            let n = a.str(0)?;
            // The name is a colour set of `ColorAtlas.xml` (`Gray_Black`: text, shadow; `UIManager::GetColorSetByName`);
            // a plain colour name still colours the text. M5 WO-I: the trade table greys what cannot be offered.
            if let Some(set) = st.styles.color_set(&n) {
                st.label_change(id, |l| {
                    l.colors = set;
                    l.color_explicit = true;
                });
            } else if let Some(c) = st.styles.color(&n) {
                st.label_change(id, |l| {
                    l.colors.colors[0] = c;
                    l.color_explicit = true;
                });
            }
        }
        "SetFontByName" => {
            let f = a.str(0)?;
            st.label_change(id, |l| l.font = Some(f));
        }
        "SetForceIME" | "SetForceNonIME" => {}
        "SetTruncateWidth" => {
            let w = a.int(0)?;
            st.label_change(id, |l| {
                l.truncate_width = w;
                l.wrap_width = -1;
            });
        }
        "SetWrapWidth" => {
            let w = a.int(0)?;
            if matches!(st.tree[id].kind, Kind::Stack(_)) {
                if let Kind::Stack(s) = &mut st.tree[id].kind {
                    s.wrap_width = w;
                }
            } else {
                st.label_change(id, |l| {
                    l.wrap_width = w;
                    l.truncate_width = -1;
                });
            }
        }
        "ClearString" => {
            set_text(st, id, "");
        }

        // ---- textures ----
        "SetTexture" | "SetTextureAndResize" => {
            let n = a.str(0)?;
            let t = st.texture(&n);
            let ok = t.is_some();
            match name {
                "SetTextureAndResize" => {
                    let r = st.image_set_texture_and_resize(id, &n);
                    drop(guard);
                    return one(lua, r);
                }
                _ => {
                    if let Some(t) = t {
                        match &mut st.tree[id].kind {
                            Kind::Image(i) => {
                                i.tex_w = t.w;
                                i.tex_h = t.h;
                                i.quad.set_texture(Some(t), None);
                            }
                            Kind::Button(b) => {
                                b.quad.set_texture(Some(t), None);
                                st.tree[id].flags &= !button_flag::NO_QUAD;
                                let s = st.tree[id]
                                    .kind
                                    .button_base()
                                    .map(|b| b.vis_state)
                                    .unwrap_or(0);
                                st.set_vis_state(id, s);
                            }
                            Kind::Anim(an) => {
                                an.quad
                                    .get_or_insert_with(Quad::default)
                                    .set_texture(Some(t), None);
                            }
                            Kind::TextureBar(b) => b.quad.set_texture(Some(t), None),
                            _ => {}
                        }
                    }
                    drop(guard);
                    return one(lua, ok);
                }
            }
        }
        "SetMask" | "SetTextureAndMask" => {
            let (tn, mn) = if name == "SetMask" {
                (None, a.str(0)?)
            } else {
                (Some(a.str(0)?), a.str(1)?)
            };
            let t = tn.and_then(|n| st.texture(&n));
            let m = st.texture(&mn);
            if let Kind::Anim(an) = &mut st.tree[id].kind {
                an.quad.get_or_insert_with(Quad::default).set_texture(t, m);
            }
        }
        "SetTextureHandle" => {
            let h = a.int(0)?;
            // The minimap texture is handle 1 (`TextureId::MINIMAP`); others are renderer-owned.
            let tid = draw_list::TextureId::external(h.max(0) as u64);
            if let Kind::Image(i) = &mut st.tree[id].kind {
                i.quad.texture = None;
                i.quad.handle = Some(tid);
                i.quad.uv_scale = Vec2::ONE;
                i.quad.uv_offset = Vec2::ZERO;
            } else if let Kind::Button(b) = &mut st.tree[id].kind {
                b.quad.texture = None;
                b.quad.handle = Some(tid);
                // The button had no texture, so Initialize hid its quad; a handle gives it one (the minimap button).
                st.tree[id].flags &= !button_flag::NO_QUAD;
            }
        }
        "SetTextureOffset" | "SetTextureOffsetVal" => {
            let v = if name == "SetTextureOffset" {
                a.vec2(0)?
            } else {
                Vec2::new(a.f(0)?, a.f(1)?)
            };
            let no_state = st.tree[id].flags & button_flag::NO_STATE_CHANGE != 0;
            match &mut st.tree[id].kind {
                Kind::Image(i) => {
                    i.quad.src_x = v.x;
                    i.quad.src_y = v.y;
                    i.quad.normalize();
                }
                Kind::Button(b) => {
                    b.tex_offset = v;
                    if no_state {
                        b.quad.src_x = v.x;
                        b.quad.src_y = v.y;
                        b.quad.normalize();
                    }
                }
                Kind::Grid(_) => grid_texture_offset(st, id, v),
                Kind::GridButton(g) => {
                    let grid = g.grid;
                    grid_texture_offset(st, grid, v);
                }
                Kind::Anim(an) => {
                    if let Some(q) = &mut an.quad {
                        q.src_x = v.x;
                        q.src_y = v.y;
                        q.normalize();
                    }
                }
                _ => {}
            }
        }
        "NormalizeTexture" => {
            if let Kind::Image(i) = &mut st.tree[id].kind {
                i.quad.normalize();
            }
        }
        "Resize" => {
            let (w, h) = (a.f(0)?, a.f(1)?);
            st.image_resize(id, w, h);
        }
        "SetTextureSizeVal" => {
            let (w, h) = (a.f(0)?, a.f(1)?);
            if let Kind::Image(i) = &mut st.tree[id].kind {
                i.tex_w = w;
                i.tex_h = h;
            }
        }
        "UnloadTexture" => {
            if let Kind::Image(i) = &mut st.tree[id].kind {
                i.quad.texture = None;
            }
        }

        // ---- animations ----
        "GetProgress" => {
            let p = match &st.tree[id].kind {
                Kind::Anim(an) => an.progress,
                _ => 0.0,
            };
            return one(lua, p);
        }
        "SetProgress" => {
            let p = a.f(0)?;
            if let Kind::Anim(an) = &mut st.tree[id].kind {
                an.progress = p;
            }
            let eased = st.anim_function_out(id, p);
            st.anim_update(id, eased);
        }
        "IncrementPause" => {
            let d = a.f(0)?;
            if let Kind::Anim(an) = &mut st.tree[id].kind {
                an.pause_remaining += d;
            }
        }
        "SetPauseTime" => {
            let d = a.f(0)?;
            if let Kind::Anim(an) = &mut st.tree[id].kind {
                an.pause = d;
                an.pause_remaining = d;
            }
        }
        "IsReversing" => {
            let r = matches!(&st.tree[id].kind, Kind::Anim(an) if an.reversing);
            return one(lua, r);
        }
        "IsStopped" => {
            let r = matches!(&st.tree[id].kind, Kind::Anim(an) if an.stopped);
            return one(lua, r);
        }
        "Play" => st.anim_play(id),
        "Stop" => st.anim_stop(id),
        "Reverse" => st.anim_reverse(id),
        "SetToBeginning" => st.anim_to_beginning(id),
        "SetToEnd" => st.anim_to_end(id),
        "RegisterAnimCallback" => {
            let f = a.func(0)?;
            let k = Rc::new(lua.create_registry_value(f)?);
            if let Kind::Anim(an) = &mut st.tree[id].kind {
                an.callback = Some(k);
            }
        }
        "SetBeginVal" | "SetEndVal" => {
            let v = Vec2::new(a.f(0)?, a.f(1)?);
            if let Kind::Anim(an) = &mut st.tree[id].kind {
                if name == "SetBeginVal" {
                    an.slide_begin = v;
                } else {
                    an.slide_end = v;
                }
            }
        }
        "SetFrame" => {
            let f = a.int(0)?;
            st.flip_set_frame(id, f);
        }

        // ---- stack, grid ----
        "CalculateSize" => st.stack_calculate_size(id),
        "DoAutoSize" => {
            if let Kind::GridButton(g) = &st.tree[id].kind {
                let g = g.grid;
                // GridButtonControl::DoAutoSize
                let n = &st.tree[id];
                let mut max = Vec2::ZERO;
                for &c in &n.children {
                    if st.tree[c].flags & 0x240 == 0 {
                        max = max.max(st.tree[c].full);
                    }
                }
                let margin = match &st.tree[g].kind {
                    Kind::Grid(gr) => gr.auto_margin,
                    _ => Vec2::ZERO,
                };
                let size = margin * 2.0 + max;
                let n = &mut st.tree[id];
                n.size = size;
                n.full = n.offset.abs() + size;
                st.reprocess_anchoring(id);
                st.set_size_val(g, size.x, size.y);
            } else {
                st.grid_do_auto_size(id);
            }
        }

        // ---- scroll panel ----
        "CalculateInternalSize" => st.scroll_panel_calc_internal(id),
        "GetScrollValue" => {
            let v = match &st.tree[id].kind {
                Kind::ScrollPanel(s) => s.value,
                _ => 0.0,
            };
            return one(lua, v);
        }
        "SetScrollValue" => {
            let v = a.f(0)?;
            st.scroll_set_value(id, v);
        }
        "GetRatio" => {
            let v = match &st.tree[id].kind {
                Kind::ScrollPanel(s) => s.ratio,
                _ => 1.0,
            };
            return one(lua, v);
        }
        "HideScrollBar" => {
            let h = a.bool(0);
            let n = &mut st.tree[id];
            if h {
                n.flags |= sp_flag::BAR_HIDDEN;
            } else {
                n.flags &= !sp_flag::BAR_HIDDEN;
            }
        }
        "GetUpButton" | "GetDownButton" | "GetScrollBar" => {
            let t = match &st.tree[id].kind {
                Kind::ScrollPanel(s) => match name {
                    "GetUpButton" => s.up,
                    "GetDownButton" => s.down,
                    _ => s.bar,
                },
                _ => None,
            };
            drop(guard);
            return match t {
                Some(t) => one(lua, sh.push_control(lua, t)?),
                None => none(),
            };
        }
        "RegisterUpEndCallback" | "RegisterDownEndCallback" => {
            let f = a.func(0)?;
            let k = Rc::new(lua.create_registry_value(f)?);
            if let Kind::ScrollPanel(s) = &mut st.tree[id].kind {
                if name == "RegisterUpEndCallback" {
                    s.up_end = Some(k);
                } else {
                    s.down_end = Some(k);
                }
            }
        }

        // ---- slider ----
        "GetValue" => {
            let v = match &st.tree[id].kind {
                Kind::Slider(s) => s.value,
                _ => 0.0,
            };
            return one(lua, v);
        }
        "SetValue" => {
            let v = a.f(0)?;
            st.slider_set_value(id, v);
        }
        "RegisterSliderCallback" => {
            let f = a.func(0)?;
            let k = Rc::new(lua.create_registry_value(f)?);
            if let Kind::Slider(s) = &mut st.tree[id].kind {
                s.callback = Some(k);
            }
        }

        // ---- check box ----
        "IsChecked" => {
            let c =
                matches!(&st.tree[id].kind, Kind::CheckBox(c) | Kind::RadioButton(c) if c.checked);
            return one(lua, c);
        }
        "SetCheck" => {
            let v = a.bool(0);
            if let Kind::CheckBox(c) | Kind::RadioButton(c) = &mut st.tree[id].kind {
                c.checked = v;
            }
        }
        "RegisterCheckHandler" => {
            let f = a.func(0)?;
            let k = Rc::new(lua.create_registry_value(f)?);
            if let Kind::CheckBox(c) | Kind::RadioButton(c) = &mut st.tree[id].kind {
                c.handler = Some(k);
            }
        }
        "SetAndCall" => {
            let v = a.get(0).clone();
            if matches!(st.tree[id].kind, Kind::Slider(_)) {
                let f = v.as_f64().unwrap_or(0.0) as f32;
                st.slider_set_and_call(id, f);
            } else {
                st.check_set_and_call(id, v.is_truthy());
            }
        }
        "GetButton" => {
            let t = match &st.tree[id].kind {
                Kind::CheckBox(c) | Kind::RadioButton(c) => Some(c.boxed),
                Kind::PullDown(p) => p.main,
                _ => None,
            };
            drop(guard);
            return match t {
                Some(t) => one(lua, sh.push_control(lua, t)?),
                None => none(),
            };
        }

        // ---- pull-down ----
        "GetGrid" | "GetScrollPanel" | "GetStack" => {
            let t = match &st.tree[id].kind {
                Kind::PullDown(p) => Some(match name {
                    "GetGrid" => p.grid,
                    "GetScrollPanel" => p.scroll,
                    _ => p.stack,
                }),
                _ => None,
            };
            drop(guard);
            return match t {
                Some(t) => one(lua, sh.push_control(lua, t)?),
                None => none(),
            };
        }
        "RegisterSelectionCallback" => {
            let f = a.func(0)?;
            let k = Rc::new(lua.create_registry_value(f)?);
            if let Kind::PullDown(p) = &mut st.tree[id].kind {
                p.callback = Some(k);
            }
        }
        "ClearEntries" => {
            st.pull_down_clear(id);
            drop(guard);
            sh.sweep_userdata();
            return none();
        }
        "BuildEntry" => {
            let inst = a.str(0)?;
            let table = a.table(1)?;
            let (stack, el) = match &st.tree[id].kind {
                Kind::PullDown(p) => (p.stack, p.instances.get(&inst).cloned()),
                _ => return none(),
            };
            let Some(el) = el else {
                drop(guard);
                return none();
            };
            let owner = st.tree[id].owner;
            let mut ids = Vec::new();
            let mark = st.touch(&el);
            let c = st.build_control(&el, owner);
            st.finish_touched(mark);
            if let Some(c) = c {
                st.expose_list(c, &mut ids);
                st.add_child(stack, c);
                if let Some(b) = st.tree[c].kind.button_base_mut() {
                    b.native = Native::PullDownSub(id);
                }
                st.initialize_plain(c);
                if let Kind::PullDown(p) = &mut st.tree[id].kind {
                    p.entries.push(c);
                }
            }
            drop(guard);
            for i in &ids {
                let n = sh.st.borrow().node(*i).id.clone();
                table.set(n, sh.push_control(lua, *i)?)?;
            }
            return match c {
                Some(c) => one(lua, sh.push_control(lua, c)?),
                None => none(),
            };
        }
        "CalculateInternals" => st.pull_down_calculate(id),

        // ---- edit box ----
        "HasFocus" => return one(lua, st.focus == Some(id)),
        "TakeFocus" => st.focus = Some(id),

        // ---- bars ----
        "SetPercent" | "SetPercents" | "SetShadowPercent" => {
            let (p, q) = (a.f(0)?, a.opt_f(1));
            match &mut st.tree[id].kind {
                Kind::Bar(b) => b.percent = p,
                Kind::Meter(m) => {
                    if name != "SetShadowPercent" {
                        m.percent = p;
                    }
                    if name == "SetShadowPercent" {
                        m.shadow_percent = p;
                    } else if let Some(q) = q {
                        m.shadow_percent = q;
                    }
                }
                Kind::TextureBar(b) => {
                    if name != "SetShadowPercent" {
                        b.percent = p;
                    }
                    if name == "SetShadowPercent" {
                        b.shadow_percent = p;
                    } else if let Some(q) = q {
                        b.shadow_percent = q;
                    }
                }
                _ => {}
            }
        }
        "SetShadowColor" => {
            let c = a.color(0)?;
            match &mut st.tree[id].kind {
                Kind::Meter(m) => m.shadow_color = c,
                Kind::TextureBar(b) => b.shadow_color = c,
                _ => {}
            }
        }
        "SetFGColor" | "SetBGColor" => {
            let c = a.color(0)?;
            if let Kind::Bar(b) = &mut st.tree[id].kind {
                if name == "SetFGColor" {
                    b.fg = c;
                } else {
                    b.bg = c;
                    st.tree[id].flags |= 0x40_0000;
                }
            }
        }
        "SetColorVal" => {
            let c = [a.f(0)?, a.f(1)?, a.f(2)?, a.opt_f(3).unwrap_or(1.0)];
            match &mut st.tree[id].kind {
                Kind::ColorBox(b) => b.color = c,
                Kind::ColorBoxButton(b) => b.1.color = c,
                Kind::Line(l) => l.color = c,
                _ => {}
            }
        }
        "SetColorChannel" => {
            let (ch, v) = (a.int(0)?.clamp(0, 3) as usize, a.f(1)?);
            match &mut st.tree[id].kind {
                Kind::ColorBox(b) => b.color[ch] = v,
                Kind::ColorBoxButton(b) => b.1.color[ch] = v,
                _ => {}
            }
        }
        "SetStartVal" | "SetEndVal2" => {
            let v = Vec2::new(a.f(0)?, a.f(1)?);
            if let Kind::Line(l) = &mut st.tree[id].kind {
                l.start = v;
            }
        }
        "SetWidth" => {
            let w = a.f(0)?;
            if let Kind::Line(l) = &mut st.tree[id].kind {
                l.width = w;
            }
        }

        // ---- world anchor, replay map ----
        "SetWorldPosition" | "SetWorldPositionVal" | "SetHexPosition" => {
            let w = match name {
                "SetWorldPosition" => {
                    let v = a.get(0).clone();
                    [a.field(&v, "x")?, a.field(&v, "y")?, a.field(&v, "z")?]
                }
                "SetWorldPositionVal" => [a.f(0)?, a.f(1)?, a.opt_f(2).unwrap_or(0.0)],
                // Hex coordinates are turned into world units by `exe-api`'s `HexToWorld`.
                _ => [a.f(0)?, a.f(1)?, 0.0],
            };
            if let Kind::WorldAnchor(wa) = &mut st.tree[id].kind {
                wa.world = w;
            }
            st.world_anchor_recalc(id);
        }
        "SetMapSize" | "SetPlot" => {
            if name == "SetMapSize" {
                let (w, h) = (a.f(0)?, a.f(1)?);
                if let Kind::ReplayMap(r) = &mut st.tree[id].kind {
                    r.resize(w, h);
                }
            } else {
                // SetPlot(x, y, terrain, red, green, blue, alpha)
                let (x, y) = (a.f(0)? as i32, a.f(1)? as i32);
                let color = [a.f(3)?, a.f(4)?, a.f(5)?, a.opt_f(6).unwrap_or(1.0)];
                if let Kind::ReplayMap(r) = &mut st.tree[id].kind {
                    r.set_plot(x, y, color);
                }
            }
        }

        // ---- LuaContext ----
        "SetInputHandler" | "SetShowHideHandler" | "SetPostInit" | "SetUpdate" | "SetShutdown" => {
            let f = a.func(0)?;
            let k = Rc::new(lua.create_registry_value(f)?);
            let bit = match name {
                "SetInputHandler" => ctx_flag::HAS_INPUT,
                "SetShowHideHandler" => ctx_flag::HAS_SHOW_HIDE,
                "SetPostInit" => ctx_flag::POST_INIT_PENDING,
                "SetUpdate" => ctx_flag::HAS_UPDATE,
                _ => 0,
            };
            st.tree[id].flags |= bit;
            if let Kind::Context(c) = &mut st.tree[id].kind {
                match name {
                    "SetInputHandler" => c.input_handler = Some(k),
                    "SetShowHideHandler" => c.show_hide = Some(k),
                    "SetPostInit" => c.post_init = Some(k),
                    "SetUpdate" => c.update_fn = Some(k),
                    _ => c.shutdown_fn = Some(k),
                }
            }
        }
        "ClearUpdate" => {
            st.tree[id].flags &= !ctx_flag::HAS_UPDATE;
            if let Kind::Context(c) = &mut st.tree[id].kind {
                c.update_fn = None;
            }
        }
        "CallShowHideHandler" => {
            let hide = a.bool(0);
            drop(guard);
            sh.call_show_hide(id, hide, false);
            return none();
        }
        "CallParentShowHideHandler" => {
            let hide = a.bool(0);
            let p = st.tree[id].owner;
            drop(guard);
            if let Some(p) = p {
                sh.call_show_hide(p, hide, false);
            }
            return none();
        }
        "DoDeferredLoad" => {
            let n = &mut st.tree[id];
            if n.flags & (ctx_flag::DEFER_LOAD | INITIALIZED) == ctx_flag::DEFER_LOAD {
                n.flags &= !ctx_flag::DEFER_LOAD;
                drop(guard);
                sh.init_context(id);
                return none();
            }
        }
        "IsHotLoad" => return one(lua, false),
        "Reload" => {}
        "LookUpControl" => {
            let p = a.str(0)?;
            drop(guard);
            return match sh.look_up(Some(id), &p) {
                Some(c) => one(lua, sh.push_control(lua, c)?),
                None => none(),
            };
        }
        "LoadNewContext" => {
            let path = a.str(0)?;
            drop(guard);
            let c = sh.load_new_context(id, &path);
            return one(lua, sh.push_control(lua, c)?);
        }
        "BuildInstance" | "BuildInstanceForControl" | "BuildInstanceForControlAtIndex" => {
            let inst = a.str(0)?;
            // The exe fills a nil table silently (the Lekmod OptionsMenu passes a global `instance` that is nil).
            let table = match a.get(1) {
                LuaValue::Nil => lua.create_table(),
                _ => a.table(1)?,
            };
            drop(guard);
            let (parent, index) = match name {
                "BuildInstance" => (None, None),
                "BuildInstanceForControl" => (Some(a.control(sh, 2)?), None),
                _ => (Some(a.control(sh, 2)?), Some(a.int(3)?.max(0) as usize)),
            };
            return match sh.build_instance(lua, id, &inst, &table, parent, index)? {
                Some(c) => one(lua, sh.push_control(lua, c)?),
                None => none(),
            };
        }
        _ => {
            let _ = scr;
            // SIMPLIFIED(SIMP-exe-954): a method the class lists but this port has no body for does nothing (the exe's
            // bodies are not all recovered): it is logged once per method and class and the handler goes on, instead of
            // a Lua error that stops it.
            note_unimplemented(name, st.tree[id].kind.name());
        }
    }
    drop(guard);
    none()
}

thread_local! {
    static UNIMPLEMENTED: std::cell::RefCell<std::collections::HashSet<(&'static str, &'static str)>> =
        std::cell::RefCell::new(std::collections::HashSet::new());
}

/// Logs a control method without a body once per (method, class).
fn note_unimplemented(name: &'static str, kind: &'static str) {
    let first = UNIMPLEMENTED.with(|s| s.borrow_mut().insert((name, kind)));
    if first {
        tracing::warn!(target: "ui.lua", "{name}: not implemented for {kind}; the call does nothing");
    }
}

fn grid_texture_offset(st: &mut State, id: ControlId, v: Vec2) {
    let no_state = st.tree[id].flags & grid_flag::NO_STATE_CHANGE != 0;
    if let Kind::Grid(g) = &mut st.tree[id].kind {
        g.tex_offset = v;
        if no_state {
            g.cur_offset = v;
        }
    }
    st.grid_normalize(id);
}

// ---- UIManager and TTManager ----

pub struct UiManagerRef {
    pub sh: Weak<Shared>,
}

pub struct TtManagerRef {
    pub sh: Weak<Shared>,
}

macro_rules! up {
    ($this:expr) => {
        match $this.sh.upgrade() {
            Some(s) => s,
            None => return Err(err("the UI is gone")),
        }
    };
}

impl UserData for UiManagerRef {
    fn add_methods<'lua, M: UserDataMethods<'lua, Self>>(m: &mut M) {
        m.add_method(
            "QueuePopup",
            |_, this, (ctx, prio): (AnyUserData, Option<f64>)| {
                let sh = up!(this);
                let id = ctx.borrow::<CtrlRef>()?.id;
                let p = prio.unwrap_or(0.0).max(0.0) as u32;
                sh.run(|st| st.queue_popup(id, p));
                Ok(())
            },
        );
        m.add_method("DequeuePopup", |_, this, ctx: AnyUserData| {
            let sh = up!(this);
            let id = ctx.borrow::<CtrlRef>()?.id;
            sh.run(|st| st.dequeue_popup(id));
            Ok(())
        });
        m.add_method(
            "PushModal",
            |_, this, (c, pass): (AnyUserData, Option<bool>)| {
                let sh = up!(this);
                let id = c.borrow::<CtrlRef>()?.id;
                sh.run(|st| st.push_modal(id, pass.unwrap_or(false)));
                Ok(())
            },
        );
        m.add_method("PopModal", |_, this, c: AnyUserData| {
            let sh = up!(this);
            let id = c.borrow::<CtrlRef>()?.id;
            sh.run(|st| st.pop_modal(id));
            Ok(())
        });
        m.add_method("IsModal", |_, this, c: AnyUserData| {
            let sh = up!(this);
            let id = c.borrow::<CtrlRef>()?.id;
            let r = sh.st.borrow().is_modal(id);
            Ok(r)
        });
        m.add_method("IsTopModal", |_, this, c: AnyUserData| {
            let sh = up!(this);
            let id = c.borrow::<CtrlRef>()?.id;
            let r = sh.st.borrow().is_top_modal(id);
            Ok(r)
        });
        // `UIManager:GetControl()` without an argument is the Ctrl key (WorldView, Options); with a
        // path it looks a control up.
        m.add_method("GetControl", |lua, this, path: Option<String>| {
            let sh = up!(this);
            let Some(path) = path else {
                let down = sh.st.borrow().ctrl;
                return Ok(LuaValue::Boolean(down));
            };
            match sh.look_up(None, &path) {
                Some(c) => Ok(LuaValue::UserData(sh.push_control(lua, c)?)),
                None => Ok(LuaValue::Nil),
            }
        });
        m.add_method("GetScreenSizeVal", |_, this, ()| {
            let sh = up!(this);
            let (w, h) = sh.st.borrow().screen();
            Ok((w, h))
        });
        m.add_method("GetMousePos", |_, this, ()| {
            let sh = up!(this);
            let p = sh.st.borrow().mouse;
            Ok((p.x as i32, p.y as i32))
        });
        m.add_method("GetMouseDelta", |_, this, ()| {
            let sh = up!(this);
            let p = sh.st.borrow().mouse_delta;
            Ok((p.x as i32, p.y as i32))
        });
        m.add_method("GetShift", |_, this, ()| Ok(up!(this).st.borrow().shift));
        m.add_method("GetAlt", |_, this, ()| Ok(up!(this).st.borrow().alt));
        m.add_method("SetUICursor", |_, this, c: i32| {
            up!(this).st.borrow_mut().host.set_cursor(c);
            // The exe answers the previous cursor (the scripts hand it back on hide: `UIManager:SetUICursor(old)`); the
            // game's cursor is always the default one here.
            Ok(0)
        });
        m.add_method("GetVisibleNamedContext", |lua, this, name: String| {
            let sh = up!(this);
            let found = {
                let st = sh.st.borrow();
                st.tree.ids().find(|&i| {
                    let n = st.node(i);
                    matches!(n.kind, Kind::Context(_)) && n.id == name && !n.has(HIDDEN)
                })
            };
            match found {
                Some(c) => Ok(Some(sh.push_control(lua, c)?)),
                None => Ok(None),
            }
        });
        m.add_method("GetNamedContextCount", |_, this, ()| {
            let sh = up!(this);
            let st = sh.st.borrow();
            Ok(st
                .tree
                .ids()
                .filter(|&i| {
                    matches!(st.node(i).kind, Kind::Context(_)) && !st.node(i).id.is_empty()
                })
                .count() as i32)
        });
        m.add_method("GetNamedContextByIndex", |lua, this, idx: i32| {
            let sh = up!(this);
            let found = {
                let st = sh.st.borrow();
                st.tree
                    .ids()
                    .filter(|&i| {
                        matches!(st.node(i).kind, Kind::Context(_)) && !st.node(i).id.is_empty()
                    })
                    .nth(idx.max(0) as usize)
            };
            match found {
                Some(c) => Ok(Some(sh.push_control(lua, c)?)),
                None => Ok(None),
            }
        });
        m.add_method("RequestReprocessAnchoring", |_, this, ()| {
            let sh = up!(this);
            sh.run(|st| {
                for r in st.roots.clone() {
                    st.reprocess_anchoring(r);
                }
            });
            Ok(())
        });
        m.add_method("PopAllInterrupts", |_, _, ()| Ok(()));
        m.add_method("FlushInputMessages", |_, _, ()| Ok(()));
        m.add_method("SetHideWorldAnchors", |_, this, h: bool| {
            up!(this).st.borrow_mut().world_anchors_hidden = h;
            Ok(())
        });
        m.add_method("SetDrawControlBounds", |_, _, _: bool| Ok(()));
        m.add_method("IsExtendedFont", |_, _, ()| Ok(false));
        m.add_method("GetGameGridTopLeft", |_, _, ()| Ok((0, 0)));
        m.add_method("GetGameGridBottomRight", |_, this, ()| {
            let sh = up!(this);
            let (w, h) = sh.st.borrow().screen();
            Ok((w, h))
        });
        m.add_method("GetNumPointers", |_, _, ()| Ok(0));
        // WO-G (K20, SIMP-exe-391): the options menu lists the screen's modes; a browser tab has one.
        m.add_method("GetResCount", |_, _, ()| Ok(1));
        // width, height, refresh rate, scale, display, adapter
        m.add_method("GetResInfo", |_, _, _: i32| Ok((1920, 1080, 60, 1, 0, 0)));
        m.add_method("PrintAllocationInfo", |_, _, ()| Ok(()));
        m.add_method("PrintTextureInfo", |_, _, ()| Ok(()));
        m.add_method("PrintWindowInfo", |_, _, ()| Ok(()));
        m.add_method("ReleaseTextureByName", |_, _, _: String| Ok(()));
        m.add_method("ReleaseAndUnloadTextureByName", |_, _, _: String| Ok(()));
        m.add_method("RequestReloadUI", |_, _, ()| Ok(()));
        m.add_method("ApplyResolution", |_, _, _: MultiValue| Ok(()));
    }

    fn type_name() -> &'static str {
        "UIManager"
    }
}

impl UserData for TtManagerRef {
    fn add_methods<'lua, M: UserDataMethods<'lua, Self>>(m: &mut M) {
        m.add_method(
            "RegisterBasicControls",
            |_, this, (root, content, text, grid): (AnyUserData, AnyUserData, AnyUserData, AnyUserData)| {
                let sh = up!(this);
                let ids = [root, content, text, grid].map(|u| u.borrow::<CtrlRef>().map(|r| r.id));
                let [a, b, c, d] = ids;
                let (a, b, c, d) = (a?, b?, c?, d?);
                sh.run(|st| st.tooltips_register_basic(a, b, c, d));
                Ok(())
            },
        );
        m.add_method("SetToolTipString", |_, this, s: Option<String>| {
            let sh = up!(this);
            sh.run(|st| st.tooltip_set_string(&s.unwrap_or_default()));
            Ok(())
        });
        m.add_method("SetToolTipType", |_, this, name: String| {
            let sh = up!(this);
            sh.run(|st| st.tooltip_set_type(crc32(&name)));
            Ok(())
        });
        m.add_method(
            "GetTypeControlTable",
            |lua, this, (name, table): (String, Table)| {
                let sh = up!(this);
                let found = sh.st.borrow().tooltips.types.get(&crc32(&name)).copied();
                if let Some(c) = found {
                    fill_type_table(&sh, lua, c, &table)?;
                } else {
                    // SIMPLIFIED(SIMP-exe-563): the exe's manager has every type the moment its XML is read; ours builds the
                    // types read before the basic controls (EUI_context's) only when they come (SIMP-exe-73), after contexts
                    // like UnitFlagManager asked for their table at load. The table is filled on its first read instead.
                    let weak = this.sh.clone();
                    let hash = crc32(&name);
                    let index = lua.create_function(move |lua, (t, k): (Table, LuaValue)| {
                        let Some(sh) = weak.upgrade() else {
                            return Ok(LuaValue::Nil);
                        };
                        let found = sh.st.borrow().tooltips.types.get(&hash).copied();
                        match found {
                            Some(c) => {
                                t.set_metatable(None);
                                fill_type_table(&sh, lua, c, &t)?;
                                t.raw_get(k)
                            }
                            None => Ok(LuaValue::Nil),
                        }
                    });
                    let mt = lua.create_table();
                    mt.set("__index", index)?;
                    table.set_metatable(Some(mt));
                }
                Ok(())
            },
        );
    }

    fn type_name() -> &'static str {
        "TTManager"
    }
}

/// `TTManager:GetTypeControlTable`: the controls of the tooltip type `c` with an `ID`, by that ID, into `table`.
fn fill_type_table(
    sh: &Rc<Shared>,
    lua: &Lua,
    c: ControlId,
    table: &Table<'_>,
) -> lua_sys::Result<()> {
    let mut ids = Vec::new();
    sh.st.borrow().expose_list(c, &mut ids);
    for i in ids {
        let n = sh.st.borrow().node(i).id.clone();
        table.set(n, sh.push_control(lua, i)?)?;
    }
    Ok(())
}

/// The globals every context sees: `UIManager`, `TTManager`, `LookUpControl`.
pub fn install_globals(weak: &Weak<Shared>, lua: &Lua, g: &Table<'_>) -> lua_sys::Result<()> {
    g.raw_set(
        "UIManager",
        lua.create_userdata(UiManagerRef { sh: weak.clone() })?,
    )?;
    g.raw_set(
        "TTManager",
        lua.create_userdata(TtManagerRef { sh: weak.clone() })?,
    )?;
    // LookUpControl(path): relative paths start in the context of the caller.
    let w = weak.clone();
    let raw = lua.create_function(move |lua, (path, env): (String, Option<Table>)| {
        let Some(sh) = w.upgrade() else {
            return Ok(None);
        };
        let from = match env {
            Some(e) => match e.raw_get::<_, LuaValue>("ContextPtr")? {
                LuaValue::UserData(u) => Some(u.borrow::<CtrlRef>()?.id),
                _ => None,
            },
            None => None,
        };
        match sh.look_up(from, &path) {
            Some(c) => Ok(Some(sh.push_control(lua, c)?)),
            None => Ok(None),
        }
    });
    g.raw_set("__ui_lookup", raw)?;
    // `__ui_find_label(suffix)` -> the first label of the tree whose path (the ids from the root, `/A/B/C`) ends with
    // `suffix`, or nothing. For the host's own text on a label of the original UI (the AI progress on the end-turn
    // button, SIMP-exe-790), which no context can reach through `LookUpControl`.
    let w = weak.clone();
    let find = lua.create_function(move |lua, suffix: String| {
        let Some(sh) = w.upgrade() else {
            return Ok(None);
        };
        fn walk(
            st: &crate::state::State,
            id: crate::tree::ControlId,
            path: &str,
            suffix: &str,
        ) -> Option<crate::tree::ControlId> {
            let n = st.tree.get(id)?;
            let path = format!("{path}/{}", n.id);
            if path.ends_with(suffix) && matches!(n.kind, Kind::Label(_)) {
                return Some(id);
            }
            n.children.iter().find_map(|&c| walk(st, c, &path, suffix))
        }
        let found = {
            let st = sh.st.borrow();
            st.roots.iter().find_map(|&r| walk(&st, r, "", &suffix))
        };
        match found {
            Some(c) => Ok(Some(sh.push_control(lua, c)?)),
            None => Ok(None),
        }
    });
    g.raw_set("__ui_find_label", find)?;
    // Dev (the playtests): `__ui_screen_rect(control)` -> left, top, width, height (UI pixels), shown; nothing for
    // a dead control. Absolute paths find the control with `LookUpControl`.
    let w = weak.clone();
    let rect = lua.create_function(move |_, c: LuaValue| {
        let (Some(sh), LuaValue::UserData(u)) = (w.upgrade(), c) else {
            return Ok(None);
        };
        let id = u.borrow::<CtrlRef>()?.id;
        let r = sh.st.borrow().screen_rect(id);
        // [left, top, width, height, shown (1/0)]
        Ok(r.map(|(r, shown)| vec![r[0], r[1], r[2], r[3], f32::from(u8::from(shown))]))
    });
    g.raw_set("__ui_screen_rect", rect)?;
    // Dev: `__ui_dump_screen(path_filter)` -> the shown controls whose path contains the filter, a line each
    // (`debug::dump_screen`).
    let w = weak.clone();
    let dump = lua.create_function(move |_, filter: Option<String>| {
        Ok(w.upgrade()
            .map(|sh| crate::debug::dump_screen(&sh.st.borrow(), &filter.unwrap_or_default())))
    });
    g.raw_set("__ui_dump_screen", dump)?;
    lua.load("function LookUpControl(path) return __ui_lookup(path, getfenv(2)) end")
        .set_name("=ui")
        .set_environment(g.clone())
        .exec()
}
