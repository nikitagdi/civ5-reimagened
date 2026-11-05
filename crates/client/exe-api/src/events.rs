//! `Events`, `LuaEvents`, `GameEvents`: the event objects Lua code listens to and fires.
//!
//! An event object has `Add(f [, key])`, `Remove(f | key)`, `RemoveAll()`, `Count()`, `Call(...)`,
//! `CallImmediate(...)` and is callable (`Events.X(...)`). Listeners run in FIFO order of registration.
//!
//! Dispatch: events of `SerialContainer` (and everything the game core sends) go to one FIFO that `ExeApi::dispatch`
//! drains once per frame; an event fired from a listener goes to the end of the same FIFO and runs in the same
//! pass. `LocalMachineContainer` events (and `CallImmediate`) run at once. Every listener runs through
//! `Context::call_key`, so a failing listener is counted and logged and the others still run (AC-A4).

use std::rc::{Rc, Weak};

use lua_sys::{Function, Lua, LuaError, LuaValue, MultiValue, Result, Table};

use crate::core::{Arg, Core, Family, Fire, Handler, Hook, Payload, pack_lua, unpack_lua};
use crate::event_table::{Container, container_of};
use crate::registry::Status;

/// `ButtonPopupTypes::BUTTONPOPUP_CHOOSEPRODUCTION` (CvEnums.h: the 17th entry after BUTTONPOPUP_TEXT = 0).
/// The scripts of the "look here" effect: the VisEffect `ART_DEF_VEFFECT_LOOK_AT` and the event effect `GameplayFX`.
const LOOK_AT_SOUNDS: [&str; 2] = ["AS2D_WORLD_RAY_OF_GOD", "AS2D_BUILD_UNIT"];
const POPUP_CHOOSEPRODUCTION: i32 = 16;

/// The longest FIFO pass (events handled in one `dispatch`) before it is cut off with an error.
const MAX_PASS: usize = 200_000;

impl Core {
    // ---- listeners ----

    pub fn add_handler(&self, family: Family, name: &str, h: Handler) {
        self.handlers
            .borrow_mut()
            .entry((family, name.to_owned()))
            .or_default()
            .push(Rc::new(h));
    }

    fn snapshot(&self, family: Family, name: &str) -> Vec<Rc<Handler>> {
        let mut v = self
            .handlers
            .borrow()
            .get(&(family, name.to_owned()))
            .cloned()
            .unwrap_or_default();
        // SIMPLIFIED(SIMP-exe-330): the gather event of the diplomacy corner's information buttons runs its listeners
        // last-registered first. The EUI NotificationPanel (loaded before the popups) builds the corner's buttons from
        // the entries that the other listeners added, so it has to run after them; with the registration order the
        // espionage, religion and World Congress buttons never appeared. The order of the other events is untouched
        // (the exe's own order is not reversed from the binary).
        if family == Family::LuaEvents && name == "AdditionalInformationDropdownGatherEntries" {
            v.reverse();
        }
        v
    }

    pub fn handler_count(&self, family: Family, name: &str) -> usize {
        self.snapshot(family, name).len()
    }

    pub fn remove_all(&self, family: Family, name: &str) {
        if let Some(v) = self
            .handlers
            .borrow_mut()
            .remove(&(family, name.to_owned()))
        {
            for h in v {
                h.removed.set(true);
            }
        }
    }

    pub fn remove_by_key(&self, family: Family, name: &str, key: &str) -> usize {
        self.remove_where(family, name, |h| h.name_key.as_deref() == Some(key))
    }

    pub fn remove_where(
        &self,
        family: Family,
        name: &str,
        pred: impl Fn(&Handler) -> bool,
    ) -> usize {
        let mut map = self.handlers.borrow_mut();
        let Some(v) = map.get_mut(&(family, name.to_owned())) else {
            return 0;
        };
        let before = v.len();
        v.retain(|h| {
            let hit = pred(h);
            if hit {
                h.removed.set(true);
            }
            !hit
        });
        before - v.len()
    }

    // ---- firing ----

