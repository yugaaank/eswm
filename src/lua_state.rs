// Minimal Lua embed for eswm config API (M3/M4).
// Provides: Lua state init, API registration, and do_file/do_chunk.
// Bridges `Snapshot` and `CompCmd` so Lua scripts can keybind, reload,
// and query state — matching Hyprland's `hl.bind`, `hl.define_submap`, `hl.dsp.*`, `hl.config{}`.

use std::sync::{Arc, Mutex};
use std::collections::HashMap;
use flume::Sender;
use mlua::{Function, Lua, RegistryKey, Table};
use serde_json::Value;
use xkbcommon::xkb;

use crate::control::{Event, EventKind};
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

/// A fully validated `es.config{...}` block. Nothing is applied until every
/// key has been accepted, so one bad key cannot leave a half-applied config —
/// nobody wants a compositor wearing only one border and a monocle.
#[derive(Debug, Default)]
struct ConfigValues {
    wm_mode: Option<WmMode>,
    tiling_layout: Option<TilingLayout>,
    border_size: Option<u32>,
    outer_gap: Option<u32>,
    inner_gap: Option<u32>,
    titlebar_height: Option<u32>,
    dock_height: Option<u32>,
    corner_radius: Option<u32>,
    min_size: Option<(u32, u32)>,
    border_color_focused: Option<[u8; 4]>,
    border_color_unfocused: Option<[u8; 4]>,
    titlebar_color_focused: Option<[u8; 4]>,
    titlebar_color_unfocused: Option<[u8; 4]>,
    output_scales: Vec<(String, f64)>,
    output_positions: Vec<(String, i32, i32)>,
    touchpad_tap: Option<bool>,
    touchpad_natural_scroll: Option<bool>,
    touchpad_speed: Option<f64>,
    keymap: Option<String>,
}

const CONFIG_KEYS: &[&str] = &[
    "wm_mode", "tiling_layout", "general", "border_size", "outer_gap", "inner_gap",
    "titlebar_height", "dock_height", "corner_radius", "min_window_size",
    "border_color_focused", "border_color_unfocused", "titlebar_color_focused",
    "titlebar_color_unfocused", "output_scale", "output_position", "touchpad", "keymap",
];

fn config_err(key: &str, expected: &str, value: &mlua::Value) -> mlua::Error {
    mlua::Error::RuntimeError(format!(
        "config key '{key}': expected {expected}, got {}",
        value.type_name()
    ))
}

fn check_config_keys(table: &Table, allowed: &[&str], scope: &str) -> Result<(), mlua::Error> {
    for entry in table.clone().pairs::<mlua::Value, mlua::Value>() {
        let (key, _) = entry?;
        let mlua::Value::String(key) = key else {
            return Err(mlua::Error::RuntimeError(format!(
                "config key in '{scope}' must be a string"
            )));
        };
        let name = key.to_str()?.to_string();
        if !allowed.contains(&name.as_str()) {
            return Err(mlua::Error::RuntimeError(format!(
                "unknown config key '{scope}{name}'"
            )));
        }
    }
    Ok(())
}

fn config_field(table: &Table, key: &str) -> Result<mlua::Value, mlua::Error> {
    table.get::<mlua::Value>(key)
}

fn config_uint(table: &Table, key: &str) -> Result<Option<u32>, mlua::Error> {
    match config_field(table, key)? {
        mlua::Value::Nil => Ok(None),
        mlua::Value::Integer(n) if (0..=u32::MAX as i64).contains(&n) => Ok(Some(n as u32)),
        value => Err(config_err(key, "non-negative integer", &value)),
    }
}

fn config_bool(table: &Table, key: &str) -> Result<Option<bool>, mlua::Error> {
    match config_field(table, key)? {
        mlua::Value::Nil => Ok(None),
        mlua::Value::Boolean(v) => Ok(Some(v)),
        value => Err(config_err(key, "boolean", &value)),
    }
}

fn config_string(table: &Table, key: &str) -> Result<Option<String>, mlua::Error> {
    match config_field(table, key)? {
        mlua::Value::Nil => Ok(None),
        mlua::Value::String(s) => Ok(Some(s.to_str()?.to_string())),
        value => Err(config_err(key, "string", &value)),
    }
}

fn config_number(table: &Table, key: &str) -> Result<Option<f64>, mlua::Error> {
    match config_field(table, key)? {
        mlua::Value::Nil => Ok(None),
        mlua::Value::Integer(n) => Ok(Some(n as f64)),
        mlua::Value::Number(n) => Ok(Some(n)),
        value => Err(config_err(key, "number", &value)),
    }
}

