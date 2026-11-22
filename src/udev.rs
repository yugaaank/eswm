// Real DRM/libinput backend (M4 Option A), ported from Smithay 0.7's anvil udev.rs.
// Simplified for M4: single seat, no drm lease/syncobj, no tablet/touch/gestures,
// windows render full-window stacked at (0,0) on one output (layout = M5).
use std::{
    collections::{HashMap, HashSet},
    io,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

pub use smithay::utils::SERIAL_COUNTER as SCOUNTER;
use smithay::desktop::{layer_map_for_output, WindowSurfaceType};
use wayland_protocols::wp::linux_dmabuf::zv1::server::zwp_linux_dmabuf_feedback_v1::TrancheFlags;
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
            compositor::{FrameError, FrameFlags},
            exporter::gbm::GbmFramebufferExporter,
            output::{DrmOutput, DrmOutputManager, DrmOutputRenderElements},
            CreateDrmNodeError, DrmAccessError, DrmDevice, DrmDeviceFd, DrmError, DrmEvent,
            DrmEventMetadata, DrmEventTime, DrmNode, NodeType,
        },
        egl::{self, context::ContextPriority, EGLDevice, EGLDisplay},
        input::{
            self, AbsolutePositionEvent as _, Event as _, InputBackend, InputEvent, KeyboardKeyEvent, PointerAxisEvent,
            PointerButtonEvent, PointerMotionEvent,
        },
        libinput::{LibinputInputBackend, LibinputSessionInterface},
        renderer::{
            damage::Error as OutputDamageTrackerError,
            element::{
                Id,
                memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
                solid::SolidColorRenderElement,
                surface::{render_elements_from_surface_tree, WaylandSurfaceRenderElement},
                Kind, render_elements,
            },
            gles::GlesRenderer,
            multigpu::{gbm::GbmGlesBackend, GpuManager, MultiRenderer},
            ImportAll, ImportDma, ImportEgl, ImportMem, Renderer, Color32F,
            utils::CommitCounter,
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
            EventLoop, Interest, Mode, PostAction, RegistrationToken,
            generic::Generic,
        },
        drm::control::{connector, crtc, Device, ModeTypeFlags},
        input::{DeviceCapability, Libinput},
        rustix::fs::OFlags,
        wayland_protocols::wp::presentation_time::server::wp_presentation_feedback,
        wayland_server::{backend::GlobalId, protocol::wl_surface::{self, WlSurface}, Display, DisplayHandle, Resource},
    },
    utils::{
        DeviceFd, Logical, Physical, Point, Rectangle, Scale, Size, Time, Transform,
    },    wayland::{
        dmabuf::{DmabufFeedbackBuilder, DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier},
        presentation::Refresh,
        socket::ListeningSocketSource,
    },
};
use tracing::{debug, error, info, trace, warn};


use smithay_drm_extras::drm_scanner::{DrmScanEvent, DrmScanner};

use crate::{send_frames_surface_tree, State, clip::ClippedSurfaceElement, wm, ipc, lua_state};
use crate::wm::{Rect, border_frame};

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
    ClippedSurface = ClippedSurfaceElement<R>,
    Solid = SolidColorRenderElement,
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
    pub gpus: GpuManager<GbmGlesBackend<GlesRenderer, DrmDeviceFd>>,
    pub backends: HashMap<DrmNode, BackendData>,
    pub dmabuf_state: Option<(DmabufState, DmabufGlobal)>,
    cursor_images: Vec<xcursor::parser::Image>,
    cursor_buffer: Option<(MemoryRenderBuffer, (f64, f64))>, // (buffer, hotspot)
    /// Latest cursor image requested by a client (e08s03t6).
    pub cursor_image: smithay::input::pointer::CursorImageStatus,
    /// Cache of titlebar background render buffers keyed by (window_id, rect, color, titlebar_height)
    /// to avoid reallocating per-frame buffers for static windows.
    titlebar_buffer_cache: HashMap<(u64, Rect, [u8; 4], u32), MemoryRenderBuffer>,
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
    /// Rolling render stats, logged at INFO every ~5s so release runs can be
    /// measured without debug logging inflating the frame time.
    pub stats: RenderStats,
    /// When the most recent client commit arrived. Used to measure the wait the
    /// user feels between "key pressed" and "pixels on screen". Taken (not
    /// copied) when a present serves it, so an already-painted commit cannot
    /// keep aging and fake a hitch on every later present. (A commit that is
    /// never taken is the freeze: it waited for input to force a flip.)
    pub last_commit_at: Option<Instant>,
    /// Same, for the last input event. A cursor that does not move is the
    /// failure the user notices before anything else.
    pub last_input_at: Option<Instant>,
    /// Key codes whose press was intercepted by a shortcut; matching releases are intercepted too.
    intercepted_keys: HashSet<u32>,
}

#[derive(Default)]
pub struct RenderStats {
    frames: u64,
    ms: f64,
    /// Slowest single frame in the window: the "random lag" moments live here,
    /// not in the average.
    max_ms: f64,
    /// Frames that could not fit inside a 60Hz period.
    slow: u64,
    /// Time inside render_frame (element drawing / dmabuf import).
    prep_ms: f64,
    /// Time inside queue_frame (atomic commit / page-flip submission).
    queue_ms: f64,
    deferred: u64,
    empty: u64,
    /// Client commits seen in the window: demand for frames.
    pub commits: u64,
    /// queue_frame failures that are not EmptyFrame (e.g. swapchain starvation).
    queue_err: u64,
    /// Longest gap between presented frames: a "whole screen hitch" shows up
    /// here even when every individual frame is fast.
    max_gap_ms: f64,
    /// Slowest queue_frame -> vblank round trip.
    flip_max_ms: f64,
    /// Slowest commit -> present latency, the user-visible number.
    lat_max_ms: f64,
    last_present: Option<Instant>,
    since: Option<Instant>,
}

/// A frame-to-frame gap longer than this is visible as a hitch on 60Hz — the
/// screen equivalent of a conversational pause that lasts long enough for
/// everyone to notice.
const HITCH_GAP_MS: f64 = 100.0;

/// A frame slower than this misses the next 60Hz vblank.
const SLOW_FRAME_MS: f64 = 1000.0 / 60.0;

impl RenderStats {
    fn record_frame(&mut self, total_ms: f64, prep_ms: f64, queue_ms: f64) {
        self.frames += 1;
        self.ms += total_ms;
        self.prep_ms += prep_ms;
        self.queue_ms += queue_ms;
        self.max_ms = self.max_ms.max(total_ms);
        if total_ms >= SLOW_FRAME_MS {
            self.slow += 1;
        }
    }

    /// Record a presented frame and return the gap since the previous one, if
    /// any. The gap is what the user perceives, not the frame cost.
    fn record_present(&mut self, now: Instant) -> Option<f64> {
        let gap = self
            .last_present
            .map(|last| now.duration_since(last).as_secs_f64() * 1000.0);
        self.last_present = Some(now);
        if let Some(gap) = gap {
            self.max_gap_ms = self.max_gap_ms.max(gap);
        }
        gap
    }

    fn record_flip(&mut self, ms: f64) {
        self.flip_max_ms = self.flip_max_ms.max(ms);
    }

    fn record_latency(&mut self, ms: f64) {
        self.lat_max_ms = self.lat_max_ms.max(ms);
    }
}

