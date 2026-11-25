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
    wayland::shell::wlr_layer::KeyboardInteractivity,
};
use tracing::info;

use crate::{
    grabs::{MoveSurfaceGrab, ResizeEdge, ResizeSurfaceGrab},
    state::{NeonState, TitlebarHit},
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
    OpenScreenshotUi,
    TakeScreenshot,
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

            // GNOME-style interactive screenshot UI: Print key, Super+Shift+S, or Super+Shift+4
            if sym == Keysym::from(keysyms::KEY_Print) {
                return FilterResult::Intercept(CompositorAction::OpenScreenshotUi);
            }
            if modifiers.logo
                && modifiers.shift
                && (sym == Keysym::from(keysyms::KEY_s)
                    || sym == Keysym::from(keysyms::KEY_S)
                    || sym == Keysym::from(keysyms::KEY_4)
                    || sym == Keysym::from(keysyms::KEY_dollar))
            {
                return FilterResult::Intercept(CompositorAction::OpenScreenshotUi);
            }
            // Instant full-screen screenshot: macOS Super+Shift+3
            if modifiers.logo
                && modifiers.shift
                && (sym == Keysym::from(keysyms::KEY_3)
                    || sym == Keysym::from(keysyms::KEY_numbersign))
            {
                return FilterResult::Intercept(CompositorAction::TakeScreenshot);
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
                    if !modifiers.ctrl
                        && !modifiers.alt
                        && let Some(ws_idx) = digit_keysym_to_workspace_index(sym).or_else(|| {
                            handle
                                .raw_syms()
                                .iter()
                                .copied()
                                .find_map(digit_keysym_to_workspace_index)
                        })
                    {
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
            CompositorAction::OpenScreenshotUi => {
                state.pending_screenshot_ui = true;
            }
            CompositorAction::TakeScreenshot => {
                state.request_screenshot(None, None);
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
        return Some((raw - keysyms::KEY_1) as usize);
    }
    if raw == keysyms::KEY_0 {
        return Some(9);
    }
    // Numpad digits (NumLock ON)
    if (keysyms::KEY_KP_1..=keysyms::KEY_KP_9).contains(&raw) {
        return Some((raw - keysyms::KEY_KP_1) as usize);
    }
    if raw == keysyms::KEY_KP_0 {
        return Some(9);
    }
    // Numpad navigation keysyms (NumLock OFF) so Super + Numpad 1..9/0 always works
    match raw {
        keysyms::KEY_KP_End => Some(0),
        keysyms::KEY_KP_Down => Some(1),
        keysyms::KEY_KP_Page_Down => Some(2),
        keysyms::KEY_KP_Left => Some(3),
        keysyms::KEY_KP_Begin => Some(4),
        keysyms::KEY_KP_Right => Some(5),
        keysyms::KEY_KP_Home => Some(6),
        keysyms::KEY_KP_Up => Some(7),
        keysyms::KEY_KP_Page_Up => Some(8),
        keysyms::KEY_KP_Insert => Some(9),
        _ => None,
    }
}

fn apply_pointer_motion_at(
    state: &mut NeonState,
    pos: smithay::utils::Point<f64, smithay::utils::Logical>,
    time: u32,
) {
    state.pointer_location = pos;
    state.needs_redraw = true;

    // Update hover highlight for macOS SSD traffic-light controls (×, −, + glyphs on hover).
    let hovered_tb = if !state.is_locked() && state.upper_layer_surface_under(pos).is_none() {
        state.ssd_titlebar_hit_test(pos).and_then(|(id, _, hit)| {
            matches!(
                hit,
                TitlebarHit::Close | TitlebarHit::Minimize | TitlebarHit::Maximize
            )
            .then_some(id)
        })
    } else {
        None
    };
    state.hovered_titlebar_window = hovered_tb;

    let serial = SERIAL_COUNTER.next_serial();
    let under = if state.is_locked() {
        state
            .lock_state
            .primary_surface()
            .cloned()
            .map(|s| (s.into(), (0.0, 0.0).into()))
    } else {
        state.surface_under(pos)
    };

    // Optional FocusFollowsMouse policy (ClickToFocus remains default; disabled while locked).
    // Never pull focus off a visible transient/modal child dialog (e.g. DaVinci Resolve's
    // "Create New Project") back onto its parent window behind it.
    if !state.is_locked()
        && state.wm.focus_policy() == FocusPolicy::FocusFollowsMouse
        && let Some((window, _)) = state.window_under(pos)
        && let Some(hovered_id) = window_id(&window)
        && Some(hovered_id) != state.wm.focused_id()
        && state.wm.visible_transient_children(hovered_id).is_empty()
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
        // Refresh pointer focus before delivering the press if a newly-mapped surface (such as
        // Quick Settings' click-catcher backdrop) appeared underneath a stationary cursor.
        let under = state.surface_under(pos);
        let cur_id = pointer.current_focus().and_then(|f| f.wl_surface_id());
        let under_id = under.as_ref().and_then(|(f, _)| f.wl_surface_id());
        if cur_id != under_id {
            pointer.motion(
                state,
                under,
                &MotionEvent {
                    location: pos,
                    serial: SERIAL_COUNTER.next_serial(),
                    time: event.time_msec(),
                },
            );
            pointer.frame(state);
        }

        let (alt_held, super_held, shift_held) = state
            .seat
            .get_keyboard()
            .map(|kbd| {
                let m = kbd.modifier_state();
                (m.alt, m.logo, m.shift)
            })
            .unwrap_or((false, false, false));

        // If clicking an upper layer-shell surface (Panel, Dock, Overview, Context Menu),
        // deliver the click directly to the layer surface.
        if let Some((layer_surface, _, interactivity)) = state.upper_layer_surface_under(pos) {
            if matches!(
                interactivity,
                KeyboardInteractivity::OnDemand | KeyboardInteractivity::Exclusive
            ) && let Some(keyboard) = state.seat.get_keyboard()
            {
                keyboard.set_focus(state, Some(layer_surface.into()), serial);
            }
        } else if button == BTN_RIGHT
            && let Some((id, window, _hit)) = state.ssd_titlebar_hit_test(pos)
        {
            // Right-clicking a server-side macOS titlebar opens the OS Window Context Menu.
            state.focus_window(Some(&window), serial);
            state.pending_context_menu = Some((
                pos.x.round() as i32,
                pos.y.round() as i32,
                neon_ipc::ContextMenuTarget::Window { window_id: id.0 },
            ));
            state.needs_redraw = true;
            return;
        } else if button == BTN_LEFT
            && let Some((id, window, hit)) = state.ssd_titlebar_hit_test(pos)
        {
            // Clicked a server-side macOS titlebar (traffic-light Close/Minimize/Maximize or drag).
            state.focus_window(Some(&window), serial);
            state.needs_redraw = true;
            match hit {
                TitlebarHit::Close => {
                    state.close_window_by_id(id);
                    return;
                }
                TitlebarHit::Minimize => {
                    state.minimize_window(id, serial);
                    return;
                }
                TitlebarHit::Maximize => {
                    state.toggle_maximize_window(id);
                    return;
                }
                TitlebarHit::Drag => {
                    let work_area = state.effective_work_area_for(id);
                    if let Some(update) = state.wm.set_floating(id, work_area) {
                        state.apply_layout_update(&update);
                    }
                    if let Some(window_loc) = state.space.element_location(&window) {
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
                    }
                    return;
                }
            }
        } else if let Some((window, mut window_loc)) = state.window_under(pos) {
            // Click-to-focus & raise; if this window currently owns a visible transient/modal
            // child dialog (e.g. DaVinci Resolve's "Create New Project"), keep that child
            // raised and focused instead of stealing X11 input focus back onto its parent.
            let focus_target = window_id(&window)
                .and_then(|id| state.wm.visible_transient_children(id).into_iter().last())
                .and_then(|child_id| state.find_window_by_id(child_id))
                .unwrap_or_else(|| window.clone());
            state.focus_window(Some(&focus_target), serial);

            // Super+Shift+Right-Click opens the OS Window Context Menu on any window (CSD or SSD).
            if super_held
                && shift_held
                && button == BTN_RIGHT
                && let Some(id) = window_id(&window)
            {
                state.pending_context_menu = Some((
                    pos.x.round() as i32,
                    pos.y.round() as i32,
                    neon_ipc::ContextMenuTarget::Window { window_id: id.0 },
                ));
                state.needs_redraw = true;
                return;
            }

            // Super+Left-Click or Alt+Left-Click interactive window Move (auto-unmaximizes/unsnaps cleanly).
            if (super_held || alt_held) && button == BTN_LEFT {
                let old_loc = window_loc;
                let old_size = window.geometry().size;
                if let Some(id) = window_id(&window) {
                    let was_non_floating = state
                        .wm
                        .get(id)
                        .map(|r| !r.mode.is_floating())
                        .unwrap_or(false);
                    let work_area = state.effective_work_area_for(id);
                    if let Some(update) = state.wm.set_floating(id, work_area) {
                        if let Some(x11) = window.x11_surface() {
                            let _ = x11.set_maximized(false);
                            let _ = x11.set_fullscreen(false);
                            if let (Some(loc), Some(sz)) =
                                (update.target_location, update.configure_size)
                            {
                                let _ = x11.configure(Rectangle::new(
                                    crate::window::from_wm_point(loc),
                                    crate::window::from_wm_size(sz),
                                ));
                            }
                        }
                        state.apply_layout_update(&update);
                        // Keep pointer proportionally positioned over the restored window when
                        // dragging out of maximized/snapped mode.
                        if was_non_floating && old_size.w > 0 {
                            let new_w = update
                                .configure_size
                                .map(|s| s.w)
                                .unwrap_or(old_size.w)
                                .max(200);
                            let ratio_x = ((pos.x - old_loc.x as f64) / old_size.w as f64)
                                .clamp(0.12, 0.88);
                            let new_x = (pos.x - new_w as f64 * ratio_x).round() as i32;
                            let new_y = (pos.y - 24.0).round().max(work_area.loc.y as f64) as i32;
                            let adjusted: smithay::utils::Point<i32, smithay::utils::Logical> =
                                (new_x, new_y).into();
                            state.space.map_element(window.clone(), adjusted, true);
                            state
                                .wm
                                .set_floating_location(id, crate::window::to_wm_point(adjusted));
                            if let Some(x11) = window.x11_surface() {
                                let mut g = x11.geometry();
                                g.loc = adjusted;
                                let _ = x11.configure(g);
                            }
                            window_loc = adjusted;
                        } else if let Some(loc) = state.space.element_location(&window) {
                            window_loc = loc;
                        }
                    }
                }
                state.cursor_status = smithay::input::pointer::CursorImageStatus::Named(
                    smithay::input::pointer::CursorIcon::Grabbing,
                );
                state.needs_redraw = true;
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

            // Super+Right-Click or Alt+Right-Click interactive quadrant-smart window Resize.
            if (super_held || alt_held) && button == BTN_RIGHT {
                if let Some(id) = window_id(&window) {
                    let work_area = state.effective_work_area_for(id);
                    if let Some(update) = state.wm.set_floating(id, work_area) {
                        if let Some(x11) = window.x11_surface() {
                            let _ = x11.set_maximized(false);
                            let _ = x11.set_fullscreen(false);
                        }
                        state.apply_layout_update(&update);
                        if let Some(loc) = state.space.element_location(&window) {
                            window_loc = loc;
                        }
                    }
                }
                let initial_size = window.geometry().size;
                let rel_x = pos.x - window_loc.x as f64;
                let rel_y = pos.y - window_loc.y as f64;
                let horiz = if rel_x < initial_size.w as f64 * 0.5 {
                    ResizeEdge::LEFT
                } else {
                    ResizeEdge::RIGHT
                };
                let vert = if rel_y < initial_size.h as f64 * 0.5 {
                    ResizeEdge::TOP
                } else {
                    ResizeEdge::BOTTOM
                };
                let edges = horiz | vert;

                let resize_cursor = match (
                    edges.contains(ResizeEdge::LEFT),
                    edges.contains(ResizeEdge::TOP),
                ) {
                    (true, true) => smithay::input::pointer::CursorIcon::NwResize,
                    (false, true) => smithay::input::pointer::CursorIcon::NeResize,
                    (true, false) => smithay::input::pointer::CursorIcon::SwResize,
                    (false, false) => smithay::input::pointer::CursorIcon::SeResize,
                };
                state.cursor_status =
                    smithay::input::pointer::CursorImageStatus::Named(resize_cursor);
                state.needs_redraw = true;

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
                    edges,
                    Rectangle::new(window_loc, initial_size),
                );
                pointer.set_grab(state, grab, serial, Focus::Clear);
                return;
            }
        } else {
            // Clicking empty desktop canvas clears window focus cleanly.
            state.focus_window(None, serial);
            // If a Background/Bottom layer surface (such as `neon-wallpaper` with interactive
            // Desktop Icons) sits under the pointer, let the right-click flow directly to it so
            // it can distinguish File/Folder icon right-clicks from empty Desktop right-clicks.
            if button == BTN_RIGHT
                && state
                    .layer_surface_under(
                        pos,
                        &[
                            smithay::wayland::shell::wlr_layer::Layer::Bottom,
                            smithay::wayland::shell::wlr_layer::Layer::Background,
                        ],
                    )
                    .is_none()
            {
                state.pending_context_menu = Some((
                    pos.x.round() as i32,
                    pos.y.round() as i32,
                    neon_ipc::ContextMenuTarget::Desktop,
                ));
                state.needs_redraw = true;
                return;
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn super_number_row_and_numpad_map_to_workspaces() {
        // Top row 1..9, 0
        for (sym, expected) in [
            (keysyms::KEY_1, 0),
            (keysyms::KEY_2, 1),
            (keysyms::KEY_3, 2),
            (keysyms::KEY_4, 3),
            (keysyms::KEY_5, 4),
            (keysyms::KEY_6, 5),
            (keysyms::KEY_7, 6),
            (keysyms::KEY_8, 7),
            (keysyms::KEY_9, 8),
            (keysyms::KEY_0, 9),
        ] {
            assert_eq!(
                digit_keysym_to_workspace_index(Keysym::from(sym)),
                Some(expected),
                "top row key should map to workspace {expected}"
            );
        }

        // Numpad with NumLock ON
        for (sym, expected) in [
            (keysyms::KEY_KP_1, 0),
            (keysyms::KEY_KP_2, 1),
            (keysyms::KEY_KP_3, 2),
            (keysyms::KEY_KP_4, 3),
            (keysyms::KEY_KP_5, 4),
            (keysyms::KEY_KP_6, 5),
            (keysyms::KEY_KP_7, 6),
            (keysyms::KEY_KP_8, 7),
            (keysyms::KEY_KP_9, 8),
            (keysyms::KEY_KP_0, 9),
        ] {
            assert_eq!(
                digit_keysym_to_workspace_index(Keysym::from(sym)),
                Some(expected),
                "numpad (NumLock ON) key should map to workspace {expected}"
            );
        }

        // Numpad with NumLock OFF
        for (sym, expected) in [
            (keysyms::KEY_KP_End, 0),
            (keysyms::KEY_KP_Down, 1),
            (keysyms::KEY_KP_Page_Down, 2),
            (keysyms::KEY_KP_Left, 3),
            (keysyms::KEY_KP_Begin, 4),
            (keysyms::KEY_KP_Right, 5),
            (keysyms::KEY_KP_Home, 6),
            (keysyms::KEY_KP_Up, 7),
            (keysyms::KEY_KP_Page_Up, 8),
            (keysyms::KEY_KP_Insert, 9),
        ] {
            assert_eq!(
                digit_keysym_to_workspace_index(Keysym::from(sym)),
                Some(expected),
                "numpad (NumLock OFF) key should map to workspace {expected}"
            );
        }

        // Regular arrows & page keys must NOT collide with numpad workspace switching
        for non_ws in [
            keysyms::KEY_Left,
            keysyms::KEY_Right,
            keysyms::KEY_Up,
            keysyms::KEY_Down,
            keysyms::KEY_Page_Up,
            keysyms::KEY_Page_Down,
        ] {
            assert_eq!(digit_keysym_to_workspace_index(Keysym::from(non_ws)), None);
        }
    }
}