fn config_color(table: &Table, key: &str) -> Result<Option<[u8; 4]>, mlua::Error> {
    let value = config_field(table, key)?;
    let mlua::Value::Table(colors) = &value else {
        if matches!(value, mlua::Value::Nil) {
            return Ok(None);
        }
        return Err(config_err(key, "array of 4 RGBA bytes", &value));
    };
    let mut rgba = [0u8; 4];
    for (index, byte) in rgba.iter_mut().enumerate() {
        let value = colors.get::<mlua::Value>(index + 1)?;
        let mlua::Value::Integer(n) = value else {
            return Err(config_err(key, "array of 4 RGBA bytes", &value));
        };
        if !(0..=255).contains(&n) {
            return Err(config_err(key, "RGBA byte in 0..=255", &value));
        }
        *byte = n as u8;
    }
    Ok(Some(rgba))
}

fn config_table_field(table: &Table, key: &str) -> Result<Option<Table>, mlua::Error> {
    match config_field(table, key)? {
        mlua::Value::Nil => Ok(None),
        mlua::Value::Table(nested) => Ok(Some(nested)),
        value => Err(config_err(key, "table", &value)),
    }
}

fn config_enum(table: &Table, key: &str, variants: &[&str]) -> Result<Option<String>, mlua::Error> {
    let Some(value) = config_string(table, key)? else {
        return Ok(None);
    };
    if !variants.contains(&value.as_str()) {
        return Err(mlua::Error::RuntimeError(format!(
            "config key '{key}' must be one of {}",
            variants.join(" | ")
        )));
    }
    Ok(Some(value))
}

/// Validate the whole declarative block before returning anything applyable.
#[allow(clippy::field_reassign_with_default)] // staged validation: each field is checked in order
fn validate_config_table(lua: &Lua, table: &Table) -> Result<ConfigValues, mlua::Error> {
    let _ = lua;
    check_config_keys(table, CONFIG_KEYS, "")?;
    let mut values = ConfigValues::default();

    values.wm_mode = config_enum(table, "wm_mode", &["tiling", "floating"])?.map(|mode| {
        if mode == "floating" { WmMode::Floating } else { WmMode::Tiling }
    });
    values.tiling_layout = config_enum(table, "tiling_layout", &["master", "equal"])?.map(|mode| {
        if mode == "equal" { TilingLayout::Equal } else { TilingLayout::Master }
    });
    values.border_size = config_uint(table, "border_size")?;
    values.outer_gap = config_uint(table, "outer_gap")?;
    values.inner_gap = config_uint(table, "inner_gap")?;
    values.titlebar_height = config_uint(table, "titlebar_height")?;
    values.dock_height = config_uint(table, "dock_height")?;
    values.corner_radius = config_uint(table, "corner_radius")?;
    values.border_color_focused = config_color(table, "border_color_focused")?;
    values.border_color_unfocused = config_color(table, "border_color_unfocused")?;
    values.titlebar_color_focused = config_color(table, "titlebar_color_focused")?;
    values.titlebar_color_unfocused = config_color(table, "titlebar_color_unfocused")?;
    values.keymap = config_string(table, "keymap")?;
    if let Some(keymap) = &values.keymap
        && !validate_keymap(keymap) {
            return Err(mlua::Error::RuntimeError("config key 'keymap': empty keymap".into()));
        }

    if let Some(general) = config_table_field(table, "general")? {
        check_config_keys(&general, &["layout", "mode"], "general.")?;
        if let Some(layout) = config_enum(&general, "layout", &["master", "equal"])? {
            values.tiling_layout = Some(if layout == "equal" { TilingLayout::Equal } else { TilingLayout::Master });
        }
        if let Some(mode) = config_enum(&general, "mode", &["tiling", "floating"])? {
            values.wm_mode = Some(if mode == "floating" { WmMode::Floating } else { WmMode::Tiling });
        }
    }

    if let Some(min_size) = config_table_field(table, "min_window_size")? {
        check_config_keys(&min_size, &["width", "height"], "min_window_size.")?;
        let width = config_uint(&min_size, "width")?.unwrap_or(0);
        let height = config_uint(&min_size, "height")?.unwrap_or(0);
        values.min_size = Some(validate_min_window_size(width, height)?);
    }

    if let Some(touchpad) = config_table_field(table, "touchpad")? {
        check_config_keys(&touchpad, &["tap", "natural_scroll", "speed"], "touchpad.")?;
        values.touchpad_tap = config_bool(&touchpad, "tap")?;
        values.touchpad_natural_scroll = config_bool(&touchpad, "natural_scroll")?;
        values.touchpad_speed = config_number(&touchpad, "speed")?;
        if let Some(speed) = values.touchpad_speed
            && (!speed.is_finite() || speed < 0.0) {
                return Err(config_err("touchpad.speed", "number >= 0", &mlua::Value::Number(speed)));
            }
    }

    // Reuse the existing strict setters as validators: a bad entry must fail
    // here, before any value is written to live state.
    let mut probe_decoration = DecorationConfig::standard();
    if let Some(scales) = config_table_field(table, "output_scale")? {
        for entry in scales.clone().pairs::<mlua::Value, mlua::Value>() {
            let (key, value) = entry?;
            let mlua::Value::String(key) = key else {
                return Err(mlua::Error::RuntimeError("config key 'output_scale': output name must be a string".into()));
            };
            let name = key.to_str()?.to_string();
            let scale = match value {
                mlua::Value::Integer(n) => n as f64,
                mlua::Value::Number(n) => n,
                _ => return Err(config_err("output_scale", "number", &value)),
            };
            let mut probe_input = InputConfig::default();
            if !probe_input.set_output_scale(&name, scale) {
                return Err(mlua::Error::RuntimeError(format!(
                    "config key 'output_scale.{name}': positive finite scale required"
                )));
            }
            values.output_scales.push((name, scale));
        }
    }
    if let Some(positions) = config_table_field(table, "output_position")? {
        for entry in positions.clone().pairs::<mlua::Value, mlua::Value>() {
            let (key, value) = entry?;
            let mlua::Value::String(key) = key else {
                return Err(mlua::Error::RuntimeError("config key 'output_position': output name must be a string".into()));
            };
            let name = key.to_str()?.to_string();
            let mlua::Value::Table(pos) = value else {
                return Err(config_err("output_position", "table with x and y", &value));
            };
            check_config_keys(&pos, &["x", "y"], "output_position.")?;
            let x = config_uint(&pos, "x")?.unwrap_or(0) as i32;
            let y = config_uint(&pos, "y")?.unwrap_or(0) as i32;
            if !probe_decoration.set_output_position(&name, x, y) {
                return Err(mlua::Error::RuntimeError(format!(
                    "config key 'output_position.{name}': invalid output position"
                )));
            }
            values.output_positions.push((name, x, y));
        }
    }

    Ok(values)
}