    /// How an event fired from Lua or from the core is handed over.
    pub fn container(family: Family, name: &str) -> Container {
        match family {
            Family::Events => container_of(name).unwrap_or(Container::Serial),
            Family::LuaEvents | Family::GameEvents => Container::Immediate,
        }
    }

    /// Fires by the container rule: queued, or at once (when the runtime is known; before it, queued).
    pub fn fire(&self, f: Fire) {
        match Self::container(f.family, &f.name) {
            Container::Serial => self.enqueue(f),
            Container::Immediate if self.has_runtime() => self.run(&f),
            Container::Immediate => self.enqueue(f),
        }
    }

    /// An `Events.<name>` made by Rust.
    pub fn fire_events(&self, name: &str, args: Vec<Arg>) {
        self.fire(Fire::events(name, args));
    }

    pub fn enqueue(&self, f: Fire) {
        self.queue.borrow_mut().push_back(f);
    }

    pub fn queue_len(&self) -> usize {
        self.queue.borrow().len()
    }

    /// Handles the FIFO until it is empty. Not re-entrant: a call from a listener returns at once (its events are
    /// in the FIFO and the outer loop handles them).
    pub fn drain(&self) {
        if self.draining.replace(true) {
            return;
        }
        let mut n = 0usize;
        loop {
            let next = self.queue.borrow_mut().pop_front();
            let Some(f) = next else { break };
            self.run(&f);
            n += 1;
            if n >= MAX_PASS {
                tracing::error!(target: "exe.events", "event pass cut off after {n} events (a listener fires without end?)");
                self.queue.borrow_mut().clear();
                break;
            }
        }
        self.draining.set(false);
    }

    /// Runs the exe's own reaction, then every listener in order.
    pub fn run(&self, f: &Fire) {
        self.counters.events.set(self.counters.events.get() + 1);
        if let Some(h) = f.hook {
            self.run_hook(h);
        }
        self.run_tap(f);
        self.watch_music(f);
        for h in self.snapshot(f.family, &f.name) {
            if h.removed.get() {
                continue;
            }
            let Some(ctx) = self.context_for(&h.ctx) else {
                continue;
            };
            let lua = ctx.lua();
            let args = match &f.payload {
                Payload::Args(a) => a.iter().map(|a| a.to_lua(lua)).collect::<Result<Vec<_>>>(),
                Payload::Lua(k) => unpack_lua(lua, k),
            };
            let args = match args {
                Ok(a) => a,
                Err(e) => {
                    tracing::warn!(target: "exe.events", "{}.{}: arguments: {e}", f.family.global(), f.name);
                    continue;
                }
            };
            // The error is counted and logged by the context; the next listener still runs.
            let _ = ctx.call_key::<_, ()>(&h.key, MultiValue::from_vec(args));
        }
    }

    /// Shows the event to the tap, if one is set for its name (cheap when none is).
    fn run_tap(&self, f: &Fire) {
        if f.family != Family::Events {
            return;
        }
        let tap = match self.tap.borrow().as_ref() {
            Some(t) if t.names.contains(&f.name) => t.f.clone(),
            _ => return,
        };
        match &f.payload {
            Payload::Args(a) => tap(&f.name, a),
            Payload::Lua(k) => {
                let Some(ctx) = self.lua_ctx() else { return };
                let lua = ctx.lua();
                match unpack_lua(lua, k) {
                    Ok(v) => {
                        let args: Vec<Arg> = v.iter().map(lua_to_arg).collect();
                        tap(&f.name, &args);
                    }
                    Err(e) => {
                        tracing::warn!(target: "exe.events", "Events.{}: tap arguments: {e}", f.name);
                    }
                }
            }
        }
    }

