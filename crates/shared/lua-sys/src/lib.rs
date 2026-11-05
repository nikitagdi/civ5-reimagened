//! Lua 5.1.5 as C++ with C linkage, plus a safe wrapper. Raw FFI is in [`ffi`], the API is [`Lua`].
//! Rust functions reach Lua through a C++ trampoline, so C++ exceptions never cross Rust frames (P5).

mod convert;
mod error;
pub mod ffi;
mod function;
mod rt;
mod sandbox;
mod state;
mod table;
mod userdata;
mod value;
pub mod wasi;

pub use convert::{FromLua, FromLuaMulti, Nil, ToLua, ToLuaMulti};
pub use error::{LuaError, LuaErrorKind, Result};
pub use function::Chunk;
pub use state::{CallContext, Lua, RustFn, Value};
pub use table::TablePairs;
pub use userdata::{MetaMethod, UserData, UserDataMethods};
pub use value::{
    AnyUserData, Function, LightUserData, LuaString, LuaValue, MultiValue, RegistryKey, Table,
    Thread, Variadic,
};

/// Runs C++ static constructors once per process (wasm only). Other crates must not call
/// `__wasm_call_ctors` directly, since a second call recreates globals.
pub fn init_cpp_runtime() {
    #[cfg(target_arch = "wasm32")]
    {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            // SAFETY: wasm-ld synthesizes `__wasm_call_ctors`; `Once` ensures exactly one call,
            // before any C++ function from the safe wrappers runs.
            unsafe { ffi::__wasm_call_ctors() }
        });
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use rt::DirSource;
pub use rt::{
    Context, ErrorRecord, IncludePolicy, IncludeSource, LogLevel, MemorySource, Runtime,
    RuntimeConfig, StateMode, WeakRuntime,
};