/// Apply a validated block. Validation has already rejected bad values, so
/// this is a straight write into the single-threaded shared config.
fn apply_config_values(
    values: ConfigValues,
    decoration: &std::sync::Mutex<DecorationConfig>,
    input: &std::sync::Mutex<InputConfig>,
) -> Result<(), mlua::Error> {
    {
        let mut dec = decoration.lock().unwrap();
        if let Some(v) = values.wm_mode { dec.wm_mode = v; }
        if let Some(v) = values.tiling_layout { dec.tiling_layout = v; }
        if let Some(v) = values.border_size { dec.border_size = v; }
        if let Some(v) = values.outer_gap { dec.outer = v; }
        if let Some(v) = values.inner_gap { dec.inner = v; }
        if let Some(v) = values.titlebar_height { dec.titlebar_height = v; }
        if let Some(v) = values.dock_height { dec.dock_height = v; }
        if let Some(v) = values.corner_radius { dec.corner_radius = v; }
        if let Some((w, h)) = values.min_size {
            dec.min_window_width = w;
            dec.min_window_height = h;
        }
        if let Some(v) = values.border_color_focused { dec.border_color_focused = v; }
        if let Some(v) = values.border_color_unfocused { dec.border_color_unfocused = v; }
        if let Some(v) = values.titlebar_color_focused { dec.titlebar_color_focused = v; }
        if let Some(v) = values.titlebar_color_unfocused { dec.titlebar_color_unfocused = v; }
        for (name, x, y) in values.output_positions {
            let _ = dec.set_output_position(&name, x, y);
        }
    }
    let mut cfg = input.lock().unwrap();
    for (name, scale) in values.output_scales {
        let _ = cfg.set_output_scale(&name, scale);
    }
    if let Some(v) = values.touchpad_tap { cfg.touchpad_tap = Some(v); }
    if let Some(v) = values.touchpad_natural_scroll { cfg.touchpad_natural_scroll = Some(v); }
    if let Some(v) = values.touchpad_speed { cfg.touchpad_speed = Some(v); }
    if let Some(v) = values.keymap { cfg.keymap = Some(v); }
    Ok(())
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
    /// Lua callback for `es.bind(keys, function, arg)`. Arc keeps Keybind
    /// cloneable while RegistryKey deliberately is not.
    pub callback: Option<Arc<RegistryKey>>,
}

