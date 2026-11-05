//! The part of the UI core that talks to Lua: contexts, callbacks, per-frame update, the Lua
//! objects of controls. `State` never calls Lua; it queues [`Pending`] calls, and `Shared`
//! makes them after it let go of its borrow, so Lua may call back into the UI at once.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use assets::UiFiles;
use lua_sys::{
    AnyUserData, Context as LuaCtx, IncludeSource, Lua, LuaValue, MultiValue, RegistryKey, Runtime,
    RuntimeConfig, Table,
};

use crate::flags::*;
use crate::host::FileSource;
use crate::state::{LuaArg, Pending, State};
use crate::style::StyleBook;
use crate::tree::*;
use crate::xml::{self, XmlElem};

/// The UI files: the index says which names exist, the source gives the bytes.
pub struct Files {
    pub ui: UiFiles,
    pub src: Rc<dyn FileSource>,
}

impl Files {
    /// `(path, bytes)` of a file by bare name and extension.
    pub fn read_stem(&self, stem: &str, ext: &str) -> Option<(String, Vec<u8>)> {
        let info = self.ui.index().find_stem(stem, &[ext])?;
        let bytes = self.src.read(&info.path)?;
        Some((info.path.clone(), bytes))
    }
}

/// `include` over the UI file set: by bare name, highest layer wins (decided by the index).
struct LuaFiles(Rc<Files>);

impl IncludeSource for LuaFiles {
    fn resolve(&self, name: &str) -> Option<String> {
        let bare = crate::bare_lower(name);
        let stem = bare.strip_suffix(".lua").unwrap_or(&bare);
        self.0
            .ui
            .index()
            .find_stem(stem, &["lua"])
            .map(|f| f.path.clone())
    }

    fn list(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .0
            .ui
            .index()
            .files
            .iter()
            .filter(|f| f.kind == asset_format::UiFileKind::Lua)
            .map(|f| f.path.clone())
            .collect();
        v.sort();
        v
    }

    fn xml_stems(&self) -> Vec<String> {
        self.0
            .ui
            .index()
            .files
            .iter()
            .filter(|f| f.kind == asset_format::UiFileKind::Xml)
            .filter_map(|f| {
                let b = f.path.rsplit(['/', '\\']).next()?;
                Some(b.strip_suffix(".xml").unwrap_or(b).to_ascii_lowercase())
            })
            .collect()
    }

    fn read(&self, path: &str) -> Option<Vec<u8>> {
        self.0.src.read(path)
    }
}

/// The Lua userdata of a control: its id and the way back to the UI.
pub struct CtrlRef {
    pub id: ControlId,
    pub sh: Weak<Shared>,
}

