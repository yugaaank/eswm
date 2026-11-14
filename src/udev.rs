// Real DRM/libinput backend (M4 Option A), ported from Smithay 0.7's anvil udev.rs.
// Simplified for M4: single seat, no drm lease/syncobj, no tablet/touch/gestures,
// windows render full-window stacked at (0,0) on one output (layout = M5).
use std::{
    collections::HashMap,
    io,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

pub use smithay::utils::SERIAL_COUNTER as SCOUNTER;
use smithay::desktop::{layer_map_for_output, LayerMap, WindowSurfaceType};
use smithay::utils::IsAlive;
use smithay::wayland::shell::wlr_layer::Layer as WlrLayer;
use smithay::{
    backend::{
        allocator::{
            dmabuf::Dmabuf,
            format::FormatSet,
            gbm::{GbmAllocator, GbmBufferFlags, GbmDevice},
            Fourcc, Modifier,
        },
        drm::{
            compositor::FrameFlags,
            exporter::gbm::GbmFramebufferExporter,
            output::{DrmOutput, DrmOutputManager, DrmOutputRenderElements},
            CreateDrmNodeError, DrmAccessError, DrmDevice, DrmDeviceFd, DrmError, DrmEvent,
            DrmEventMetadata, DrmEventTime, DrmNode, NodeType,
        },
        egl::{self, context::ContextPriority, EGLDevice, EGLDisplay},
        input::{
            self, AbsolutePositionEvent as _, Event as _, InputBackend, InputEvent, KeyboardKeyEvent, PointerAxisEvent,
            PointerButtonEvent, PointerMotionAbsoluteEvent, PointerMotionEvent,
        },
        libinput::{LibinputInputBackend, LibinputSessionInterface},
        renderer::{
            damage::{Error as OutputDamageTrackerError, OutputDamageTracker},
            element::{
                memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
                surface::{render_elements_from_surface_tree, WaylandSurfaceRenderElement},
                Kind, RenderElementStates, render_elements,
            },
            gles::GlesRenderer,
            multigpu::{gbm::GbmGlesBackend, GpuManager, MultiRenderer},
            ImportAll, ImportDma, ImportEgl, ImportMem, Renderer,
        },
        session::{libseat::LibSeatSession, Event as SessionEvent, Session},
        udev::{all_gpus, primary_gpu, UdevBackend, UdevEvent},
        SwapBuffersError,
    },
    delegate_dmabuf,
    input::{
        keyboard::{FilterResult, KeysymHandle, ModifiersState},
        pointer::{AxisFrame, ButtonEvent, MotionEvent},
    },
    output::{Mode as WlMode, Output, PhysicalProperties},
    reexports::{
        calloop::{
            timer::{TimeoutAction, Timer},
            EventLoop, RegistrationToken,
        },
        drm::control::{connector, crtc, Device, ModeTypeFlags},
        input::{DeviceCapability, Libinput},
        rustix::fs::OFlags,
        wayland_protocols::wp::presentation_time::server::wp_presentation_feedback,
        wayland_server::{backend::GlobalId, protocol::wl_surface, Display, DisplayHandle, ListeningSocket},
    },
    utils::{
        DeviceFd, Logical, Physical, Point, Rectangle, Scale, Size, Time, Transform,
    },    wayland::{
        dmabuf::{DmabufFeedbackBuilder, DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier},
        presentation::Refresh,
    },
};
use tracing::{debug, error, info, warn};


use smithay_drm_extras::drm_scanner::{DrmScanEvent, DrmScanner};

use crate::{send_frames_surface_tree, State, wm, ipc, lua_state};

type UdevRenderer<'a> = MultiRenderer<
    'a,
    'a,
    GbmGlesBackend<GlesRenderer, DrmDeviceFd>,
    GbmGlesBackend<GlesRenderer, DrmDeviceFd>,
>;

render_elements! {
    pub EswmRenderElements<R> where
        R: Renderer + ImportAll + ImportMem;
    Cursor = MemoryRenderBufferRenderElement<R>,
    Surface = WaylandSurfaceRenderElement<R>,
}

// Widely supported gbm formats; 8-bit only unless 10-bit is explicitly wanted.
const SUPPORTED_FORMATS: &[Fourcc] = &[
    Fourcc::Abgr2101010,
    Fourcc::Argb2101010,
    Fourcc::Abgr8888,
    Fourcc::Argb8888,
];
const SUPPORTED_FORMATS_8BIT_ONLY: &[Fourcc] = &[Fourcc::Abgr8888, Fourcc::Argb8888];

pub type OutputPresentationFeedback = smithay::desktop::utils::OutputPresentationFeedback;

#[derive(Debug, PartialEq)]
struct UdevOutputId {
    device_id: DrmNode,
    crtc: crtc::Handle,
}

#[derive(Clone, Copy)]
enum DragMode {
    Move,
    Resize,
}

#[derive(Clone, Copy)]
struct DragState {
    window: wm::WindowId,
    mode: DragMode,
    start: Point<f64, Logical>,
    orig_rect: wm::Rect,
    button: u32,
}

pub struct UdevData {
    pub session: LibSeatSession,
    pub primary_gpu: DrmNode,
    pub dmabuf_primary_gpu: DrmNode, // first GPU with hw accel; clients allocate here
    pub gpu_nodes: Vec<DrmNode>,
    pub gpus: GpuManager<GbmGlesBackend<GlesRenderer, DrmDeviceFd>>,
    pub backends: HashMap<DrmNode, BackendData>,
    pub dmabuf_state: Option<(DmabufState, DmabufGlobal)>,
    cursor_images: Vec<xcursor::parser::Image>,
    cursor_buffer: Option<(MemoryRenderBuffer, (f64, f64))>, // (buffer, hotspot)
    border_focused: Option<MemoryRenderBuffer>,
    border_unfocused: Option<MemoryRenderBuffer>,
    pub pointer_location: Point<f64, Logical>,
    /// Modifiers seen on the last key event (used to detect Super for drags)
    last_mods: ModifiersState,
    /// Window currently under the pointer (active workspace, hit-tested)
    pointer_focus: Option<wm::WindowId>,
    /// Window currently holding wl_keyboard focus
    keyboard_focus: Option<wm::WindowId>,
    /// In-flight move/resize gesture started with Super+button
    drag: Option<DragState>,
    start: Instant,
    pub quit: AtomicBool,
    pub paused: bool,
}

impl DmabufHandler for State {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.udev.as_mut().unwrap().dmabuf_state.as_mut().unwrap().0
    }

    fn dmabuf_imported(&mut self, _global: &DmabufGlobal, dmabuf: Dmabuf, notifier: ImportNotifier) {
        let udev = self.udev.as_mut().unwrap();
        if udev
            .gpus
            .single_renderer(&udev.dmabuf_primary_gpu)
            .and_then(|mut renderer| renderer.import_dmabuf(&dmabuf, None))
            .is_ok()
        {
            dmabuf.set_node(udev.dmabuf_primary_gpu);
            let _ = notifier.successful::<State>();
        } else {
            notifier.failed();
        }
    }
}
delegate_dmabuf!(State);

fn load_cursor_images() -> Vec<xcursor::parser::Image> {
    let name = std::env::var("XCURSOR_THEME").ok().unwrap_or_else(|| "default".into());
    match xcursor::CursorTheme::load(&name)
        .load_icon("default")
        .map(std::fs::read)
        .and_then(|r| r.ok().and_then(|d| xcursor::parser::parse_xcursor(&d)))
    {
        Some(icons) if !icons.is_empty() => icons,
        _ => {
            warn!("no xcursor theme found, cursor will be invisible");
            Vec::new()
        }
    }
}