impl DmabufHandler for State {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.udev.as_mut().unwrap().dmabuf_state.as_mut().unwrap().0
    }

    fn dmabuf_imported(&mut self, _global: &DmabufGlobal, dmabuf: Dmabuf, notifier: ImportNotifier) {
        // BUG-2026-11-21-DMABUF-IMPORT: do NOT force the node hint to the
        // scanout GPU. Clients on hybrid systems allocate on another device;
        // forcing the node makes smithay import on the wrong EGL context and
        // produces `eglCreateImageKHR ... BAD_MATCH`. Leaving `node` unset
        // lets the buffer tell us where it was born — no GPU gaslighting.
        // lets smithay's multi-GPU importer discover the source at render time
        // and copy through a linear buffer when required.
        use smithay::backend::allocator::Buffer as _;
        let format = dmabuf.format();
        debug!(
            ?format,
            node = ?dmabuf.node(),
            "dmabuf imported (accepting without node hint)"
        );
        let _ = notifier.successful::<State>();
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
    let display: Display<State> = Display::new()?;
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
    let gpu_nodes = if let Ok(_var) = std::env::var("TACIT_DRM_DEVICE") {
        vec![primary_gpu]
    } else {
        let mut nodes = all_gpus(&seat_name)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|x| DrmNode::from_path(&x).ok())
            .collect::<Vec<_>>();
        // Also add the render node of the primary GPU if different
        if let Some(render) = primary_gpu.node_with_type(NodeType::Render)
            && let Ok(r) = render
                && !nodes.iter().any(|n| n.dev_id() == r.dev_id() && n.ty() == r.ty()) {
                    nodes.push(r);
                }
        nodes
    };
    info!("Using {primary_gpu} as primary gpu; all GPUs: {gpu_nodes:?}");

    let cursor_images = load_cursor_images();
    let state_u = UdevData {
        session,
        primary_gpu,
        dmabuf_primary_gpu: primary_gpu, // will be updated after hw accel detection
        gpus: GpuManager::new(GbmGlesBackend::with_context_priority(ContextPriority::High))?,
        backends: HashMap::new(),
        dmabuf_state: None,
        cursor_images,
        cursor_image: smithay::input::pointer::CursorImageStatus::default_named(),
        cursor_buffer: None,
        titlebar_buffer_cache: HashMap::new(),
        pointer_location: (0.0, 0.0).into(),
        last_mods: ModifiersState::default(),
        pointer_focus: None,
        keyboard_focus: None,
        drag: None,
        start: Instant::now(),
        quit: AtomicBool::new(false),
        paused: false,
        stats: RenderStats::default(),
        last_commit_at: None,
        last_input_at: None,
        intercepted_keys: HashSet::new(),
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
                if device.has_capability(DeviceCapability::Keyboard)
                    && let Some(led_state) = data.seat.get_keyboard().map(|key| key.led_state()) {
                        device.led_update(led_state.into());
                    }
                // Touchpad usability: libinput's defaults leave tap-to-click,
                // tap-drag and disable-while-typing off, and the cursor drifts
                // at low accel. Configure touchpads here (ref: wm-wayland
                // applies its config as each device appears, so hotplug works).
                if device.config_tap_finger_count() > 0 {
                    use smithay::reexports::input::ScrollMethod;
                    // e08s03t7: tap/natural-scroll/speed come from Lua config;
                    // drag, DWT and two-finger scroll keep their sane defaults.
                    let settings = crate::lua_state::resolve_touchpad(
                        &data.input_config.lock().unwrap(),
                    );
                    let _ = device.config_tap_set_enabled(settings.tap);
                    let _ = device.config_tap_set_drag_enabled(true);
                    let _ = device.config_tap_set_drag_lock_enabled(true);
                    let _ = device.config_dwt_set_enabled(true);
                    let _ = device.config_scroll_set_method(ScrollMethod::TwoFinger);
                    let _ = device.config_scroll_set_natural_scroll_enabled(settings.natural_scroll);
                    let _ = device.config_accel_set_speed(settings.speed);
                    info!(
                        "touchpad configured: {} (tap={}, natural_scroll={}, speed={})",
                        device.name(), settings.tap, settings.natural_scroll, settings.speed
                    );
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
        if let Err(err) = state.device_added(node, path) {
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
    
    // Enable hw acceleration on ALL GPUs for multi-GPU dmabuf import. Only one
    // of them can bind the process-global wl_drm reader; the rest still supply
    // renderers for the cross-GPU Linear tranche.
    let renderer_nodes: Vec<DrmNode> = candidate_nodes
        .iter()
        .copied()
        .filter(|node| udev.gpus.single_renderer(node).is_ok())
        .collect();
    let hw_accel_gpus = bind_wl_display_once(&renderer_nodes, |node| {
        info!("trying GPU {} for hardware acceleration", node);
        let mut r = match udev.gpus.single_renderer(&node) {
            Ok(r) => r,
            Err(e) => {
                info!(?e, "GPU {} lost its renderer", node);
                return false;
            }
        };
        match r.bind_wl_display(&display_handle) {
            Ok(_) => {
                info!("egl hardware-acceleration enabled on {}", node);
                true
            }
            Err(e) => {
                info!(?e, "GPU {} cannot bind the Wayland display", node);
                false
            }
        }
    });
    
    // Use the scanout GPU as the dmabuf GPU: it is the only choice that lets
    // client buffers be imported without a cross-GPU or cpu copy. The GPU that
    // can `bind_wl_display` is not necessarily the one driving the panel.
    let scanout_render_node = udev
        .backends
        .values()
        .find(|backend| !backend.surfaces.is_empty())
        .and_then(|backend| backend.surfaces.values().find_map(|surface| surface.render_node));
    let primary_for_dmabuf = choose_dmabuf_gpu(scanout_render_node, &hw_accel_gpus, primary_gpu);
    // Store for later use in render_surface and dmabuf_imported
    udev.dmabuf_primary_gpu = primary_for_dmabuf;
    info!(
        "dmabuf device: {primary_for_dmabuf} (scanout {:?}, hw-accel {:?})",
        scanout_render_node, hw_accel_gpus
    );
    
    // Create renderer for the primary GPU (used for dmabuf formats)
    // e08s01t5: propagate a renderer failure through init instead of panicking.
    let renderer = udev.gpus.single_renderer(&primary_for_dmabuf)?;

    // BUG-2026-11-21-DMABUF-IMPORT: on hybrid-GPU systems clients may allocate
    // on a different GPU than the scanout device. Advertise both: a Scanout
    // tranche for the panel's device and a Linear-only render tranche for the
    // other GPU so smithay's multi-GPU copy can bridge them.
    let dmabuf_formats = renderer.dmabuf_formats();
    let render_gpu = hw_accel_gpus
        .iter()
        .copied()
        .find(|node| Some(*node) != scanout_render_node);
    let cross_device = render_gpu.is_some();
    let render_formats = render_gpu
        .and_then(|node| udev.gpus.single_renderer(&node).ok())
        .map(|r| r.dmabuf_formats())
        .unwrap_or_default();
    let (scanout_tranche, render_tranche) =
        dmabuf_feedback_formats(&dmabuf_formats, &render_formats, cross_device);
    let mut builder = DmabufFeedbackBuilder::new(primary_for_dmabuf.dev_id(), dmabuf_formats.clone());
    if scanout_tranche.iter().next().is_some() {
        builder = builder.add_preference_tranche(
            primary_for_dmabuf.dev_id(),
            Some(TrancheFlags::Scanout),
            scanout_tranche,
        );
        info!("dmabuf feedback: scanout tranche on {primary_for_dmabuf}");
    }
    if let Some(node) = render_gpu
        && render_tranche.iter().next().is_some()
    {
        builder = builder.add_preference_tranche(node.dev_id(), None, render_tranche);
        info!("dmabuf feedback: added linear render tranche for {node}");
    }
    let default_feedback = builder.build()?;
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

    let listening_socket = ListeningSocketSource::with_name(&crate::socket_name())?;
    println!("listening on {:#?}", listening_socket.socket_name());
    event_loop
        .handle()
        .insert_source(listening_socket, move |stream, _, data: &mut State| {
            match data
                .dh
                .insert_client(stream, Arc::new(crate::ClientState::default()))
            {
                Ok(_) => {}
                Err(e) => tracing::warn!("failed to insert wayland client: {e}"),
            }
        })
        .unwrap();

    // Client fds must be in the poll set, or commits sit unprocessed until some
    // unrelated event (a vblank or cursor motion) wakes the loop — the desktop
    // then looks frozen until the mouse moves. Hyprland gets this free from
    // wl_display_run; anvil registers the display as a calloop Generic. Same trick.
    event_loop
        .handle()
        .insert_source(
            Generic::new(display, Interest::READ, Mode::Level),
            |_, display, data: &mut State| {
                // SAFETY: the display lives for the whole event loop; this is
                // the same pattern anvil uses (NoIoDrop::get_mut is unsafe by design).
                let display = unsafe { display.get_mut() };
                tracing::debug!("wayland source fired");
                if let Err(e) = display.dispatch_clients(data) {
                    tracing::warn!("failed to dispatch wayland clients: {e}");
                }
                Ok(PostAction::Continue)
            },
        )
        .expect("failed to register wayland display source");

    // Single SIGCHLD source: reap exited children with waitpid(-1, WNOHANG) loop.
    // signalfd makes SIGCHLD pollable for calloop without per-child wait threads.
    {
        use std::os::unix::io::{AsRawFd, FromRawFd, OwnedFd};
        unsafe {
            let mut mask: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut mask);
            libc::sigaddset(&mut mask, libc::SIGCHLD);
            libc::sigprocmask(libc::SIG_BLOCK, &mask, std::ptr::null_mut());
            let sfd = libc::signalfd(-1, &mask, libc::SFD_NONBLOCK | libc::SFD_CLOEXEC);
            if sfd >= 0 {
                let fd = OwnedFd::from_raw_fd(sfd);
                if let Err(e) = event_loop.handle().insert_source(
                    Generic::new(fd, Interest::READ, Mode::Level),
                    |_, source, _data: &mut State| {
                        let mut buf = [0u8; 128];
                        loop {
                            let n = libc::read(
                                source.as_raw_fd(),
                                buf.as_mut_ptr() as *mut libc::c_void,
                                std::mem::size_of_val(&buf),
                            );
                            if n <= 0 {
                                break;
                            }
                        }
                        let _ = crate::reap_spawned_children();
                        Ok(PostAction::Continue)
                    },
                ) {
                    warn!("failed to register SIGCHLD source: {e}");
                }
            } else {
                warn!("failed to create SIGCHLD signalfd: {}", io::Error::last_os_error());
            }
        }
    }

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
    state.input_config.clone(),
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

if lua_config_path.exists()
    && let Some(lua) = &state.lua {
        let snapshot = state.ipc.as_ref().unwrap().snapshot.clone();
        let cmd_tx = state.ipc.as_ref().unwrap().cmd_tx.clone();
        if let Err(e) = crate::lua_state::_do_file(lua, lua_config_path.to_str().unwrap(), snapshot, &cmd_tx) {
            tracing::warn!("Failed to load Lua config {}: {e}", lua_config_path.display());
        }
    }
    // e08s03t5: outputs may have been set up before the config was read.
    state.apply_configured_scales();
    // e08s03t8: apply a config-provided XKB keymap; an invalid string leaves
    // smithay's default keymap in place and is logged for the operator.
    let configured_keymap = state.input_config.lock().unwrap().keymap.clone();
    if let Some(keymap) = configured_keymap
        && let Some(keyboard) = state.seat.get_keyboard()
    {
        if let Err(err) = keyboard.set_keymap_from_string(&mut state, keymap) {
            tracing::warn!("failed to apply configured XKB keymap: {err}");
        } else {
            info!("applied configured XKB keymap");
        }
    }


    while !state.udev.as_ref().unwrap().quit.load(Ordering::SeqCst) {
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
let ipc_opt = state.ipc.take();
if let Some(ref ipc) = ipc_opt {
    for cmd in cmds {
        ipc::apply_cmd(&mut state, ipc, cmd);
    }
}
state.ipc = ipc_opt;

        // A hitch with fast frames means the loop sat on a pending redraw. Time
        // the blocking dispatch: nothing pending = idle wait, work pending =
        // we are the ones holding the frame hostage.
        let pending_before = state.redraw_needed;
        let dispatch_started = Instant::now();
        match event_loop.dispatch(None, &mut state) {
            Ok(()) => {
                let blocked_ms = dispatch_started.elapsed().as_secs_f64() * 1000.0;
                if loop_stall(pending_before, blocked_ms) {
                    warn!("loop blocked {blocked_ms:.0} ms with redraw pending");
                }
                // client dispatch happens in the display-source callback; only
                // flush our outgoing events here.
                display_handle.flush_clients()?;
                // e05s04t1: skip idle page-flips — only render when something
                // changed since the last frame (commit, cursor motion, layout).
                if !state.redraw_needed {
                    continue;
                }
                let nodes: Vec<DrmNode> = state
                    .udev
                    .as_ref()
                    .unwrap()
                    .backends
                    .keys()
                    .copied()
                    .collect();
                let t = now(&state);
                let mut painted_all = true;
                for node in nodes {
                    painted_all &= state.render(node, None, t);
                }
                // Keep redraw_needed when a frame was deferred (output busy):
                // the vblank completion will schedule the paint instead.
                if painted_all {
                    state.redraw_needed = false;
                }
            }
            Err(e) => {
                tracing::error!("event loop error: {e}");
                std::process::exit(1);
            }
        }
    }
    Ok(())
}

fn now(state: &State) -> u32 {
    state.udev.as_ref().unwrap().start.elapsed().as_millis() as u32
}

/// Every present delivers the frame callbacks that are pending. Pacing comes
/// from the output itself: only one flip is in flight, so presents cannot
/// outrun the refresh and a throttle on top of that can only lose callbacks.
fn callback_due(ms_since_last: u32, min_gap_ms: u32) -> bool {
    // Diagnostics only. This used to gate delivery, which deadlocked clients:
    // a suppressed callback leaves them waiting with nothing left to commit.
    trace!(ms_since_last, min_gap_ms, "delivering frame callbacks");
    true
}

/// A long gap between presents is only a hitch if a commit or an input event
/// actually waited for it. A commit that arrives just before the present ends
/// the gap and is served in a millisecond: the screen was simply idle, and
/// warning about it buries the real stalls in noise. A nap is not a coma.
fn is_hitch(waited_ms: Option<f64>) -> bool {
    matches!(waited_ms, Some(ms) if ms > HITCH_GAP_MS)
}

/// Time between an event the user is waiting on (a commit, an input event) and
/// the frame that answers it reaching the scanout. This is the wait the user
/// feels; a frame gap alone cannot tell an idle screen from a dropped frame.
fn latency_ms(event_at: Instant, presented_at: Instant) -> f64 {
    presented_at.duration_since(event_at).as_secs_f64() * 1000.0
}

/// Time between queue_frame and the matching vblank.
fn flip_latency_ms(queued_at: Instant, finished_at: Instant) -> f64 {
    finished_at.duration_since(queued_at).as_secs_f64() * 1000.0
}

/// Blocking the event loop is fine when there is nothing to paint. Blocking it
/// with a redraw pending is a hitch: the frame existed and we sat on it.
const LOOP_STALL_MS: f64 = 30.0;

fn loop_stall(pending: bool, blocked_ms: f64) -> bool {
    pending && blocked_ms >= LOOP_STALL_MS
}

fn render_timer_delay(redraw_needed: bool, _frame_duration: Duration) -> Option<Duration> {
    // Paint as soon as the vblank frees the output. The old 0.6-frame client
    // head start assumes a repaint under ~2ms; ours is ~15ms, so waiting meant
    // missing every second vblank and visibly skipping frames.
    redraw_needed.then_some(Duration::ZERO)
}

// One frame in flight per output. Rendering while a flip is pending just
// queues work the panel cannot present — that was the 147fps stutter bug.
// Pattern cross-checked against Hyprland's m_pendingFrame and cosmic-comp's
// QueueState::WaitingForVBlank { redraw_needed }.
fn should_render(redraw_needed: bool, frame_in_flight: bool) -> bool {
    redraw_needed && !frame_in_flight
}

/// Log render cost every ~5s at INFO so release runs are measurable without
/// debug logging inflating the numbers (debug adds ~40 lines per frame).
fn render_stats_tick(u: &mut UdevData) {
    let now = Instant::now();
    let since = *u.stats.since.get_or_insert(now);
    if now.duration_since(since) < Duration::from_secs(5) {
        return;
    }
    let s = &mut u.stats;
    if s.frames > 0 || s.deferred > 0 || s.empty > 0 {
        let n = s.frames.max(1) as f64;
        info!(
            "render stats: {} frames, {:.1} avg ms (max {:.1}, {} slow), prep {:.1}, queue {:.1} ({} err), {} commits, late max {:.0} ms, gap max {:.0} ms, flip max {:.0} ms, {} deferred, {} empty",
            s.frames,
            s.ms / n,
            s.max_ms,
            s.slow,
            s.prep_ms / n,
            s.queue_ms / n,
            s.queue_err,
            s.commits,
            s.lat_max_ms,
            s.max_gap_ms,
            s.flip_max_ms,
            s.deferred,
            s.empty
        );
    }
    *s = RenderStats { since: Some(now), ..RenderStats::default() };
}

// (render_now, keep_pending): a deferred redraw must survive the in-flight
// window, otherwise the commit is silently dropped (Hyprland learned this the
// hard way — see its "pending damage must keep the frame alive" comment).
#[cfg_attr(not(test), allow(dead_code))] // exercised by the redraw tests
fn next_redraw_state(redraw_needed: bool, frame_in_flight: bool) -> (bool, bool) {
    if should_render(redraw_needed, frame_in_flight) {
        (true, false)
    } else {
        (false, redraw_needed && frame_in_flight)
    }
}

/// What one render attempt achieved. Decides whether the pending redraw request
/// is satisfied and whether a retry timer is worth arming.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FrameOutcome {
    /// The frame reached the scanout.
    Presented,
    /// Nothing changed: the current scanout already shows this content.
    NothingToDo,
    /// A real failure (no free slot, device busy): retry on a timer.
    Retry,
}

