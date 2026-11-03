// M1 test: comp binary starts, advertises the protocol floor, and a client roundtrips.
use std::os::unix::net::UnixStream;
use wayland_client::{
    Connection, Dispatch, globals::{GlobalListContents, registry_queue_init},
    protocol::wl_registry, QueueHandle,
};

struct Listing;
impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Listing {
    fn event(
        _state: &mut Self,
        _registry: &wl_registry::WlRegistry,
        _event: wl_registry::Event,
        _data: &GlobalListContents,
        _conn: &Connection,
        _qh: &QueueHandle<Listing>,
    ) {
    }
}

const REQUIRED: [&str; 5] = ["wl_compositor", "wl_seat", "wl_shm", "xdg_wm_base", "zwlr_layer_shell_v1"];

#[test]
fn compositor_advertises_shell_globals() {
    let socket = "eswm-test";
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_eswm"))
        .env("TACIT_COMP_SOCKET", socket)
        .env("TACIT_COMP_BACKEND", "winit")
        // The winit backend is nested here: this test needs a graphical session
        // (the dev's desktop). CI would need headless; not until that exists.
        .spawn()
        .expect("start compositor");

    let runtime = std::env::var("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR");
    let path = format!("{runtime}/{socket}");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    let conn = loop {
        match UnixStream::connect(&path) {
            Ok(stream) => break Connection::from_socket(stream).expect("handshake"),
            Err(_) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(200))
            }
            Err(e) => {
                let _ = child.kill();
                panic!("socket {path} never appeared: {e}");
            }
        }
    };

    let (globals, mut queue) = registry_queue_init::<Listing>(&conn).expect("registry init");
    let names: Vec<String> = globals.contents().clone_list()
        .iter().map(|g| g.interface.clone()).collect();

    for required in REQUIRED {
        assert!(names.contains(&required.to_string()), "missing global {required} in {names:?}");
    }

    queue.roundtrip(&mut Listing).expect("roundtrip");
    child.kill().ok();
    child.wait().ok();
}
