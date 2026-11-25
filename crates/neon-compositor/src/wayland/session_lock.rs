//! `ext_session_lock_v1` handler providing strict session-lock security isolation.

use std::collections::HashMap;

use smithay::{
    output::Output,
    reexports::wayland_server::protocol::{wl_output::WlOutput, wl_surface::WlSurface},
    utils::SERIAL_COUNTER,
    wayland::session_lock::{
        LockSurface, SessionLockHandler, SessionLockManagerState, SessionLocker,
    },
};
use tracing::{info, warn};

use crate::state::NeonState;

/// Tracks whether the desktop session is unlocked or locked via `ext_session_lock_v1`.
#[derive(Debug, Default)]
pub enum SessionLockState {
    #[default]
    Unlocked,
    Locked {
        /// Active lock surfaces keyed by output name.
        surfaces: HashMap<String, LockSurface>,
    },
}

impl SessionLockState {
    pub fn is_locked(&self) -> bool {
        matches!(self, Self::Locked { .. })
    }

    pub fn surface_for_output(&self, output_name: &str) -> Option<&LockSurface> {
        match self {
            Self::Unlocked => None,
            Self::Locked { surfaces } => surfaces
                .get(output_name)
                .or_else(|| surfaces.values().next()),
        }
    }

    pub fn primary_surface(&self) -> Option<&WlSurface> {
        match self {
            Self::Unlocked => None,
            Self::Locked { surfaces } => surfaces.values().next().map(|s| s.wl_surface()),
        }
    }
}

impl SessionLockHandler for NeonState {
    fn lock_state(&mut self) -> &mut SessionLockManagerState {
        &mut self.session_lock_manager_state
    }

    fn lock(&mut self, confirmation: SessionLocker) {
        if self.lock_state.is_locked() {
            warn!("Rejecting duplicate session lock request while already locked");
            return;
        }

        // Security invariant: immediately clear focus from all regular clients & close overview.
        let serial = SERIAL_COUNTER.next_serial();
        self.overview_open = false;
        if let Some(keyboard) = self.seat.get_keyboard() {
            keyboard.set_focus(self, None, serial);
        }
        if let Some(pointer) = self.seat.get_pointer() {
            pointer.motion(
                self,
                None,
                &smithay::input::pointer::MotionEvent {
                    location: self.pointer_location,
                    serial,
                    time: self.clock.now().as_millis(),
                },
            );
            pointer.frame(self);
        }

        self.lock_state = SessionLockState::Locked {
            surfaces: HashMap::new(),
        };
        confirmation.lock();
        info!("Session locked via ext_session_lock_v1 (desktop rendering & input isolated)");
    }

    fn unlock(&mut self) {
        self.lock_state = SessionLockState::Unlocked;
        info!("Session unlocked via ext_session_lock_v1");
        self.focus_topmost_window(SERIAL_COUNTER.next_serial());
    }

    fn new_surface(&mut self, surface: LockSurface, wl_output: WlOutput) {
        let output = Output::from_resource(&wl_output).or_else(|| self.primary_output.clone());
        let output_name = output
            .as_ref()
            .map(Output::name)
            .unwrap_or_else(|| "default".to_string());

        let logical_size = output
            .as_ref()
            .and_then(|o| self.space.output_geometry(o))
            .map(|geo| geo.size)
            .unwrap_or_else(|| self.output_logical_size());

        surface.with_pending_state(|state| {
            state.size = Some((logical_size.w.max(1) as u32, logical_size.h.max(1) as u32).into());
        });
        surface.send_configure();

        let wl_surf = surface.wl_surface().clone();
        if let SessionLockState::Locked { surfaces } = &mut self.lock_state {
            surfaces.insert(output_name.clone(), surface);
        }

        if let Some(keyboard) = self.seat.get_keyboard() {
            keyboard.set_focus(self, Some(wl_surf.into()), SERIAL_COUNTER.next_serial());
        }
        info!(output = %output_name, ?logical_size, "Configured ext_session_lock_v1 surface");
    }
}