pub fn run_udev() -> Result<(), Box<dyn std::error::Error>> {
    let mut event_loop = EventLoop::<'static, State>::try_new()?;
    let handle = event_loop.handle();
    let mut display: Display<State> = Display::new()?;
    let mut display_handle = display.handle();

    // Grab the seat before any DRM access (libseat/logind).
    let (session, notifier) = match LibSeatSession::new() {
        Ok(ret) => ret,
        Err(err) => return Err(format!("could not initialize a session: {err}").into()),
    };    let seat_name = session.seat();

    let primary_gpu = if let Ok(var) = std::env::var("TACIT_DRM_DEVICE") {
        DrmNode::from_path(&var).map_err(|e| format!("invalid drm device path {var}: {e:?}"))?
    } else {
        primary_gpu(&seat_name)
            .unwrap()
            .and_then(|x| DrmNode::from_path(&x).ok()?.node_with_type(NodeType::Render)?.ok())
            .unwrap_or_else(|| {
                all_gpus(&seat_name)
                    .unwrap()
                    .into_iter()
                    .find_map(|x| DrmNode::from_path(&x).ok())
                    .expect("no usable GPU found")
            })
    };
    // Collect all GPU nodes for fallback hardware acceleration
    let gpu_nodes = if let Ok(var) = std::env::var("TACIT_DRM_DEVICE") {
        vec![primary_gpu]
    } else {
        let mut nodes = all_gpus(&seat_name)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|x| DrmNode::from_path(&x).ok())
            .collect::<Vec<_>>();
        // Also add the render node of the primary GPU if different
        if let Some(render) = primary_gpu.node_with_type(NodeType::Render) {
            if let Ok(r) = render {
                if !nodes.iter().any(|n| n.dev_id() == r.dev_id() && n.ty() == r.ty()) {
                    nodes.push(r);
                }
            }
        }
        nodes
    };
    info!("Using {primary_gpu} as primary gpu; all GPUs: {gpu_nodes:?}");

    let cursor_images = load_cursor_images();
    let state_u = UdevData {
        session,
        primary_gpu,
        dmabuf_primary_gpu: primary_gpu, // will be updated after hw accel detection
        gpu_nodes,
        gpus: GpuManager::new(GbmGlesBackend::with_context_priority(ContextPriority::High))?,
        backends: HashMap::new(),
        dmabuf_state: None,
        cursor_images,
        cursor_buffer: None,
        border_focused: None,
        border_unfocused: None,
        pointer_location: (0.0, 0.0).into(),
        last_mods: ModifiersState::default(),
        pointer_focus: None,
        keyboard_focus: None,
        drag: None,
        start: Instant::now(),
        quit: AtomicBool::new(false),
        paused: false,
    };

    // Shared protocol/global/socket setup from main.rs.
    let mut state = crate::new_state(&seat_name, &display_handle, false);
    state.udev = Some(state_u);
    state.handle = Some(handle.clone());

    state.seat.add_pointer();
    state.seat.add_keyboard(Default::default(), 200, 200).unwrap();

    /*
     * udev backend
     */
    let udev_backend = UdevBackend::new(&seat_name)?;

    /*
     * libinput backend
     */
    let mut libinput_context = Libinput::new_with_udev::<LibinputSessionInterface<LibSeatSession>>(
        state.udev.as_ref().unwrap().session.clone().into(),
    );
    libinput_context.udev_assign_seat(&seat_name).expect("failed to assign seat");
    let libinput_backend = LibinputInputBackend::new(libinput_context.clone());

    event_loop
        .handle()
        .insert_source(libinput_backend, move |mut event, _, data: &mut State| {
            if let InputEvent::DeviceAdded { device } = &mut event {
                if device.has_capability(DeviceCapability::Keyboard) {
                    if let Some(led_state) = data.seat.get_keyboard().map(|key| key.led_state()) {
                        device.led_update(led_state.into());
                    }
                }
                // Touchpad usability: libinput's defaults leave tap-to-click,
                // tap-drag and disable-while-typing off, and the cursor drifts
                // at low accel. Configure touchpads here (ref: wm-wayland
                // applies its config as each device appears, so hotplug works).
                if device.config_tap_finger_count() > 0 {
                    use smithay::reexports::input::ScrollMethod;
                    device.config_tap_set_enabled(true);
                    device.config_tap_set_drag_enabled(true);
                    device.config_tap_set_drag_lock_enabled(true);
                    device.config_dwt_set_enabled(true);
                    device.config_scroll_set_method(ScrollMethod::TwoFinger);
                    device.config_accel_set_speed(0.7);
                    info!("touchpad configured: {} (tap, dwt, two-finger scroll)", device.name());
                }
            }
            data.process_input_event(&event);
        })
        .unwrap();

    event_loop
        .handle()
        .insert_source(notifier, move |event, &mut (), data: &mut State| match event {
            SessionEvent::PauseSession => {
                libinput_context.suspend();
                info!("pausing session");
                data.udev.as_mut().unwrap().paused = true;
                for backend in data.udev.as_mut().unwrap().backends.values_mut() {
                    backend.drm_output_manager.pause();
                }
            }
            SessionEvent::ActivateSession => {
                info!("resuming session");
                data.udev.as_mut().unwrap().paused = false;
                if let Err(err) = libinput_context.resume() {
                    error!("failed to resume libinput context: {err:?}");
                }
                for (node, backend) in data
                    .udev
                    .as_mut()
                    .unwrap()
                    .backends
                    .iter_mut()
                    .map(|(node, backend)| (*node, backend))
                {
                    // assume connector state survived; reset when queuing a frame fails
                    if let Err(err) = backend.drm_output_manager.activate(false) {
                        error!("failed to activate drm backend: {err:?}");
                    }
                    let handle = data.handle.clone().unwrap();
                    handle.insert_idle(move |data: &mut State| {
                        let now_ms = now(data);
                        data.render(node, None, now_ms);
                    });
                }
            }
        })
        .unwrap();

    // Initialize the primary node first so display-only nodes can fall back to it.
    let primary_node = primary_gpu.node_with_type(NodeType::Primary).and_then(|node| node.ok());
    let primary_device = udev_backend.device_list().find(|(device_id, _)| {
        primary_node
            .map(|primary_node| *device_id == primary_node.dev_id())
            .unwrap_or(false)
            || *device_id == primary_gpu.dev_id()
    });

    if let Some((device_id, path)) = primary_device {
        let node = DrmNode::from_dev_id(device_id).expect("failed to get primary node");
        state
            .device_added(node, path)
            .expect("failed to initialize primary node");
    }

    let primary_device_id = primary_device.map(|(device_id, _)| device_id);
    for (device_id, path) in udev_backend.device_list() {
        if Some(device_id) == primary_device_id {
            continue;
        }
        let node = match DrmNode::from_dev_id(device_id) {
            Ok(node) => node,
            Err(err) => {
                warn!("skipping device {device_id}: {err:?}");
                continue;
            }
        };
        if let Err(err) = state.device_added(node, &path) {
            warn!("skipping device {device_id}: {err:?}");
        }
    }

    // Now all devices are added and their render nodes are registered in gpus.
    // Try hardware acceleration on each GPU that has an output (backend).
    let udev = state.udev.as_mut().unwrap();
    
    // Collect unique GPUs from backends (both render and primary nodes for multi-GPU import)
    let mut candidate_nodes: Vec<DrmNode> = udev
        .backends
        .values()
        .flat_map(|b| {
            let mut nodes = Vec::new();
            if let Some(rn) = b.render_node {
                nodes.push(rn);
            }
            // Also try primary node (for NVIDIA which doesn't expose extension on render node)
            if let Ok(primary) = DrmNode::from_dev_id(b.drm_output_manager.device().device_id()) {
                nodes.push(primary);
            }
            nodes
        })
        .collect();
    // Also add primary_gpu in case no backend has nodes yet
    candidate_nodes.push(primary_gpu);
    candidate_nodes.sort_by_key(|n| (n.dev_id(), n.ty() as u32));
    candidate_nodes.dedup_by(|a, b| a.dev_id() == b.dev_id() && a.ty() == b.ty());
    
    info!("trying hardware acceleration on GPUs: {:?}", candidate_nodes);
    
    // Enable hw acceleration on ALL GPUs for multi-GPU dmabuf import
    let mut hw_accel_gpus = Vec::new();
    for node in &candidate_nodes {
        info!("trying GPU {} for hardware acceleration", node);
        let r_opt = udev.gpus.single_renderer(node).ok();
        if let Some(mut r) = r_opt {
            let result = r.bind_wl_display(&display_handle);
            drop(r);
            match result {
                Ok(_) => {
                    info!("egl hardware-acceleration enabled on {}", node);
                    hw_accel_gpus.push(*node);
                }
                Err(e) => info!(?e, "GPU {} failed", node),
            }
        }
    }
    
    // Use the first GPU with hw acceleration as primary for dmabuf feedback
    let primary_for_dmabuf = hw_accel_gpus.first().copied().unwrap_or(primary_gpu);
    // Store for later use in render_surface and dmabuf_imported
    udev.dmabuf_primary_gpu = primary_for_dmabuf;
    
    // Create renderer for the primary GPU (used for dmabuf formats)
    let renderer = udev.gpus.single_renderer(&primary_for_dmabuf).unwrap();

    // init dmabuf support with format list from the first GPU with hw acceleration
    let dmabuf_formats = renderer.dmabuf_formats();
    let default_feedback =
        DmabufFeedbackBuilder::new(primary_for_dmabuf.dev_id(), dmabuf_formats).build()?;
    let mut dmabuf_state = DmabufState::new();
    let global = dmabuf_state.create_global_with_default_feedback::<State>(&display_handle, &default_feedback);
    state.udev.as_mut().unwrap().dmabuf_state = Some((dmabuf_state, global));

    event_loop
        .handle()
        .insert_source(udev_backend, move |event, _, data: &mut State| match event {
            UdevEvent::Added { device_id, path } => {
                let node = match DrmNode::from_dev_id(device_id) {
                    Ok(node) => node,
                    Err(err) => {
                        warn!("skipping device {device_id}: {err:?}");
                        return;
                    }
                };
                if let Err(err) = data.device_added(node, &path) {
                    warn!("skipping device {device_id}: {err:?}");
                }
            }
            UdevEvent::Changed { device_id } => {
                if let Ok(node) = DrmNode::from_dev_id(device_id) {
                    data.device_changed(node);
                }
            }
            UdevEvent::Removed { device_id } => {
                if let Ok(node) = DrmNode::from_dev_id(device_id) {
                    data.device_removed(node);
                }
            }
        })
        .unwrap();

    let listener = ListeningSocket::bind(&crate::socket_name())?;
    println!("listening on {:#?}", listener.socket_name());

    match crate::control::Control::start(&crate::socket_name()) {
        Ok(control) => state.control = Some(control),
        Err(err) => tracing::warn!("control socket disabled: {err}"),
    }

// IPC command channel (hyprctl compat). No Lua here: the DRM path does not
// initialize the historical Hyprland/Tacit compatibility path.
let sig = std::env::var("TACIT_COMP_SIG").unwrap_or_else(|_| "eswm".into());
state.ipc = Some(ipc::Ipc::new(sig));

// Lua config API (es.dsp.exec_cmd, es.bind, etc.)
let lua = crate::lua_state::init().unwrap_or_else(|_| mlua::Lua::new());
let snapshot = state.ipc.as_ref().unwrap().snapshot.clone();
let cmd_tx = state.ipc.as_ref().unwrap().cmd_tx.clone();
if let Err(e) = crate::lua_state::register_api(
    &lua,
    snapshot,
    &cmd_tx,
    state.lua_keybinds.clone(),
    state.decoration.clone(),
    state.dock_entries.clone(),
) {
    tracing::warn!("Lua API registration failed: {e}");
}
state.lua = Some(lua);

// Load Lua config file (~/.config/eswm/config.lua) if it exists.
let lua_config_path = std::env::var("XDG_CONFIG_HOME")
    .map(PathBuf::from)
    .unwrap_or_else(|_| {
        std::env::var("HOME")
            .map(|h| PathBuf::from(h).join(".config"))
            .unwrap_or_default()
    })
    .join("eswm")
    .join("config.lua");

if lua_config_path.exists() {
    if let Some(lua) = &state.lua {
        let snapshot = state.ipc.as_ref().unwrap().snapshot.clone();
        let cmd_tx = state.ipc.as_ref().unwrap().cmd_tx.clone();
        if let Err(e) = crate::lua_state::_do_file(lua, lua_config_path.to_str().unwrap(), snapshot, &cmd_tx) {
            tracing::warn!("Failed to load Lua config {}: {e}", lua_config_path.display());
        }
    }
}

    let mut clients = Vec::new();

    while !state.udev.as_ref().unwrap().quit.load(Ordering::SeqCst) {
        if let Some(stream) = listener.accept()? {
            let client = display
                .handle()
                .insert_client(stream, Arc::new(crate::ClientState::default()))?;
            clients.push(client);
        }

        crate::control::drain(&mut state);
// Drain IPC commands (hyprctl compat)
let cmds: Vec<crate::ipc::CompCmd> = if let Some(ipc) = &state.ipc {
    let mut cmds = Vec::new();
    while let Ok(cmd) = ipc.cmd_rx.try_recv() {
        cmds.push(cmd);
    }
    cmds
} else {
    Vec::new()
};
let mut ipc_opt = state.ipc.take();
if let Some(ref ipc) = ipc_opt {
    for cmd in cmds {
        ipc::apply_cmd(&mut state, ipc, cmd);
    }
}
state.ipc = ipc_opt;

        match event_loop.dispatch(Some(Duration::from_millis(16)), &mut state) {
            Ok(()) => {
                display.dispatch_clients(&mut state)?;
                display_handle.flush_clients()?;
                // Render every iteration like the winit path: smithay only reports
                // non-empty damage when elements exist, so commit-triggered redraws
                // need this prime mover until proper damage-driven scheduling exists.
                let nodes: Vec<DrmNode> = state
                    .udev
                    .as_ref()
                    .unwrap()
                    .backends
                    .keys()
                    .copied()
                    .collect();
                let t = now(&state);
                for node in nodes {
                    state.render(node, None, t);
                }
            }
            Err(_) => break,
        }
    }
    Ok(())
}

fn now(state: &State) -> u32 {
    state.udev.as_ref().unwrap().start.elapsed().as_millis() as u32
}

// Parse a list of modifier strings ("logo", "ctrl", "alt", "shift") into a ModifiersState.
fn parse_mods(mods: &[String]) -> ModifiersState {
    let mut m = ModifiersState::default();
    for mod_str in mods {
        match mod_str.to_lowercase().as_str() {
            "logo" => m.logo = true,
            "ctrl" => m.ctrl = true,
            "alt" => m.alt = true,
            "shift" => m.shift = true,
            _ => {}
        }
    }
    m
}