pub struct Shared {
    pub st: RefCell<State>,
    pub rt: Runtime,
    pub files: Rc<Files>,
    pub(crate) me: Weak<Shared>,
    ud_cache: RefCell<HashMap<(u64, ControlId), RegistryKey>>,
    fn_cache: RefCell<HashMap<(u64, &'static str), RegistryKey>>,
    /// Lua errors from the UI core itself (not from scripts).
    pub core_errors: RefCell<Vec<String>>,
    /// SIMPLIFIED(SIMP-exe-4): asks whether a context (bare name) may load; a context it refuses stays an empty,
    /// hidden, initialised node (clusters outside the milestone, the Lekmod add-ins the game core runs itself).
    pub context_filter: RefCell<Option<ContextFilter>>,
    /// Names the filter refused (for the report).
    pub filtered: RefCell<Vec<String>>,
    /// SIMPLIFIED(SIMP-exe-955): controls of the original screens that do nothing in this port, as (context bare name,
    /// control ID): they are hidden when their context is built and stay hidden whatever the script's `SetHide` says.
    pub removed_controls: RefCell<Vec<(String, String)>>,
}

/// Asks whether a context (bare name) may load.
pub type ContextFilter = Rc<dyn Fn(&str) -> bool>;

type BaseSetup = Box<dyn Fn(&Lua, &Table<'_>) -> lua_sys::Result<()>>;

impl Shared {
    pub fn new(
        state: State,
        files: Rc<Files>,
        rt_cfg: RuntimeConfig,
        extra_setup: BaseSetup,
    ) -> lua_sys::Result<Rc<Shared>> {
        let mut err = None;
        let sh = Rc::new_cyclic(|me: &Weak<Shared>| {
            let weak = me.clone();
            let setup = move |lua: &Lua, g: &Table<'_>| -> lua_sys::Result<()> {
                crate::lua_api::install_globals(&weak, lua, g)?;
                extra_setup(lua, g)
            };
            let rt = Runtime::new(rt_cfg, Rc::new(LuaFiles(files.clone())), setup);
            let rt = match rt {
                Ok(r) => r,
                Err(e) => {
                    err = Some(e);
                    // A runtime is needed to build the struct; build a throwaway one.
                    Runtime::new(
                        RuntimeConfig::default(),
                        Rc::new(lua_sys::MemorySource::new()),
                        |_, _| Ok(()),
                    )
                    .expect("a default runtime")
                }
            };
            Shared {
                st: RefCell::new(state),
                rt,
                files,
                me: me.clone(),
                ud_cache: RefCell::new(HashMap::new()),
                fn_cache: RefCell::new(HashMap::new()),
                core_errors: RefCell::new(Vec::new()),
                context_filter: RefCell::new(None),
                filtered: RefCell::new(Vec::new()),
                removed_controls: RefCell::new(Vec::new()),
            }
        });
        match err {
            Some(e) => Err(e),
            None => Ok(sh),
        }
    }

    pub fn with<R>(&self, f: impl FnOnce(&mut State) -> R) -> R {
        f(&mut self.st.borrow_mut())
    }

    /// Runs a state operation, then makes the Lua calls it queued.
    pub fn run<R>(&self, f: impl FnOnce(&mut State) -> R) -> R {
        let r = f(&mut self.st.borrow_mut());
        self.flush();
        r
    }

    // ---- Lua objects of controls ----

    /// The userdata for a control: one per control and state, so `==` works.
    pub fn push_control<'l>(
        &self,
        lua: &'l Lua,
        id: ControlId,
    ) -> lua_sys::Result<AnyUserData<'l>> {
        let key = (lua.id(), id);
        if let Some(k) = self.ud_cache.borrow().get(&key) {
            return lua.registry_value::<AnyUserData>(k);
        }
        let ud = lua.create_userdata(CtrlRef {
            id,
            sh: self.me.clone(),
        })?;
        let k = lua.create_registry_value(ud.clone())?;
        self.ud_cache.borrow_mut().insert(key, k);
        Ok(ud)
    }

    /// Forgets the userdata of deleted controls.
    pub fn sweep_userdata(&self) {
        let st = self.st.borrow();
        self.ud_cache
            .borrow_mut()
            .retain(|(_, id), _| st.alive(*id));
    }

    pub(crate) fn method_fn<'l>(
        &self,
        lua: &'l Lua,
        name: &'static str,
    ) -> lua_sys::Result<lua_sys::Function<'l>> {
        let key = (lua.id(), name);
        if let Some(k) = self.fn_cache.borrow().get(&key) {
            return lua.registry_value(k);
        }
        let weak = self.me.clone();
        let f = lua.create_function(move |lua, args: MultiValue| {
            let Some(sh) = weak.upgrade() else {
                return Err(lua_sys::LuaError::msg("the UI is gone"));
            };
            crate::lua_api::call_method(&sh, lua, name, args)
        });
        let k = lua.create_registry_value(f.clone())?;
        self.fn_cache.borrow_mut().insert(key, k);
        Ok(f)
    }

    // ---- queued calls ----

    /// Makes the Lua calls the state queued, in order, until none is left.
    pub fn flush(&self) {
        loop {
            let batch = std::mem::take(&mut self.st.borrow_mut().pending);
            if batch.is_empty() {
                return;
            }
            for p in batch {
                self.run_pending(p);
            }
        }
    }

    fn lua_of(&self, ctx: Option<ControlId>) -> Option<LuaCtx> {
        let st = self.st.borrow();
        ctx.and_then(|c| st.ctx(c)).and_then(|c| c.lua.clone())
    }

    fn run_pending(&self, p: Pending) {
        match p {
            Pending::Call { owner, key, args } => {
                let Some(lc) = self.lua_of(owner) else { return };
                let vals: Vec<LuaValue<'_>> = match self.args_to_lua(lc.lua(), &args) {
                    Ok(v) => v,
                    Err(e) => {
                        self.core_errors.borrow_mut().push(e.to_string());
                        return;
                    }
                };
                let _ = lc.call_key::<_, ()>(&key, MultiValue::from_vec(vals));
            }
            Pending::ShowHide { ctx, hide } => self.call_show_hide(ctx, hide, false),
            Pending::SetHide { ctx, hide } => self.set_hide(ctx, hide),
            Pending::InitContext { ctx } => self.init_context(ctx),
            Pending::Shutdown { ctx } => {
                let key = self
                    .st
                    .borrow()
                    .ctx(ctx)
                    .and_then(|c| c.shutdown_fn.clone());
                if let (Some(k), Some(lc)) = (key, self.lua_of(Some(ctx))) {
                    let _ = lc.call_key::<_, ()>(&k, ());
                }
            }
        }
    }

    pub(crate) fn args_to_lua<'l>(
        &self,
        lua: &'l Lua,
        args: &[LuaArg],
    ) -> lua_sys::Result<Vec<LuaValue<'l>>> {
        use lua_sys::ToLua;
        args.iter()
            .map(|a| match a {
                LuaArg::Nil => Ok(LuaValue::Nil),
                LuaArg::Bool(b) => b.to_lua(lua),
                LuaArg::Int(i) => i.to_lua(lua),
                LuaArg::Num(n) => n.to_lua(lua),
                LuaArg::Str(s) => s.as_str().to_lua(lua),
                LuaArg::Control(c) => Ok(LuaValue::UserData(self.push_control(lua, *c)?)),
            })
            .collect()
    }

    /// `ShowHideHandler(bIsHide, bIsInit)`.
    pub fn call_show_hide(&self, ctx: ControlId, hide: bool, init: bool) {
        let key = self.st.borrow().ctx(ctx).and_then(|c| c.show_hide.clone());
        if let (Some(k), Some(lc)) = (key, self.lua_of(Some(ctx))) {
            let _ = lc.call_key::<_, ()>(&k, (hide, init));
        }
    }

    // ---- contexts ----

    /// Creates a root context for `path` (`Assets/UI/InGame/InGame`) and loads it.
    pub fn load_root(&self, path: &str) -> ControlId {
        let id = self.with(|st| {
            let mut c = Context::new(path.to_owned());
            c.default_style = String::new();
            let id = st.new_node(Kind::Context(Box::new(c)), None);
            st.stats.controls_built += 1;
            // The ID keeps its case: EUI asks `LookUpControl("..")` for `GetID() == "InGame"` (a lower-case ID made the
            // contexts skip loading EUI_context, whose tooltip types and shared table then never existed).
            let stem = path.rsplit(['/', '\\']).next().unwrap_or(path).trim();
            st.tree[id].id = stem.strip_suffix(".xml").unwrap_or(stem).to_owned();
            id
        });
        self.add_root(id);
        id
    }

    /// `LuaContext::LoadNewContext`: a new context, initialised first and then added as a child.
    pub fn load_new_context(&self, parent: ControlId, path: &str) -> ControlId {
        let id = self.with(|st| {
            let id = st.new_node(
                Kind::Context(Box::new(Context::new(path.to_owned()))),
                Some(parent),
            );
            st.stats.controls_built += 1;
            st.tree[id].flags |= PARENT_DELETE;
            id
        });
        self.init_context(id);
        self.with(|st| st.add_child(parent, id));
        id
    }

    /// `UIManager::AddRootControl`: newest in front for input; shown.
    pub fn add_root(&self, id: ControlId) {
        let new = self.with(|st| {
            if st.roots.contains(&id) {
                false
            } else {
                st.roots.insert(0, id);
                true
            }
        });
        if new {
            self.set_hide(id, false);
            self.init_context(id);
        }
    }

    /// The directory-less name of the context files: `Assets/UI/InGame/TopPanel` -> `TopPanel`.
    fn stem(path: &str) -> String {
        let b = path.rsplit(['/', '\\']).next().unwrap_or(path);
        b.strip_suffix(".xml")
            .or_else(|| b.strip_suffix(".lua"))
            .unwrap_or(b)
            .to_owned()
    }

    /// `LuaContext::Initialize`: XML tree, child contexts, then the Lua file, then the handlers.
    pub fn init_context(&self, id: ControlId) {
        // SIMPLIFIED(SIMP-uic-3): the order follows the dump of `LuaContext::Initialize` and
        // `LoadXML`; the thunks that bind `ContextPtr` are not read.
        let (path, deferred, initialized) = {
            let st = self.st.borrow();
            let Some(c) = st.ctx(id) else { return };
            let n = st.node(id);
            (
                c.path.clone(),
                n.has(ctx_flag::DEFER_LOAD),
                n.has(INITIALIZED),
            )
        };
        if deferred || initialized {
            return;
        }
        let stem = Self::stem(&path);
        let refused = self
            .context_filter
            .borrow()
            .as_ref()
            .is_some_and(|f| !f(&stem));
        if refused {
            self.filtered.borrow_mut().push(stem);
            self.with(|st| {
                let n = st.node_mut(id);
                n.flags |= INITIALIZED | HIDDEN;
            });
            return;
        }
        // The file name of a context is `FileName`, found by bare name.
        // SIMPLIFIED(SIMP-exe-423): the exe's rule for the `_small` layouts is read off the goldens (S2 and S3 show them), not off the exe.
        let small = if self.st.borrow().cfg.small_assets {
            self.files.read_stem(&format!("{stem}_small"), "xml")
        } else {
            None
        };
        let xml_file = small.or_else(|| self.files.read_stem(&stem, "xml"));
        let doc = match xml_file {
            Some((p, bytes)) => match xml::parse(&p, &bytes) {
                Ok(d) => Some(d),
                Err(e) if e.empty => None,
                Err(e) => {
                    tracing::error!(target: "ui.xml", "{e}");
                    self.st
                        .borrow_mut()
                        .warnings
                        .push(crate::state::XmlWarning {
                            file: e.file.clone(),
                            line: e.line,
                            tag: String::new(),
                            what: format!("parse error: {}", e.message),
                        });
                    self.st.borrow_mut().stats.xml_warnings += 1;
                    None
                }
            },
            None => {
                tracing::warn!(target: "ui.xml", "no XML file for context {path}");
                None
            }
        };
        let lua_file = self.files.read_stem(&stem, "lua");
        // The Lua environment of the context.
        let weak = self.me.clone();
        let lctx = match self.rt.create_context(&stem, |lua, env| {
            let sh = weak
                .upgrade()
                .ok_or_else(|| lua_sys::LuaError::msg("the UI is gone"))?;
            let cp = sh.push_control(lua, id)?;
            env.raw_set("ContextPtr", cp)?;
            env.raw_set("Controls", lua.create_table())?;
            Ok(())
        }) {
            Ok(c) => c,
            Err(e) => {
                self.core_errors.borrow_mut().push(e.to_string());
                return;
            }
        };
        self.with(|st| {
            if let Some(c) = st.ctx_mut(id) {
                c.lua = Some(lctx.clone());
                c.doc = doc.clone();
            }
            let (w, h) = st.screen();
            let n = st.node_mut(id);
            n.size = glam::Vec2::new(w as f32, h as f32);
            n.flags |= SIZE_FULL_X | SIZE_FULL_Y;
        });
        // LoadXML
        if let Some(root) = &doc {
            self.load_xml(id, root, &lctx);
        }
        // ControlBase::Initialize, which loads the child contexts on the way.
        self.initialize_node(id);
        // RegisterNamedControl("..")
        self.with(|st| {
            let parent_ctx = st.node(id).owner.unwrap_or(id);
            if let Some(c) = st.ctx_mut(id) {
                c.named.insert("..".to_owned(), parent_ctx);
            }
        });
        // The script.
        if let Some((p, bytes)) = lua_file {
            match lctx
                .lua()
                .load(&bytes)
                .set_name(&format!("@{p}"))
                .set_environment(lctx.env())
                .into_function()
            {
                Ok(f) => {
                    let r = f.call::<_, ()>(());
                    if let Err(e) = r {
                        lctx.record_error(&e);
                    }
                }
                Err(e) => lctx.record_error(&e),
            }
        }
        // The context is loaded; the show/hide handler learns the initial state.
        self.with(|st| {
            if let Some(c) = st.ctx_mut(id) {
                c.loaded = true;
            }
        });
        let hidden = self.st.borrow().node(id).has(HIDDEN);
        let has = self.st.borrow().node(id).has(ctx_flag::HAS_SHOW_HIDE);
        if has {
            self.call_show_hide(id, hidden, true);
        }
        self.flush();
    }

    /// `LuaContext::LoadXML`: styles, instances and controls of the root element.
    fn load_xml(&self, id: ControlId, root: &Rc<XmlElem>, lctx: &LuaCtx) {
        self.with(|st| {
            if st.node(id).id.is_empty()
                && let Some(n) = root.attr("Name", st.cfg.attr_case_insensitive)
            {
                st.node_mut(id).id = n.to_owned();
            }
            st.load_layout_defaults(id, root);
        });
        for el in root.children.clone() {
            match el.name.as_str() {
                "Styles" => self.with(|st| {
                    if let Some(c) = st.ctx_mut(id) {
                        c.styles = Some(el.clone());
                    }
                }),
                "Instance" => self.with(|st| {
                    let name = st.attr(&el, "Name").map(str::to_owned);
                    if let (Some(n), Some(first)) = (name, el.children.first())
                        && let Some(c) = st.ctx_mut(id)
                    {
                        c.instances.insert(n, first.clone());
                    }
                }),
                _ => {
                    let mut ids = Vec::new();
                    let built = self.with(|st| {
                        let mark = st.touch(&el);
                        let c = st.build_control(&el, Some(id));
                        st.finish_touched(mark);
                        if let Some(c) = c {
                            st.expose_list(c, &mut ids);
                            // RegisterNamedControl on the owning context.
                            let named: Vec<(String, ControlId)> =
                                ids.iter().map(|&i| (st.node(i).id.clone(), i)).collect();
                            if let Some(cx) = st.ctx_mut(id) {
                                for (n, i) in named {
                                    cx.named.insert(n, i);
                                }
                            }
                        }
                        c
                    });
                    if let Some(c) = built {
                        let controls: Option<Table<'_>> = lctx.env().raw_get("Controls").ok();
                        if let Some(t) = controls {
                            for i in &ids {
                                let name = self.st.borrow().node(*i).id.clone();
                                match self.push_control(lctx.lua(), *i) {
                                    Ok(ud) => {
                                        let _ = t.raw_set(name, ud);
                                    }
                                    Err(e) => self.core_errors.borrow_mut().push(e.to_string()),
                                }
                            }
                        }
                        self.with(|st| st.add_child(id, c));
                    }
                }
            }
        }
    }

    // ---- initialisation ----

    /// `Initialize` of a control: the class's own work around `ControlBase::Initialize`.
    /// Contexts below load their Lua at their place in the order.
    pub fn initialize(&self, id: ControlId) {
        let is_ctx = matches!(self.st.borrow().node(id).kind, Kind::Context(_));
        let loaded = self
            .st
            .borrow()
            .ctx(id)
            .map(|c| c.loaded || c.lua.is_some())
            .unwrap_or(false);
        if is_ctx {
            if !loaded {
                self.init_context(id);
            }
            return;
        }
        self.initialize_node(id);
    }

    /// The generic order: before, self, children, after.
    pub fn initialize_node(&self, id: ControlId) {
        self.with(|st| {
            st.init_pre(id);
            st.init_self(id);
        });
        let kids = self.st.borrow().node(id).children.clone();
        for c in kids {
            if !self.st.borrow().alive(c) {
                continue;
            }
            self.initialize(c);
            self.with(|st| {
                if st.alive(c) {
                    st.init_union(id, c);
                }
            });
        }
        self.with(|st| st.init_post(id));
        self.hide_removed_in(id);
    }

    /// Hides the removed controls (`removed_controls`) that belong to the context `ctx`.
    fn hide_removed_in(&self, ctx: ControlId) {
        if self.removed_controls.borrow().is_empty() {
            return;
        }
        let ids: Vec<ControlId> = {
            let st = self.st.borrow();
            let mut out = Vec::new();
            let mut todo = st.node(ctx).children.clone();
            while let Some(c) = todo.pop() {
                let Some(n) = st.tree.get(c) else { continue };
                if n.owner == Some(ctx) {
                    out.push(c);
                }
                todo.extend(n.children.iter().copied());
            }
            out
        };
        for cid in ids {
            if self.is_removed(cid) {
                self.st.borrow_mut().node_mut(cid).flags |= HIDDEN;
            }
        }
    }

    /// Whether a control is one of the removed ones (its context's bare name and its ID match).
    fn is_removed(&self, id: ControlId) -> bool {
        let list = self.removed_controls.borrow();
        if list.is_empty() {
            return false;
        }
        let st = self.st.borrow();
        let Some(n) = st.tree.get(id) else {
            return false;
        };
        let Some(c) = n.owner.and_then(|o| st.ctx(o)) else {
            return false;
        };
        let stem = Self::stem(&c.path);
        list.iter().any(|(s, i)| *s == stem && *i == n.id)
    }

    // ---- show / hide ----

    /// `ControlBase::SetHide` and `LuaContext::SetHide`.
    pub fn set_hide(&self, id: ControlId, hide: bool) {
        let hide = hide || self.is_removed(id);
        let (is_ctx, flags_before) = {
            let mut st = self.st.borrow_mut();
            let n = st.node_mut(id);
            let before = n.flags;
            if hide {
                n.flags |= HIDDEN;
            } else {
                n.flags &= !HIDDEN;
            }
            (matches!(n.kind, Kind::Context(_)), before)
        };
        if !is_ctx {
            return;
        }
        let f = self.st.borrow().node(id).flags;
        if f & INITIALIZED == 0 {
            if f & (ctx_flag::DEFER_LOAD | INITIALIZED) == ctx_flag::DEFER_LOAD && !hide {
                self.st.borrow_mut().node_mut(id).flags &= !ctx_flag::DEFER_LOAD;
                self.init_context(id);
            }
        } else if f & ctx_flag::HAS_SHOW_HIDE != 0 {
            let _ = flags_before;
            self.call_show_hide(id, hide, false);
        }
        self.flush();
    }

    // ---- lookup ----

    /// `UIManager::LookUpControl` for absolute paths and `LuaContext::LookUpControl` for the rest.
    pub fn look_up(&self, from: Option<ControlId>, path: &str) -> Option<ControlId> {
        let st = self.st.borrow();
        let mut segs = path.split('/').filter(|s| !s.is_empty());
        let mut cur: ControlId;
        if path.starts_with('/') {
            let first = segs.next()?;
            cur = *st
                .roots
                .iter()
                .find(|&&r| st.node(r).id.eq_ignore_ascii_case(first) || st.node(r).id == first)?;
        } else {
            let first = segs.next()?;
            let ctx = from?;
            cur = *st.ctx(ctx)?.named.get(first)?;
        }
        for s in segs {
            if !matches!(st.node(cur).kind, Kind::Context(_)) {
                return None;
            }
            cur = *st.ctx(cur)?.named.get(s)?;
        }
        Some(cur)
    }

    // ---- update ----

    /// `UIManager::Update`: modals, the popup, the roots, the tips.
    pub fn update(&self, dt: f32) {
        self.st.borrow_mut().elapsed += f64::from(dt);
        let pending = std::mem::take(&mut self.st.borrow_mut().tooltips.init_pending);
        for c in pending {
            if self.st.borrow().alive(c) && self.st.borrow().node(c).flags & INITIALIZED == 0 {
                self.initialize(c);
            }
        }
        let (modals, popup, roots) = {
            let st = self.st.borrow();
            (st.modals.clone(), st.popup_current, st.roots.clone())
        };
        for m in modals {
            self.update_node(m, dt);
        }
        if let Some(p) = popup {
            self.update_node(p, dt);
        }
        for r in roots {
            if self.st.borrow().alive(r) && !self.st.borrow().node(r).has(HIDDEN) {
                self.update_node(r, dt);
            }
        }
        self.flush();
        // SIMPLIFIED(SIMP-uic-7): a tip switched on under a resting mouse is found by a mouse move to the same place (no
        // delta); the exe's ToolTipManager shows such a tip without a move (the plot help of PlotHelpManager.lua).
        let recheck = std::mem::take(&mut self.st.borrow_mut().tooltips.recheck);
        if recheck {
            let (m, keys) = {
                let st = self.st.borrow();
                (st.mouse, st.key_state)
            };
            let l = (i32::from(m.y as i16) << 16) | i32::from(m.x as i16 as u16);
            self.process_message(crate::input::WM_MOUSEMOVE, keys, l);
        }
        self.st.borrow_mut().tooltips_refresh_position();
        self.st.borrow_mut().tooltip_drop_if_hidden();
        self.sweep_userdata();
    }

    /// `ControlBase::Update` and the classes that add to it.
    pub fn update_node(&self, id: ControlId, dt: f32) {
        let (ctx_calls, kids) = {
            let mut st = self.st.borrow_mut();
            if !st.alive(id) {
                return;
            }
            let mut calls: Vec<(Callback, Vec<LuaArg>)> = Vec::new();
            match &mut st.node_mut(id).kind {
                Kind::Anim(_) => st.anim_tick(id, dt),
                Kind::WorldAnchor(_) => st.world_anchor_recalc(id),
                Kind::Context(_) => {
                    let n = st.node_mut(id);
                    if n.flags & ctx_flag::POST_INIT_PENDING != 0 {
                        n.flags &= !ctx_flag::POST_INIT_PENDING;
                        if let Kind::Context(c) = &mut n.kind
                            && let Some(k) = c.post_init.take()
                        {
                            calls.push((k, Vec::new()));
                        }
                    }
                    if let Kind::Context(c) = &st.node(id).kind
                        && st.node(id).flags & ctx_flag::HAS_UPDATE != 0
                        && let Some(k) = &c.update_fn
                    {
                        calls.push((k.clone(), vec![LuaArg::Num(f64::from(dt))]));
                    }
                }
                Kind::Button(_) | Kind::TextButton(_) | Kind::GridButton(_) => {
                    st.node_mut(id).flags &= !btn::FORCED_UP;
                }
                _ => {}
            }
            // An anchor that is off the screen keeps projecting (it comes back when the camera moves) but its
            // children are not updated.
            let kids = if matches!(st.node(id).kind, Kind::WorldAnchor(_))
                && st.node(id).has(MOUSE_OVER_HIDDEN)
            {
                Vec::new()
            } else {
                st.node(id).children.clone()
            };
            (calls, kids)
        };
        for (k, args) in ctx_calls {
            self.st.borrow_mut().queue_call(Some(id), &k, args);
        }
        self.flush();
        for c in kids {
            let ok = {
                let st = self.st.borrow();
                st.alive(c)
                    && (!st.node(c).has(NOT_ACTIVE)
                        || (matches!(st.node(c).kind, Kind::WorldAnchor(_))
                            && !st.node(c).has(HIDDEN)))
            };
            if ok {
                self.update_node(c, dt);
            }
        }
    }

    // ---- instances ----

    /// `LuaContext::BuildInstance*`: a copy of `<Instance Name>` into `parent`, its IDs into `table`.
    pub fn build_instance<'l>(
        &self,
        lua: &'l Lua,
        ctx: ControlId,
        name: &str,
        table: &Table<'l>,
        parent: Option<ControlId>,
        index: Option<usize>,
    ) -> lua_sys::Result<Option<ControlId>> {
        let el = self
            .st
            .borrow()
            .ctx(ctx)
            .and_then(|c| c.instances.get(name).cloned());
        let Some(el) = el else {
            return Ok(None);
        };
        let mut ids = Vec::new();
        let built = self.with(|st| {
            let mark = st.touch(&el);
            let c = st.build_control(&el, Some(ctx));
            st.finish_touched(mark);
            if let Some(c) = c {
                st.expose_list(c, &mut ids);
                st.add_child_at(parent.unwrap_or(ctx), c, index);
            }
            c
        });
        let Some(c) = built else { return Ok(None) };
        for i in &ids {
            let n = self.st.borrow().node(*i).id.clone();
            table.set(n, self.push_control(lua, *i)?)?;
        }
        self.initialize(c);
        self.flush();
        Ok(Some(c))
    }
}

/// A style book from the files: skin sheets and colour files in index order, then the base ones.
pub fn style_book(files: &Files) -> StyleBook {
    let idx = files.ui.index();
    let load = |path: &str| -> Option<Rc<XmlElem>> {
        let info = idx.find_path(path)?;
        let bytes = files.src.read(&info.path)?;
        xml::parse(&info.path, &bytes).ok()
    };
    let skin_styles: Vec<Rc<XmlElem>> = idx.skin_styles.iter().filter_map(|p| load(p)).collect();
    let skin_colors: Vec<Rc<XmlElem>> = idx.skin_colors.iter().filter_map(|p| load(p)).collect();
    let base_styles = load("styles.xml");
    let base_colors = load("coloratlas.xml");
    StyleBook::new(
        &skin_styles,
        base_styles.as_ref(),
        &skin_colors,
        base_colors.as_ref(),
    )
}
