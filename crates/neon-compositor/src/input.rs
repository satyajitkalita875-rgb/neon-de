//! Input routing, compositor shortcuts, focus policies, and interactive move/resize grabs.

use smithay::{
    backend::input::{
        AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, GestureBeginEvent,
        GestureEndEvent, GesturePinchUpdateEvent, GestureSwipeUpdateEvent, InputBackend,
        InputEvent, KeyState, KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent,
        PointerMotionEvent,
    },
    input::{
        keyboard::{FilterResult, Keysym, keysyms},
        pointer::{
            AxisFrame, ButtonEvent, Focus, GestureHoldBeginEvent, GestureHoldEndEvent,
            GesturePinchBeginEvent, GesturePinchEndEvent,
            GesturePinchUpdateEvent as PointerGesturePinchUpdateEvent, GestureSwipeBeginEvent,
            GestureSwipeEndEvent, GestureSwipeUpdateEvent as PointerGestureSwipeUpdateEvent,
            GrabStartData as PointerGrabStartData, MotionEvent,
        },
    },
    reexports::wayland_protocols::xdg::shell::server::xdg_toplevel,
    utils::{Rectangle, SERIAL_COUNTER},
    wayland::shell::wlr_layer::{KeyboardInteractivity, Layer as WlrLayer},
};
use tracing::info;

use crate::{
    grabs::{MoveSurfaceGrab, ResizeEdge, ResizeSurfaceGrab},
    state::NeonState,
    window::{FocusPolicy, SnapRegion, window_id},
};

const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;

enum CompositorAction {
    Quit,
    SwitchVt(i32),
    SpawnTerminal,
    ToggleOverview,
    LockSession,
    CloseFocused,
    CycleFocus,
    ToggleMaximize,
    UnmaximizeOrMinimize,
    ToggleFullscreen,
    MinimizeFocused,
    RestoreLastMinimized,
    Snap(SnapRegion),
    SwitchWorkspaceNext,
    SwitchWorkspacePrev,
    SwitchWorkspaceIndex(usize),
    MoveFocusedToNextWorkspace,
    MoveFocusedToPrevWorkspace,
}

/// Processes a backend-agnostic Smithay [`InputEvent`] and routes it through NEON DE's seat.
pub fn process_input_event<B: InputBackend>(state: &mut NeonState, event: InputEvent<B>) {
    match event {
        InputEvent::Keyboard { event } => handle_keyboard::<B>(state, event),
        InputEvent::PointerMotion { event } => handle_pointer_motion::<B>(state, event),
        InputEvent::PointerMotionAbsolute { event } => {
            handle_pointer_motion_absolute::<B>(state, event)
        }
        InputEvent::PointerButton { event } => handle_pointer_button::<B>(state, event),
        InputEvent::PointerAxis { event } => handle_pointer_axis::<B>(state, event),
        InputEvent::TouchDown { event } => {
            handle_touch_pointer::<B, _>(state, &event, Some(ButtonState::Pressed))
        }
        InputEvent::TouchMotion { event } => handle_touch_pointer::<B, _>(state, &event, None),
        InputEvent::TouchUp { event } => {
            if let Some(pointer) = state.seat.get_pointer() {
                let serial = SERIAL_COUNTER.next_serial();
                pointer.button(
                    state,
                    &ButtonEvent {
                        button: BTN_LEFT,
                        state: ButtonState::Released,
                        serial,
                        time: event.time_msec(),
                    },
                );
                pointer.frame(state);
            }
        }
        InputEvent::GestureSwipeBegin { event } => handle_gesture_swipe_begin::<B>(state, event),
        InputEvent::GestureSwipeUpdate { event } => handle_gesture_swipe_update::<B>(state, event),
        InputEvent::GestureSwipeEnd { event } => handle_gesture_swipe_end::<B>(state, event),
        InputEvent::GesturePinchBegin { event } => handle_gesture_pinch_begin::<B>(state, event),
        InputEvent::GesturePinchUpdate { event } => handle_gesture_pinch_update::<B>(state, event),
        InputEvent::GesturePinchEnd { event } => handle_gesture_pinch_end::<B>(state, event),
        InputEvent::GestureHoldBegin { event } => handle_gesture_hold_begin::<B>(state, event),
        InputEvent::GestureHoldEnd { event } => handle_gesture_hold_end::<B>(state, event),
        _ => {}
    }
}

