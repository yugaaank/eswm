// M2: hyprctl-compatible IPC server. Compat contract (see notes/status.md):
// eswm shells out to the `hyprctl` binary, which finds us through
// $HYPRLAND_INSTANCE_SIGNATURE -> $XDG_RUNTIME_DIR/hypr/<sig>/.socket.sock.
// Wire protocol (verified with hyprctl 0.57 via spy server): one request blob
// "<flags>/<request>" (flags: j = -j, r = raw, b = batch), read until EOF.
// Reply = plain bytes (JSON when `j`) then close. Event stream is a second
// socket `.socket2.sock`: "name>>data\n" lines, connection stays open.
//
// Design: one dedicated command-socket thread (blocking accept, one connection
// = one request = one reply = close). The compositor loop broadcasts events by
// calling `emit` (writes "event>>data\n" to every connected event client).
use std::{
    io::{Read, Write},
    os::unix::{fs::FileTypeExt, net::UnixListener, net::UnixStream},
    path::PathBuf,
    sync::{Arc, Mutex},
};

use serde_json::{json, Value};

use smithay::{
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    wayland::{
        compositor::with_states,
        shell::xdg::XdgToplevelSurfaceData,
    },
};
/// One window/surface we track (toplevel or layer surface).
#[derive(Clone)]
pub struct ClientData {
    /// "0x..." stable per session (Hyprland-style address)
    pub address: String,
    pub class: String,
    pub title: String,
    pub initial_class: String,
    pub initial_title: String,
    pub pid: u32,
    pub workspace_id: Option<i32>,
    /// winit session is single-monitor
    pub monitor: u32,
    pub floating: bool,
    /// Hyprland bitmask: 1 = fullscreen, 2 = true fullscreen (client request)
    pub fullscreen: u8,
    pub focus_history_id: u32,
    pub at: (i32, i32),
    pub size: (i32, i32),
    /// true -> belongs in hyprctl layers, not clients
    pub layer: bool,
    pub layer_namespace: String,
    pub layer_level: u32,
    pub visible: bool,
}

/// Read-only view of compositor state; the main loop refreshes it on changes.
#[derive(Clone, Default)]
pub struct Snapshot {
    pub clients: Vec<ClientData>,
    /// (w, h, scale, refresh) of the single (winit) monitor
    pub monitor: (i32, i32, f64, f64),
    pub focused_address: Option<String>,
    pub active_workspace_id: i32,
    pub workspaces: Vec<(i32, String)>,
    pub cursor_pos: (i32, i32),
    pub keyboard_layout: String,
    pub keyboard_rules: String,
}

/// Commands for the compositor main loop.
#[derive(Debug)]
pub enum CompCmd {
    /// hyprctl reload: main loop emits "configreloaded>>"
    Reload,
    /// `hyprctl dispatch workspace N`
    GotoWorkspace(i32),
    /// `hyprctl dispatch [pin] closewindow <addr>`
    CloseWindow(String),
    /// anything eswm sends before we implement it
    Unsupported(String),
}

/// Event clients; main loop pushes lines through SharedSinks.
pub struct SharedSinks(Arc<Mutex<Vec<UnixStream>>>);
impl SharedSinks {
    /// Send "event>>data\n" to every connected event client (socket2 listeners).
    pub fn emit(&self, event: &str, data: &str) {
        let mut sinks = self.0.lock().unwrap();
        let line = format!("{event}>>{data}\n");
        // dead sockets get dropped: they'll be re-listed after reconnect
        sinks.retain(|mut c| c.write_all(line.as_bytes()).is_ok());
    }
}

pub struct Ipc {
    pub snapshot: Arc<Mutex<Snapshot>>,
    pub sinks: SharedSinks,
    pub cmd_rx: flume::Receiver<CompCmd>,
    pub cmd_tx: flume::Sender<CompCmd>,
}

