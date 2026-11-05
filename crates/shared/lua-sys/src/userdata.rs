//! Userdata with a typed Rust payload, methods and metamethods.
//!
//! The payload sits inside the Lua userdata block, behind a `RefCell`. The block starts with the
//! drop function the C++ `__gc` runs (also when the state closes). Each type has one metatable per
//! state, found by `TypeId`; a userdata is accepted as `T` only if its metatable is that one.

use std::cell::{Ref, RefCell, RefMut};
use std::ffi::{c_int, c_void};
use std::marker::PhantomData;
use std::panic::{AssertUnwindSafe, catch_unwind};

use crate::convert::{FromLuaMulti, ToLuaMulti};
use crate::error::{LuaError, LuaErrorKind, Result};
use crate::function::Callback;
use crate::state::{Lua, StackGuard, ensure_stack};
use crate::value::{LuaRef, LuaValue, MultiValue};
use crate::{AnyUserData, ffi};

/// A Rust type usable as Lua userdata.
pub trait UserData: Sized + 'static {
    /// Registers the methods and metamethods.
    fn add_methods<'lua, M: UserDataMethods<'lua, Self>>(_methods: &mut M) {}

    /// Name in error messages.
    fn type_name() -> &'static str {
        std::any::type_name::<Self>()
    }
}

/// Lua metamethods a userdata type can define.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetaMethod {
    /// `obj[key]` for keys that are not methods.
    Index,
    NewIndex,
    /// `obj(...)`.
    Call,
    ToString,
    Eq,
    Len,
    Concat,
    Unm,
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
    Lt,
    Le,
}

impl MetaMethod {
    fn name(self) -> &'static str {
        match self {
            MetaMethod::Index => "__index",
            MetaMethod::NewIndex => "__newindex",
            MetaMethod::Call => "__call",
            MetaMethod::ToString => "__tostring",
            MetaMethod::Eq => "__eq",
            MetaMethod::Len => "__len",
            MetaMethod::Concat => "__concat",
            MetaMethod::Unm => "__unm",
            MetaMethod::Add => "__add",
            MetaMethod::Sub => "__sub",
            MetaMethod::Mul => "__mul",
            MetaMethod::Div => "__div",
            MetaMethod::Mod => "__mod",
            MetaMethod::Pow => "__pow",
            MetaMethod::Lt => "__lt",
            MetaMethod::Le => "__le",
        }
    }
}

