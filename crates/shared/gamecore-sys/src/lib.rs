//! FFI to gamecore's C ABI v2 (`cpp/include/gamecore_abi.h`) and the thin safe [`Game`] wrapper.

pub mod ffi;
pub mod spike;

/// Which gamecore is linked: `"lekmod"` (real archives from `GAMECORE_LIB_DIR`) or `"stub"`.
pub const IMPLEMENTATION: &str = if cfg!(gamecore_real) {
    "lekmod"
} else {
    "stub"
};

use std::cell::RefCell;
use std::ffi::{CStr, CString, c_char, c_void};
use std::fmt;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};

/// ABI version of the linked gamecore.
pub fn abi_version() -> u32 {
    lua_sys::init_cpp_runtime();
    // SAFETY: no args; constant.
    unsafe { ffi::gc_abi_version() }
}

/// Game event as the core reports it (`gc_event`); borrowed for the callback only.
#[derive(Debug, Clone, Copy)]
pub struct RawEvent<'a> {
    pub kind: i32,
    pub args: &'a [i32],
    pub text: Option<&'a str>,
    pub text2: Option<&'a str>,
}

/// Host callbacks, run on the calling thread.
pub trait Host: Send {
    fn log(&self, level: i32, msg: &str);
    fn read_file(&self, path: &str) -> Option<Vec<u8>>;
    fn game_event(&self, ev: &RawEvent<'_>);
}

/// Error of a `gc_*` call. Variants with a message carry the `gc_error.message` text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GcError {
    AlreadyExists,
    InvalidArg(String),
    WrongMode,
    BadData(String),
    /// The game refused: `reason` is `gc_error.reason` (`ffi::GC_REASON_*`, 0 = none given).
    Rejected {
        reason: i32,
        message: String,
    },
    NotStarted,
    NotFound,
    /// The turn cannot continue without UI (`gc_pump`).
    Stuck(String),
    /// Declared in the ABI, not built yet in the linked core.
    NotImplemented,
    /// B4 gc-replica: the replica left the server's game (`gc_journal_apply`); it needs a resync.
    Diverged(String),
    Internal(String),
    Unknown(i32),
}

impl GcError {
    /// `reason` is `gc_error.reason`; only `GC_ERR_REJECTED` carries it.
    fn from_code(code: i32, message: String, reason: i32) -> Self {
        match code {
            ffi::GC_ERR_ALREADY_EXISTS => GcError::AlreadyExists,
            ffi::GC_ERR_INVALID_ARG => GcError::InvalidArg(message),
            ffi::GC_ERR_WRONG_MODE => GcError::WrongMode,
            ffi::GC_ERR_BAD_DATA => GcError::BadData(message),
            ffi::GC_ERR_REJECTED => GcError::Rejected { reason, message },
            ffi::GC_ERR_NOT_STARTED => GcError::NotStarted,
            ffi::GC_ERR_NOT_FOUND => GcError::NotFound,
            ffi::GC_ERR_STUCK => GcError::Stuck(message),
            ffi::GC_ERR_NOT_IMPLEMENTED => GcError::NotImplemented,
            ffi::GC_ERR_DIVERGED => GcError::Diverged(message),
            ffi::GC_ERR_INTERNAL => GcError::Internal(message),
            other => GcError::Unknown(other),
        }
    }
}

impl fmt::Display for GcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GcError::AlreadyExists => f.write_str("gamecore instance or game already exists"),
            GcError::InvalidArg(why) => write!(f, "invalid argument: {why}"),
            GcError::WrongMode => f.write_str("call not allowed in this mode"),
            GcError::BadData(why) => write!(f, "malformed or incompatible save data: {why}"),
            GcError::Rejected { message, .. } => write!(f, "command rejected: {message}"),
            GcError::NotStarted => f.write_str("game not started"),
            GcError::NotFound => f.write_str("not found"),
            GcError::Stuck(why) => write!(f, "turn cannot continue: {why}"),
            GcError::NotImplemented => f.write_str("not implemented in the linked gamecore"),
            GcError::Diverged(why) => write!(f, "replica diverged from the server: {why}"),
            GcError::Internal(why) => write!(f, "internal gamecore error: {why}"),
            GcError::Unknown(c) => write!(f, "unknown gamecore error code {c}"),
        }
    }
}

impl std::error::Error for GcError {}

/// Message of a `gc_error`, always NUL-terminated here.
fn error_message(err: &mut ffi::gc_error) -> String {
    err.message[255] = 0;
    // SAFETY: last byte zeroed above.
    unsafe { CStr::from_ptr(err.message.as_ptr()) }
        .to_string_lossy()
        .into_owned()
}

fn check(code: i32, err: &mut ffi::gc_error) -> Result<(), GcError> {
    if code == ffi::GC_OK {
        Ok(())
    } else {
        Err(GcError::from_code(code, error_message(err), err.reason))
    }
}