/// Map a key name string to its xkb keysym u32 value.
fn parse_key(name: &str) -> Option<u32> {
    use xkbcommon::xkb::keysyms;
    let name = name.to_lowercase();
    match name.as_str() {
        "return" | "enter" => Some(keysyms::KEY_Return as u32),
        "q" => Some(keysyms::KEY_q as u32),
        "f" => Some(keysyms::KEY_f as u32),
        "m" => Some(keysyms::KEY_m as u32),
        "space" => Some(keysyms::KEY_space as u32),
        "backspace" => Some(keysyms::KEY_BackSpace as u32),
        "left" => Some(keysyms::KEY_Left as u32),
        "right" => Some(keysyms::KEY_Right as u32),
        "up" => Some(keysyms::KEY_Up as u32),
        "down" => Some(keysyms::KEY_Down as u32),
        "1" => Some(keysyms::KEY_1 as u32),
        "2" => Some(keysyms::KEY_2 as u32),
        "3" => Some(keysyms::KEY_3 as u32),
        "4" => Some(keysyms::KEY_4 as u32),
        "5" => Some(keysyms::KEY_5 as u32),
        "6" => Some(keysyms::KEY_6 as u32),
        "7" => Some(keysyms::KEY_7 as u32),
        "8" => Some(keysyms::KEY_8 as u32),
        "9" => Some(keysyms::KEY_9 as u32),
        "vt1" => Some(keysyms::KEY_XF86Switch_VT_1 as u32),
        "vt2" => Some(keysyms::KEY_XF86Switch_VT_2 as u32),
        "vt3" => Some(keysyms::KEY_XF86Switch_VT_3 as u32),
        "vt4" => Some(keysyms::KEY_XF86Switch_VT_4 as u32),
        "vt5" => Some(keysyms::KEY_XF86Switch_VT_5 as u32),
        "vt6" => Some(keysyms::KEY_XF86Switch_VT_6 as u32),
        "vt7" => Some(keysyms::KEY_XF86Switch_VT_7 as u32),
        "vt8" => Some(keysyms::KEY_XF86Switch_VT_8 as u32),
        "vt9" => Some(keysyms::KEY_XF86Switch_VT_9 as u32),
        "vt10" => Some(keysyms::KEY_XF86Switch_VT_10 as u32),
        "vt11" => Some(keysyms::KEY_XF86Switch_VT_11 as u32),
        "vt12" => Some(keysyms::KEY_XF86Switch_VT_12 as u32),
        _ => None,
    }
}

/// Return the first Lua keybind whose mods and key match the current event.
fn find_lua_keybind<'a>(
    keybinds: &'a [lua_state::Keybind],
    mods: &ModifiersState,
    raw: u32,
) -> Option<&'a lua_state::Keybind> {
    keybinds.iter().find(|kb| {
        let kb_mods = parse_mods(&kb.mods);
        let mods_match = kb_mods.logo == mods.logo
            && kb_mods.ctrl == mods.ctrl
            && kb_mods.alt == mods.alt
            && kb_mods.shift == mods.shift;
        let key_match = parse_key(&kb.key).map_or(false, |k| k == raw);
        mods_match && key_match
    })
}

/// Execute a single keybind action.
fn execute_keybind(state: &mut State, kb: &lua_state::Keybind) -> FilterResult<()> {
    match kb.action.as_str() {
        "spawn_terminal" => crate::spawn_terminal(),
        "spawn_fuzzel" => crate::spawn_on_socket("fuzzel", &[]),
        "close_window" => {
            if let Some(id) = state.wm.focused_id() {
                if let Some(toplevel) = crate::toplevel_for_window(state, id) {
                    toplevel.send_close();
                }
            }
        }
        "switch_workspace" => {
            if let Some(ws) = kb.arg {
                switch_workspace(state, ws);
            }
        }
        "move_window_to_workspace" => {
            if let Some(ws) = kb.arg {
                if let Some(id) = state.wm.focused_id() {
                    state.wm.move_window_to_workspace(id, ws);
                    crate::apply_layout(state);
                }
            }
        }
        "cycle_focus_prev" => {
            if state.wm.cycle_focus(-1) {
                crate::apply_layout(state);
            }
        }
        "cycle_focus_next" => {
            if state.wm.cycle_focus(1) {
                crate::apply_layout(state);
            }
        }
        "swap_focused_prev" => {
            if state.wm.swap_focused(-1) {
                crate::apply_layout(state);
            }
        }
        "swap_focused_next" => {
            if state.wm.swap_focused(1) {
                crate::apply_layout(state);
            }
        }
        "focus_workspace_prev" => {
            if state.wm.focus_workspace_relative(-1).is_some() {
                crate::apply_layout(state);
            }
        }
        "focus_workspace_next" => {
            if state.wm.focus_workspace_relative(1).is_some() {
                crate::apply_layout(state);
            }
        }
        "toggle_fullscreen" => {
            if let Some(id) = state.wm.focused_id() {
                state.wm.toggle_fullscreen(id);
                crate::apply_layout(state);
            }
        }
        "toggle_maximized" => {
            if let Some(id) = state.wm.focused_id() {
                state.wm.toggle_maximized(id);
                crate::apply_layout(state);
            }
        }
        "toggle_minimize" => {
            if let Some(id) = state.wm.focused_id() {
                state.wm.toggle_minimize(id);
                crate::apply_layout(state);
            }
        }
        "toggle_window_mode" => {
            if let Some(id) = state.wm.focused_id() {
                state.wm.toggle_window_mode(id);
            }
            crate::apply_layout(state);
        }
        "toggle_layout_mode" => {
            state.wm.toggle_layout_mode();
            crate::apply_layout(state);
        }
        "exec_cmd" => {
            if let Some(cmd) = &kb.cmd {
                let (program, args) = crate::parse_cmd(cmd);
                if !program.is_empty() {
                    crate::spawn_on_socket(&program, &args);
                }
            }
        }
        "quit" => {
            info!("quit requested via keybind");
            state
                .udev
                .as_ref()
                .unwrap()
                .quit
                .store(true, Ordering::SeqCst);
        }
        "change_vt" => {
            if let Some(vt) = kb.arg {
                if let Err(err) = state.udev.as_mut().unwrap().session.change_vt(vt) {
                    warn!("failed to switch to vt {vt}: {err:?}");
                }
            }
        }
        _ => {
            warn!("unknown keybind action: {}", kb.action);
            return FilterResult::Forward;
        }
    }
    FilterResult::Intercept(())
}

// All keybinds are driven from Lua es.bind() calls in ~/.config/eswm/config.lua.
// process_shortcut checks Lua keybinds; anything not configured is forwarded to clients.
fn process_shortcut(state: &mut State, mods: &ModifiersState, keysym: KeysymHandle) -> FilterResult<()> {
    crate::prune_dead_windows(state);
    let raw = u32::from(keysym.modified_sym());

    // Lua keybinds (from es.bind() in config.lua).
    let lua_kb = {
        let guard = state.lua_keybinds.lock().unwrap();
        find_lua_keybind(&guard, mods, raw).cloned()
    };
    if let Some(kb) = lua_kb {
        return execute_keybind(state, &kb);
    }

    // No keybind matched — forward to clients.
    FilterResult::Forward
}

// Linux input event codes for mouse buttons
const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;
/// Focus frame width drawn behind every placed window.
const BORDER_PX: i32 = 2;

/// Recompute pointer (hover/drag) focus from `pos` and return the surface-local
/// hit for `Pointer.motion`, or None when over empty space.
fn pointer_focus(state: &mut State, pos: Point<f64, Logical>) -> Option<(wl_surface::WlSurface, Point<f64, Logical>)> {
    crate::prune_dead_windows(state);
    let dragging = state.udev.as_ref().unwrap().drag;
    let id = dragging.map(|drag| drag.window).or_else(|| state.wm.window_at((pos.x, pos.y), state.wm.border_size()));
    if let Some(udev) = state.udev.as_mut() {
        udev.pointer_focus = id;
    }
    let id = id?;
    let toplevel = crate::toplevel_for_window(state, id)?;
    let rect = state.wm.window(id)?.target_rect;
    Some((
        toplevel.wl_surface().clone(),
        // smithay computes surface-local coords as `event.location - loc`,
        // so this must be the window origin, which is where we now render.
        Point::from((rect.x as f64, rect.y as f64)),
    ))
}

/// Send wl_keyboard leave/enter for `id`; None => just leave.
pub fn keyboard_focus_set(state: &mut State, id: Option<wm::WindowId>) {
    let changed = state
        .udev
        .as_ref()
        .map(|udev| udev.keyboard_focus != id)
        .unwrap_or(true);
    let surface = id
        .and_then(|id| crate::toplevel_for_window(state, id))
        .map(|toplevel| toplevel.wl_surface().clone());
    let keyboard = state.seat.get_keyboard().unwrap();
    keyboard.set_focus(state, surface, SCOUNTER.next_serial());
    if let Some(udev) = state.udev.as_mut() {
        udev.keyboard_focus = id;
    }
    if changed {
        tracing::info!("keyboard focus -> {id:?}");
    }
}

/// Advance an in-flight move/resize and return the dragged window id.
fn drag_update(state: &mut State, pos: Point<f64, Logical>) -> Option<wm::WindowId> {
    let drag = state.udev.as_ref().unwrap().drag?;
    let dx = pos.x - drag.start.x;
    let dy = pos.y - drag.start.y;
    let mut rect = match drag.mode {
        DragMode::Move => wm::Rect { x: drag.orig_rect.x + dx as i32, y: drag.orig_rect.y + dy as i32, ..drag.orig_rect },
        DragMode::Resize => wm::Rect {
            width: (drag.orig_rect.width as f64 + dx).max(200.0) as u32,
            height: (drag.orig_rect.height as f64 + dy).max(150.0) as u32,
            ..drag.orig_rect
        },
    };
    // Keep the window on its output: an unbounded drag could wrap the i32
    // geometry used by hit-testing and leave the window unclickable.
    if let Some(geometry) = state
        .wm
        .window(drag.window)
        .and_then(|window| state.wm.output(window.output))
        .map(|output| output.geometry)
    {
        rect.x = rect
            .x
            .clamp(geometry.x, (geometry.x + geometry.width as i32 - 200).max(geometry.x));
        rect.y = rect
            .y
            .clamp(geometry.y, (geometry.y + geometry.height as i32 - 150).max(geometry.y));
        rect.width = rect.width.clamp(200, geometry.width.max(200));
        rect.height = rect.height.clamp(150, geometry.height.max(150));
    }
    if state.wm.set_window_rect(drag.window, rect) {
        crate::apply_layout(state);
    }
    Some(drag.window)
}

fn switch_workspace(state: &mut State, ws: i32) {
    let Some(output) = state.wm.primary_output_id() else {
        return;
    };
    state.wm.ensure_workspace(output, ws);
    if state.wm.focus_workspace(output, ws) {
        crate::apply_layout(state);
    }
}

