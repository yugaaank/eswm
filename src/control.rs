// Native control channel v0 (see notes/tacit-integration.md).
//
// Newline-delimited JSON over $XDG_RUNTIME_DIR/eswm-<wayland-display>.sock.
// Requests are handed to the compositor loop, so every command sees live WM
// state; each request carries a reply channel so socket threads never touch
// `State`. Local, single-user socket: one thread per connection is enough.
use std::{
    io::{BufRead, BufReader, Write},
    os::unix::{
        fs::FileTypeExt,
        net::{UnixListener, UnixStream},
    },
    path::PathBuf,
    time::Duration,
};

use serde_json::{json, Value};

use crate::{
    wm::{ClientMode, Placement, Presentation, Rect},
    State,
};

const REPLY_TIMEOUT: Duration = Duration::from_secs(5);

/// e09s03t1: one flat, eswm-owned name per state transition we can emit.
/// Hyprland's dotted names and `>>` framing stayed in Hyprland; no confetti.
// e09s03 lands this model in stages: t1 defines the shape, t2+ wire the bus
// and sockets. Suppress only until the first emission site constructs it.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventKind {
    WindowOpen,
    WindowClose,
    WindowActive,
    WindowTitle,
    WindowClass,
    WindowMode,
    WindowWorkspace,
    WorkspaceActive,
    WorkspaceCreated,
    MonitorAdded,
    MonitorRemoved,
    MonitorActive,
    MonitorLayout,
    LayerOpened,
    LayerClosed,
    ConfigReloaded,
    Ready,
}

#[allow(dead_code)]
impl EventKind {
    /// Every kind we can emit today; also the `hello` vocabulary.
    pub const ALL: [Self; 17] = [
        Self::WindowOpen,
        Self::WindowClose,
        Self::WindowActive,
        Self::WindowTitle,
        Self::WindowClass,
        Self::WindowMode,
        Self::WindowWorkspace,
        Self::WorkspaceActive,
        Self::WorkspaceCreated,
        Self::MonitorAdded,
        Self::MonitorRemoved,
        Self::MonitorActive,
        Self::MonitorLayout,
        Self::LayerOpened,
        Self::LayerClosed,
        Self::ConfigReloaded,
        Self::Ready,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::WindowOpen => "window_open",
            Self::WindowClose => "window_close",
            Self::WindowActive => "window_active",
            Self::WindowTitle => "window_title",
            Self::WindowClass => "window_class",
            Self::WindowMode => "window_mode",
            Self::WindowWorkspace => "window_workspace",
            Self::WorkspaceActive => "workspace_active",
            Self::WorkspaceCreated => "workspace_created",
            Self::MonitorAdded => "monitor_added",
            Self::MonitorRemoved => "monitor_removed",
            Self::MonitorActive => "monitor_active",
            Self::MonitorLayout => "monitor_layout",
            Self::LayerOpened => "layer_opened",
            Self::LayerClosed => "layer_closed",
            Self::ConfigReloaded => "config_reloaded",
            Self::Ready => "ready",
        }
    }

    // Hyprland's camelCase checked into the flat snake_case hotel; no dots
    // made it past the lobby.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == name)
    }
}

/// e09s03t1: a single NDJSON frame. `seq` is assigned at flush, not emission.
#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub seq: u64,
    pub kind: EventKind,
    pub data: Value,
}

#[allow(dead_code)]
impl Event {
    pub fn frame(&self) -> String {
        json!({"seq": self.seq, "kind": self.kind.as_str(), "data": self.data}).to_string()
    }
}

struct Request {
    line: String,
    reply: flume::Sender<String>,
}

pub struct Control {
    rx: flume::Receiver<Request>,
}

impl Control {
    pub fn start(socket: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let path = socket_path(socket);
        if let Ok(meta) = std::fs::symlink_metadata(&path)
            && meta.file_type().is_socket() {
                std::fs::remove_file(&path)?;
            }
        let listener = UnixListener::bind(&path)?;
        let (tx, rx) = flume::unbounded::<Request>();
        std::thread::Builder::new()
            .name("eswm-control".into())
            .spawn(move || {
                for stream in listener.incoming() {
                    let Ok(stream) = stream else { continue };
                    let tx = tx.clone();
                    std::thread::spawn(move || serve(stream, tx));
                }
            })?;
        Ok(Self { rx })
    }

    fn drain(&self, state: &mut State) {
        while let Ok(request) = self.rx.try_recv() {
            let _ = request.reply.send(handle_request(state, &request.line));
        }
    }
}

/// Apply every pending control request. Call once per compositor loop tick.
pub fn drain(state: &mut State) {
    if let Some(control) = state.control.take() {
        control.drain(state);
        state.control = Some(control);
    }
}

