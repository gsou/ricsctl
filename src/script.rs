//! Rics scripting features

use super::libloading::{Library, Symbol};
use super::rlua;
use super::server::RICSServer;
use std::sync::Mutex;
use std;

pub trait ScriptingInterface {

    /// Called at ScriptingInterface creation
    fn initialize(&self) -> bool { true }

    /// Called just after server start
    fn start(&self, _svr: &mut RICSServer, _node: i32) -> bool { true }

    /// Can message rx callback
    fn can_rx(&self, _svr: &mut RICSServer, _id: u32, _data: Vec<u8>) -> bool { true }

    /// Generic slave update loop called as often as possible.
    /// This function should return as soon as possible if low latency is required.
    /// This function is only called when there is no more callbacks to call,
    /// so this can never be called in some cases if traffic is high.
    fn update(&self, _svr: &mut RICSServer) -> bool { true }

}

pub struct ScriptingInterfaceWrapper {
    pub iface : Mutex<Box<dyn ScriptingInterface>>
}
unsafe impl Send for ScriptingInterfaceWrapper {}
unsafe impl Sync for ScriptingInterfaceWrapper {}

#[cfg(target_family="unix")]
type DynRawSymbol<T> = libloading::os::unix::Symbol<T>;
#[cfg(target_family="windows")]
type DynRawSymbol<T> = libloading::os::windows::Symbol<T>;

/// Dynamic library script
///
/// # Functions supported
///
/// bool rics_init(void)
/// Called at initialization
///
/// bool rics_start(int32_t node)
/// Called after the system is started
///
/// bool rics_can_callback(uint32_t id, usize_t len, uint8_t* dat)
/// Called when a can message is detected by the program
pub struct DynlibScript {
    /// Needed to keep library alive while calling
    lib: Library,
    // /// Called after before init
    // on_init: *const unsafe extern fn() -> i32,
    // /// Called after system start
    // on_start: *const unsafe extern fn(i32) -> i32,
    /// Called whenever a can message happen
    on_can_msg: Option<DynRawSymbol<unsafe extern fn(u32, usize, *const u8) -> i32>>,
    // TODO Other callbacks
}

impl DynlibScript {
    pub fn new(path: String) -> DynlibScript {
        trace!("Creating DynlibScript from {}", path.clone());
        let lib = Library::new(path).unwrap();

            //let (fn_init, fn_start, fn_can_msg) = {
            //    let func_init: Symbol<unsafe extern fn() -> i32> = lib.get(b"rics_init").unwrap();
            //    let func_start: Symbol<unsafe extern fn(i32) -> i32> = lib.get(b"rics_start").unwrap();
            //    let func_can_msg: Symbol<unsafe extern fn(u32, usize, *const u8) -> i32> = lib.get(b"rics_can_callback").unwrap();
            //    (&*func_init, &*func_start, &*func_can_msg)
            //};

            DynlibScript {
                lib: lib,
                on_can_msg: None,
            }
    }

    pub fn load(mut self) -> Self {
        unsafe {
            let symbol: Symbol<unsafe extern fn(u32, usize, *const u8) -> i32> = self.lib.get(b"rics_can_callback").unwrap();
            self.on_can_msg = Some(symbol.into_raw());
        }
        self
    }
}

impl ScriptingInterface for DynlibScript {
    fn initialize(&self) -> bool {
        0 != unsafe {(&*self.lib.get::<fn() -> i32>(b"rics_init").unwrap())()}
    }
    fn start(&self, _svr: &mut RICSServer, node: i32) -> bool {
        0 != unsafe {(&*self.lib.get::<fn(i32) -> i32>(b"rics_start").unwrap())(node)}
    }
    fn can_rx(&self, _svr: &mut RICSServer, id: u32, data: Vec<u8>) -> bool {
        let len = data.len();
        0 != unsafe {(&*self.on_can_msg.as_ref().unwrap())(id, len, data.as_ptr())}
    }

    /// No update function is implemented since a thread can simply be created in the start phase.
    fn update(&self, _svr: &mut RICSServer) -> bool {
        true
    }
}

struct ServerBox {
    svr: *mut RICSServer
}
unsafe impl Send for ServerBox {}



