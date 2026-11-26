// Native control channel v0 (see notes/tacit-integration.md).
//
// Newline-delimited JSON over $XDG_RUNTIME_DIR/eswm-<wayland-display>.sock.
// Requests are handed to the compositor loop, so every command sees live WM
// state; each request carries a reply channel so socket threads never touch
// `State`. Local, single-user socket: one thread per connection is enough.
use std::{
    collections::HashMap,
    io::{BufRead, BufReader, ErrorKind, Write},
    os::unix::{
        fs::{FileTypeExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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

    /// One frame per entity per kind per tick. Global kinds ignore a stray
    /// `id` in their data, so they still collapse to a single frame.
    pub fn dedupe_key(&self) -> (EventKind, Option<u64>) {
        let entity = match self.kind {
            EventKind::WindowOpen
            | EventKind::WindowClose
            | EventKind::WindowActive
            | EventKind::WindowTitle
            | EventKind::WindowClass
            | EventKind::WindowMode
            | EventKind::WindowWorkspace
            | EventKind::WorkspaceActive
            | EventKind::WorkspaceCreated
            | EventKind::MonitorAdded
            | EventKind::MonitorRemoved
            | EventKind::MonitorActive
            | EventKind::MonitorLayout
            | EventKind::LayerOpened
            | EventKind::LayerClosed => self.data.get("id").and_then(Value::as_u64),
            EventKind::ConfigReloaded | EventKind::Ready => None,
        };
        (self.kind, entity)
    }
}

/// e09s03t2: transitions wait here until the tick ends. A flush yields one
/// gap-free seq run; t3 broadcasts that run to every subscriber.
#[allow(dead_code)]
pub struct EventBus {
    pending: Vec<Event>,
    next_seq: u64,
}

#[allow(dead_code)]
impl Default for EventBus {
    fn default() -> Self {
        Self {
            pending: Vec::new(),
            next_seq: 1,
        }
    }
}

#[allow(dead_code)]
impl EventBus {
    pub fn queue(&mut self, kind: EventKind, data: Value) {
        self.pending.push(Event {
            seq: 0,
            kind,
            data,
        });
    }

    pub fn flush(&mut self) -> Vec<Event> {
        let mut latest: HashMap<(EventKind, Option<u64>), usize> = HashMap::new();
        let mut ready = Vec::new();
        for event in self.pending.drain(..) {
            let key = event.dedupe_key();
            match latest.get(&key) {
                Some(&index) => ready[index] = event,
                None => {
                    latest.insert(key, ready.len());
                    ready.push(event);
                }
            }
        }
        for (offset, event) in ready.iter_mut().enumerate() {
            event.seq = self.next_seq + offset as u64;
        }
        self.next_seq += ready.len() as u64;
        ready
    }
}

/// e09s03t3: subscribe-on-connect socket. The accept thread only touches
/// sockets; the compositor thread pushes frames through `broadcast`.
#[allow(dead_code)]
#[derive(Clone)]
pub struct EventStream {
    subscribers: Arc<Mutex<Vec<UnixStream>>>,
}

#[allow(dead_code)]
impl EventStream {
    pub fn start(socket: &str) -> Result<Self, Box<dyn std::error::Error>> {
        Self::start_at(event_socket_path(socket))
    }

    fn start_at(path: PathBuf) -> Result<Self, Box<dyn std::error::Error>> {
        let listener = bind_socket(&path)?;
        let subscribers = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&subscribers);
        std::thread::Builder::new()
            .name("eswm-events".into())
            .spawn(move || {
                let hello = hello_frame();
                for accepted in listener.incoming() {
                    let Ok(stream) = accepted else { continue };
                    let Ok(mut writer) = stream.try_clone() else { continue };
                    // Registered before hello, so reading hello proves the
                    // accept loop has this subscriber in its list.
                    sink.lock().unwrap().push(stream);
                    let _ = writeln!(writer, "{hello}");
                }
            })?;
        Ok(Self { subscribers })
    }

    pub fn broadcast(&self, events: &[Event]) {
        let mut subscribers = self.subscribers.lock().unwrap();
        subscribers.retain_mut(|stream| {
            events
                .iter()
                .all(|event| writeln!(stream, "{}", event.frame()).is_ok())
        });
    }

    #[cfg(test)]
    fn subscriber_count(&self) -> usize {
        self.subscribers.lock().unwrap().len()
    }
}

#[cfg(test)]
impl EventBus {
    /// Inspect pending frames without consuming them; producers use flush().
    pub fn pending(&self) -> &[Event] {
        &self.pending
    }
}

fn hello_frame() -> String {
    let events: Vec<&str> = EventKind::ALL.iter().map(|kind| kind.as_str()).collect();
    json!({"seq": 0, "kind": "hello", "data": {"version": 1, "events": events}}).to_string()
}

fn event_socket_path(socket: &str) -> PathBuf {
    let runtime = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| {
        let uid = unsafe { libc::getuid() };
        format!("/run/user/{uid}")
    });
    PathBuf::from(runtime).join(format!("eswm-{socket}.events.sock"))
}