/// `gc_pump_result`: the state after a `pump` call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PumpResult {
    pub turn: i32,
    /// `ffi::GC_PUMP_*`.
    pub status: i32,
    /// Who is moving now; -1 = none.
    pub active_player: i32,
    /// `EndTurnBlockingTypes` of the human; -1 = none.
    pub blocker: i32,
    pub slices_used: i32,
    /// Humans the turn still waits for (v2).
    pub humans_left: i32,
}

impl From<ffi::gc_pump_result> for PumpResult {
    fn from(r: ffi::gc_pump_result) -> Self {
        PumpResult {
            turn: r.turn,
            status: r.status,
            active_player: r.active_player,
            blocker: r.blocker,
            slices_used: r.slices_used,
            humans_left: r.humans_left,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Command<'a> {
    pub kind: u16,
    pub player: u8,
    pub args: &'a [i32],
    pub text: Option<&'a str>,
}

static INSTANCE_EXISTS: AtomicBool = AtomicBool::new(false);

/// One event of a replica's UI as the sink gave it (`gc_ui_event`, copied; B4 M2 WO-1).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RawUiEvent {
    /// `ffi::GC_UI_EV_*`.
    pub kind: i32,
    /// `ffi::GC_UI_F_*`.
    pub flags: u32,
    pub args: Vec<i32>,
    pub text: Option<String>,
    pub text2: Option<String>,
    /// The values of GAME_EVENT / LUA_EVENT (`ffi::GC_UI_VAL_*` blob) or the path of UNIT_MOVED (u32 plot indices).
    pub data: Vec<u8>,
}

/// gamecore instance. One per process, not `Sync`. All calls run on one thread at a time.
pub struct Game {
    ptr: NonNull<ffi::gc_instance>,
    // Double Box: gc_host::user is a thin pointer.
    _host: Box<Box<dyn Host>>,
    // The events of the UI sink (replica role only); boxed so the pointer given to the core stays put.
    ui_events: Box<RefCell<Vec<RawUiEvent>>>,
}

// SAFETY: calls take &mut, one thread at a time; Host is Send.
unsafe impl Send for Game {}

impl Drop for Game {
    fn drop(&mut self) {
        // SAFETY: created once by gc_create; host still alive.
        unsafe { ffi::gc_destroy(self.ptr.as_ptr()) }
        // Pairs with the Acquire in create.
        INSTANCE_EXISTS.store(false, Ordering::Release);
    }
}

impl Game {
    /// Fails with `AlreadyExists` if an instance exists.
    pub fn create(host: Box<dyn Host>) -> Result<Self, GcError> {
        lua_sys::init_cpp_runtime();
        if INSTANCE_EXISTS
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            return Err(GcError::AlreadyExists);
        }
        let host = Box::new(host);
        let raw_host = ffi::gc_host {
            user: (&*host as *const Box<dyn Host>).cast_mut().cast::<c_void>(),
            log: Some(host_log),
            read_file: Some(host_read_file),
            game_event: Some(host_game_event),
        };
        let mut err = ffi::gc_error::zeroed();
        // SAFETY: gc_create copies raw_host; user outlives the instance; err valid.
        let ptr = unsafe { ffi::gc_create(&raw_host, &mut err) };
        let Some(ptr) = NonNull::new(ptr) else {
            INSTANCE_EXISTS.store(false, Ordering::Release);
            let code = if err.code == ffi::GC_OK {
                ffi::GC_ERR_INTERNAL
            } else {
                err.code
            };
            return Err(GcError::from_code(
                code,
                error_message(&mut err),
                err.reason,
            ));
        };
        let ui_events: Box<RefCell<Vec<RawUiEvent>>> = Box::default();
        let sink = ffi::gc_ui_sink {
            user: (&*ui_events as *const RefCell<Vec<RawUiEvent>>)
                .cast_mut()
                .cast::<c_void>(),
            event: Some(ui_sink_event),
        };
        let mut game = Game {
            ptr,
            _host: host,
            ui_events,
        };
        // The core sends events only in the replica role, so a server pays nothing for the sink. A core without
        // the UI bridge (an old library) just has no events.
        let mut serr = ffi::gc_error::zeroed();
        // SAFETY: instance alive; the sink is copied, its user pointer lives as long as `game`.
        let _ = unsafe { ffi::gc_set_ui_sink(game.raw(), &sink, &mut serr) };
        Ok(game)
    }

    fn raw(&mut self) -> *mut ffi::gc_instance {
        self.ptr.as_ptr()
    }