fn handle_keyboard<B: InputBackend>(state: &mut NeonState, event: B::KeyboardKeyEvent) {
    let serial = SERIAL_COUNTER.next_serial();
    let time = Event::time_msec(&event);
    let keycode = event.key_code();
    let key_state = event.state();

    let Some(keyboard) = state.seat.get_keyboard() else {
        return;
    };

    let is_locked = state.is_locked();
    let action = keyboard.input::<CompositorAction, _>(
        state,
        keycode,
        key_state,
        serial,
        time,
        |_state, modifiers, handle| {
            let sym = handle.modified_sym();

            // Bare-metal TTY VT switching (Ctrl+Alt+F1..F12): always honored, even while locked.
            if key_state == KeyState::Pressed
                && (keysyms::KEY_XF86Switch_VT_1..=keysyms::KEY_XF86Switch_VT_12)
                    .contains(&sym.raw())
            {
                let vt = (sym.raw() - keysyms::KEY_XF86Switch_VT_1 + 1) as i32;
                return FilterResult::Intercept(CompositorAction::SwitchVt(vt));
            }

            // Security invariant: while locked via ext_session_lock_v1, forward all keys strictly
            // to the lock screen surface and suppress compositor shortcuts.
            if is_locked {
                return FilterResult::Forward;
            }
            if key_state != KeyState::Pressed {
                return FilterResult::Forward;
            }

            // Emergency / nested exit
            if modifiers.ctrl && modifiers.alt && sym == Keysym::from(keysyms::KEY_Escape) {
                return FilterResult::Intercept(CompositorAction::Quit);
            }

            // Standard Linux terminal shortcut: Ctrl+Alt+T
            if modifiers.ctrl
                && modifiers.alt
                && (sym == Keysym::from(keysyms::KEY_t) || sym == Keysym::from(keysyms::KEY_T))
            {
                return FilterResult::Intercept(CompositorAction::SpawnTerminal);
            }

            // Dynamic workspace navigation: Ctrl+Alt+Left/Right or Super+Page_Up/Page_Down
            if modifiers.ctrl && modifiers.alt && !modifiers.shift {
                if sym == Keysym::from(keysyms::KEY_Right) {
                    return FilterResult::Intercept(CompositorAction::SwitchWorkspaceNext);
                }
                if sym == Keysym::from(keysyms::KEY_Left) {
                    return FilterResult::Intercept(CompositorAction::SwitchWorkspacePrev);
                }
            }
            if modifiers.ctrl && modifiers.alt && modifiers.shift {
                if sym == Keysym::from(keysyms::KEY_Right) {
                    return FilterResult::Intercept(CompositorAction::MoveFocusedToNextWorkspace);
                }
                if sym == Keysym::from(keysyms::KEY_Left) {
                    return FilterResult::Intercept(CompositorAction::MoveFocusedToPrevWorkspace);
                }
            }

            if modifiers.logo {
                if modifiers.shift {
                    if sym == Keysym::from(keysyms::KEY_Page_Down) {
                        return FilterResult::Intercept(
                            CompositorAction::MoveFocusedToNextWorkspace,
                        );
                    }
                    if sym == Keysym::from(keysyms::KEY_Page_Up) {
                        return FilterResult::Intercept(
                            CompositorAction::MoveFocusedToPrevWorkspace,
                        );
                    }
                    if sym == Keysym::from(keysyms::KEY_H) || sym == Keysym::from(keysyms::KEY_h) {
                        return FilterResult::Intercept(CompositorAction::RestoreLastMinimized);
                    }
                } else {
                    if sym == Keysym::from(keysyms::KEY_Return) {
                        return FilterResult::Intercept(CompositorAction::SpawnTerminal);
                    }
                    if sym == Keysym::from(keysyms::KEY_space)
                        || sym == Keysym::from(keysyms::KEY_a)
                        || sym == Keysym::from(keysyms::KEY_A)
                    {
                        return FilterResult::Intercept(CompositorAction::ToggleOverview);
                    }
                    if sym == Keysym::from(keysyms::KEY_l) || sym == Keysym::from(keysyms::KEY_L) {
                        return FilterResult::Intercept(CompositorAction::LockSession);
                    }
                    if sym == Keysym::from(keysyms::KEY_q) || sym == Keysym::from(keysyms::KEY_Q) {
                        return FilterResult::Intercept(CompositorAction::CloseFocused);
                    }
                    if sym == Keysym::from(keysyms::KEY_Page_Down) {
                        return FilterResult::Intercept(CompositorAction::SwitchWorkspaceNext);
                    }
                    if sym == Keysym::from(keysyms::KEY_Page_Up) {
                        return FilterResult::Intercept(CompositorAction::SwitchWorkspacePrev);
                    }
                    if sym == Keysym::from(keysyms::KEY_Left) {
                        return FilterResult::Intercept(CompositorAction::Snap(
                            SnapRegion::LeftHalf,
                        ));
                    }
                    if sym == Keysym::from(keysyms::KEY_Right) {
                        return FilterResult::Intercept(CompositorAction::Snap(
                            SnapRegion::RightHalf,
                        ));
                    }
                    if sym == Keysym::from(keysyms::KEY_Up) {
                        return FilterResult::Intercept(CompositorAction::ToggleMaximize);
                    }
                    if sym == Keysym::from(keysyms::KEY_Down) {
                        return FilterResult::Intercept(CompositorAction::UnmaximizeOrMinimize);
                    }
                    if sym == Keysym::from(keysyms::KEY_f)
                        || sym == Keysym::from(keysyms::KEY_F)
                        || sym == Keysym::from(keysyms::KEY_F11)
                    {
                        return FilterResult::Intercept(CompositorAction::ToggleFullscreen);
                    }
                    if sym == Keysym::from(keysyms::KEY_h) || sym == Keysym::from(keysyms::KEY_H) {
                        return FilterResult::Intercept(CompositorAction::MinimizeFocused);
                    }
                    if let Some(ws_idx) = digit_keysym_to_workspace_index(sym) {
                        return FilterResult::Intercept(CompositorAction::SwitchWorkspaceIndex(
                            ws_idx,
                        ));
                    }
                }
            }

            if modifiers.alt
                && (sym == Keysym::from(keysyms::KEY_q) || sym == Keysym::from(keysyms::KEY_Q))
            {
                return FilterResult::Intercept(CompositorAction::CloseFocused);
            }

            if modifiers.alt && sym == Keysym::from(keysyms::KEY_F1) {
                return FilterResult::Intercept(CompositorAction::ToggleOverview);
            }

            if modifiers.alt && sym == Keysym::from(keysyms::KEY_Tab) {
                return FilterResult::Intercept(CompositorAction::CycleFocus);
            }

            FilterResult::Forward
        },
    );

    if let Some(action) = action {
        match action {
            CompositorAction::Quit => {
                info!("Ctrl+Alt+Escape pressed; exiting NEON DE session");
                state.loop_signal.stop();
            }
            CompositorAction::SwitchVt(vt) => state.switch_vt(vt),
            CompositorAction::SpawnTerminal => state.spawn_terminal(),
            CompositorAction::ToggleOverview => {
                state.toggle_overview(serial);
            }
            CompositorAction::LockSession => {
                state.lock_session();
            }
            CompositorAction::CloseFocused => state.close_focused_window(),
            CompositorAction::CycleFocus => state.cycle_focus(serial),
            CompositorAction::ToggleMaximize => state.toggle_maximize_focused(),
            CompositorAction::UnmaximizeOrMinimize => state.unmaximize_or_minimize_focused(serial),
            CompositorAction::ToggleFullscreen => state.toggle_fullscreen_focused(),
            CompositorAction::MinimizeFocused => state.minimize_focused(serial),
            CompositorAction::RestoreLastMinimized => state.restore_last_minimized(serial),
            CompositorAction::Snap(region) => state.snap_focused(region),
            CompositorAction::SwitchWorkspaceNext => {
                state.switch_workspace_next(serial);
            }
            CompositorAction::SwitchWorkspacePrev => {
                state.switch_workspace_prev(serial);
            }
            CompositorAction::SwitchWorkspaceIndex(index) => {
                state.switch_workspace_index(index, serial);
            }
            CompositorAction::MoveFocusedToNextWorkspace => {
                state.move_focused_to_next_workspace(true, serial);
            }
            CompositorAction::MoveFocusedToPrevWorkspace => {
                state.move_focused_to_prev_workspace(true, serial);
            }
        }
    }
}

