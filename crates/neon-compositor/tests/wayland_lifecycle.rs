//! End-to-end Wayland protocol integration tests for `neon-compositor`.
//!
//! Spins up a real `NeonState` + `wayland-server::Display` on an isolated Unix socket pair
//! and drives real `wayland-client` connections exercising `wl_compositor`, `wl_shm`,
//! `xdg_wm_base`, `xdg_toplevel`, `wl_seat` (keyboard & pointer), and window lifecycle.

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

use neon_compositor::state::NeonState;
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
        if let xdg_toplevel::Event::Close = event {
            state.received_close = true;
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
        // Second roundtrip to receive wl_seat capabilities & keyboard keymap
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

        // Wait until server injects a key event and sends close request
        let deadline = Instant::now() + Duration::from_secs(5);
        while !allow_exit_clone.load(Ordering::SeqCst) && Instant::now() < deadline {
            queue.roundtrip(&mut cstate).unwrap();
            thread::sleep(Duration::from_millis(10));
        }
        queue.roundtrip(&mut cstate).unwrap();

        // Destroy window cleanly
        toplevel.destroy();
        xdg_surface.destroy();
        buffer.destroy();
        pool.destroy();
        surface.destroy();
        conn.flush().unwrap();

        cstate
    });

    // Pump server until client maps its buffer
    let deadline = Instant::now() + Duration::from_secs(5);
    while !client_mapped.load(Ordering::SeqCst) && Instant::now() < deadline {
        pump_server(&mut display, &mut state);
        thread::sleep(Duration::from_millis(5));
    }
    pump_server(&mut display, &mut state);

    assert_eq!(state.windows.len(), 1, "one window should be registered");
    let focused_id = state
        .windows
        .focused_id()
        .expect("newly mapped window must be focused");
    let record = state.windows.get(focused_id).unwrap();
    assert!(
        record.mapped,
        "window must be marked mapped after SHM commit"
    );
    assert_eq!(record.title.as_deref(), Some("NEON DE Test Terminal"));
    assert_eq!(record.app_id.as_deref(), Some("org.neonde.test-terminal"));
    assert_eq!(record.size, (320, 200).into());

    // Deliver a real pointer enter + left click inside the window bounds
    let click_pos: Point<f64, Logical> = (
        f64::from(record.location.x) + 20.0,
        f64::from(record.location.y) + 20.0,
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

    // Deliver a real keyboard press (KEY_A -> evdev 30, XKB keycode 38) and close request
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
    // Pump after client disconnect to confirm cleanup
    for _ in 0..5 {
        pump_server(&mut display, &mut state);
        thread::sleep(Duration::from_millis(5));
    }

    assert!(cstate.received_keymap, "client must receive XKB keymap");
    assert!(
        cstate.received_keyboard_enter,
        "focused client must receive wl_keyboard.enter"
    );
    assert!(
        cstate.received_keys.contains(&30),
        "client must receive forwarded key event (evdev 30)"
    );
    assert!(
        cstate.received_pointer_enter,
        "client must receive wl_pointer.enter"
    );
    assert!(
        cstate.received_pointer_buttons.contains(&0x110),
        "client must receive forwarded left pointer button (0x110)"
    );
    assert!(
        cstate.received_close,
        "client must receive xdg_toplevel.close"
    );
    assert!(
        state.windows.is_empty(),
        "window registry must be empty after destruction"
    );
    assert_eq!(
        state.space.elements().count(),
        0,
        "desktop space must have no mapped windows after destruction"
    );
    assert_eq!(state.windows.focused_id(), None);
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

        // Window 1
        let s1 = compositor.create_surface(&qh, ());
        let xs1 = wm_base.get_xdg_surface(&s1, &qh, ());
        let t1 = xs1.get_toplevel(&qh, ());
        t1.set_title("Window One".into());
        s1.commit();
        queue.roundtrip(&mut cstate).unwrap();
        let (_f1, p1, b1) = create_shm_buffer(&shm, &qh, 400, 300);
        s1.attach(Some(&b1), 0, 0);
        s1.commit();

        // Window 2
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

        // Destroy Window 1 while Window 2 remains alive
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

    assert_eq!(state.windows.len(), 2);

    // Collect both windows and verify distinct cascaded positions
    let ids: Vec<_> = state
        .space
        .elements()
        .filter_map(neon_compositor::window::window_id)
        .collect();
    assert_eq!(ids.len(), 2);
    let loc0 = state.windows.get(ids[0]).unwrap().location;
    let loc1 = state.windows.get(ids[1]).unwrap().location;
    assert_ne!(
        loc0, loc1,
        "floating windows must cascade to distinct locations"
    );

    // Cycle focus from Window 2 -> Window 1
    let before_cycle = state.windows.focused_id().unwrap();
    state.cycle_focus(SERIAL_COUNTER.next_serial());
    let after_cycle = state.windows.focused_id().unwrap();
    assert_ne!(before_cycle, after_cycle, "Alt+Tab cycle must switch focus");
    assert_eq!(
        state.windows.get(after_cycle).unwrap().title.as_deref(),
        Some("Window One")
    );

    // Close focused Window 1 -> focus must automatically return to Window 2
    allow_close_first.store(true, Ordering::SeqCst);
    while !first_closed.load(Ordering::SeqCst) && Instant::now() < deadline {
        pump_server(&mut display, &mut state);
        thread::sleep(Duration::from_millis(5));
    }
    pump_server(&mut display, &mut state);

    assert_eq!(state.windows.len(), 1);
    let remaining_focus = state
        .windows
        .focused_id()
        .expect("Window Two should gain focus");
    assert_eq!(
        state.windows.get(remaining_focus).unwrap().title.as_deref(),
        Some("Window Two")
    );

    // Verify focused surface matches Window Two's WlSurface
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
    assert!(state.windows.is_empty());
}