    /// Starts a game from the setup JSON. Takes seconds on the real core.
    pub fn new_game(&mut self, setup_json: &str) -> Result<(), GcError> {
        let setup = CString::new(setup_json)
            .map_err(|_| GcError::InvalidArg("setup JSON has a NUL byte".into()))?;
        let mut err = ffi::gc_error::zeroed();
        // SAFETY: instance alive; string lives for the call.
        let code = unsafe { ffi::gc_new_game(self.raw(), setup.as_ptr(), &mut err) };
        check(code, &mut err)
    }

    /// Loads `gc_save` bytes instead of `new_game`.
    pub fn load_game(&mut self, bytes: &[u8]) -> Result<(), GcError> {
        let n = u32::try_from(bytes.len())
            .map_err(|_| GcError::InvalidArg("save data longer than 4 GiB".into()))?;
        let mut err = ffi::gc_error::zeroed();
        // SAFETY: instance alive; slice valid for the call.
        let code = unsafe { ffi::gc_load_game(self.raw(), bytes.as_ptr(), n, &mut err) };
        check(code, &mut err)
    }

    /// Runs at most `max_slices` (>= 1) slices of the turn loop. `GC_PUMP_STUCK` comes back as
    /// `Err(Stuck)`.
    pub fn pump(&mut self, max_slices: u32) -> Result<PumpResult, GcError> {
        let mut out = ffi::gc_pump_result::default();
        let mut err = ffi::gc_error::zeroed();
        // SAFETY: instance alive; out and err valid.
        let code = unsafe { ffi::gc_pump(self.raw(), max_slices, &mut out, &mut err) };
        check(code, &mut err).map(|()| out.into())
    }

    /// Queues a human command. `Rejected` carries the reason.
    pub fn apply_command(&mut self, cmd: &Command<'_>) -> Result<(), GcError> {
        let text = match cmd.text {
            Some(t) => Some(
                CString::new(t)
                    .map_err(|_| GcError::InvalidArg("command text has a NUL byte".into()))?,
            ),
            None => None,
        };
        let raw = ffi::gc_command {
            kind: cmd.kind,
            player: cmd.player,
            reserved: 0,
            argc: u32::try_from(cmd.args.len())
                .map_err(|_| GcError::InvalidArg("too many command args".into()))?,
            args: cmd.args.as_ptr(),
            text: text.as_ref().map_or(std::ptr::null(), |t| t.as_ptr()),
        };
        let mut err = ffi::gc_error::zeroed();
        // SAFETY: instance alive; borrowed inputs live for the call.
        let code = unsafe { ffi::gc_apply_command(self.raw(), &raw, &mut err) };
        check(code, &mut err)
    }

    /// Raw `GCQ1` reply of query `kind` (`ffi::GC_Q_*`); parsing is the caller's job.
    pub fn query(&mut self, kind: i32, args: &[i32]) -> Result<Vec<u8>, GcError> {
        let argc = u32::try_from(args.len())
            .map_err(|_| GcError::InvalidArg("too many query args".into()))?;
        let game = self.raw();
        // SAFETY: instance alive; args valid for the call; the core fills buf.
        take_buf(|b, err| unsafe { ffi::gc_query(game, kind, args.as_ptr(), argc, b, err) })
    }

    /// The game as `GCS1` bytes.
    pub fn save(&mut self) -> Result<Vec<u8>, GcError> {
        let game = self.raw();
        // SAFETY: instance alive; the core fills buf.
        take_buf(|b, err| unsafe { ffi::gc_save(game, b, err) })
    }

    /// The build id of the core (UTF-8, as the `build_id` of a GCS1 save header). Valid right
    /// after `create`, before a game.
    pub fn build_id(&mut self) -> Result<String, GcError> {
        let game = self.raw();
        // SAFETY: instance alive; the core fills buf.
        let bytes = take_buf(|b, err| unsafe { ffi::gc_build_id(game, b, err) })?;
        String::from_utf8(bytes)
            .map_err(|_| GcError::InvalidArg("gc_build_id: the core gave non-UTF-8 bytes".into()))
    }

    /// 64-bit state digest; `viewer_team` is -1 (whole game) or a team id.
    pub fn state_digest(&mut self, viewer_team: i32) -> Result<u64, GcError> {
        let mut out = 0u64;
        let mut err = ffi::gc_error::zeroed();
        // SAFETY: instance alive; out and err valid.
        let code = unsafe { ffi::gc_state_digest(self.raw(), viewer_team, &mut out, &mut err) };
        check(code, &mut err).map(|()| out)
    }

    /// Changes the connection of a human slot (`ffi::GC_SLOT_*`); `nick` is required by a
    /// CONNECT onto an AI slot. Allowed at a safe point only.
    pub fn slot_control(
        &mut self,
        player: i32,
        op: i32,
        nick: Option<&str>,
    ) -> Result<(), GcError> {
        let nick = match nick {
            Some(n) => Some(
                CString::new(n).map_err(|_| GcError::InvalidArg("nick has a NUL byte".into()))?,
            ),
            None => None,
        };
        let mut err = ffi::gc_error::zeroed();
        // SAFETY: instance alive; nick NUL-terminated for the call; err valid.
        let code = unsafe {
            ffi::gc_slot_control(
                self.raw(),
                player,
                op,
                nick.as_ref().map_or(std::ptr::null(), |n| n.as_ptr()),
                &mut err,
            )
        };
        check(code, &mut err)
    }
}