fn digit_keysym_to_workspace_index(sym: Keysym) -> Option<usize> {
    let raw = sym.raw();
    if (keysyms::KEY_1..=keysyms::KEY_9).contains(&raw) {
        Some((raw - keysyms::KEY_1) as usize)
    } else {
        None
    }
}

fn apply_pointer_motion_at(
    state: &mut NeonState,
    pos: smithay::utils::Point<f64, smithay::utils::Logical>,
    time: u32,
) {
    state.pointer_location = pos;

    let serial = SERIAL_COUNTER.next_serial();
    let under = if state.is_locked() {
        state
            .lock_state
            .primary_surface()
            .cloned()
            .map(|s| (s, (0.0, 0.0).into()))
    } else {
        state.surface_under(pos)
    };

    // Optional FocusFollowsMouse policy (ClickToFocus remains default; disabled while locked).
    if !state.is_locked()
        && state.wm.focus_policy() == FocusPolicy::FocusFollowsMouse
        && let Some((window, _)) = state.window_under(pos)
        && window_id(&window) != state.wm.focused_id()
    {
        state.focus_window_with_raise(Some(&window), serial, false);
    }

    if let Some(pointer) = state.seat.get_pointer() {
        pointer.motion(
            state,
            under,
            &MotionEvent {
                location: pos,
                serial,
                time,
            },
        );
        pointer.frame(state);
    }
}