/// Rebuild the IPC snapshot from live compositor state. Cheap; called once
/// per frame by the main loop. `size` is the (single) output size.
pub fn refresh_snapshot(state: &crate::State, size: (i32, i32)) {
    let Some(ipc) = &state.ipc else { return };
    let mut clients = Vec::new();
    for (i, surface) in state.xdg_shell_state.toplevel_surfaces().iter().enumerate() {
        let (class, title) = toplevel_class_title(surface.wl_surface());
        clients.push(ClientData {
            address: client_address(i),
            class: class.clone(),
            title: title.clone(),
            initial_class: class,
            initial_title: title,
            pid: 0, // proper pid via fd-passing later; fixture-level parity first
            workspace_id: Some(1),
            monitor: 0,
            floating: false,
            fullscreen: 0,
            focus_history_id: i as u32,
            at: (0, 0),
            size: (size.0, size.1),
            layer: false,
            layer_namespace: String::new(),
            layer_level: 0,
            visible: true,
        });
    }
    let focused = clients.first().map(|c| c.address.clone());
    let snap = Snapshot {
        clients,
        monitor: (size.0, size.1, 1.0, 60.0),
        focused_address: focused,
        active_workspace_id: 1,
        workspaces: vec![(1, "1".into())],
        cursor_pos: (0, 0),
        keyboard_layout: "English (US)".into(),
        keyboard_rules: String::new(),
    };
    *ipc.snapshot.lock().unwrap() = snap;
}

/// title/app_id straight from the role attributes (set by xdg_toplevel.set_title et al)
fn toplevel_class_title(surface: &WlSurface) -> (String, String) {
    with_states(surface, |states| {
        states
            .data_map
            .get::<XdgToplevelSurfaceData>()
            .map(|m| {
                let d = m.lock().unwrap();
                (d.app_id.clone().unwrap_or_default(), d.title.clone().unwrap_or_default())
            })
            .unwrap_or_default()
    })
}

/// Hyprland-style "0x<hex>" address; stable within a session via enumerate
/// index (true pointer addresses come with the M5 window-state rewrite).
fn client_address(i: usize) -> String {
    format!("0x{:x}", 1000 + i)
}

fn hypr_dir(sig: &str) -> PathBuf {
    let xdg = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| {
        // shell-owned UID lookup via getuid(2); not a trust boundary
        let uid = unsafe { libc::getuid() };
        format!("/run/user/{uid}")
    });
    let d = PathBuf::from(xdg).join("hypr").join(sig);
    std::fs::create_dir_all(&d).expect("cannot create $XDG_RUNTIME_DIR/hypr/<sig>");
    d
}