/// B4 gc-replica: the server's journal, the replica's replay and resync, the UI's net messages.
impl Game {
    /// Sets the role (`ffi::GC_ROLE_*`); `player` is the seat a replica's UI serves (-1 = none).
    pub fn set_role(&mut self, role: i32, player: i32) -> Result<(), GcError> {
        let mut err = ffi::gc_error::zeroed();
        // SAFETY: instance alive; err valid.
        let code = unsafe { ffi::gc_set_role(self.raw(), role, player, &mut err) };
        check(code, &mut err)
    }

    /// Server: the inputs since the last call as `GCJ1` bytes; `check` ends a non-empty batch with
    /// the replica digest.
    pub fn journal_take(&mut self, check_digest: bool) -> Result<Vec<u8>, GcError> {
        let game = self.raw();
        let flags = if check_digest {
            ffi::GC_JOURNAL_TAKE_CHECK
        } else {
            0
        };
        // SAFETY: instance alive; the core fills buf.
        take_buf(|b, err| unsafe { ffi::gc_journal_take(game, flags, b, err) })
    }

    /// Replica: replays `GCJ1` bytes; `Err(Diverged)` when the replica left the server's game.
    pub fn journal_apply(&mut self, journal: &[u8]) -> Result<(), GcError> {
        let n = u32::try_from(journal.len())
            .map_err(|_| GcError::InvalidArg("journal longer than 4 GiB".into()))?;
        let mut err = ffi::gc_error::zeroed();
        // SAFETY: instance alive; slice valid for the call.
        let code = unsafe { ffi::gc_journal_apply(self.raw(), journal.as_ptr(), n, &mut err) };
        check(code, &mut err)
    }

    /// Replica: the net messages the UI sent since the last call (`GCN1`).
    pub fn net_take(&mut self) -> Result<Vec<u8>, GcError> {
        let game = self.raw();
        // SAFETY: instance alive; the core fills buf.
        take_buf(|b, err| unsafe { ffi::gc_net_take(game, b, err) })
    }

    /// Server: queues a human UI's net messages (`GCN1`) as sent by `player`, all or nothing.
    pub fn net_submit(&mut self, player: i32, messages: &[u8]) -> Result<(), GcError> {
        let n = u32::try_from(messages.len())
            .map_err(|_| GcError::InvalidArg("messages longer than 4 GiB".into()))?;
        let mut err = ffi::gc_error::zeroed();
        // SAFETY: instance alive; slice valid for the call.
        let code =
            unsafe { ffi::gc_net_submit(self.raw(), player, messages.as_ptr(), n, &mut err) };
        check(code, &mut err)
    }

    /// Replaces the running game with `GCS1` bytes in place.
    pub fn resync(&mut self, snapshot: &[u8]) -> Result<(), GcError> {
        let n = u32::try_from(snapshot.len())
            .map_err(|_| GcError::InvalidArg("snapshot longer than 4 GiB".into()))?;
        let mut err = ffi::gc_error::zeroed();
        // SAFETY: instance alive; slice valid for the call.
        let code = unsafe { ffi::gc_resync(self.raw(), snapshot.as_ptr(), n, &mut err) };
        check(code, &mut err)
    }
}

/// B4 M2 WO-1: the UI bridge of a replica (`gamecore_abi.h`).
impl Game {
    /// Puts the DLL's Lua libraries (`Game`, `Players`, `Map`, `GameInfo`, `DB.Query`, ...) into the UI's Lua state.
    ///
    /// # Safety
    /// `lua` must be a live Lua state of the Lua this crate links, and the game must outlive it (`GameInfo` holds the database).
    pub unsafe fn lua_attach_ui(
        &mut self,
        lua: *mut lua_sys::ffi::lua_State,
    ) -> Result<(), GcError> {
        let mut err = ffi::gc_error::zeroed();
        // SAFETY: instance alive; `lua` per the contract.
        let code = unsafe { ffi::gc_lua_attach_ui(self.raw(), lua, &mut err) };
        check(code, &mut err)
    }

    /// After a resync: objects of the old game in the UI's Lua state raise a Lua error when used.
    ///
    /// # Safety
    /// `lua` must be a live Lua state of the Lua this crate links.
    pub unsafe fn lua_invalidate_ui(
        &mut self,
        lua: *mut lua_sys::ffi::lua_State,
    ) -> Result<(), GcError> {
        let mut err = ffi::gc_error::zeroed();
        // SAFETY: instance alive; `lua` per the contract.
        let code = unsafe { ffi::gc_lua_invalidate_ui(self.raw(), lua, &mut err) };
        check(code, &mut err)
    }

