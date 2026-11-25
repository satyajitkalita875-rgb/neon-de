//! End-to-end Wayland protocol integration tests for `neon-compositor`.
//!
//! Spins up a real `NeonState` + `wayland-server::Display` on an isolated Unix socket pair
//! and drives real `wayland-client` connections exercising:
//! - Milestone 1: `wl_compositor`, `wl_shm`, `xdg_wm_base`, `xdg_toplevel`, `wl_seat`
//!   (keyboard & pointer delivery), cascading placement, focus cycling, and destruction.
//! - Milestone 2: Dynamic workspaces, workspace switching, moving windows between workspaces,
//!   empty workspace pruning, minimize/restore, smart snapping, maximize/fullscreen state
//!   machine, and window rules.

use std::{
    io::Write,
    os::fd::AsFd,
    os::unix::net::UnixStream,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use neon_compositor::{
    state::NeonState,
    window::{SnapRegion, WindowMatcher, WindowMode, WindowRule, WindowRuleAction},
};
use smithay::{
    backend::input::{ButtonState, KeyState},
    input::{
        keyboard::FilterResult,
        pointer::{ButtonEvent, MotionEvent},
    },
    output::{Mode, Output, PhysicalProperties, Subpixel},
    reexports::{
        calloop::EventLoop,
        wayland_server::{Display, Resource},
    },
    utils::{Logical, Point, SERIAL_COUNTER, Transform},
};
use wayland_client::{
    Connection, Dispatch, QueueHandle,
    protocol::{
        wl_buffer, wl_compositor, wl_keyboard, wl_pointer, wl_registry, wl_seat, wl_shm,
        wl_shm_pool, wl_surface,
    },
};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

#[derive(Default)]
struct TestClientState {
    compositor: Option<wl_compositor::WlCompositor>,
    shm: Option<wl_shm::WlShm>,
    wm_base: Option<xdg_wm_base::XdgWmBase>,
    seat: Option<wl_seat::WlSeat>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    pointer: Option<wl_pointer::WlPointer>,
    configured: bool,
    last_configure_size: (i32, i32),
    last_configure_states: Vec<u32>,
    received_keymap: bool,
    received_keyboard_enter: bool,
    received_keys: Vec<u32>,
    received_pointer_enter: bool,
    received_pointer_buttons: Vec<u32>,
    received_close: bool,
}

impl Dispatch<wl_registry::WlRegistry, ()> for TestClientState {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            match interface.as_str() {
                "wl_compositor" => {
                    state.compositor = Some(registry.bind(name, version.min(5), qh, ()));
                }
                "wl_shm" => {
                    state.shm = Some(registry.bind(name, version.min(1), qh, ()));
                }
                "xdg_wm_base" => {
                    state.wm_base = Some(registry.bind(name, version.min(4), qh, ()));
                }
                "wl_seat" => {
                    state.seat = Some(registry.bind(name, version.min(7), qh, ()));
                }
                _ => {}
            }
        }
    }
}