impl State {
    fn process_input_event<B: InputBackend>(&mut self, event: &InputEvent<B>) {
        match event {
            InputEvent::Keyboard { event, .. } => {
                let keyboard = self.seat.get_keyboard().unwrap();
                // libinput/smithay call the filter for press *and* release;
                // shortcuts must only fire on press.
                let pressed = event.state() == input::KeyState::Pressed;
                keyboard.input::<(), _>(
                    self,
                    event.key_code(),
                    event.state(),
                    SCOUNTER.next_serial(),
                    event.time_msec(),
                    |state, mods, keysym| {
                        if let Some(udev) = state.udev.as_mut() {
                            udev.last_mods = *mods;
                        }
                        if pressed {
                            process_shortcut(state, mods, keysym)
                        } else {
                            FilterResult::Forward
                        }
                    },
                );
            }
            InputEvent::PointerMotion { event, .. } => {
                let serial = SCOUNTER.next_serial();
                let (max_x, max_y) = self.max_screen_size();
                let moved = self.udev.as_ref().unwrap().pointer_location + event.delta();
                let pointer_location = Point::<f64, Logical>::from((
                    moved.x.clamp(0.0, (max_x.max(1) - 1) as f64),
                    moved.y.clamp(0.0, (max_y.max(1) - 1) as f64),
                ));
                self.udev.as_mut().unwrap().pointer_location = pointer_location;
                drag_update(self, pointer_location);
                // Diagnostics: ~1 Hz pointer position in the log (helps spot a
                // stuck/invisible cursor); drop when focus is stable.
                if event.time_msec() % 1000 < 16 {
                    tracing::debug!(
                        "pointer at ({:.0},{:.0}) focus {:?}",
                        pointer_location.x,
                        pointer_location.y,
                        self.udev.as_ref().unwrap().pointer_focus
                    );
                }
                let focus = pointer_focus(self, pointer_location);
                let pointer = self.seat.get_pointer().unwrap();
                pointer.motion(
                    self,
                    focus,
                    &MotionEvent {
                        location: pointer_location,
                        serial,
                        time: event.time_msec(),
                    },
                );
                pointer.frame(self);
            }
            InputEvent::PointerMotionAbsolute { event, .. } => {
                // shortcut: screen assumed to be one stacked output starting at (0,0)
                let (max_x, max_y) = self.max_screen_size();
                let location =
                    Point::<f64, Logical>::from((event.x_transformed(max_x as i32), event.y_transformed(max_y as i32)));
                let serial = SCOUNTER.next_serial();
                self.udev.as_mut().unwrap().pointer_location = location;
                drag_update(self, location);
                let focus = pointer_focus(self, location);
                let pointer = self.seat.get_pointer().unwrap();
                pointer.motion(
                    self,
                    focus,
                    &MotionEvent {
                        location,
                        serial,
                        time: event.time_msec(),
                    },
                );
                pointer.frame(self);
            }
            InputEvent::PointerButton { event, .. } => {
                let button = event.button_code();
                let button_state =
                    smithay::reexports::wayland_server::protocol::wl_pointer::ButtonState::from(event.state());
                crate::prune_dead_windows(self);
                if button_state
                    == smithay::reexports::wayland_server::protocol::wl_pointer::ButtonState::Pressed
                {
                    let location = self.udev.as_ref().unwrap().pointer_location;
                    // Refresh pointer focus first: the pointer may have been sitting
                    // still since the layout/workspace under it changed.
                    let serial = SCOUNTER.next_serial();
                    let focus = pointer_focus(self, location);
                    let pointer = self.seat.get_pointer().unwrap();
                    pointer.motion(
                        self,
                        focus,
                        &MotionEvent {
                            location,
                            serial,
                            time: event.time_msec(),
                        },
                    );
                    pointer.frame(self);

                    let mods = self.udev.as_ref().unwrap().last_mods;
                    // Super+button starts a move/resize drag on a floating window
                    if mods.logo && (button == BTN_LEFT || button == BTN_RIGHT) {
                        if let Some(id) = self.wm.window_at((location.x, location.y), self.wm.border_size()) {
                            if self.wm.window(id).map(|w| w.mode) == Some(wm::WindowMode::Floating) {
                                let rect = self.wm.window(id).unwrap().target_rect;
                                self.udev.as_mut().unwrap().drag = Some(DragState {
                                    window: id,
                                    mode: if button == BTN_LEFT {
                                        DragMode::Move
                                    } else {
                                        DragMode::Resize
                                    },
                                    start: location,
                                    orig_rect: rect,
                                    button,
                                });
                            }
                        }
                    }
                    // Titlebar button clicks: close / maximize / minimize.
                    // Clicking the titlebar itself (not a button) drags a floating window.
                    let titlebar_height = self.decoration.lock().unwrap().titlebar_height;
                    if titlebar_height > 0 {
                        if let Some(id) = self.wm.window_at((location.x, location.y), self.wm.border_size()) {
                            let window = self.wm.window(id).unwrap();
                            let rect = window.target_rect;
                            let tb = titlebar_height as f64;
                            // Titlebar is drawn above the client surface: y in [rect.y - tb, rect.y)
                            let in_titlebar = location.y >= rect.y as f64 - tb && location.y < rect.y as f64
                                && location.x >= rect.x as f64 && location.x < rect.x as f64 + rect.width as f64;
                            if in_titlebar {
                                let btn_size = (tb - 2.0).max(0.0);
                                let btn_gap = 4.0;
                                // Buttons are right-aligned at rect.x + rect.width.
                                let offset_from_right = (rect.x as f64 + rect.width as f64) - location.x;
                                if offset_from_right < btn_size + btn_gap {
                                    // Close button (rightmost)
                                    self.wm.remove_window(id);
                                } else if offset_from_right < 2.0 * (btn_size + btn_gap) {
                                    // Maximize button
                                    self.wm.toggle_maximized(id);
                                } else if offset_from_right < 3.0 * (btn_size + btn_gap) {
                                    // Minimize button: hide window (with focus handling)
                                    self.wm.toggle_minimize(id);
                                } else if window.mode == wm::WindowMode::Floating {
                                    // Titlebar drag: start move on floating window
                                    let rect = self.wm.window(id).unwrap().target_rect;
                                    self.udev.as_mut().unwrap().drag = Some(DragState {
                                        window: id,
                                        mode: DragMode::Move,
                                        start: location,
                                        orig_rect: rect,
                                        button,
                                    });
                                }
                            }
                        }
                    }
                    // Dock bar click-to-focus / toggle-minimize (bottom strip of primary output).
                    let dec = self.decoration.lock().unwrap();
                    let dock_height = dec.dock_height;
                    let dock = self.primary_output.as_ref().and_then(|o| {
                        let loc = o.current_location();
                        let size = o.current_mode().map(|m| m.size)?;
                        Some((
                            loc.x as f64,
                            loc.y as f64,
                            size.w as f64,
                            size.h as f64,
                        ))
                    });
                    drop(dec);
                    if dock_height > 0 {
                        if let Some((gx, gy, gw, gh)) = dock {
                            if location.y >= gy + gh - dock_height as f64
                                && location.y < gy + gh
                                && location.x >= gx
                                && location.x < gx + gw
                            {
                                let entries = self.wm.dock_entries();
                                let n = entries.len();
                                if n > 0 {
                                    let seg_w = gw / n as f64;
                                    let idx = ((location.x - gx) / seg_w).floor() as usize;
                                    let idx = idx.min(n.saturating_sub(1));
                                    if let Some(entry) = entries.get(idx) {
                                        if button == BTN_LEFT {
                                            // Left click: focus, restoring from minimized if needed.
                                            self.wm.focus_window_or_restore(entry.id);
                                        } else if button == BTN_RIGHT {
                                            // Right click: toggle minimize.
                                            self.wm.toggle_minimize(entry.id);
                                        }
                                        // Skip normal click-to-focus on windows behind the dock.
                                        let pointer = self.seat.get_pointer().unwrap();
                                        pointer.button(
                                            self,
                                            &ButtonEvent {
                                                button,
                                                state: button_state.try_into().unwrap(),
                                                serial: SCOUNTER.next_serial(),
                                                time: event.time_msec(),
                                            },
                                        );
                                        pointer.frame(self);
                                        return;
                                    }
                                }
                            }
                        }
                    }
                    // click to focus + wl_keyboard enter (empty clicks keep focus)
                    let id = self.wm.window_at((location.x, location.y), self.wm.border_size());
                    if let Some(id) = id {
                        self.wm.focus_window(id);
                    }
                    let hit_rect = id.and_then(|id| self.wm.window(id)).map(|w| w.target_rect);
                    tracing::debug!(
                        "click at ({:.0},{:.0}) -> window {id:?} rect {hit_rect:?}",
                        location.x,
                        location.y
                    );
                    if let Some(id) = id {
                        keyboard_focus_set(self, Some(id));
                    }
                } else {
                    // release: end drag if it matches this button
                    let active = self.udev.as_ref().unwrap().drag.map(|drag| drag.button);
                    if active == Some(button) {
                        self.udev.as_mut().unwrap().drag = None;
                    }
                }
                let pointer = self.seat.get_pointer().unwrap();
                pointer.button(
                    self,
                    &ButtonEvent {
                        button,
                        state: button_state.try_into().unwrap(),
                        serial: SCOUNTER.next_serial(),
                        time: event.time_msec(),
                    },
                );
                pointer.frame(self);
            }
            InputEvent::PointerAxis { event, .. } => {
                let horizontal_amount = event
                    .amount(input::Axis::Horizontal)
                    .unwrap_or_else(|| event.amount_v120(input::Axis::Horizontal).unwrap_or(0.0) * 15.0 / 120.);
                let vertical_amount = event
                    .amount(input::Axis::Vertical)
                    .unwrap_or_else(|| event.amount_v120(input::Axis::Vertical).unwrap_or(0.0) * 15.0 / 120.);
                let horizontal_discrete = event.amount_v120(input::Axis::Horizontal);
                let vertical_discrete = event.amount_v120(input::Axis::Vertical);

                let mut frame = AxisFrame::new(event.time_msec()).source(event.source());
                if horizontal_amount != 0.0 {
                    frame = frame.value(input::Axis::Horizontal, horizontal_amount);
                    if let Some(discrete) = horizontal_discrete {
                        frame = frame.v120(input::Axis::Horizontal, discrete as i32);
                    }
                }
                if vertical_amount != 0.0 {
                    frame = frame.value(input::Axis::Vertical, vertical_amount);
                    if let Some(discrete) = vertical_discrete {
                        frame = frame.v120(input::Axis::Vertical, discrete as i32);
                    }
                }
                let pointer = self.seat.get_pointer().unwrap();
                pointer.axis(self, frame);
                pointer.frame(self);
            }
            _ => {
                // touch/tablet/gestures intentionally unsupported until M5
            }
        }
    }

    fn max_screen_size(&self) -> (u32, u32) {
        let udev = self.udev.as_ref().unwrap();
        let mut w = 0u32;
        let mut h = 0u32;
        for backend in udev.backends.values() {
            for surface in backend.surfaces.values() {
                if let Some(mode) = surface.output.current_mode() {
                    w += mode.size.w as u32;
                    h = h.max(mode.size.h as u32);
                }
            }
        }
        (w.max(1), h.max(1))
    }