fn socket_path(socket: &str) -> PathBuf {
    let runtime = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| {
        let uid = unsafe { libc::getuid() };
        format!("/run/user/{uid}")
    });
    PathBuf::from(runtime).join(format!("eswm-{socket}.sock"))
}

fn serve(stream: UnixStream, tx: flume::Sender<Request>) {
    let Ok(mut writer) = stream.try_clone() else { return };
    let mut reader = BufReader::new(stream);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        // shortcut: trusted local client, no line-length cap; add one if the
        // socket ever becomes reachable by other users
        let line = String::from_utf8_lossy(&buf).into_owned();
        let (reply_tx, reply_rx) = flume::bounded::<String>(1);
        if tx.send(Request { line, reply: reply_tx }).is_err() {
            return;
        }
        let reply = reply_rx
            .recv_timeout(REPLY_TIMEOUT)
            .unwrap_or_else(|_| json!({"id": null, "error": "compositor did not respond"}).to_string());
        if reply.is_empty() {
            continue;
        }
        if writeln!(writer, "{reply}").is_err() {
            return;
        }
    }
}

pub fn handle_request(state: &mut State, line: &str) -> String {
    let line = line.trim();
    if line.is_empty() {
        return String::new();
    }
    let request: Value = match serde_json::from_str(line) {
        Ok(request) => request,
        Err(_) => return json!({"id": null, "error": "invalid json"}).to_string(),
    };
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let empty = json!({});
    let args = request.get("args").unwrap_or(&empty);
    let cmd = request.get("cmd").and_then(Value::as_str).unwrap_or_default();
    match dispatch(state, cmd, args) {
        Ok(value) => json!({"id": id, "ok": value}).to_string(),
        Err(message) => json!({"id": id, "error": message}).to_string(),
    }
}

fn dispatch(state: &mut State, cmd: &str, args: &Value) -> Result<Value, String> {
    match cmd {
        "state" => Ok(state_json(state)),
        "window.list" => Ok(Value::Array(windows_json(state))),
        "workspace.list" => Ok(Value::Array(workspaces_json(state))),
        "output.list" => Ok(Value::Array(outputs_json(state))),
        "window.focus" => {
            let id = window_id(args)?;
            let window = state.wm.window(id).ok_or_else(|| format!("no window {id}"))?;
            let (output, workspace) = (window.output, window.workspace);
            if !state.wm.focus_window(id) {
                // the window lives on another workspace: switch, then focus
                if !state.wm.focus_workspace(output, workspace) || !state.wm.focus_window(id) {
                    return Err(format!("cannot focus window {id}"));
                }
            }
            crate::apply_layout(state);
            Ok(json!({"focused": id}))
        }
        "window.close" => {
            let id = window_id(args)?;
            let toplevel = state
                .xdg_shell_state
                .toplevel_surfaces()
                .iter()
                .find(|t| t.alive() && crate::window_id_of(t.wl_surface()) == Some(id))
                .cloned()
                .ok_or_else(|| format!("no live window {id}"))?;
            toplevel.send_close();
            Ok(json!({"closed": id}))
        }
        "window.move" => {
            let id = window_id(args)?;
            let workspace = args
                .get("workspace")
                .and_then(Value::as_i64)
                .ok_or("args.workspace (integer) is required")? as i32;
            if !state.wm.move_window_to_workspace(id, workspace) {
                return Err(format!("cannot move window {id} to workspace {workspace}"));
            }
            crate::apply_layout(state);
            Ok(json!({"moved": id, "workspace": workspace}))
        }
        "window.move_to" => {
            let id = window_id(args)?;
            let x = args.get("x").and_then(Value::as_i64).ok_or("args.x is required")? as i32;
            let y = args.get("y").and_then(Value::as_i64).ok_or("args.y is required")? as i32;
            let rect = floating_rect(state, id)?;
            let rect = clamp_to_output(state, id, Rect { x, y, ..rect })?;
            if !state.wm.set_window_rect(id, rect) {
                return Err(format!("window {id} is tiled"));
            }
            crate::apply_layout(state);
            Ok(json!({"moved": id, "rect": rect_json(rect)}))
        }
        "window.resize" => {
            let id = window_id(args)?;
            let width = args
                .get("width")
                .and_then(Value::as_u64)
                .ok_or("args.width is required")? as u32;
            let height = args
                .get("height")
                .and_then(Value::as_u64)
                .ok_or("args.height is required")? as u32;
            let rect = floating_rect(state, id)?;
            let rect = clamp_to_output(
                state,
                id,
                Rect { width: width.max(1), height: height.max(1), ..rect },
            )?;
            if !state.wm.set_window_rect(id, rect) {
                return Err(format!("window {id} is tiled"));
            }
            crate::apply_layout(state);
            Ok(json!({"resized": id, "rect": rect_json(rect)}))
        }
        "window.swap" => {
            let a = args.get("a").and_then(Value::as_u64).ok_or("args.a is required")?;
            let b = args.get("b").and_then(Value::as_u64).ok_or("args.b is required")?;
            if !state.wm.swap_windows(a, b) {
                return Err(format!("cannot swap windows {a} and {b}"));
            }
            crate::apply_layout(state);
            Ok(json!({"swapped": [a, b]}))
        }
        "window.set_mode" => {
            let id = window_id(args)?;
            let mode = args
                .get("mode")
                .and_then(Value::as_str)
                .and_then(parse_mode)
                .ok_or("args.mode must be tiled, floating, maximized or fullscreen")?;
            if !state.wm.set_mode(id, mode) {
                return Err(format!("no window {id}"));
            }
            crate::apply_layout(state);
            Ok(json!({"window": id, "mode": mode.mode_name()}))
        }
        "workspace.focus" => {
            let output = state.wm.focused_output_id().ok_or("no output")?;
            let workspace = match (args.get("id").and_then(Value::as_i64), args.get("delta").and_then(Value::as_i64)) {
                (Some(id), _) => id as i32,
                (None, Some(delta)) => state
                    .wm
                    .focus_workspace_relative(delta as i32)
                    .ok_or("no output")?,
                (None, None) => return Err("args.id or args.delta is required".into()),
            };
            // parity with the Super+N keybind: spawn the workspace if missing
            state.wm.ensure_workspace(output, workspace);
            if !state.wm.focus_workspace(output, workspace) {
                return Err(format!("no workspace {workspace} on the primary output"));
            }
            crate::apply_layout(state);
            Ok(json!({"workspace": workspace}))
        }
        _ => Err(format!("unknown command '{cmd}'")),
    }
}