impl Ipc {
    /// Binds both sockets under `$XDG_RUNTIME_DIR/hypr/$sig/*.sock` and starts
    /// the IPC thread. Tests set XDG_RUNTIME_DIR (and the signature is theirs
    /// to choose) before calling this.
    pub fn new(sig: String) -> Self {
        let dir = hypr_dir(&sig);
        let cmd_path = dir.join(".socket.sock");
        let event_path = dir.join(".socket2.sock");
        for p in [&cmd_path, &event_path] {
            if let Ok(md) = std::fs::metadata(p) {
                if md.file_type().is_socket() {
                    std::fs::remove_file(p).ok();
                }
            }
        }
        let cmd_l = UnixListener::bind(&cmd_path).expect("bind .socket.sock");
        let event_l = UnixListener::bind(&event_path).expect("bind .socket2.sock");

        let sinks = SharedSinks(Arc::new(Mutex::new(Vec::new())));
        let (cmd_tx, cmd_rx) = flume::unbounded::<CompCmd>();
        let snapshot: Arc<Mutex<Snapshot>> = Arc::default();

        // Event socket: accept + keep connections. One thread, small poll loop
        // over nonblocking accept (clients never send anything).
        {
            let sinks = sinks.0.clone();
            std::thread::Builder::new()
                .name("ipc-events".into())
                .spawn(move || {
                    event_l.set_nonblocking(true).unwrap();
                    loop {
                        if let Ok((c, _)) = event_l.accept() {
                            c.set_nonblocking(true).ok();
                            let mut list = sinks.lock().unwrap();
                            list.push(c);
                        }
                        std::thread::sleep(std::time::Duration::from_millis(50));
                    }
                })
                .unwrap();
        }

        // Command socket: blocking accept; one connection = one request = reply
        // then close (REPLY_MODE_CLOSE, as verified with hyprctl 0.57).
        {
            let snapshot = snapshot.clone();
            let cmd_tx = cmd_tx.clone();
            std::thread::Builder::new()
                .name("ipc-cmd".into())
                .spawn(move || {
                    for stream in cmd_l.incoming() {
                        let Ok(stream) = stream else { continue };
                        // hyprctl does NOT shutdown its write side (verified via
                        // spy + strace-less probe): it sends the whole request in
                        // one blob and polls for the reply. Read the first chunk
                        // blocking, then briefly drain; EOF (tests/other clients)
                        // also ends the request.
                        let mut buf = [0; 4096];
                        stream.set_nonblocking(false).ok();
                        let len = match (&stream).read(&mut buf) {
                            Ok(0) | Err(_) => continue,
                            Ok(len) => len,
                        };
                        let buf = String::from_utf8_lossy(&buf[..len]).into_owned();
                        let reply = handle_request(&buf, &snapshot, &cmd_tx);
                        stream.set_nonblocking(false).ok();
                        let _ = (&stream).write_all(reply.as_bytes());
                        let _ = (&stream).write_all(b"\n");
                        let _ = stream.shutdown(std::net::Shutdown::Both);
                    }
                })
                .unwrap();
        }

        Ipc { snapshot, sinks, cmd_rx, cmd_tx }
    }
}

/// Parse "<flags>/<request>" and produce the reply string.
pub fn handle_request(request: &str, snap: &Arc<Mutex<Snapshot>>, cmd_tx: &flume::Sender<CompCmd>) -> String {
    let req = request.trim();
    let (raw_flags, req) = match req.split_once('/') {
        Some((f, r)) => (f.to_string(), r.to_string()),
        None => (String::new(), req.to_string()),
    };
    let j = raw_flags.contains('j');
    let r = raw_flags.contains('r');
    let cmd = req.trim().to_owned();
    let path = cmd.split_whitespace().next().unwrap_or_default().to_owned();
    let args = cmd[path.len()..].trim().to_owned();

    // reload is the one request with a side effect before a reply; the main
    // loop drains the channel and emits `configreloaded` when it processes it
    macro_rules! requires {
        ($variant:expr) => {
            match cmd_tx.send($variant) {
                Ok(()) => "ok".to_string(),
                Err(_) => "compositor loop gone".into(),
            }
        };
    }

    let snap = snap.lock().unwrap();
    match (j, r, path.as_str()) {
        (true, _, "monitors") => serde_json::to_string(&monitors_json(&snap)).unwrap(),
        (true, _, "clients") => serde_json::to_string(&clients_json(&snap)).unwrap(),
        (true, _, "layers") => serde_json::to_string(&layers_json(&snap)).unwrap(),
        (true, _, "devices") => serde_json::to_string(&devices_json(&snap)).unwrap(),
        (true, _, "activewindow") => serde_json::to_string(&active_window(&snap)).unwrap(),
        (true, _, "activewindowv2") => snap.focused_address.clone().unwrap_or_default(),
        (true, _, "activeworkspace") => serde_json::to_string(&active_workspace(&snap)).unwrap(),
        (false, true, "activeworkspace") => active_workspace(&snap)["name"].as_str().unwrap_or_default().to_string(),
        (true, _, "workspaces") => serde_json::to_string(&workspaces_json(&snap)).unwrap(),
        (true, _, "cursorpos") => {
            let (x, y) = snap.cursor_pos;
            format!("{x},{y}")
        }
        (true, _, "version") => version_json().to_string(),
        (false, _, "reload") => {
            let _ = cmd_tx.send(CompCmd::Reload);
            "ok".to_string()
        }
        (false, _, "workspace") => {
            let ws = parse_ws(&args);
            let _ = cmd_tx.send(CompCmd::GotoWorkspace(ws));
            "ok".to_string()
        }
        (false, _, "closewindow") => {
            let args = args.trim().to_string();
            let _ = cmd_tx.send(CompCmd::CloseWindow(args));
            "ok".to_string()
        }
        (false, _, "startup") | (false, _, "startupscripts") => "ok".into(),
        (false, _, "notify") => "ok".into(), // notify [icon] [time_ms] text
        (false, _, "setcursor") => "ok".into(),
        // Keyword/reload dispatch via Lua API
        (false, _, "bind") => {
            // Parse "keybind_string" from args; send as CompCmd::Unsupported for now
            // In full implementation, parse Hyprland keybind format and dispatch
            let _keybind = args.trim().to_string();
            let _ = cmd_tx.send(CompCmd::Unsupported(format!("bind {}", _keybind)));
            "ok".to_string()
        }
        (false, _, "define_submap") => {
            // Parse "map_name" from args; send as CompCmd
            let map_name = args.trim().split_whitespace().next().unwrap_or("").to_string();
            let _ = cmd_tx.send(CompCmd::Unsupported(format!("define_submap {}", map_name)));
            "ok".to_string()
        }
        // Hyprland answers unknown commands with "invalid request"; hyprctl
        // exits 0 and eswm depends on those failing soft
        _ => "invalid request".to_string(),
    }
}