    fn device_added(&mut self, node: DrmNode, path: &Path) -> Result<(), DeviceAddError> {
        let udev = self.udev.as_mut().unwrap();
        let fd = udev
            .session
            .open(
                path,
                OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOCTTY | OFlags::NONBLOCK,
            )
            .map_err(|err| DeviceAddError::DeviceOpen(err.to_string()))?;
        let fd = DrmDeviceFd::new(DeviceFd::from(fd));

        let (drm, notifier) = DrmDevice::new(fd.clone(), true).map_err(DeviceAddError::DrmDevice)?;
        let gbm = GbmDevice::new(fd).map_err(DeviceAddError::GbmDevice)?;

        let registration_token = self
            .handle
            .as_ref()
            .unwrap()
            .clone()
            .insert_source(
                notifier,
                move |event, metadata, data: &mut State| match event {
                    DrmEvent::VBlank(crtc) => data.frame_finish(node, crtc, metadata),
                    DrmEvent::Error(error) => error!("{error:?}"),
                },
            )
            .unwrap();

        let mut try_initialize_gpu = || {
            let display = unsafe { EGLDisplay::new(gbm.clone()).map_err(DeviceAddError::AddNode)? };
            let egl_device = EGLDevice::device_for_display(&display).map_err(DeviceAddError::AddNode)?;

            if egl_device.is_software() {
                return Err(DeviceAddError::NoRenderNode);
            }

            let render_node = egl_device.try_get_render_node().ok().flatten().unwrap_or(node);
            self.udev
                .as_mut()
                .unwrap()
                .gpus
                .as_mut()
                .add_node(render_node, gbm.clone())
                .map_err(DeviceAddError::AddNode)?;

            // Also add the primary node for GPUs like NVIDIA that need EGL_WL_bind_wayland_display on primary
            if render_node != node {
                self.udev
                    .as_mut()
                    .unwrap()
                    .gpus
                    .as_mut()
                    .add_node(node, gbm.clone())
                    .map_err(DeviceAddError::AddNode)?;
            }

            Ok::<DrmNode, DeviceAddError>(render_node)
        };

        let render_node = try_initialize_gpu()
            .inspect_err(|err| warn!(?err, "failed to initialize gpu"))
            .ok();

        let allocator = render_node
            .is_some()
            .then(|| GbmAllocator::new(gbm.clone(), GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT))
            .or_else(|| {
                let primary = self.udev.as_ref().unwrap().primary_gpu;
                self.udev
                    .as_ref()
                    .unwrap()
                    .backends
                    .get(&primary)
                    .or_else(|| {
                        self.udev
                            .as_ref()
                            .unwrap()
                            .backends
                            .values()
                            .find(|backend| backend.render_node == Some(primary))
                    })
                    .map(|backend| backend.drm_output_manager.allocator().clone())
            })
            .ok_or(DeviceAddError::PrimaryGpuMissing)?;

        let framebuffer_exporter = GbmFramebufferExporter::new(gbm.clone(), render_node);

        let color_formats = if std::env::var("TACIT_DISABLE_10BIT").is_ok() {
            SUPPORTED_FORMATS_8BIT_ONLY
        } else {
            SUPPORTED_FORMATS
        };
        let target = render_node.unwrap_or(self.udev.as_ref().unwrap().primary_gpu);
        let mut renderer = self.udev.as_mut().unwrap().gpus.single_renderer(&target).unwrap();
        let render_formats = renderer
            .as_mut()
            .egl_context()
            .dmabuf_render_formats()
            .iter()
            .filter(|format| render_node.is_some() || format.modifier == Modifier::Linear)
            .copied()
            .collect::<FormatSet>();

        let drm_output_manager = DrmOutputManager::new(
            drm,
            allocator,
            framebuffer_exporter,
            Some(gbm),
            color_formats.iter().copied(),
            render_formats,
        );

        self.udev.as_mut().unwrap().backends.insert(
            node,
            BackendData {
                registration_token,
                drm_output_manager,
                drm_scanner: DrmScanner::new(),
                render_node,
                surfaces: HashMap::new(),
            },
        );

        self.device_changed(node);

        Ok(())
    }

    fn connector_connected(&mut self, node: DrmNode, connector: connector::Info, crtc: crtc::Handle) {
        let primary_gpu = self.udev.as_ref().unwrap().primary_gpu;
        let udev = self.udev.as_mut().unwrap();
        let device = match udev.backends.get_mut(&node) {
            Some(device) => device,
            None => return,
        };

        let render_node = device.render_node.unwrap_or(primary_gpu);
        let mut renderer = udev.gpus.single_renderer(&render_node).unwrap();

        let output_name = format!("{}-{}", connector.interface().as_str(), connector.interface_id());
        info!(?crtc, "trying to setup connector {output_name}");

        let drm_device = device.drm_output_manager.device();

        let non_desktop = drm_device
            .get_properties(connector.handle())
            .ok()
            .and_then(|props| {
                let (info, value) = props
                    .into_iter()
                    .filter_map(|(handle, value)| {
                        let info = drm_device.get_property(handle).ok()?;
                        Some((info, value))
                    })
                    .find(|(info, _)| info.name().to_str() == Ok("non-desktop"))?;
                info.value_type().convert_value(value).as_boolean()
            })
            .unwrap_or(false);

        if non_desktop {
            // e.g. VR headsets: no lease support yet, so skip them
            warn!("connector {output_name} is non-desktop, skipping");
            return;
        }

        let display_info = smithay_drm_extras::display_info::for_connector(drm_device, connector.handle());
        let make = display_info
            .as_ref()
            .and_then(|info| info.make())
            .unwrap_or_else(|| "Unknown".into());
        let model = display_info
            .as_ref()
            .and_then(|info| info.model())
            .unwrap_or_else(|| "Unknown".into());

        let mode_id = connector
            .modes()
            .iter()
            .position(|mode| mode.mode_type().contains(ModeTypeFlags::PREFERRED))
            .unwrap_or(0);

        let drm_mode = connector.modes()[mode_id];
        let wl_mode = WlMode::from(drm_mode);

        let (phys_w, phys_h) = connector.size().unwrap_or((0, 0));
        let output = Output::new(
            output_name.clone(),
            PhysicalProperties {
                size: (phys_w as i32, phys_h as i32).into(),
                subpixel: connector.subpixel().into(),
                make,
                model,
            },
        );
        let global = output.create_global::<State>(&self.dh);

        output.set_preferred(wl_mode);
        output.change_current_state(Some(wl_mode), None, None, Some((0, 0).into()));

        // register real output geometry + workspaces 1..=9 into the window manager
        let mode_size = wl_mode.size;
        let wm_output = self.wm.add_output(
            output_name.as_str(),
            wm::Rect {
                x: 0,
                y: 0,
                width: mode_size.w as u32,
                height: mode_size.h as u32,
            },
        );
        for i in 1..=9 {
            self.wm.add_workspace(wm_output, i, i.to_string());
        }
        self.wm.focus_workspace(wm_output, 1);

        output.user_data().insert_if_missing(|| UdevOutputId {
            crtc,
            device_id: node,
        });
        self.primary_output = Some(output.clone());

        let drm_output = match device
            .drm_output_manager
            .initialize_output::<_, EswmRenderElements<UdevRenderer<'_>>>(
                crtc,
                drm_mode,
                &[connector.handle()],
                &output,
                Some(device.drm_output_manager.device().planes(&crtc).unwrap()),
                &mut renderer,
                &DrmOutputRenderElements::default(),
            )
        {
            Ok(drm_output) => drm_output,
            Err(err) => {
                warn!("failed to initialize drm output: {err:?}");
                return;
            }
        };

        let surface = SurfaceData {
            dh: self.dh.clone(),
            device_id: node,
            render_node: device.render_node,
            global: Some(global),
            output: output.clone(),
            drm_output,
            damage_tracker: OutputDamageTracker::from_output(&output),
            last_presentation_time: None,
            vblank_throttle_timer: None,
        };

        device.surfaces.insert(crtc, surface);

