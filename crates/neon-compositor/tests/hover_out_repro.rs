use std::{
    cell::RefCell,
    fs,
    io::{BufRead, BufReader, Write},
    os::fd::AsFd,
    os::unix::net::{UnixListener, UnixStream},
    path::PathBuf,
    process::{Child, Command, Stdio},
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use neon_compositor::{
    input::process_input_event,
    ipc::IpcServer,
    state::NeonState,
};
use smithay::{
    backend::{
        input::{
            AbsolutePositionEvent, ButtonState, Device, DeviceCapability, Event as InputEventTrait,
            InputBackend, InputEvent, KeyState, KeyboardKeyEvent, PointerAxisEvent,
            PointerButtonEvent, PointerMotionAbsoluteEvent, PointerMotionEvent, UnusedEvent,
        },
        renderer::utils::with_renderer_surface_state,
    },
    desktop::layer_map_for_output,
    output::{Mode, Output, PhysicalProperties, Subpixel},
    reexports::{
        calloop::EventLoop,
        wayland_server::{Display, Resource},
    },
    utils::Transform,
    wayland::{
        shm::with_buffer_contents,
        socket::ListeningSocketSource,
    },
};
use wayland_client::{
    Connection, Dispatch, QueueHandle,
    protocol::{
        wl_buffer, wl_compositor, wl_registry, wl_shm, wl_shm_pool, wl_surface,
    },
};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

struct MockBackend;
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct MockDevice;
impl Device for MockDevice {
    fn id(&self) -> String { "mock-ptr".into() }
    fn name(&self) -> String { "Mock Pointer".into() }
    fn has_capability(&self, _: DeviceCapability) -> bool { true }
    fn usb_id(&self) -> Option<(u32, u32)> { None }
    fn syspath(&self) -> Option<PathBuf> { None }
}
struct MockAbsMotion { x: f64, y: f64, time_ms: u64 }
impl InputEventTrait<MockBackend> for MockAbsMotion {
    fn time(&self) -> u64 { self.time_ms * 1000 }
    fn device(&self) -> MockDevice { MockDevice }
}
impl AbsolutePositionEvent<MockBackend> for MockAbsMotion {
    fn x(&self) -> f64 { self.x }
    fn y(&self) -> f64 { self.y }
    fn x_transformed(&self, width: i32) -> f64 { self.x * f64::from(width) / 1280.0 }
    fn y_transformed(&self, height: i32) -> f64 { self.y * f64::from(height) / 800.0 }
}
impl PointerMotionAbsoluteEvent<MockBackend> for MockAbsMotion {}

struct NeverKey;
impl InputEventTrait<MockBackend> for NeverKey {
    fn time(&self) -> u64 { 0 }
    fn device(&self) -> MockDevice { MockDevice }
}
impl KeyboardKeyEvent<MockBackend> for NeverKey {
    fn key_code(&self) -> smithay::backend::input::Keycode { unreachable!() }
    fn state(&self) -> KeyState { KeyState::Released }
    fn count(&self) -> u32 { 0 }
}
struct NeverMotion;
impl InputEventTrait<MockBackend> for NeverMotion {
    fn time(&self) -> u64 { 0 }
    fn device(&self) -> MockDevice { MockDevice }
}
impl PointerMotionEvent<MockBackend> for NeverMotion {
    fn delta_x(&self) -> f64 { 0.0 }
    fn delta_y(&self) -> f64 { 0.0 }
    fn delta_x_unaccel(&self) -> f64 { 0.0 }
    fn delta_y_unaccel(&self) -> f64 { 0.0 }
}
struct MockBtn { code: u32, state: ButtonState, time_ms: u64 }
impl InputEventTrait<MockBackend> for MockBtn {
    fn time(&self) -> u64 { self.time_ms * 1000 }
    fn device(&self) -> MockDevice { MockDevice }
}
impl PointerButtonEvent<MockBackend> for MockBtn {
    fn button_code(&self) -> u32 { self.code }
    fn state(&self) -> ButtonState { self.state }
}
struct NeverAxis;
impl InputEventTrait<MockBackend> for NeverAxis {
    fn time(&self) -> u64 { 0 }
    fn device(&self) -> MockDevice { MockDevice }
}
impl PointerAxisEvent<MockBackend> for NeverAxis {
    fn amount(&self, _: smithay::backend::input::Axis) -> Option<f64> { None }
    fn amount_v120(&self, _: smithay::backend::input::Axis) -> Option<f64> { None }
    fn source(&self) -> smithay::backend::input::AxisSource { smithay::backend::input::AxisSource::Continuous }
    fn relative_direction(&self, _: smithay::backend::input::Axis) -> smithay::backend::input::AxisRelativeDirection {
        smithay::backend::input::AxisRelativeDirection::Identical
    }
}

impl InputBackend for MockBackend {
    type Device = MockDevice;
    type KeyboardKeyEvent = NeverKey;
    type PointerAxisEvent = NeverAxis;
    type PointerButtonEvent = MockBtn;
    type PointerMotionEvent = NeverMotion;
    type PointerMotionAbsoluteEvent = MockAbsMotion;
    type GestureSwipeBeginEvent = UnusedEvent;
    type GestureSwipeUpdateEvent = UnusedEvent;
    type GestureSwipeEndEvent = UnusedEvent;
    type GesturePinchBeginEvent = UnusedEvent;
    type GesturePinchUpdateEvent = UnusedEvent;
    type GesturePinchEndEvent = UnusedEvent;
    type GestureHoldBeginEvent = UnusedEvent;
    type GestureHoldEndEvent = UnusedEvent;
    type TouchDownEvent = UnusedEvent;
    type TouchUpEvent = UnusedEvent;
    type TouchMotionEvent = UnusedEvent;
    type TouchCancelEvent = UnusedEvent;
    type TouchFrameEvent = UnusedEvent;
    type TabletToolAxisEvent = UnusedEvent;
    type TabletToolProximityEvent = UnusedEvent;
    type TabletToolTipEvent = UnusedEvent;
    type TabletToolButtonEvent = UnusedEvent;
    type SwitchToggleEvent = UnusedEvent;
    type SpecialEvent = UnusedEvent;
}

struct ChildGuard(Option<Child>);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(mut c) = self.0.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

#[derive(Default)]
struct DummyWinState {
    compositor: Option<wl_compositor::WlCompositor>,
    shm: Option<wl_shm::WlShm>,
    wm_base: Option<xdg_wm_base::XdgWmBase>,
    configured: bool,
}
impl Dispatch<wl_registry::WlRegistry, ()> for DummyWinState {
    fn event(
        state: &mut Self,
        reg: &wl_registry::WlRegistry,
        ev: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global { name, interface, version } = ev {
            match interface.as_str() {
                "wl_compositor" => state.compositor = Some(reg.bind(name, version.min(5), qh, ())),
                "wl_shm" => state.shm = Some(reg.bind(name, version.min(1), qh, ())),
                "xdg_wm_base" => state.wm_base = Some(reg.bind(name, version.min(4), qh, ())),
                _ => {}
            }
        }
    }
}
impl Dispatch<wl_compositor::WlCompositor, ()> for DummyWinState {
    fn event(_: &mut Self, _: &wl_compositor::WlCompositor, _: wl_compositor::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<wl_surface::WlSurface, ()> for DummyWinState {
    fn event(_: &mut Self, _: &wl_surface::WlSurface, _: wl_surface::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<wl_shm::WlShm, ()> for DummyWinState {
    fn event(_: &mut Self, _: &wl_shm::WlShm, _: wl_shm::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<wl_shm_pool::WlShmPool, ()> for DummyWinState {
    fn event(_: &mut Self, _: &wl_shm_pool::WlShmPool, _: wl_shm_pool::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<wl_buffer::WlBuffer, ()> for DummyWinState {
    fn event(_: &mut Self, _: &wl_buffer::WlBuffer, _: wl_buffer::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<xdg_wm_base::XdgWmBase, ()> for DummyWinState {
    fn event(_: &mut Self, wm: &xdg_wm_base::XdgWmBase, ev: xdg_wm_base::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let xdg_wm_base::Event::Ping { serial } = ev { wm.pong(serial); }
    }
}
impl Dispatch<xdg_surface::XdgSurface, ()> for DummyWinState {
    fn event(state: &mut Self, xs: &xdg_surface::XdgSurface, ev: xdg_surface::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let xdg_surface::Event::Configure { serial } = ev {
            xs.ack_configure(serial);
            state.configured = true;
        }
    }
}
impl Dispatch<xdg_toplevel::XdgToplevel, ()> for DummyWinState {
    fn event(_: &mut Self, _: &xdg_toplevel::XdgToplevel, _: xdg_toplevel::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

#[derive(Debug)]
struct LayerSummary {
    ns: String,
    geo: (i32, i32, i32, i32),
    opaque_pixels: usize,
    bbox: Option<(i32, i32, i32, i32)>,
}

fn inspect_layer(output: &Output, target_ns: &str) -> Option<LayerSummary> {
    let map = layer_map_for_output(output);
    let layer = map.layers().find(|l| l.namespace() == target_ns)?;
    let geo = map.layer_geometry(&layer)?;
    let (opaque_pixels, bbox) = with_renderer_surface_state(layer.wl_surface(), |st| {
        let Some(buf) = st.buffer() else {
            return (0usize, None);
        };
        with_buffer_contents(&**buf, |ptr, pool_len, bdata| {
            let w = bdata.width.max(0) as usize;
            let h = bdata.height.max(0) as usize;
            let stride = bdata.stride.max(0) as usize;
            let off = bdata.offset.max(0) as usize;
            if w == 0 || h == 0 || off + stride * h > pool_len {
                return (0, None);
            }
            let slice = unsafe { std::slice::from_raw_parts(ptr.add(off), stride * h) };
            let mut count = 0usize;
            let mut minx = usize::MAX;
            let mut miny = usize::MAX;
            let mut maxx = 0usize;
            let mut maxy = 0usize;
            for y in 0..h {
                let row = &slice[y * stride..y * stride + w * 4];
                for x in 0..w {
                    let a = row[x * 4 + 3];
                    if a > 4 {
                        count += 1;
                        minx = minx.min(x);
                        miny = miny.min(y);
                        maxx = maxx.max(x);
                        maxy = maxy.max(y);
                    }
                }
            }
            let b = if count > 0 {
                Some((minx as i32, miny as i32, maxx as i32, maxy as i32))
            } else {
                None
            };
            (count, b)
        })
        .unwrap_or((0, None))
    })
    .unwrap_or((0, None));

    Some(LayerSummary {
        ns: target_ns.to_string(),
        geo: (geo.loc.x, geo.loc.y, geo.size.w, geo.size.h),
        opaque_pixels,
        bbox,
    })
}

fn find_neon_shell_bin() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let root = manifest.parent().unwrap().parent().unwrap();
    let release = root.join("target-user/release/neon-shell");
    if release.exists() {
        return release;
    }
    PathBuf::from("/usr/local/bin/neon-shell")
}

#[test]
fn repro_hover_out_behavior() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg_home = tmp.path().join("cfg");
    let cfg_dir = cfg_home.join("neon-de");
    fs::create_dir_all(&cfg_dir).unwrap();
    let cfg_toml = cfg_dir.join("config.toml");
    fs::write(
        &cfg_toml,
        r##"schema_version = 4
[appearance]
theme = "neon_dark"
accent_color = "#0A84FF"
wallpaper_path = "/home/satyajit/Pictures/Wallpapers/macOS-Golden-Gate-Dark.png"
[effects]
enabled = true
blur_enabled = true
blur_strength = 0.80
[dock]
height = 59
icon_size = 43
[panel]
mode = "combined_bottom"
dynamic_island_enabled = true
"##,
    )
    .unwrap();
    fs::write(
        cfg_dir.join("notch-prefs.json"),
        r#"{"appearance":"liquid_glass","pinned":["home","media","tools","clipboard","hud"]}"#,
    )
    .unwrap();

    let sock_name = format!("wayland-neon-hover-repro-{}", std::process::id());
    let real_ipc_path = tmp.path().join("ipc-real.sock");
    let proxy_ipc_path = tmp.path().join("ipc.sock");
    let shell_log = tmp.path().join("shell.log");

    let mut event_loop: EventLoop<NeonState> = EventLoop::try_new().unwrap();
    let display = Rc::new(RefCell::new(Display::<NeonState>::new().unwrap()));
    let listening_socket = ListeningSocketSource::with_name(&sock_name).unwrap();

    let mut state = NeonState::new(
        &display.borrow(),
        event_loop.handle(),
        event_loop.get_signal(),
        sock_name.clone().into(),
    );
    state.backend_name = "headless-test".into();
    let loaded_cfg = neon_config::load_or_default(&cfg_toml);
    state.apply_config(loaded_cfg);
    state.ipc_socket_path = Some(real_ipc_path.clone());

    let output = Output::new(
        "NEON-Test-1".to_string(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "NEON DE".into(),
            model: "Test".into(),
        },
    );
    let mode = Mode { size: (1280, 800).into(), refresh: 60_000 };
    output.change_current_state(Some(mode), Some(Transform::Normal), None, Some((0, 0).into()));
    output.set_preferred(mode);
    state.register_output(output.clone(), (0, 0).into());

    let mut ipc_server = IpcServer::bind(real_ipc_path.clone()).unwrap();

    let proxy_listener = UnixListener::bind(&proxy_ipc_path).unwrap();
    let proxy_stop = Arc::new(AtomicBool::new(false));
    let proxy_stop_c = Arc::clone(&proxy_stop);
    let real_ipc_c = real_ipc_path.clone();
    thread::spawn(move || {
        proxy_listener.set_nonblocking(true).ok();
        while !proxy_stop_c.load(Ordering::SeqCst) {
            if let Ok((client_stream, _)) = proxy_listener.accept() {
                let _ = client_stream.set_nonblocking(false);
                if let Ok(server_stream) = UnixStream::connect(&real_ipc_c) {
                    let _ = server_stream.set_nonblocking(false);
                    let s_in = server_stream.try_clone().unwrap();
                    let c_out = client_stream.try_clone().unwrap();
                    thread::spawn(move || {
                        let mut reader = BufReader::new(client_stream);
                        let mut sw = server_stream;
                        let mut line = String::new();
                        while reader.read_line(&mut line).unwrap_or(0) > 0 {
                            if sw.write_all(line.as_bytes()).is_err() { break; }
                            let _ = sw.flush();
                            line.clear();
                        }
                    });
                    thread::spawn(move || {
                        let mut reader = BufReader::new(s_in);
                        let mut cw = c_out;
                        let mut line = String::new();
                        while reader.read_line(&mut line).unwrap_or(0) > 0 {
                            let rewritten = line.replace("\"protocol_rev\":7", "\"protocol_rev\":6");
                            if cw.write_all(rewritten.as_bytes()).is_err() { break; }
                            let _ = cw.flush();
                            line.clear();
                        }
                    });
                }
            }
            thread::sleep(Duration::from_millis(10));
        }
    });

    event_loop
        .handle()
        .insert_source(listening_socket, |client_stream, _, state| {
            state.insert_client(client_stream);
        })
        .unwrap();

    let log_file = fs::File::create(&shell_log).unwrap();
    let log_err = log_file.try_clone().unwrap();
    let shell_bin = find_neon_shell_bin();
    let child = Command::new(&shell_bin)
        .arg("--force-combined-mode")
        .arg("--no-polkit")
        .env("WAYLAND_DISPLAY", &sock_name)
        .env("NEON_IPC_SOCKET", &proxy_ipc_path)
        .env("XDG_CONFIG_HOME", &cfg_home)
        .env("XDG_DATA_HOME", tmp.path().join("data"))
        .env("GSK_RENDERER", "cairo")
        .env("GDK_DISABLE", "vulkan")
        .env("DBUS_SESSION_BUS_ADDRESS", format!("unix:path={}/no-dbus", tmp.path().display()))
        .env("GSETTINGS_BACKEND", "memory")
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(log_err))
        .spawn()
        .unwrap();
    let _shell_guard = ChildGuard(Some(child));

    let mut pump = |el: &mut EventLoop<NeonState>, st: &mut NeonState, ms: u64| {
        let end = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < end {
            ipc_server.poll_requests(st);
            let _ = el.dispatch(Some(Duration::from_millis(8)), st);
            display.borrow_mut().dispatch_clients(st).unwrap();
            let now = st.clock.now();
            for w in st.space.elements() {
                w.send_frame(&output, now, Some(Duration::ZERO), |_, _| Some(output.clone()));
            }
            for layer in layer_map_for_output(&output).layers() {
                layer.send_frame(&output, now, Some(Duration::ZERO), |_, _| Some(output.clone()));
            }
            display.borrow_mut().flush_clients().unwrap();
            st.cleanup_dead_windows();
            thread::sleep(Duration::from_millis(8));
        }
    };

    let wait_start = Instant::now();
    let mut island_mapped = false;
    while wait_start.elapsed() < Duration::from_secs(14) {
        pump(&mut event_loop, &mut state, 50);
        if let Some(sum) = inspect_layer(&output, "neon-dynamic-island") && sum.opaque_pixels > 0 {
            island_mapped = true;
            break;
        }
    }
    pump(&mut event_loop, &mut state, 800);
    assert!(island_mapped, "neon-dynamic-island should map and draw");

    let mut t_ms = 1000u64;
    let move_ptr = |st: &mut NeonState, x: f64, y: f64, tm: &mut u64| {
        *tm += 20;
        process_input_event::<MockBackend>(
            st,
            InputEvent::PointerMotionAbsolute { event: MockAbsMotion { x, y, time_ms: *tm } },
        );
    };
    let click_ptr = |st: &mut NeonState, x: f64, y: f64, tm: &mut u64| {
        move_ptr(st, x, y, tm);
        *tm += 10;
        process_input_event::<MockBackend>(
            st,
            InputEvent::PointerButton { event: MockBtn { code: 0x110, state: ButtonState::Pressed, time_ms: *tm } },
        );
        *tm += 15;
        process_input_event::<MockBackend>(
            st,
            InputEvent::PointerButton { event: MockBtn { code: 0x110, state: ButtonState::Released, time_ms: *tm } },
        );
    };

    // Map maximized Covering App (obscured = true)
    let stop_win = Arc::new(AtomicBool::new(false));
    let rename_win = Arc::new(AtomicBool::new(false));
    let stop_c = Arc::clone(&stop_win);
    let rename_c = Arc::clone(&rename_win);
    let sock_c = sock_name.clone();
    let win_thread = thread::spawn(move || {
        let stream = UnixStream::connect(
            std::env::var_os("XDG_RUNTIME_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("/run/user/1000"))
                .join(&sock_c),
        ).unwrap();
        let conn = Connection::from_socket(stream).unwrap();
        let mut queue = conn.new_event_queue::<DummyWinState>();
        let qh = queue.handle();
        let mut wst = DummyWinState::default();
        let _reg = conn.display().get_registry(&qh, ());
        queue.roundtrip(&mut wst).unwrap();
        queue.roundtrip(&mut wst).unwrap();
        let comp = wst.compositor.clone().unwrap();
        let shm = wst.shm.clone().unwrap();
        let wm = wst.wm_base.clone().unwrap();
        let surf = comp.create_surface(&qh, ());
        let xsurf = wm.get_xdg_surface(&surf, &qh, ());
        let top = xsurf.get_toplevel(&qh, ());
        top.set_title("Covering App".into());
        top.set_app_id("org.test.covering".into());
        top.set_maximized();
        surf.commit();
        while !wst.configured {
            queue.blocking_dispatch(&mut wst).unwrap();
        }
        let stride = 1280i32 * 4;
        let size = (stride * 720) as usize;
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(&vec![0xFFu8; size]).unwrap();
        f.flush().unwrap();
        let pool = shm.create_pool(f.as_file().as_fd(), size as i32, &qh, ());
        let buf = pool.create_buffer(0, 1280, 720, stride, wl_shm::Format::Argb8888, &qh, ());
        surf.attach(Some(&buf), 0, 0);
        surf.commit();
        conn.flush().unwrap();
        let mut renamed = false;
        while !stop_c.load(Ordering::SeqCst) {
            if !renamed && rename_c.load(Ordering::SeqCst) {
                renamed = true;
                top.set_title("Covering App - Ep 45 Playing".into());
                surf.commit();
                let _ = conn.flush();
            }
            let _ = queue.dispatch_pending(&mut wst);
            let _ = conn.flush();
            thread::sleep(Duration::from_millis(20));
        }
    });
    pump(&mut event_loop, &mut state, 1800);

    // 1. Dynamically open Store (`⊞` button at rightmost icon inside dock_pill), click Style inside Store -> hover out
    move_ptr(&mut state, 640.0, 2.0, &mut t_ms);
    pump(&mut event_loop, &mut state, 900);
    let home_sum = inspect_layer(&output, "neon-dynamic-island").unwrap();
    let home_dock_y = home_sum.bbox.map(|b| f64::from(b.3) - 18.0).unwrap_or(202.0);
    println!("HOME EXPANDED: {home_sum:?} -> clicking store at (742, {home_dock_y})");
    click_ptr(&mut state, 742.0, home_dock_y, &mut t_ms);
    pump(&mut event_loop, &mut state, 600);
    let store_sum = inspect_layer(&output, "neon-dynamic-island").unwrap();
    println!("STORE ACTUALLY OPENED: {store_sum:?}");
    // Click around Style = Black Glass / Liquid Glass chip (~760, 62) inside Store
    click_ptr(&mut state, 760.0, 62.0, &mut t_ms);
    pump(&mut event_loop, &mut state, 250);
    move_ptr(&mut state, 200.0, 400.0, &mut t_ms);
    pump(&mut event_loop, &mut state, 2000);
    println!("STORE AFTER STYLE CLICK + HOVER OUT: {:?}", inspect_layer(&output, "neon-dynamic-island"));

    // 2. Re-hover at (640, 2) to check why st_sum was 0 after Store style click!
    move_ptr(&mut state, 640.0, 2.0, &mut t_ms);
    pump(&mut event_loop, &mut state, 900);
    let st_sum = inspect_layer(&output, "neon-dynamic-island").unwrap();
    println!("RE-HOVER AFTER STORE CLICK: {st_sum:?}");
    let st_dock_y = st_sum.bbox.map(|b| f64::from(b.3) - 18.0).unwrap_or(202.0);
    // Click 5th dock pill icon ("hud" at ~694, st_dock_y) -> then test HUD Scale slider drag-out!
    click_ptr(&mut state, 694.0, st_dock_y, &mut t_ms);
    pump(&mut event_loop, &mut state, 500);
    let hud_sum = inspect_layer(&output, "neon-dynamic-island").unwrap();
    println!("HUD OPENED: {hud_sum:?}");
    // Also switch to Timer if we click Store->Timer or let's drag on HUD Scale (~640, 82) out to (200, 400)
    move_ptr(&mut state, 640.0, 82.0, &mut t_ms);
    pump(&mut event_loop, &mut state, 60);
    t_ms += 10;
    process_input_event::<MockBackend>(
        &mut state,
        InputEvent::PointerButton { event: MockBtn { code: 0x110, state: ButtonState::Pressed, time_ms: t_ms } },
    );
    pump(&mut event_loop, &mut state, 80);
    move_ptr(&mut state, 200.0, 400.0, &mut t_ms);
    pump(&mut event_loop, &mut state, 120);
    t_ms += 10;
    process_input_event::<MockBackend>(
        &mut state,
        InputEvent::PointerButton { event: MockBtn { code: 0x110, state: ButtonState::Released, time_ms: t_ms } },
    );
    pump(&mut event_loop, &mut state, 2000);
    println!("HUD AFTER SLIDER DRAG-OUT RELEASE: {:?}", inspect_layer(&output, "neon-dynamic-island"));

    // 2B. Also test Timer ruler drag-out release (click Timer in Store? Or open Store->Timer via dock_pill Store->Timer card)
    // Open Store -> click Timer card in Store (~550, 120) -> drag ruler (640, 145) -> release outside (200, 400)!
    move_ptr(&mut state, 640.0, 2.0, &mut t_ms);
    pump(&mut event_loop, &mut state, 900);
    let b2 = inspect_layer(&output, "neon-dynamic-island").unwrap();
    println!("BEFORE TIMER RE-HOVER: {b2:?}");
    let dy2 = b2.bbox.map(|b| f64::from(b.3) - 18.0).unwrap_or(202.0);
    // If Store isn't active, click rightmost store icon ~742, dy2
    click_ptr(&mut state, 742.0, dy2, &mut t_ms);
    pump(&mut event_loop, &mut state, 400);
    // In Store 3x4 grid, row 0 col 2 ("Focus Timer" is #3 = top-right of grid around x=790, y=130)
    click_ptr(&mut state, 790.0, 130.0, &mut t_ms);
    pump(&mut event_loop, &mut state, 400);
    println!("AFTER TIMER TILE CLICK: {:?}", inspect_layer(&output, "neon-dynamic-island"));
    move_ptr(&mut state, 640.0, 155.0, &mut t_ms);
    pump(&mut event_loop, &mut state, 60);
    t_ms += 10;
    process_input_event::<MockBackend>(
        &mut state,
        InputEvent::PointerButton { event: MockBtn { code: 0x110, state: ButtonState::Pressed, time_ms: t_ms } },
    );
    pump(&mut event_loop, &mut state, 80);
    move_ptr(&mut state, 200.0, 400.0, &mut t_ms);
    pump(&mut event_loop, &mut state, 120);
    t_ms += 10;
    process_input_event::<MockBackend>(
        &mut state,
        InputEvent::PointerButton { event: MockBtn { code: 0x110, state: ButtonState::Released, time_ms: t_ms } },
    );
    pump(&mut event_loop, &mut state, 2000);
    println!("TIMER AFTER RULER DRAG-OUT RELEASE: {:?}", inspect_layer(&output, "neon-dynamic-island"));

    // 3. Hover Dock -> preview -> move onto preview -> move back onto icon -> move to empty taskbar -> hover out
    let mut preview_seen_at = None;
    for px in (320..=960).step_by(16) {
        move_ptr(&mut state, f64::from(px), 772.0, &mut t_ms);
        pump(&mut event_loop, &mut state, 35);
        if inspect_layer(&output, "neon-dock-tooltip").is_some() {
            pump(&mut event_loop, &mut state, 480);
            if let Some(prev) = inspect_layer(&output, "neon-dock-preview") {
                println!("DOCK PREVIEW AT x={px}: {prev:?}");
                preview_seen_at = Some((px, prev.geo));
                break;
            }
        }
    }
    if let Some((px, (gx, gy, gw, gh))) = preview_seen_at {
        move_ptr(&mut state, f64::from(gx + gw / 2), f64::from(gy + gh / 2), &mut t_ms);
        pump(&mut event_loop, &mut state, 250);
        move_ptr(&mut state, f64::from(px), 772.0, &mut t_ms);
        pump(&mut event_loop, &mut state, 250);
        move_ptr(&mut state, 1000.0, 772.0, &mut t_ms);
        pump(&mut event_loop, &mut state, 1000);
        println!("DOCK AFTER PREVIEW->ICON->EMPTY: tooltip={:?} preview={:?}",
            inspect_layer(&output, "neon-dock-tooltip").is_some(),
            inspect_layer(&output, "neon-dock-preview").is_some());
    }

    proxy_stop.store(true, Ordering::SeqCst);
    stop_win.store(true, Ordering::SeqCst);
    let _ = win_thread.join();
}


mod textbox_repro {
    use super::*;
    use std::io::Write;
    use std::os::unix::net::UnixStream;
    use smithay::backend::input::{KeyState, KeyboardKeyEvent, UnusedEvent};
    use tempfile::TempDir;
    use wayland_client::{
        Connection, Dispatch, QueueHandle,
        protocol::{wl_buffer, wl_compositor, wl_registry, wl_shm, wl_shm_pool, wl_surface},
    };
    use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

    struct KeyEv {
        code: u32,
        state: KeyState,
        time_ms: u64,
    }
    impl InputEventTrait<KeyMockBackend> for KeyEv {
        fn time(&self) -> u64 {
            self.time_ms
        }
        fn device(&self) -> MockDevice {
            MockDevice
        }
    }
    impl KeyboardKeyEvent<KeyMockBackend> for KeyEv {
        fn key_code(&self) -> smithay::backend::input::Keycode {
            self.code.into()
        }
        fn state(&self) -> KeyState {
            self.state
        }
        fn count(&self) -> u32 {
            1
        }
    }

    struct KeyMockBackend;
    impl InputBackend for KeyMockBackend {
        type Device = MockDevice;
        type KeyboardKeyEvent = KeyEv;
        type PointerAxisEvent = UnusedEvent;
        type PointerButtonEvent = UnusedEvent;
        type PointerMotionEvent = UnusedEvent;
        type PointerMotionAbsoluteEvent = UnusedEvent;
        type GestureSwipeBeginEvent = UnusedEvent;
        type GestureSwipeUpdateEvent = UnusedEvent;
        type GestureSwipeEndEvent = UnusedEvent;
        type GesturePinchBeginEvent = UnusedEvent;
        type GesturePinchUpdateEvent = UnusedEvent;
        type GesturePinchEndEvent = UnusedEvent;
        type GestureHoldBeginEvent = UnusedEvent;
        type GestureHoldEndEvent = UnusedEvent;
        type TouchDownEvent = UnusedEvent;
        type TouchMotionEvent = UnusedEvent;
        type TouchUpEvent = UnusedEvent;
        type TouchCancelEvent = UnusedEvent;
        type TouchFrameEvent = UnusedEvent;
        type TabletToolAxisEvent = UnusedEvent;
        type TabletToolProximityEvent = UnusedEvent;
        type TabletToolTipEvent = UnusedEvent;
        type TabletToolButtonEvent = UnusedEvent;
        type SwitchToggleEvent = UnusedEvent;
        type SpecialEvent = UnusedEvent;
    }

    #[derive(Default)]
    struct MaxWin {
        compositor: Option<wl_compositor::WlCompositor>,
        shm: Option<wl_shm::WlShm>,
        wm_base: Option<xdg_wm_base::XdgWmBase>,
        configured: bool,
        cfg_w: i32,
        cfg_h: i32,
    }
    impl Dispatch<wl_registry::WlRegistry, ()> for MaxWin {
        fn event(
            s: &mut Self,
            reg: &wl_registry::WlRegistry,
            e: wl_registry::Event,
            _: &(),
            _: &Connection,
            qh: &QueueHandle<Self>,
        ) {
            if let wl_registry::Event::Global { name, interface, version } = e {
                match interface.as_str() {
                    "wl_compositor" => s.compositor = Some(reg.bind(name, version.min(6), qh, ())),
                    "wl_shm" => s.shm = Some(reg.bind(name, version.min(1), qh, ())),
                    "xdg_wm_base" => s.wm_base = Some(reg.bind(name, version.min(5), qh, ())),
                    _ => {}
                }
            }
        }
    }
    impl Dispatch<wl_compositor::WlCompositor, ()> for MaxWin {
        fn event(_: &mut Self, _: &wl_compositor::WlCompositor, _: wl_compositor::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
    }
    impl Dispatch<wl_surface::WlSurface, ()> for MaxWin {
        fn event(_: &mut Self, _: &wl_surface::WlSurface, _: wl_surface::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
    }
    impl Dispatch<wl_shm::WlShm, ()> for MaxWin {
        fn event(_: &mut Self, _: &wl_shm::WlShm, _: wl_shm::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
    }
    impl Dispatch<wl_shm_pool::WlShmPool, ()> for MaxWin {
        fn event(_: &mut Self, _: &wl_shm_pool::WlShmPool, _: wl_shm_pool::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
    }
    impl Dispatch<wl_buffer::WlBuffer, ()> for MaxWin {
        fn event(_: &mut Self, _: &wl_buffer::WlBuffer, _: wl_buffer::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
    }
    impl Dispatch<xdg_wm_base::XdgWmBase, ()> for MaxWin {
        fn event(_: &mut Self, wm: &xdg_wm_base::XdgWmBase, e: xdg_wm_base::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
            if let xdg_wm_base::Event::Ping { serial } = e {
                wm.pong(serial);
            }
        }
    }
    impl Dispatch<xdg_surface::XdgSurface, ()> for MaxWin {
        fn event(s: &mut Self, xs: &xdg_surface::XdgSurface, e: xdg_surface::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
            if let xdg_surface::Event::Configure { serial } = e {
                xs.ack_configure(serial);
                s.configured = true;
            }
        }
    }
    impl Dispatch<xdg_toplevel::XdgToplevel, ()> for MaxWin {
        fn event(s: &mut Self, _: &xdg_toplevel::XdgToplevel, e: xdg_toplevel::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
            if let xdg_toplevel::Event::Configure { width, height, .. } = e {
                s.cfg_w = width;
                s.cfg_h = height;
            }
        }
    }

    #[test]
    fn repro_textbox_click_and_type_hover_out() {
        let tmp = TempDir::new().unwrap();
        let cfg_home = tmp.path().join("cfg");
        let cfg_dir = cfg_home.join("neon-de");
        std::fs::create_dir_all(&cfg_dir).unwrap();
        std::fs::write(
            cfg_dir.join("config.toml"),
            "[panel]\nmode = \"combined_bottom\"\ndynamic_island_enabled = true\n[dock]\nauto_hide = false\n",
        )
        .unwrap();
        std::fs::write(
            cfg_dir.join("notch-prefs.json"),
            r#"{"appearance":"liquid_glass","pinned":["home","clipboard","notes","calendar","tools"]}"#,
        )
        .unwrap();

        let runtime_dir = tmp.path().join("run");
        std::fs::create_dir_all(&runtime_dir).unwrap();
        let sock_name = format!("repro-tb-{}", std::process::id());
        let sock_path = runtime_dir.join(&sock_name);

        let mut event_loop = smithay::reexports::calloop::EventLoop::<NeonState>::try_new().unwrap();
        let mut display = Display::<NeonState>::new().unwrap();
        let mut state = NeonState::new(
            &display,
            event_loop.handle(),
            event_loop.get_signal(),
            sock_name.clone().into(),
        );
        state.config.panel.mode = neon_config::PanelMode::CombinedBottom;
        state.config.panel.dynamic_island_enabled = true;
        let listener = std::os::unix::net::UnixListener::bind(&sock_path).unwrap();
        listener.set_nonblocking(true).unwrap();

        let output = Output::new(
            "HEADLESS-1".into(),
            smithay::output::PhysicalProperties {
                size: (340, 190).into(),
                subpixel: smithay::output::Subpixel::Unknown,
                make: "NEON".into(),
                model: "Test".into(),
            },
        );
        let mode = smithay::output::Mode {
            size: (1280, 800).into(),
            refresh: 60_000,
        };
        output.change_current_state(Some(mode), None, None, Some((0, 0).into()));
        output.set_preferred(mode);
        state.register_output(output.clone(), (0, 0).into());

        let real_ipc_path = runtime_dir.join("ipc.sock");
        let mut ipc_server = IpcServer::bind(real_ipc_path.clone()).unwrap();

        let shell_bin = {
            let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
            let tdir = std::env::var_os("CARGO_TARGET_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| root.join("target-user"));
            let dbg = tdir.join("debug/neon-shell");
            let rel = tdir.join("release/neon-shell");
            match (std::fs::metadata(&dbg).and_then(|m| m.modified()), std::fs::metadata(&rel).and_then(|m| m.modified())) {
                (Ok(dm), Ok(rm)) => if dm >= rm { dbg } else { rel },
                (Ok(_), Err(_)) => dbg,
                (Err(_), Ok(_)) => rel,
                _ => find_neon_shell_bin(),
            }
        };
        println!("USING SHELL BIN: {}", shell_bin.display());
        let mut child = Command::new(&shell_bin)
            .env("WAYLAND_DISPLAY", &sock_name)
            .env_remove("DISPLAY")
            .env("NEON_IPC_SOCKET", &real_ipc_path)
            .env("XDG_CONFIG_HOME", &cfg_home)
            .env("XDG_DATA_HOME", tmp.path().join("data"))
            .env("XDG_RUNTIME_DIR", &runtime_dir)
            .env("GSK_RENDERER", "cairo")
            .env("GDK_DISABLE", "vulkan")
            .env("DBUS_SESSION_BUS_ADDRESS", format!("unix:path={}/no-dbus", tmp.path().display()))
            .env("GSETTINGS_BACKEND", "memory")
            .env("GTK_A11Y", "none")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn neon-shell");
        let stdout_pipe = child.stdout.take();
        let stderr_pipe = child.stderr.take();
        let _guard = ChildGuard(Some(child));

        let mut accepted_wl = 0usize;
        let t_epoch = Instant::now();
        let mut last_frame = Instant::now();
        let mut send_frames_if_due = |st: &NeonState| {
            if last_frame.elapsed() >= Duration::from_millis(16) {
                last_frame = Instant::now();
                let now_dur = t_epoch.elapsed();
                for w in st.space.elements() {
                    w.send_frame(&output, now_dur, Some(Duration::ZERO), |_, _| Some(output.clone()));
                }
                for layer in smithay::desktop::layer_map_for_output(&output).layers() {
                    layer.send_frame(&output, now_dur, Some(Duration::ZERO), |_, _| Some(output.clone()));
                }
            }
        };

        let start = Instant::now();
        let mut island_mapped = false;
        while start.elapsed() < Duration::from_secs(10) {
            while let Ok((stream, _)) = listener.accept() {
                accepted_wl += 1;
                let _ = stream.set_nonblocking(true);
                state.insert_client(stream);
            }
            let _ = event_loop.dispatch(Some(Duration::from_millis(4)), &mut state);
            ipc_server.accept_pending();
            ipc_server.poll_requests(&mut state);
            let _ = display.dispatch_clients(&mut state);
            send_frames_if_due(&state);
            let _ = display.flush_clients();
            if inspect_layer(&output, "neon-dynamic-island").is_some() {
                island_mapped = true;
                break;
            }
            thread::sleep(Duration::from_millis(12));
        }
        if !island_mapped {
            drop(_guard);
            let mut out_txt = String::new();
            let mut err_txt = String::new();
            if let Some(mut s) = stdout_pipe {
                let _ = std::io::Read::read_to_string(&mut s, &mut out_txt);
            }
            if let Some(mut s) = stderr_pipe {
                let _ = std::io::Read::read_to_string(&mut s, &mut err_txt);
            }
            panic!("neon-dynamic-island should map; accepted_wl={accepted_wl} layers={:?} stdout={out_txt} stderr={err_txt}", smithay::desktop::layer_map_for_output(&output).layers().map(|l| l.namespace().to_string()).collect::<Vec<_>>());
        }
        let pump = |st: &mut NeonState, disp: &mut Display<NeonState>, el: &mut smithay::reexports::calloop::EventLoop<NeonState>, ipc: &mut IpcServer, lst: &std::os::unix::net::UnixListener| {
            while let Ok((stream, _)) = lst.accept() {
                let _ = stream.set_nonblocking(true);
                st.insert_client(stream);
            }
            let _ = el.dispatch(Some(Duration::from_millis(4)), st);
            ipc.accept_pending();
            ipc.poll_requests(st);
            let _ = disp.dispatch_clients(st);
            let now_dur = t_epoch.elapsed();
            for w in st.space.elements() {
                w.send_frame(&output, now_dur, Some(Duration::ZERO), |_, _| Some(output.clone()));
            }
            for layer in smithay::desktop::layer_map_for_output(&output).layers() {
                layer.send_frame(&output, now_dur, Some(Duration::ZERO), |_, _| Some(output.clone()));
            }
            let _ = disp.flush_clients();
        };

        // Map maximized client window so Dynamic Island enters obscured stealth mode.
        let (srv_sock, cli_sock) = UnixStream::pair().unwrap();
        let _ = srv_sock.set_nonblocking(true);
        state.insert_client(srv_sock);
        let allow_exit = Arc::new(AtomicBool::new(false));
        let exit_flag = Arc::clone(&allow_exit);
        let win_t = thread::spawn(move || {
            let conn = Connection::from_socket(cli_sock).unwrap();
            let mut q = conn.new_event_queue::<MaxWin>();
            let qh = q.handle();
            let mut ws = MaxWin::default();
            let _r = conn.display().get_registry(&qh, ());
            q.roundtrip(&mut ws).unwrap();
            let comp = ws.compositor.clone().unwrap();
            let shm = ws.shm.clone().unwrap();
            let wm = ws.wm_base.clone().unwrap();
            let surf = comp.create_surface(&qh, ());
            let xs = wm.get_xdg_surface(&surf, &qh, ());
            let top = xs.get_toplevel(&qh, ());
            top.set_maximized();
            surf.commit();
            while !ws.configured {
                q.blocking_dispatch(&mut ws).unwrap();
            }
            let w = ws.cfg_w.max(1280);
            let h = ws.cfg_h.max(720);
            let mut f = tempfile::tempfile().unwrap();
            f.write_all(&vec![255u8; (w * h * 4) as usize]).unwrap();
            use std::os::fd::AsFd;
            let pool = shm.create_pool(f.as_fd(), w * h * 4, &qh, ());
            let buf = pool.create_buffer(0, w, h, w * 4, wl_shm::Format::Argb8888, &qh, ());
            surf.attach(Some(&buf), 0, 0);
            surf.commit();
            while !exit_flag.load(Ordering::Relaxed) {
                let _ = conn.flush();
                let _ = q.dispatch_pending(&mut ws);
                thread::sleep(Duration::from_millis(20));
            }
        });

        let wait_ms = |st: &mut NeonState, disp: &mut Display<NeonState>, el: &mut smithay::reexports::calloop::EventLoop<NeonState>, ipc: &mut IpcServer, lst: &std::os::unix::net::UnixListener, ms: u64| {
            let t0 = Instant::now();
            while t0.elapsed() < Duration::from_millis(ms) {
                pump(st, disp, el, ipc, lst);
                thread::sleep(Duration::from_millis(12));
            }
        };
        wait_ms(&mut state, &mut display, &mut event_loop, &mut ipc_server, &listener, 1500);
        let stealth = inspect_layer(&output, "neon-dynamic-island").unwrap();
        assert_eq!(stealth.ns, "neon-dynamic-island");
        println!("INITIAL STEALTH MAXIMIZED: {stealth:?}");
        assert!(stealth.opaque_pixels < 1500, "should not show compact pill when maximized: {stealth:?}");

        let mut t_ms = 1000u64;
        let move_ptr = |st: &mut NeonState, x: f64, y: f64, tm: &mut u64| {
            *tm += 20;
            process_input_event::<MockBackend>(
                st,
                InputEvent::PointerMotionAbsolute {
                    event: MockAbsMotion { x, y, time_ms: *tm },
                },
            );
        };
        let click_ptr = |st: &mut NeonState, disp: &mut Display<NeonState>, el: &mut smithay::reexports::calloop::EventLoop<NeonState>, ipc: &mut IpcServer, lst: &std::os::unix::net::UnixListener, x: f64, y: f64, tm: &mut u64| {
            move_ptr(st, x, y, tm);
            pump(st, disp, el, ipc, lst);
            thread::sleep(Duration::from_millis(20));
            *tm += 20;
            process_input_event::<MockBackend>(
                st,
                InputEvent::PointerButton {
                    event: MockBtn { code: 0x110, state: ButtonState::Pressed, time_ms: *tm },
                },
            );
            pump(st, disp, el, ipc, lst);
            thread::sleep(Duration::from_millis(20));
            *tm += 20;
            process_input_event::<MockBackend>(
                st,
                InputEvent::PointerButton {
                    event: MockBtn { code: 0x110, state: ButtonState::Released, time_ms: *tm },
                },
            );
            pump(st, disp, el, ipc, lst);
        };
        let type_key = |st: &mut NeonState, disp: &mut Display<NeonState>, el: &mut smithay::reexports::calloop::EventLoop<NeonState>, ipc: &mut IpcServer, lst: &std::os::unix::net::UnixListener, xkb_code: u32, tm: &mut u64| {
            *tm += 20;
            process_input_event::<KeyMockBackend>(
                st,
                InputEvent::Keyboard {
                    event: KeyEv { code: xkb_code, state: KeyState::Pressed, time_ms: *tm },
                },
            );
            pump(st, disp, el, ipc, lst);
            thread::sleep(Duration::from_millis(15));
            *tm += 20;
            process_input_event::<KeyMockBackend>(
                st,
                InputEvent::Keyboard {
                    event: KeyEv { code: xkb_code, state: KeyState::Released, time_ms: *tm },
                },
            );
            pump(st, disp, el, ipc, lst);
        };

        // Test helper for each module with a textbox:
        // Dock has 5 items ["home", "clipboard", "notes", "calendar", "tools"] + Store:
        let cases = [
            ("clipboard", 583.0_f64, 425.0_f64, 38.0_f64, &[38u32, 56, 54][..]), // clip_search ("abc")
            ("notes", 620.0_f64, 700.0_f64, 110.0_f64, &[43u32, 26, 38][..]),     // notes_editor ("hea")
            ("calendar", 657.0_f64, 760.0_f64, 38.0_f64, &[50u32, 26, 26, 36][..]), // cal_add_entry ("mee" + Enter)
            ("tools", 692.0_f64, 760.0_f64, 62.0_f64, &[10u32, 11, 12][..]),     // tools_calc_entry ("123")
        ];

        for (name, dock_x, tb_x, tb_y, keys) in cases {
            move_ptr(&mut state, 640.0, 2.0, &mut t_ms);
            wait_ms(&mut state, &mut display, &mut event_loop, &mut ipc_server, &listener, 750);
            let exp = inspect_layer(&output, "neon-dynamic-island").unwrap();
            let dy = f64::from(exp.geo.1 + exp.bbox.map(|b| b.3).unwrap_or(220) - 16);
            println!("[{name}] expanded before dock click: {exp:?}, dy={dy}");
            assert!(exp.opaque_pixels > 5000, "[{name}] failed to expand on top-edge hover: {exp:?}");

            click_ptr(&mut state, &mut display, &mut event_loop, &mut ipc_server, &listener, dock_x, dy, &mut t_ms);
            // Move into target panel immediately so tall->short page transitions (e.g. Calendar->Tools)
            // mirror natural pointer upward movement onto the newly opened panel instead of staying below it.
            move_ptr(&mut state, tb_x, tb_y, &mut t_ms);
            wait_ms(&mut state, &mut display, &mut event_loop, &mut ipc_server, &listener, 550);
            let mod_sum = inspect_layer(&output, "neon-dynamic-island").unwrap();
            let upper_after_switch = state.upper_layer_surface_under((tb_x, tb_y).into()).map(|(s, p, ki)| (s.id(), p, ki));
            println!("[{name}] after module switch: {mod_sum:?} upper={upper_after_switch:?}");
            assert!(mod_sum.opaque_pixels > 10_000, "[{name}] module should be open: {mod_sum:?}");

            // Click inside textbox & type characters
            click_ptr(&mut state, &mut display, &mut event_loop, &mut ipc_server, &listener, tb_x, tb_y, &mut t_ms);
            wait_ms(&mut state, &mut display, &mut event_loop, &mut ipc_server, &listener, 150);
            let island_wl_id = smithay::desktop::layer_map_for_output(&output)
                .layers()
                .find(|l| l.namespace() == "neon-dynamic-island")
                .map(|l| l.wl_surface().id());
            let mut kbd_focus_id = state.seat.get_keyboard().and_then(|k| k.current_focus()).and_then(|f| f.wl_surface_id());
            println!("[{name}] after click ({tb_x},{tb_y}): kbd_focus={kbd_focus_id:?} island_id={island_wl_id:?} upper={:?}", state.upper_layer_surface_under((tb_x, tb_y).into()).map(|(s, p, ki)| (s.id(), p, ki)));
            if kbd_focus_id != island_wl_id && (name == "clipboard" || name == "calendar") {
                for probe_y in [44.0_f64, 50.0, 56.0, 64.0] {
                    click_ptr(&mut state, &mut display, &mut event_loop, &mut ipc_server, &listener, tb_x, probe_y, &mut t_ms);
                    wait_ms(&mut state, &mut display, &mut event_loop, &mut ipc_server, &listener, 150);
                    kbd_focus_id = state.seat.get_keyboard().and_then(|k| k.current_focus()).and_then(|f| f.wl_surface_id());
                    println!("[{name}] probe click ({tb_x},{probe_y}): kbd_focus={kbd_focus_id:?}");
                    if kbd_focus_id == island_wl_id {
                        break;
                    }
                }
            }
            let kbd_after_click = kbd_focus_id.is_some() && kbd_focus_id == island_wl_id;
            assert!(kbd_after_click, "[{name}] clicking textbox ({tb_x},{tb_y}) should grant keyboard focus to neon-dynamic-island");

            let after_click = inspect_layer(&output, "neon-dynamic-island").unwrap();
            for &k in keys {
                type_key(&mut state, &mut display, &mut event_loop, &mut ipc_server, &listener, k, &mut t_ms);
            }
            wait_ms(&mut state, &mut display, &mut event_loop, &mut ipc_server, &listener, 180);
            let after_type = inspect_layer(&output, "neon-dynamic-island").unwrap();
            println!("[{name}] after click ({tb_x},{tb_y}): {:?} -> after type (kbd_focus={kbd_after_click}): {:?}", after_click.opaque_pixels, after_type.opaque_pixels);
            assert!(after_type.opaque_pixels > 10_000, "[{name}] island should stay open while typing inside");
            if name == "notes" || name == "calendar" {
                assert_ne!(after_type.opaque_pixels, mod_sum.opaque_pixels, "[{name}] typing should visibly update textbox content");
            }

            // Hover outside to (200, 400)
            move_ptr(&mut state, 200.0, 400.0, &mut t_ms);
            wait_ms(&mut state, &mut display, &mut event_loop, &mut ipc_server, &listener, 1000);
            let after_leave = inspect_layer(&output, "neon-dynamic-island").unwrap();
            let kbd_after_leave = state.seat.get_keyboard().and_then(|k| k.current_focus()).and_then(|f| f.wl_surface_id());
            println!("[{name}] AFTER HOVER OUT: {after_leave:?} (kbd_returned={})", kbd_after_leave != island_wl_id);
            assert_eq!(
                after_leave.opaque_pixels, 0,
                "[{name}] dynamic island did NOT disappear after clicking & typing in textbox then hovering out: {after_leave:?}"
            );
            assert_ne!(
                kbd_after_leave, island_wl_id,
                "[{name}] keyboard focus remained stuck on neon-dynamic-island after hover out"
            );
        }

        // Also verify Esc closes cleanly while typing in hover-focused Notes textbox
        move_ptr(&mut state, 640.0, 2.0, &mut t_ms);
        wait_ms(&mut state, &mut display, &mut event_loop, &mut ipc_server, &listener, 750);
        let exp = inspect_layer(&output, "neon-dynamic-island").unwrap();
        let dy = f64::from(exp.geo.1 + exp.bbox.map(|b| b.3).unwrap_or(220) - 16);
        click_ptr(&mut state, &mut display, &mut event_loop, &mut ipc_server, &listener, 620.0, dy, &mut t_ms);
        move_ptr(&mut state, 700.0, 110.0, &mut t_ms);
        wait_ms(&mut state, &mut display, &mut event_loop, &mut ipc_server, &listener, 500);
        click_ptr(&mut state, &mut display, &mut event_loop, &mut ipc_server, &listener, 700.0, 110.0, &mut t_ms);
        wait_ms(&mut state, &mut display, &mut event_loop, &mut ipc_server, &listener, 150);
        type_key(&mut state, &mut display, &mut event_loop, &mut ipc_server, &listener, 9, &mut t_ms);
        wait_ms(&mut state, &mut display, &mut event_loop, &mut ipc_server, &listener, 800);
        let after_esc = inspect_layer(&output, "neon-dynamic-island").unwrap();
        println!("NOTES AFTER ESC WHILE HOVER-FOCUSED: {after_esc:?}");
        assert_eq!(after_esc.opaque_pixels, 0, "Esc should collapse hover-focused Notes textbox");

        allow_exit.store(true, Ordering::Relaxed);
        let _ = win_t.join();
        wait_ms(&mut state, &mut display, &mut event_loop, &mut ipc_server, &listener, 1100);
        let compact = inspect_layer(&output, "neon-dynamic-island").unwrap();
        println!("UNOBSTRUCTED COMPACT PILL: {compact:?}");
        assert!(
            (300..=8500).contains(&compact.opaque_pixels) && compact.bbox.is_some_and(|b| b.3 <= 55),
            "expected unobstructed compact pill after closing window: {compact:?}"
        );

        move_ptr(&mut state, 640.0, 20.0, &mut t_ms);
        wait_ms(&mut state, &mut display, &mut event_loop, &mut ipc_server, &listener, 650);
        let unobs_exp = inspect_layer(&output, "neon-dynamic-island").unwrap();
        assert!(unobs_exp.opaque_pixels > 10_000, "unobstructed pill should expand on hover: {unobs_exp:?}");
        click_ptr(&mut state, &mut display, &mut event_loop, &mut ipc_server, &listener, 700.0, 110.0, &mut t_ms);
        wait_ms(&mut state, &mut display, &mut event_loop, &mut ipc_server, &listener, 150);
        type_key(&mut state, &mut display, &mut event_loop, &mut ipc_server, &listener, 38, &mut t_ms);
        wait_ms(&mut state, &mut display, &mut event_loop, &mut ipc_server, &listener, 150);
        move_ptr(&mut state, 200.0, 400.0, &mut t_ms);
        wait_ms(&mut state, &mut display, &mut event_loop, &mut ipc_server, &listener, 1000);
        let compact_after = inspect_layer(&output, "neon-dynamic-island").unwrap();
        println!("UNOBSTRUCTED AFTER NOTES TYPE + HOVER OUT: {compact_after:?}");
        assert!(
            (300..=8500).contains(&compact_after.opaque_pixels) && compact_after.bbox.is_some_and(|b| b.3 <= 55),
            "unobstructed Notes textbox should collapse back to compact pill on hover out: {compact_after:?}"
        );
    }
}
