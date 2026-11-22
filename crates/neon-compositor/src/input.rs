//! Input routing, compositor shortcuts, focus policies, and interactive move/resize grabs.

use smithay::{
    backend::input::{
        AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, InputBackend, InputEvent,
        KeyState, KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent,
    },
    input::{
        keyboard::{FilterResult, Keysym, keysyms},
        pointer::{
            AxisFrame, ButtonEvent, Focus, GrabStartData as PointerGrabStartData, MotionEvent,
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
    SpawnTerminal,
    ToggleOverview,
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
        InputEvent::PointerMotionAbsolute { event } => {
            handle_pointer_motion_absolute::<B>(state, event)
        }
        InputEvent::PointerButton { event } => handle_pointer_button::<B>(state, event),
        InputEvent::PointerAxis { event } => handle_pointer_axis::<B>(state, event),
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

    let action = keyboard.input::<CompositorAction, _>(
        state,
        keycode,
        key_state,
        serial,
        time,
        |_state, modifiers, handle| {
            if key_state != KeyState::Pressed {
                return FilterResult::Forward;
            }

            let sym = handle.modified_sym();

            // Emergency / nested exit
            if modifiers.ctrl && modifiers.alt && sym == Keysym::from(keysyms::KEY_Escape) {
                return FilterResult::Intercept(CompositorAction::Quit);
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

            if modifiers.alt && sym == Keysym::from(keysyms::KEY_Tab) {
                return FilterResult::Intercept(CompositorAction::CycleFocus);
            }

            FilterResult::Forward
        },
    );

    if let Some(action) = action {
        match action {
            CompositorAction::Quit => {
                info!("Ctrl+Alt+Escape pressed; exiting NEON DE nested session");
                state.loop_signal.stop();
            }
            CompositorAction::SpawnTerminal => state.spawn_terminal(),
            CompositorAction::ToggleOverview => {
                state.toggle_overview(serial);
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

fn handle_pointer_motion_absolute<B: InputBackend>(
    state: &mut NeonState,
    event: B::PointerMotionAbsoluteEvent,
) {
    let output_size = state.output_logical_size();
    let output_geo = Rectangle::from_size(output_size);
    let pos = event.position_transformed(output_geo.size) + output_geo.loc.to_f64();
    state.pointer_location = pos;

    let serial = SERIAL_COUNTER.next_serial();
    let under = state.surface_under(pos);

    // Optional FocusFollowsMouse policy (ClickToFocus remains default).
    if state.wm.focus_policy() == FocusPolicy::FocusFollowsMouse
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

    if button_state == ButtonState::Pressed && !pointer.is_grabbed() {
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