fn window_id(args: &Value) -> Result<u64, String> {
    args.get("id")
        .and_then(Value::as_u64)
        .ok_or_else(|| "args.id (integer) is required".into())
}

fn floating_rect(state: &State, id: u64) -> Result<Rect, String> {
    let window = state.wm.window(id).ok_or_else(|| format!("no window {id}"))?;
    if window.mode.placement == Placement::Tiled && window.mode.presentation == Presentation::Normal {
        return Err(format!("window {id} is tiled"));
    }
    Ok(window.target_rect)
}

fn clamp_to_output(state: &State, id: u64, rect: Rect) -> Result<Rect, String> {
    let window = state.wm.window(id).ok_or_else(|| format!("no window {id}"))?;
    let geometry = state
        .wm
        .output(window.output)
        .map(|output| output.geometry)
        .ok_or_else(|| format!("window {id} has no output"))?;
    let width = rect.width.min(geometry.width).max(1);
    let height = rect.height.min(geometry.height).max(1);
    // Clamp in global coordinates: an output at x=1920 starts there, not at 0.
    let min_x = geometry.x;
    let min_y = geometry.y;
    let max_x = geometry.x.saturating_add(geometry.width as i32).saturating_sub(width as i32).max(min_x);
    let max_y = geometry.y.saturating_add(geometry.height as i32).saturating_sub(height as i32).max(min_y);
    Ok(Rect {
        x: rect.x.clamp(min_x, max_x),
        y: rect.y.clamp(min_y, max_y),
        width,
        height,
    })
}

fn state_json(state: &State) -> Value {
    json!({
        "focused": state.wm.focused_id(),
        "outputs": outputs_json(state),
        "workspaces": workspaces_json(state),
        "windows": windows_json(state),
    })
}

fn windows_json(state: &State) -> Vec<Value> {
    state
        .wm
        .window_ids()
        .into_iter()
        .filter_map(|id| state.wm.window(id).map(window_json))
        .collect()
}

fn window_json(window: &crate::wm::Window) -> Value {
    json!({
        "id": window.id,
        "app_id": window.app_id,
        "title": window.title,
        "workspace": window.workspace,
        "output": window.output,
        "mode": window.mode.mode_name(),
        "mapped": window.mapped && !window.minimized,
        "focused": window.focused,
        "rect": rect_json(window.rect),
        "target_rect": rect_json(window.target_rect),
    })
}

fn workspaces_json(state: &State) -> Vec<Value> {
    state
        .wm
        .workspaces()
        .map(|workspace| {
            let active = state
                .wm
                .output(workspace.output)
                .map(|output| output.active_workspace == workspace.id)
                .unwrap_or(false);
            json!({
                "id": workspace.id,
                "name": workspace.name,
                "output": workspace.output,
                "windows": workspace.windows,
                "focused": workspace.focused,
                "active": active,
            })
        })
        .collect()
}

