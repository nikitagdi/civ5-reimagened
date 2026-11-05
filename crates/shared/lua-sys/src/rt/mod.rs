//! Context runtime: one Lua environment per Civ5 context, `include`, error isolation.
//!
//! A context is a pair of globals and code. Its globals are a table whose `__index` is the
//! state's base globals, so reads fall through to the exe API and writes stay in the context.
//! Chunks of the context run with that table as their environment (`setfenv`), and so do the
//! functions they define. See the module spec for the two state modes.

mod source;

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::{Rc, Weak};

use crate::{
    FromLuaMulti, Function, Lua, LuaError, LuaString, LuaValue, RegistryKey, Result, Table,
    ToLuaMulti, Variadic,
};

#[cfg(not(target_arch = "wasm32"))]
pub use source::DirSource;
pub use source::{IncludeSource, MemorySource};

const INCLUDE_LUA: &str = include_str!("include.lua");
const INCLUDED_REGISTRY: &str = "lua_rt.included";
const INCLUDE_FN_REGISTRY: &str = "lua_rt.include";

/// How contexts map to Lua states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateMode {
    /// One state; every context has its own globals table in it. Tables set in the base
    /// globals (`MapModData`, exe API objects) are shared by all contexts.
    Shared,
    /// One state per context. Nothing is shared between contexts: tables cannot cross states.
    PerContext,
}

/// What a repeated `include` of the same file in the same context does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IncludePolicy {
    /// Runs the file the first time only (the file is marked before it runs, so cycles end).
    OncePerContext,
    /// Runs the file every time.
    Always,
}

#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    pub mode: StateMode,
    pub include_policy: IncludePolicy,
    /// VM instruction limit per outermost call from the host (`None`: unlimited).
    pub instruction_limit: Option<u32>,
    /// Number of recent errors kept for `Runtime::recent_errors`.
    pub error_ring: usize,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        RuntimeConfig {
            mode: StateMode::Shared,
            include_policy: IncludePolicy::OncePerContext,
            instruction_limit: Some(200_000_000),
            error_ring: 64,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    Error,
}

/// One error that a context call returned.
#[derive(Debug, Clone)]
pub struct ErrorRecord {
    /// 1, 2, ... in order of occurrence.
    pub seq: u64,
    pub context: String,
    pub error: LuaError,
}