    /// Moves the UI events since the last call to `out` (replica role).
    pub fn take_ui_events(&mut self, out: &mut Vec<RawUiEvent>) {
        out.append(&mut self.ui_events.borrow_mut());
    }

    /// Operation on the exe's interface state (`ffi::GC_UI_OP_*`); the answer is appended to `out`.
    pub fn ui_op(&mut self, op: i32, args: &[i32], out: &mut Vec<i32>) -> Result<(), GcError> {
        let n_in = u32::try_from(args.len())
            .map_err(|_| GcError::InvalidArg("too many arguments".into()))?;
        let mut buf = [0_i32; 4096];
        let mut n_out = buf.len() as u32;
        let mut err = ffi::gc_error::zeroed();
        // SAFETY: instance alive; slices valid for the call.
        let code = unsafe {
            ffi::gc_ui_op(
                self.raw(),
                op,
                if args.is_empty() {
                    std::ptr::null()
                } else {
                    args.as_ptr()
                },
                n_in,
                buf.as_mut_ptr(),
                &mut n_out,
                &mut err,
            )
        };
        check(code, &mut err)?;
        out.extend_from_slice(&buf[..(n_out as usize).min(buf.len())]);
        Ok(())
    }

    /// The UI listens to `GameEvents.<name>`: after the core's listeners it gets the call as a GAME_EVENT.
    pub fn ui_subscribe_game_event(&mut self, name: &str) -> Result<(), GcError> {
        let name = CString::new(name)
            .map_err(|_| GcError::InvalidArg("event name has a NUL byte".into()))?;
        let mut err = ffi::gc_error::zeroed();
        // SAFETY: instance alive; string lives for the call.
        let code = unsafe { ffi::gc_ui_subscribe_game_event(self.raw(), name.as_ptr(), &mut err) };
        check(code, &mut err)
    }

    /// After a load or resync: the engine announces everything alive to the UI (events flagged `GC_UI_F_REBUILD`).
    pub fn ui_setup_graphical(&mut self) -> Result<(), GcError> {
        let mut err = ffi::gc_error::zeroed();
        // SAFETY: instance alive.
        let code = unsafe { ffi::gc_ui_setup_graphical(self.raw(), &mut err) };
        check(code, &mut err)
    }
}

unsafe extern "C" fn ui_sink_event(user: *mut c_void, ev: *const ffi::gc_ui_event) {
    if user.is_null() || ev.is_null() {
        return;
    }
    // SAFETY: user is the boxed queue of the Game that installed the sink; ev is valid for the callback.
    let (queue, ev) = unsafe { (&*user.cast::<RefCell<Vec<RawUiEvent>>>(), &*ev) };
    let string = |p: *const c_char| {
        (!p.is_null()).then(|| {
            // SAFETY: NUL-terminated for the callback.
            unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
        })
    };
    let n = (ev.n_args as usize).min(ffi::GC_UI_MAX_ARGS);
    let data = if ev.data.is_null() || ev.data_len == 0 {
        Vec::new()
    } else {
        // SAFETY: data_len bytes valid for the callback.
        unsafe { std::slice::from_raw_parts(ev.data, ev.data_len as usize) }.to_vec()
    };
    // The core must not call back into a gc_* entry from the sink, so the queue is never borrowed here.
    if let Ok(mut q) = queue.try_borrow_mut() {
        q.push(RawUiEvent {
            kind: ev.kind,
            flags: ev.flags,
            args: ev.args[..n].to_vec(),
            text: string(ev.text),
            text2: string(ev.text2),
            data,
        });
    }
}

/// Copies out the data of an empty buffer, then releases it.
fn take_buf(
    f: impl FnOnce(*mut ffi::gc_buf, *mut ffi::gc_error) -> i32,
) -> Result<Vec<u8>, GcError> {
    let mut buf = ffi::gc_buf::empty();
    let mut err = ffi::gc_error::zeroed();
    let code = f(&mut buf, &mut err);
    let bytes = if code == ffi::GC_OK && !buf.data.is_null() {
        // SAFETY: GC_OK means data/len are valid.
        unsafe { std::slice::from_raw_parts(buf.data, buf.len) }.to_vec()
    } else {
        Vec::new()
    };
    // SAFETY: filled by this call (or still empty); released once.
    unsafe { ffi::gc_buf_release(&mut buf) };
    check(code, &mut err).map(|()| bytes)
}

/// SAFETY: `user` is the live `Box<dyn Host>` from `Game::create`.
unsafe fn host_ref<'a>(user: *mut c_void) -> &'a dyn Host {
    // SAFETY: see the function contract.
    unsafe { &**user.cast::<Box<dyn Host>>() }
}