fn handle_pointer_motion<B: InputBackend>(state: &mut NeonState, event: B::PointerMotionEvent) {
    let new_pos = state.clamp_pointer_location(state.pointer_location + event.delta());
    apply_pointer_motion_at(state, new_pos, event.time_msec());
}

fn handle_pointer_motion_absolute<B: InputBackend>(
    state: &mut NeonState,
    event: B::PointerMotionAbsoluteEvent,
) {
    let output_size = state.output_logical_size();
    let output_geo = Rectangle::from_size(output_size);
    let pos = event.position_transformed(output_geo.size) + output_geo.loc.to_f64();
    apply_pointer_motion_at(state, pos, event.time_msec());
}

fn handle_touch_pointer<B: InputBackend, E: AbsolutePositionEvent<B>>(
    state: &mut NeonState,
    event: &E,
    button_state: Option<ButtonState>,
) {
    let output_size = state.output_logical_size();
    let output_geo = Rectangle::from_size(output_size);
    let pos = event.position_transformed(output_geo.size) + output_geo.loc.to_f64();
    apply_pointer_motion_at(state, pos, event.time_msec());

    if let Some(bstate) = button_state
        && let Some(pointer) = state.seat.get_pointer()
    {
        let serial = SERIAL_COUNTER.next_serial();
        pointer.button(
            state,
            &ButtonEvent {
                button: BTN_LEFT,
                state: bstate,
                serial,
                time: event.time_msec(),
            },
        );
        pointer.frame(state);
    }
}