/// Collects the methods of a userdata type, see [`UserData::add_methods`].
pub trait UserDataMethods<'lua, T: UserData> {
    /// `obj:name(args)` with `&T`. A call while the payload is mutably borrowed fails with an
    /// error instead of aliasing.
    fn add_method<A, R, F>(&mut self, name: &str, method: F)
    where
        A: FromLuaMulti<'lua>,
        R: ToLuaMulti<'lua>,
        F: Fn(&'lua Lua, &T, A) -> Result<R> + 'static;

    /// `obj:name(args)` with `&mut T`.
    fn add_method_mut<A, R, F>(&mut self, name: &str, method: F)
    where
        A: FromLuaMulti<'lua>,
        R: ToLuaMulti<'lua>,
        F: Fn(&'lua Lua, &mut T, A) -> Result<R> + 'static;

    /// `obj.name(args)`: a function in the method table that gets every argument as it is.
    fn add_function<A, R, F>(&mut self, name: &str, function: F)
    where
        A: FromLuaMulti<'lua>,
        R: ToLuaMulti<'lua>,
        F: Fn(&'lua Lua, A) -> Result<R> + 'static;

    fn add_meta_method<A, R, F>(&mut self, meta: MetaMethod, method: F)
    where
        A: FromLuaMulti<'lua>,
        R: ToLuaMulti<'lua>,
        F: Fn(&'lua Lua, &T, A) -> Result<R> + 'static;

    fn add_meta_method_mut<A, R, F>(&mut self, meta: MetaMethod, method: F)
    where
        A: FromLuaMulti<'lua>,
        R: ToLuaMulti<'lua>,
        F: Fn(&'lua Lua, &mut T, A) -> Result<R> + 'static;

    /// A metamethod that gets every argument as it is (for `__eq` between two objects, `__concat`).
    fn add_meta_function<A, R, F>(&mut self, meta: MetaMethod, function: F)
    where
        A: FromLuaMulti<'lua>,
        R: ToLuaMulti<'lua>,
        F: Fn(&'lua Lua, A) -> Result<R> + 'static;
}

struct Registry<'lua, T> {
    methods: Vec<(String, Callback<'lua>)>,
    metas: Vec<(MetaMethod, Callback<'lua>)>,
    _t: PhantomData<fn(T)>,
}

/// Splits `self` off the front of the arguments.
fn take_self<'lua>(args: &mut MultiValue<'lua>, type_name: &str) -> Result<AnyUserData<'lua>> {
    if args.0.is_empty() {
        return Err(LuaError::conversion("no value", type_name));
    }
    match args.0.remove(0) {
        LuaValue::UserData(u) => Ok(u),
        other => Err(LuaError::new(
            LuaErrorKind::Conversion,
            format!(
                "bad argument #1 ({type_name} expected, got {})",
                other.type_name()
            ),
        )),
    }
}

impl<'lua, T: UserData> Registry<'lua, T> {
    fn method_cb<A, R, F>(method: F) -> Callback<'lua>
    where
        A: FromLuaMulti<'lua>,
        R: ToLuaMulti<'lua>,
        F: Fn(&'lua Lua, &T, A) -> Result<R> + 'static,
    {
        Box::new(move |lua, mut args| {
            let ud = take_self(&mut args, T::type_name())?;
            let this = ud.borrow::<T>()?;
            let a = A::from_lua_multi(args, lua)?;
            method(lua, &this, a)?.to_lua_multi(lua)
        })
    }

    fn method_mut_cb<A, R, F>(method: F) -> Callback<'lua>
    where
        A: FromLuaMulti<'lua>,
        R: ToLuaMulti<'lua>,
        F: Fn(&'lua Lua, &mut T, A) -> Result<R> + 'static,
    {
        Box::new(move |lua, mut args| {
            let ud = take_self(&mut args, T::type_name())?;
            let mut this = ud.borrow_mut::<T>()?;
            let a = A::from_lua_multi(args, lua)?;
            method(lua, &mut this, a)?.to_lua_multi(lua)
        })
    }

    fn function_cb<A, R, F>(function: F) -> Callback<'lua>
    where
        A: FromLuaMulti<'lua>,
        R: ToLuaMulti<'lua>,
        F: Fn(&'lua Lua, A) -> Result<R> + 'static,
    {
        Box::new(move |lua, args| function(lua, A::from_lua_multi(args, lua)?)?.to_lua_multi(lua))
    }
}

impl<'lua, T: UserData> UserDataMethods<'lua, T> for Registry<'lua, T> {
    fn add_method<A, R, F>(&mut self, name: &str, method: F)
    where
        A: FromLuaMulti<'lua>,
        R: ToLuaMulti<'lua>,
        F: Fn(&'lua Lua, &T, A) -> Result<R> + 'static,
    {
        self.methods
            .push((name.to_owned(), Self::method_cb(method)));
    }

    fn add_method_mut<A, R, F>(&mut self, name: &str, method: F)
    where
        A: FromLuaMulti<'lua>,
        R: ToLuaMulti<'lua>,
        F: Fn(&'lua Lua, &mut T, A) -> Result<R> + 'static,
    {
        self.methods
            .push((name.to_owned(), Self::method_mut_cb(method)));
    }

    fn add_function<A, R, F>(&mut self, name: &str, function: F)
    where
        A: FromLuaMulti<'lua>,
        R: ToLuaMulti<'lua>,
        F: Fn(&'lua Lua, A) -> Result<R> + 'static,
    {
        self.methods
            .push((name.to_owned(), Self::function_cb(function)));
    }

    fn add_meta_method<A, R, F>(&mut self, meta: MetaMethod, method: F)
    where
        A: FromLuaMulti<'lua>,
        R: ToLuaMulti<'lua>,
        F: Fn(&'lua Lua, &T, A) -> Result<R> + 'static,
    {
        self.metas.push((meta, Self::method_cb(method)));
    }

    fn add_meta_method_mut<A, R, F>(&mut self, meta: MetaMethod, method: F)
    where
        A: FromLuaMulti<'lua>,
        R: ToLuaMulti<'lua>,
        F: Fn(&'lua Lua, &mut T, A) -> Result<R> + 'static,
    {
        self.metas.push((meta, Self::method_mut_cb(method)));
    }

    fn add_meta_function<A, R, F>(&mut self, meta: MetaMethod, function: F)
    where
        A: FromLuaMulti<'lua>,
        R: ToLuaMulti<'lua>,
        F: Fn(&'lua Lua, A) -> Result<R> + 'static,
    {
        self.metas.push((meta, Self::function_cb(function)));
    }
}

/// The userdata block: the drop function first (the C++ `__gc` reads it), then the payload.
#[repr(C)]
struct Block<T> {
    drop: Option<ffi::lua_sys_drop_fn>,
    cell: RefCell<T>,
}

unsafe extern "C" fn drop_block<T>(block: *mut c_void) {
    // SAFETY: `block` is a Block<T> written by create_userdata; the C++ side calls this once.
    let block = block.cast::<Block<T>>();
    // A panicking Drop must not unwind into C++.
    let _ = catch_unwind(AssertUnwindSafe(|| unsafe {
        std::ptr::drop_in_place(&raw mut (*block).cell);
    }));
}

impl Lua {
    /// Moves `data` into a new userdata. Its methods come from [`UserData::add_methods`].
    pub fn create_userdata<'lua, T: UserData>(&'lua self, data: T) -> Result<AnyUserData<'lua>> {
        if std::mem::align_of::<Block<T>>() > 8 {
            return Err(LuaError::api(format!(
                "{} needs alignment above 8, which Lua userdata cannot give",
                T::type_name()
            )));
        }
        self.expire_registry_values();
        let slot = self.userdata_metatable::<T>()?;
        let l = self.l();
        let _g = StackGuard::new(l);
        // SAFETY: the block is initialised right after allocation (nothing can collect it in
        // between: it is on the stack and the GC is paused); the metatable is set last.
        unsafe {
            ensure_stack(l, 3);
            let p = ffi::lua_sys_newuserdata(l, std::mem::size_of::<Block<T>>()).cast::<Block<T>>();
            std::ptr::write(
                p,
                Block {
                    drop: Some(drop_block::<T>),
                    cell: RefCell::new(data),
                },
            );
            ffi::lua_rawgeti(l, ffi::LUA_REGISTRYINDEX, slot);
            ffi::lua_setmetatable(l, -2);
            Ok(AnyUserData(LuaRef::from_top(self)))
        }
    }

    /// Registry slot of the metatable of `T`, built on first use.
    fn userdata_metatable<'lua, T: UserData>(&'lua self) -> Result<c_int> {
        let id = std::any::TypeId::of::<T>();
        if let Some(slot) = self.inner.type_mts.borrow().get(&id) {
            return Ok(*slot);
        }
        let mut reg = Registry::<T> {
            methods: Vec::new(),
            metas: Vec::new(),
            _t: PhantomData,
        };
        T::add_methods(&mut reg);

        let methods = self.create_table();
        for (name, cb) in reg.methods {
            methods.raw_set(name, self.create_callback(cb))?;
        }
        let mt = self.create_table();
        let mut user_index = None;
        for (meta, cb) in reg.metas {
            if meta == MetaMethod::Index {
                user_index = Some(cb);
            } else {
                mt.raw_set(meta.name(), self.create_callback(cb))?;
            }
        }
        match user_index {
            None => mt.raw_set("__index", methods)?,
            Some(user_index) => {
                // Methods first, then the type's own lookup.
                let key = self.create_registry_value(methods)?;
                let combined: Callback<'lua> = Box::new(move |lua, args| {
                    let table: crate::Table<'lua> = lua.registry_value(&key)?;
                    if let Some(name) = args.0.get(1) {
                        let found: LuaValue<'lua> = table.raw_get(name.clone())?;
                        if !found.is_nil() {
                            return Ok(MultiValue(vec![found]));
                        }
                    }
                    user_index(lua, args)
                });
                mt.raw_set("__index", self.create_callback(combined))?;
            }
        }
        // SAFETY: GC-paused push of the C++ __gc function (no upvalues).
        let gc = unsafe {
            let l = self.l();
            let _g = StackGuard::new(l);
            ensure_stack(l, 2);
            ffi::lua_sys_pushcclosure(l, ffi::lua_sys_gc_block, 0);
            LuaValue::Function(crate::Function(LuaRef::from_top(self)))
        };
        mt.raw_set("__gc", gc)?;
        mt.raw_set("__type", T::type_name())?;

        let l = self.l();
        let _g = StackGuard::new(l);
        // SAFETY: one push; the slot is released only when the state closes.
        unsafe { ensure_stack(l, 1) };
        mt.0.push();
        let slot = self.pop_ref();
        self.inner.type_mts.borrow_mut().insert(id, slot);
        Ok(slot)
    }
}

impl<'lua> AnyUserData<'lua> {
    fn block<T: UserData>(&self) -> Result<*const Block<T>> {
        let lua = self.0.lua;
        let mismatch = || {
            LuaError::new(
                LuaErrorKind::Conversion,
                format!("userdata is not a {}", T::type_name()),
            )
        };
        let slot = *lua
            .inner
            .type_mts
            .borrow()
            .get(&std::any::TypeId::of::<T>())
            .ok_or_else(mismatch)?;
        let l = lua.l();
        let _g = StackGuard::new(l);
        // SAFETY: pushes and raw reads only. The metatable is the type's own, so the block has
        // the layout written by create_userdata.
        unsafe {
            ensure_stack(l, 3);
            self.0.push();
            if ffi::lua_getmetatable(l, -1) == 0 {
                return Err(mismatch());
            }
            ffi::lua_rawgeti(l, ffi::LUA_REGISTRYINDEX, slot);
            if ffi::lua_rawequal(l, -1, -2) == 0 {
                return Err(mismatch());
            }
            Ok(ffi::lua_touserdata(l, -3).cast::<Block<T>>())
        }
    }

    /// True if the payload is a `T`.
    pub fn is<T: UserData>(&self) -> bool {
        self.block::<T>().is_ok()
    }

    /// Borrows the payload. Fails for another type, or while it is mutably borrowed.
    pub fn borrow<T: UserData>(&self) -> Result<Ref<'_, T>> {
        let block = self.block::<T>()?;
        // SAFETY: the handle keeps the userdata alive for as long as `&self`.
        let block = unsafe { &*block };
        block.cell.try_borrow().map_err(|_| {
            LuaError::new(
                LuaErrorKind::Api,
                format!("{} is already mutably borrowed", T::type_name()),
            )
        })
    }

    /// Mutably borrows the payload. Fails for another type, or while it is borrowed.
    pub fn borrow_mut<T: UserData>(&self) -> Result<RefMut<'_, T>> {
        let block = self.block::<T>()?;
        // SAFETY: as in `borrow`.
        let block = unsafe { &*block };
        block.cell.try_borrow_mut().map_err(|_| {
            LuaError::new(
                LuaErrorKind::Api,
                format!("{} is already borrowed", T::type_name()),
            )
        })
    }
}