impl FrameOutcome {
    /// True when the pending redraw request is done and must be cleared.
    fn satisfied(self) -> bool {
        !matches!(self, FrameOutcome::Retry)
    }
}

/// Refresh period for a mode, in ms. Used only to describe pacing in logs: the
/// real pacing is the one-frame-in-flight gate, which cannot outrun the panel.
fn callback_min_gap_ms(refresh_mhz: u32) -> u32 {
    // One refresh period. The old hard 33ms gave every callback-driven client
    // exactly every second frame on a 60Hz panel — i.e. visible frame skipping.
    // It was the compositor holding up a "no running by the pool" sign at 120Hz.
    if refresh_mhz == 0 {
        return 16;
    }
    (1_000_000 / refresh_mhz).max(1)
}

/// Pure, panic-safe dock segment geometry. Inset 1px on each side/top so the
/// base bar shows as separators; saturating on negative/tiny sizes.
fn dock_inner_size(w: i32, dock_h: i32) -> (usize, usize) {
    (
        w.saturating_sub(2).max(0) as usize,
        dock_h.saturating_sub(1).max(0) as usize,
    )
}

/// Glyph placement within a dock segment, computed without underflow. A scale
/// of 0 means no glyph is drawn (segment too small), which the renderer skips.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DockSegmentLayout {
    scale: usize,
    glyph_width: usize,
    glyph_height: usize,
    offset_x: usize,
    offset_y: usize,
}

fn dock_segment_layout(seg_w_in: usize, seg_h: usize) -> DockSegmentLayout {
    let scale = (seg_w_in / 5)
        .min(seg_h.saturating_sub(2) / 7)
        .min(3);
    let glyph_width = 5 * scale;
    let glyph_height = 7 * scale;
    DockSegmentLayout {
        scale,
        glyph_width,
        glyph_height,
        offset_x: seg_w_in.saturating_sub(glyph_width) / 2,
        offset_y: seg_h.saturating_sub(glyph_height) / 2,
    }
}

/// A lost GPU context is fatal: no frames can ever be presented again. e08s01t6
/// turns the old panic into one clear error log and a clean status-1 exit.
fn exit_on_context_lost(err: &dyn std::fmt::Display) -> ! {
    error!("GPU context lost: {err}; exiting so the session can recover");
    std::process::exit(1)
}

/// Index of the preferred DRM mode, or the first mode, or `None` when a
/// connector reports no modes at all. e08s01t4: indexing `modes()[0]` on an
/// empty list used to panic during connector setup.
fn preferred_mode_index(len: usize, is_preferred: impl Fn(usize) -> bool) -> Option<usize> {
    if len == 0 {
        return None;
    }
    Some((0..len).find(|&i| is_preferred(i)).unwrap_or(0))
}

/// Bind the Wayland display at most once across all candidate GPUs.
///
/// Smithay's `EGLBufferReader` is a process-wide singleton. A second
/// successful `bind_wl_display` warns and silently replaces the first
/// (smithay 0.7.0 `display.rs:857-864`), so binding one display per GPU is a
/// compositor wearing two hats and getting a stern letter from smithay about
/// it. Keep trying while binds fail, stop at the first success, and keep every
/// renderer-capable node in the usable set for dmabuf format advertising.
///
/// Reason for depth: hides the singleton rule behind one call so the DRM setup
/// loop cannot accidentally bind twice.
fn bind_wl_display_once<T: Copy>(
    nodes: &[T],
    mut bind: impl FnMut(T) -> bool,
) -> Vec<T> {
    let mut usable = Vec::with_capacity(nodes.len());
    let mut bound = false;
    for &node in nodes {
        let ok = if bound { true } else { bind(node) };
        if ok {
            bound = true;
            usable.push(node);
        }
    }
    usable
}

/// Pick the GPU whose formats the dmabuf global advertises.
///
/// It must be the GPU that scans out: clients allocate on the advertised
/// device, and any other choice means their buffers cross GPUs (or fall into
/// smithay's cpu-copy path) on every single frame. `bind_wl_display` is *not* a
/// good signal here — Mesa exposes EGL_WL_bind_wayland_display only on Wayland
/// platform displays, so it fails on the Intel GBM display that actually drives
/// the panel while succeeding on the idle NVIDIA one.
fn choose_dmabuf_gpu(
    scanout: Option<DrmNode>,
    hw_accel: &[DrmNode],
    fallback: DrmNode,
) -> DrmNode {
    scanout
        .or_else(|| hw_accel.first().copied())
        .unwrap_or(fallback)
}

/// Formats for the dmabuf feedback tranches (BUG-2026-11-21-DMABUF-IMPORT).
///
/// Returns `(scanout_formats, render_tranche_formats)`. When the render device
/// differs from the scanout device, the scanout tranche advertises the scanout
/// device's own formats (target = scanout, `TrancheFlags::Scanout`) and the
/// extra render tranche is restricted to Linear modifiers, which are the only
/// ones that can be copied between two GPUs. Callers omit empty tranches.
fn dmabuf_feedback_formats(
    scanout: &FormatSet,
    render: &FormatSet,
    cross_device: bool,
) -> (FormatSet, FormatSet) {
    if !cross_device {
        return (FormatSet::default(), FormatSet::default());
    }
    let scanout_formats = scanout.clone();
    let render_tranche: FormatSet = render
        .iter()
        .filter(|f| f.modifier == Modifier::Linear)
        .copied()
        .collect();
    (scanout_formats, render_tranche)
}

fn frame_outcome(empty_error: bool, queued: bool) -> FrameOutcome {
    if queued {
        // A flip happened. This includes cursor-plane-only updates: the damage
        // tracker calls those "empty" but the cursor plane still needs the
        // flip, and only queue_frame applies it.
        FrameOutcome::Presented
    } else if empty_error {
        // queue_frame said EmptyFrame: nothing changed, nothing to apply.
        // Request satisfied. Retrying here is what spun empty frames back to
        // back. Think of it as the compositor saying "I'm already wearing
        // that" — no need to ask again in 16ms.
        FrameOutcome::NothingToDo
    } else {
        FrameOutcome::Retry
    }
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


/// Return the first Lua keybind whose mods and keysym match the current event.
/// Compares bind-relevant modifiers only (logo/ctrl/alt/shift), ignoring Caps/Num Lock.
fn find_lua_keybind<'a>(
    keybinds: &'a [lua_state::Keybind],
    mods: &ModifiersState,
    raw_sym: u32,
) -> Option<&'a lua_state::Keybind> {
    keybinds.iter().find(|kb| {
        let kb_mods = parse_mods(&kb.mods);
        let mods_match = kb_mods.logo == mods.logo
            && kb_mods.ctrl == mods.ctrl
            && kb_mods.alt == mods.alt
            && kb_mods.shift == mods.shift;
        mods_match && kb.keysym == raw_sym
    })
}