/// Run a Lua-bound keybind callback. Errors travel back to the caller so the
/// key path can log them instead of silently pretending the key worked.
pub(crate) fn call_keybind_callback(lua: &Lua, kb: &Keybind) -> Result<(), mlua::Error> {
    let Some(key) = &kb.callback else {
        return Ok(());
    };
    let function: Function = lua.registry_value(key)?;
    match kb.arg {
        Some(arg) => function.call::<()>(arg),
        None => function.call::<()>(()),
    }
}

/// One `es.on(kind, fn)` subscriber. `Arc` lets the hook vector be cloned out
/// of its mutex before callbacks run — re-entrant setters would otherwise find
/// the mutex acting like a bouncer who refuses to let anyone leave the party.
#[derive(Clone)]
pub struct EventHook {
    kind: EventKind,
    callback: Arc<RegistryKey>,
}

/// Shared hook registry. State owns one instance; `es.on` pushes into it.
pub type EventHooks = Arc<Mutex<Vec<EventHook>>>;

/// Convert one JSON value into a Lua value. Event data is JSON at the socket
/// boundary and a table in Lua; arrays keep 1-based Lua indices.
fn json_to_lua(lua: &Lua, value: &Value) -> Result<mlua::Value, mlua::Error> {
    match value {
        Value::Null => Ok(mlua::Value::Nil),
        Value::Bool(v) => Ok(mlua::Value::Boolean(*v)),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(mlua::Value::Integer(i))
            } else if let Some(f) = n.as_f64() {
                Ok(mlua::Value::Number(f))
            } else {
                Err(mlua::Error::RuntimeError("invalid event number".into()))
            }
        }
        Value::String(s) => Ok(mlua::Value::String(lua.create_string(s)?)),
        Value::Array(items) => {
            let table = lua.create_table()?;
            for (index, item) in items.iter().enumerate() {
                table.set(index + 1, json_to_lua(lua, item)?)?;
            }
            Ok(mlua::Value::Table(table))
        }
        Value::Object(map) => {
            let table = lua.create_table()?;
            for (key, item) in map {
                table.set(key.as_str(), json_to_lua(lua, item)?)?;
            }
            Ok(mlua::Value::Table(table))
        }
    }
}