    fn run_hook(&self, h: Hook) {
        let fires = match h {
            Hook::ExitToMainMenu => {
                self.host.exit_to_lobby();
                Vec::new()
            }
            Hook::PopupProcessed(t) => self.buddy.borrow_mut().processed(t),
            Hook::SetupDone => {
                self.setup_done.set(true);
                self.apply_stored_map_options();
                for f in self.yield_fires(true) {
                    self.enqueue(f);
                }
                self.yields_changed();
                Vec::new()
            }
            Hook::PopupShown(t) => {
                self.buddy.borrow_mut().shown(t);
                Vec::new()
            }
            Hook::AiTurnState { player, active } => {
                self.set_ai_turn_state(player, Some(active));
                Vec::new()
            }
            Hook::ShowMovementRange { player, unit } => self.movement_range_fires(player, unit),
            Hook::ShowAttackTargets { player, unit } => self.attack_targets_fires(player, unit),
            Hook::RequestYieldDisplay(ty, args) => self.yield_display_fires(ty, args),
            Hook::GameplayFx { hx, hy } => {
                // re: cvParticleSimSystem::OnGameplayFX 0x087eb302 (night-3.txt): the vis effects of the entry
                // CRC32("GameplayFX") of the event effects at the hex of the event, scale 1.
                let (x, y) = crate::hex::hex_to_grid(hx, hy);
                // `GameplayDoFX(NULL)` (the production notification) has no plot: (-1, -1).
                if x >= 0 && y >= 0 {
                    self.host.gameplay_fx(x, y);
                    // SIMPLIFIED(SIMP-exe-972): the effect sounds with its sparkle: the VisEffect's own sound
                    // (`AS2D_WORLD_RAY_OF_GOD`) and the EventEffect's (`AS2D_BUILD_UNIT`), both from the art defines the
                    // exe reads; that the exe plays them for this event follows from the players' report (the sparkle
                    // has a sound), not from a trace. Nothing sounds while the replica joins or loads.
                    if !self.sound_quiet() {
                        for script in LOOK_AT_SOUNDS {
                            self.host.effect_sound(script);
                        }
                    }
                }
                Vec::new()
            }
            Hook::MovementIndicator(on) => {
                self.ui.borrow_mut().path_shown = on;
                Vec::new()
            }
            Hook::UnitFlagSelected(o, id) => {
                // The exe selects an own unit whose flag was clicked (an other player's flag does nothing).
                let active = self.lua_ctx().and_then(|ctx| {
                    ctx.lua()
                        .load("return Game and Game.GetActivePlayer and Game.GetActivePlayer()")
                        .set_name("=exe_flag_select")
                        .eval::<Option<f64>>()
                        .ok()
                        .flatten()
                });
                if active.is_some_and(|a| a as i32 == o) {
                    self.op(crate::UiOp::SelectUnit, &[o, id]);
                }
                Vec::new()
            }
            Hook::ExitCityScreen => {
                // The exe's own reaction to a script's exit: the city screen is down (the game side knows it too).
                // The core answers with CITY_SCREEN_CHANGED(0); that echo must not fire the event a second time.
                if self.op(crate::UiOp::IsCityScreenUp, &[]).first() == Some(&1) {
                    self.city_exit_echo.set(self.city_exit_echo.get() + 1);
                    self.op(crate::UiOp::SetCityScreenUp, &[0]);
                }
                self.host.city_screen(false);
                // The exit a script fires (RETURN, Esc, the exit button, another screen's
                // closing) swallows the core's echo, which was the only place the city ambience ended: the loop
                // played on after the screen (live report 2026-10-30). It ends here too.
                self.host.city_ambience(None);
                // CityView.lua reports the city screen opened for a CHOOSEPRODUCTION popup with
                // `SerialEventGameMessagePopupShown` (which counts the type as open) and never sends
                // `Processed` for it. The exe closes that popup with the screen; without it the type stays open
                // and every later "choose production" popup waits behind it for ever (END TURN does nothing).
                if self.buddy.borrow().is_type_open(POPUP_CHOOSEPRODUCTION) {
                    self.buddy.borrow_mut().processed(POPUP_CHOOSEPRODUCTION)
                } else {
                    Vec::new()
                }
            }
        };
        for f in fires {
            self.enqueue(f);
        }
    }
}

