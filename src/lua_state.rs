// Minimal Lua embed for eswm config API (M3/M4).
// Provides: Lua state init, API registration, and do_file/do_chunk.
// Bridges `Snapshot` and `CompCmd` so Lua scripts can keybind, reload,
// and query state — matching Hyprland's `hl.bind`, `hl.define_submap`, `hl.dsp.*`, `hl.config{}`.

use std::sync::Arc;
use std::collections::HashMap;
use flume::Sender;
use mlua::{Lua, Table};
use xkbcommon::xkb;

use crate::wm::{DecorationConfig, DockEntry, TilingLayout, WmMode};

/// Display/input settings supplied by Lua config. Shared with the compositor
/// via Arc<Mutex<_>> so setters take effect without reloading the config file.
#[derive(Debug, Default, Clone)]
pub struct InputConfig {
    pub output_scales: HashMap<String, f64>,
    pub keymap: Option<String>,
    pub touchpad_tap: Option<bool>,
    pub touchpad_natural_scroll: Option<bool>,
    pub touchpad_speed: Option<f64>,
}

impl InputConfig {
    pub fn set_output_scale(&mut self, name: &str, value: f64) -> bool {
        if output_scale_for(value).is_none() {
            return false;
        }
        self.output_scales.insert(name.to_string(), value);
        true
    }

    pub fn output_scale(&self, name: &str) -> Option<smithay::output::Scale> {
        self.output_scales.get(name).copied().and_then(output_scale_for)
    }
}

/// Resolved touchpad settings. e08s03t2: defaults preserve the values that
/// used to be hardcoded in the libinput device-added callback.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TouchpadSettings {
    pub tap: bool,
    pub natural_scroll: bool,
    pub speed: f64,
}

pub fn resolve_touchpad(config: &InputConfig) -> TouchpadSettings {
    TouchpadSettings {
        tap: config.touchpad_tap.unwrap_or(true),
        natural_scroll: config.touchpad_natural_scroll.unwrap_or(false),
        speed: config.touchpad_speed.filter(|s| s.is_finite() && *s >= 0.0).unwrap_or(0.7),
    }
}

/// An empty or whitespace-only keymap would fail at compile time; reject it
/// early so the current keymap stays in effect (e08s03t2).
pub fn validate_keymap(keymap: &str) -> bool {
    !keymap.trim().is_empty()
}

/// Map a positive finite f64 to a smithay fractional Scale (e08s03t1).
fn output_scale_for(value: f64) -> Option<smithay::output::Scale> {
    if value.is_finite() && value > 0.0 {
        Some(smithay::output::Scale::Fractional(value))
    } else {
        None
    }
}