impl Dispatch<wl_compositor::WlCompositor, ()> for TestClientState {
    fn event(
        _: &mut Self,
        _: &wl_compositor::WlCompositor,
        _: wl_compositor::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_surface::WlSurface, ()> for TestClientState {
    fn event(
        _: &mut Self,
        _: &wl_surface::WlSurface,
        _: wl_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_shm::WlShm, ()> for TestClientState {
    fn event(
        _: &mut Self,
        _: &wl_shm::WlShm,
        _: wl_shm::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_shm_pool::WlShmPool, ()> for TestClientState {
    fn event(
        _: &mut Self,
        _: &wl_shm_pool::WlShmPool,
        _: wl_shm_pool::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_buffer::WlBuffer, ()> for TestClientState {
    fn event(
        _: &mut Self,
        _: &wl_buffer::WlBuffer,
        _: wl_buffer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for TestClientState {
    fn event(
        _: &mut Self,
        wm_base: &xdg_wm_base::XdgWmBase,
        event: xdg_wm_base::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            wm_base.pong(serial);
        }
    }
}

impl Dispatch<xdg_surface::XdgSurface, ()> for TestClientState {
    fn event(
        state: &mut Self,
        xdg_surface: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            xdg_surface.ack_configure(serial);
            state.configured = true;
        }
    }
}

impl Dispatch<xdg_toplevel::XdgToplevel, ()> for TestClientState {
    fn event(
        state: &mut Self,
        _: &xdg_toplevel::XdgToplevel,
        event: xdg_toplevel::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            xdg_toplevel::Event::Configure {
                width,
                height,
                states,
            } => {
                state.last_configure_size = (width, height);
                state.last_configure_states = states
                    .chunks_exact(4)
                    .map(|c| u32::from_ne_bytes([c[0], c[1], c[2], c[3]]))
                    .collect();
            }
            xdg_toplevel::Event::Close => {
                state.received_close = true;
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for TestClientState {
    fn event(
        state: &mut Self,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities {
            capabilities: wayland_client::WEnum::Value(caps),
        } = event
        {
            if caps.contains(wl_seat::Capability::Keyboard) && state.keyboard.is_none() {
                state.keyboard = Some(seat.get_keyboard(qh, ()));
            }
            if caps.contains(wl_seat::Capability::Pointer) && state.pointer.is_none() {
                state.pointer = Some(seat.get_pointer(qh, ()));
            }
        }
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for TestClientState {
    fn event(
        state: &mut Self,
        _: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_keyboard::Event::Keymap { .. } => {
                state.received_keymap = true;
            }
            wl_keyboard::Event::Enter { .. } => {
                state.received_keyboard_enter = true;
            }
            wl_keyboard::Event::Key { key, .. } => {
                state.received_keys.push(key);
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_pointer::WlPointer, ()> for TestClientState {
    fn event(
        state: &mut Self,
        _: &wl_pointer::WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_pointer::Event::Enter { .. } => {
                state.received_pointer_enter = true;
            }
            wl_pointer::Event::Button { button, .. } => {
                state.received_pointer_buttons.push(button);
            }
            _ => {}
        }
    }
}

fn create_headless_server() -> (EventLoop<'static, NeonState>, Display<NeonState>, NeonState) {
    let event_loop: EventLoop<NeonState> = EventLoop::try_new().unwrap();
    let display: Display<NeonState> = Display::new().unwrap();
    let mut state = NeonState::new(
        &display,
        event_loop.handle(),
        event_loop.get_signal(),
        "neon-test".into(),
    );

    let output = Output::new(
        "NEON-Test-1".to_string(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "NEON DE".into(),
            model: "Headless Test Output".into(),
        },
    );
    let mode = Mode {
        size: (1280, 800).into(),
        refresh: 60_000,
    };
    output.change_current_state(
        Some(mode),
        Some(Transform::Normal),
        None,
        Some((0, 0).into()),
    );
    output.set_preferred(mode);
    state.register_output(output, (0, 0).into());

    (event_loop, display, state)
}

fn pump_server(display: &mut Display<NeonState>, state: &mut NeonState) {
    display.dispatch_clients(state).unwrap();
    display.flush_clients().unwrap();
    state.cleanup_dead_windows();
}

fn create_shm_buffer(
    shm: &wl_shm::WlShm,
    qh: &QueueHandle<TestClientState>,
    width: i32,
    height: i32,
) -> (
    tempfile::NamedTempFile,
    wl_shm_pool::WlShmPool,
    wl_buffer::WlBuffer,
) {
    let stride = width * 4;
    let size = (stride * height) as usize;
    let mut named = tempfile::NamedTempFile::new().unwrap();
    let pixels = vec![0xFFu8; size];
    named.write_all(&pixels).unwrap();
    named.flush().unwrap();
    let pool = shm.create_pool(named.as_file().as_fd(), size as i32, qh, ());
    let buffer = pool.create_buffer(0, width, height, stride, wl_shm::Format::Argb8888, qh, ());
    (named, pool, buffer)
}

#[test]
fn client_window_create_commit_keyboard_and_destroy() {
    let (_event_loop, mut display, mut state) = create_headless_server();
    let (server_sock, client_sock) = UnixStream::pair().unwrap();
    state.insert_client(server_sock);

    let client_mapped = Arc::new(AtomicBool::new(false));
    let client_mapped_clone = Arc::clone(&client_mapped);
    let allow_exit = Arc::new(AtomicBool::new(false));
    let allow_exit_clone = Arc::clone(&allow_exit);

    let client_handle = thread::spawn(move || {
        let conn = Connection::from_socket(client_sock).unwrap();
        let mut queue = conn.new_event_queue::<TestClientState>();
        let qh = queue.handle();
        let mut cstate = TestClientState::default();

        let _registry = conn.display().get_registry(&qh, ());
        queue.roundtrip(&mut cstate).unwrap();
        queue.roundtrip(&mut cstate).unwrap();

        let compositor = cstate.compositor.clone().expect("wl_compositor advertised");
        let shm = cstate.shm.clone().expect("wl_shm advertised");
        let wm_base = cstate.wm_base.clone().expect("xdg_wm_base advertised");

        let surface = compositor.create_surface(&qh, ());
        let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
        let toplevel = xdg_surface.get_toplevel(&qh, ());
        toplevel.set_title("NEON DE Test Terminal".into());
        toplevel.set_app_id("org.neonde.test-terminal".into());
        surface.commit();

        queue.roundtrip(&mut cstate).unwrap();
        assert!(
            cstate.configured,
            "client must receive initial xdg_surface.configure"
        );

        let (_tmp, pool, buffer) = create_shm_buffer(&shm, &qh, 320, 200);
        surface.attach(Some(&buffer), 0, 0);
        surface.damage_buffer(0, 0, 320, 200);
        surface.commit();
        queue.roundtrip(&mut cstate).unwrap();

        client_mapped_clone.store(true, Ordering::SeqCst);

        let deadline = Instant::now() + Duration::from_secs(5);
        while !allow_exit_clone.load(Ordering::SeqCst) && Instant::now() < deadline {
            queue.roundtrip(&mut cstate).unwrap();
            thread::sleep(Duration::from_millis(10));
        }
        queue.roundtrip(&mut cstate).unwrap();

        toplevel.destroy();
        xdg_surface.destroy();
        buffer.destroy();
        pool.destroy();
        surface.destroy();
        conn.flush().unwrap();

        cstate
    });

    let deadline = Instant::now() + Duration::from_secs(5);
    while !client_mapped.load(Ordering::SeqCst) && Instant::now() < deadline {
        pump_server(&mut display, &mut state);
        thread::sleep(Duration::from_millis(5));
    }
    pump_server(&mut display, &mut state);

    assert_eq!(state.wm.len(), 1, "one window should be registered");
    let focused_id = state
        .wm
        .focused_id()
        .expect("newly mapped window must be focused");
    let record = state.wm.get(focused_id).unwrap();
    assert!(
        record.mapped,
        "window must be marked mapped after SHM commit"
    );
    assert_eq!(record.title.as_deref(), Some("NEON DE Test Terminal"));
    assert_eq!(record.app_id.as_deref(), Some("org.neonde.test-terminal"));
    assert_eq!(record.current_geometry.size.w, 320);
    assert_eq!(record.current_geometry.size.h, 200);

    let click_pos: Point<f64, Logical> = (
        f64::from(record.current_geometry.loc.x) + 20.0,
        f64::from(record.current_geometry.loc.y) + 20.0,
    )
        .into();
    let under = state.surface_under(click_pos);
    assert!(
        under.is_some(),
        "surface_under must resolve the mapped window at {click_pos:?}"
    );
    if let Some(pointer) = state.seat.get_pointer() {
        pointer.motion(
            &mut state,
            under,
            &MotionEvent {
                location: click_pos,
                serial: SERIAL_COUNTER.next_serial(),
                time: 90,
            },
        );
        pointer.button(
            &mut state,
            &ButtonEvent {
                button: 0x110,
                state: ButtonState::Pressed,
                serial: SERIAL_COUNTER.next_serial(),
                time: 95,
            },
        );
        pointer.frame(&mut state);
    }

    if let Some(kbd) = state.seat.get_keyboard() {
        let serial = SERIAL_COUNTER.next_serial();
        kbd.input::<(), _>(
            &mut state,
            38u32.into(),
            KeyState::Pressed,
            serial,
            100,
            |_, _, _| FilterResult::Forward,
        );
    }
    state.close_focused_window();
    pump_server(&mut display, &mut state);

    allow_exit.store(true, Ordering::SeqCst);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !client_handle.is_finished() && Instant::now() < deadline {
        pump_server(&mut display, &mut state);
        thread::sleep(Duration::from_millis(5));
    }

    let cstate = client_handle.join().expect("client thread exited cleanly");
    for _ in 0..5 {
        pump_server(&mut display, &mut state);
        thread::sleep(Duration::from_millis(5));
    }

    assert!(cstate.received_keymap);
    assert!(cstate.received_keyboard_enter);
    assert!(cstate.received_keys.contains(&30));
    assert!(cstate.received_pointer_enter);
    assert!(cstate.received_pointer_buttons.contains(&0x110));
    assert!(cstate.received_close);
    assert!(state.wm.is_empty());
    assert_eq!(state.space.elements().count(), 0);
    assert_eq!(state.wm.focused_id(), None);
}

#[test]
fn multiple_windows_cascade_focus_and_cycle() {
    let (_event_loop, mut display, mut state) = create_headless_server();
    let (server_sock, client_sock) = UnixStream::pair().unwrap();
    state.insert_client(server_sock);

    let both_mapped = Arc::new(AtomicBool::new(false));
    let both_mapped_clone = Arc::clone(&both_mapped);
    let allow_close_first = Arc::new(AtomicBool::new(false));
    let allow_close_first_clone = Arc::clone(&allow_close_first);
    let first_closed = Arc::new(AtomicBool::new(false));
    let first_closed_clone = Arc::clone(&first_closed);
    let allow_exit = Arc::new(AtomicBool::new(false));
    let allow_exit_clone = Arc::clone(&allow_exit);

    let client_handle = thread::spawn(move || {
        let conn = Connection::from_socket(client_sock).unwrap();
        let mut queue = conn.new_event_queue::<TestClientState>();
        let qh = queue.handle();
        let mut cstate = TestClientState::default();

        let _registry = conn.display().get_registry(&qh, ());
        queue.roundtrip(&mut cstate).unwrap();

        let compositor = cstate.compositor.clone().unwrap();
        let shm = cstate.shm.clone().unwrap();
        let wm_base = cstate.wm_base.clone().unwrap();

        let s1 = compositor.create_surface(&qh, ());
        let xs1 = wm_base.get_xdg_surface(&s1, &qh, ());
        let t1 = xs1.get_toplevel(&qh, ());
        t1.set_title("Window One".into());
        s1.commit();
        queue.roundtrip(&mut cstate).unwrap();
        let (_f1, p1, b1) = create_shm_buffer(&shm, &qh, 400, 300);
        s1.attach(Some(&b1), 0, 0);
        s1.commit();

        let s2 = compositor.create_surface(&qh, ());
        let xs2 = wm_base.get_xdg_surface(&s2, &qh, ());
        let t2 = xs2.get_toplevel(&qh, ());
        t2.set_title("Window Two".into());
        s2.commit();
        queue.roundtrip(&mut cstate).unwrap();
        let (_f2, p2, b2) = create_shm_buffer(&shm, &qh, 420, 320);
        s2.attach(Some(&b2), 0, 0);
        s2.commit();
        queue.roundtrip(&mut cstate).unwrap();

        both_mapped_clone.store(true, Ordering::SeqCst);

        let deadline = Instant::now() + Duration::from_secs(5);
        while !allow_close_first_clone.load(Ordering::SeqCst) && Instant::now() < deadline {
            queue.roundtrip(&mut cstate).unwrap();
            thread::sleep(Duration::from_millis(10));
        }

        t1.destroy();
        xs1.destroy();
        b1.destroy();
        p1.destroy();
        s1.destroy();
        queue.roundtrip(&mut cstate).unwrap();
        first_closed_clone.store(true, Ordering::SeqCst);

        while !allow_exit_clone.load(Ordering::SeqCst) && Instant::now() < deadline {
            queue.roundtrip(&mut cstate).unwrap();
            thread::sleep(Duration::from_millis(10));
        }

        t2.destroy();
        xs2.destroy();
        b2.destroy();
        p2.destroy();
        s2.destroy();
        conn.flush().unwrap();
    });

    let deadline = Instant::now() + Duration::from_secs(5);
    while !both_mapped.load(Ordering::SeqCst) && Instant::now() < deadline {
        pump_server(&mut display, &mut state);
        thread::sleep(Duration::from_millis(5));
    }
    pump_server(&mut display, &mut state);

    assert_eq!(state.wm.len(), 2);

    let ids: Vec<_> = state
        .space
        .elements()
        .filter_map(neon_compositor::window::window_id)
        .collect();
    assert_eq!(ids.len(), 2);
    let loc0 = state.wm.get(ids[0]).unwrap().current_geometry.loc;
    let loc1 = state.wm.get(ids[1]).unwrap().current_geometry.loc;
    assert_ne!(
        loc0, loc1,
        "floating windows must cascade to distinct locations"
    );

    let before_cycle = state.wm.focused_id().unwrap();
    state.cycle_focus(SERIAL_COUNTER.next_serial());
    let after_cycle = state.wm.focused_id().unwrap();
    assert_ne!(before_cycle, after_cycle);
    assert_eq!(
        state.wm.get(after_cycle).unwrap().title.as_deref(),
        Some("Window One")
    );

    allow_close_first.store(true, Ordering::SeqCst);
    while !first_closed.load(Ordering::SeqCst) && Instant::now() < deadline {
        pump_server(&mut display, &mut state);
        thread::sleep(Duration::from_millis(5));
    }
    pump_server(&mut display, &mut state);

    assert_eq!(state.wm.len(), 1);
    let remaining_focus = state.wm.focused_id().expect("Window Two should gain focus");
    assert_eq!(
        state.wm.get(remaining_focus).unwrap().title.as_deref(),
        Some("Window Two")
    );

    let kbd_focus = state
        .seat
        .get_keyboard()
        .and_then(|k| k.current_focus())
        .and_then(|f| f.wl_surface_id())
        .expect("keyboard focus must point to Window Two");
    let win2 = state.find_window_by_id(remaining_focus).unwrap();
    assert_eq!(kbd_focus, win2.toplevel().unwrap().wl_surface().id());

    allow_exit.store(true, Ordering::SeqCst);
    while !client_handle.is_finished() && Instant::now() < deadline {
        pump_server(&mut display, &mut state);
        thread::sleep(Duration::from_millis(5));
    }
    client_handle.join().unwrap();
    for _ in 0..5 {
        pump_server(&mut display, &mut state);
        thread::sleep(Duration::from_millis(5));
    }
    assert!(state.wm.is_empty());
}

#[test]
fn workspace_switching_window_movement_and_dynamic_pruning() {
    let (_event_loop, mut display, mut state) = create_headless_server();
    let (server_sock, client_sock) = UnixStream::pair().unwrap();
    state.insert_client(server_sock);

    let both_mapped = Arc::new(AtomicBool::new(false));
    let both_mapped_clone = Arc::clone(&both_mapped);
    let allow_close_second = Arc::new(AtomicBool::new(false));
    let allow_close_second_clone = Arc::clone(&allow_close_second);
    let second_closed = Arc::new(AtomicBool::new(false));
    let second_closed_clone = Arc::clone(&second_closed);
    let allow_exit = Arc::new(AtomicBool::new(false));
    let allow_exit_clone = Arc::clone(&allow_exit);

    let client_handle = thread::spawn(move || {
        let conn = Connection::from_socket(client_sock).unwrap();
        let mut queue = conn.new_event_queue::<TestClientState>();
        let qh = queue.handle();
        let mut cstate = TestClientState::default();

        let _registry = conn.display().get_registry(&qh, ());
        queue.roundtrip(&mut cstate).unwrap();

        let compositor = cstate.compositor.clone().unwrap();
        let shm = cstate.shm.clone().unwrap();
        let wm_base = cstate.wm_base.clone().unwrap();

        let s1 = compositor.create_surface(&qh, ());
        let xs1 = wm_base.get_xdg_surface(&s1, &qh, ());
        let t1 = xs1.get_toplevel(&qh, ());
        t1.set_title("Workspace 1 App".into());
        s1.commit();
        queue.roundtrip(&mut cstate).unwrap();
        let (_f1, p1, b1) = create_shm_buffer(&shm, &qh, 400, 300);
        s1.attach(Some(&b1), 0, 0);
        s1.commit();

        let s2 = compositor.create_surface(&qh, ());
        let xs2 = wm_base.get_xdg_surface(&s2, &qh, ());
        let t2 = xs2.get_toplevel(&qh, ());
        t2.set_title("Workspace 2 App".into());
        s2.commit();
        queue.roundtrip(&mut cstate).unwrap();
        let (_f2, p2, b2) = create_shm_buffer(&shm, &qh, 400, 300);
        s2.attach(Some(&b2), 0, 0);
        s2.commit();
        queue.roundtrip(&mut cstate).unwrap();

        both_mapped_clone.store(true, Ordering::SeqCst);

        let deadline = Instant::now() + Duration::from_secs(5);
        while !allow_close_second_clone.load(Ordering::SeqCst) && Instant::now() < deadline {
            queue.roundtrip(&mut cstate).unwrap();
            thread::sleep(Duration::from_millis(10));
        }

        t2.destroy();
        xs2.destroy();
        b2.destroy();
        p2.destroy();
        s2.destroy();
        queue.roundtrip(&mut cstate).unwrap();
        second_closed_clone.store(true, Ordering::SeqCst);

        while !allow_exit_clone.load(Ordering::SeqCst) && Instant::now() < deadline {
            queue.roundtrip(&mut cstate).unwrap();
            thread::sleep(Duration::from_millis(10));
        }

        t1.destroy();
        xs1.destroy();
        b1.destroy();
        p1.destroy();
        s1.destroy();
        conn.flush().unwrap();
    });

    let deadline = Instant::now() + Duration::from_secs(5);
    while !both_mapped.load(Ordering::SeqCst) && Instant::now() < deadline {
        pump_server(&mut display, &mut state);
        thread::sleep(Duration::from_millis(5));
    }
    pump_server(&mut display, &mut state);

    // Initially: 2 windows on workspace 0, plus trailing empty workspace 1.
    assert_eq!(state.wm.workspaces().len(), 2);
    assert_eq!(state.space.elements().count(), 2);

    // Move focused "Workspace 2 App" to next workspace and follow it -> creates workspace 2.
    state
        .move_focused_to_next_workspace(true, SERIAL_COUNTER.next_serial())
        .expect("should move to workspace 1");
    assert_eq!(state.wm.workspaces().len(), 3);
    assert_eq!(state.wm.workspaces().active_index(), 1);
    assert_eq!(
        state.space.elements().count(),
        1,
        "only Workspace 2 App should be mapped in Space on workspace 1"
    );
    assert_eq!(
        state.managed_windows.len(),
        2,
        "both windows remain managed across workspaces"
    );

    // Switch back to workspace 0 -> Workspace 1 App is mapped and focused.
    state
        .switch_workspace_prev(SERIAL_COUNTER.next_serial())
        .expect("switch to ws 0");
    assert_eq!(state.space.elements().count(), 1);
    let focused_ws0 = state.wm.focused_id().unwrap();
    assert_eq!(
        state.wm.get(focused_ws0).unwrap().title.as_deref(),
        Some("Workspace 1 App")
    );

    // Switch back to workspace 1, close Workspace 2 App, then switch back to workspace 0 ->
    // empty workspace 1 is automatically pruned so only [ws0, trailing empty] remain!
    state
        .switch_workspace_next(SERIAL_COUNTER.next_serial())
        .expect("switch to ws 1");
    allow_close_second.store(true, Ordering::SeqCst);
    while !second_closed.load(Ordering::SeqCst) && Instant::now() < deadline {
        pump_server(&mut display, &mut state);
        thread::sleep(Duration::from_millis(5));
    }
    pump_server(&mut display, &mut state);

    state
        .switch_workspace_prev(SERIAL_COUNTER.next_serial())
        .expect("return to ws 0");
    assert_eq!(
        state.wm.workspaces().len(),
        2,
        "empty dynamic workspace must be pruned when left"
    );
    assert_eq!(state.space.elements().count(), 1);

    allow_exit.store(true, Ordering::SeqCst);
    while !client_handle.is_finished() && Instant::now() < deadline {
        pump_server(&mut display, &mut state);
        thread::sleep(Duration::from_millis(5));
    }
    client_handle.join().unwrap();
}

#[test]
fn rules_snap_minimize_restore_and_fullscreen_state_machine() {
    let (_event_loop, mut display, mut state) = create_headless_server();
    // Register a window rule that opens `org.neonde.ruled-app` maximized.
    state.wm.add_rule(WindowRule {
        name: "start-maximized".into(),
        matcher: WindowMatcher {
            app_id_exact: Some("org.neonde.ruled-app".into()),
            ..Default::default()
        },
        action: WindowRuleAction {
            initial_mode: Some(WindowMode::Maximized),
            ..Default::default()
        },
    });

    let (server_sock, client_sock) = UnixStream::pair().unwrap();
    state.insert_client(server_sock);

    let mapped = Arc::new(AtomicBool::new(false));
    let mapped_clone = Arc::clone(&mapped);
    let allow_exit = Arc::new(AtomicBool::new(false));
    let allow_exit_clone = Arc::clone(&allow_exit);

    let client_handle = thread::spawn(move || {
        let conn = Connection::from_socket(client_sock).unwrap();
        let mut queue = conn.new_event_queue::<TestClientState>();
        let qh = queue.handle();
        let mut cstate = TestClientState::default();

        let _registry = conn.display().get_registry(&qh, ());
        queue.roundtrip(&mut cstate).unwrap();

        let compositor = cstate.compositor.clone().unwrap();
        let shm = cstate.shm.clone().unwrap();
        let wm_base = cstate.wm_base.clone().unwrap();

        let surface = compositor.create_surface(&qh, ());
        let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
        let toplevel = xdg_surface.get_toplevel(&qh, ());
        toplevel.set_app_id("org.neonde.ruled-app".into());
        surface.commit();

        queue.roundtrip(&mut cstate).unwrap();
        // Initial configure must reflect the Maximized rule (1280x800).
        assert_eq!(cstate.last_configure_size, (1280, 800));
        assert!(
            cstate
                .last_configure_states
                .contains(&(xdg_toplevel::State::Maximized as u32))
        );

        let (_f, pool, buffer) = create_shm_buffer(&shm, &qh, 1280, 800);
        surface.attach(Some(&buffer), 0, 0);
        surface.commit();
        queue.roundtrip(&mut cstate).unwrap();

        mapped_clone.store(true, Ordering::SeqCst);

        let deadline = Instant::now() + Duration::from_secs(5);
        while !allow_exit_clone.load(Ordering::SeqCst) && Instant::now() < deadline {
            queue.roundtrip(&mut cstate).unwrap();
            thread::sleep(Duration::from_millis(10));
        }
        queue.roundtrip(&mut cstate).unwrap();

        toplevel.destroy();
        xdg_surface.destroy();
        buffer.destroy();
        pool.destroy();
        surface.destroy();
        conn.flush().unwrap();
        cstate
    });

    let deadline = Instant::now() + Duration::from_secs(5);
    while !mapped.load(Ordering::SeqCst) && Instant::now() < deadline {
        pump_server(&mut display, &mut state);
        thread::sleep(Duration::from_millis(5));
    }
    pump_server(&mut display, &mut state);

    let id = state.wm.focused_id().unwrap();
    assert_eq!(state.wm.get(id).unwrap().mode, WindowMode::Maximized);

    // Snap left half -> 640x800 at (0, 0)
    state.snap_focused(SnapRegion::LeftHalf);
    pump_server(&mut display, &mut state);
    assert_eq!(
        state.wm.get(id).unwrap().mode,
        WindowMode::Snapped(SnapRegion::LeftHalf)
    );
    assert_eq!(state.wm.get(id).unwrap().current_geometry.size.w, 640);

    // Minimize -> unmapped from Space and focus cleared
    state.minimize_focused(SERIAL_COUNTER.next_serial());
    pump_server(&mut display, &mut state);
    assert!(state.wm.get(id).unwrap().mode.is_minimized());
    assert_eq!(state.space.elements().count(), 0);
    assert_eq!(state.wm.focused_id(), None);

    // Restore -> remapped into Space back in Snapped(LeftHalf) and refocused
    state.restore_last_minimized(SERIAL_COUNTER.next_serial());
    pump_server(&mut display, &mut state);
    assert_eq!(
        state.wm.get(id).unwrap().mode,
        WindowMode::Snapped(SnapRegion::LeftHalf)
    );
    assert_eq!(state.space.elements().count(), 1);
    assert_eq!(state.wm.focused_id(), Some(id));

    // Toggle fullscreen -> Fullscreen (never simultaneously Maximized!)
    state.toggle_fullscreen_focused();
    pump_server(&mut display, &mut state);
    assert_eq!(state.wm.get(id).unwrap().mode, WindowMode::Fullscreen);

    allow_exit.store(true, Ordering::SeqCst);
    while !client_handle.is_finished() && Instant::now() < deadline {
        pump_server(&mut display, &mut state);
        thread::sleep(Duration::from_millis(5));
    }
    let cstate = client_handle.join().unwrap();
    assert_eq!(cstate.last_configure_size, (1280, 800));
    assert!(
        cstate
            .last_configure_states
            .contains(&(xdg_toplevel::State::Fullscreen as u32))
    );
    assert!(
        !cstate
            .last_configure_states
            .contains(&(xdg_toplevel::State::Maximized as u32)),
        "Fullscreen and Maximized must never be active simultaneously"
    );
}

#[test]
fn scene_graph_and_animations_sync_with_window_and_workspace_events() {
    let (_event_loop, mut display, mut state) = create_headless_server();
    let (server_sock, client_sock) = UnixStream::pair().unwrap();
    state.insert_client(server_sock);

    let mapped = Arc::new(AtomicBool::new(false));
    let mapped_clone = Arc::clone(&mapped);
    let allow_exit = Arc::new(AtomicBool::new(false));
    let allow_exit_clone = Arc::clone(&allow_exit);

    let client_handle = thread::spawn(move || {
        let conn = Connection::from_socket(client_sock).unwrap();
        let mut queue = conn.new_event_queue::<TestClientState>();
        let qh = queue.handle();
        let mut cstate = TestClientState::default();

        let _registry = conn.display().get_registry(&qh, ());
        queue.roundtrip(&mut cstate).unwrap();

        let compositor = cstate.compositor.clone().unwrap();
        let shm = cstate.shm.clone().unwrap();
        let wm_base = cstate.wm_base.clone().unwrap();

        let surface = compositor.create_surface(&qh, ());
        let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
        let toplevel = xdg_surface.get_toplevel(&qh, ());
        toplevel.set_title("Scene Test Window".into());
        surface.commit();
        queue.roundtrip(&mut cstate).unwrap();

        let (_f, pool, buffer) = create_shm_buffer(&shm, &qh, 480, 320);
        surface.attach(Some(&buffer), 0, 0);
        surface.commit();
        queue.roundtrip(&mut cstate).unwrap();

        mapped_clone.store(true, Ordering::SeqCst);

        let deadline = Instant::now() + Duration::from_secs(5);
        while !allow_exit_clone.load(Ordering::SeqCst) && Instant::now() < deadline {
            queue.roundtrip(&mut cstate).unwrap();
            thread::sleep(Duration::from_millis(10));
        }

        toplevel.destroy();
        xdg_surface.destroy();
        buffer.destroy();
        pool.destroy();
        surface.destroy();
        conn.flush().unwrap();
    });

    let deadline = Instant::now() + Duration::from_secs(5);
    while !mapped.load(Ordering::SeqCst) && Instant::now() < deadline {
        pump_server(&mut display, &mut state);
        thread::sleep(Duration::from_millis(5));
    }
    pump_server(&mut display, &mut state);

    // Sync scene graph right after mapping -> open animation is active and window has shadow & border.
    let animating = state.sync_scene_graph();
    assert!(animating, "window open animation should be in flight");
    let nodes = state.scene.collect_front_to_back();
    assert_eq!(nodes.len(), 2, "backdrop + 1 floating window");
    assert!(
        nodes[0].shadow.is_some(),
        "floating window must have drop shadow"
    );
    assert!(
        nodes[0].border.is_some(),
        "floating window must have focus border"
    );

    // Switching to empty workspace 1 immediately unmaps the window from interactive Space while
    // keeping it gliding off-screen in the SceneGraph during the Hyprland `slidefade` transition.
    state
        .switch_workspace_next(SERIAL_COUNTER.next_serial())
        .expect("switch to dynamic workspace 1");
    let slide_active = state.sync_scene_graph();
    assert!(
        slide_active,
        "workspace transition animation should be active"
    );
    assert_eq!(
        state.space.elements().count(),
        0,
        "departing workspace window must be immediately non-interactive on workspace 1"
    );
    assert_eq!(
        state.scene.collect_front_to_back().len(),
        2,
        "backdrop + outgoing workspace window gliding off-screen mid-transition"
    );

    // Once the Hyprland workspace transition completes, the outgoing window is pruned cleanly.
    thread::sleep(Duration::from_millis(430));
    state.sync_scene_graph();
    assert!(
        state.outgoing_workspace_windows.is_empty(),
        "outgoing workspace snapshot must clear after slide completes"
    );
    assert_eq!(
        state.scene.collect_front_to_back().len(),
        1,
        "only backdrop remains visible on empty workspace 1 after slide settles"
    );

    allow_exit.store(true, Ordering::SeqCst);
    while !client_handle.is_finished() && Instant::now() < deadline {
        pump_server(&mut display, &mut state);
        thread::sleep(Duration::from_millis(5));
    }
    client_handle.join().unwrap();
}

#[test]
fn ipc_server_state_overview_workspace_and_effects_roundtrip() {
    use neon_compositor::ipc::IpcServer;
    use neon_ipc::{IpcClient, IpcPayload, IpcRequest, IpcResult};

    let (_event_loop, _display, mut state) = create_headless_server();
    let dir = tempfile::tempdir().unwrap();
    let ipc_path = dir.path().join("neon-ipc-test.sock");
    let mut ipc_server = IpcServer::bind(ipc_path.clone()).unwrap();

    let client_done = Arc::new(AtomicBool::new(false));
    let client_done_clone = Arc::clone(&client_done);

    let client_thread = thread::spawn(move || {
        let mut client = IpcClient::connect(&ipc_path).unwrap();

        let state_res = client.request(IpcRequest::GetState).unwrap();
        let IpcResult::Ok {
            payload: IpcPayload::State { snapshot },
        } = state_res
        else {
            panic!("Expected State payload");
        };
        assert!(!snapshot.overview_open);
        assert_eq!(snapshot.workspaces.len(), 1);
        assert_eq!(snapshot.outputs.len(), 1);

        let ov_res = client.request(IpcRequest::ToggleOverview).unwrap();
        assert_eq!(
            ov_res,
            IpcResult::Ok {
                payload: IpcPayload::Overview { open: true }
            }
        );

        let fx_res = client
            .request(IpcRequest::UpdateEffects {
                enabled: None,
                reduced_motion: Some(true),
                shadows_enabled: Some(false),
                borders_enabled: None,
                blur_enabled: Some(false),
                blur_strength: Some(1.5),
                inactive_opacity: Some(0.9),
                corner_radius: Some(16.0),
                speed_scale: None,
            })
            .unwrap();
        let IpcResult::Ok {
            payload: IpcPayload::Effects { effects },
        } = fx_res
        else {
            panic!("Expected Effects payload");
        };
        assert!(effects.reduced_motion);
        assert!(!effects.shadows_enabled);
        assert!(!effects.blur_enabled);
        assert!((effects.corner_radius - 16.0).abs() < 1e-4);

        // Configure fractional scaling via IPC ConfigureOutput
        let out_res = client
            .request(IpcRequest::ConfigureOutput {
                name: "NEON-Test-1".into(),
                x: None,
                y: None,
                scale: Some(1.5),
                primary: Some(true),
                transform: Some("normal".into()),
            })
            .unwrap();
        let IpcResult::Ok {
            payload: IpcPayload::Outputs { outputs },
        } = out_res
        else {
            panic!("Expected Outputs payload");
        };
        assert_eq!(outputs.len(), 1);
        assert!((outputs[0].scale - 1.5).abs() < 1e-4);

        // Live appearance & window behavior updates (used by neon-settings)
        let app_res = client
            .request(IpcRequest::from(neon_ipc::AppearanceUpdate {
                theme: Some("neon_light".into()),
                accent_color: Some("#A855F7".into()),
                dock_transparency: Some(0.9),
                panel_transparency: Some(0.92),
                panel_visible: Some(true),
                show_clock: Some(true),
                show_workspaces: Some(true),
                high_contrast: Some(true),
                reduce_blur: Some(true),
                blur_strength: Some(0.4),
                cursor_theme: Some("macOS".into()),
                cursor_size: Some(20),
                ..Default::default()
            }))
            .unwrap();
        let IpcResult::Ok {
            payload: IpcPayload::Appearance { appearance },
        } = app_res
        else {
            panic!("Expected Appearance payload");
        };
        assert_eq!(appearance.theme, "neon_light");
        assert_eq!(appearance.accent_color, "#A855F7");
        assert!(appearance.high_contrast);
        assert!(appearance.reduce_blur);
        assert_eq!(appearance.cursor_theme.as_deref(), Some("macOS"));
        assert_eq!(appearance.cursor_size, Some(20));

        let diag_res = client.request(IpcRequest::GetDiagnostics).unwrap();
        let IpcResult::Ok {
            payload: IpcPayload::Diagnostics { diagnostics },
        } = diag_res
        else {
            panic!("Expected Diagnostics payload");
        };
        assert_eq!(diagnostics.output_count, 1);
        assert!(!diagnostics.safe_mode);

        let wb_res = client
            .request(IpcRequest::UpdateWindowBehavior {
                focus_follows_mouse: Some(true),
                snap_gap: Some(12),
                button_layout: Some("right_standard".into()),
            })
            .unwrap();
        let IpcResult::Ok {
            payload: IpcPayload::WindowBehavior { behavior },
        } = wb_res
        else {
            panic!("Expected WindowBehavior payload");
        };
        assert!(behavior.focus_follows_mouse);
        assert_eq!(behavior.snap_gap, 12);
        assert_eq!(behavior.button_layout, "right_standard");

        client_done_clone.store(true, Ordering::SeqCst);
    });

    let deadline = Instant::now() + Duration::from_secs(5);
    while !client_done.load(Ordering::SeqCst) && Instant::now() < deadline {
        ipc_server.poll_requests(&mut state);
        thread::sleep(Duration::from_millis(5));
    }
    client_thread.join().unwrap();

    assert!(state.overview_open);
    assert!(state.effects.config.reduced_motion);
    assert!(!state.effects.config.shadows_enabled);
    assert!(!state.effects.config.blur_enabled);
}

#[test]
fn multi_monitor_hotplug_disconnect_migrates_safely() {
    let (_event_loop, _display, mut state) = create_headless_server();
    let second_output = Output::new(
        "HDMI-A-1".to_string(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "NEON DE".into(),
            model: "Secondary Monitor".into(),
        },
    );
    let mode = Mode {
        size: (1920, 1080).into(),
        refresh: 60_000,
    };
    second_output.change_current_state(
        Some(mode),
        Some(Transform::Normal),
        None,
        Some((1280, 0).into()),
    );
    second_output.set_preferred(mode);

    let first_id = state.output_manager.primary_output().unwrap().id;
    let second_id = state.register_output(second_output, (1280, 0).into());
    assert_eq!(state.output_manager.all_outputs().len(), 2);

    // Hotplug-disconnect primary monitor -> secondary becomes primary seamlessly
    assert!(state.disconnect_output(first_id));
    assert_eq!(state.output_manager.all_outputs().len(), 1);
    assert_eq!(state.output_manager.primary_output().unwrap().id, second_id);
    assert_eq!(state.primary_output.as_ref().unwrap().name(), "HDMI-A-1");
}

#[test]
fn ext_session_lock_isolates_focus_and_restores_on_unlock() {
    use wayland_protocols::ext::session_lock::v1::client::{
        ext_session_lock_manager_v1, ext_session_lock_surface_v1, ext_session_lock_v1,
    };

    #[derive(Default)]
    struct LockTestClient {
        compositor: Option<wl_compositor::WlCompositor>,
        output: Option<wayland_client::protocol::wl_output::WlOutput>,
        lock_mgr: Option<ext_session_lock_manager_v1::ExtSessionLockManagerV1>,
        locked_event: bool,
        configured_size: (u32, u32),
    }

    impl Dispatch<wl_registry::WlRegistry, ()> for LockTestClient {
        fn event(
            state: &mut Self,
            reg: &wl_registry::WlRegistry,
            event: wl_registry::Event,
            _: &(),
            _: &Connection,
            qh: &QueueHandle<Self>,
        ) {
            if let wl_registry::Event::Global {
                name,
                interface,
                version,
            } = event
            {
                match interface.as_str() {
                    "wl_compositor" => {
                        state.compositor = Some(reg.bind(name, version.min(5), qh, ()))
                    }
                    "wl_output" => state.output = Some(reg.bind(name, version.min(4), qh, ())),
                    "ext_session_lock_manager_v1" => {
                        state.lock_mgr = Some(reg.bind(name, 1, qh, ()))
                    }
                    _ => {}
                }
            }
        }
    }
    impl Dispatch<wl_compositor::WlCompositor, ()> for LockTestClient {
        fn event(
            _: &mut Self,
            _: &wl_compositor::WlCompositor,
            _: wl_compositor::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }
    impl Dispatch<wl_surface::WlSurface, ()> for LockTestClient {
        fn event(
            _: &mut Self,
            _: &wl_surface::WlSurface,
            _: wl_surface::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }
    impl Dispatch<wayland_client::protocol::wl_output::WlOutput, ()> for LockTestClient {
        fn event(
            _: &mut Self,
            _: &wayland_client::protocol::wl_output::WlOutput,
            _: wayland_client::protocol::wl_output::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }
    impl Dispatch<ext_session_lock_manager_v1::ExtSessionLockManagerV1, ()> for LockTestClient {
        fn event(
            _: &mut Self,
            _: &ext_session_lock_manager_v1::ExtSessionLockManagerV1,
            _: ext_session_lock_manager_v1::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }
    impl Dispatch<ext_session_lock_v1::ExtSessionLockV1, ()> for LockTestClient {
        fn event(
            state: &mut Self,
            _: &ext_session_lock_v1::ExtSessionLockV1,
            event: ext_session_lock_v1::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            if let ext_session_lock_v1::Event::Locked = event {
                state.locked_event = true;
            }
        }
    }
    impl Dispatch<ext_session_lock_surface_v1::ExtSessionLockSurfaceV1, ()> for LockTestClient {
        fn event(
            state: &mut Self,
            ls: &ext_session_lock_surface_v1::ExtSessionLockSurfaceV1,
            event: ext_session_lock_surface_v1::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            if let ext_session_lock_surface_v1::Event::Configure {
                serial,
                width,
                height,
            } = event
            {
                ls.ack_configure(serial);
                state.configured_size = (width, height);
            }
        }
    }

    let (_event_loop, mut display, mut state) = create_headless_server();
    let (server_sock, client_sock) = UnixStream::pair().unwrap();
    state.insert_client(server_sock);

    let locked_flag = Arc::new(AtomicBool::new(false));
    let locked_flag_clone = Arc::clone(&locked_flag);
    let allow_unlock = Arc::new(AtomicBool::new(false));
    let allow_unlock_clone = Arc::clone(&allow_unlock);

    let handle = thread::spawn(move || {
        let conn = Connection::from_socket(client_sock).unwrap();
        let mut queue = conn.new_event_queue::<LockTestClient>();
        let qh = queue.handle();
        let mut cstate = LockTestClient::default();

        let _reg = conn.display().get_registry(&qh, ());
        queue.roundtrip(&mut cstate).unwrap();

        let comp = cstate.compositor.clone().unwrap();
        let output = cstate.output.clone().unwrap();
        let mgr = cstate
            .lock_mgr
            .clone()
            .expect("ext_session_lock_manager_v1 advertised");

        let lock = mgr.lock(&qh, ());
        let surf = comp.create_surface(&qh, ());
        let lock_surf = lock.get_lock_surface(&surf, &output, &qh, ());
        queue.roundtrip(&mut cstate).unwrap();

        assert!(
            cstate.locked_event,
            "client must receive Locked confirmation"
        );
        assert_eq!(cstate.configured_size, (1280, 800));
        locked_flag_clone.store(true, Ordering::SeqCst);

        let deadline = Instant::now() + Duration::from_secs(5);
        while !allow_unlock_clone.load(Ordering::SeqCst) && Instant::now() < deadline {
            queue.roundtrip(&mut cstate).unwrap();
            thread::sleep(Duration::from_millis(10));
        }

        lock_surf.destroy();
        surf.destroy();
        lock.unlock_and_destroy();
        let _ = queue.roundtrip(&mut cstate);
        conn.flush().unwrap();
    });

    let deadline = Instant::now() + Duration::from_secs(5);
    while !locked_flag.load(Ordering::SeqCst) && Instant::now() < deadline {
        pump_server(&mut display, &mut state);
        thread::sleep(Duration::from_millis(5));
    }
    pump_server(&mut display, &mut state);

    assert!(state.is_locked(), "compositor must report locked state");
    assert!(
        state.lock_state.primary_surface().is_some(),
        "lock surface must be registered"
    );

    allow_unlock.store(true, Ordering::SeqCst);
    while !handle.is_finished() && Instant::now() < deadline {
        pump_server(&mut display, &mut state);
        thread::sleep(Duration::from_millis(5));
    }
    handle.join().unwrap();
    for _ in 0..5 {
        pump_server(&mut display, &mut state);
        thread::sleep(Duration::from_millis(5));
    }

    assert!(
        !state.is_locked(),
        "compositor must return to unlocked state cleanly"
    );
}

#[test]
fn relative_pointer_motion_and_vt_switch_work_for_bare_metal_input() {
    use neon_compositor::input::process_input_event;
    use smithay::backend::input::{
        Device, DeviceCapability, Event as InputEventTrait, InputBackend, InputEvent, KeyState,
        KeyboardKeyEvent, PointerMotionEvent, UnusedEvent,
    };
    use std::sync::{Arc, Mutex};

    struct MockBackend;
    #[derive(Debug, Clone, PartialEq, Eq, Hash)]
    struct MockDevice;
    impl Device for MockDevice {
        fn id(&self) -> String {
            "mock".into()
        }
        fn name(&self) -> String {
            "Mock Libinput".into()
        }
        fn has_capability(&self, _: DeviceCapability) -> bool {
            true
        }
        fn usb_id(&self) -> Option<(u32, u32)> {
            None
        }
        fn syspath(&self) -> Option<std::path::PathBuf> {
            None
        }
    }
    struct MockMotion {
        dx: f64,
        dy: f64,
    }
    impl InputEventTrait<MockBackend> for MockMotion {
        fn time(&self) -> u64 {
            1000
        }
        fn device(&self) -> MockDevice {
            MockDevice
        }
    }
    impl PointerMotionEvent<MockBackend> for MockMotion {
        fn delta_x(&self) -> f64 {
            self.dx
        }
        fn delta_y(&self) -> f64 {
            self.dy
        }
        fn delta_x_unaccel(&self) -> f64 {
            self.dx
        }
        fn delta_y_unaccel(&self) -> f64 {
            self.dy
        }
    }
    struct MockKey {
        code: u32,
        state: KeyState,
    }
    impl InputEventTrait<MockBackend> for MockKey {
        fn time(&self) -> u64 {
            2000
        }
        fn device(&self) -> MockDevice {
            MockDevice
        }
    }
    impl KeyboardKeyEvent<MockBackend> for MockKey {
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
    impl InputBackend for MockBackend {
        type Device = MockDevice;
        type KeyboardKeyEvent = MockKey;
        type PointerAxisEvent = UnusedEvent;
        type PointerButtonEvent = UnusedEvent;
        type PointerMotionEvent = MockMotion;
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

    let (_event_loop, _display, mut state) = create_headless_server();
    let start = state.pointer_location;
    process_input_event::<MockBackend>(
        &mut state,
        InputEvent::PointerMotion {
            event: MockMotion {
                dx: 120.0,
                dy: -50.0,
            },
        },
    );
    assert_eq!(state.pointer_location.x, start.x + 120.0);
    assert_eq!(state.pointer_location.y, start.y - 50.0);

    // Clamping prevents cursor from escaping output bounds
    process_input_event::<MockBackend>(
        &mut state,
        InputEvent::PointerMotion {
            event: MockMotion {
                dx: 9999.0,
                dy: 9999.0,
            },
        },
    );
    assert_eq!(state.pointer_location.x, 1279.0);
    assert_eq!(state.pointer_location.y, 799.0);

    // Verify Ctrl+Alt+F3 invokes vt_switch_hook(3)
    let switched_vt = Arc::new(Mutex::new(None::<i32>));
    let switched_clone = Arc::clone(&switched_vt);
    state.vt_switch_hook = Some(Box::new(move |vt| {
        *switched_clone.lock().unwrap() = Some(vt);
    }));
    // XKB keycodes: Left Ctrl = 37 (evdev 29+8), Left Alt = 64 (evdev 56+8), F3 = 69 (evdev 61+8)
    for code in [37u32, 64, 69] {
        process_input_event::<MockBackend>(
            &mut state,
            InputEvent::Keyboard {
                event: MockKey {
                    code,
                    state: KeyState::Pressed,
                },
            },
        );
    }
    assert_eq!(*switched_vt.lock().unwrap(), Some(3));
}

#[test]
fn app_drawer_and_relayout_preserve_maximized_window_z_order() {
    let (_event_loop, mut display, mut state) = create_headless_server();
    let (server_sock, client_sock) = UnixStream::pair().unwrap();
    state.insert_client(server_sock);

    let mapped = Arc::new(AtomicBool::new(false));
    let mapped_clone = Arc::clone(&mapped);
    let allow_exit = Arc::new(AtomicBool::new(false));
    let allow_exit_clone = Arc::clone(&allow_exit);

    let client_handle = thread::spawn(move || {
        let conn = Connection::from_socket(client_sock).unwrap();
        let mut queue = conn.new_event_queue::<TestClientState>();
        let qh = queue.handle();
        let mut cstate = TestClientState::default();

        let _registry = conn.display().get_registry(&qh, ());
        queue.roundtrip(&mut cstate).unwrap();

        let compositor = cstate.compositor.clone().unwrap();
        let shm = cstate.shm.clone().unwrap();
        let wm_base = cstate.wm_base.clone().unwrap();

        let mut windows = Vec::new();
        for title in ["Chrome", "Editor", "Terminal"] {
            let surface = compositor.create_surface(&qh, ());
            let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
            let toplevel = xdg_surface.get_toplevel(&qh, ());
            toplevel.set_title(title.into());
            toplevel.set_maximized();
            surface.commit();
            queue.roundtrip(&mut cstate).unwrap();

            let (f, pool, buffer) = create_shm_buffer(&shm, &qh, 1280, 800);
            surface.attach(Some(&buffer), 0, 0);
            surface.commit();
            queue.roundtrip(&mut cstate).unwrap();
            windows.push((surface, xdg_surface, toplevel, f, pool, buffer));
        }

        mapped_clone.store(true, Ordering::SeqCst);
        let deadline = Instant::now() + Duration::from_secs(5);
        while !allow_exit_clone.load(Ordering::SeqCst) && Instant::now() < deadline {
            queue.roundtrip(&mut cstate).unwrap();
            thread::sleep(Duration::from_millis(10));
        }
        for (surface, xdg_surface, toplevel, _f, pool, buffer) in windows {
            toplevel.destroy();
            xdg_surface.destroy();
            buffer.destroy();
            pool.destroy();
            surface.destroy();
        }
        conn.flush().unwrap();
    });

    let deadline = Instant::now() + Duration::from_secs(5);
    while !mapped.load(Ordering::SeqCst) && Instant::now() < deadline {
        pump_server(&mut display, &mut state);
        thread::sleep(Duration::from_millis(5));
    }
    pump_server(&mut display, &mut state);

    // Find Chrome (lowest WindowId) and focus/raise it above Editor and Terminal.
    let mut ids = state.wm.all_window_ids();
    ids.sort_by_key(|w| w.0);
    assert_eq!(ids.len(), 3);
    let chrome_id = ids[0];
    let chrome_win = state.find_window_by_id(chrome_id).unwrap();
    state.focus_window(Some(&chrome_win), SERIAL_COUNTER.next_serial());

    let stack_before: Vec<_> = state
        .space
        .elements()
        .filter_map(|w| state.window_id_for_window(w))
        .collect();
    assert_eq!(stack_before.last(), Some(&chrome_id));

    // Simulate opening & closing the App Drawer AND firing relayout_non_floating_windows().
    state.set_overview_open(true, SERIAL_COUNTER.next_serial());
    state.relayout_non_floating_windows();
    state.set_overview_open(false, SERIAL_COUNTER.next_serial());
    state.relayout_non_floating_windows();

    let stack_after: Vec<_> = state
        .space
        .elements()
        .filter_map(|w| state.window_id_for_window(w))
        .collect();
    assert_eq!(
        stack_after, stack_before,
        "App Drawer open/close and relayout must never bury the active window behind other windows"
    );
    assert_eq!(state.wm.focused_id(), Some(chrome_id));

    allow_exit.store(true, Ordering::SeqCst);
    while !client_handle.is_finished() && Instant::now() < deadline {
        pump_server(&mut display, &mut state);
        thread::sleep(Duration::from_millis(5));
    }
    client_handle.join().unwrap();
}

#[test]
fn transient_child_dialog_stays_above_fullscreen_parent() {
    let (_event_loop, mut display, mut state) = create_headless_server();
    let (server_sock, client_sock) = UnixStream::pair().unwrap();
    state.insert_client(server_sock);

    let mapped = Arc::new(AtomicBool::new(false));
    let mapped_clone = Arc::clone(&mapped);
    let allow_exit = Arc::new(AtomicBool::new(false));
    let allow_exit_clone = Arc::clone(&allow_exit);

    let client_handle = thread::spawn(move || {
        let conn = Connection::from_socket(client_sock).unwrap();
        let mut queue = conn.new_event_queue::<TestClientState>();
        let qh = queue.handle();
        let mut cstate = TestClientState::default();

        let _registry = conn.display().get_registry(&qh, ());
        queue.roundtrip(&mut cstate).unwrap();

        let compositor = cstate.compositor.clone().unwrap();
        let shm = cstate.shm.clone().unwrap();
        let wm_base = cstate.wm_base.clone().unwrap();

        // 1. Main fullscreen window (e.g. DaVinci Resolve main window)
        let p_surf = compositor.create_surface(&qh, ());
        let p_xdg = wm_base.get_xdg_surface(&p_surf, &qh, ());
        let p_top = p_xdg.get_toplevel(&qh, ());
        p_top.set_title("Main App".into());
        p_top.set_fullscreen(None);
        p_surf.commit();
        queue.roundtrip(&mut cstate).unwrap();
        let (_pf, p_pool, p_buf) = create_shm_buffer(&shm, &qh, 1280, 800);
        p_surf.attach(Some(&p_buf), 0, 0);
        p_surf.commit();
        queue.roundtrip(&mut cstate).unwrap();

        // 2. Child transient dialog (e.g. "New Project")
        let d_surf = compositor.create_surface(&qh, ());
        let d_xdg = wm_base.get_xdg_surface(&d_surf, &qh, ());
        let d_top = d_xdg.get_toplevel(&qh, ());
        d_top.set_title("New Project".into());
        d_top.set_parent(Some(&p_top));
        d_surf.commit();
        queue.roundtrip(&mut cstate).unwrap();
        let (_df, d_pool, d_buf) = create_shm_buffer(&shm, &qh, 480, 320);
        d_surf.attach(Some(&d_buf), 0, 0);
        d_surf.commit();
        queue.roundtrip(&mut cstate).unwrap();

        mapped_clone.store(true, Ordering::SeqCst);
        let deadline = Instant::now() + Duration::from_secs(5);
        while !allow_exit_clone.load(Ordering::SeqCst) && Instant::now() < deadline {
            queue.roundtrip(&mut cstate).unwrap();
            thread::sleep(Duration::from_millis(10));
        }
        d_top.destroy();
        d_xdg.destroy();
        d_buf.destroy();
        d_pool.destroy();
        d_surf.destroy();
        p_top.destroy();
        p_xdg.destroy();
        p_buf.destroy();
        p_pool.destroy();
        p_surf.destroy();
        conn.flush().unwrap();
    });

    let deadline = Instant::now() + Duration::from_secs(5);
    while !mapped.load(Ordering::SeqCst) && Instant::now() < deadline {
        pump_server(&mut display, &mut state);
        thread::sleep(Duration::from_millis(5));
    }
    pump_server(&mut display, &mut state);

    let mut ids = state.wm.all_window_ids();
    ids.sort_by_key(|w| w.0);
    assert_eq!(ids.len(), 2);
    let parent_id = ids[0];
    let dialog_id = ids[1];

    // Parent must be full-output fullscreen (0,0,1280,800) and trigger fullscreen layer yielding.
    let parent_rec = state.wm.get(parent_id).unwrap();
    assert_eq!(parent_rec.mode, WindowMode::Fullscreen);
    assert_eq!(parent_rec.current_geometry.loc.x, 0);
    assert_eq!(parent_rec.current_geometry.loc.y, 0);
    assert_eq!(parent_rec.current_geometry.size.w, 1280);
    assert_eq!(parent_rec.current_geometry.size.h, 800);
    assert!(state.has_visible_fullscreen_window());

    // Dialog must be stacked above parent.
    let parent_win = state.find_window_by_id(parent_id).unwrap();
    let z_order = |s: &NeonState| -> Vec<_> {
        s.space
            .elements()
            .filter_map(|w| s.window_id_for_window(w))
            .collect()
    };
    assert_eq!(z_order(&state), vec![parent_id, dialog_id]);

    // Even if the parent is clicked/re-focused or reconfigured, its transient dialog stays on top!
    state.focus_window(Some(&parent_win), SERIAL_COUNTER.next_serial());
    assert_eq!(
        z_order(&state),
        vec![parent_id, dialog_id],
        "Transient dialog must stay above parent when parent is focused"
    );
    state.map_preserving_z_order(parent_win, (0, 0).into());
    assert_eq!(
        z_order(&state),
        vec![parent_id, dialog_id],
        "Parent configure must never jump above its transient dialog"
    );

    allow_exit.store(true, Ordering::SeqCst);
    while !client_handle.is_finished() && Instant::now() < deadline {
        pump_server(&mut display, &mut state);
        thread::sleep(Duration::from_millis(5));
    }
    client_handle.join().unwrap();
}


#[test]
fn ipc_screencast_target_and_preview_validation_roundtrip() {
    use neon_compositor::ipc::IpcServer;
    use neon_ipc::{IpcClient, IpcPayload, IpcRequest, IpcResult, ScreencastTarget};

    let (_event_loop, _display, mut state) = create_headless_server();
    let dir = tempfile::tempdir().unwrap();
    let ipc_path = dir.path().join("neon-ipc-sc-test.sock");
    let mut ipc_server = IpcServer::bind(ipc_path.clone()).unwrap();

    let done = Arc::new(AtomicBool::new(false));
    let done_c = Arc::clone(&done);
    let preview_ok_path = dir.path().join("thumb.png").to_string_lossy().into_owned();

    let client_thread = thread::spawn(move || {
        let mut client = IpcClient::connect(&ipc_path).unwrap();

        // Default target is Output { name: None }.
        assert_eq!(
            client.request(IpcRequest::GetScreencastTarget).unwrap(),
            IpcResult::Ok {
                payload: IpcPayload::ScreencastTarget {
                    target: ScreencastTarget::Output { name: None },
                }
            }
        );

        // Switch to Window target.
        let win_target = ScreencastTarget::Window { window_id: 7 };
        assert_eq!(
            client
                .request(IpcRequest::SetScreencastTarget {
                    target: win_target.clone(),
                })
                .unwrap(),
            IpcResult::Ok {
                payload: IpcPayload::ScreencastTarget { target: win_target }
            }
        );

        // Switch to Workspace target.
        let ws_target = ScreencastTarget::Workspace { index: 1 };
        assert_eq!(
            client
                .request(IpcRequest::SetScreencastTarget {
                    target: ws_target.clone(),
                })
                .unwrap(),
            IpcResult::Ok {
                payload: IpcPayload::ScreencastTarget { target: ws_target }
            }
        );

        // Valid preview request is accepted & queued; path traversal is rejected.
        assert_eq!(
            client
                .request(IpcRequest::CaptureScreencastPreview {
                    target: ScreencastTarget::Workspace { index: 1 },
                    path: preview_ok_path,
                    max_width: Some(320),
                    max_height: Some(200),
                })
                .unwrap(),
            IpcResult::Ok {
                payload: IpcPayload::Ack
            }
        );
        let bad = client
            .request(IpcRequest::CaptureScreencastPreview {
                target: ScreencastTarget::Output { name: None },
                path: "/tmp/../etc/passwd.png".into(),
                max_width: None,
                max_height: None,
            })
            .unwrap();
        assert!(matches!(bad, IpcResult::Err { .. }));

        done_c.store(true, Ordering::SeqCst);
    });

    let deadline = Instant::now() + Duration::from_secs(5);
    while !done.load(Ordering::SeqCst) && Instant::now() < deadline {
        ipc_server.poll_requests(&mut state);
        thread::sleep(Duration::from_millis(5));
    }
    client_thread.join().unwrap();

    assert_eq!(
        state.screencast_target,
        ScreencastTarget::Workspace { index: 1 }
    );
    assert_eq!(state.pending_screencast_previews.len(), 1);
}

#[test]
fn super_left_move_super_right_resize_and_maximize_respects_topbar() {
    use std::sync::Mutex;
    use neon_compositor::input::process_input_event;
    use smithay::backend::input::{
        ButtonState, Device, DeviceCapability, Event as InputEventTrait, InputBackend, InputEvent,
        KeyState, KeyboardKeyEvent, PointerButtonEvent, PointerMotionEvent, UnusedEvent,
    };

    fn pump_display(display: &mut Display<NeonState>, state: &mut NeonState) {
        display.dispatch_clients(state).unwrap();
        display.flush_clients().unwrap();
    }

    #[derive(Clone, Debug, PartialEq, Eq, Hash)]
    struct MockDev;
    impl Device for MockDev {
        fn id(&self) -> String {
            "mock-super-grab".into()
        }
        fn name(&self) -> String {
            "Mock Super Pointer".into()
        }
        fn has_capability(&self, _: DeviceCapability) -> bool {
            true
        }
        fn usb_id(&self) -> Option<(u32, u32)> {
            None
        }
        fn syspath(&self) -> Option<std::path::PathBuf> {
            None
        }
    }
    struct MockB;
    struct MockKey {
        code: u32,
        state: KeyState,
    }
    impl InputEventTrait<MockB> for MockKey {
        fn time(&self) -> u64 {
            1000
        }
        fn device(&self) -> MockDev {
            MockDev
        }
    }
    impl KeyboardKeyEvent<MockB> for MockKey {
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
    struct MockBtn {
        button: u32,
        state: ButtonState,
    }
    impl InputEventTrait<MockB> for MockBtn {
        fn time(&self) -> u64 {
            1100
        }
        fn device(&self) -> MockDev {
            MockDev
        }
    }
    impl PointerButtonEvent<MockB> for MockBtn {
        fn button_code(&self) -> u32 {
            self.button
        }
        fn state(&self) -> ButtonState {
            self.state
        }
    }
    struct MockMot {
        dx: f64,
        dy: f64,
    }
    impl InputEventTrait<MockB> for MockMot {
        fn time(&self) -> u64 {
            1200
        }
        fn device(&self) -> MockDev {
            MockDev
        }
    }
    impl PointerMotionEvent<MockB> for MockMot {
        fn delta_x(&self) -> f64 {
            self.dx
        }
        fn delta_y(&self) -> f64 {
            self.dy
        }
        fn delta_x_unaccel(&self) -> f64 {
            self.dx
        }
        fn delta_y_unaccel(&self) -> f64 {
            self.dy
        }
    }
    impl InputBackend for MockB {
        type Device = MockDev;
        type KeyboardKeyEvent = MockKey;
        type PointerAxisEvent = UnusedEvent;
        type PointerButtonEvent = MockBtn;
        type PointerMotionEvent = MockMot;
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

    let (_event_loop, mut display, mut state) = create_headless_server();
    // Simulate a live 31px top bar with smart-hide OFF.
    state.last_panel_height = 31;
    state.config.panel.visible = true;
    state.config.panel.smart_hide = false;

    let (server_sock, client_sock) = UnixStream::pair().unwrap();
    state.insert_client(server_sock);

    let mapped = Arc::new(AtomicBool::new(false));
    let mapped_c = Arc::clone(&mapped);
    let allow_exit = Arc::new(AtomicBool::new(false));
    let allow_exit_c = Arc::clone(&allow_exit);
    let last_size = Arc::new(Mutex::new((0i32, 0i32)));
    let last_size_c = Arc::clone(&last_size);

    let client_handle = thread::spawn(move || {
        let conn = Connection::from_socket(client_sock).unwrap();
        let mut queue = conn.new_event_queue::<TestClientState>();
        let qh = queue.handle();
        let mut cstate = TestClientState::default();

        let _registry = conn.display().get_registry(&qh, ());
        queue.roundtrip(&mut cstate).unwrap();

        let compositor = cstate.compositor.clone().unwrap();
        let shm = cstate.shm.clone().unwrap();
        let wm_base = cstate.wm_base.clone().unwrap();

        let surface = compositor.create_surface(&qh, ());
        let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
        let toplevel = xdg_surface.get_toplevel(&qh, ());
        toplevel.set_title("Resizable App".into());
        surface.commit();
        queue.roundtrip(&mut cstate).unwrap();

        let (_f, pool, buffer) = create_shm_buffer(&shm, &qh, 400, 300);
        surface.attach(Some(&buffer), 0, 0);
        surface.commit();
        queue.roundtrip(&mut cstate).unwrap();

        mapped_c.store(true, Ordering::SeqCst);
        let deadline = Instant::now() + Duration::from_secs(5);
        while !allow_exit_c.load(Ordering::SeqCst) && Instant::now() < deadline {
            queue.roundtrip(&mut cstate).unwrap();
            *last_size_c.lock().unwrap() = cstate.last_configure_size;
            thread::sleep(Duration::from_millis(10));
        }

        toplevel.destroy();
        xdg_surface.destroy();
        buffer.destroy();
        pool.destroy();
        surface.destroy();
        conn.flush().unwrap();
    });

    let deadline = Instant::now() + Duration::from_secs(5);
    while !mapped.load(Ordering::SeqCst) && Instant::now() < deadline {
        pump_display(&mut display, &mut state);
        thread::sleep(Duration::from_millis(5));
    }
    pump_display(&mut display, &mut state);

    let wid = state.wm.all_window_ids()[0];

    // 1. Maximize with panel smart-hide OFF must NOT go above the 31px top bar.
    state.toggle_maximize_window(wid);
    pump_display(&mut display, &mut state);
    let max_rec = state.wm.get(wid).unwrap().clone();
    assert!(max_rec.mode.is_maximized());
    assert_eq!(max_rec.current_geometry.loc.y, 31);
    assert_eq!(max_rec.current_geometry.size.h, 800 - 31);

    // 2. Enabling panel smart-hide expands maximized window cleanly to y = 0; disabling restores y = 31.
    state.config.panel.smart_hide = true;
    state.relayout_non_floating_windows();
    assert_eq!(state.wm.get(wid).unwrap().current_geometry.loc.y, 0);
    state.config.panel.smart_hide = false;
    state.relayout_non_floating_windows();
    assert_eq!(state.wm.get(wid).unwrap().current_geometry.loc.y, 31);

    // Unmaximize back to floating at (200, 150) for Super+Left move and Super+Right resize tests.
    state.toggle_maximize_window(wid);
    let win = state.find_window_by_id(wid).unwrap();
    state.space.map_element(win.clone(), (200, 150), true);
    state
        .wm
        .set_floating_location(wid, neon_window_manager::geometry::Point::new(200, 150));
    pump_display(&mut display, &mut state);

    // Position pointer inside the bottom-right quadrant of the 400x300 window at (500, 380).
    state.pointer_location = (500.0, 380.0).into();

    // Hold Super_L (XKB keycode 133 = evdev 125 + 8).
    process_input_event::<MockB>(
        &mut state,
        InputEvent::Keyboard {
            event: MockKey {
                code: 133,
                state: KeyState::Pressed,
            },
        },
    );

    // 3. Super + Left-Click + Drag moves the window and clamps top edge >= 31px.
    process_input_event::<MockB>(
        &mut state,
        InputEvent::PointerButton {
            event: MockBtn {
                button: 0x110,
                state: ButtonState::Pressed,
            },
        },
    );
    process_input_event::<MockB>(
        &mut state,
        InputEvent::PointerMotion {
            event: MockMot {
                dx: 80.0,
                dy: -250.0, // tries to push y = 150 - 250 = -100 above top bar
            },
        },
    );
    process_input_event::<MockB>(
        &mut state,
        InputEvent::PointerButton {
            event: MockBtn {
                button: 0x110,
                state: ButtonState::Released,
            },
        },
    );
    let moved_loc = state.space.element_location(&win).unwrap();
    assert_eq!(moved_loc.x, 280);
    assert_eq!(moved_loc.y, 31, "Super+Left drag must stay below top bar when autohide is off");

    // 4. Super + Right-Click + Drag resizes the window cleanly from bottom-right quadrant.
    state.pointer_location = (moved_loc.x as f64 + 320.0, moved_loc.y as f64 + 240.0).into();
    process_input_event::<MockB>(
        &mut state,
        InputEvent::PointerButton {
            event: MockBtn {
                button: 0x111,
                state: ButtonState::Pressed,
            },
        },
    );
    process_input_event::<MockB>(
        &mut state,
        InputEvent::PointerMotion {
            event: MockMot {
                dx: 120.0,
                dy: 90.0,
            },
        },
    );
    process_input_event::<MockB>(
        &mut state,
        InputEvent::PointerButton {
            event: MockBtn {
                button: 0x111,
                state: ButtonState::Released,
            },
        },
    );
    for _ in 0..8 {
        pump_display(&mut display, &mut state);
        thread::sleep(Duration::from_millis(10));
    }
    let configured = *last_size.lock().unwrap();
    assert_eq!(
        configured,
        (520, 390),
        "Super+Right drag must resize window from 400x300 by (+120,+90)"
    );

    allow_exit.store(true, Ordering::SeqCst);
    let end_deadline = Instant::now() + Duration::from_secs(2);
    while !client_handle.is_finished() && Instant::now() < end_deadline {
        pump_display(&mut display, &mut state);
        thread::sleep(Duration::from_millis(5));
    }
    client_handle.join().unwrap();
}

#[test]
fn window_buttons_double_click_maximize_and_dock_minimize_close_animations() {
    use neon_compositor::{input::process_input_event, state::TitlebarHit};
    use neon_config::WindowButtonLayout;
    use smithay::{
        backend::input::{
            ButtonState, Device, DeviceCapability, Event as InputEventTrait, InputBackend,
            InputEvent, KeyState, KeyboardKeyEvent, PointerButtonEvent, PointerMotionEvent,
            UnusedEvent,
        },
        utils::SERIAL_COUNTER,
    };

    #[derive(Clone, Debug, PartialEq, Eq, Hash)]
    struct Dev;
    impl Device for Dev {
        fn id(&self) -> String {
            "mock".into()
        }
        fn name(&self) -> String {
            "mock".into()
        }
        fn has_capability(&self, cap: DeviceCapability) -> bool {
            matches!(cap, DeviceCapability::Pointer | DeviceCapability::Keyboard)
        }
        fn usb_id(&self) -> Option<(u32, u32)> {
            None
        }
        fn syspath(&self) -> Option<std::path::PathBuf> {
            None
        }
    }
    struct B;
    struct KeyEv {
        code: u32,
        state: KeyState,
    }
    impl InputEventTrait<B> for KeyEv {
        fn time(&self) -> u64 {
            1000
        }
        fn device(&self) -> Dev {
            Dev
        }
    }
    impl KeyboardKeyEvent<B> for KeyEv {
        fn key_code(&self) -> smithay::input::keyboard::Keycode {
            smithay::input::keyboard::Keycode::new(self.code)
        }
        fn state(&self) -> KeyState {
            self.state
        }
        fn count(&self) -> u32 {
            1
        }
    }
    struct BtnEv {
        button: u32,
        state: ButtonState,
    }
    impl InputEventTrait<B> for BtnEv {
        fn time(&self) -> u64 {
            1100
        }
        fn device(&self) -> Dev {
            Dev
        }
    }
    impl PointerButtonEvent<B> for BtnEv {
        fn button_code(&self) -> u32 {
            self.button
        }
        fn state(&self) -> ButtonState {
            self.state
        }
    }
    struct MotEv {
        dx: f64,
        dy: f64,
    }
    impl InputEventTrait<B> for MotEv {
        fn time(&self) -> u64 {
            1200
        }
        fn device(&self) -> Dev {
            Dev
        }
    }
    impl PointerMotionEvent<B> for MotEv {
        fn delta_x(&self) -> f64 {
            self.dx
        }
        fn delta_y(&self) -> f64 {
            self.dy
        }
        fn delta_x_unaccel(&self) -> f64 {
            self.dx
        }
        fn delta_y_unaccel(&self) -> f64 {
            self.dy
        }
    }
    impl InputBackend for B {
        type Device = Dev;
        type KeyboardKeyEvent = KeyEv;
        type PointerAxisEvent = UnusedEvent;
        type PointerButtonEvent = BtnEv;
        type PointerMotionEvent = MotEv;
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

    let (_event_loop, mut display, mut state) = create_headless_server();
    // Simulate interactive backend so minimize/close animations & closing ghosts engage.
    state.backend_name = "winit".into();
    state.last_panel_height = 31;
    let pump_display = |display: &mut smithay::reexports::wayland_server::Display<NeonState>,
                        state: &mut NeonState| {
        let _ = display.dispatch_clients(state);
        let _ = display.flush_clients();
    };

    let (server_sock, client_sock) = UnixStream::pair().unwrap();
    state.insert_client(server_sock);

    let mapped = Arc::new(AtomicBool::new(false));
    let mapped_c = Arc::clone(&mapped);
    let commit_resized = Arc::new(AtomicBool::new(false));
    let commit_resized_c = Arc::clone(&commit_resized);
    let allow_exit = Arc::new(AtomicBool::new(false));
    let allow_exit_c = Arc::clone(&allow_exit);

    let client_handle = thread::spawn(move || {
        let conn = Connection::from_socket(client_sock).unwrap();
        let mut queue = conn.new_event_queue::<TestClientState>();
        let qh = queue.handle();
        let mut cstate = TestClientState::default();

        let _registry = conn.display().get_registry(&qh, ());
        queue.roundtrip(&mut cstate).unwrap();

        let compositor = cstate.compositor.clone().unwrap();
        let shm = cstate.shm.clone().unwrap();
        let wm_base = cstate.wm_base.clone().unwrap();

        let surface = compositor.create_surface(&qh, ());
        let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
        let toplevel = xdg_surface.get_toplevel(&qh, ());
        toplevel.set_title("Animated App".into());
        surface.commit();
        queue.roundtrip(&mut cstate).unwrap();

        let (_f1, pool1, buf1) = create_shm_buffer(&shm, &qh, 400, 300);
        surface.attach(Some(&buf1), 0, 0);
        surface.commit();
        queue.roundtrip(&mut cstate).unwrap();

        mapped_c.store(true, Ordering::SeqCst);
        let mut resized_holder = None;
        let deadline = Instant::now() + Duration::from_secs(5);
        while !allow_exit_c.load(Ordering::SeqCst) && Instant::now() < deadline {
            queue.roundtrip(&mut cstate).unwrap();
            if commit_resized_c.swap(false, Ordering::SeqCst) {
                let (w, h) = cstate.last_configure_size;
                let (rw, rh) = (w.max(600), h.max(400));
                let (f2, pool2, buf2) = create_shm_buffer(&shm, &qh, rw, rh);
                surface.attach(Some(&buf2), 0, 0);
                surface.commit();
                queue.roundtrip(&mut cstate).unwrap();
                resized_holder = Some((f2, pool2, buf2));
            }
            thread::sleep(Duration::from_millis(8));
        }
        drop(resized_holder);

        toplevel.destroy();
        xdg_surface.destroy();
        buf1.destroy();
        pool1.destroy();
        surface.destroy();
        conn.flush().unwrap();
    });

    let deadline = Instant::now() + Duration::from_secs(5);
    while !mapped.load(Ordering::SeqCst) && Instant::now() < deadline {
        pump_display(&mut display, &mut state);
        thread::sleep(Duration::from_millis(5));
    }
    pump_display(&mut display, &mut state);

    let wid = state.wm.all_window_ids()[0];
    let win = state.find_window_by_id(wid).unwrap();
    state.enable_ssd_for_window(wid);
    state.space.map_element(win.clone(), (200, 150), true);
    state
        .wm
        .set_floating_location(wid, neon_window_manager::geometry::Point::new(200, 150));

    // 1. Left vs Right SSD titlebar button layout + hit-testing.
    state.config.windows.button_layout = WindowButtonLayout::LeftTrafficLights;
    state.sync_scene_graph();
    let tb_y = 150.0 - neon_scene::MACOS_TITLEBAR_HEIGHT as f64 * 0.5;
    assert_eq!(
        state.ssd_titlebar_hit_test((216.0, tb_y).into()).map(|x| x.2),
        Some(TitlebarHit::Close)
    );
    assert_eq!(
        state.ssd_titlebar_hit_test((236.0, tb_y).into()).map(|x| x.2),
        Some(TitlebarHit::Minimize)
    );
    assert_eq!(
        state.ssd_titlebar_hit_test((256.0, tb_y).into()).map(|x| x.2),
        Some(TitlebarHit::Maximize)
    );
    let left_node = state
        .scene
        .collect_front_to_back()
        .into_iter()
        .find(|n| matches!(n.content, neon_scene::NodeContent::Window { window_id, .. } if window_id == wid))
        .unwrap();
    assert!(!left_node.titlebar.unwrap().buttons_right);

    state.config.windows.button_layout = WindowButtonLayout::RightStandard;
    state.sync_scene_graph();
    assert_eq!(
        state.ssd_titlebar_hit_test((584.0, tb_y).into()).map(|x| x.2),
        Some(TitlebarHit::Close)
    );
    assert_eq!(
        state.ssd_titlebar_hit_test((564.0, tb_y).into()).map(|x| x.2),
        Some(TitlebarHit::Maximize)
    );
    assert_eq!(
        state.ssd_titlebar_hit_test((544.0, tb_y).into()).map(|x| x.2),
        Some(TitlebarHit::Minimize)
    );
    let right_node = state
        .scene
        .collect_front_to_back()
        .into_iter()
        .find(|n| matches!(n.content, neon_scene::NodeContent::Window { window_id, .. } if window_id == wid))
        .unwrap();
    assert!(right_node.titlebar.unwrap().buttons_right);

    // 2. Double-clicking SSD titlebar drag strip maximizes AND triggers the exact same
    //    Material 3 Expressive geometry morph animation once the resized buffer commits!
    state.pointer_location = (400.0, tb_y).into();
    for _ in 0..2 {
        process_input_event::<B>(
            &mut state,
            InputEvent::PointerButton {
                event: BtnEv {
                    button: 0x110,
                    state: ButtonState::Pressed,
                },
            },
        );
        process_input_event::<B>(
            &mut state,
            InputEvent::PointerButton {
                event: BtnEv {
                    button: 0x110,
                    state: ButtonState::Released,
                },
            },
        );
    }
    assert!(
        state.wm.get(wid).unwrap().mode.is_maximized(),
        "Double-clicking SSD titlebar must maximize the window"
    );
    assert!(
        state.pending_geometry_anims.contains_key(&wid),
        "Double-click maximize must register a pending geometry morph"
    );
    commit_resized.store(true, Ordering::SeqCst);
    for _ in 0..15 {
        pump_display(&mut display, &mut state);
        if state.find_window_by_id(wid).unwrap().geometry().size.w > 400 {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    let animating_after_dbl = state.sync_scene_graph();
    let now_after_dbl = state.now_duration();
    assert!(
        animating_after_dbl && state.effects.has_active_animations(now_after_dbl),
        "Double-click maximize must launch active geometry morph animation"
    );

    // 3. Minimize targets exact Dock icon anchor and elevates Z-index above other windows.
    state.window_dock_anchors.insert(wid, (900, 770).into());
    state.minimize_window(wid, SERIAL_COUNTER.next_serial());
    assert!(state.minimizing_windows.contains_key(&wid));
    assert!(state.sync_scene_graph());
    let min_node = state
        .scene
        .collect_front_to_back()
        .into_iter()
        .find(|n| matches!(n.content, neon_scene::NodeContent::Window { window_id, .. } if window_id == wid))
        .unwrap();
    assert!(
        min_node.z_index >= 10_000,
        "Minimizing window must stay elevated above remaining windows during dock swoop"
    );

    // Restore from Dock cancels minimizing state and launches rise animation.
    assert!(state.restore_minimized_window(wid, SERIAL_COUNTER.next_serial()));
    assert!(!state.minimizing_windows.contains_key(&wid));
    assert!(state.sync_scene_graph());

    // 4. Client-initiated close (destroying xdg_toplevel directly) queues a GPU PendingCloseGhost.
    allow_exit.store(true, Ordering::SeqCst);
    let end_deadline = Instant::now() + Duration::from_secs(2);
    while !client_handle.is_finished() && Instant::now() < end_deadline {
        pump_display(&mut display, &mut state);
        thread::sleep(Duration::from_millis(5));
    }
    pump_display(&mut display, &mut state);
    assert!(
        !state.pending_close_ghosts.is_empty(),
        "Client-initiated window close must queue a GPU closing ghost animation"
    );
    client_handle.join().unwrap();
}