        // kick-off rendering
        let handle = self.handle.as_ref().unwrap().clone();
        handle.insert_idle(move |data: &mut State| {
            data.render(node, Some(crtc), 0);
        });
    }

    fn connector_disconnected(&mut self, node: DrmNode, connector: connector::Info, crtc: crtc::Handle) {
        if let Some(device) = self.udev.as_mut().unwrap().backends.get_mut(&node) {
            device.surfaces.remove(&crtc);
        }
        // Drop the WM output too, otherwise its windows stay on a dead output and
        // become unreachable (no hit-test, no focus, never rendered).
        let output_name = format!("{}-{}", connector.interface().as_str(), connector.interface_id());
        if let Some(output) = self.wm.output_id_by_name(&output_name) {
            self.wm.remove_output(output);
            crate::apply_layout(self);
        }
    }

    fn device_changed(&mut self, node: DrmNode) {
        let scan_result = {
            let device = match self.udev.as_mut().unwrap().backends.get_mut(&node) {
                Some(device) => device,
                None => return,
            };

            match device
                .drm_scanner
                .scan_connectors(device.drm_output_manager.device())
            {
                Ok(scan_result) => scan_result,
                Err(err) => {
                    warn!(?err, "failed to scan connectors");
                    return;
                }
            }
        };

        for event in scan_result {
            match event {
                DrmScanEvent::Connected {
                    connector,
                    crtc: Some(crtc),
                } => self.connector_connected(node, connector, crtc),
                DrmScanEvent::Disconnected {
                    connector,
                    crtc: Some(crtc),
                } => self.connector_disconnected(node, connector, crtc),
                _ => {}
            }
        }
    }

    fn device_removed(&mut self, node: DrmNode) {
        let crtcs: Vec<_> = if let Some(device) = self.udev.as_mut().unwrap().backends.get_mut(&node) {
            device
                .drm_scanner
                .crtcs()
                .map(|(info, crtc)| (info.clone(), crtc))
                .collect()
        } else {
            return;
        };

        for (connector, crtc) in crtcs {
            self.connector_disconnected(node, connector, crtc);
        }

        if let Some(mut backend_data) = self.udev.as_mut().unwrap().backends.remove(&node) {
            if let Some(render_node) = backend_data.render_node {
                self.udev.as_mut().unwrap().gpus.as_mut().remove_node(&render_node);
            }
            self.handle
                .as_ref()
                .unwrap()
                .clone()
                .remove(backend_data.registration_token);
            debug!("dropping device");
        }
    }

    fn frame_finish(&mut self, dev_id: DrmNode, crtc: crtc::Handle, metadata: &mut Option<DrmEventMetadata>) {
        let device_backend = match self.udev.as_mut().unwrap().backends.get_mut(&dev_id) {
            Some(backend) => backend,
            None => {
                error!("trying to finish frame on non-existent backend {dev_id}");
                return;
            }
        };

        let (last_presentation_time, timer_token) = if let Some(surface) = device_backend.surfaces.get_mut(&crtc) {
            (surface.last_presentation_time, surface.vblank_throttle_timer)
        } else {
            error!("trying to finish frame on non-existent crtc {crtc:?}");
            return;
        };

        if let Some(timer_token) = timer_token {
            self.handle.as_ref().unwrap().clone().remove(timer_token);
        }

        let output = match self
            .udev
            .as_ref()
            .unwrap()
            .backends
            .get(&dev_id)
            .and_then(|b| b.surfaces.get(&crtc))
        {
            Some(surface) => surface.output.clone(),
            None => return,
        };

        let Some(frame_duration) = output
            .current_mode()
            .map(|mode| Duration::from_secs_f64(1_000f64 / mode.refresh as f64))
        else {
            return;
        };

        let tp = metadata.as_ref().and_then(|metadata| match metadata.time {
            DrmEventTime::Monotonic(tp) => (!tp.is_zero()).then_some(tp),
            DrmEventTime::Realtime(_) => None,
        });

        let seq = metadata.as_ref().map(|metadata| metadata.sequence).unwrap_or(0);

        let clock = tp.unwrap_or(Duration::from_millis(now(self) as u64));

        let vblank_remaining_time = last_presentation_time.map(|last| {
            frame_duration.saturating_sub(clock.saturating_sub(last))
        });

        if let Some(vblank_remaining_time) = vblank_remaining_time {
            if vblank_remaining_time > frame_duration / 2 {
                static WARN_ONCE: std::sync::Once = std::sync::Once::new();
                WARN_ONCE.call_once(|| {
                    warn!("display running faster than expected, throttling vblanks");
                });
                let throttled_time = tp
                    .map(|tp| tp.saturating_add(vblank_remaining_time))
                    .unwrap_or(Duration::ZERO);
                let throttled_metadata = DrmEventMetadata {
                    sequence: seq,
                    time: DrmEventTime::Monotonic(throttled_time),
                };
                let timer_token = self
                    .handle
                    .as_ref()
                    .unwrap()
                    .clone()
                    .insert_source(
                        Timer::from_duration(vblank_remaining_time),
                        move |_, _, data: &mut State| {
                            data.frame_finish(dev_id, crtc, &mut Some(throttled_metadata));
                            TimeoutAction::Drop
                        },
                    )
                    .expect("failed to register vblank throttle timer");
                if let Some(surface) = self
                    .udev
                    .as_mut()
                    .unwrap()
                    .backends
                    .get_mut(&dev_id)
                    .and_then(|b| b.surfaces.get_mut(&crtc))
                {
                    surface.vblank_throttle_timer = Some(timer_token);
                }
                return;
            }
        }

        if let Some(surface) = self
            .udev
            .as_mut()
            .unwrap()
            .backends
            .get_mut(&dev_id)
            .and_then(|b| b.surfaces.get_mut(&crtc))
        {
            surface.last_presentation_time = Some(clock);
        }

        let submit_result = self
            .udev
            .as_mut()
            .unwrap()
            .backends
            .get_mut(&dev_id)
            .and_then(|b| b.surfaces.get_mut(&crtc))
            .map(|surface| surface.drm_output.frame_submitted().map_err(Into::<SwapBuffersError>::into));

        let schedule_render = match submit_result {
            Some(Ok(user_data)) => {
                if let Some(mut feedback) = user_data.flatten() {
                    let clock = Time::<smithay::utils::Monotonic>::from(clock);
                    feedback.presented(clock, Refresh::fixed(frame_duration), seq as u64,
                        wp_presentation_feedback::Kind::Vsync);
                }
                true
            }
            Some(Err(err)) => {
                debug!("frame_finish render: {err:?}");
                match err {
                    SwapBuffersError::AlreadySwapped => true,
                    SwapBuffersError::TemporaryFailure(err)
                        if matches!(err.downcast_ref::<DrmError>(), Some(&DrmError::DeviceInactive)) =>
                    {
                        // session was deactivated; resumed via session notifier
                        false
                    }
                    SwapBuffersError::TemporaryFailure(err) => matches!(
                        err.downcast_ref::<DrmError>(),
                        Some(DrmError::Access(DrmAccessError { source, .. }))
                            if source.kind() == io::ErrorKind::PermissionDenied
                    ),
                    SwapBuffersError::ContextLost(err) => panic!("rendering loop lost: {err}"),
                }
            }
            None => false,
        };

        if schedule_render {
            let next_frame_target = clock + frame_duration;

            // split the two vblanks: 0.6 for clients, rest for the compositor (see anvil)
            let repaint_delay = Duration::from_secs_f64(frame_duration.as_secs_f64() * 0.6f64);
            // shortcut: no multi-gpu copy check; always schedule with the simple delay
            let timer = Timer::from_duration(repaint_delay);
            self.handle
                .as_ref()
                .unwrap()
                .clone()
                .insert_source(
                    timer,
                    move |_, _, data: &mut State| {
                        data.render(dev_id, Some(crtc), next_frame_target.as_millis() as u32);
                        TimeoutAction::Drop
                    },
                )
                .expect("failed to schedule frame timer");
        }
    }

    fn render(&mut self, node: DrmNode, crtc: Option<crtc::Handle>, _frame_target_ms: u32) {
        // DRM is suspended when the user VT-switched away; rendering here only
        // yields DeviceInactive warnings and CPU burn
        if self.udev.as_ref().map(|u| u.paused).unwrap_or(true) {
            return;
        }
        if let Some(crtc) = crtc {
            self.render_surface(node, crtc);
        } else {
            let crtcs: Vec<_> = self
                .udev
                .as_ref()
                .unwrap()
                .backends
                .get(&node)
                .map(|b| b.surfaces.keys().copied().collect())
                .unwrap_or_default();
            for crtc in crtcs {
                self.render_surface(node, crtc);
            }
        }
    }

    fn render_surface(&mut self, node: DrmNode, crtc: crtc::Handle) {
        // Real tiling (M5): placement logic is shared with the winit loop (placed_windows).
        let placed = crate::placed_windows(self);
        tracing::debug!("render frame: {} window(s) placed", placed.len());
        tracing::debug!("render debug MARKER: debug block compiled in, placed.len()={}", placed.len());
        if let Some(udev) = self.udev.as_ref() {
            tracing::debug!("render debug: pointer=({:.0},{:.0}) cursor_buffer={} placed_windows={}",
                udev.pointer_location.x, udev.pointer_location.y,
                udev.cursor_buffer.is_some(),
                placed.len());
            for (_ws, x, y, id, focused, rect) in &placed {
                tracing::debug!("render debug: window id={id:?} focused={focused} rect=({},{}) {}x{}", x, y, rect.width, rect.height);
            }
        }

        // Use dmabuf_primary_gpu (first hw-accel GPU) for importing client buffers
        let primary_gpu = self.udev.as_ref().unwrap().dmabuf_primary_gpu;

        let (output, render_node, format_needed, mode_size) = {
            let udev = self.udev.as_ref().unwrap();
            let device_backend = match udev.backends.get(&node) {
                Some(b) => b,
                None => return,
            };
            let surface = match device_backend.surfaces.get(&crtc) {
                Some(s) => s,
                None => return,
            };
            let render_node = surface.render_node.unwrap_or(udev.primary_gpu);
            (
                surface.output.clone(),
                render_node,
                surface.drm_output.format(),
                surface.output.current_mode().map(|m| m.size),
            )
        };

        let time_ms = now(self); // must precede the mutable udev borrow below
        let udev = self.udev.as_mut().unwrap();
        let mut renderer = if primary_gpu == render_node {
            udev.gpus.single_renderer(&render_node)
        } else {
            udev.gpus.renderer(&primary_gpu, &render_node, format_needed)
        }
        .unwrap();
        tracing::debug!("render debug2: output_mode={:?} scale={:?} mode_size={:?} pointer=({:.0},{:.0}) cursor_buffer={}",
            mode_size.map(|s| (s.w, s.h)),
            output.current_scale(),
            mode_size,
            udev.pointer_location.x, udev.pointer_location.y,
            udev.cursor_buffer.is_some());

// Build the frame: placed client surfaces + cursor.
        let mut elements: Vec<EswmRenderElements<UdevRenderer<'_>>> = Vec::new();

        // Cursor first so it renders on top (smithay renders elements top-to-bottom).
        let pointer_location = udev.pointer_location;
        if let Some((buffer, hotspot)) = udev.cursor_buffer.clone() {
            let pos = (pointer_location - Point::from(hotspot))
                .to_physical(Scale::from(output.current_scale().fractional_scale()))
                .to_i32_round();
            // shortcut: cursor buffer import failure just drops the cursor for a frame
            if let Ok(cursor) = MemoryRenderBufferRenderElement::from_buffer(
                &mut renderer,
                pos,
                &buffer,
                None,
                None,
                None,
                Kind::Cursor,
            ) {
                elements.push(EswmRenderElements::Cursor(cursor));
            }
        } else if !udev.cursor_images.is_empty() {
            let img = udev.cursor_images[0].clone();
            let buf = MemoryRenderBuffer::from_slice(
                &img.pixels_rgba,
                Fourcc::Argb8888,
                (img.width as i32, img.height as i32),
                1,
                Transform::Normal,
                None,
            );
            udev.cursor_buffer = Some((buf.clone(), (img.xhot as f64, img.yhot as f64)));
            let pos = (pointer_location - Point::from((img.xhot as f64, img.yhot as f64)))
                .to_physical(Scale::from(output.current_scale().fractional_scale()))
                .to_i32_round();
            if let Ok(cursor) = MemoryRenderBufferRenderElement::from_buffer(
                &mut renderer,
                pos,
                &buf,
                None,
                None,
                None,
                Kind::Cursor,
            ) {
                elements.push(EswmRenderElements::Cursor(cursor));
            }
        }

        // Layer surfaces (wallpapers, fuzzel): positioned by their LayerMap
        // geometry. Overlay/top layers render above windows, bottom/background
        // below (elements list is top-to-bottom, cursor is pushed first).
        let mut layer_elements_above: Vec<EswmRenderElements<UdevRenderer<'_>>> = Vec::new();
        let mut layer_elements_below: Vec<EswmRenderElements<UdevRenderer<'_>>> = Vec::new();
        if let Some(output) = self.primary_output.clone() {
            let mut map = layer_map_for_output(&output);
            for layer in &self.layer_wrappers {
                if map.layer_for_surface(layer.wl_surface(), WindowSurfaceType::ALL).is_none() {
                    let _ = map.map_layer(layer);
                }
            }
            map.arrange();
            for layer in map.layers() {
                let Some(geom) = map.layer_geometry(layer) else { continue };
                let tree = render_elements_from_surface_tree(
                    &mut renderer,
                    layer.wl_surface(),
                    geom.loc.to_physical_precise_round(1.0),
                    1.0,
                    1.0,
                    Kind::Unspecified,
                );
                let target = match layer.layer() {
                    WlrLayer::Overlay | WlrLayer::Top => &mut layer_elements_above,
                    WlrLayer::Bottom | WlrLayer::Background => &mut layer_elements_below,
                };
                target.extend(tree.into_iter().map(EswmRenderElements::Surface));
            }
        }
        elements.extend(layer_elements_above);

        // M5/P2: visible borders around each window (Hyprland-style: outside the rect,
        // fullscreen/maximized windows skip them). Edge strips plus corner circles
        // when corner_radius > 0, giving rounded corners. Titlebar strip with buttons
        // when titlebar_height > 0.
        let border_size = self.wm.border_size();
        let color_focused = self.wm.border_color_focused();
        let color_unfocused = self.wm.border_color_unfocused();
        let titlebar_color_focused = self.wm.titlebar_color_focused();
        let titlebar_color_unfocused = self.wm.titlebar_color_unfocused();
        let dec = self.decoration.lock().unwrap();
        let corner_radius = dec.corner_radius;
        let titlebar_height = dec.titlebar_height;
        drop(dec);
        // Titlebar height and border size are independent: a titlebar can be
        // drawn even when the border is disabled (border_size == 0). The titlebar
        // strip overlaps the top of the client surface by design (the client
        // fills target_rect; its top zone is hidden behind the titlebar).
        if border_size > 0 || titlebar_height > 0 {
            let bp = border_size as f64;
            for (ws, x, y, id, focused, rect) in &placed {
                let mode = self.wm.window(*id).map(|w| w.mode).unwrap_or(wm::WindowMode::Tiled);
                // Smithay draws input[0] last (on top), so the focused window
                // (first in `placed`) paints on top overall. Within each window
                // the order is [borders, titlebar, surface]: borders pushed
                // first (index 0 → drawn last → on top, frame the window),
                // titlebar next (middle, drawn above the window frame, no
                // overlap with client content), surface pushed last (drawn
                // first → bottom layer = client content).
                let mut border_elements: Vec<EswmRenderElements<UdevRenderer>> = Vec::new();
                // Border frame extends upward by titlebar_height so borders capture the titlebar.
                let wx = rect.x as f64 - bp;
                let wy = rect.y as f64 - bp - titlebar_height as f64;
                let w = rect.width as f64 + 2.0 * bp;
                let h = rect.height as f64 + 2.0 * bp + titlebar_height as f64;
                // Borders pushed first → lowest index → drawn last → on top, frame titlebar + surface.
                // Titlebar next (middle, inside frame top, flush with surface top).
                // Surface last (bottom layer = client content).
                if !matches!(mode, wm::WindowMode::Fullscreen) {
                    let color = if *focused { &color_focused } else { &color_unfocused };
                    let px = [color[0], color[1], color[2], color[3]];
                    let edge_w = rect.width as u32 + 2 * border_size;
                    let edge_h = border_size;
                    let side_w = border_size;
                    let side_h = (rect.height as f64 + titlebar_height as f64 + bp) as u32;
                    let mut make_edge = |ew: u32, eh: u32, pos: (f64, f64)| {
                        let mut pix = Vec::with_capacity((ew * eh * 4) as usize);
                        for _ in 0..eh {
                            for _ in 0..ew {
                                pix.extend_from_slice(&px);
                            }
                        }
                        let buf = MemoryRenderBuffer::from_slice(
                            &pix, Fourcc::Argb8888, (ew as i32, eh as i32), 1, Transform::Normal, None,
                        );
                        MemoryRenderBufferRenderElement::from_buffer(
                            &mut renderer,
                            Point::from(pos),
                            &buf,
                            None, None, None, Kind::Unspecified,
                        ).ok().map(EswmRenderElements::Cursor)
                    };

                    if corner_radius > 0 && corner_radius * 2 <= rect.width && corner_radius * 2 <= rect.height {
                        // Rounded corners: shorten edges by corner_radius at each end,
                        // fill corners with circles centered at the outer corners.
                        let cr = corner_radius as f64;
                        // top edge
                        if let Some(elem) = make_edge(
                            edge_w.saturating_sub(2 * corner_radius),
                            edge_h,
                            (wx + cr, wy),
                        ) {
                            elements.push(elem);
                        }
                        // bottom edge
                        if let Some(elem) = make_edge(
                            edge_w.saturating_sub(2 * corner_radius),
                            edge_h,
                            (wx + cr, wy + h - bp),
                        ) {
                            elements.push(elem);
                        }
                        // left edge
                        if let Some(elem) = make_edge(
                            side_w,
                            side_h.saturating_sub(2 * corner_radius),
                            (wx, wy + cr),
                        ) {
                            border_elements.push(elem);
                        }
                        // right edge
                        if let Some(elem) = make_edge(
                            side_w,
                            side_h.saturating_sub(2 * corner_radius),
                            (wx + w - bp, wy + cr),
                        ) {
                            border_elements.push(elem);
                        }
                        // corner circles at the outer corners of the border rect
                        let circle_positions = [
                            (wx, wy),                                        // top-left
                            (wx + w - (corner_radius * 2) as f64, wy),       // top-right
                            (wx, wy + h - (corner_radius * 2) as f64),       // bottom-left
                            (wx + w - (corner_radius * 2) as f64,            // bottom-right
                             wy + h - (corner_radius * 2) as f64),
                        ];
                        for pos in circle_positions {
                            // Re-import the buffer at each position (render_element is position-bound).
                            let d = corner_radius * 2;
                            let r = corner_radius as f64;
                            let mut pix = Vec::with_capacity((d * d * 4) as usize);
                            for cy in 0..d {
                                for cx in 0..d {
                                    let dx = cx as f64 - r;
                                    let dy = cy as f64 - r;
                                    if dx * dx + dy * dy <= r * r {
                                        pix.extend_from_slice(&px);
                                    } else {
                                        pix.extend_from_slice(&[0, 0, 0, 0]);
                                    }
                                }
                            }
                            let buf = MemoryRenderBuffer::from_slice(
                                &pix, Fourcc::Argb8888, (d as i32, d as i32), 1, Transform::Normal, None,
                            );
                            if let Some(elem) = MemoryRenderBufferRenderElement::from_buffer(
                                &mut renderer,
                                Point::from(pos),
                                &buf,
                                None, None, None, Kind::Unspecified,
                            ).ok() {
                                border_elements.push(EswmRenderElements::Cursor(elem));
                            }
                        }
                    } else {
                        // Square corners (original behavior)
                        // top edge
                        if let Some(elem) = make_edge(
                            edge_w, edge_h, (wx, wy),
                        ) {
                            border_elements.push(elem);
                        }
                        // bottom edge
                        if let Some(elem) = make_edge(
                            edge_w, edge_h, (wx, wy + h - bp),
                        ) {
                            border_elements.push(elem);
                        }
                        // left edge
                        if let Some(elem) = make_edge(
                            side_w, side_h, (wx, wy),
                        ) {
                            border_elements.push(elem);
                        }
                        // right edge
                        if let Some(elem) = make_edge(
                            side_w, side_h, (wx + w - bp, wy),
                        ) {
                            border_elements.push(elem);
                        }
                    }
                }
                // Borders pushed first → index 0 → drawn last → on top (capture titlebar edges).
                elements.extend(border_elements);
                // Titlebar strip (middle layer: on top of surface, under borders).
                if titlebar_height > 0 && !matches!(mode, wm::WindowMode::Fullscreen) {
                    let tb_w = rect.width as u32;
                    let tb_h = titlebar_height;
                    let tb_px = if *focused { titlebar_color_focused } else { titlebar_color_unfocused };
                    let tb_px = [tb_px[0], tb_px[1], tb_px[2], tb_px[3]];
                    // Button elements collected first so they draw on top of
                    // the titlebar background (smithay draws index 0 last).
                    let mut button_elements: Vec<EswmRenderElements<UdevRenderer>> = Vec::new();
                    let btn_size = tb_h.saturating_sub(2);
                    let btn_gap = 4u32;
                    let mut btn_x = (rect.x as f64 + rect.width as f64) - (btn_size + btn_gap) as f64;
                    // Helper: draw a solid-color square button at (bx, by)
                    let mut draw_button = |bx: f64, by: f64, bcol: [u8; 4], bsize: u32| {
                        let mut bpix = Vec::with_capacity((bsize * bsize * 4) as usize);
                        for _ in 0..(bsize * bsize) {
                            bpix.extend_from_slice(&bcol);
                        }
                        let buf = MemoryRenderBuffer::from_slice(
                            &bpix, Fourcc::Argb8888, (bsize as i32, bsize as i32), 1, Transform::Normal, None,
                        );
                        if let Some(elem) = MemoryRenderBufferRenderElement::from_buffer(
                            &mut renderer,
                            Point::from((bx, by)),
                            &buf,
                            None, None, None, Kind::Unspecified,
                        ).ok() {
                            button_elements.push(EswmRenderElements::Cursor(elem));
                        }
                    };
                    draw_button(btn_x, rect.y as f64 - titlebar_height as f64 + 1.0, [255, 0, 0, 255], btn_size); // close
                    btn_x -= btn_size as f64 + btn_gap as f64;
                    draw_button(btn_x, rect.y as f64 - titlebar_height as f64 + 1.0, [200, 200, 0, 255], btn_size); // maximize
                    btn_x -= btn_size as f64 + btn_gap as f64;
                    draw_button(btn_x, rect.y as f64 - titlebar_height as f64 + 1.0, [100, 100, 100, 255], btn_size); // minimize
                    // Titlebar background buffer
                    let mut tb_pix = Vec::with_capacity((tb_w * tb_h * 4) as usize);
                    for _ in 0..(tb_w * tb_h) {
                        tb_pix.extend_from_slice(&tb_px);
                    }
                    let tb_buf = MemoryRenderBuffer::from_slice(
                        &tb_pix, Fourcc::Argb8888, (tb_w as i32, tb_h as i32), 1, Transform::Normal, None,
                    );
                    // Borders pushed earlier are at low index (on top). Buttons next,
                    // then titlebar background last (below buttons, above surface).
                    elements.extend(button_elements);
                    if let Some(elem) = MemoryRenderBufferRenderElement::from_buffer(
                        &mut renderer,
                        Point::from((rect.x as f64, rect.y as f64 - titlebar_height as f64)),
                        &tb_buf,
                        None, None, None, Kind::Unspecified,
                    ).ok() {
                        elements.push(EswmRenderElements::Cursor(elem));
                    }
                }
                // Always render the window surface tree (bottom layer).
                let tree_elements = render_elements_from_surface_tree(
                    &mut renderer,
                    ws,
                    (*x, *y),
                    1.0,
                    1.0,
                    Kind::Unspecified,
                );
                elements.extend(
                    tree_elements
                        .into_iter()
                        .map(EswmRenderElements::Surface),
                );
            }
        } else {
            for (ws, x, y, _, _, rect) in &placed {
                let tree_elements = render_elements_from_surface_tree(
                    &mut renderer,
                    ws,
                    (*x, *y),
                    1.0,
                    1.0,
                    Kind::Unspecified,
                );
                elements.extend(
                    tree_elements
                        .into_iter()
                        .map(EswmRenderElements::Surface),
                );
            }
        }

        // Dock bar at bottom of output (DRM primary path). Click-to-focus /
        // toggle-minimize handled in udev input handler.
        // Rendered into dock_elements and spliced to the FRONT of `elements`
        // afterwards: smithay draws index 0 last (on top), so windows pushed
        // earlier must not cover the dock strip.
        let mut dock_elements: Vec<EswmRenderElements<UdevRenderer<'_>>> = Vec::new();

        /// Embedded 5x7 bitmap font for dock labels (no external crate, no panic risk).
        /// Each glyph is 5 columns x 7 rows, 1 bit per pixel, MSB-first bytes.
        const FONT_5X7: &[(char, [u8; 7])] = &[
            ('A', [0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001]),
            ('B', [0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110]),
            ('C', [0b01110, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b01110]),
            ('D', [0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110]),
            ('E', [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111]),
            ('F', [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000]),
            ('G', [0b01110, 0b10000, 0b10000, 0b10011, 0b10101, 0b10001, 0b01110]),
            ('H', [0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001]),
            ('I', [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b11111]),
            ('J', [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b10100, 0b01110]),
            ('K', [0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001]),
            ('L', [0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111]),
            ('M', [0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001]),
            ('N', [0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001, 0b10001]),
            ('O', [0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110]),
            ('P', [0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000]),
            ('Q', [0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b11010, 0b01110]),
            ('R', [0b11110, 0b10001, 0b10001, 0b11110, 0b11000, 0b10100, 0b10010]),
            ('S', [0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b10001, 0b01110]),
            ('T', [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100]),
            ('U', [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110]),
            ('V', [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100]),
            ('W', [0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b11011, 0b10001]),
            ('X', [0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001]),
            ('Y', [0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100]),
            ('Z', [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111]),
            ('0', [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110]),
            ('1', [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110]),
            ('2', [0b01110, 0b10001, 0b00001, 0b00110, 0b01000, 0b10000, 0b11111]),
            ('3', [0b01110, 0b10001, 0b00001, 0b00110, 0b00001, 0b10001, 0b01110]),
            ('4', [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010]),
            ('5', [0b11111, 0b10000, 0b10000, 0b11110, 0b00001, 0b10001, 0b01110]),
            ('6', [0b01110, 0b10000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110]),
            ('7', [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000]),
            ('8', [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110]),
            ('9', [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b10001, 0b01110]),
            (' ', [0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000]),
            ('.', [0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00011]),
            (':', [0b00000, 0b00000, 0b00011, 0b00000, 0b00011, 0b00000, 0b00000]),
            ('-', [0b00000, 0b00000, 0b00000, 0b01110, 0b00000, 0b00000, 0b00000]),
            ('/', [0b00000, 0b00000, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000]),
            ('?', [0b01110, 0b10001, 0b00001, 0b00110, 0b00100, 0b00000, 0b00100]),
        ];

        let dec = self.decoration.lock().unwrap();
        let dock_height = dec.dock_height;
        drop(dec);
        tracing::debug!("dock debug: dock_height={} mode_size={:?} entries={}", dock_height, mode_size.map(|s| (s.w, s.h)), self.wm.dock_entries().len());
        if dock_height > 0 {
            if let Some(size) = mode_size {
                let loc = output.current_location();
                let entries = self.wm.dock_entries();
                let n = entries.len() as u32;
                tracing::debug!("dock debug: drawing dock at y={} h={} n={} size={}x{} loc={:?}", loc.y + size.h as i32 - dock_height as i32, dock_height, n, size.w, size.h, loc);
                if n > 0 {
                    let dock_y = (loc.y + size.h as i32 - dock_height as i32) as f64;
                    let dock_h = dock_height as i32;
                    let total_w = size.w as i32;
                    let seg_w = total_w / n as i32;

                    // Z-order (smithay: earlier index = on top): segments are
                    // pushed first, the base bar LAST so it sits at the BOTTOM
                    // and shows through the 1px gaps between segments. Pushing
                    // it first made it cover everything (black dock bug).

                    // Buffer cache: rebuild only when geometry or entry states
                    // change. Same MemoryRenderBuffer across frames = smithay
                    // texture cache hit = no per-frame upload (the lag cause).
                    let sig = (
                        n,
                        total_w,
                        dock_h,
                        entries.iter().map(|e| {
                            let label = e.app_id.chars().next()
                                .or_else(|| e.title.chars().next())
                                .unwrap_or('?')
                                .to_ascii_uppercase();
                            (e.id, e.focused, e.minimized, label)
                        }).collect::<Vec<_>>(),
                    );
                    let rebuild = {
                        let cached = self.dock_buffers.lock().unwrap();
                        match &*cached {
                            Some((old_sig, _)) => *old_sig != sig,
                            None => true,
                        }
                    };
                    if rebuild {
                        let mut bufs = Vec::with_capacity(entries.len() + 1);
                        for (i, entry) in entries.iter().enumerate() {
                            let w = if i as u32 == n.saturating_sub(1) {
                                total_w.saturating_sub(seg_w * (n.saturating_sub(1) as i32))
                            } else {
                                seg_w
                            };
                            if w <= 2 {
                                continue;
                            }
                            // Inset 1px left/right/top so the base bar shows as
                            // separators; bottom stays flush with the screen edge.
                            let seg_w_in = (w - 2) as usize;
                            let seg_h = (dock_h - 1) as usize;
                            let color: [u8; 4] = if entry.focused {
                                [70u8, 110, 170, 255]  // focused: clearly blue
                            } else if entry.minimized {
                                [30u8, 30, 30, 255]    // minimized: near-black
                            } else {
                                [70u8, 70, 70, 255]    // active: medium gray
                            };
                            let mut pix = Vec::with_capacity(seg_w_in * seg_h * 4);
                            for _ in 0..(seg_w_in * seg_h) {
                                pix.extend_from_slice(&color);
                            }
                            // Stamp the label glyph directly into the segment
                            // pixels (white on the segment color, fully opaque).
                            let label = entry.app_id.chars().next()
                                .or_else(|| entry.title.chars().next())
                                .unwrap_or('?')
                                .to_ascii_uppercase();
                            let rows = FONT_5X7.iter()
                                .find(|(g, _)| *g == label)
                                .map(|(_, r)| *r)
                                .unwrap_or([0; 7]);
                            let s = ((seg_w_in / 5).min((seg_h - 2) / 7)).clamp(1, 3);
                            let gw = 5 * s;
                            let gh = 7 * s;
                            let ox = (seg_w_in - gw) / 2;
                            let oy = (seg_h - gh) / 2;
                            for (ry, &bits) in rows.iter().enumerate() {
                                for rx in 0..5u32 {
                                    if bits & (1 << (4 - rx)) != 0 {
                                        for sy in 0..s {
                                            for sx in 0..s {
                                                let x = ox + rx as usize * s + sx;
                                                let y = oy + ry * s + sy;
                                                let idx = (y * seg_w_in + x) * 4;
                                                pix[idx] = 255;
                                                pix[idx + 1] = 255;
                                                pix[idx + 2] = 255;
                                            }
                                        }
                                    }
                                }
                            }
                            bufs.push(MemoryRenderBuffer::from_slice(
                                &pix, Fourcc::Argb8888, (seg_w_in as i32, seg_h as i32), 1, Transform::Normal, None,
                            ));
                        }
                        // Base bar LAST = bottom z: shows through the 1px gaps
                        // between segments so they read as separate buttons.
                        {
                            let mut pix = Vec::with_capacity(total_w as usize * dock_h as usize * 4);
                            for _ in 0..(total_w as usize * dock_h as usize) {
                                pix.extend_from_slice(&[15u8, 15, 15, 255]);
                            }
                            bufs.push(MemoryRenderBuffer::from_slice(
                                &pix, Fourcc::Argb8888, (total_w, dock_h), 1, Transform::Normal, None,
                            ));
                        }
                        *self.dock_buffers.lock().unwrap() = Some((sig, bufs));
                    }
                    let dock_buffers = self.dock_buffers.lock().unwrap();
                    if let Some((_, bufs)) = dock_buffers.as_ref() {
                        for (i, buf) in bufs.iter().enumerate() {
                            let pos = if i < entries.len() {
                                let seg_x = loc.x as f64 + i as f64 * seg_w as f64 + 1.0;
                                Point::from((seg_x, dock_y))
                            } else {
                                Point::from((loc.x as f64, dock_y))
                            };
                            if let Some(elem) = MemoryRenderBufferRenderElement::from_buffer(
                                &mut renderer,
                                pos,
                                buf,
                                None, None, None, Kind::Unspecified,
                            ).ok() {
                                dock_elements.push(EswmRenderElements::Cursor(elem));
                            }
                        }
                    }
                }
            }
        }
        // Dock on top of windows: index 0 renders last (on top).
        elements.splice(0..0, dock_elements);

        elements.extend(layer_elements_below);

        // shortcut: with no clients AND no cursor the damage tracker marks the
        // frame empty and it never scans out; prime it with a 1x1 element
        // (upgrade: proper cursor always present)
        if elements.is_empty() {
            let blank = MemoryRenderBuffer::from_slice(
                &[1, 0, 0, 255],
                Fourcc::Argb8888,
                (1, 1),
                1,
                Transform::Normal,
                None,
            );
            if let Ok(primed) = MemoryRenderBufferRenderElement::from_buffer(
                &mut renderer, Point::from((0f64, 0f64)), &blank, None, None, None, Kind::Unspecified,
            ) {
                elements.push(EswmRenderElements::Cursor(primed));
            }
        }

        let device_backend = match udev.backends.get_mut(&node) {
            Some(b) => b,
            None => return,
        };
        let surface = match device_backend.surfaces.get_mut(&crtc) {
            Some(s) => s,
            None => return,
        };

        tracing::debug!("render debug: output.current_location={:?} output.current_transform={:?} output.current_scale={:?} output.current_mode={:?}",
            output.current_location(), output.current_transform(), output.current_scale(), output.current_mode());
        let _ = mode_size;

        let result = surface
            .drm_output
            .render_frame(
                &mut renderer,
                &elements,
                smithay::backend::renderer::Color32F::new(0.6, 0.6, 0.6, 1.0),
                FrameFlags::ALLOW_PRIMARY_PLANE_SCANOUT_ANY | FrameFlags::ALLOW_CURSOR_PLANE_SCANOUT,
            )
            .map(|r| (!r.is_empty, r.states))
            .map_err(|err| match err {
                smithay::backend::drm::compositor::RenderFrameError::PrepareFrame(err) => SwapBuffersError::from(err),
                smithay::backend::drm::compositor::RenderFrameError::RenderFrame(
                    OutputDamageTrackerError::Rendering(err),
                ) => SwapBuffersError::from(err),
                _ => unreachable!(),
            });

        let mut sent_callbacks = false;
        let reschedule = match result {
            Ok((_is_empty, _states)) => {
                sent_callbacks = true;
                // render_frame only prepares the buffer; queue_frame actually
                // page-flips it to the scanout. Without it the screen stays black.
                // Also queue when there are placed windows but damage tracker sees no damage
                // (e.g. first window before client commits first buffer).
                // Always queue: render_frame only prepares the buffer;
                // queue_frame actually page-flips it. Gating on damage (is_empty)
                // froze the screen when the last window died — cursor-only frames
                // report is_empty (cursor elements are damage-ignored), so no
                // flip happened and the scanout kept the killed window's buffer.
                if let Err(err) = surface.drm_output.queue_frame(None::<OutputPresentationFeedback>) {
                    debug!("queue_frame: {err:?}");
                }
                tracing::debug!("render: {} elements, queued (placed={})", elements.len(), placed.len());
                false
            }
            Err(err) => {
                debug!("error during rendering: {err:#?}");
                match err {
                    SwapBuffersError::AlreadySwapped => false,
                    SwapBuffersError::TemporaryFailure(err) => match err.downcast_ref::<DrmError>() {
                        Some(DrmError::DeviceInactive) => true,
                        Some(DrmError::Access(DrmAccessError { source, .. })) => {
                            source.kind() == io::ErrorKind::PermissionDenied
                        }
                        _ => false,
                    },
                    SwapBuffersError::ContextLost(err) => match err.downcast_ref::<DrmError>() {
                        Some(DrmError::TestFailed(_)) => {
                            // reset state after a tty switch broke crtc <-> connector bindings
                            let res = self
                                .udev
                                .as_mut()
                                .unwrap()
                                .backends
                                .get_mut(&node)
                                .map(|b| b.drm_output_manager.device_mut().reset_state());
                            if let Some(res) = res {
                                res.expect("failed to reset drm device");
                            }
                            true
                        }
                        _ => panic!("rendering loop lost: {err}"),
                    },
                }
            }
        };

        // frame callbacks were previously sent inline; the udev mutable borrow
        // (needed for queue_frame) is live here, so send after the match instead.
        if sent_callbacks {
            self.frame_callbacks(time_ms);
        }

        if reschedule {
            let output_refresh = output.current_mode().map(|m| m.refresh).unwrap_or(60_000);
            let wait = Duration::from_millis(1_000_000 / output_refresh as u64);
            let timer = Timer::from_duration(wait);
            self.handle
                .as_ref()
                .unwrap()
                .clone()
                .insert_source(
                    timer,
                    move |_, _, data: &mut State| {
                        data.render(node, Some(crtc), now(data));
                        TimeoutAction::Drop
                    },
                )
                .expect("failed to schedule damage test timer");
        }
    }

    fn frame_callbacks(&mut self, time: u32) {
        for surface in self.xdg_shell_state.toplevel_surfaces() {
            send_frames_surface_tree(surface.wl_surface(), time);
        }
        for surface in self.layer_shell_state.layer_surfaces() {
            send_frames_surface_tree(surface.wl_surface(), time);
        }
    }
}

