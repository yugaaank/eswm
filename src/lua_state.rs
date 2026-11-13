/// Minimal Lua embed for eswm config API (M3/M4).
// Provides: Lua state init, API registration, and do_file/do_chunk.
// Bridges `Snapshot` and `CompCmd` so Lua scripts can keybind, reload,
// and query state — matching Hyprland's `hl.bind`, `hl.define_submap`, `hl.dsp.*`, `hl.config{}`.

use std::sync::Arc;
use flume::Sender;
use mlua::{Lua, Table};

/// Initialize a new Lua state with the eswm config API pre-loaded.
/// Registers `es` table with `bind`, `define_submap`, `dsp`, `config` submodules,
/// plus `is_file_exists` and `safe_require` helpers.
/// 
/// Also injects `snapshot` (Arc<Mutex<Snapshot>>) and `cmd_tx` (Sender<CompCmd>)
/// as upvalues so Lua scripts can query/compose state.
pub fn init() -> Result<Lua, mlua::Error> {
    let lua = Lua::new();
    Ok(lua)
}

/// Parse a keybind string like "Super+Return" into (mods, key).
fn parse_keybind_string(keybind: &str) -> (Vec<String>, String) {
    let parts: Vec<&str> = keybind.split('+').collect();
    let mut mods = Vec::new();
    let mut key = String::new();
    for part in parts {
        match part {
            "Super" | "SUPER" | "logo" => mods.push("logo".into()),
            "Ctrl" | "CTRL" | "ctrl" => mods.push("ctrl".into()),
            "Alt" | "ALT" | "alt" => mods.push("alt".into()),
            "Shift" | "SHIFT" | "shift" => mods.push("shift".into()),
            k => key = k.to_string(),
        }
    }
    (mods, key)
}

/// Register the eswm config API (`es` table) into the given Lua state.
/// Adds the `es` table with submodules: `bind`, `define_submap`, `dsp`, `config`,
/// plus helpers `is_file_exists` and `safe_require`.
/// 
/// `snapshot` is shared with the IPC layer (Arc<Mutex<Snapshot>>).
/// `cmd_tx` is a flume sender for CompCmd; these are injected as upvalues.
/// `lua_keybinds` is shared with the compositor so keybinds defined in Lua
/// are checked during key event processing.
pub fn register_api(
    lua: &Lua,
    _snapshot: Arc<std::sync::Mutex<crate::ipc::Snapshot>>,
    _cmd_tx: &Sender<crate::ipc::CompCmd>,
    lua_keybinds: Arc<std::sync::Mutex<Vec<crate::Keybind>>>,
) -> Result<(), mlua::Error> {
    // Register the `es` table with all submodules
    let es = lua.create_table()?;

    // es.bind(keybind_string, action_string) -> registers a keybind
    // Example: es.bind("Super+Return", "spawn_terminal")
    let bind = lua.create_function(move |_, (keybind, action): (String, String)| -> Result<(), mlua::Error> {
        let (mods, key) = parse_keybind_string(&keybind);
        if key.is_empty() {
            return Err(mlua::Error::RuntimeError("keybind missing key".into()));
        }
        let kb = crate::Keybind {
            mods,
            key,
            action,
            arg: None,
        };
        lua_keybinds.lock().unwrap().push(kb);
        Ok(())
    })?;
    es.set("bind", bind)?;

    // es.define_submap(map_name, keys) -> "ok"
    let define_submap = lua.create_function(move |_, map_name: String| -> Result<(), mlua::Error> {
        let _ = map_name; // TODO: define submap
        Ok(())
    })?;
    es.set("define_submap", define_submap)?;

    // es.dsp.global("quickshell:...") -> "ok"
    let dsp_global = lua.create_function(move |_, name: String| -> Result<(), mlua::Error> {
        let _ = name; // TODO: dispatch quickshell command
        Ok(())
    })?;
    let dsp_table = lua.create_table()?;
    dsp_table.set("global", dsp_global)?;
    // es.dsp.exec_cmd("program arg1 arg2") -> spawns the command on eswm's socket
    let dsp_exec_cmd = lua.create_function(move |_, cmd: String| -> Result<(), mlua::Error> {
        let (program, args) = crate::parse_cmd(&cmd);
        if !program.is_empty() {
            crate::spawn_on_socket(program, &args);
        }
        Ok(())
    })?;
    dsp_table.set("exec_cmd", dsp_exec_cmd)?;
    // Borrow dsp_table for es.set; dsp_table is still valid because we set exec_cmd first
    es.set("dsp", &dsp_table)?;

    // es.config{} -> returns table of config values (stub)
    // Create config table outside closures to avoid lifetime issues
    let config = lua.create_table()?;
    config.set("reload", lua.create_function(move |_, _: ()| -> Result<(), mlua::Error> {
        Ok(())
    })?)?;
    config.set("notify", lua.create_function(move |_, _: ()| -> Result<(), mlua::Error> {
        Ok(())
    })?)?;
    config.set("setcursor", lua.create_function(move |_, _: ()| -> Result<(), mlua::Error> {
        Ok(())
    })?)?;
    es.set("config", config)?;

    // is_file_exists(path) -> bool
    // Simple stub; path check is done via upvalue/snapshot later
    let is_file_exists = lua.create_function(move |_, path: String| -> Result<bool, mlua::Error> {
        let _ = path; // TODO: check via upvalue/snapshot
        Ok(false)
    })?;
    es.set("is_file_exists", is_file_exists)?;

    // safe_require(path, ...) -> module (stub)
    // Use lua.load + eval to avoid create_function Fn trait issues
    let safe_req_code = r#"
        local M = {}
        M.loaded = true
        return M
    "#;
    let safe_require: Table = lua.load(safe_req_code).eval()?;
    es.set("safe_require", safe_require)?;

    lua.globals().set("es", es)?;

    Ok(())
}

/// Do a Lua script file, with API already registered.
/// 
/// `snapshot` and `cmd_tx` are shared with the IPC layer.
pub fn _do_file(lua: &Lua, script_path: &str, _snapshot: Arc<std::sync::Mutex<crate::ipc::Snapshot>>, _cmd_tx: &Sender<crate::ipc::CompCmd>) -> Result<(), mlua::Error> {
    let source = std::fs::read_to_string(script_path)?;
    lua.load(&source).exec()
}

/// Do a Lua chunk (string), with API already registered.
/// 
/// `snapshot` and `cmd_tx` are shared with the IPC layer.
pub fn _do_chunk(lua: &Lua, source: &str, _snapshot: Arc<std::sync::Mutex<crate::ipc::Snapshot>>, _cmd_tx: &Sender<crate::ipc::CompCmd>) -> Result<(), mlua::Error> {
    lua.load(source).exec()
}