type BaseSetup = Rc<dyn Fn(&Lua, &Table<'_>) -> Result<()>>;
/// Answers `true` for an error the host expects (`Runtime::set_error_filter`).
pub type ErrorFilter = Rc<dyn Fn(&LuaError) -> bool>;
type LogSink = Rc<dyn Fn(LogLevel, &str)>;

struct Shared {
    cfg: RuntimeConfig,
    files: Rc<dyn IncludeSource>,
    base_setup: BaseSetup,
    shared_lua: RefCell<Option<Rc<Lua>>>,
    sink: RefCell<Option<LogSink>>,
    log: RefCell<VecDeque<String>>,
    errors: RefCell<VecDeque<ErrorRecord>>,
    error_count: Cell<u64>,
    /// Errors for which this answers `true` are logged but not counted or kept (a host that knows a class of
    /// errors is expected).
    error_filter: RefCell<Option<ErrorFilter>>,
    next_id: Cell<u32>,
}

const LOG_RING: usize = 2000;

impl Shared {
    fn log(&self, level: LogLevel, text: &str) {
        {
            let mut ring = self.log.borrow_mut();
            if ring.len() >= LOG_RING {
                ring.pop_front();
            }
            ring.push_back(match level {
                LogLevel::Info => text.to_owned(),
                LogLevel::Error => format!("ERROR {text}"),
            });
        }
        let sink = self.sink.borrow().clone();
        if let Some(sink) = sink {
            sink(level, text);
        }
    }

    fn record(&self, context: &str, error: &LuaError) {
        let expected = self.error_filter.borrow().clone().is_some_and(|f| f(error));
        if expected {
            self.log(
                LogLevel::Info,
                &format!("[{context}] (expected) {}", error.full_text()),
            );
            return;
        }
        let seq = self.error_count.get() + 1;
        self.error_count.set(seq);
        {
            let mut ring = self.errors.borrow_mut();
            if ring.len() >= self.cfg.error_ring.max(1) {
                ring.pop_front();
            }
            ring.push_back(ErrorRecord {
                seq,
                context: context.to_owned(),
                error: error.clone(),
            });
        }
        self.log(
            LogLevel::Error,
            &format!("[{context}] {}", error.full_text()),
        );
    }

    /// A fresh sandboxed state with `print`, `include` and the host's base globals.
    fn new_state(self: &Rc<Self>) -> Result<Lua> {
        let lua = Lua::new()?;
        self.init_state(&lua)?;
        Ok(lua)
    }

    fn init_state(self: &Rc<Self>, lua: &Lua) -> Result<()> {
        lua.sandbox()?;
        lua.set_instruction_limit(self.cfg.instruction_limit);
        let g = lua.globals();

        let weak = Rc::downgrade(self);
        g.raw_set(
            "print",
            lua.create_function(move |lua, args: Variadic<LuaValue>| {
                let mut line = String::new();
                for (i, v) in args.iter().enumerate() {
                    if i > 0 {
                        line.push('\t');
                    }
                    line.push_str(&lua.tostring(v)?);
                }
                if let Some(shared) = weak.upgrade() {
                    shared.log(LogLevel::Info, &line);
                }
                Ok(())
            }),
        )?;

        // Per-environment set of included files; the environments are weak keys.
        let included = lua.create_table();
        let meta = lua.create_table();
        meta.raw_set("__mode", "k")?;
        included.set_metatable(Some(meta));
        lua.set_named_registry_value(INCLUDED_REGISTRY, included)?;

        let find: Function = {
            let string: Table = g.raw_get("string")?;
            string.raw_get("find")?
        };
        let find_key = lua.create_registry_value(find)?;
        let files = self.files.clone();
        let policy = self.cfg.include_policy;
        let resolve = lua.create_function(move |lua, (env, name, wild): (Table, String, bool)| {
            // Lookup failures are returned as `nil, message` so the prelude can blame the
            // caller of `include`; a syntax error in the file raises with its own position.
            Ok(
                match include_resolve(lua, &*files, policy, &find_key, &env, &name, wild)? {
                    Ok((paths, chunks)) => (LuaValue::Table(paths), LuaValue::Table(chunks)),
                    Err(message) => (LuaValue::Nil, LuaValue::String(message.into())),
                },
            )
        });
        let getfenv: Function = g.raw_get("getfenv")?;
        let type_fn: Function = g.raw_get("type")?;
        let error_fn: Function = g.raw_get("error")?;
        lua.load(INCLUDE_LUA)
            .set_name("=include")
            .into_function()?
            .call::<_, ()>((resolve, getfenv, type_fn, error_fn))?;
        // Kept for `Context::include`; scripts can shadow the global in their environment.
        let include: Function = g.raw_get("include")?;
        lua.set_named_registry_value(INCLUDE_FN_REGISTRY, include)?;

        (self.base_setup)(lua, &g)
    }
}

/// Resolves `name` (a file name, or a Lua pattern when `wild`) to chunks that run in `env`.
fn include_resolve<'lua>(
    lua: &'lua Lua,
    files: &dyn IncludeSource,
    policy: IncludePolicy,
    find_key: &RegistryKey,
    env: &Table<'lua>,
    name: &str,
    wild: bool,
) -> Result<std::result::Result<(Table<'lua>, Table<'lua>), String>> {
    let paths: Vec<String> = if wild {
        let pattern = name.replace('/', "\\");
        let find: Function = lua.registry_value(find_key)?;
        let mut hits = Vec::new();
        let all = files.list();
        // SIMPLIFIED(SIMP-exe-74): a pattern with directories also takes a Lua file of another directory whose name
        // matches the last segment when it is no context (no XML twin): EUI keeps the generic popup layouts
        // ConfirmCommandPopup and ConfirmCityTaskPopup in `Improvements` and `CityView`, outside `PopupsGeneric`.
        let xml_stems: std::collections::HashSet<String> = files.xml_stems().into_iter().collect();
        let last_seg = pattern.rsplit('\\').next().unwrap_or(&pattern).to_owned();
        let has_dirs = pattern.contains('\\');
        for path in all {
            let target = path.replace('/', "\\");
            let mut found: Result<LuaValue> = find.call((target.clone(), pattern.as_str()));
            if has_dirs
                && matches!(&found, Ok(v) if v.is_nil())
                && path.to_ascii_lowercase().ends_with(".lua")
            {
                let base = target.rsplit('\\').next().unwrap_or(&target).to_owned();
                if !xml_stems.contains(&base[..base.len() - 4].to_ascii_lowercase()) {
                    found = find.call((base, last_seg.as_str()));
                }
            }
            let found = match found {
                Ok(f) => f,
                Err(e) => {
                    return Ok(Err(format!("include: bad pattern '{name}': {}", e.message)));
                }
            };
            if !found.is_nil() {
                hits.push(path);
            }
        }
        hits
    } else {
        match files.resolve(name) {
            Some(p) => vec![p],
            None => return Ok(Err(format!("include: file '{name}' not found"))),
        }
    };

    let included: Table = lua.named_registry_value(INCLUDED_REGISTRY)?;
    let seen: Table = match included.raw_get::<_, LuaValue>(env.clone())? {
        LuaValue::Table(t) => t,
        _ => {
            let t = lua.create_table();
            included.raw_set(env.clone(), t.clone())?;
            t
        }
    };
    let chunks = lua.create_table();
    let out_paths = lua.create_table();
    for path in &paths {
        out_paths.push(path.as_str())?;
        let key = path.to_ascii_lowercase();
        if policy == IncludePolicy::OncePerContext && seen.contains_key(key.as_str())? {
            continue;
        }
        let Some(bytes) = files.read(path) else {
            return Ok(Err(format!("include: cannot read '{path}'")));
        };
        let f = lua
            .load(&bytes)
            .set_name(&format!("@{path}"))
            .set_environment(env.clone())
            .into_function()?;
        seen.raw_set(key.as_str(), true)?;
        chunks.push(f)?;
    }
    Ok(Ok((out_paths, chunks)))
}