/// Shared bind rule for both sockets: never unlink a live peer, chmod 0600.
fn bind_socket(path: &Path) -> Result<UnixListener, Box<dyn std::error::Error>> {
    if let Ok(meta) = std::fs::symlink_metadata(path)
        && meta.file_type().is_socket()
    {
        if UnixStream::connect(path).is_ok() {
            return Err(std::io::Error::new(
                ErrorKind::AddrInUse,
                format!("{} is already served", path.display()),
            )
            .into());
        }
        std::fs::remove_file(path)?;
    }
    let listener = UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

const MAX_LINE_BYTES: usize = 64 * 1024;

#[derive(Debug, PartialEq, Eq)]
enum CappedLine {
    Line(String),
    TooLong,
    Closed,
}

/// Read one NDJSON request without letting a client allocate unbounded memory.
fn read_capped_line<R: BufRead>(reader: &mut R) -> CappedLine {
    let mut buf = Vec::new();
    let mut limited = std::io::Read::take(reader, (MAX_LINE_BYTES + 2) as u64);
    let read = limited.read_until(b'\n', &mut buf);
    match read {
        Ok(0) => CappedLine::Closed,
        Ok(_) if !buf.ends_with(b"\n") || buf.len() - 1 > MAX_LINE_BYTES => CappedLine::TooLong,
        Ok(_) => CappedLine::Line(String::from_utf8_lossy(&buf).into_owned()),
        Err(_) => CappedLine::Closed,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PeerCred {
    pub uid: u32,
    pub gid: u32,
    pub pid: i32,
}

fn peer_cred(stream: &UnixStream) -> Option<PeerCred> {
    use std::{mem::MaybeUninit, os::fd::AsRawFd};
    let mut cred = MaybeUninit::<libc::ucred>::uninit();
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let rc = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            cred.as_mut_ptr().cast(),
            &mut len,
        )
    };
    (rc == 0).then(|| {
        let cred = unsafe { cred.assume_init() };
        PeerCred {
            uid: cred.uid,
            gid: cred.gid,
            pid: cred.pid,
        }
    })
}

/// Stable numeric codes so clients never parse English error text.
fn error_code(message: &str) -> u32 {
    match message {
        "invalid json" => 1,
        "line too long" => 5,
        "compositor did not respond" => 6,
        "denied" => 7,
        _ if message.starts_with("unknown command") => 2,
        _ if message.starts_with("no window") || message.starts_with("no live window") => 3,
        _ => 4,
    }
}

fn error_reply(id: &Value, message: &str) -> String {
    json!({"id": id, "error": message, "code": error_code(message)}).to_string()
}

/// Config writes never travel over a socket, even if a dispatcher shows up.
fn denied_reply(cmd: &str, id: &Value) -> Option<String> {
    matches!(cmd, "exec" | "config" | "config.write" | "reload" | "eval")
        .then(|| error_reply(id, "denied"))
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
        Self::start_at(socket_path(socket))
    }

    fn start_at(path: PathBuf) -> Result<Self, Box<dyn std::error::Error>> {
        let listener = bind_socket(&path)?;
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
    match peer_cred(&stream) {
        Some(cred) => tracing::info!(
            uid = cred.uid,
            pid = cred.pid,
            gid = cred.gid,
            "control client connected"
        ),
        None => tracing::warn!("control client credentials unavailable"),
    }
    let Ok(mut writer) = stream.try_clone() else { return };
    let mut reader = BufReader::new(stream);
    loop {
        let line = match read_capped_line(&mut reader) {
            CappedLine::Line(line) => line,
            CappedLine::TooLong => {
                let _ = writeln!(writer, "{}", error_reply(&Value::Null, "line too long"));
                return;
            }
            CappedLine::Closed => return,
        };
        let (reply_tx, reply_rx) = flume::bounded::<String>(1);
        if tx.send(Request { line, reply: reply_tx }).is_err() {
            return;
        }
        let reply = reply_rx
            .recv_timeout(REPLY_TIMEOUT)
            .unwrap_or_else(|_| error_reply(&Value::Null, "compositor did not respond"));
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
        Err(_) => return error_reply(&Value::Null, "invalid json"),
    };
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let empty = json!({});
    let args = request.get("args").unwrap_or(&empty);
    let cmd = request.get("cmd").and_then(Value::as_str).unwrap_or_default();
    if let Some(reply) = denied_reply(cmd, &id) {
        return reply;
    }
    match dispatch(state, cmd, args) {
        Ok(value) => json!({"id": id, "ok": value}).to_string(),
        Err(message) => error_reply(&id, &message),
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

    #[test]
    fn event_bus_flush_coalesces_keeping_last_value_at_first_position() {
        let mut bus = EventBus::default();
        bus.queue(EventKind::WindowActive, json!({"id": 1}));
        bus.queue(EventKind::WindowTitle, json!({"id": 2, "title": "old"}));
        bus.queue(EventKind::WindowActive, json!({"id": 3}));
        bus.queue(EventKind::WindowTitle, json!({"id": 2, "title": "new"}));

        let flushed = bus.flush();
        assert_eq!(flushed.len(), 3);
        assert_eq!(flushed[0].kind, EventKind::WindowActive);
        assert_eq!(flushed[0].data, json!({"id": 1}));
        assert_eq!(flushed[1].kind, EventKind::WindowTitle);
        assert_eq!(flushed[1].data, json!({"id": 2, "title": "new"}));
        assert_eq!(flushed[2].data, json!({"id": 3}));
    }

    #[test]
    fn event_bus_flush_assigns_gap_free_seq_across_ticks() {
        let mut bus = EventBus::default();
        bus.queue(EventKind::ConfigReloaded, json!({}));
        assert_eq!(bus.flush()[0].seq, 1);
        assert!(bus.flush().is_empty());

        bus.queue(EventKind::Ready, json!({}));
        bus.queue(EventKind::ConfigReloaded, json!({}));
        let flushed = bus.flush();
        assert_eq!(flushed.iter().map(|e| e.seq).collect::<Vec<_>>(), vec![2, 3]);
    }

    #[test]
    fn event_bus_coalesces_global_events_by_kind() {
        let mut bus = EventBus::default();
        bus.queue(EventKind::ConfigReloaded, json!({"n": 1}));
        bus.queue(EventKind::ConfigReloaded, json!({"n": 2}));
        let flushed = bus.flush();
        assert_eq!(flushed.len(), 1);
        assert_eq!(flushed[0].data, json!({"n": 2}));
    }

    use std::{
        os::unix::fs::PermissionsExt,
        path::{Path, PathBuf},
        sync::atomic::{AtomicUsize, Ordering},
    };

    struct TempSocket(PathBuf);

    impl TempSocket {
        fn new(label: &str) -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            Self(std::env::temp_dir().join(format!(
                "eswm-{}-{}-{}.sock",
                label,
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            )))
        }
    }

    impl Drop for TempSocket {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    fn client(path: &Path) -> (UnixStream, BufReader<UnixStream>) {
        let stream = UnixStream::connect(path).expect("connect to event socket");
        let reader = BufReader::new(stream.try_clone().expect("clone client"));
        (stream, reader)
    }

    fn read_line(reader: &mut BufReader<UnixStream>) -> String {
        let mut line = String::new();
        reader.read_line(&mut line).expect("read NDJSON line");
        line
    }

    #[test]
    fn event_socket_binds_0600_and_rejects_live_peer() {
        let path = TempSocket::new("mode");
        let stream = EventStream::start_at(path.0.clone()).expect("bind event socket");
        let mode = std::fs::metadata(&path.0).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert!(EventStream::start_at(path.0.clone()).is_err(), "live peer owns the path");
        drop(stream);
    }

    #[test]
    fn event_socket_hello_advertises_every_kind() {
        let path = TempSocket::new("hello");
        let _stream = EventStream::start_at(path.0.clone()).expect("bind event socket");
        let (_client, mut reader) = client(&path.0);
        let hello: Value = serde_json::from_str(&read_line(&mut reader)).unwrap();
        let events: Vec<&str> = EventKind::ALL.iter().map(|kind| kind.as_str()).collect();
        assert_eq!(
            hello,
            json!({"seq": 0, "kind": "hello", "data": {"version": 1, "events": events}})
        );
    }

    #[test]
    fn event_socket_broadcasts_same_seq_to_all_subscribers() {
        let path = TempSocket::new("broadcast");
        let stream = EventStream::start_at(path.0.clone()).expect("bind event socket");
        let (client_a, mut reader_a) = client(&path.0);
        let (client_b, mut reader_b) = client(&path.0);
        assert_eq!(read_line(&mut reader_a), read_line(&mut reader_b));

        let mut bus = EventBus::default();
        bus.queue(EventKind::WindowActive, json!({"id": 42}));
        stream.broadcast(&bus.flush());

        let frame_a: Value = serde_json::from_str(&read_line(&mut reader_a)).unwrap();
        let frame_b: Value = serde_json::from_str(&read_line(&mut reader_b)).unwrap();
        assert_eq!(frame_a, frame_b);
        assert_eq!(frame_a, json!({"seq": 1, "kind": "window_active", "data": {"id": 42}}));
        drop(client_a);
        drop(client_b);
    }

    #[test]
    fn event_socket_drops_dead_subscribers() {
        let path = TempSocket::new("dead");
        let stream = EventStream::start_at(path.0.clone()).expect("bind event socket");
        let (client, mut reader) = client(&path.0);
        let _hello = read_line(&mut reader);
        assert_eq!(stream.subscriber_count(), 1);

        // Both handles are clones of the same local endpoint; leaving the
        // reader open would keep the peer alive and make writes succeed.
        drop((client, reader));
        let mut bus = EventBus::default();
        // A first write after peer close can still succeed; two make the
        // removal deterministic instead of racing the kernel's FIN handling.
        bus.queue(EventKind::Ready, json!({}));
        bus.queue(EventKind::ConfigReloaded, json!({}));
        stream.broadcast(&bus.flush());
        assert_eq!(stream.subscriber_count(), 0);
    }

    #[test]
    fn event_socket_replaces_stale_socket_file() {
        let path = TempSocket::new("stale");
        let stale = UnixListener::bind(&path.0).expect("bind stale socket");
        drop(stale);
        assert!(EventStream::start_at(path.0.clone()).is_ok());
    }

    #[test]
    fn control_hardening_control_socket_is_0600_with_live_peer_guard() {
        let path = TempSocket::new("control-mode");
        let control = Control::start_at(path.0.clone()).expect("bind control socket");
        let mode = std::fs::metadata(&path.0).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert!(Control::start_at(path.0.clone()).is_err(), "live peer owns the path");
        drop(control);
    }

    #[test]
    fn control_hardening_line_cap_rejects_oversized_line() {
        let (mut writer, reader) = UnixStream::pair().unwrap();
        let mut reader = BufReader::new(reader);
        writer.write_all(&vec![b'x'; MAX_LINE_BYTES + 1024]).unwrap();
        assert!(matches!(read_capped_line(&mut reader), CappedLine::TooLong));

        let (mut writer, reader) = UnixStream::pair().unwrap();
        let mut reader = BufReader::new(reader);
        writer.write_all(b"{\"id\":1,\"cmd\":\"state\"}\n").unwrap();
        match read_capped_line(&mut reader) {
            CappedLine::Line(line) => assert_eq!(line.trim(), "{\"id\":1,\"cmd\":\"state\"}"),
            other => panic!("a short line must be accepted: {other:?}"),
        }
    }

    #[test]
    fn control_hardening_peer_cred_matches_caller() {
        let (local, peer) = UnixStream::pair().unwrap();
        let cred = peer_cred(&local).expect("SO_PEERCRED on a connected socket");
        assert_eq!(cred.uid, unsafe { libc::getuid() });
        assert_eq!(cred.pid, std::process::id() as i32);
        drop(peer);
    }

    #[test]
    fn control_hardening_denies_exec_config_reload_and_eval() {
        for cmd in ["exec", "config", "config.write", "reload", "eval"] {
            let reply = denied_reply(cmd, &json!(7)).expect("denied");
            let parsed: Value = serde_json::from_str(&reply).unwrap();
            assert_eq!(parsed, json!({"id": 7, "error": "denied", "code": 7}), "{cmd}");
        }
        assert!(denied_reply("state", &json!(7)).is_none());
        assert!(denied_reply("window.focus", &json!(7)).is_none());
    }

    #[test]
    fn control_errors_line_too_long_and_timeout_have_codes() {
        let too_long: Value = serde_json::from_str(&error_reply(&Value::Null, "line too long")).unwrap();
        assert_eq!(too_long["code"], 5);
        assert_eq!(too_long["error"], "line too long");

        let timeout: Value =
            serde_json::from_str(&error_reply(&json!(9), "compositor did not respond")).unwrap();
        assert_eq!(timeout["code"], 6);
        assert_eq!(timeout["id"], 9);
    }
}