/// Execute a single keybind action.
fn execute_keybind(state: &mut State, kb: &lua_state::Keybind) -> FilterResult<()> {
    match kb.action.as_str() {
        "spawn_terminal" => crate::spawn_terminal(),
        "spawn_fuzzel" => crate::spawn_on_socket("fuzzel", &[]),
        "close_window" => {
            if let Some(id) = state.wm.focused_id()
                && let Some(toplevel) = crate::toplevel_for_window(state, id) {
                    toplevel.send_close();
                }
        }
        "switch_workspace" => {
            if let Some(ws) = kb.arg {
                switch_workspace(state, ws);
            }
        }
        "move_window_to_workspace" => {
            if let Some(ws) = kb.arg
                && let Some(id) = state.wm.focused_id() {
                    state.wm.move_window_to_workspace(id, ws);
                    crate::apply_layout(state);
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
        "exec_cmd" => {
            if let Some(cmd) = &kb.cmd {
                let _ = crate::spawn_shell_command(cmd);
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
            if let Some(vt) = kb.arg
                && let Err(err) = state.udev.as_mut().unwrap().session.change_vt(vt) {
                    warn!("failed to switch to vt {vt}: {err:?}");
                }
        }
        _ => {
            warn!("unknown keybind action: {}", kb.action);
            return FilterResult::Forward;
        }
    }
    FilterResult::Intercept(())
}



/// Track intercepted key presses so their releases are intercepted symmetrically.
fn key_filter_result(
    intercepted: &mut HashSet<u32>,
    key_code: u32,
    pressed: bool,
    shortcut: impl FnOnce() -> FilterResult<()>,
) -> FilterResult<()> {
    if pressed {
        let result = shortcut();
        if matches!(result, FilterResult::Intercept(())) {
            intercepted.insert(key_code);
        }
        result
    } else if intercepted.remove(&key_code) {
        FilterResult::Intercept(())
    } else {
        FilterResult::Forward
    }
}

/// Built-in shortcuts that are always active and cannot be removed/overridden by config.
/// Returns a synthetic keybind action for emergency quit and VT switching.
fn builtin_shortcut(mods: &ModifiersState, raw_sym: u32) -> Option<lua_state::Keybind> {
    use xkbcommon::xkb::keysyms;

    let logo = mods.logo;
    let ctrl = mods.ctrl;
    let alt = mods.alt;
    let shift = mods.shift;

    // Emergency quit: Super+Alt+Backspace (always on)
    if logo && alt && !ctrl && !shift && raw_sym == keysyms::KEY_BackSpace {
        return Some(lua_state::Keybind {
            mods: vec!["logo".into(), "alt".into()],
            key: "BackSpace".into(),
            keysym: raw_sym,
            action: "quit".into(),
            arg: None,
            cmd: None,
        });
    }

    // VT switch: Ctrl+Alt+F1..F12 / XF86Switch_VT_1..12 (always on)
    if ctrl && alt && !logo && !shift
        && (keysyms::KEY_XF86Switch_VT_1..=keysyms::KEY_XF86Switch_VT_12).contains(&raw_sym)
    {
        let vt = (raw_sym - keysyms::KEY_XF86Switch_VT_1 + 1) as i32;
        return Some(lua_state::Keybind {
            mods: vec!["ctrl".into(), "alt".into()],
            key: format!("XF86Switch_VT_{vt}"),
            keysym: raw_sym,
            action: "change_vt".into(),
            arg: Some(vt),
            cmd: None,
        });
    }

    None
}

// All keybinds are driven from Lua es.bind() calls in ~/.config/eswm/config.lua.
// process_shortcut checks built-in always-on shortcuts first, then Lua keybinds;
// anything not configured is forwarded to clients.
fn process_shortcut(state: &mut State, mods: &ModifiersState, keysym: KeysymHandle) -> FilterResult<()> {
    crate::prune_dead_windows(state);
    let Some(raw_sym) = keysym.raw_latin_sym_or_raw_current_sym() else {
        return FilterResult::Forward;
    };
    let raw = raw_sym.raw();

    // Always-on built-ins (emergency quit, VT switch) take precedence and cannot be removed.
    if let Some(kb) = builtin_shortcut(mods, raw) {
        return execute_keybind(state, &kb);
    }

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

/// Recompute pointer (hover/drag) focus from `pos` and return the surface-local
/// hit for `Pointer.motion`, or None when over empty space.
/// If a popup has an active grab, all pointer events go to the popup surface.
fn pointer_focus(state: &mut State, pos: Point<f64, Logical>) -> Option<(wl_surface::WlSurface, Point<f64, Logical>)> {
    use smithay::wayland::selection::primary_selection::set_primary_focus;

    let focus = (|| {
        // Popup grab takes priority: route pointer input to the grabbed popup only
        // while the pointer is over it. Outside the popup we return None so the
        // motion does not leak into the surface underneath (the button handler
        // turns an outside press into a dismissal).
        if let Some(popup) = state.popup_grab.clone() {
            let rect = crate::popup_screen_rect(state, &popup)?;
            if crate::rect_contains(rect, pos.x, pos.y) {
                return Some((
                    popup.wl_surface().clone(),
                    // smithay computes surface-local coords as `event.location - loc`,
                    // so this must be the popup's screen origin.
                    Point::from((rect.x as f64, rect.y as f64)),
                ));
            }
            return None;
        }

        // Non-grabbed popups (tooltips, some menus) still need pointer input:
        // hit-test them topmost-first by their resolved screen rect.
        for popup in state.popups.iter().rev() {
            if !popup.surface.alive() || !popup.mapped {
                continue;
            }
            if crate::rect_contains(popup.screen_rect, pos.x, pos.y) {
                return Some((
                    popup.surface.wl_surface().clone(),
                    Point::from((popup.screen_rect.x as f64, popup.screen_rect.y as f64)),
                ));
            }
        }

        crate::prune_dead_windows(state);
        let dragging = state.udev.as_ref().unwrap().drag;
        let hit = if let Some(drag) = dragging {
            let id = drag.window;
            let rect = state.wm.window(id)?.target_rect;
            Some((id, Point::from((rect.x as f64, rect.y as f64))))
        } else {
            state
                .wm
                .surface_under((pos.x, pos.y))
                .map(|(id, local)| (id, Point::from((pos.x - local.0, pos.y - local.1))))
        };
        let Some((id, origin)) = hit else {
            if let Some(udev) = state.udev.as_mut() {
                udev.pointer_focus = None;
            }
            return None;
        };
        if let Some(udev) = state.udev.as_mut() {
            udev.pointer_focus = Some(id);
        }
        let toplevel = crate::toplevel_for_window(state, id)?;
        Some((toplevel.wl_surface().clone(), origin))
    })();

    // e06s04t2: primary selection follows pointer focus (client under pointer).
    let primary_client = focus
        .as_ref()
        .and_then(|(surface, _)| state.dh.get_client(surface.id()).ok());
    set_primary_focus(&state.dh, &state.seat, primary_client);

    focus
}

/// Render bands, listed top-first (smithay draws index 0 last / on top).
/// e08s02t3: the z-order policy is pure so it can be tested without a GPU.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RenderBand {
    Cursor,
    Fullscreen,
    OverlayTop,
    Dock,
    Windows,
    Background,
}

/// Top-to-bottom render order. With a fullscreen window it sits above all
/// layer groups and the dock (operator decision), but below the cursor.
fn render_band_order(fullscreen_active: bool) -> Vec<RenderBand> {
    let mut bands = vec![RenderBand::Cursor];
    if fullscreen_active {
        bands.push(RenderBand::Fullscreen);
    }
    bands.extend([
        RenderBand::OverlayTop,
        RenderBand::Dock,
        RenderBand::Windows,
        RenderBand::Background,
    ]);
    bands
}

/// What the titlebar close button may do. e08s02t1: the decision is kept pure
/// so "never orphan a client" is a regression-testable policy, not a comment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TitlebarCloseAction {
    RequestClientClose,
    LeaveWindowUntouched,
}

fn titlebar_close_action(toplevel_present: bool) -> TitlebarCloseAction {
    if toplevel_present {
        TitlebarCloseAction::RequestClientClose
    } else {
        TitlebarCloseAction::LeaveWindowUntouched
    }
}

/// Perform a server-side titlebar button action and push the resulting state
/// to clients.
///
/// BUG-2026-11-19T235000: every branch must go through `apply_layout`. The old
/// inline handler called the WM directly, so the frame grew to the maximised
/// rect while the client kept its old size — a window wearing a suit two sizes
/// too big. (The suit has been tailored.)
pub fn apply_titlebar_action(state: &mut State, id: wm::WindowId, button: wm::TitlebarButton) {
    match button {
        wm::TitlebarButton::Close => {
            // e08s02t2: ask the client to close; its destroy path removes the
            // WM window. Removing it here orphaned the process (invisible app).
            let toplevel = crate::toplevel_for_window(state, id);
            match titlebar_close_action(toplevel.is_some()) {
                TitlebarCloseAction::RequestClientClose => {
                    if let Some(toplevel) = toplevel {
                        toplevel.send_close();
                    }
                }
                TitlebarCloseAction::LeaveWindowUntouched => {}
            }
            return;
        }
        wm::TitlebarButton::Maximize => {
            state.wm.toggle_maximized(id);
        }
        wm::TitlebarButton::Minimize => {
            state.wm.toggle_minimize(id);
        }
    }
    // This is the actual fix: without it the WM rect changes but no configure
    // reaches the client, so only the frame grows (BUG-2026-11-19T235000).
    crate::apply_layout(state);
    state.redraw_needed = true;
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

/// Clear tracked toplevel keyboard focus when a layer surface holds wl_keyboard
/// focus (e06s03).
pub fn keyboard_focus_set_layer(state: &mut State) {
    if let Some(udev) = state.udev.as_mut() {
        udev.keyboard_focus = None;
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
        // Input arriving while nothing presents is a freeze the user feels.
        // Record when it arrived so the next present can say how long it
        // waited, instead of guessing from "was there any input".
        if let Some(u) = self.udev.as_mut() {
            u.last_input_at = Some(Instant::now());
        }
        match event {
            InputEvent::Keyboard { event, .. } => {
                let keyboard = self.seat.get_keyboard().unwrap();
                // libinput/smithay call the filter for press *and* release;
                // shortcuts fire on press; consumed presses must also intercept release.
                let key_code = event.key_code();
                let key_code_u32: u32 = key_code.into();
                let pressed = event.state() == input::KeyState::Pressed;
                keyboard.input::<(), _>(
                    self,
                    key_code,
                    event.state(),
                    SCOUNTER.next_serial(),
                    event.time_msec(),
                    |state, mods, keysym| {
                        if let Some(udev) = state.udev.as_mut() {
                            udev.last_mods = *mods;
                        }
                        let mut intercepted = state
                            .udev
                            .as_mut()
                            .map(|u| std::mem::take(&mut u.intercepted_keys))
                            .unwrap_or_default();
                        let result = key_filter_result(&mut intercepted, key_code_u32, pressed, || {
                            process_shortcut(state, mods, keysym)
                        });
                        if let Some(udev) = state.udev.as_mut() {
                            udev.intercepted_keys = intercepted;
                        }
                        result
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
                self.redraw_needed = true;
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
                // Same-class sibling of the freeze above: moving the pointer is
                // the user waiting for a new cursor pixel. Without this flag the
                // location updates in memory and no flip is ever queued, so an
                // absolute (tablet/touchpad-mapped) pointer could sit frozen
                // while the compositor believes it has nothing to do.
                self.redraw_needed = true;
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
                    // Popup grab active: clicks inside the popup belong to it, a
                    // click anywhere else dismisses it (xdg-shell popup grab). This
                    // port follows Hyprland's seat-grab behaviour: outside press ->
                    // popup_done + focus back to the owning toplevel.
                    if let Some(popup) = self.popup_grab.clone() {
                        let inside = crate::popup_screen_rect(self, &popup)
                            .map(|rect| crate::rect_contains(rect, location.x, location.y))
                            .unwrap_or(false);
                        if inside {
                            // motion() above already targeted the popup surface, so
                            // the button is delivered to the client with popup-local
                            // coordinates.
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
                        // Outside: dismiss. popup_done tells the client its menu was
                        // closed; without it GTK keeps waiting for input that never
                        // comes (and the whole app looks like it froze — fun times).
                        // Then give keyboard focus back to the toplevel.
                        if popup.alive() {
                            popup.send_popup_done();
                        }
                        self.popup_grab = None;
                        let owner = crate::popup_owner_window(self, &popup);
                        keyboard_focus_set(self, owner.or(self.wm.focused_id()));
                        self.redraw_needed = true;
                        tracing::info!(
                            "popup dismissed on click outside at ({:.0},{:.0}) owner={owner:?}",
                            location.x,
                            location.y
                        );
                        // Don't forward this click to any surface — the popup is gone.
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
                    // Super+button starts a move/resize drag on a floating window
                    if mods.logo && (button == BTN_LEFT || button == BTN_RIGHT)
                        && let Some(id) = self.wm.window_at((location.x, location.y), self.wm.border_size())
                            && self.wm.window(id).map(|w| w.mode.placement == wm::Placement::Floating && w.mode.presentation == wm::Presentation::Normal).unwrap_or(false) {
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
                    // Titlebar button clicks: close / maximize / minimize.
                    // Clicking the titlebar itself (not a button) drags a floating window.
                    let titlebar_height = self.decoration.lock().unwrap().titlebar_height;
                    if titlebar_height > 0
                        && let Some(id) = self.wm.window_at((location.x, location.y), self.wm.border_size()) {
                            let window = self.wm.window(id).unwrap();
                            let rect = window.target_rect;
                            let mode = window.mode;
                            let tb = titlebar_height as f64;
                            // Titlebar is drawn above the client surface: y in [rect.y - tb, rect.y)
                            let in_titlebar = location.y >= rect.y as f64 - tb && location.y < rect.y as f64
                                && location.x >= rect.x as f64 && location.x < rect.x as f64 + rect.width as f64;
                            if in_titlebar {
                                match wm::titlebar_button_at(rect, titlebar_height, location.x) {
                                    Some(button) => apply_titlebar_action(self, id, button),
                                    None if mode.placement == wm::Placement::Floating && mode.presentation == wm::Presentation::Normal => {
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
                                    None => {}
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
                    if dock_height > 0
                        && let Some((gx, _gy, gw, gh)) = dock {
                            // Dock strip sits inside the non-exclusive zone (above
                            // any bottom-anchored layer like waybar), matching the
                            // render side; work_area already has dock_h subtracted.
                            let wa_bottom = self
                                .wm
                                .work_area
                                .map(|wa| wa.y as f64 + wa.height as f64)
                                .unwrap_or(gh - dock_height as f64);
                            if location.y >= wa_bottom
                                && location.y < wa_bottom + dock_height as f64
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
        // e08s01t5: a renderer failure here is a device-add failure, not a
        // compositor-wide panic. Propagate it through the existing error path.
        let mut renderer = self
            .udev
            .as_mut()
            .unwrap()
            .gpus
            .single_renderer(&target)
            .map_err(|err| DeviceAddError::Renderer(err.to_string()))?;
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
        let output_name = format!("{}-{}", connector.interface().as_str(), connector.interface_id());
        // e08s01t5: keep a dead GPU from taking down the whole compositor.
        let mut renderer = match udev.gpus.single_renderer(&render_node) {
            Ok(renderer) => renderer,
            Err(err) => {
                warn!(?err, ?render_node, "renderer unavailable, skipping connector {output_name}");
                return;
            }
        };
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

                                let edid_data = drm_device
            .get_properties(connector.handle())
            .ok()
            .and_then(|props| {
                props.into_iter().find_map(|(handle, value)| {
                    let info = drm_device.get_property(handle).ok()?;
                    if info.name().to_str() == Ok("EDID") {
                        drm_device.get_property_blob(value).ok()
                    } else {
                        None
                    }
                })
            });

        let (make, model) = edid_data.as_deref().map_or_else(
            || ("Unknown".into(), "Unknown".into()),
            |edid| {
                let make = crate::edid::edid_manufacturer_name(edid).unwrap_or_else(|| "Unknown".into());
                let model = crate::edid::edid_monitor_name(edid).unwrap_or_else(|| "Unknown".into());
                (make, model)
            },
        );

        // e08s01t4: a connector with no modes cannot drive an output. Skip it
        // with a clear warning instead of panicking on `modes()[0]`.
        let mode_id = match preferred_mode_index(connector.modes().len(), |i| {
            connector.modes()[i].mode_type().contains(ModeTypeFlags::PREFERRED)
        }) {
            Some(id) => id,
            None => {
                warn!("connector {output_name} reports no modes, skipping");
                return;
            }
        };

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
        // e08s03t5: apply the configured per-output scale before the first
        // frame; Scale::Fractional is what wl_output/fractional clients see.
        let scale = self
            .input_config
            .lock()
            .unwrap()
            .output_scale(&output_name);
        output.change_current_state(Some(wl_mode), None, scale, Some((0, 0).into()));

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
            device_id: node,
            crtc,
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
            render_node: device.render_node,
            global: Some(global),
            output: output.clone(),
            drm_output,
            last_presentation_time: None,
            vblank_throttle_timer: None,
            frame_in_flight: false,
            queued_at: None,
        };

        device.surfaces.insert(crtc, surface);

        // kick-off rendering
        let handle = self.handle.as_ref().unwrap().clone();
        handle.insert_idle(move |data: &mut State| {
            data.render(node, Some(crtc), 0);
        });
    }

    /// Apply configured scales to outputs created before the Lua config was
    /// loaded (e08s03t5). Hotplugged outputs get their scale at setup instead.
    fn apply_configured_scales(&self) {
        let Some(udev) = self.udev.as_ref() else { return };
        let config = self.input_config.lock().unwrap();
        for backend in udev.backends.values() {
            for surface in backend.surfaces.values() {
                let name = surface.output.name();
                if let Some(scale) = config.output_scale(&name) {
                    surface.output.change_current_state(None, None, Some(scale), None);
                    info!("output {name} scale set to {}", scale.fractional_scale());
                }
            }
        }
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

        if let Some(backend_data) = self.udev.as_mut().unwrap().backends.remove(&node) {
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

        let (last_presentation_time, timer_token, flip_ms) = if let Some(surface) = device_backend.surfaces.get_mut(&crtc) {
            // The vblank landed: this crtc is free to accept another frame.
            surface.frame_in_flight = false;
            // If the flip itself was slow, no amount of render-loop tuning
            // helps: the kernel/GPU is the one holding the picture hostage.
            let flip_ms = surface.queued_at.take().map(|queued| flip_latency_ms(queued, Instant::now()));
            (surface.last_presentation_time, surface.vblank_throttle_timer, flip_ms)
        } else {
            error!("trying to finish frame on non-existent crtc {crtc:?}");
            return;
        };

        if let Some(timer_token) = timer_token {
            self.handle.as_ref().unwrap().clone().remove(timer_token);
        }

        if let Some(ms) = flip_ms {
            if let Some(u) = self.udev.as_mut() {
                u.stats.record_flip(ms);
            }
            if ms > HITCH_GAP_MS {
                warn!("flip took {ms:.0} ms (vblank late)");
            }
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

        if let Some(vblank_remaining_time) = vblank_remaining_time
            && vblank_remaining_time > frame_duration / 2 {
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
                    SwapBuffersError::ContextLost(err) => exit_on_context_lost(&err),
                }
            }
            None => false,
        };

        if schedule_render {
            let next_frame_target = clock + frame_duration;

            let Some(repaint_delay) = render_timer_delay(self.redraw_needed, frame_duration) else {
                return;
            };
            // shortcut: no multi-gpu copy check; always schedule with the simple delay
            let timer = Timer::from_duration(repaint_delay);
            self.handle
                .as_ref()
                .unwrap()
                .clone()
                .insert_source(
                    timer,
                    move |_, _, data: &mut State| {
                        if render_timer_delay(data.redraw_needed, frame_duration).is_none() {
                            return TimeoutAction::Drop;
                        }
                        // Only consume the request once the frame actually hit the
                        // scanout; a deferred paint must stay queued.
                        if data.render(dev_id, Some(crtc), next_frame_target.as_millis() as u32) {
                            data.redraw_needed = false;
                        }
                        TimeoutAction::Drop
                    },
                )
                .expect("failed to schedule frame timer");
        }
    }

    fn render(&mut self, node: DrmNode, crtc: Option<crtc::Handle>, _frame_target_ms: u32) -> bool {
        // DRM is suspended when the user VT-switched away; rendering here only
        // yields DeviceInactive warnings and CPU burn
        if self.udev.as_ref().map(|u| u.paused).unwrap_or(true) {
            return false;
        }
        if let Some(crtc) = crtc {
            self.render_surface(node, crtc)
        } else {
            let crtcs: Vec<_> = self
                .udev
                .as_ref()
                .unwrap()
                .backends
                .get(&node)
                .map(|b| b.surfaces.keys().copied().collect())
                .unwrap_or_default();
            let mut painted = true;
            for crtc in crtcs {
                painted &= self.render_surface(node, crtc);
            }
            painted
        }
    }

    fn render_surface(&mut self, node: DrmNode, crtc: crtc::Handle) -> bool {
        // The snake cannot eat its own tail: one flip in flight per crtc.
        // Rendering while the previous frame is still heading for the panel
        // is what produced 147 fps worth of stutter on a 60 Hz screen.
        let in_flight = self
            .udev
            .as_ref()
            .and_then(|u| u.backends.get(&node))
            .and_then(|b| b.surfaces.get(&crtc))
            .map(|s| s.frame_in_flight)
            .unwrap_or(true);
        if !should_render(true, in_flight) {
            tracing::debug!("render deferred: frame in flight on {crtc:?}");
            if let Some(u) = self.udev.as_mut() {
                u.stats.deferred += 1;
                render_stats_tick(u);
            }
            return false;
        }
        let render_started = Instant::now();
        // Real tiling (M5): placement logic lives in placed_windows, shared with the
        // test fixtures. (The winit loop it once shared with has sailed away.)
        let placed = crate::placed_windows(self);
        // e05s04t3: collect surfaces that will be drawn this frame, so frame
        // callbacks go only to drawn surfaces (not hidden workspaces).
        let drawn_surfaces: Vec<&WlSurface> = placed.iter().map(|(s, _, _, _, _, _)| s).collect();
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
                None => return false,
            };
            let surface = match device_backend.surfaces.get(&crtc) {
                Some(s) => s,
                None => return false,
            };
            let render_node = surface.render_node.unwrap_or(udev.primary_gpu);
            (
                surface.output.clone(),
                render_node,
                surface.drm_output.format(),
                surface.output.current_mode().map(|m| m.size),
            )
        };

        let _time_ms = now(self); // must precede the mutable udev borrow below
        let udev = self.udev.as_mut().unwrap();
        // e08s01t5: renderer acquisition can fail (GPU reset, device hotplug).
        // Panicking here took down the compositor on every frame; log and skip
        // this frame instead so the retry timer can try again later.
        let mut renderer = match if primary_gpu == render_node {
            udev.gpus.single_renderer(&render_node)
        } else {
            udev.gpus.renderer(&primary_gpu, &render_node, format_needed)
        } {
            Ok(renderer) => renderer,
            Err(err) => {
                warn!(?err, ?node, ?crtc, "renderer acquisition failed, skipping frame");
                return false;
            }
        };
        tracing::debug!("render debug2: output_mode={:?} scale={:?} mode_size={:?} pointer=({:.0},{:.0}) cursor_buffer={}",
            mode_size.map(|s| (s.w, s.h)),
            output.current_scale(),
            mode_size,
            udev.pointer_location.x, udev.pointer_location.y,
            udev.cursor_buffer.is_some());

// Build the frame: placed client surfaces + cursor. e08s02t4: elements are
        // collected per band and assembled by `render_band_order` at the end.
        let mut elements: Vec<EswmRenderElements<UdevRenderer<'_>>> = Vec::new();
        let mut cursor_elements: Vec<EswmRenderElements<UdevRenderer<'_>>> = Vec::new();
        let mut fullscreen_elements: Vec<EswmRenderElements<UdevRenderer<'_>>> = Vec::new();

        // Cursor first so it renders on top (smithay renders elements top-to-bottom).
        // e08s03t6: a client-provided surface wins; Hidden removes the pointer;
        // Named keeps the Xcursor-derived buffer fallback.
        let pointer_location = udev.pointer_location;
        let cursor_decision = crate::cursor_render_decision(&udev.cursor_image);
        if cursor_decision == crate::CursorRenderDecision::ClientSurface {
            let surface = match &udev.cursor_image {
                smithay::input::pointer::CursorImageStatus::Surface(surface) => Some(surface.clone()),
                _ => None,
            };
            if let Some(surface) = surface {
                let hotspot = smithay::wayland::compositor::with_states(&surface, |states| {
                    use smithay::input::pointer::CursorImageSurfaceData;
                    states.data_map.get::<CursorImageSurfaceData>().map(|data| data.lock().unwrap().hotspot)
                }).unwrap_or_default();
                let pos = (pointer_location - Point::from((hotspot.x as f64, hotspot.y as f64)))
                    .to_physical(Scale::from(output.current_scale().fractional_scale()))
                    .to_i32_round();
                let tree = render_elements_from_surface_tree(
                    &mut renderer,
                    &surface,
                    pos,
                    1.0,
                    1.0,
                    Kind::Cursor,
                );
                cursor_elements.extend(tree.into_iter().map(EswmRenderElements::Surface));
            }
        } else if cursor_decision == crate::CursorRenderDecision::XcursorFallback
            && let Some((buffer, hotspot)) = udev.cursor_buffer.clone() {
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
                cursor_elements.push(EswmRenderElements::Cursor(cursor));
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
                cursor_elements.push(EswmRenderElements::Cursor(cursor));
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
        // Overlay/top layers are collected in `layer_elements_above` and appended
        // in band order after the frame is assembled (e08s02t4).

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
        let _corner_radius = dec.corner_radius;
        let titlebar_height = dec.titlebar_height;
        drop(dec);
        // Titlebar height and border size are independent: a titlebar can be
        // drawn even when the border is disabled (border_size == 0). The titlebar
        // strip overlaps the top of the client surface by design (the client
        // fills target_rect; its top zone is hidden behind the titlebar).
        if border_size > 0 || titlebar_height > 0 {
            let _bp = border_size as f64;
            for (ws, x, y, id, focused, rect) in &placed {
                // Popups are not wm windows: no server-side border or titlebar.
                // They render as a plain surface tree at the positioner-resolved
                // origin (GTK draws its own menu frame and shadow).
                let is_popup = *id == crate::POPUP_FAKE_ID;
                let mode = self.wm.window(*id).map(|w| w.mode).unwrap_or(wm::ClientMode::tiled());
                // Smithay draws input[0] last (on top), so the focused window
                // (first in `placed`) paints on top overall. Within each window
                // the order is [borders, titlebar, surface]: borders pushed
                // first (index 0 -> drawn last -> on top, frame the window),
                // titlebar next (middle, drawn above the window frame, no
                // overlap with client content), surface pushed last (drawn
                // first -> bottom layer = client content).
                let mut border_elements: Vec<EswmRenderElements<UdevRenderer>> = Vec::new();
                // Borders pushed first -> lowest index -> drawn last -> on top, frame titlebar + surface.
                // Titlebar next (middle, inside frame top, flush with surface top).
                // Surface last (bottom layer = client content).
                // e05s04t4: use SolidColorRenderElement for borders instead of
                // per-frame MemoryRenderBuffer allocations. One element per edge,
                // re-created when color/size changes (no persistent cache needed
                // because SolidColorRenderElement is cheap to construct).
                if !is_popup && mode.presentation != wm::Presentation::Fullscreen && border_size > 0 {
                    let color = if *focused { &color_focused } else { &color_unfocused };
                    let color32f = Color32F::new(
                        color[0] as f32 / 255.0,
                        color[1] as f32 / 255.0,
                        color[2] as f32 / 255.0,
                        color[3] as f32 / 255.0,
                    );
                    // Use shared border_frame helper so all four edges connect with zero gap.
                    let frame = border_frame(*rect, border_size, titlebar_height);

                    // Top edge
                    border_elements.push(EswmRenderElements::Solid(SolidColorRenderElement::new(
                        Id::new(),
                        Rectangle::<i32, Physical>::new(
                            Point::from((frame.top.x, frame.top.y)),
                            Size::from((frame.top.width as i32, frame.top.height as i32)),
                        ),
                        CommitCounter::default(),
                        color32f,
                        Kind::Unspecified,
                    )));
                    // Bottom edge
                    border_elements.push(EswmRenderElements::Solid(SolidColorRenderElement::new(
                        Id::new(),
                        Rectangle::<i32, Physical>::new(
                            Point::from((frame.bottom.x, frame.bottom.y)),
                            Size::from((frame.bottom.width as i32, frame.bottom.height as i32)),
                        ),
                        CommitCounter::default(),
                        color32f,
                        Kind::Unspecified,
                    )));
                    // Left edge
                    border_elements.push(EswmRenderElements::Solid(SolidColorRenderElement::new(
                        Id::new(),
                        Rectangle::<i32, Physical>::new(
                            Point::from((frame.left.x, frame.left.y)),
                            Size::from((frame.left.width as i32, frame.left.height as i32)),
                        ),
                        CommitCounter::default(),
                        color32f,
                        Kind::Unspecified,
                    )));
                    // Right edge
                    border_elements.push(EswmRenderElements::Solid(SolidColorRenderElement::new(
                        Id::new(),
                        Rectangle::<i32, Physical>::new(
                            Point::from((frame.right.x, frame.right.y)),
                            Size::from((frame.right.width as i32, frame.right.height as i32)),
                        ),
                        CommitCounter::default(),
                        color32f,
                        Kind::Unspecified,
                    )));
                }
                // Borders pushed first -> index 0 -> drawn last -> on top (capture titlebar edges).
                elements.extend(border_elements);
                // Titlebar strip (middle layer: on top of surface, under borders).
                if !is_popup && titlebar_height > 0 && mode.presentation != wm::Presentation::Fullscreen {
                    let tb_w = rect.width;
                    let tb_h = titlebar_height;
                    let tb_px = if *focused { titlebar_color_focused } else { titlebar_color_unfocused };
                    let tb_px = [tb_px[0], tb_px[1], tb_px[2], tb_px[3]];
                    // Button elements collected first so they draw on top of
                    // the titlebar background (smithay draws index 0 last).
                    let mut button_elements: Vec<EswmRenderElements<UdevRenderer>> = Vec::new();
                    let btn_size = tb_h.saturating_sub(2);
                    let btn_gap = 4u32;
                    let mut btn_x = (rect.x as f64 + rect.width as f64) - (btn_size + btn_gap) as f64;
                    // e05s04t6: buttons are solid-color squares — use
                    // SolidColorRenderElement instead of allocating a
                    // MemoryRenderBuffer every frame.
                    let mut draw_button = |bx: f64, by: f64, bcol: [u8; 4], bsize: u32| {
                        let color32f = Color32F::new(
                            bcol[0] as f32 / 255.0,
                            bcol[1] as f32 / 255.0,
                            bcol[2] as f32 / 255.0,
                            bcol[3] as f32 / 255.0,
                        );
                        button_elements.push(EswmRenderElements::Solid(SolidColorRenderElement::new(
                            Id::new(),
                            Rectangle::<i32, Physical>::new(
                                Point::from((bx as i32, by as i32)),
                                Size::from((bsize as i32, bsize as i32)),
                            ),
                            CommitCounter::default(),
                            color32f,
                            Kind::Unspecified,
                        )));
                    };
                    draw_button(btn_x, rect.y as f64 - titlebar_height as f64 + 1.0, [255, 0, 0, 255], btn_size); // close
                    btn_x -= btn_size as f64 + btn_gap as f64;
                    draw_button(btn_x, rect.y as f64 - titlebar_height as f64 + 1.0, [200, 200, 0, 255], btn_size); // maximize
                    btn_x -= btn_size as f64 + btn_gap as f64;
                    draw_button(btn_x, rect.y as f64 - titlebar_height as f64 + 1.0, [100, 100, 100, 255], btn_size); // minimize
                    // Titlebar background buffer - cached by (window_id, rect, color, titlebar_height)
                    let tb_buf = {
                        let tb_cache_key = (*id, *rect, tb_px, tb_h);
                        udev.titlebar_buffer_cache.entry(tb_cache_key).or_insert_with(|| {
                            let mut tb_pix = Vec::with_capacity((tb_w * tb_h * 4) as usize);
                            for _ in 0..(tb_w * tb_h) {
                                tb_pix.extend_from_slice(&tb_px);
                            }
                            MemoryRenderBuffer::from_slice(
                                &tb_pix, Fourcc::Argb8888, (tb_w as i32, tb_h as i32), 1, Transform::Normal, None,
                            )
                        }).clone()
                    };
                    // Borders pushed earlier are at low index (on top). Buttons next,
                    // then titlebar background last (below buttons, above surface).
                    elements.extend(button_elements);
                    if let Ok(elem) = MemoryRenderBufferRenderElement::from_buffer(
                        &mut renderer,
                        Point::from((rect.x as f64, rect.y as f64 - titlebar_height as f64)),
                        &tb_buf,
                        None, None, None, Kind::Unspecified,
                    ) {
                        elements.push(EswmRenderElements::Cursor(elem));
                    }
                }
                // Always render the window surface tree. e08s02t4: a fullscreen
                // window's tree goes to its own band so it can paint above all
                // layers and the dock.
                //
                // BUG-2026-11-22-OVERSIZE-RETRY-CAP follow-up: clients that
                // ignore the size configure commit a buffer larger than the
                // frame. Clipping keeps their content inside the border.
                let tree_elements: Vec<WaylandSurfaceRenderElement<_>> =
                    render_elements_from_surface_tree(
                        &mut renderer,
                        ws,
                        (*x, *y),
                        1.0,
                        1.0,
                        Kind::Unspecified,
                    );
                let tree_target = if matches!(
                    self.wm.window(*id).map(|w| w.mode.presentation),
                    Some(wm::Presentation::Fullscreen)
                ) {
                    &mut fullscreen_elements
                } else {
                    &mut elements
                };
                if *id == crate::POPUP_FAKE_ID {
                    // Popups own their shadows; let them paint past the frame.
                    tree_target.extend(
                        tree_elements.into_iter().map(EswmRenderElements::Surface),
                    );
                } else {
                    let clip = Rectangle::new(
                        Point::from((rect.x, rect.y)),
                        Size::from((rect.width as i32, rect.height as i32)),
                    );
                    let scale = Scale::from(output.current_scale().fractional_scale());
                    tree_target.extend(tree_elements.into_iter().map(|elem| {
                        EswmRenderElements::ClippedSurface(
                            ClippedSurfaceElement::new(elem, clip, scale),
                        )
                    }));
                }
            }
        } else {
            for (ws, x, y, id, _, rect) in &placed {
                let tree_elements: Vec<WaylandSurfaceRenderElement<_>> =
                    render_elements_from_surface_tree(
                        &mut renderer,
                        ws,
                        (*x, *y),
                        1.0,
                        1.0,
                        Kind::Unspecified,
                    );
                let tree_target = if matches!(
                    self.wm.window(*id).map(|w| w.mode.presentation),
                    Some(wm::Presentation::Fullscreen)
                ) {
                    &mut fullscreen_elements
                } else {
                    &mut elements
                };
                if *id == crate::POPUP_FAKE_ID {
                    tree_target.extend(
                        tree_elements.into_iter().map(EswmRenderElements::Surface),
                    );
                } else {
                    let clip = Rectangle::new(
                        Point::from((rect.x, rect.y)),
                        Size::from((rect.width as i32, rect.height as i32)),
                    );
                    let scale = Scale::from(output.current_scale().fractional_scale());
                    tree_target.extend(tree_elements.into_iter().map(|elem| {
                        EswmRenderElements::ClippedSurface(
                            ClippedSurfaceElement::new(elem, clip, scale),
                        )
                    }));
                }
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
        if dock_height > 0
            && let Some(size) = mode_size {
                let loc = output.current_location();
                let entries = self.wm.dock_entries();
                let n = entries.len() as u32;
                tracing::debug!("dock debug: drawing dock at y={} h={} n={} size={}x{} loc={:?}", loc.y + size.h - dock_height as i32, dock_height, n, size.w, size.h, loc);
                if n > 0 {
                    let dock_y = self.wm.work_area
                        .map(|wa| (wa.y + wa.height as i32) as f64)
                        .unwrap_or((loc.y + size.h - dock_height as i32) as f64);
                    let dock_h = dock_height as i32;
                    let total_w = size.w;
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
                            // e08s01t2: use saturating helper — subtraction used
                            // to underflow and panic on tiny dock heights.
                            let (seg_w_in, seg_h) = dock_inner_size(w, dock_h);
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
                            let layout = dock_segment_layout(seg_w_in, seg_h);
                            let s = layout.scale;
                            let ox = layout.offset_x;
                            let oy = layout.offset_y;
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
                            if let Ok(elem) = MemoryRenderBufferRenderElement::from_buffer(
                                &mut renderer,
                                pos,
                                buf,
                                None, None, None, Kind::Unspecified,
                            ) {
                                dock_elements.push(EswmRenderElements::Cursor(elem));
                            }
                        }
                    }
                }
            }
        // e08s02t4: assemble bands in the policy order. Index 0 draws last
        // (on top): cursor, fullscreen, overlay/top, dock, windows, background.
        let fullscreen_active = placed.iter().any(|(_, _, _, id, _, _)| {
            matches!(self.wm.window(*id).map(|w| w.mode.presentation), Some(wm::Presentation::Fullscreen))
        });
        let mut frame_elements: Vec<EswmRenderElements<UdevRenderer<'_>>> = Vec::new();
        for band in render_band_order(fullscreen_active) {
            match band {
                RenderBand::Cursor => frame_elements.append(&mut cursor_elements),
                RenderBand::Fullscreen => frame_elements.append(&mut fullscreen_elements),
                RenderBand::OverlayTop => frame_elements.append(&mut layer_elements_above),
                RenderBand::Dock => frame_elements.append(&mut dock_elements),
                RenderBand::Windows => frame_elements.append(&mut elements),
                RenderBand::Background => frame_elements.append(&mut layer_elements_below),
            }
        }
        elements = frame_elements;

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
            None => return false,
        };
        let surface = match device_backend.surfaces.get_mut(&crtc) {
            Some(s) => s,
            None => return false,
        };

        tracing::debug!("render debug: output.current_location={:?} output.current_transform={:?} output.current_scale={:?} output.current_mode={:?}",
            output.current_location(), output.current_transform(), output.current_scale(), output.current_mode());
        let _ = mode_size;

        let prep_started = Instant::now();
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

        let prep_ms = prep_started.elapsed().as_secs_f64() * 1000.0;

        let mut sent_callbacks = false;
        let mut queued_ok = false;
        let mut stat_frame = false;
        let mut stat_empty = false;
        let mut stat_ms = 0.0f64;
        let mut stat_queue_ms = 0.0f64;
        let mut stat_queue_err = false;
        let reschedule = match result {
            Ok((_is_empty, _states)) => {
                // Only tell clients "your frame is on screen" when a flip
                // actually happened; a callback for unchanged content invites a
                // pointless commit.
                sent_callbacks = false;
                // queue_frame is what page-flips the prepared buffer, and it is
                // also what applies cursor-plane-only updates. Never skip it on
                // the strength of render_frame's is_empty hint: a pointer-only
                // change has no primary-plane damage yet still moves the cursor,
                // and skipping the flip made the cursor vanish.
                let mut empty_error = false;
                let queue_started = Instant::now();
                let queued = match surface.drm_output.queue_frame(None::<OutputPresentationFeedback>) {
                    Ok(()) => {
                        surface.frame_in_flight = true;
                        surface.queued_at = Some(queue_started);
                        true
                    }
                    Err(err) => {
                        // EmptyFrame is benign: nothing changed, so there is
                        // nothing to present. Any other error deserves a retry.
                        if matches!(err, FrameError::EmptyFrame) {
                            empty_error = true;
                        } else {
                            stat_queue_err = true;
                            debug!("queue_frame: {err:?}");
                        }
                        false
                    }
                };
                stat_queue_ms = queue_started.elapsed().as_secs_f64() * 1000.0;
                let outcome = frame_outcome(empty_error, queued);
                if queued {
                    stat_frame = true;
                    stat_ms = render_started.elapsed().as_secs_f64() * 1000.0;
                    // The flip is queued, so the buffer is committed: this is
                    // the moment the client may render its next frame while
                    // ours is in flight (Hyprland sends callbacks here too).
                    sent_callbacks = true;
                    tracing::debug!("render: {} elements, queued (placed={})", elements.len(), placed.len());
                } else if empty_error {
                    stat_empty = true;
                }
                queued_ok = outcome.satisfied();
                // Retry only on a real failure; an empty frame is already done.
                !outcome.satisfied()
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
                        _ => exit_on_context_lost(&err),
                    },
                }
            }
        };

        // frame callbacks were previously sent inline; the udev mutable borrow
        // (needed for queue_frame) is live here, so send after the match instead.
        if sent_callbacks {
            let gap = output
                .current_mode()
                .map(|m| callback_min_gap_ms(m.refresh as u32))
                .unwrap_or(16);
            self.frame_callbacks(gap, drawn_surfaces.iter().copied());
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
                        if !data.redraw_needed {
                            return TimeoutAction::Drop;
                        }
                        if data.render(node, Some(crtc), now(data)) {
                            data.redraw_needed = false;
                        }
                        TimeoutAction::Drop
                    },
                )
                .expect("failed to schedule damage test timer");
        }

        if let Some(u) = self.udev.as_mut() {
            if stat_frame {
                u.stats.record_frame(stat_ms, prep_ms, stat_queue_ms);
                // How long did the user wait for this frame? A commit that
                // arrived just before the present waited a millisecond; one
                // that sat through the whole gap is the stall we care about.
                let presented = Instant::now();
                let commit_wait = u.last_commit_at.take().map(|at| latency_ms(at, presented));
                if let Some(ms) = commit_wait {
                    u.stats.record_latency(ms);
                    if is_hitch(commit_wait) {
                        warn!("presented {ms:.0} ms after commit (late)");
                    }
                }
                // Input that waited is a frozen cursor / unresponsive keys,
                // which is a different failure from a late client frame.
                let input_wait = u.last_input_at.take().map(|at| latency_ms(at, presented));
                if is_hitch(input_wait) {
                    let ms = input_wait.unwrap_or_default();
                    warn!("presented {ms:.0} ms after input (late)");
                }
                // Keep the gap for the stats line; it is only a hitch if one of
                // the waits above says something was actually queued.
                if let Some(gap) = u.stats.record_present(presented) {
                    if is_hitch(commit_wait) || is_hitch(input_wait) {
                        warn!("frame gap {gap:.0} ms had work waiting");
                    } else {
                        debug!("frame gap {gap:.0} ms (idle)");
                    }
                }
            }
            if stat_empty {
                u.stats.empty += 1;
            }
            if stat_queue_err {
                u.stats.queue_err += 1;
            }
            render_stats_tick(u);
        }

        queued_ok
    }

    // e05s04t3: send frame callbacks only to surfaces drawn this frame, paced by
    // the panel refresh (see callback_min_gap_ms).
    fn frame_callbacks<'a, I>(&mut self, min_gap_ms: u32, surfaces: I)
    where
        I: Iterator<Item = &'a WlSurface>,
    {
        // One clock, always. render() takes its timestamp from the caller, and
        // the two callers use different epochs: the event loop passes ms since
        // start while the vblank timer passes the DRM event's ms since boot.
        // Mixing them made the pacing gate read 0 and drop every callback, so
        // animated clients went quiet and then burst. Derive the time here from
        // the same clock that stamps last_frame_callback_ms.
        // Two clocks, two opinions on what time it is, and the client pays for
        // the argument. One clock from now on.
        let time = now(self);
        let elapsed = time.saturating_sub(self.last_frame_callback_ms);
        if !callback_due(elapsed, min_gap_ms) {
            return;
        }
        self.last_frame_callback_ms = time;
        for surface in surfaces {
            send_frames_surface_tree(surface, time);
        }
        for surface in self.layer_shell_state.layer_surfaces() {
            send_frames_surface_tree(surface.wl_surface(), time);
        }
    }
}

struct SurfaceData {
    dh: DisplayHandle,
    render_node: Option<DrmNode>,
    global: Option<GlobalId>,
    output: Output,
    drm_output: DrmOutput<
        GbmAllocator<DrmDeviceFd>,
        GbmFramebufferExporter<DrmDeviceFd>,
        Option<OutputPresentationFeedback>,
        DrmDeviceFd,
    >,
    last_presentation_time: Option<Duration>,
    vblank_throttle_timer: Option<RegistrationToken>,
    /// True between queue_frame and the matching vblank (see frame_finish).
    frame_in_flight: bool,
    /// When queue_frame accepted the flip; the vblank latency is measured from
    /// here so a slow flip can be blamed on the kernel rather than on us.
    queued_at: Option<Instant>,
}

impl Drop for SurfaceData {
    fn drop(&mut self) {
        if let Some(global) = self.global.take() {
            self.dh.remove_global::<State>(global);
        }
    }
}

pub(crate) struct BackendData {
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
#[allow(dead_code)] // payloads exist to make hotplug failure logs useful
enum DeviceAddError {
    DeviceOpen(String),
    DrmDevice(DrmError),
    GbmDevice(std::io::Error),
    DrmNode(CreateDrmNodeError),
    AddNode(egl::Error),
    NoRenderNode,
    PrimaryGpuMissing,
    Renderer(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use smithay::backend::allocator::Format;

    #[test]
    fn idle_frame_skips_render_timer() {
        assert!(render_timer_delay(false, Duration::from_millis(16)).is_none());
    }

    #[test]
    fn active_frame_schedules_render_timer() {
        // A paint must start right after vblank: a 0.6-frame head start for
        // clients pushed our ~15ms software repaint past the next vblank, so
        // every other frame was dropped (measured 26fps on a 60Hz panel).
        let delay = render_timer_delay(true, Duration::from_millis(16));
        assert_eq!(delay, Some(Duration::ZERO));
    }

    // BUG-2026-11-17T00:30:00 — at most one frame in flight per output.
    // Measured 147 renders/s on a 60 Hz panel before this gate existed.
    #[test]
    fn should_render_truth_table() {
        assert!(!should_render(false, false), "idle, nothing in flight");
        assert!(should_render(true, false), "pending redraw, free to paint");
        assert!(!should_render(true, true), "pending redraw but a flip is in flight");
        assert!(!should_render(false, true), "flip in flight, nothing pending");
    }

    /// BUG-2026-11-17T00:30:00 — an empty frame means "nothing changed", not
    /// "paint failed". Treating it as failure kept redraw_needed set, so the
    /// loop re-rendered empty frames back to back (317 in 5s, one every
    /// 0.4ms) and each one armed another retry timer. That is the spin that
    /// froze the desktop until the pointer moved.
    #[test]
    fn empty_frame_is_satisfied_not_retried() {
        assert_eq!(frame_outcome(true, false), FrameOutcome::NothingToDo);
        assert_eq!(frame_outcome(false, true), FrameOutcome::Presented);
        assert_eq!(frame_outcome(false, false), FrameOutcome::Retry);
    }

    #[test]
    fn satisfied_outcomes_consume_the_redraw_request() {
        assert!(frame_outcome(true, false).satisfied(), "empty is done");
        assert!(frame_outcome(false, true).satisfied(), "presented is done");
        assert!(!frame_outcome(false, false).satisfied(), "real error retries");
    }

    /// Frame callbacks must be paced by the panel, not by a hard 33ms: on a
    /// 60Hz output that halves every callback-driven animation to 30fps, which
    /// reads as skipped frames. 60001 mHz is a 16.6ms period.
    #[test]
    fn frame_callback_gap_follows_refresh_rate() {
        assert_eq!(callback_min_gap_ms(60_001), 16);
        assert_eq!(callback_min_gap_ms(143_981), 6);
        assert_eq!(callback_min_gap_ms(0), 16, "unknown refresh falls back to 60Hz");
    }

    /// A cursor-plane-only update can still be worth flipping even when the
    /// damage tracker reported nothing for the primary plane. `queue_frame`
    /// succeeding is the authority here; the caller's `is_empty` hint is not.
    #[test]
    fn cursor_only_update_still_presents() {
        assert_eq!(frame_outcome(true, true), FrameOutcome::Presented);
    }

    /// The lag the user feels lives in the outliers, not the mean: track the
    /// slowest frame and how often a frame misses the 16.7ms budget, plus where
    /// the time goes (render vs DRM submission).
    #[test]
    fn render_stats_track_outliers_and_breakdown() {
        let mut s = RenderStats::default();
        s.record_frame(8.0, 5.0, 1.0);
        s.record_frame(25.0, 15.0, 6.0);
        assert_eq!(s.frames, 2);
        assert_eq!(s.max_ms, 25.0);
        assert_eq!(s.slow, 1, "25ms misses a 60Hz vblank");
        assert_eq!(s.ms, 33.0);
        assert_eq!(s.prep_ms, 20.0);
        assert_eq!(s.queue_ms, 7.0);
    }

    /// A "whole screen hitch" is a gap *between* frames, not a slow frame: every
    /// individual render can be 1ms and the screen still freezes for 200ms while
    /// the loop is blocked or a flip is refused. Track the gap, keep the worst.
    #[test]
    fn render_stats_measure_frame_gaps() {
        let mut s = RenderStats::default();
        let t0 = Instant::now();
        assert_eq!(s.record_present(t0), None, "first frame has no gap");
        let gap = s.record_present(t0 + Duration::from_millis(16)).unwrap();
        assert!((gap - 16.0).abs() < 0.01);
        assert!((s.max_gap_ms - 16.0).abs() < 0.01);
        // A hitch shows up as a long gap; max keeps the worst one seen.
        s.record_present(t0 + Duration::from_millis(216));
        assert!((s.max_gap_ms - 200.0).abs() < 0.01);
        s.record_present(t0 + Duration::from_millis(232));
        assert!((s.max_gap_ms - 200.0).abs() < 0.01, "max never shrinks");
    }

    /// A hitch has two possible owners: the compositor held the frame, or the
    /// kernel/GPU held the flip. Comparing queue-to-vblank time against render
    /// time says which one owes the user an apology.
    #[test]
    fn render_stats_track_flip_latency() {
        let mut s = RenderStats::default();
        assert_eq!(s.flip_max_ms, 0.0);
        s.record_flip(16.0);
        s.record_flip(4.0);
        assert_eq!(s.flip_max_ms, 16.0);
        s.record_flip(320.0);
        assert_eq!(s.flip_max_ms, 320.0);
    }

    #[test]
    fn a_hitch_needs_something_that_waited() {
        assert!(!is_hitch(None), "no commit and no input: an idle screen");
        assert!(!is_hitch(Some(2.0)), "commit arrived right before the present");
        assert!(is_hitch(Some(1500.0)), "a commit waited 1.5s");
    }

    /// The frame gap cannot tell an idle screen from a dropped frame. Latency
    /// from the client's commit to the present can: it is the wait the user
    /// experiences after pressing a key.
    #[test]
    fn render_stats_track_commit_to_present_latency() {
        let mut s = RenderStats::default();
        s.record_latency(12.0);
        s.record_latency(400.0);
        s.record_latency(8.0);
        assert_eq!(s.lat_max_ms, 400.0);
        let t0 = Instant::now();
        let ms = latency_ms(t0, t0 + Duration::from_millis(25));
        assert!((ms - 25.0).abs() < 0.01);
    }

    /// A frame callback is the client's permission to draw. Dropping one does
    /// not delay the client, it *blocks* it: the client has committed its frame
    /// and has nothing else to do, so it stops committing. With no commits
    /// there is no damage, so no flip happens, so no callback is ever delivered
    /// — a deadlock that freezes the whole screen (cursor plane included, since
    /// the pointer only moves when a flip commits) until unrelated input forces
    /// a present. Back-to-back presents must both deliver.
    #[test]
    fn every_present_delivers_pending_callbacks() {
        assert!(callback_due(0, 16), "same-millisecond present still owes");
        assert!(callback_due(5, 16), "early present still owes");
        assert!(callback_due(16, 16));
        assert!(callback_due(1_000, 16));
    }

    #[test]
    fn loop_stall_only_counts_blocked_work() {
        assert!(!loop_stall(false, 500.0), "idle blocking is not a hitch");
        assert!(!loop_stall(true, 16.0), "waiting for a vblank is normal");
        assert!(loop_stall(true, 120.0));
        assert!((flip_latency_ms(
            Instant::now(),
            Instant::now() + Duration::from_millis(16)
        ) - 16.0)
            .abs()
            < 0.01);
    }

    /// Smithay's EGL wl_drm reader is a process-wide singleton. A second bind
    /// warns and silently replaces the first — a compositor trying to wear two
    /// hats and getting a stern letter from smithay about it.
    #[test]
    fn bind_wl_display_once_stops_after_first_success() {
        let mut attempts = Vec::new();
        let usable = bind_wl_display_once(&[1, 2, 3], |node| {
            attempts.push(node);
            node != 1
        });
        assert_eq!(attempts, [1, 2], "no bind attempts after the first success");
        assert_eq!(usable, [2, 3], "nodes after the bound one remain usable");
    }

    #[test]
    fn bind_wl_display_once_tries_every_node_when_all_fail() {
        let mut attempts = Vec::new();
        let usable = bind_wl_display_once(&[1, 2, 3], |node| {
            attempts.push(node);
            false
        });
        assert_eq!(attempts, [1, 2, 3], "every node is tried while no bind succeeds");
        assert!(usable.is_empty(), "a failed bind leaves no usable GPU");
    }

    #[test]
    fn bind_wl_display_once_accepts_empty_candidates() {
        assert!(bind_wl_display_once(&[] as &[u8], |_| true).is_empty());
    }

    /// Clients allocate dmabufs on the GPU the feedback advertises. If that is
    /// not the GPU that scans out, every client frame crosses GPUs or lands in
    /// the cpu-copy fallback — measured here as 6-15ms per frame in
    /// render_frame while the DRM submit took 0.4ms.
    #[test]
    fn dmabuf_gpu_follows_the_scanout_gpu() {
        // CI containers have no /dev/dri, so the nodes cannot be constructed.
        // Skip there rather than fail: the selection logic is pure and runs on
        // any host that has a DRM device (this used to be the only red test).
        let (Ok(intel), Ok(nvidia)) = (
            DrmNode::from_dev_id(libc::makedev(226, 0) as u64),
            DrmNode::from_dev_id(libc::makedev(226, 1) as u64),
        ) else {
            return;
        };
        // Hybrid laptop: Intel drives eDP-1, but primary_gpu() picked NVIDIA.
        assert_eq!(choose_dmabuf_gpu(Some(intel), &[nvidia], nvidia), intel);
        // No output up yet: first hw-accel GPU, then the primary as last resort.
        assert_eq!(choose_dmabuf_gpu(None, &[nvidia], intel), nvidia);
        assert_eq!(choose_dmabuf_gpu(None, &[], intel), intel);
    }

    #[test]
    fn deferred_redraw_keeps_pending_flag() {
        // (render_now, keep_pending)
        assert_eq!(next_redraw_state(true, true), (false, true), "defer, keep the request");
        assert_eq!(next_redraw_state(true, false), (true, false), "paint, consume the request");
        assert_eq!(next_redraw_state(false, false), (false, false), "nothing to do");
        assert_eq!(next_redraw_state(false, true), (false, false), "flip in flight, nothing pending");
    }

    /// Super+Alt+Backspace must always trigger quit (built-in, config-independent).
    #[test]
    fn quit_keybind_matches_super_alt_backspace() {
        let mods = ModifiersState {
            logo: true,
            alt: true,
            ..Default::default()
        };
        let raw = xkbcommon::xkb::keysyms::KEY_BackSpace;
        let kb = builtin_shortcut(&mods, raw).expect("Super+Alt+Backspace should always quit");
        assert_eq!(kb.action, "quit");
        // Wrong modifiers must not trigger.
        let wrong = ModifiersState {
            logo: true,
            ..Default::default()
        };
        assert!(builtin_shortcut(&wrong, raw).is_none());
    }


    #[test]
    fn keybind_match_uses_stored_keysym_for_shifted_number() {
        let kb = lua_state::Keybind {
            mods: vec!["logo".into(), "shift".into()],
            key: "1".into(),
            keysym: xkbcommon::xkb::keysyms::KEY_1,
            arg: Some(1),
            cmd: None,
            action: "move_window_to_workspace".into(),
        };
        let mods = ModifiersState {
            logo: true,
            shift: true,
            ..Default::default()
        };
        // Raw sym for the physical '1' key remains KEY_1 even when shift is held.
        let matched = find_lua_keybind(std::slice::from_ref(&kb), &mods, xkbcommon::xkb::keysyms::KEY_1);
        assert!(matched.is_some(), "Super+Shift+1 should match stored KEY_1");
    }

    #[test]
    fn keybind_match_ignores_caps_and_num_lock() {
        let kb = lua_state::Keybind {
            mods: vec!["logo".into()],
            key: "r".into(),
            keysym: xkbcommon::xkb::keysyms::KEY_r,
            arg: None,
            cmd: None,
            action: "spawn_terminal".into(),
        };
        let mods = ModifiersState {
            logo: true,
            caps_lock: true,
            num_lock: true,
            ..Default::default()
        };
        let matched = find_lua_keybind(std::slice::from_ref(&kb), &mods, xkbcommon::xkb::keysyms::KEY_r);
        assert!(matched.is_some(), "Caps/Num Lock must not break keybind matching");
    }



    #[test]
    fn emergency_quit_super_alt_backspace_is_builtin() {
        let mods = ModifiersState {
            logo: true,
            alt: true,
            ..Default::default()
        };
        let kb = builtin_shortcut(&mods, xkbcommon::xkb::keysyms::KEY_BackSpace)
            .expect("Super+Alt+Backspace must be built-in quit");
        assert_eq!(kb.action, "quit");
        // Must not depend on config; wrong modifiers must not trigger.
        let wrong = ModifiersState {
            logo: true,
            ..Default::default()
        };
        assert!(builtin_shortcut(&wrong, xkbcommon::xkb::keysyms::KEY_BackSpace).is_none());
    }

    #[test]
    fn emergency_quit_ctrl_alt_f_keys_switch_vt_builtin() {
        let mods = ModifiersState {
            ctrl: true,
            alt: true,
            ..Default::default()
        };
        let kb = builtin_shortcut(&mods, xkbcommon::xkb::keysyms::KEY_XF86Switch_VT_1)
            .expect("Ctrl+Alt+F1 must be built-in VT switch");
        assert_eq!(kb.action, "change_vt");
        assert_eq!(kb.arg, Some(1));
        let kb12 = builtin_shortcut(&mods, xkbcommon::xkb::keysyms::KEY_XF86Switch_VT_12)
            .expect("Ctrl+Alt+F12 must be built-in VT switch");
        assert_eq!(kb12.arg, Some(12));
        // Ctrl+Alt alone on non-VT key must not trigger.
        assert!(builtin_shortcut(&mods, xkbcommon::xkb::keysyms::KEY_a).is_none());
    }

    #[test]
    fn emergency_quit_builtin_ignores_caps_num_lock() {
        let mods = ModifiersState {
            logo: true,
            alt: true,
            caps_lock: true,
            num_lock: true,
            ..Default::default()
        };
        let kb = builtin_shortcut(&mods, xkbcommon::xkb::keysyms::KEY_BackSpace)
            .expect("Caps/Num Lock must not break emergency quit");
        assert_eq!(kb.action, "quit");
    }



    #[test]
    fn intercept_release_consumed_press_intercepts_matching_release() {
        let mut intercepted = std::collections::HashSet::new();
        let key_code = 42u32;

        let press = key_filter_result(&mut intercepted, key_code, true, || FilterResult::Intercept(()));
        assert!(matches!(press, FilterResult::Intercept(())));
        assert!(intercepted.contains(&key_code));

        let release = key_filter_result(&mut intercepted, key_code, false, || FilterResult::Forward);
        assert!(matches!(release, FilterResult::Intercept(())));
        assert!(!intercepted.contains(&key_code), "release must clear intercepted key");
    }

    #[test]
    fn intercept_release_forwarded_press_does_not_intercept_release() {
        let mut intercepted = std::collections::HashSet::new();
        let key_code = 7u32;

        let press = key_filter_result(&mut intercepted, key_code, true, || FilterResult::Forward);
        assert!(matches!(press, FilterResult::Forward));
        assert!(intercepted.is_empty());

        let release = key_filter_result(&mut intercepted, key_code, false, || FilterResult::Forward);
        assert!(matches!(release, FilterResult::Forward));
    }

    // e08s01t4 — connectors reporting no modes must be skipped, not indexed.
    #[test]
    fn preferred_mode_index_none_for_empty_modes() {
        assert_eq!(preferred_mode_index(0, |_| false), None, "no modes -> skip connector");
    }

    #[test]
    fn preferred_mode_index_prefers_flagged_mode() {
        assert_eq!(preferred_mode_index(3, |i| i == 2), Some(2));
        assert_eq!(preferred_mode_index(3, |_| false), Some(0), "fallback to first mode");
    }

    // e08s01t1 — dock segment/layout math must never underflow or panic,
    // including absurdly small docks and many windows. Integers are
    // surprisingly good at finding the floor when nobody gives them a handrail.
    #[test]
    fn dock_segment_layout_never_underflows_for_edge_sizes() {
        for dock_h in 0..=10i32 {
            for n in 1..=20u32 {
                let total_w = 1920i32;
                let seg_w = total_w / n as i32;
                for i in 0..n {
                    let w = if i == n.saturating_sub(1) {
                        total_w.saturating_sub(seg_w * (n.saturating_sub(1) as i32))
                    } else {
                        seg_w
                    };
                    let (seg_w_in, seg_h) = dock_inner_size(w, dock_h);
                    let layout = dock_segment_layout(seg_w_in, seg_h);
                    // Offsets and glyph geometry must stay inside the segment.
                    if layout.scale > 0 {
                        assert!(layout.offset_x + layout.glyph_width <= seg_w_in);
                        assert!(layout.offset_y + layout.glyph_height <= seg_h);
                    }
                    assert!(layout.scale <= 3);
                }
            }
        }
    }

    #[test]
    fn dock_inner_size_clamps_to_zero_for_tiny_docks() {
        assert_eq!(dock_inner_size(1, 0), (0, 0));
        assert_eq!(dock_inner_size(1, 1), (0, 0));
        assert_eq!(dock_inner_size(1, 2), (0, 1));
        assert_eq!(dock_inner_size(100, 7), (98, 6));
        assert_eq!(dock_inner_size(-5, -5), (0, 0));
    }

    #[test]
    fn dock_segment_layout_keeps_hint_scale_in_bounds() {
        // Normal-size segment: scale clamps to 3 and glyph fits.
        let l = dock_segment_layout(60, 40);
        assert_eq!(l.scale, 3);
        assert_eq!(l.glyph_width, 15);
        assert_eq!(l.glyph_height, 21);
        assert!(l.offset_x + l.glyph_width <= 60);
        assert!(l.offset_y + l.glyph_height <= 40);

        // Tiny segment: scale collapses to 0, no glyph drawn, no underflow.
        let tiny = dock_segment_layout(2, 1);
        assert_eq!(tiny.scale, 0);
        assert_eq!(tiny.glyph_width, 0);
        assert_eq!(tiny.glyph_height, 0);
    }

    // e08s02t1 — close button must never orphan the client: with no toplevel
    // to send close to, leaving the WM window alone is the only safe action.
    #[test]
    fn titlebar_close_without_toplevel_leaves_window_untouched() {
        assert_eq!(
            titlebar_close_action(false),
            TitlebarCloseAction::LeaveWindowUntouched,
            "no toplevel -> do not remove the WM window"
        );
    }

    #[test]
    fn titlebar_close_with_toplevel_requests_client_close() {
        assert_eq!(
            titlebar_close_action(true),
            TitlebarCloseAction::RequestClientClose,
            "a live toplevel must get xdg_toplevel.close"
        );
    }

    // e08s02t3 — z-order is a policy decision, tested without a GPU.
    #[test]
    fn render_order_puts_overlay_above_dock() {
        let order = render_band_order(false);
        let overlay = order.iter().position(|b| *b == RenderBand::OverlayTop).unwrap();
        let dock = order.iter().position(|b| *b == RenderBand::Dock).unwrap();
        assert!(overlay < dock, "overlay/top layers must paint above the dock");
    }

    #[test]
    fn render_order_puts_fullscreen_above_layers_and_dock() {
        let order = render_band_order(true);
        let fullscreen = order.iter().position(|b| *b == RenderBand::Fullscreen).unwrap();
        for other in [RenderBand::OverlayTop, RenderBand::Dock, RenderBand::Windows, RenderBand::Background] {
            let pos = order.iter().position(|b| *b == other).unwrap();
            assert!(fullscreen < pos, "fullscreen must paint above {other:?}");
        }
        let cursor = order.iter().position(|b| *b == RenderBand::Cursor).unwrap();
        assert!(cursor < fullscreen, "cursor stays topmost overall");
    }

    #[test]
    fn render_order_without_fullscreen_has_no_fullscreen_band() {
        assert!(!render_band_order(false).contains(&RenderBand::Fullscreen));
    }

    // BUG-2026-11-21-DMABUF-IMPORT — cross-GPU feedback must advertise only
    // modifiers that can actually be copied between the two devices.
    #[test]
    fn dmabuf_feedback_cross_device_render_tranche_is_linear_only() {
        let scanout: FormatSet = [
            Format { code: Fourcc::Abgr8888, modifier: Modifier::Linear },
            Format { code: Fourcc::Xrgb8888, modifier: Modifier::Linear },
        ]
        .into_iter()
        .collect();
        let render: FormatSet = [
            Format { code: Fourcc::Abgr8888, modifier: Modifier::Linear },
            Format { code: Fourcc::Abgr8888, modifier: Modifier::from(0x1234) },
        ]
        .into_iter()
        .collect();

        let (scanout_formats, render_tranche) = dmabuf_feedback_formats(&scanout, &render, true);
        // The scanout tranche advertises the scanout device's own formats.
        assert_eq!(scanout_formats.iter().count(), 2);
        // Only Linear modifiers can be copied across devices.
        assert_eq!(render_tranche.iter().count(), 1);
        assert!(render_tranche.iter().all(|f| f.modifier == Modifier::Linear));
    }

    #[test]
    fn dmabuf_feedback_same_device_has_no_extra_tranche() {
        let formats: FormatSet = [Format { code: Fourcc::Abgr8888, modifier: Modifier::from(0x1234) }].into_iter().collect();
        let (scanout_formats, render_tranche) = dmabuf_feedback_formats(&formats, &formats, false);
        assert_eq!(scanout_formats.iter().count(), 0);
        assert_eq!(render_tranche.iter().count(), 0);
    }

}