fn outputs_json(state: &State) -> Vec<Value> {
    let focused = state.wm.focused_output_id();
    state
        .wm
        .outputs()
        .map(|output| {
            let is_focused = Some(output.id) == focused;
            json!({
                "id": output.id,
                "name": output.name,
                "geometry": rect_json(output.geometry),
                "active_workspace": output.active_workspace,
                "focused": is_focused,
                // Deprecated alias of `focused`, kept for Tacit's current
                // consumers. Two names, one boolean, no drama.
                "primary": is_focused,
            })
        })
        .collect()
}

fn rect_json(rect: Rect) -> Value {
    json!({"x": rect.x, "y": rect.y, "width": rect.width, "height": rect.height})
}

fn parse_mode(name: &str) -> Option<ClientMode> {
    match name {
        "tiled" => Some(ClientMode::tiled()),
        "floating" => Some(ClientMode::floating()),
        "maximized" => Some(ClientMode::maximized()),
        "fullscreen" => Some(ClientMode::fullscreen()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_model_frame_shape_is_seq_kind_data() {
        let event = Event {
            seq: 7,
            kind: EventKind::WindowActive,
            data: json!({"id": 42}),
        };
        let frame = event.frame();
        let parsed: Value = serde_json::from_str(&frame).expect("frame is NDJSON");
        assert_eq!(parsed, json!({"seq": 7, "kind": "window_active", "data": {"id": 42}}));
        assert!(!frame.contains(">>"), "eswm frames are not Hyprland text");
        assert!(!frame.contains("\"window.active\""), "eswm names are flat");
    }

    #[test]
    fn event_model_names_are_flat_and_roundtrip() {
        let names: Vec<&str> = EventKind::ALL.iter().map(|kind| kind.as_str()).collect();
        assert_eq!(names.len(), 17);
        assert!(names.iter().all(|name| name.chars().all(|c| c.is_ascii_lowercase() || c == '_')));
        for kind in EventKind::ALL {
            assert_eq!(EventKind::from_name(kind.as_str()), Some(kind));
        }
        assert_eq!(EventKind::from_name("window.active"), None);
        assert_eq!(EventKind::from_name("openwindow"), None);
        assert_eq!(EventKind::from_name("monitorAdded"), None);
    }

    #[test]
    fn event_model_dedupe_key_collapses_same_entity() {
        let titled = |id, title| Event {
            seq: 0,
            kind: EventKind::WindowTitle,
            data: json!({"id": id, "title": title}),
        };
        assert_eq!(titled(9, "old").dedupe_key(), titled(9, "new").dedupe_key());
        assert_ne!(titled(9, "old").dedupe_key(), titled(10, "old").dedupe_key());
        let mode = Event {
            kind: EventKind::WindowMode,
            ..titled(9, "old")
        };
        assert_ne!(mode.dedupe_key(), titled(9, "old").dedupe_key());
    }

    #[test]
    fn event_model_dedupe_key_covers_window_workspace_monitor_and_layer() {
        let entity_kinds = [
            EventKind::WindowOpen,
            EventKind::WindowClose,
            EventKind::WindowActive,
            EventKind::WindowTitle,
            EventKind::WindowClass,
            EventKind::WindowMode,
            EventKind::WindowWorkspace,
            EventKind::WorkspaceActive,
            EventKind::WorkspaceCreated,
            EventKind::MonitorAdded,
            EventKind::MonitorRemoved,
            EventKind::MonitorActive,
            EventKind::MonitorLayout,
            EventKind::LayerOpened,
            EventKind::LayerClosed,
        ];
        for kind in entity_kinds {
            let event = |id| Event {
                seq: 0,
                kind,
                data: json!({"id": id}),
            };
            assert_eq!(event(4).dedupe_key(), event(4).dedupe_key(), "{kind:?}");
            assert_ne!(event(4).dedupe_key(), event(5).dedupe_key(), "{kind:?}");
        }
    }

    #[test]
    fn event_model_dedupe_key_for_global_events_ignores_data() {
        let a = Event {
            seq: 0,
            kind: EventKind::ConfigReloaded,
            data: json!({"id": 7}),
        };
        let b = Event {
            seq: 0,
            kind: EventKind::ConfigReloaded,
            data: json!({"id": 8}),
        };
        assert_eq!(a.dedupe_key(), b.dedupe_key());
        let ready = Event {
            seq: 0,
            kind: EventKind::Ready,
            data: json!({"id": 7}),
        };
        assert_ne!(a.dedupe_key(), ready.dedupe_key());
    }
}
