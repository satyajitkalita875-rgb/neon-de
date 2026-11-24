//! Clean page router for the 12 genuine, working NEON DE Settings categories.

pub mod about;
pub mod appearance;
pub mod applications;
pub mod bluetooth;
pub mod datetime;
pub mod displays;
pub mod dock_panels;
pub mod network;
pub mod power;
pub mod shortcuts;
pub mod sound;
pub mod storage;

use std::rc::Rc;

use gtk4::ScrolledWindow;

use crate::state::SettingsController;

/// Builds the GTK4 page widget for any active NEON DE Settings category.
pub fn build_page(category_id: &str, ctrl: Rc<SettingsController>) -> ScrolledWindow {
    match category_id {
        "network" => network::build_page(ctrl),
        "bluetooth" => bluetooth::build_page(ctrl),
        "appearance" => appearance::build_page(ctrl),
        "dock" => dock_panels::build_page(ctrl),
        "displays" => displays::build_page(ctrl),
        "sound" => sound::build_page(ctrl),
        "power" => power::build_page(ctrl),
        "storage" => storage::build_page(ctrl),
        "applications" => applications::build_page(ctrl),
        "shortcuts" => shortcuts::build_page(ctrl),
        "datetime" => datetime::build_page(ctrl),
        "about" => about::build_page(ctrl),
        _ => appearance::build_page(ctrl),
    }
}