/// A Lua value as an `Arg`: a table with numeric `x`, `y` [, `z` [, `w`]] is a `Vec`; another table gives its
/// string-keyed fields that convert (one level); functions and userdata are `Nil`.
fn lua_to_arg(v: &LuaValue<'_>) -> Arg {
    match v {
        LuaValue::Boolean(b) => Arg::Bool(*b),
        LuaValue::Number(n) => Arg::Num(*n),
        LuaValue::String(s) => Arg::Str(s.to_string_lossy().into_owned()),
        LuaValue::Table(t) => {
            let num = |k: &str| {
                t.raw_get::<_, LuaValue>(k).ok().and_then(|v| match v {
                    LuaValue::Number(n) => Some(n),
                    _ => None,
                })
            };
            if let (Some(x), Some(y)) = (num("x"), num("y")) {
                let mut out = vec![x, y];
                if let Some(z) = num("z") {
                    out.push(z);
                    if let Some(w) = num("w") {
                        out.push(w);
                    }
                }
                return Arg::Vec(out);
            }
            let mut fields = Vec::new();
            for (k, v) in t.clone().pairs::<LuaValue, LuaValue>().flatten() {
                let LuaValue::String(k) = k else { continue };
                let a = match &v {
                    LuaValue::Boolean(b) => Arg::Bool(*b),
                    LuaValue::Number(n) => Arg::Num(*n),
                    LuaValue::String(s) => Arg::Str(s.to_string_lossy().into_owned()),
                    _ => continue,
                };
                fields.push((k.to_string_lossy().into_owned(), a));
            }
            Arg::Table(fields)
        }
        _ => Arg::Nil,
    }
}

// ---- the Lua objects ----

fn hook_of(family: Family, name: &str, args: &[LuaValue<'_>]) -> Option<Hook> {
    if family != Family::Events {
        return None;
    }
    let ty = |v: Option<&LuaValue<'_>>| match v {
        Some(LuaValue::Number(n)) => Some(*n as i32),
        Some(LuaValue::Table(t)) => t.raw_get::<_, f64>("Type").ok().map(|n| n as i32),
        _ => None,
    };
    match name {
        "SerialEventGameMessagePopupProcessed" => ty(args.first()).map(Hook::PopupProcessed),
        "SerialEventGameMessagePopupShown" => ty(args.first()).map(Hook::PopupShown),
        "SerialEventExitCityScreen" => Some(Hook::ExitCityScreen),
        "ExitToMainMenu" | "UserRequestClose" => Some(Hook::ExitToMainMenu),
        "ShowMovementRange" => match (args.first(), args.get(1)) {
            (Some(LuaValue::Number(p)), Some(LuaValue::Number(u))) => {
                Some(Hook::ShowMovementRange {
                    player: *p as i32,
                    unit: *u as i32,
                })
            }
            _ => None,
        },
        "ShowAttackTargets" => match (args.first(), args.get(1)) {
            (Some(LuaValue::Number(p)), Some(LuaValue::Number(u))) => {
                Some(Hook::ShowAttackTargets {
                    player: *p as i32,
                    unit: *u as i32,
                })
            }
            _ => None,
        },
        "DisplayMovementIndicator" => Some(Hook::MovementIndicator(matches!(
            args.first(),
            Some(LuaValue::Boolean(true))
        ))),
        "RequestYieldDisplay" => {
            let at = |i: usize| match args.get(i) {
                Some(LuaValue::Number(n)) => *n as i32,
                _ => 0,
            };
            ty(args.first()).map(|t| Hook::RequestYieldDisplay(t, [at(1), at(2), at(3)]))
        }
        "GameplayFX" => match (ty(args.first()), ty(args.get(1))) {
            (Some(hx), Some(hy)) => Some(Hook::GameplayFx { hx, hy }),
            _ => None,
        },
        "SerialEventUnitFlagSelected" => match (ty(args.first()), ty(args.get(1))) {
            (Some(o), Some(id)) => Some(Hook::UnitFlagSelected(o, id)),
            _ => None,
        },
        _ => None,
    }
}

/// The name of the context a function belongs to (`ContextPtr:GetID()` of its environment), and the runtime that
/// runs it, learned on the way.
fn context_of(core: &Core, f: &Function<'_>) -> String {
    let Some(env) = f.environment() else {
        return "lua".to_owned();
    };
    let Ok(LuaValue::UserData(cp)) = env.raw_get::<_, LuaValue>("ContextPtr") else {
        return "lua".to_owned();
    };
    let Ok(r) = cp.borrow::<ui::shared::CtrlRef>() else {
        return "lua".to_owned();
    };
    let Some(sh) = r.sh.upgrade() else {
        return "lua".to_owned();
    };
    if !core.has_runtime() {
        *core.rt.borrow_mut() = Some(sh.rt.downgrade());
    }
    sh.st
        .try_borrow()
        .map(|st| st.node(r.id).id.clone())
        .unwrap_or_else(|_| "lua".to_owned())
}