impl rlua::UserData for ServerBox {
    fn add_methods<'lua, M: rlua::UserDataMethods<'lua, Self>>(methods: &mut M) {
        methods.add_method_mut("send_can", |_, this, (id, dat, dest): (u32,Vec<u8>, Option<i32>)| {
            match dest {
                Some(target) => unsafe {(*this.svr).send_packet_to(super::server::can_packet(id as i32, dat), target); }
                None => unsafe {(*this.svr).send_packet(super::server::can_packet(id as i32, dat)); }
            }
            Ok(())
        });
    }
}

pub struct LuaScript {
    lua: rlua::Lua,
    on_init: rlua::RegistryKey,
    on_start: rlua::RegistryKey,
    on_can_msg: rlua::RegistryKey,
    on_update: rlua::RegistryKey
}

impl LuaScript {
    pub fn new(path: String) -> LuaScript {
        let lua = rlua::Lua::new();

        let content = std::fs::read_to_string(path).unwrap();

        let (func_init, func_start, func_can_msg, func_update) = lua.context(move|ctx| {
            if let Err(e) = ctx.load(&content).exec() {
                error!("Lua error loading file: {}", e);
            }

            let func_init = ctx.create_registry_value(match ctx.globals().get("rics_init") {
                Ok(f) => f,
                Err(e) => { error!("Lua error rics_init undefined: {}", e);
                            ctx.create_function(|_,()|Ok(true)).unwrap()
                }
            }).unwrap();
            let func_start = ctx.create_registry_value(match ctx.globals().get("rics_start") {
                Ok(f) => f,
                Err(e) => { error!("Lua error rics_start undefined: {}", e);
                            ctx.create_function(|_,_:i32|Ok(true)).unwrap() }
            }).unwrap();
            let func_can_msg = ctx.create_registry_value(match ctx.globals().get("rics_can_callback") {
                Ok(f) => f,
                Err(e) => { error!("Lua error rics_can_callback undefined: {}", e);
                            ctx.create_function(|_, _:(u32,Vec<u8>)|Ok(true)).unwrap() }
            }).unwrap();
            let func_update = ctx.create_registry_value(match ctx.globals().get("rics_update") {
                Ok(f) => f,
                Err(e) => { error!("Lua error rics_update undefined: {}", e);
                            ctx.create_function(|_, ()|Ok(true)).unwrap() }
            }).unwrap();

            //////// API



            ////////

            (func_init, func_start, func_can_msg, func_update)
        });


        LuaScript {
            lua: lua,
            on_init: func_init,
            on_start: func_start,
            on_can_msg: func_can_msg,
            on_update: func_update,
        }
    }

}

impl ScriptingInterface for LuaScript {
    fn initialize(&self) -> bool {
        self.lua.context(|ctx| { match ctx.registry_value::<rlua::Function>(&self.on_init).unwrap().call(()) {
            Ok(o) => o,
            Err(e) => { error!("Lua error initialization: {}", e); false }
        }})
    }

    fn start(&self, svr: &mut RICSServer, node: i32) -> bool {
        self.lua.context(|ctx| { match ctx.registry_value::<rlua::Function>(&self.on_start).unwrap().call((ServerBox {svr:svr as *mut _},node)) {
            Ok(o) => o,
            Err(e) => { error!("Lua error start: {}", e); false },
        }})
    }

    fn can_rx(&self, svr: &mut RICSServer, id: u32, data: Vec<u8>) -> bool {
        self.lua.context(|ctx| { match ctx.registry_value::<rlua::Function>(&self.on_can_msg).unwrap().call((ServerBox {svr:svr as *mut _}, id,data)) {
            Ok(o) => o,
            Err(e) => { error!("Lua error can_callback: {}", e); false },
        }})
    }

    fn update(&self, svr: &mut RICSServer) -> bool {

        self.lua.context(|ctx| { match ctx.registry_value::<rlua::Function>(&self.on_update).unwrap().call(ServerBox{ svr: svr as *mut RICSServer }) {
            Ok(p) => p,
            Err(e) => { error!("Lua error update: {}", e); false },
        }})
    }
}

pub struct NoEngine;
impl ScriptingInterface for NoEngine {}