fn parse_ws(args: &str) -> i32 {
    args.trim().split_whitespace().next().and_then(|w| w.replace("special:", "r").parse().ok()).unwrap_or(1)
}

/// hyprctl -j version: Hyprland field shape, our own identity. Clients parse
/// by reading keys (eswm only checks version in a log line / fixture match).
fn version_json() -> Value {
    json!({
        "branch": "eswm",
        "commit": "unknown",
        "version": "0.0.1",
        "dirty": false,
        "commit_message": "eswm v0.1.0",
        "commit_date": "",
        "tag": "v0.0.1",
        "commits": "0",
        "buildAquamarine": "",
        "buildHyprlang": "",
        "buildHyprutils": "",
        "buildHyprcursor": "",
        "buildHyprgraphics": "",
        "systemAquamarine": "",
        "systemHyprlang": "",
        "systemHyprutils": "",
        "systemHyprcursor": "",
        "systemHyprgraphics": "",
        "abiHash": "",
        "flags": []
    })
}

fn monitors_json(snap: &Snapshot) -> Value {
    let (w, h, scale, refresh) = snap.monitor;
    json!([{
        "id": 0,
        "name": "winit",
        "description": "winit (virtual)",
        "make": "",
        "model": "",
        "serial": "",
        "width": w,
        "height": h,
        "physicalWidth": 0,
        "physicalHeight": 0,
        "refreshRate": refresh,
        "x": 0,
        "y": 0,
        "activeWorkspace": { "id": snap.active_workspace_id, "name": snap.active_workspace_id.to_string() },
        "specialWorkspace": { "id": 0, "name": "" },
        "reserved": [0, 0, 0, 0],
        "scale": scale,
        "transform": 0,
        "focused": true,
        "dpmsStatus": true,
        "vrr": false,
        "solitary": "0",
        "solitaryBlockedBy": "",
        "activelyTearing": false,
        "tearingBlockedBy": "",
        "directScanoutTo": "0",
        "directScanoutBlockedBy": "",
        "disabled": false,
        "currentFormat": "8-bit RGB",
        "mirrorOf": "none",
        "availableModes": [],
        "colorManagementPreset": "srgb",
        "sdrBrightness": 1.0,
        "sdrSaturation": 1.0,
        "sdrMinLuminance": 0.2,
        "sdrMaxLuminance": 80.0,
        "hardwareCursorsInUse": 0,
        "hardwareDetails": {},
    }])
}