fn handle_pointer_button<B: InputBackend>(state: &mut NeonState, event: B::PointerButtonEvent) {
    let Some(pointer) = state.seat.get_pointer() else {
        return;
    };

    let serial = SERIAL_COUNTER.next_serial();
    let button = event.button_code();
    let button_state = event.state();

    if !state.is_locked() && button_state == ButtonState::Pressed && !pointer.is_grabbed() {
        let pos = state.pointer_location;
        let alt_held = state
            .seat
            .get_keyboard()
            .map(|kbd| kbd.modifier_state().alt)
            .unwrap_or(false);

        // If clicking an upper layer-shell surface (Panel, Dock, Overview), deliver the click
        // to the layer surface instead of windows underneath.
        if let Some((layer_surface, _, interactivity)) =
            state.layer_surface_under(pos, &[WlrLayer::Overlay, WlrLayer::Top])
        {
            if matches!(
                interactivity,
                KeyboardInteractivity::OnDemand | KeyboardInteractivity::Exclusive
            ) && let Some(keyboard) = state.seat.get_keyboard()
            {
                keyboard.set_focus(state, Some(layer_surface), serial);
            }
        } else if let Some((window, mut window_loc)) = state.window_under(pos) {
            // Click-to-focus & raise
            state.focus_window(Some(&window), serial);

            // Compositor-initiated Alt+Left move (auto-unmaximizes/unsnaps cleanly)
            if alt_held && button == BTN_LEFT {
                if let Some(id) = window_id(&window) {
                    let work_area = state.work_area();
                    if let Some(update) = state.wm.set_floating(id, work_area) {
                        state.apply_layout_update(&update);
                        if let Some(loc) = state.space.element_location(&window) {
                            window_loc = loc;
                        }
                    }
                }
                let start_data = PointerGrabStartData {
                    focus: state.surface_under(pos),
                    button,
                    location: pos,
                };
                let grab = MoveSurfaceGrab {
                    start_data,
                    window,
                    initial_window_location: window_loc,
                };
                pointer.set_grab(state, grab, serial, Focus::Clear);
                return;
            }

            if alt_held && button == BTN_RIGHT {
                if let Some(id) = window_id(&window) {
                    let work_area = state.work_area();
                    if let Some(update) = state.wm.set_floating(id, work_area) {
                        state.apply_layout_update(&update);
                        if let Some(loc) = state.space.element_location(&window) {
                            window_loc = loc;
                        }
                    }
                }
                let initial_size = window.geometry().size;
                if let Some(toplevel) = window.toplevel() {
                    toplevel.with_pending_state(|s| {
                        s.states.set(xdg_toplevel::State::Resizing);
                    });
                    toplevel.send_pending_configure();
                }
                let start_data = PointerGrabStartData {
                    focus: state.surface_under(pos),
                    button,
                    location: pos,
                };
                let grab = ResizeSurfaceGrab::start(
                    start_data,
                    window,
                    ResizeEdge::BOTTOM_RIGHT,
                    Rectangle::new(window_loc, initial_size),
                );
                pointer.set_grab(state, grab, serial, Focus::Clear);
                return;
            }
        } else {
            // Clicking empty desktop canvas clears window focus cleanly
            state.focus_window(None, serial);
        }
    }

    pointer.button(
        state,
        &ButtonEvent {
            button,
            state: button_state,
            serial,
            time: event.time_msec(),
        },
    );
    pointer.frame(state);
}

fn handle_pointer_axis<B: InputBackend>(state: &mut NeonState, event: B::PointerAxisEvent) {
    let source = event.source();

    let horizontal_amount = event
        .amount(Axis::Horizontal)
        .unwrap_or_else(|| event.amount_v120(Axis::Horizontal).unwrap_or(0.0) * 15.0 / 120.0);
    let vertical_amount = event
        .amount(Axis::Vertical)
        .unwrap_or_else(|| event.amount_v120(Axis::Vertical).unwrap_or(0.0) * 15.0 / 120.0);

    let horizontal_amount_discrete = event.amount_v120(Axis::Horizontal);
    let vertical_amount_discrete = event.amount_v120(Axis::Vertical);

    let mut frame = AxisFrame::new(event.time_msec()).source(source);
    if horizontal_amount != 0.0 {
        frame = frame.value(Axis::Horizontal, horizontal_amount);
        if let Some(discrete) = horizontal_amount_discrete {
            frame = frame.v120(Axis::Horizontal, discrete as i32);
        }
    } else if source == AxisSource::Finger {
        frame = frame.stop(Axis::Horizontal);
    }

    if vertical_amount != 0.0 {
        frame = frame.value(Axis::Vertical, vertical_amount);
        if let Some(discrete) = vertical_amount_discrete {
            frame = frame.v120(Axis::Vertical, discrete as i32);
        }
    } else if source == AxisSource::Finger {
        frame = frame.stop(Axis::Vertical);
    }

    if let Some(pointer) = state.seat.get_pointer() {
        pointer.axis(state, frame);
        pointer.frame(state);
    }
}

fn handle_gesture_swipe_begin<B: InputBackend>(
    state: &mut NeonState,
    event: B::GestureSwipeBeginEvent,
) {
    let fingers = event.fingers();
    if !state.is_locked() && fingers == 3 {
        state.swipe_gesture = crate::state::SwipeGestureState {
            active: true,
            fingers,
            dx: 0.0,
            dy: 0.0,
            triggered: false,
        };
        return;
    }

    if let Some(pointer) = state.seat.get_pointer() {
        pointer.gesture_swipe_begin(
            state,
            &GestureSwipeBeginEvent {
                serial: SERIAL_COUNTER.next_serial(),
                time: event.time_msec(),
                fingers,
            },
        );
    }
}