/// Fire hooks for one flushed batch. The hook list is copied before any Lua
/// runs, and each error is returned so the caller can log it without stopping
/// the remaining subscribers.
pub(crate) fn fire_event_hooks(lua: &Lua, hooks: &EventHooks, events: &[Event]) -> Vec<String> {
    let snapshot = hooks.lock().unwrap().clone();
    let mut errors = Vec::new();
    for event in events {
        for hook in &snapshot {
            if hook.kind != event.kind {
                continue;
            }
            let result = (|| -> Result<(), mlua::Error> {
                let function: Function = lua.registry_value(&hook.callback)?;
                let data = json_to_lua(lua, &event.data)?;
                function.call::<()>(data)
            })();
            if let Err(err) = result {
                errors.push(format!("{}: {err}", event.kind.as_str()));
            }
        }
    }
    errors
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
// Eight upvalues is two hands short of a good time; grouping them is a later
// refactor, not a reason to fail the gate today.
#[allow(clippy::too_many_arguments)]
pub fn register_api(
    lua: &Lua,
    _snapshot: Arc<std::sync::Mutex<crate::ipc::Snapshot>>,
    cmd_tx: &Sender<crate::ipc::CompCmd>,
    lua_keybinds: Arc<std::sync::Mutex<Vec<Keybind>>>,
    lua_hooks: EventHooks,
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
    let bind = lua.create_function(move |lua, (keybind, action, arg): (String, mlua::Value, Option<i32>)| -> Result<(), mlua::Error> {
        let (mods, key) = parse_keybind_string(&keybind);
        if key.is_empty() {
            return Err(mlua::Error::RuntimeError("keybind missing key".into()));
        }
        let keysym = parse_keysym_name(&key)?;
        // A function action is kept in the registry; string actions keep the
        // old closed-list behaviour byte-for-byte.
        let (action, callback) = match action {
            mlua::Value::String(name) => (name.to_str()?.to_string(), None),
            mlua::Value::Function(function) => (
                "lua_function".to_string(),
                Some(Arc::new(lua.create_registry_value(function)?)),
            ),
            other => {
                return Err(mlua::Error::RuntimeError(format!(
                    "bind action must be a string or function, got {}",
                    other.type_name()
                )));
            }
        };
        let kb = Keybind {
            mods,
            key,
            keysym,
            action,
            arg,
            cmd: None,
            callback,
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
            callback: None,
        };
        lua_keybinds_cmd.lock().unwrap().push(kb);
        Ok(())
    })?;
    es.set("bind_cmd", bind_cmd)?;

    // es.on(event_kind, fn) -> registers a hook for one flat e09s03 event.
    // Unknown names fail here, at config load, instead of at first event.
    let on = lua.create_function(move |lua, (kind, callback): (String, Function)| -> Result<(), mlua::Error> {
        let kind = EventKind::from_name(&kind).ok_or_else(|| {
            mlua::Error::RuntimeError(format!("unknown event kind: {kind}"))
        })?;
        let key = Arc::new(lua.create_registry_value(callback)?);
        lua_hooks.lock().unwrap().push(EventHook { kind, callback: key });
        Ok(())
    })?;
    es.set("on", on)?;

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
        let keymap_config = input_config.clone();
        let setter = lua.create_function(move |_, keymap: String| -> Result<(), mlua::Error> {
            if !validate_keymap(&keymap) {
                return Err(mlua::Error::RuntimeError("empty keymap".into()));
            }
            keymap_config.lock().unwrap().keymap = Some(keymap);
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
    // `es.config { ... }` calls the table through __call. Validation happens
    // on a temporary copy so a rejected key leaves live state untouched.
    let config_call_meta = lua.create_table()?;
    let call_decoration = decoration.clone();
    let call_input = input_config.clone();
    config_call_meta.set(
        "__call",
        lua.create_function(
            move |lua, (_, block): (Table, Table)| -> Result<(), mlua::Error> {
                let values = validate_config_table(lua, &block)?;
                apply_config_values(values, &call_decoration, &call_input)
            },
        )?,
    )?;
    config.set_metatable(Some(config_call_meta))?;
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
    use crate::control::{Event, EventKind};
    use crate::wm::{ClientMode, Rect, WindowManager};
    use serde_json::json;
    use std::sync::{Arc, Mutex};

    type TestApi = (Lua, Arc<Mutex<Vec<Keybind>>>, Arc<Mutex<InputConfig>>, Arc<Mutex<DecorationConfig>>, flume::Receiver<crate::ipc::CompCmd>);
    fn test_api() -> TestApi {
        test_api_with_extras().0
    }

    fn test_api_with_hooks() -> (TestApi, EventHooks) {
        let (api, hooks, _rules) = test_api_with_extras();
        (api, hooks)
    }

    fn test_api_with_extras() -> (TestApi, EventHooks, WindowRules) {
        let lua = init().expect("lua init");
        let snapshot = Arc::new(Mutex::new(crate::ipc::Snapshot::default()));
        let (cmd_tx, cmd_rx) = flume::unbounded();
        let keybinds = Arc::new(Mutex::new(Vec::new()));
        let decoration = Arc::new(Mutex::new(DecorationConfig::default()));
        let dock_entries = Arc::new(Mutex::new(Vec::new()));
        let input_config = Arc::new(Mutex::new(InputConfig::default()));
        let hooks = Arc::new(Mutex::new(Vec::new()));
        let rules = Arc::new(Mutex::new(Vec::new()));
        register_api(
            &lua,
            snapshot,
            &cmd_tx,
            keybinds.clone(),
            hooks.clone(),
            rules.clone(),
            decoration.clone(),
            dock_entries,
            input_config.clone(),
        )
        .expect("register api");
        ((lua, keybinds, input_config, decoration, cmd_rx), hooks, rules)
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

    // e09s04t1: `es.config{}` is a declarative call, not a bag of values.
    #[test]
    fn config_table_callable_applies_validated_values() {
        let (lua, _keybinds, _input_config, decoration, _cmd_rx) = test_api();
        lua.load(
            r#"
            es.config {
                wm_mode = "floating",
                tiling_layout = "equal",
                border_size = 3,
                outer_gap = 10,
                inner_gap = 4,
                titlebar_height = 28,
                dock_height = 32,
                corner_radius = 6,
                min_window_size = { width = 200, height = 150 },
            }
            "#,
        )
        .exec()
        .expect("es.config table call");
        let cfg = decoration.lock().unwrap();
        assert_eq!(cfg.wm_mode, WmMode::Floating);
        assert_eq!(cfg.tiling_layout, TilingLayout::Equal);
        assert_eq!(cfg.border_size, 3);
        assert_eq!(cfg.outer, 10);
        assert_eq!(cfg.inner, 4);
        assert_eq!(cfg.titlebar_height, 28);
        assert_eq!(cfg.dock_height, 32);
        assert_eq!(cfg.corner_radius, 6);
        assert_eq!((cfg.min_window_width, cfg.min_window_height), (200, 150));
    }

    #[test]
    fn config_table_unknown_key_errors_and_applies_nothing() {
        let (lua, _keybinds, _input_config, decoration, _cmd_rx) = test_api();
        let err = lua
            .load(r#"es.config { border_size = 9, totally_bogus_key = 1 }"#)
            .exec()
            .expect_err("unknown key must error");
        assert!(err.to_string().contains("totally_bogus_key"), "unexpected error: {err}");
        assert_eq!(
            decoration.lock().unwrap().border_size,
            DecorationConfig::standard().border_size,
            "no partial application"
        );
    }

    #[test]
    fn config_table_wrong_type_errors_by_key_and_applies_nothing() {
        let (lua, _keybinds, _input_config, decoration, _cmd_rx) = test_api();
        let err = lua
            .load(r#"es.config { outer_gap = 7, border_size = "wide" }"#)
            .exec()
            .expect_err("wrong type must error");
        assert!(err.to_string().contains("border_size"), "unexpected error: {err}");
        let nested = lua
            .load(r#"es.config { general = { mode = "sideways" } }"#)
            .exec()
            .expect_err("bad nested value must error");
        assert!(nested.to_string().contains("mode"), "unexpected error: {nested}");
        let cfg = decoration.lock().unwrap();
        assert_eq!(cfg.outer, DecorationConfig::standard().outer, "no partial application");
        assert_eq!(cfg.wm_mode, WmMode::Tiling, "no partial nested application");
    }

    #[test]
    fn config_table_nested_settings_apply_atomically() {
        let (lua, _keybinds, input_config, decoration, _cmd_rx) = test_api();
        lua.load(
            r#"
            es.config {
                general = { layout = "equal", mode = "floating" },
                border_color_focused = { 10, 20, 30, 40 },
                titlebar_color_unfocused = { 1, 2, 3, 4 },
                output_scale = { ["HDMI-A-1"] = 2.0 },
                output_position = { ["HDMI-A-1"] = { x = 1920, y = 0 } },
                touchpad = { tap = false, natural_scroll = true, speed = 1.25 },
                keymap = "xkb_keymap { };",
            }
            "#,
        )
        .exec()
        .expect("nested es.config call");
        {
            let cfg = decoration.lock().unwrap();
            assert_eq!(cfg.tiling_layout, TilingLayout::Equal);
            assert_eq!(cfg.wm_mode, WmMode::Floating);
            assert_eq!(cfg.border_color_focused, [10, 20, 30, 40]);
            assert_eq!(cfg.titlebar_color_unfocused, [1, 2, 3, 4]);
            assert_eq!(cfg.output_position("HDMI-A-1"), Some((1920, 0)));
        }
        let input = input_config.lock().unwrap();
        assert_eq!(input.output_scale("HDMI-A-1").map(|s| s.fractional_scale()), Some(2.0));
        assert_eq!(input.touchpad_tap, Some(false));
        assert_eq!(input.touchpad_natural_scroll, Some(true));
        assert_eq!(input.touchpad_speed, Some(1.25));
        assert_eq!(input.keymap.as_deref(), Some("xkb_keymap { };"));
    }

    // e09s04t2: a bind may name a Lua function instead of a closed action string.
    #[test]
    fn lua_function_bind_registers_callback_and_keeps_string_binds() {
        let (lua, keybinds, _input_config, _decoration, _cmd_rx) = test_api();
        lua.load(
            r#"
            es.bind("Super+Return", "spawn_terminal")
            es.bind("Super+F", function(n) callback_arg = n end, 42)
            "#,
        )
        .exec()
        .expect("function bind");
        let kbs = keybinds.lock().unwrap();
        assert_eq!(kbs.len(), 2);
        assert_eq!(kbs[0].action, "spawn_terminal");
        assert!(kbs[0].callback.is_none(), "string binds keep their old shape");
        assert_eq!(kbs[1].action, "lua_function");
        assert!(kbs[1].callback.is_some());
        assert_eq!(kbs[1].arg, Some(42));
    }

    #[test]
    fn lua_function_bind_runs_callback_with_and_without_arg() {
        let (lua, keybinds, _input_config, _decoration, _cmd_rx) = test_api();
        lua.load(
            r#"
            callback_hits = {}
            es.bind("Super+F", function(n) callback_hits[#callback_hits + 1] = n end, 7)
            es.bind("Super+G", function() callback_hits[#callback_hits + 1] = 0 end)
            "#,
        )
        .exec()
        .expect("function binds");
        let binds = keybinds.lock().unwrap().clone();
        drop(keybinds);
        call_keybind_callback(&lua, &binds[0]).expect("callback with arg");
        call_keybind_callback(&lua, &binds[1]).expect("callback without arg");
        let hits: Vec<i32> = lua.globals().get("callback_hits").expect("hits table");
        assert_eq!(hits, vec![7, 0]);
    }

    #[test]
    fn lua_function_bind_surfaces_callback_errors() {
        let (lua, keybinds, _input_config, _decoration, _cmd_rx) = test_api();
        lua.load(r#"es.bind("Super+E", function() error("callback exploded") end)"#)
            .exec()
            .expect("function bind");
        let bind = keybinds.lock().unwrap()[0].clone();
        let err = call_keybind_callback(&lua, &bind).expect_err("callback error must surface");
        assert!(err.to_string().contains("callback exploded"), "unexpected error: {err}");
    }

    #[test]
    fn lua_function_bind_rejects_non_action_value() {
        let (lua, keybinds, _input_config, _decoration, _cmd_rx) = test_api();
        let err = lua
            .load(r#"es.bind("Super+H", 123)"#)
            .exec()
            .expect_err("a number is not an action");
        assert!(err.to_string().contains("string or function"), "unexpected error: {err}");
        assert!(keybinds.lock().unwrap().is_empty(), "failed bind must not register");
    }

    // e09s04t3: hooks receive the event's JSON data as a Lua table after flush.
    #[test]
    fn lua_event_hook_registers_and_fires_with_data() {
        let ((lua, _keybinds, _input_config, _decoration, _cmd_rx), hooks) = test_api_with_hooks();
        lua.load(
            r#"
            events_seen = {}
            es.on("window_open", function(data)
                events_seen[#events_seen + 1] = data.app_id
            end)
            "#,
        )
        .exec()
        .expect("es.on registration");
        assert_eq!(hooks.lock().unwrap().len(), 1, "one registered hook");
        let events = vec![Event {
            seq: 1,
            kind: EventKind::WindowOpen,
            data: json!({"app_id": "alacritty"}),
        }];
        let errors = fire_event_hooks(&lua, &hooks, &events);
        assert!(errors.is_empty(), "hook errors: {errors:?}");
        let seen: Vec<String> = lua.globals().get("events_seen").expect("seen table");
        assert_eq!(seen, vec!["alacritty".to_string()]);
    }

    #[test]
    fn lua_event_hook_rejects_unknown_kind() {
        let ((lua, _keybinds, _input_config, _decoration, _cmd_rx), hooks) = test_api_with_hooks();
        let err = lua
            .load(r#"es.on("key_press", function() end)"#)
            .exec()
            .expect_err("key content is not an event kind");
        assert!(err.to_string().contains("key_press"), "unexpected error: {err}");
        assert!(hooks.lock().unwrap().is_empty(), "bad kind must not register");
        lua.load(r#"es.on("config_reloaded", function() end)"#)
            .exec()
            .expect("known kind still registers");
        assert_eq!(hooks.lock().unwrap().len(), 1);
    }

    #[test]
    fn lua_event_hook_filters_kinds_and_converts_nested_data() {
        let ((lua, _keybinds, _input_config, _decoration, _cmd_rx), hooks) = test_api_with_hooks();
        lua.load(
            r#"
            opened = {}
            closed = {}
            es.on("window_open", function(data)
                opened[#opened + 1] = { data.app_id, data.tags[1], data.rect.x }
            end)
            es.on("window_close", function(data) closed[#closed + 1] = data.id end)
            "#,
        )
        .exec()
        .expect("two hooks");
        let events = vec![
            Event {
                seq: 1,
                kind: EventKind::WindowOpen,
                data: json!({"app_id": "foot", "tags": ["alpha", "beta"], "rect": {"x": 12, "y": 8}}),
            },
            Event {
                seq: 2,
                kind: EventKind::WindowActive,
                data: json!({"id": 99}),
            },
        ];
        let errors = fire_event_hooks(&lua, &hooks, &events);
        assert!(errors.is_empty(), "hook errors: {errors:?}");
        let opened: Table = lua.globals().get("opened").expect("opened table");
        let first: Table = opened.get(1).expect("first opened event");
        assert_eq!(first.get::<String>(1).expect("app_id"), "foot");
        assert_eq!(first.get::<String>(2).expect("array item"), "alpha");
        assert_eq!(first.get::<i64>(3).expect("nested x"), 12);
        assert!(opened.get::<Table>(2).is_err(), "only one hook fired");
        let closed: Vec<i64> = lua.globals().get("closed").expect("closed table");
        assert!(closed.is_empty(), "unsubscribed kind must not fire");
    }

    #[test]
    fn lua_event_hook_continues_after_callback_error() {
        let ((lua, _keybinds, _input_config, _decoration, _cmd_rx), hooks) = test_api_with_hooks();
        lua.load(
            r#"
            survivor = 0
            es.on("window_open", function() error("hook exploded") end)
            es.on("window_open", function() survivor = survivor + 1 end)
            "#,
        )
        .exec()
        .expect("two hooks on one kind");
        let events = vec![Event {
            seq: 1,
            kind: EventKind::WindowOpen,
            data: json!({}),
        }];
        let errors = fire_event_hooks(&lua, &hooks, &events);
        assert_eq!(errors.len(), 1, "only the bad hook errors");
        assert!(errors[0].contains("window_open"), "error names the event");
        assert!(errors[0].contains("hook exploded"), "error keeps Lua message");
        let survivor: i32 = lua.globals().get("survivor").expect("survivor");
        assert_eq!(survivor, 1, "a bad hook must not silence the next one");
    }

    // e09s04t4: config rules are staged, then installed before windows appear.
    #[test]
    fn lua_window_rule_registers_and_installs_before_add_window() {
        let ((lua, _keybinds, _input_config, _decoration, _cmd_rx), _hooks, rules) = test_api_with_extras();
        lua.load(
            r#"es.window_rule { app_id = "foot", workspace = 2, mode = "floating" }"#,
        )
        .exec()
        .expect("window rule registration");
        assert_eq!(rules.lock().unwrap().len(), 1);
        let mut wm = WindowManager::new();
        let output = wm.add_output("eDP-1", Rect { x: 0, y: 0, width: 1920, height: 1080 });
        assert!(wm.add_workspace(output, 2, "2"));
        drain_window_rules(&mut wm, &rules);
        assert!(rules.lock().unwrap().is_empty(), "drained rules are gone");
        let id = wm.add_window(output, "foot", "one").expect("window added");
        let window = wm.window(id).expect("window record");
        assert_eq!(window.workspace, 2);
        assert_eq!(window.mode, ClientMode::floating());
    }

    #[test]
    fn lua_window_rule_validates_app_id_mode_and_workspace() {
        let ((lua, _keybinds, _input_config, _decoration, _cmd_rx), _hooks, rules) = test_api_with_extras();
        for (source, expected) in [
            (r#"es.window_rule { mode = "floating" }"#, "app_id"),
            (r#"es.window_rule { app_id = "" }"#, "app_id"),
            (r#"es.window_rule { app_id = "foot", mode = "sideways" }"#, "mode"),
            (r#"es.window_rule { app_id = "foot", workspace = "two" }"#, "workspace"),
            (r#"es.window_rule { app_id = "foot", bogus = 1 }"#, "bogus"),
        ] {
            let err = lua.load(source).exec().expect_err("bad rule must error");
            assert!(err.to_string().contains(expected), "source: {source}; error: {err}");
        }
        assert!(rules.lock().unwrap().is_empty(), "no bad rule is staged");
    }

    #[test]
    fn lua_window_rule_replaces_on_second_load() {
        let ((lua, _keybinds, _input_config, _decoration, _cmd_rx), _hooks, rules) = test_api_with_extras();
        lua.load(r#"es.window_rule { app_id = "one", workspace = 2 }"#)
            .exec()
            .expect("first load");
        assert_eq!(rules.lock().unwrap().len(), 1);
        clear_window_rules(&rules);
        lua.load(r#"es.window_rule { app_id = "two", mode = "floating" }"#)
            .exec()
            .expect("second load");
        {
            let staged = rules.lock().unwrap();
            assert_eq!(staged.len(), 1, "reload replaces instead of stacking");
            assert_eq!(staged[0].app_id, "two");
        }
        let mut wm = WindowManager::new();
        let output = wm.add_output("eDP-1", Rect { x: 0, y: 0, width: 1920, height: 1080 });
        assert!(wm.add_workspace(output, 2, "2"));
        drain_window_rules(&mut wm, &rules);
        let old = wm.add_window(output, "one", "old").expect("old app");
        let new = wm.add_window(output, "two", "new").expect("new app");
        assert_eq!(wm.window(old).unwrap().workspace, 1, "old rule is gone");
        assert_eq!(wm.window(new).unwrap().mode, ClientMode::floating());
    }
}