fn ws_ref(id: Option<i32>) -> Value {
    // Hyprland "none" workspace id is -1 named EMPTY
    match id {
        Some(id) => json!({ "address": id.to_string(), "id": id, "type": "normal", "name": id.to_string() }),
        None => json!({ "address": "-1", "id": -1, "type": "normal", "name": "EMPTY" }),
    }
}

fn clients_json(snap: &Snapshot) -> Value {
    Value::Array(snap.clients.iter().filter(|c| !c.layer).map(|c| client_obj(c, snap)).collect())
}

fn client_obj(c: &ClientData, _snap: &Snapshot) -> Value {
    let workspace = ws_ref(c.workspace_id);
    json!({
        "address": c.address,
        "mapped": true,
        "hidden": false,
        "visible": c.visible,
        "acceptsInput": true,
        "at": [c.at.0, c.at.1],
        "size": [c.size.0, c.size.1],
        "workspace": workspace,
        "floating": c.floating,
        "monitor": c.monitor,
        "class": c.class,
        "title": c.title,
        "initialClass": c.initial_class,
        "initialTitle": c.initial_title,
        "pid": c.pid,
        "xwayland": false,
        "pinned": false,
        "pinFullscreened": false,
        "fullscreen": c.fullscreen as i64,
        "fullscreenClient": 0,
        "fullscreenHandler": false,
        "allowedOverFullscreen": false,
        "grouped": [],
        "tags": [],
        "swallowing": "0x0",
        "focusHistoryID": c.focus_history_id,
        "inhibitingIdle": false,
        "xdgTag": "",
        "xdgDescription": "",
        "contentType": 0,
        "tearingHint": false,
        "stableId": format!("unk{:x}", c.pid),
    })
}

fn layers_json(snap: &Snapshot) -> Value {
    // fixture shape: { "<output>": { "levels": { "0": [...], "1": [...], "2": [...], "3": [...] } } }
    let mut levels: [Vec<Value>; 4] = Default::default();
    for c in snap.clients.iter().filter(|c| c.layer) {
        levels[c.layer_level as usize].push(json!({
            "address": c.address,
            "x": c.at.0,
            "y": c.at.1,
            "w": c.size.0,
            "h": c.size.1,
            "alpha": 1.0,
            "namespace": c.layer_namespace,
            "pid": c.pid,
        }));
    }
    json!({ "winit": { "levels": {
        "0": levels[0], "1": levels[1], "2": levels[2], "3": levels[3],
    } } })
}

fn devices_json(snap: &Snapshot) -> Value {
    // wants keyboards in particular (HyprlandXkb.qml reads layout/active_keymap/main)
    json!({
        "mice": [],
        "keyboards": [{
            "address": "0x0",
            "name": "winit keyboard",
            "rules": snap.keyboard_rules,
            "model": "",
            "layout": snap.keyboard_layout,
            "variant": "",
            "options": "",
            "active_layout_index": 0,
            "active_keymap": snap.keyboard_layout,
            "capsLock": false,
            "numLock": false,
            "main": true,
        }],
        "tablets": [],
        "touch": [],
        "switches": [],
    })
}

fn active_window(snap: &Snapshot) -> Value {
    // {} when nothing focused (jq-based shells expect a soft empty)
    snap.clients
        .iter()
        .find(|c| Some(&c.address) == snap.focused_address.as_ref())
        .map(|c| client_obj(c, snap))
        .unwrap_or(json!({}))
}

fn workspace_obj(ws: (i32, String), snap: &Snapshot) -> Value {
    let (id, name) = ws;
    let count = snap.clients.iter().filter(|c| c.workspace_id == Some(id)).count();
    let last = snap.clients.iter().filter(|c| c.workspace_id == Some(id)).last();
    json!({
        "address": id.to_string(),
        "id": id,
        "type": "normal",
        "name": name,
        "monitor": "winit",
        "monitorID": 0,
        "windows": count,
        "hasfullscreen": false,
        "lastwindow": last.map(|c| c.address.clone()).unwrap_or_else(|| "0x0".into()),
        "lastwindowtitle": last.map(|c| c.title.clone()).unwrap_or_default(),
        "ispersistent": false,
        "tiledLayout": [],
    })
}

