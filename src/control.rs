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
    wm::{Rect, WindowMode},
    State,
};

const REPLY_TIMEOUT: Duration = Duration::from_secs(5);

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
        if let Ok(meta) = std::fs::symlink_metadata(&path) {
            if meta.file_type().is_socket() {
                std::fs::remove_file(&path)?;
            }
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
            Ok(json!({"window": id, "mode": mode_name(mode)}))
        }
        "workspace.focus" => {
            let output = state.wm.primary_output_id().ok_or("no output")?;
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
    if window.mode == WindowMode::Tiled {
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
    let max_x = geometry.width.saturating_sub(width) as i32;
    let max_y = geometry.height.saturating_sub(height) as i32;
    Ok(Rect {
        x: rect.x.clamp(0, max_x),
        y: rect.y.clamp(0, max_y),
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
        "mode": mode_name(window.mode),
        "mapped": window.mapped,
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
    let primary = state.wm.primary_output_id();
    state
        .wm
        .outputs()
        .map(|output| {
            json!({
                "id": output.id,
                "name": output.name,
                "geometry": rect_json(output.geometry),
                "active_workspace": output.active_workspace,
                "primary": Some(output.id) == primary,
            })
        })
        .collect()
}

fn rect_json(rect: Rect) -> Value {
    json!({"x": rect.x, "y": rect.y, "width": rect.width, "height": rect.height})
}

fn mode_name(mode: WindowMode) -> &'static str {
    match mode {
        WindowMode::Tiled => "tiled",
        WindowMode::Floating => "floating",
        WindowMode::Maximized => "maximized",
        WindowMode::Fullscreen => "fullscreen",
    }
}

fn parse_mode(name: &str) -> Option<WindowMode> {
    match name {
        "tiled" => Some(WindowMode::Tiled),
        "floating" => Some(WindowMode::Floating),
        "maximized" => Some(WindowMode::Maximized),
        "fullscreen" => Some(WindowMode::Fullscreen),
        _ => None,
    }
}
