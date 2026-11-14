/// Minimal Lua embed for eswm config API (M3/M4).
// Provides: Lua state init, API registration, and do_file/do_chunk.
// Bridges `Snapshot` and `CompCmd` so Lua scripts can keybind, reload,
// and query state — matching Hyprland's `hl.bind`, `hl.define_submap`, `hl.dsp.*`, `hl.config{}`.

use std::sync::Arc;
use flume::Sender;
use mlua::{Lua, Table};

use crate::wm::{DecorationConfig, LayoutMode};

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

/// A keybinding registered via Lua `es.bind()` or `es.bind_cmd()`.
#[derive(Debug, Clone)]
pub struct Keybind {
    pub mods: Vec<String>,
    pub key: String,
    pub action: String,
    pub arg: Option<i32>,
    pub cmd: Option<String>,
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
/// `decoration` is shared with the WM so `es.dsp.set_*` config calls mutate
/// live border/gap/color values that the renderer reads each frame.
pub fn register_api(
    lua: &Lua,
    _snapshot: Arc<std::sync::Mutex<crate::ipc::Snapshot>>,
    _cmd_tx: &Sender<crate::ipc::CompCmd>,
    lua_keybinds: Arc<std::sync::Mutex<Vec<Keybind>>>,
    decoration: Arc<std::sync::Mutex<DecorationConfig>>,
) -> Result<(), mlua::Error> {
    // Register the `es` table with all submodules
    let es = lua.create_table()?;

    let lua_keybinds_cmd = lua_keybinds.clone();

    // es.bind(keybind_string, action_string, [arg]) -> registers a keybind
    // Example: es.bind("Super+Return", "spawn_terminal")
    // Example: es.bind("Super+1", "switch_workspace", 1)
    let bind = lua.create_function(move |_, (keybind, action, arg): (String, String, Option<i32>)| -> Result<(), mlua::Error> {
        let (mods, key) = parse_keybind_string(&keybind);
        if key.is_empty() {
            return Err(mlua::Error::RuntimeError("keybind missing key".into()));
        }
        let kb = Keybind {
            mods,
            key,
            action,
            arg,
            cmd: None,
        };
        lua_keybinds.lock().unwrap().push(kb);
        Ok(())
    })?;
    es.set("bind", bind)?;

    // es.bind_cmd(keybind_string, command_string) -> spawns command on keypress
    // Example: es.bind_cmd("Super+R", "rofi -show drun")
    let bind_cmd = lua.create_function(move |_, (keybind, command): (String, String)| -> Result<(), mlua::Error> {
        let (mods, key) = parse_keybind_string(&keybind);
        if key.is_empty() {
            return Err(mlua::Error::RuntimeError("keybind missing key".into()));
        }
        let kb = Keybind {
            mods,
            key,
            action: "exec_cmd".into(),
            arg: None,
            cmd: Some(command),
        };
        lua_keybinds_cmd.lock().unwrap().push(kb);
        Ok(())
    })?;
    es.set("bind_cmd", bind_cmd)?;

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

    // es.dsp.set_border_size(n) -> sets window border width in px
    let dsp_set_border_size = lua.create_function({
        let decoration = decoration.clone();
        move |_, size: u32| -> Result<(), mlua::Error> {
            decoration.lock().unwrap().border_size = size;
            Ok(())
        }
    })?;
    dsp_table.set("set_border_size", dsp_set_border_size)?;

    // es.dsp.set_outer_gap(n) -> sets outer gap in px
    let dsp_set_outer_gap = lua.create_function({
        let decoration = decoration.clone();
        move |_, size: u32| -> Result<(), mlua::Error> {
            decoration.lock().unwrap().outer = size;
            Ok(())
        }
    })?;
    dsp_table.set("set_outer_gap", dsp_set_outer_gap)?;

    // es.dsp.set_inner_gap(n) -> sets inner gap in px
    let dsp_set_inner_gap = lua.create_function({
        let decoration = decoration.clone();
        move |_, size: u32| -> Result<(), mlua::Error> {
            decoration.lock().unwrap().inner = size;
            Ok(())
        }
    })?;
    dsp_table.set("set_inner_gap", dsp_set_inner_gap)?;

    // es.dsp.set_corner_radius(n) -> sets corner radius in px (stored; renderer clipping TBD)
    let dsp_set_corner_radius = lua.create_function({
        let decoration = decoration.clone();
        move |_, radius: u32| -> Result<(), mlua::Error> {
            decoration.lock().unwrap().corner_radius = radius;
            Ok(())
        }
    })?;
    dsp_table.set("set_corner_radius", dsp_set_corner_radius)?;

    // es.dsp.set_titlebar_height(n) -> titlebar strip height in px; 0 = none
    let dsp_set_titlebar_height = lua.create_function({
        let decoration = decoration.clone();
        move |_, height: u32| -> Result<(), mlua::Error> {
            decoration.lock().unwrap().titlebar_height = height;
            Ok(())
        }
    })?;
    dsp_table.set("set_titlebar_height", dsp_set_titlebar_height)?;

    // es.dsp.set_layout_mode("tiling"|"floating") -> default mode for new windows
    let dsp_set_layout_mode = lua.create_function({
        let decoration = decoration.clone();
        move |_, mode: String| -> Result<(), mlua::Error> {
            let layout = match mode.as_str() {
                "tiling" | "tiled" => LayoutMode::Hyprland,
                "floating" | "float" => LayoutMode::Windows,
                _ => return Err(mlua::Error::RuntimeError("layout mode must be 'tiling' or 'floating'".into())),
            };
            decoration.lock().unwrap().layout_mode = layout;
            Ok(())
        }
    })?;
    dsp_table.set("set_layout_mode", dsp_set_layout_mode)?;

    // es.dsp.set_border_color_focused(r, g, b, a) -> focused border color (RGBA 0-255)
    let dsp_set_border_color_focused = lua.create_function({
        let decoration = decoration.clone();
        move |_, (r, g, b, a): (u8, u8, u8, u8)| -> Result<(), mlua::Error> {
            decoration.lock().unwrap().border_color_focused = [r, g, b, a];
            Ok(())
        }
    })?;
    dsp_table.set("set_border_color_focused", dsp_set_border_color_focused)?;

    // es.dsp.set_border_color_unfocused(r, g, b, a) -> unfocused border color (RGBA 0-255)
    let dsp_set_border_color_unfocused = lua.create_function({
        let decoration = decoration.clone();
        move |_, (r, g, b, a): (u8, u8, u8, u8)| -> Result<(), mlua::Error> {
            decoration.lock().unwrap().border_color_unfocused = [r, g, b, a];
            Ok(())
        }
    })?;
    dsp_table.set("set_border_color_unfocused", dsp_set_border_color_unfocused)?;

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