fn arg_error(what: &str) -> LuaError {
    LuaError::msg(what.to_owned())
}

fn add(core: &Rc<Core>, lua: &Lua, family: Family, name: &str, args: MultiValue<'_>) -> Result<()> {
    let mut v = args.into_vec();
    // `Events.X:Add(f)` passes the event object first.
    if matches!(v.first(), Some(LuaValue::Table(_))) && v.len() > 1 {
        v.remove(0);
    }
    // SIMPLIFIED(SIMP-exe-71): `Events.X.Add(nil)` adds nothing and raises no error (NaturalWonderPopup.lua:123 passes the
    // undefined global `OnClose`; the exe's binding takes it silently).
    if matches!(v.first(), None | Some(LuaValue::Nil)) {
        return Ok(());
    }
    let Some(LuaValue::Function(f)) = v.first().cloned() else {
        return Err(arg_error(&format!(
            "{}.{name}.Add: a function is expected",
            family.global()
        )));
    };
    let name_key = match v.get(1) {
        Some(LuaValue::String(s)) => Some(s.to_string_lossy().into_owned()),
        _ => None,
    };
    let ctx = context_of(core, &f);
    if let Some(k) = &name_key {
        // A listener under the same key is replaced.
        core.remove_by_key(family, name, k);
    }
    let id = core.next_handler.get();
    core.next_handler.set(id + 1);
    let key = lua.create_registry_value(f)?;
    core.add_handler(
        family,
        name,
        Handler {
            id,
            key,
            ctx,
            name_key,
            removed: std::cell::Cell::new(false),
        },
    );
    if family == Family::GameEvents {
        core.subscribe_game_event(name);
    }
    Ok(())
}

fn remove(
    core: &Rc<Core>,
    lua: &Lua,
    family: Family,
    name: &str,
    args: MultiValue<'_>,
) -> Result<usize> {
    let mut v = args.into_vec();
    if matches!(v.first(), Some(LuaValue::Table(_))) && v.len() > 1 {
        v.remove(0);
    }
    match v.first() {
        Some(LuaValue::String(s)) => Ok(core.remove_by_key(family, name, &s.to_string_lossy())),
        Some(LuaValue::Function(f)) => {
            let f = f.clone();
            Ok(core.remove_where(family, name, |h| {
                lua.registry_value::<Function>(&h.key)
                    .map(|g| g == f)
                    .unwrap_or(false)
            }))
        }
        _ => Ok(0),
    }
}

fn fire_from_lua(
    core: &Rc<Core>,
    lua: &Lua,
    family: Family,
    name: &str,
    args: Vec<LuaValue<'_>>,
    immediate: bool,
) -> Result<()> {
    let hook = hook_of(family, name, &args);
    let f = Fire {
        family,
        name: name.to_owned(),
        payload: Payload::Lua(pack_lua(lua, &args)?),
        hook,
    };
    if immediate && core.has_runtime() {
        core.run(&f);
    } else {
        core.fire(f);
    }
    Ok(())
}