fn active_workspace(snap: &Snapshot) -> Value {
    workspace_obj((snap.active_workspace_id, snap.active_workspace_id.to_string()), snap)
}

fn workspaces_json(snap: &Snapshot) -> Value {
    Value::Array(snap
        .workspaces
        .iter()
        .map(|(id, name)| workspace_obj((*id, name.clone()), snap))
        .collect())
}

/// Structural key-parity vs fixtures: every fixture key must exist on our
/// objects (values may differ; we don't run on real hardware here).
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn load(q: &str) -> Value {
        serde_json::from_str(&fs::read_to_string(format!("tests/fixtures/hyprctl-{q}.json")).unwrap()).unwrap()
    }

    fn snap() -> Snapshot {
        Snapshot {
            clients: vec![ClientData {
                address: "0x1".into(),
                class: "foot".into(),
                title: "t".into(),
                initial_class: "foot".into(),
                initial_title: "t".into(),
                pid: 1,
                workspace_id: Some(1),
                monitor: 0,
                floating: false,
                fullscreen: 0,
                focus_history_id: 0,
                at: (0, 0),
                size: (10, 10),
                layer: false,
                layer_namespace: String::new(),
                layer_level: 0,
                visible: true,
            }],
            monitor: (100, 200, 1.0, 60.0),
            workspaces: vec![(1, "1".into())],
            active_workspace_id: 1,
            ..Default::default()
        }
    }

    fn parity(mine: &Value, fixture: &Value, what: &str) {
        let missing: Vec<&String> = fixture
            .as_object()
            .unwrap()
            .keys()
            .filter(|k| mine.get(*k).is_none())
            .collect();
        assert!(missing.is_empty(), "{what}: missing keys {missing:?}");
    }

    #[test]
    fn key_parity_monitors() {
        let fixture = load("monitors");
        parity(&monitors_json(&snap())[0], &fixture[0], "monitor");
    }

    #[test]
    fn key_parity_client() {
        let fixture = load("clients");
        parity(&client_obj(&snap().clients[0], &snap()), &fixture[0], "client");
    }

    #[test]
    fn key_parity_workspace() {
        let fixture = load("workspaces");
        parity(&workspace_obj((1, "1".into()), &snap()), &fixture[0], "workspace");
    }

    #[test]
    fn key_parity_device() {
        let fixture = load("devices");
        let mine = devices_json(&snap());
        for k in fixture.as_object().unwrap().keys() {
            assert!(mine.get(k).is_some(), "device key missing: {k}");
        }
        // keyboards list fields must match fixture
        let fkb = &fixture["keyboards"][0];
        let mkb = &mine["keyboards"][0];
        for k in fkb.as_object().unwrap().keys() {
            assert!(mkb.get(k).is_some(), "keyboard key missing: {k}");
        }
    }

    #[test]
    fn activewindow_soft_empty() {
        // cliphist relies on empty jq output when nothing is focused
        let s = Snapshot { ..snap() };
        let s = Snapshot { focused_address: None, clients: vec![], ..s };
        assert_eq!(handle_request("j/activewindow", &Arc::new(Mutex::new(s)), &tx()), "{}\n".trim_end());
    }

    fn tx() -> flume::Sender<CompCmd> {
        flume::unbounded().0
    }

    #[test]
    fn version_shape() {
        let fixture = load("version");
        let mine = version_json();
        for k in fixture.as_object().unwrap().keys() {
            assert!(mine.get(k).is_some(), "version key missing: {k}");
        }
    }

    #[test]
    fn parse_flags() {
        // j prefix -> -j; no j prefix -> plain text mode
        let s = Arc::new(Mutex::new(snap()));
        assert!(handle_request("j/workspaces", &s, &tx()).starts_with('['));
        assert_eq!(handle_request("/unknown", &s, &tx()), "invalid request");
    }
}