/// Entry point: creates contexts, counts and logs their errors.
#[derive(Clone)]
pub struct Runtime(Rc<Shared>);

/// A runtime reference that does not keep it alive; for closures stored inside Lua.
#[derive(Clone)]
pub struct WeakRuntime(Weak<Shared>);

impl WeakRuntime {
    pub fn upgrade(&self) -> Option<Runtime> {
        self.0.upgrade().map(Runtime)
    }
}

impl Runtime {
    /// `base_setup` fills the base globals of each new state (once in `Shared` mode, once per
    /// context in `PerContext` mode): the exe API and objects shared by all contexts.
    pub fn new(
        cfg: RuntimeConfig,
        files: Rc<dyn IncludeSource>,
        base_setup: impl Fn(&Lua, &Table<'_>) -> Result<()> + 'static,
    ) -> Result<Runtime> {
        let shared = Rc::new(Shared {
            cfg,
            files,
            base_setup: Rc::new(base_setup),
            shared_lua: RefCell::new(None),
            sink: RefCell::new(None),
            log: RefCell::new(VecDeque::new()),
            errors: RefCell::new(VecDeque::new()),
            error_count: Cell::new(0),
            error_filter: RefCell::new(None),
            next_id: Cell::new(1),
        });
        if shared.cfg.mode == StateMode::Shared {
            let lua = shared.new_state()?;
            *shared.shared_lua.borrow_mut() = Some(Rc::new(lua));
        }
        Ok(Runtime(shared))
    }

    pub fn downgrade(&self) -> WeakRuntime {
        WeakRuntime(Rc::downgrade(&self.0))
    }

    pub fn config(&self) -> &RuntimeConfig {
        &self.0.cfg
    }

    /// Creates a context. `setup` puts what is the context's own into its globals (`ContextPtr`,
    /// `Controls`). The environment is an empty table that reads through to the base globals.
    pub fn create_context(
        &self,
        name: &str,
        setup: impl FnOnce(&Lua, &Table<'_>) -> Result<()>,
    ) -> Result<Context> {
        let lua = match self.0.cfg.mode {
            StateMode::Shared => self
                .0
                .shared_lua
                .borrow()
                .clone()
                .expect("shared state exists in Shared mode"),
            StateMode::PerContext => Rc::new(self.0.new_state()?),
        };
        let env_key = {
            let env = lua.create_table();
            let meta = lua.create_table();
            meta.raw_set("__index", lua.globals())?;
            env.set_metatable(Some(meta));
            env.raw_set("_G", env.clone())?;
            setup(&lua, &env)?;
            lua.create_registry_value(env)?
        };
        let id = self.0.next_id.get();
        self.0.next_id.set(id + 1);
        Ok(Context(Rc::new(ContextData {
            id,
            name: name.to_owned(),
            lua,
            env: env_key,
            rt: Rc::downgrade(&self.0),
        })))
    }

    /// Errors the filter accepts are logged as expected and neither counted nor kept (`None`: every error counts).
    pub fn set_error_filter(&self, f: Option<ErrorFilter>) {
        *self.0.error_filter.borrow_mut() = f;
    }

    /// Errors returned by context calls since the start (AC-A4, `__civ5stats`).
    pub fn error_count(&self) -> u64 {
        self.0.error_count.get()
    }

    /// The last `error_ring` errors, oldest first.
    pub fn recent_errors(&self) -> Vec<ErrorRecord> {
        self.0.errors.borrow().iter().cloned().collect()
    }

    /// Takes the buffered log lines (`print` output and errors, `ERROR ` prefix).
    pub fn take_log(&self) -> Vec<String> {
        self.0.log.borrow_mut().drain(..).collect()
    }

    /// Receives every log line as it is written (in addition to the buffer).
    pub fn set_log_sink(&self, sink: impl Fn(LogLevel, &str) + 'static) {
        *self.0.sink.borrow_mut() = Some(Rc::new(sink));
    }
}

struct ContextData {
    id: u32,
    name: String,
    lua: Rc<Lua>,
    env: RegistryKey,
    rt: Weak<Shared>,
}

/// A Civ5 context: globals and code. Cheap to clone. Every call into it that fails is counted
/// and logged by the [`Runtime`]; other contexts are never affected.
#[derive(Clone)]
pub struct Context(Rc<ContextData>);

impl Context {
    pub fn id(&self) -> u32 {
        self.0.id
    }

    pub fn name(&self) -> &str {
        &self.0.name
    }

    /// The Lua state of this context (shared in `Shared` mode).
    pub fn lua(&self) -> &Lua {
        &self.0.lua
    }

    /// The context's globals.
    pub fn env(&self) -> Table<'_> {
        self.0
            .lua
            .registry_value(&self.0.env)
            .expect("the environment key belongs to this state")
    }

    /// Counts and logs an error from code the host ran on behalf of this context.
    pub fn record_error(&self, error: &LuaError) {
        if let Some(shared) = self.0.rt.upgrade() {
            shared.record(&self.0.name, error);
        }
    }

    fn checked<T>(&self, r: Result<T>) -> Result<T> {
        if let Err(e) = &r {
            self.record_error(e);
        }
        r
    }

    /// Runs source text in the context's globals.
    pub fn exec(&self, source: &str, chunk_name: &str) -> Result<()> {
        let r = self
            .0
            .lua
            .load(source)
            .set_name(chunk_name)
            .set_environment(self.env())
            .exec();
        self.checked(r)
    }

    /// `include(name)` from the context's top level.
    pub fn include(&self, name: &str) -> Result<()> {
        self.include_impl(name, false).map(|_| ())
    }

    /// `include(pattern, true)`; returns the paths of the files that matched.
    pub fn include_pattern(&self, pattern: &str) -> Result<Vec<String>> {
        self.include_impl(pattern, true)
    }

    fn include_impl(&self, name: &str, wild: bool) -> Result<Vec<String>> {
        let lua = &*self.0.lua;
        let r = (|| {
            let include: Function = lua.named_registry_value(INCLUDE_FN_REGISTRY)?;
            // The wrapper runs in the context's globals, so `include` sees them with getfenv(2).
            let wrapper = lua
                .load("local f, n, w = ...\nlocal r = f(n, w)\nreturn r")
                .set_name("=include")
                .set_environment(self.env())
                .into_function()?;
            let paths: LuaValue = wrapper.call((include, name, wild))?;
            Ok(match paths {
                LuaValue::Table(t) => t
                    .sequence_values::<LuaString>()?
                    .iter()
                    .map(|s| s.to_string_lossy().into_owned())
                    .collect(),
                _ => Vec::new(),
            })
        })();
        self.checked(r)
    }

    /// Calls a function found in the context's globals (or the base globals behind them).
    pub fn call_global<'a, A, R>(&'a self, name: &str, args: A) -> Result<R>
    where
        A: ToLuaMulti<'a>,
        R: FromLuaMulti<'a>,
    {
        let r = (|| {
            let f: Function = match self.env().get::<_, LuaValue>(name)? {
                LuaValue::Function(f) => f,
                other => {
                    return Err(LuaError::new(
                        crate::LuaErrorKind::Api,
                        format!("global '{name}' is a {}, not a function", other.type_name()),
                    ));
                }
            };
            f.call(args)
        })();
        self.checked(r)
    }

    /// Calls a function kept with `create_registry_value` (a control callback, an event handler).
    pub fn call_key<'a, A, R>(&'a self, key: &RegistryKey, args: A) -> Result<R>
    where
        A: ToLuaMulti<'a>,
        R: FromLuaMulti<'a>,
    {
        let r = (|| {
            let f: Function = self.0.lua.registry_value(key)?;
            f.call(args)
        })();
        self.checked(r)
    }
}