fn make_event<'l>(lua: &'l Lua, core: &Rc<Core>, family: Family, name: &str) -> Result<Table<'l>> {
    let t = lua.create_table();
    let weak: Weak<Core> = Rc::downgrade(core);
    let gone = || LuaError::msg("the exe API is gone");

    {
        let (weak, n) = (weak.clone(), name.to_owned());
        t.raw_set(
            "Add",
            lua.create_function(move |lua, args: MultiValue| {
                let core = weak.upgrade().ok_or_else(gone)?;
                add(&core, lua, family, &n, args)
            }),
        )?;
    }
    {
        let (weak, n) = (weak.clone(), name.to_owned());
        t.raw_set(
            "Remove",
            lua.create_function(move |lua, args: MultiValue| {
                let core = weak.upgrade().ok_or_else(gone)?;
                remove(&core, lua, family, &n, args).map(|_| ())
            }),
        )?;
    }
    {
        let (weak, n) = (weak.clone(), name.to_owned());
        t.raw_set(
            "RemoveAll",
            lua.create_function(move |_, _: MultiValue| {
                let core = weak.upgrade().ok_or_else(gone)?;
                core.remove_all(family, &n);
                Ok(())
            }),
        )?;
    }
    {
        let (weak, n) = (weak.clone(), name.to_owned());
        t.raw_set(
            "Count",
            lua.create_function(move |_, _: MultiValue| {
                let core = weak.upgrade().ok_or_else(gone)?;
                Ok(core.handler_count(family, &n))
            }),
        )?;
    }
    {
        let (weak, n) = (weak.clone(), name.to_owned());
        t.raw_set(
            "Call",
            lua.create_function(move |lua, args: MultiValue| {
                let core = weak.upgrade().ok_or_else(gone)?;
                fire_from_lua(&core, lua, family, &n, args.into_vec(), false)
            }),
        )?;
    }
    {
        let (weak, n) = (weak.clone(), name.to_owned());
        t.raw_set(
            "CallImmediate",
            lua.create_function(move |lua, args: MultiValue| {
                let core = weak.upgrade().ok_or_else(gone)?;
                fire_from_lua(&core, lua, family, &n, args.into_vec(), true)
            }),
        )?;
    }

    // `Events.X(...)`: the first argument is the event object.
    let meta = lua.create_table();
    {
        let (weak, n) = (weak, name.to_owned());
        meta.raw_set(
            "__call",
            lua.create_function(move |lua, args: MultiValue| {
                let core = weak.upgrade().ok_or_else(gone)?;
                let mut v = args.into_vec();
                if !v.is_empty() {
                    v.remove(0);
                }
                fire_from_lua(&core, lua, family, &n, v, false)
            }),
        )?;
    }
    t.set_metatable(Some(meta));
    Ok(t)
}
/// Puts `Events`, `LuaEvents`, `GameEvents` into the base globals.
pub fn install(core: &Rc<Core>, lua: &Lua, g: &Table<'_>) -> Result<()> {
    {
        let mut reg = core.inner.registry.borrow_mut();
        for scope in ["Events", "LuaEvents", "GameEvents"] {
            reg.add(scope, Status::Real);
        }
        for name in crate::event_table::all_names() {
            reg.add(format!("Events.{name}"), Status::Real);
        }
    }
    for family in [Family::Events, Family::LuaEvents, Family::GameEvents] {
        let ns = lua.create_table();
        let meta = lua.create_table();
        let weak = Rc::downgrade(core);
        meta.raw_set(
            "__index",
            lua.create_function(move |lua, (t, name): (Table, LuaValue)| {
                let LuaValue::String(name) = name else {
                    return Ok(LuaValue::Nil);
                };
                let name = name.to_string_lossy().into_owned();
                let Some(core) = weak.upgrade() else {
                    return Ok(LuaValue::Nil);
                };
                if family == Family::Events && container_of(&name).is_none() {
                    // SIMPLIFIED(SIMP-exe-32): a name the exe does not register still gives an event object
                    // (the EUI files name events of Beyond Earth and of mods); it is counted.
                    core.simp_call(&format!("Events.{name}"));
                }
                let ev = make_event(lua, &core, family, &name)?;
                t.raw_set(name.as_str(), ev.clone())?;
                Ok(LuaValue::Table(ev))
            }),
        )?;
        ns.set_metatable(Some(meta));
        g.raw_set(family.global(), ns)?;
    }
    Ok(())
}

impl Core {
    /// The Lua side listens to `GameEvents.<name>`: the game is told (now, or when it is bound).
    pub fn subscribe_game_event(&self, name: &str) {
        let new = self.game_subscribed.borrow_mut().insert(name.to_owned());
        if new
            && let Some(g) = self.game()
            && let Ok(mut g) = g.try_borrow_mut()
        {
            g.subscribe_game_event(name);
        }
    }
}
