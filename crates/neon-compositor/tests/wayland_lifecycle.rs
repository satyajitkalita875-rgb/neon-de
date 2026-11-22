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
        .expect("keyboard focus must point to Window Two");
    let win2 = state.find_window_by_id(remaining_focus).unwrap();
    assert_eq!(kbd_focus.id(), win2.toplevel().unwrap().wl_surface().id());

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

    // Switching to empty workspace 1 removes window node from SceneGraph and triggers slide animation.
    state
        .switch_workspace_next(SERIAL_COUNTER.next_serial())
        .expect("switch to dynamic workspace 1");
    let slide_active = state.sync_scene_graph();
    assert!(
        slide_active,
        "workspace transition animation should be active"
    );
    assert_eq!(
        state.scene.collect_front_to_back().len(),
        1,
        "only backdrop remains visible on empty workspace 1"
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
            .request(IpcRequest::UpdateAppearance {
                theme: Some("neon_light".into()),
                accent_color: Some("#A855F7".into()),
                dock_transparency: Some(0.9),
                panel_transparency: Some(0.92),
                panel_visible: Some(true),
                show_clock: Some(true),
                show_workspaces: Some(true),
                high_contrast: Some(true),
                wallpaper: None,
            })
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