fn handle_gesture_swipe_update<B: InputBackend>(
    state: &mut NeonState,
    event: B::GestureSwipeUpdateEvent,
) {
    if state.swipe_gesture.active {
        state.swipe_gesture.dx += event.delta_x();
        state.swipe_gesture.dy += event.delta_y();
        if !state.swipe_gesture.triggered {
            let serial = SERIAL_COUNTER.next_serial();
            if state.swipe_gesture.dx.abs() >= 120.0
                && state.swipe_gesture.dx.abs() > state.swipe_gesture.dy.abs()
            {
                state.swipe_gesture.triggered = true;
                if state.swipe_gesture.dx > 0.0 {
                    state.switch_workspace_prev(serial);
                } else {
                    state.switch_workspace_next(serial);
                }
            } else if state.swipe_gesture.dy <= -90.0 {
                state.swipe_gesture.triggered = true;
                state.set_overview_open(true, serial);
            } else if state.swipe_gesture.dy >= 90.0 {
                state.swipe_gesture.triggered = true;
                state.set_overview_open(false, serial);
            }
        }
        return;
    }

    if let Some(pointer) = state.seat.get_pointer() {
        pointer.gesture_swipe_update(
            state,
            &PointerGestureSwipeUpdateEvent {
                time: event.time_msec(),
                delta: event.delta(),
            },
        );
    }
}

fn handle_gesture_swipe_end<B: InputBackend>(
    state: &mut NeonState,
    event: B::GestureSwipeEndEvent,
) {
    if state.swipe_gesture.active {
        state.swipe_gesture = Default::default();
        return;
    }

    if let Some(pointer) = state.seat.get_pointer() {
        pointer.gesture_swipe_end(
            state,
            &GestureSwipeEndEvent {
                serial: SERIAL_COUNTER.next_serial(),
                time: event.time_msec(),
                cancelled: event.cancelled(),
            },
        );
    }
}

fn handle_gesture_pinch_begin<B: InputBackend>(
    state: &mut NeonState,
    event: B::GesturePinchBeginEvent,
) {
    if let Some(pointer) = state.seat.get_pointer() {
        pointer.gesture_pinch_begin(
            state,
            &GesturePinchBeginEvent {
                serial: SERIAL_COUNTER.next_serial(),
                time: event.time_msec(),
                fingers: event.fingers(),
            },
        );
    }
}

fn handle_gesture_pinch_update<B: InputBackend>(
    state: &mut NeonState,
    event: B::GesturePinchUpdateEvent,
) {
    if let Some(pointer) = state.seat.get_pointer() {
        pointer.gesture_pinch_update(
            state,
            &PointerGesturePinchUpdateEvent {
                time: event.time_msec(),
                delta: event.delta(),
                scale: event.scale(),
                rotation: event.rotation(),
            },
        );
    }
}

fn handle_gesture_pinch_end<B: InputBackend>(
    state: &mut NeonState,
    event: B::GesturePinchEndEvent,
) {
    if let Some(pointer) = state.seat.get_pointer() {
        pointer.gesture_pinch_end(
            state,
            &GesturePinchEndEvent {
                serial: SERIAL_COUNTER.next_serial(),
                time: event.time_msec(),
                cancelled: event.cancelled(),
            },
        );
    }
}

fn handle_gesture_hold_begin<B: InputBackend>(
    state: &mut NeonState,
    event: B::GestureHoldBeginEvent,
) {
    if let Some(pointer) = state.seat.get_pointer() {
        pointer.gesture_hold_begin(
            state,
            &GestureHoldBeginEvent {
                serial: SERIAL_COUNTER.next_serial(),
                time: event.time_msec(),
                fingers: event.fingers(),
            },
        );
    }
}

fn handle_gesture_hold_end<B: InputBackend>(state: &mut NeonState, event: B::GestureHoldEndEvent) {
    if let Some(pointer) = state.seat.get_pointer() {
        pointer.gesture_hold_end(
            state,
            &GestureHoldEndEvent {
                serial: SERIAL_COUNTER.next_serial(),
                time: event.time_msec(),
                cancelled: event.cancelled(),
            },
        );
    }
}
