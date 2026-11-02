// M2 integration test: spawn the compositor with an isolated XDG_RUNTIME_DIR,
// then probe .socket.sock with the real hyprctl binary (the compat contract).
// Requires hyprctl on PATH (dev machines have it: the whole point).
use std::{
    process::{Command, Stdio},
    time::{Duration, Instant},
};

const SIGNATURE: &str = "eswm-m2-test";

fn xdg_dir() -> String {
    let d = std::env::temp_dir().join(format!("eswm-m2-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d.to_string_lossy().into_owned()
}

fn expose_parent_wayland_socket(xdg: &str) {
    let display = std::env::var("WAYLAND_DISPLAY").expect("nested IPC test needs WAYLAND_DISPLAY");
    let source = std::path::Path::new(&std::env::var("XDG_RUNTIME_DIR").unwrap()).join(display);
    let target = std::path::Path::new(xdg).join(source.file_name().unwrap());
    std::os::unix::fs::symlink(source, target).unwrap();
}

#[test]
fn hyprctl_speaks_to_our_ipc() {
    let xdg = xdg_dir();
    // The compositor runs nested through winit. Its isolated IPC runtime must
    // still expose the host's Wayland socket or winit exits after binding IPC.
    expose_parent_wayland_socket(&xdg);
    let mut child = Command::new(env!("CARGO_BIN_EXE_eswm"))
        .env("XDG_RUNTIME_DIR", &xdg)
        .env("TACIT_COMP_SIG", SIGNATURE)
        .env("TACIT_COMP_SOCKET", format!("wayland-m2-{}", std::process::id()))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let base = std::path::PathBuf::from(&xdg).join("hypr").join(SIGNATURE);
    let deadline = Instant::now() + Duration::from_secs(15);
    while !(base.join(".socket.sock").exists() && base.join(".socket2.sock").exists()) {
        assert!(Instant::now() < deadline, "compositor did not expose hyprctl sockets in 15s");
        std::thread::sleep(Duration::from_millis(100));
    }

    // hyprctl finds us through the env signature (the whole contract);
    // unknown commands must fail soft: text reply, hyprctl exits 0
    let probe = |args: &[&str]| -> (String, bool) {
        let out = Command::new("hyprctl")
            .args(args)
            .env("XDG_RUNTIME_DIR", &xdg)
            .env("HYPRLAND_INSTANCE_SIGNATURE", SIGNATURE)
            .stdout(Stdio::piped())
            .output()
            .unwrap();
        (String::from_utf8_lossy(&out.stdout).to_string(), out.status.success())
    };

    let (out, ok) = probe(&["-j", "monitors"]);
    let monitors = serde_json::from_str::<serde_json::Value>(&out)
        .expect("hyprctl -j monitors against ours must be valid JSON");
    assert!(ok && monitors.is_array());

    let (out, ok) = probe(&["-j", "cursorpos"]);
    assert!(ok && out.trim() == "0,0");

    let (out, ok) = probe(&["reload"]);
    assert!(ok && out.trim() == "ok");

    let (out, ok) = probe(&["notacommand"]);
    assert!(ok && out.trim() == "invalid request");

    child.kill().ok();
}