struct SurfaceData {
    dh: DisplayHandle,
    device_id: DrmNode,
    render_node: Option<DrmNode>,
    global: Option<GlobalId>,
    output: Output,
    drm_output: DrmOutput<
        GbmAllocator<DrmDeviceFd>,
        GbmFramebufferExporter<DrmDeviceFd>,
        Option<OutputPresentationFeedback>,
        DrmDeviceFd,
    >,
    damage_tracker: OutputDamageTracker,
    last_presentation_time: Option<Duration>,
    vblank_throttle_timer: Option<RegistrationToken>,
}

impl Drop for SurfaceData {
    fn drop(&mut self) {
        if let Some(global) = self.global.take() {
            self.dh.remove_global::<State>(global);
        }
    }
}

struct BackendData {
    surfaces: HashMap<crtc::Handle, SurfaceData>,
    drm_output_manager: DrmOutputManager<
        GbmAllocator<DrmDeviceFd>,
        GbmFramebufferExporter<DrmDeviceFd>,
        Option<OutputPresentationFeedback>,
        DrmDeviceFd,
    >,
    drm_scanner: DrmScanner,
    render_node: Option<DrmNode>,
    registration_token: RegistrationToken,
}

#[derive(Debug)]
enum DeviceAddError {
    DeviceOpen(String),
    DrmDevice(DrmError),
    GbmDevice(std::io::Error),
    DrmNode(CreateDrmNodeError),
    AddNode(egl::Error),
    NoRenderNode,
    PrimaryGpuMissing,
}