unsafe extern "C" fn host_log(user: *mut c_void, level: i32, msg: *const c_char) {
    if msg.is_null() {
        return;
    }
    // SAFETY: user from create; msg NUL-terminated for the callback.
    let (host, msg) = unsafe { (host_ref(user), CStr::from_ptr(msg)) };
    host.log(level, &msg.to_string_lossy());
}

unsafe extern "C" fn host_read_file(
    user: *mut c_void,
    path: *const c_char,
    out: *mut ffi::gc_buf,
) -> i32 {
    if path.is_null() || out.is_null() {
        return ffi::GC_ERR_INVALID_ARG;
    }
    // SAFETY: user from create; path NUL-terminated for the callback.
    let (host, path) = unsafe { (host_ref(user), CStr::from_ptr(path)) };
    match host.read_file(&path.to_string_lossy()) {
        Some(bytes) => {
            let len = bytes.len();
            let data = Box::into_raw(bytes.into_boxed_slice()).cast::<u8>();
            // SAFETY: out non-null; gamecore frees it via release.
            unsafe {
                *out = ffi::gc_buf {
                    data,
                    len,
                    release: Some(release_rust_buf),
                    owner: std::ptr::null_mut(),
                };
            }
            ffi::GC_OK
        }
        None => ffi::GC_ERR_NOT_FOUND,
    }
}

/// Frees a buffer from `host_read_file`.
unsafe extern "C" fn release_rust_buf(buf: *mut ffi::gc_buf) {
    // SAFETY: from Box<[u8]> of length len; called once.
    unsafe {
        let b = &mut *buf;
        drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(
            b.data, b.len,
        )));
        b.data = std::ptr::null_mut();
        b.len = 0;
    }
}

