// Native control channel v0: spawn the nested compositor with an isolated
// runtime dir and speak the documented JSON protocol to its socket.
use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use serde_json::{json, Value};

fn xdg_dir() -> String {
    let d = std::env::temp_dir().join(format!("eswm-control-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d.to_string_lossy().into_owned()
}

fn expose_parent_wayland_socket(xdg: &str) {
    let display = std::env::var("WAYLAND_DISPLAY").expect("control test needs WAYLAND_DISPLAY");
    let source = std::path::Path::new(&std::env::var("XDG_RUNTIME_DIR").unwrap()).join(display);
    let target = std::path::Path::new(xdg).join(source.file_name().unwrap());
    std::os::unix::fs::symlink(source, target).unwrap();
}

#[test]
fn control_socket_speaks_json() {
    let xdg = xdg_dir();
    expose_parent_wayland_socket(&xdg);
    let socket = format!("eswm-ctl-{}", std::process::id());
    let mut child = Command::new(env!("CARGO_BIN_EXE_eswm"))
        .env("XDG_RUNTIME_DIR", &xdg)
        .env("TACIT_COMP_SOCKET", &socket)
        .env("TACIT_COMP_BACKEND", "winit")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let path = std::path::Path::new(&xdg).join(format!("eswm-{socket}.sock"));
    let deadline = Instant::now() + Duration::from_secs(15);
    while !path.exists() {
        assert!(Instant::now() < deadline, "control socket did not appear in 15s");
        std::thread::sleep(Duration::from_millis(100));
    }

    let stream = UnixStream::connect(&path).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut writer = stream.try_clone().unwrap();
    let mut reader = BufReader::new(stream);
    let mut ask = |request: Value| -> Value {
        writeln!(writer, "{request}").unwrap();
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    };

    let state = ask(json!({"id": 1, "cmd": "state"}));
    assert_eq!(state["id"], 1);
    assert_eq!(state["ok"]["outputs"].as_array().unwrap().len(), 1);
    assert_eq!(state["ok"]["workspaces"].as_array().unwrap().len(), 1);
    assert!(state["ok"]["windows"].as_array().unwrap().is_empty());

    let unknown = ask(json!({"id": 2, "cmd": "no.such.command"}));
    assert_eq!(unknown["id"], 2);
    assert!(unknown["ok"].is_null());
    assert!(unknown["error"].as_str().unwrap().contains("unknown command"));

    let focus = ask(json!({"id": 3, "cmd": "window.focus", "args": {"id": 99}}));
    assert!(focus["error"].as_str().unwrap().contains("no window 99"));

    child.kill().ok();
    child.wait().ok();
}
