//! Modular page router dispatching all 22 NEON DE Settings categories cleanly.

pub mod about;
pub mod accessibility;
pub mod appearance;
pub mod applications;
pub mod bluetooth;
pub mod datetime;
pub mod desktop;
pub mod displays;
pub mod dock_panels;
pub mod effects;
pub mod input;
pub mod network;
pub mod notifications;
pub mod power;
pub mod privacy;
pub mod shortcuts;
pub mod sound;
pub mod startup_session;
pub mod storage;
pub mod updates;
pub mod users;
pub mod windows;

use std::rc::Rc;

use gtk4::ScrolledWindow;

use crate::state::SettingsController;

/// Builds the GTK4 page widget for any of the 22 NEON DE Settings categories.
pub fn build_page(category_id: &str, ctrl: Rc<SettingsController>) -> ScrolledWindow {
    match category_id {
        "appearance" => appearance::build_page(ctrl),
        "displays" => displays::build_page(ctrl),
        "desktop" => desktop::build_page(ctrl),
        "dock" => dock_panels::build_page(ctrl),
        "windows" => windows::build_page(ctrl),
        "effects" => effects::build_page(ctrl),
        "input" => input::build_page(ctrl),
        "shortcuts" => shortcuts::build_page(ctrl),
        "network" => network::build_page(ctrl),
        "bluetooth" => bluetooth::build_page(ctrl),
        "sound" => sound::build_page(ctrl),
        "notifications" => notifications::build_page(ctrl),
        "power" => power::build_page(ctrl),
        "privacy" => privacy::build_page(ctrl),
        "applications" => applications::build_page(ctrl),
        "users" => users::build_page(ctrl),
        "accessibility" => accessibility::build_page(ctrl),
        "startup" => startup_session::build_page(ctrl),
        "datetime" => datetime::build_page(ctrl),
        "storage" => storage::build_page(ctrl),
        "updates" => updates::build_page(ctrl),
        "about" => about::build_page(ctrl),
        _ => appearance::build_page(ctrl),
    }
}