unsafe extern "C" fn host_game_event(user: *mut c_void, ev: *const ffi::gc_event) {
    if ev.is_null() {
        return;
    }
    // SAFETY: user from create; ev and its pointers valid for the callback.
    unsafe {
        let host = host_ref(user);
        let ev = &*ev;
        let args = if ev.argc == 0 || ev.args.is_null() {
            &[][..]
        } else {
            std::slice::from_raw_parts(ev.args, ev.argc as usize)
        };
        let text = |p: *const c_char| (!p.is_null()).then(|| CStr::from_ptr(p).to_string_lossy());
        let (text, text2) = (text(ev.text), text(ev.text2));
        host.game_event(&RawEvent {
            kind: ev.kind,
            args,
            text: text.as_deref(),
            text2: text2.as_deref(),
        });
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier, Mutex};

    use super::*;

    /// Tests using `Game` run one at a time.
    static SERIAL: Mutex<()> = Mutex::new(());

    /// The real core is a process singleton (one create per process); the stub allows a new
    /// instance after `gc_destroy`, so the multi-create tests run on the stub only.
    fn stub() -> bool {
        IMPLEMENTATION == "stub"
    }

    type Events = Arc<Mutex<Vec<(i32, Vec<i32>)>>>;

    #[derive(Clone, Default)]
    struct Rec {
        logs: Arc<Mutex<Vec<String>>>,
        events: Events,
    }

    impl Host for Rec {
        fn log(&self, _level: i32, msg: &str) {
            self.logs.lock().unwrap().push(msg.to_owned());
        }
        fn read_file(&self, path: &str) -> Option<Vec<u8>> {
            (path == "vfs-index.json").then(|| b"{}".to_vec())
        }
        fn game_event(&self, ev: &RawEvent<'_>) {
            self.events
                .lock()
                .unwrap()
                .push((ev.kind, ev.args.to_vec()));
        }
    }

    fn create() -> Result<Game, GcError> {
        Game::create(Box::new(Rec::default()))
    }

    /// `gc_error.reason` reaches the error of a refusal and only of a refusal.
    #[test]
    fn refusal_carries_the_reason() {
        let mut err = ffi::gc_error::zeroed();
        err.code = ffi::GC_ERR_REJECTED;
        err.reason = ffi::GC_REASON_BLOCKED;
        err.message[..4].copy_from_slice(&[b'n', b'o', b'p', b'e'].map(|b| b as c_char));
        assert_eq!(
            check(ffi::GC_ERR_REJECTED, &mut err),
            Err(GcError::Rejected {
                reason: ffi::GC_REASON_BLOCKED,
                message: "nope".into()
            })
        );
        assert_eq!(check(ffi::GC_OK, &mut err), Ok(()));
        assert_eq!(
            check(ffi::GC_ERR_BAD_DATA, &mut err),
            Err(GcError::BadData("nope".into()))
        );
        let text = GcError::Rejected {
            reason: 3,
            message: "x".into(),
        }
        .to_string();
        assert_eq!(text, "command rejected: x");
    }

    #[test]
    fn concurrent_create_yields_one_instance() {
        if !stub() {
            return;
        }
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        const THREADS: usize = 8;
        for _ in 0..50 {
            let start = Barrier::new(THREADS);
            let created = Barrier::new(THREADS);
            let ok = std::thread::scope(|s| {
                let handles: Vec<_> = (0..THREADS)
                    .map(|_| {
                        s.spawn(|| {
                            start.wait();
                            let game = create();
                            // Hold results until every thread has tried.
                            created.wait();
                            match game {
                                Ok(_) => true,
                                Err(GcError::AlreadyExists) => false,
                                Err(e) => panic!("unexpected error: {e}"),
                            }
                        })
                    })
                    .collect();
                handles
                    .into_iter()
                    .map(|h| h.join().unwrap())
                    .filter(|&ok| ok)
                    .count()
            });
            assert_eq!(ok, 1);
        }
    }

    #[test]
    fn stub_lifecycle_pump_and_digest() {
        if !stub() {
            return;
        }
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(abi_version(), ffi::GC_ABI_VERSION);
        let rec = Rec::default();
        let mut game = Game::create(Box::new(rec.clone())).unwrap();
        assert_eq!(game.pump(1), Err(GcError::NotStarted));
        assert_eq!(game.state_digest(-1), Err(GcError::NotStarted));
        assert_eq!(game.build_id().unwrap(), "gamecore-stub/1");

        // Setup validation: unknown key and wrong value type do not use up the one-game slot.
        assert!(matches!(
            game.new_game(r#"{"sed":1}"#),
            Err(GcError::InvalidArg(m)) if m.contains("sed")
        ));
        assert!(matches!(
            game.new_game(r#"{"seed":"x"}"#),
            Err(GcError::InvalidArg(_))
        ));
        assert!(matches!(
            game.new_game("not json"),
            Err(GcError::InvalidArg(_))
        ));
        game.new_game(r#"{"seed":42,"human":{"civ":"CIVILIZATION_ROME"},"ai_count":1}"#)
            .unwrap();
        assert_eq!(game.new_game(r#"{"seed":42}"#), Err(GcError::AlreadyExists));
        assert!(matches!(game.pump(0), Err(GcError::InvalidArg(_))));

        // One slice at a time: the AI runs first, then the human is up.
        let a = game.pump(1).unwrap();
        assert_eq!(a.status, ffi::GC_PUMP_AI_RUNNING);
        assert_eq!(a.slices_used, 1);
        let b = game.pump(1).unwrap();
        assert_eq!(b.status, ffi::GC_PUMP_WAITING_HUMAN);
        assert_eq!((b.turn, b.active_player), (1, 0));
        let idle = game.pump(8).unwrap();
        assert_eq!(idle.status, ffi::GC_PUMP_WAITING_HUMAN);
        assert_eq!(idle.slices_used, 0);
        assert_eq!(
            rec.events.lock().unwrap().as_slice(),
            [
                (ffi::GC_EVENT_TURN_STARTED, vec![1]),
                (ffi::GC_EVENT_PLAYER_TURN_STARTED, vec![0]),
            ]
        );
        assert!(
            rec.logs
                .lock()
                .unwrap()
                .iter()
                .any(|l| l.contains("vfs-index.json has 2 bytes")),
            "read_file went through the host"
        );

        // Same setup, same pumping: same digest. Another seed: another digest.
        let digest = game.state_digest(-1).unwrap();
        assert_eq!(game.state_digest(0).unwrap(), digest);
        assert!(matches!(game.state_digest(-2), Err(GcError::InvalidArg(_))));
        drop(game);
        let digest_of = |seed: u32| {
            let mut g = create().unwrap();
            g.new_game(&format!(r#"{{"seed":{seed}}}"#)).unwrap();
            while g.pump(1).unwrap().status != ffi::GC_PUMP_WAITING_HUMAN {}
            g.state_digest(-1).unwrap()
        };
        assert_eq!(digest_of(42), digest_of(42));
        assert_ne!(digest_of(42), digest_of(43));
    }

    /// The stub plays the B1 game only: a v2 setup key and `gc_slot_control` answer NOT_STARTED,
    /// `gc_error.reason` stays 0, `gc_pump_result.humans_left` follows the human's wait.
    #[test]
    fn stub_v2_entries_answer_not_started() {
        if !stub() {
            return;
        }
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let mut game = create().unwrap();
        let slot = |game: &mut Game, player: i32, op: i32, nick: Option<&CStr>| {
            let mut err = ffi::gc_error::zeroed();
            err.reason = 77;
            // SAFETY: instance alive; nick NUL-terminated for the call; err valid.
            let code = unsafe {
                ffi::gc_slot_control(
                    game.raw(),
                    player,
                    op,
                    nick.map_or(std::ptr::null(), CStr::as_ptr),
                    &mut err,
                )
            };
            (code, err.code, err.reason)
        };
        let no = ffi::GC_ERR_NOT_STARTED;
        assert_eq!(slot(&mut game, 0, ffi::GC_SLOT_KICK, None), (no, no, 0));
        let nick = CString::new("friend").unwrap();
        assert_eq!(
            slot(&mut game, 3, ffi::GC_SLOT_CONNECT, Some(&nick)),
            (no, no, 0)
        );
        let bad = ffi::GC_ERR_INVALID_ARG;
        assert_eq!(slot(&mut game, -1, ffi::GC_SLOT_KICK, None).0, bad);
        assert_eq!(slot(&mut game, 0, 99, None).0, bad);

        for key in [
            r#"{"game_type":"network"}"#,
            r#"{"seed":1,"players":[{"kind":"human"}]}"#,
            r#"{"turn_mode":"hybrid"}"#,
            r#"{"world_builder_scenario":0}"#,
            r#"{"game_type":"single","turn_timer":30}"#,
            r#"{"game_type":"hotseat"}"#,
            r#"{"game_type":1}"#,
        ] {
            assert_eq!(game.new_game(key), Err(GcError::NotStarted), "{key}");
        }
        // game_type "single" is the B1 game: accepted, uses the slot.
        game.new_game(r#"{"game_type":"single","seed":9}"#).unwrap();
        assert_eq!(game.new_game(r#"{"seed":9}"#), Err(GcError::AlreadyExists));
        drop(game);
        let mut game = create().unwrap();
        // Not a game start: the one-game slot is free, B1 setups still work.
        game.new_game(r#"{"seed":5,"ai_count":1}"#).unwrap();
        let a = game.pump(1).unwrap();
        assert_eq!((a.status, a.slices_used), (ffi::GC_PUMP_AI_RUNNING, 1));
        let mut out = ffi::gc_pump_result::default();
        let mut err = ffi::gc_error::zeroed();
        // SAFETY: instance alive; out and err valid.
        let code = unsafe { ffi::gc_pump(game.raw(), 4, &mut out, &mut err) };
        assert_eq!(code, ffi::GC_OK);
        assert_eq!(
            (out.status, out.humans_left),
            (ffi::GC_PUMP_WAITING_HUMAN, 1)
        );
    }

    #[test]
    fn staged_calls_validate_then_report_not_implemented() {
        if !stub() {
            return;
        }
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let mut game = create().unwrap();
        assert!(matches!(game.load_game(b""), Err(GcError::InvalidArg(_))));
        assert!(matches!(game.load_game(b"junk"), Err(GcError::BadData(_))));
        assert!(matches!(
            game.load_game(b"GCS1...."),
            Err(GcError::BadData(_))
        ));
        game.new_game("{}").unwrap();
        let cmd = Command {
            kind: ffi::GC_CMD_END_TURN,
            player: 0,
            args: &[],
            text: None,
        };
        assert_eq!(game.apply_command(&cmd), Err(GcError::NotImplemented));
        assert_eq!(
            game.query(ffi::GC_Q_GAME, &[]),
            Err(GcError::NotImplemented)
        );
    }

    #[test]
    fn stub_save_and_load_in_a_new_instance() {
        if !stub() {
            return;
        }
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let mut game = create().unwrap();
        assert_eq!(game.save(), Err(GcError::NotStarted));
        game.new_game(r#"{"seed":7}"#).unwrap();
        // Not a save point before the pump reaches the human.
        assert!(
            matches!(game.save(), Err(GcError::Rejected { message: m, .. }) if m.contains("save point"))
        );
        while game.pump(1).unwrap().status != ffi::GC_PUMP_WAITING_HUMAN {}
        let bytes = game.save().unwrap();
        assert_eq!(&bytes[..4], b"GCS1");
        let digest = game.state_digest(-1).unwrap();
        // A loaded instance has a game already.
        assert_eq!(game.load_game(&bytes), Err(GcError::AlreadyExists));
        drop(game);

        let mut game = create().unwrap();
        // Corrupt and foreign saves are refused before anything is loaded.
        let mut corrupt = bytes.clone();
        *corrupt.last_mut().unwrap() ^= 1;
        assert!(
            matches!(game.load_game(&corrupt), Err(GcError::BadData(m)) if m.contains("checksum"))
        );
        assert!(
            matches!(game.load_game(&bytes[..bytes.len() - 1]), Err(GcError::BadData(m)) if m.contains("length"))
        );
        game.load_game(&bytes).unwrap();
        assert_eq!(game.state_digest(-1).unwrap(), digest);
        assert_eq!(game.pump(4).unwrap().status, ffi::GC_PUMP_WAITING_HUMAN);
        assert_eq!(
            game.save().unwrap(),
            bytes,
            "saving again gives the same bytes"
        );
        assert_eq!(game.new_game("{}"), Err(GcError::AlreadyExists));
    }
}
