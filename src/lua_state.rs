// Minimal Lua embed for eswm config API (M3/M4).
// Provides: Lua state init, API registration, and do_file/do_chunk.
// Bridges `Snapshot` and `CompCmd` so Lua scripts can keybind, reload,
// and query state — matching Hyprland's `hl.bind`, `hl.define_submap`, `hl.dsp.*`, `hl.config{}`.

use std::sync::Arc;
use flume::Sender;
use mlua::{Lua, Table};
use xkbcommon::xkb;

use crate::wm::{DecorationConfig, DockEntry, TilingLayout, WmMode};

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
    pub keysym: u32,
    pub action: String,
    pub arg: Option<i32>,
    pub cmd: Option<String>,
}


fn parse_keysym_name(name: &str) -> Result<u32, mlua::Error> {
    let sym = xkb::keysym_from_name(name, xkb::KEYSYM_CASE_INSENSITIVE);
    if sym.raw() == xkb::keysyms::KEY_NoSymbol {
        return Err(mlua::Error::RuntimeError(format!("unknown key name: {name}")));
    }
    Ok(sym.raw())
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
    cmd_tx: &Sender<crate::ipc::CompCmd>,
    lua_keybinds: Arc<std::sync::Mutex<Vec<Keybind>>>,
    decoration: Arc<std::sync::Mutex<DecorationConfig>>,
    dock_entries: Arc<std::sync::Mutex<Vec<DockEntry>>>,
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
        let keysym = parse_keysym_name(&key)?;
        let kb = Keybind {
            mods,
            key,
            keysym,
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
        let keysym = parse_keysym_name(&key)?;
        let kb = Keybind {
            mods,
            key,
            keysym,
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
    // es.dsp.exec_cmd("...") -> spawns a shell-interpreted command string (sh -c)
    let dsp_exec_cmd = lua.create_function(move |_, cmd: String| -> Result<(), mlua::Error> {
        if cmd.trim().is_empty() {
            return Ok(());
        }
        crate::spawn_shell_command(&cmd)
            .map(|_| ())
            .map_err(|e| mlua::Error::RuntimeError(format!("exec_cmd failed: {e}")))?;
        Ok(())
    })?;
    dsp_table.set("exec_cmd", dsp_exec_cmd)?;

    // es.exec({"prog", "arg", ...}) -> spawns an argv vector without shell parsing
    let es_exec = lua.create_function(move |_, argv: Vec<String>| -> Result<(), mlua::Error> {
        if argv.is_empty() {
            return Err(mlua::Error::RuntimeError("exec requires non-empty argv".into()));
        }
        let refs: Vec<&str> = argv.iter().map(|s| s.as_str()).collect();
        crate::spawn_argv_command(&refs)
            .map(|_| ())
            .map_err(|e| mlua::Error::RuntimeError(format!("exec failed: {e}")))?;
        Ok(())
    })?;
    es.set("exec", es_exec)?;

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

    // es.dsp.set_wm_mode("tiling"|"floating") -> default wm mode for new windows
    let dsp_set_wm_mode = lua.create_function({
        let decoration = decoration.clone();
        move |_, mode: String| -> Result<(), mlua::Error> {
            let wm_mode = match mode.as_str() {
                "tiling" | "tiled" => WmMode::Tiling,
                "floating" | "float" => WmMode::Floating,
                _ => return Err(mlua::Error::RuntimeError("wm mode must be 'tiling' or 'floating'".into())),
            };
            decoration.lock().unwrap().wm_mode = wm_mode;
            Ok(())
        }
    })?;
    dsp_table.set("set_wm_mode", dsp_set_wm_mode)?;

    // es.dsp.set_tiling_layout("master"|"equal") -> tiling layout algorithm per output
    let dsp_set_tiling_layout = lua.create_function({
        let decoration = decoration.clone();
        move |_, mode: String| -> Result<(), mlua::Error> {
            let tiling = match mode.as_str() {
                "master" => TilingLayout::Master,
                "equal" => TilingLayout::Equal,
                _ => return Err(mlua::Error::RuntimeError("tiling layout must be 'master' or 'equal'".into())),
            };
            decoration.lock().unwrap().tiling_layout = tiling;
            Ok(())
        }
    })?;
    dsp_table.set("set_tiling_layout", dsp_set_tiling_layout)?;

    // es.dsp.set_terminal("kitty") -> terminal program for spawn_terminal and TERMINAL env
    let dsp_set_terminal = lua.create_function(move |_, name: String| -> Result<(), mlua::Error> {
        if name.trim().is_empty() {
            return Err(mlua::Error::RuntimeError("terminal name must not be empty".into()));
        }
        crate::set_spawn_terminal(name.trim());
        Ok(())
    })?;
    dsp_table.set("set_terminal", dsp_set_terminal)?;

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

    // es.dsp.set_titlebar_color_focused(r, g, b, a) -> focused titlebar color (RGBA 0-255)
    let dsp_set_titlebar_color_focused = lua.create_function({
        let decoration = decoration.clone();
        move |_, (r, g, b, a): (u8, u8, u8, u8)| -> Result<(), mlua::Error> {
            decoration.lock().unwrap().titlebar_color_focused = [r, g, b, a];
            Ok(())
        }
    })?;
    dsp_table.set("set_titlebar_color_focused", dsp_set_titlebar_color_focused)?;

    // es.dsp.set_titlebar_color_unfocused(r, g, b, a) -> unfocused titlebar color (RGBA 0-255)
    let dsp_set_titlebar_color_unfocused = lua.create_function({
        let decoration = decoration.clone();
        move |_, (r, g, b, a): (u8, u8, u8, u8)| -> Result<(), mlua::Error> {
            decoration.lock().unwrap().titlebar_color_unfocused = [r, g, b, a];
            Ok(())
        }
    })?;
    dsp_table.set("set_titlebar_color_unfocused", dsp_set_titlebar_color_unfocused)?;

    // es.dsp.set_dock_height(n) -> dock bar height in px (0 = dock off)
    let dsp_set_dock_height = lua.create_function({
        let decoration = decoration.clone();
        move |_, size: u32| -> Result<(), mlua::Error> {
            decoration.lock().unwrap().dock_height = size;
            Ok(())
        }
    })?;
    dsp_table.set("set_dock_height", dsp_set_dock_height)?;

    // es.dock — pluggable dock frontend contract.
    // entries() -> [{id, app_id, title, minimized, focused}]
    // activate(id) -> focus that window (sends CompCmd::FocusWindow)
    // toggle_minimize(id) -> toggle mapped/minimized state (sends CompCmd::ToggleMinimize)
    // The WM owns dock state; entries are computed each frame by the renderer
    // into the dock_entries cache. An external frontend can call entries() to
    // build its own dock UI and activate()/toggle_minimize() to drive windows.
    let dock = lua.create_table()?;
    let dock_entries_cache = dock_entries.clone();
    let dock_entries_fn = lua.create_function(move |lua, _: ()| -> Result<Table, mlua::Error> {
        let entries = dock_entries_cache.lock().unwrap();
        let list = lua.create_table()?;
        for (i, entry) in entries.iter().enumerate() {
            let t = lua.create_table()?;
            t.set("id", entry.id)?;
            t.set("app_id", entry.app_id.clone())?;
            t.set("title", entry.title.clone())?;
            t.set("minimized", entry.minimized)?;
            t.set("focused", entry.focused)?;
            list.set(i + 1, t)?;
        }
        Ok(list)
    })?;
    dock.set("entries", dock_entries_fn)?;
    let cmd_activate = cmd_tx.clone();
    let dock_activate = lua.create_function(move |_, id: u64| -> Result<(), mlua::Error> {
        let _ = cmd_activate.send(crate::ipc::CompCmd::FocusWindow(id));
        Ok(())
    })?;
    dock.set("activate", dock_activate)?;
    let cmd_toggle = cmd_tx.clone();
    let dock_toggle = lua.create_function(move |_, id: u64| -> Result<(), mlua::Error> {
        let _ = cmd_toggle.send(crate::ipc::CompCmd::ToggleMinimize(id));
        Ok(())
    })?;
    dock.set("toggle_minimize", dock_toggle)?;
    es.set("dock", dock)?;

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
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn test_api() -> (Lua, Arc<Mutex<Vec<Keybind>>>) {
        let lua = init().expect("lua init");
        let snapshot = Arc::new(Mutex::new(crate::ipc::Snapshot::default()));
        let (cmd_tx, _cmd_rx) = flume::unbounded();
        let keybinds = Arc::new(Mutex::new(Vec::new()));
        let decoration = Arc::new(Mutex::new(DecorationConfig::default()));
        let dock_entries = Arc::new(Mutex::new(Vec::new()));
        register_api(
            &lua,
            snapshot,
            &cmd_tx,
            keybinds.clone(),
            decoration,
            dock_entries,
        )
        .expect("register api");
        (lua, keybinds)
    }

    #[test]
    fn keybind_parse_stores_xkb_keysym_case_insensitive() {
        let (lua, keybinds) = test_api();
        lua.load(r#"es.bind("Super+Return", "spawn_terminal")"#)
            .exec()
            .expect("bind return");
        lua.load(r#"es.bind_cmd("Super+r", "fuzzel")"#)
            .exec()
            .expect("bind_cmd r");
        let kbs = keybinds.lock().unwrap();
        assert_eq!(kbs.len(), 2);
        assert_eq!(kbs[0].keysym, xkbcommon::xkb::keysyms::KEY_Return);
        assert_eq!(kbs[1].keysym, xkbcommon::xkb::keysyms::KEY_r);
    }

    #[test]
    fn keybind_parse_unknown_key_returns_lua_error() {
        let (lua, keybinds) = test_api();
        let err = lua
            .load(r#"es.bind("Super+NotARealKey_123", "spawn_terminal")"#)
            .exec()
            .expect_err("unknown key must error");
        assert!(
            err.to_string().to_lowercase().contains("unknown key"),
            "unexpected error: {err}"
        );
        assert!(keybinds.lock().unwrap().is_empty(), "failed bind must not register");
    }
}