/// Validate `es.dsp.set_min_window_size` inputs (e10s01t7). Zero collapses the
/// resize clamps; anything past 4096 px cannot fit a real output and would make
/// output clamping pin every drag to a corner. Keep it boring and bounded.
fn validate_min_window_size(w: u32, h: u32) -> Result<(u32, u32), mlua::Error> {
    const MAX_MIN: u32 = 4096;
    if w == 0 || h == 0 {
        return Err(mlua::Error::RuntimeError(
            "min window size must be positive".into(),
        ));
    }
    if w > MAX_MIN || h > MAX_MIN {
        return Err(mlua::Error::RuntimeError(
            "min window size too large (max 4096)".into(),
        ));
    }
    Ok((w, h))
}

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
    #[allow(dead_code)]
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
    input_config: Arc<std::sync::Mutex<InputConfig>>,
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

    // e08s03t4: input/display settings. Invalid values raise a Lua error so
    // config mistakes surface early instead of silently poisoning the seat.
    {
        let cfg = input_config.clone();
        let setter = lua.create_function(move |_, (name, value): (String, f64)| -> Result<(), mlua::Error> {
            if !cfg.lock().unwrap().set_output_scale(&name, value) {
                return Err(mlua::Error::RuntimeError(format!("invalid scale for output {name}")));
            }
            Ok(())
        })?;
        dsp_table.set("set_output_scale", setter)?;
    }
    {
        let cfg = input_config.clone();
        let setter = lua.create_function(move |_, value: bool| -> Result<(), mlua::Error> {
            cfg.lock().unwrap().touchpad_tap = Some(value);
            Ok(())
        })?;
        dsp_table.set("set_touchpad_tap", setter)?;
    }
    {
        let cfg = input_config.clone();
        let setter = lua.create_function(move |_, value: bool| -> Result<(), mlua::Error> {
            cfg.lock().unwrap().touchpad_natural_scroll = Some(value);
            Ok(())
        })?;
        dsp_table.set("set_touchpad_natural_scroll", setter)?;
    }
    {
        let cfg = input_config.clone();
        let setter = lua.create_function(move |_, value: f64| -> Result<(), mlua::Error> {
            if !value.is_finite() || value < 0.0 {
                return Err(mlua::Error::RuntimeError("invalid touchpad speed".into()));
            }
            cfg.lock().unwrap().touchpad_speed = Some(value);
            Ok(())
        })?;
        dsp_table.set("set_touchpad_speed", setter)?;
    }
    {
        let setter = lua.create_function(move |_, keymap: String| -> Result<(), mlua::Error> {
            if !validate_keymap(&keymap) {
                return Err(mlua::Error::RuntimeError("empty keymap".into()));
            }
            input_config.lock().unwrap().keymap = Some(keymap);
            Ok(())
        })?;
        dsp_table.set("set_keymap", setter)?;
    }
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

    // es.dsp.set_min_window_size(w, h) -> compositor-side minimum window size
    // (e10s01t7). Stored in the shared DecorationConfig that drag clamps read.
    let dsp_set_min_window_size = lua.create_function({
        let decoration = decoration.clone();
        move |_, (w, h): (u32, u32)| -> Result<(), mlua::Error> {
            let (w, h) = validate_min_window_size(w, h)?;
            let mut dec = decoration.lock().unwrap();
            dec.min_window_width = w;
            dec.min_window_height = h;
            Ok(())
        }
    })?;
    dsp_table.set("set_min_window_size", dsp_set_min_window_size)?;

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

    // es.dsp.set_corner_radius(n) -> rounds the border ring and titlebar arcs;
    // 0 disables rounding, and a zero border width leaves no ring to round.
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

    // e09s02t3 — per-output overrides. Missing entries fall back to (0,0) and
    // the global dock height, so old config.lua files keep working unchanged.
    let dsp_set_output_position = {
        let decoration = decoration.clone();
        lua.create_function(
            move |_, (name, x, y): (String, i32, i32)| -> Result<(), mlua::Error> {
                if !decoration.lock().unwrap().set_output_position(&name, x, y) {
                    return Err(mlua::Error::RuntimeError("invalid output position".into()));
                }
                Ok(())
            },
        )?
    };
    dsp_table.set("set_output_position", dsp_set_output_position)?;

    let dsp_set_output_dock_height = {
        let decoration = decoration.clone();
        lua.create_function(move |_, (name, height): (String, u32)| -> Result<(), mlua::Error> {
            if !decoration.lock().unwrap().set_output_dock_height(&name, height) {
                return Err(mlua::Error::RuntimeError(
                    "output dock height must be 0..=256".into(),
                ));
            }
            Ok(())
        })?
    };
    dsp_table.set("set_output_dock_height", dsp_set_output_dock_height)?;

    // es.dsp.reset_tiling_splits() -> restore default Master/Equal ratios on
    // the primary output (e10s02t8). Routed through the command channel so the
    // same action serves Lua, keybinds, and the control socket.
    let cmd_reset_splits = cmd_tx.clone();
    let dsp_reset_tiling_splits = lua.create_function(move |_, _: ()| -> Result<(), mlua::Error> {
        let _ = cmd_reset_splits.send(crate::ipc::CompCmd::ResetTilingSplits);
        Ok(())
    })?;
    dsp_table.set("reset_tiling_splits", dsp_reset_tiling_splits)?;

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

    type TestApi = (Lua, Arc<Mutex<Vec<Keybind>>>, Arc<Mutex<InputConfig>>, Arc<Mutex<DecorationConfig>>, flume::Receiver<crate::ipc::CompCmd>);
    fn test_api() -> TestApi {
        let lua = init().expect("lua init");
        let snapshot = Arc::new(Mutex::new(crate::ipc::Snapshot::default()));
        let (cmd_tx, cmd_rx) = flume::unbounded();
        let keybinds = Arc::new(Mutex::new(Vec::new()));
        let decoration = Arc::new(Mutex::new(DecorationConfig::default()));
        let dock_entries = Arc::new(Mutex::new(Vec::new()));
        let input_config = Arc::new(Mutex::new(InputConfig::default()));
        register_api(
            &lua,
            snapshot,
            &cmd_tx,
            keybinds.clone(),
            decoration.clone(),
            dock_entries,
            input_config.clone(),
        )
        .expect("register api");
        (lua, keybinds, input_config, decoration, cmd_rx)
    }

    #[test]
    fn keybind_parse_stores_xkb_keysym_case_insensitive() {
        let (lua, keybinds, _input_config, _decoration, _cmd_rx) = test_api();
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

    // e10s01t7: es.dsp.set_min_window_size stores validated dimensions in the
    // shared DecorationConfig that the drag clamps read each motion event.
    // e10s02t8: es.dsp.reset_tiling_splits is a real command, not a Lua stub.
    #[test]
    fn reset_tiling_splits_lua_command_is_queued() {
        let (lua, _keybinds, _input_config, _decoration, cmd_rx) = test_api();
        lua.load("es.dsp.reset_tiling_splits()")
            .exec()
            .expect("reset tiling splits");
        assert!(matches!(cmd_rx.try_recv(), Ok(crate::ipc::CompCmd::ResetTilingSplits)));
    }

    #[test]
    fn min_window_size_setter_validates_and_stores() {
        let (lua, _keybinds, _input_config, decoration, _cmd_rx) = test_api();
        lua.load("es.dsp.set_min_window_size(240, 180)")
            .exec()
            .expect("set min window size");
        assert_eq!(decoration.lock().unwrap().min_window_width, 240);
        assert_eq!(decoration.lock().unwrap().min_window_height, 180);

        // Zero or absurd sizes would collapse resize math / output clamping.
        assert!(validate_min_window_size(0, 150).is_err());
        assert!(validate_min_window_size(200, 0).is_err());
        assert!(validate_min_window_size(5000, 150).is_err());
        assert!(validate_min_window_size(200, 150).is_ok());
    }

    // e08s03t1 — per-output scale mapping: invalid values must never reach
    // Output::change_current_state (Scale::Fractional(NaN) would poison layout).
    #[test]
    fn output_scale_maps_positive_float_to_fractional() {
        let scale = output_scale_for(2.0).expect("valid scale");
        assert_eq!(scale.fractional_scale(), 2.0);
    }

    #[test]
    fn output_scale_rejects_non_positive_and_non_finite() {
        assert!(output_scale_for(0.0).is_none());
        assert!(output_scale_for(-1.0).is_none());
        assert!(output_scale_for(f64::NAN).is_none());
        assert!(output_scale_for(f64::INFINITY).is_none());
    }

    // e08s03t2 — touchpad settings fall back to the current hardcoded values
    // when config leaves them unset; keymap rejects empty input.
    #[test]
    fn touchpad_config_defaults_match_previous_hardcoded_values() {
        let resolved = resolve_touchpad(&InputConfig::default());
        assert!(resolved.tap);
        assert!(!resolved.natural_scroll);
        assert!((resolved.speed - 0.7).abs() < f64::EPSILON);
    }

    #[test]
    fn touchpad_config_overrides_are_honored() {
        let config = InputConfig {
            touchpad_tap: Some(false),
            touchpad_natural_scroll: Some(true),
            touchpad_speed: Some(1.2),
            ..Default::default()
        };
        let resolved = resolve_touchpad(&config);
        assert!(!resolved.tap);
        assert!(resolved.natural_scroll);
        assert!((resolved.speed - 1.2).abs() < f64::EPSILON);
    }

    #[test]
    fn touchpad_config_keymap_validation_rejects_empty_accepts_text() {
        assert!(!validate_keymap(""));
        assert!(!validate_keymap("   \n "));
        assert!(validate_keymap("xkb_keymap { };"));
    }

    #[test]
    fn output_scale_config_stores_and_looks_up_by_name() {
        let mut config = InputConfig::default();
        assert!(config.set_output_scale("eDP-1", 2.0));
        assert!(!config.set_output_scale("eDP-1", 0.0), "invalid scale rejected");
        assert_eq!(
            config.output_scale("eDP-1").map(|s| s.fractional_scale()),
            Some(2.0)
        );
        assert!(config.output_scale("HDMI-A-1").is_none(), "unknown output");
    }

    // e08s03t4 — es.dsp setters write into the shared config consumed by the
    // compositor; invalid keymap/scales must not poison the config.
    #[test]
    fn keymap_config_setters_store_values() {
        let (lua, _keybinds, input_config, _decoration, _cmd_rx) = test_api();
        lua.load(
            r#"
            es.dsp.set_output_scale("eDP-1", 2.0)
            es.dsp.set_touchpad_tap(false)
            es.dsp.set_touchpad_natural_scroll(true)
            es.dsp.set_touchpad_speed(1.2)
            es.dsp.set_keymap("xkb_keymap { };")
            "#,
        )
        .exec()
        .expect("setters");
        let cfg = input_config.lock().unwrap();
        assert_eq!(cfg.output_scale("eDP-1").map(|s| s.fractional_scale()), Some(2.0));
        assert_eq!(cfg.touchpad_tap, Some(false));
        assert_eq!(cfg.touchpad_natural_scroll, Some(true));
        assert_eq!(cfg.touchpad_speed, Some(1.2));
        assert_eq!(cfg.keymap.as_deref(), Some("xkb_keymap { };"));
    }

    #[test]
    fn keymap_config_rejects_invalid_values() {
        let (lua, _keybinds, input_config, _decoration, _cmd_rx) = test_api();
        assert!(lua.load(r#"es.dsp.set_keymap("")"#).exec().is_err());
        assert!(lua.load(r#"es.dsp.set_output_scale("eDP-1", 0)"#).exec().is_err());
        assert!(input_config.lock().unwrap().output_scales.is_empty());
        assert!(input_config.lock().unwrap().keymap.is_none());
    }

    #[test]
    fn keybind_parse_unknown_key_returns_lua_error() {
        let (lua, keybinds, _input_config, _decoration, _cmd_rx) = test_api();
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

    // e09s02t3 — output geometry is config, not a hardcoded (0,0) for every
    // connector. Bad values must be rejected before they reach the DRM backend.
    #[test]
    fn output_config_per_output_position_and_dock_height() {
        let mut cfg = DecorationConfig::standard();
        assert_eq!(cfg.output_position("HDMI-A-1"), None);
        assert!(cfg.set_output_position("HDMI-A-1", 1920, 0));
        assert_eq!(cfg.output_position("HDMI-A-1"), Some((1920, 0)));
        assert!(!cfg.set_output_position("HDMI-A-1", 1920, 0), "no-op is not a change");
        assert!(!cfg.set_output_position("", 0, 0), "nameless output is rejected");
        assert!(!cfg.set_output_position("HDMI-A-1", 1 << 20, 0), "absurd coord");

        assert_eq!(cfg.dock_height_for("HDMI-A-1"), 0);
        assert!(cfg.set_output_dock_height("HDMI-A-1", 48));
        assert_eq!(cfg.dock_height_for("HDMI-A-1"), 48);
        assert_eq!(cfg.dock_height_for("eDP-1"), 0);
        cfg.dock_height = 24;
        assert_eq!(cfg.dock_height_for("eDP-1"), 24, "global setter is the default");
        assert_eq!(cfg.dock_height_for("HDMI-A-1"), 48, "per-output wins");
        assert!(!cfg.set_output_dock_height("eDP-1", 257), "dock height capped");
    }

    #[test]
    fn output_config_lua_setters_validate_and_store() {
        let (lua, _keybinds, _input_config, decoration, _cmd_rx) = test_api();
        lua.load(
            r#"
            es.dsp.set_output_position("HDMI-A-1", 1920, 0)
            es.dsp.set_output_dock_height("HDMI-A-1", 40)
            es.dsp.set_dock_height(24)
            "#,
        )
        .exec()
        .expect("per-output setters");
        {
            let cfg = decoration.lock().unwrap();
            assert_eq!(cfg.output_position("HDMI-A-1"), Some((1920, 0)));
            assert_eq!(cfg.dock_height_for("HDMI-A-1"), 40);
            assert_eq!(cfg.dock_height_for("eDP-1"), 24);
        }

        assert!(lua.load(r#"es.dsp.set_output_dock_height("HDMI-A-1", 999)"#).exec().is_err());
        assert!(lua.load(r#"es.dsp.set_output_position("HDMI-A-1", 1e20, 0)"#).exec().is_err());
        let cfg = decoration.lock().unwrap();
        assert_eq!(cfg.output_position("HDMI-A-1"), Some((1920, 0)), "invalid Lua left config untouched");
        assert_eq!(cfg.dock_height_for("HDMI-A-1"), 40, "invalid Lua left dock untouched");
    }
}
