//! NEON DE Liquid Glass Control Center (`zwlr_layer_shell_v1` `Layer::Overlay`).
//!
//! Built on a strict 4-column floating Liquid Glass grid (`CELL = 68px`, `GAP = 12px`, `FULL = 308px`)
//! with no heavy outer box, smooth spring open/close animations synchronized to GTK's frame clock,
//! and a transparent fullscreen click-catcher backdrop so clicking anywhere outside immediately
//! dismisses Control Center.

use std::{
    cell::{Cell, RefCell},
    f64::consts::PI,
    path::Path,
    rc::Rc,
    sync::Arc,
    time::Duration,
};

use gtk4::{
    Align, Application, ApplicationWindow, Box as GtkBox, Button, ContentFit, DrawingArea,
    EventControllerKey, GestureClick, Image, Label, Orientation, Overflow, Picture, Scale,
    ScrolledWindow, gdk, glib, prelude::*,
};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use neon_ipc::{AppearanceInfo, IpcRequest};
use neon_system::{
    SystemAction, SystemActionSender, SystemSnapshot, notifications::NotificationStore,
    power::read_backlight_percent,
};

use crate::{
    ipc_bridge::IpcCommandSender,
    notifications::NotificationOverlay,
    panel::{launch_sibling_binary, launch_sibling_binary_with_args},
    wallpaper::load_scaled_texture,
};

const CC_CELL: i32 = 70;
const CC_GAP: i32 = 12;
const CC_HALF: i32 = CC_CELL * 2 + CC_GAP; // 152px
const CC_FULL: i32 = CC_CELL * 4 + CC_GAP * 3; // 316px

type StateChangeCallback = Rc<RefCell<Option<Box<dyn Fn(bool)>>>>;

/// Live floating Liquid Glass Control Center anchored under the top-right status cluster.
#[derive(Clone)]
pub struct QuickSettingsPanel {
    window: ApplicationWindow,
    cc_surface: GtkBox,
    is_open: Rc<Cell<bool>>,
    anim_gen: Rc<Cell<u64>>,
    on_state_change: StateChangeCallback,
    syncing: Rc<Cell<bool>>,
    // Row 1–2 Left: Connectivity cluster
    wifi_btn: Button,
    wifi_sub: Label,
    wifi_enabled_state: Rc<Cell<bool>>,
    bt_btn: Button,
    bt_powered_state: Rc<Cell<bool>>,
    net_share_btn: Button,
    // Row 1–2 Right: MPRIS Now Playing (2×2)
    media_bus: Rc<RefCell<Option<String>>>,
    media_art_path: Rc<RefCell<Option<String>>>,
    media_art_pic: Picture,
    media_art_fallback: GtkBox,
    media_title: Label,
    media_artist: Label,
    media_prev_btn: Button,
    media_play_btn: Button,
    media_play_icon: Image,
    media_next_btn: Button,
    // Row 3: Focus + Overview + Displays
    dnd_btn: Button,
    dnd_sub: Label,
    // Row 4: Display Brightness
    brightness_card: GtkBox,
    brightness_pct_label: Label,
    brightness_scale: Scale,
    // Row 5: Sound
    audio_card: GtkBox,
    vol_pct_label: Label,
    volume_scale: Scale,
    mute_btn: Button,
    mute_icon: Image,
    // Row 6: 4 Utility Circles (Theme, Lock, Power Mode, Screenshot)
    theme_btn: Button,
    theme_icon_area: DrawingArea,
    is_light_theme: Rc<Cell<bool>>,
    power_mode_btn: Button,
    power_mode_icon: Image,
    active_power_profile: Rc<RefCell<String>>,
    // Row 7: Power confirmation
    power_confirm_card: GtkBox,
    pending_power_action: Rc<RefCell<Option<PendingPowerAction>>>,
    // Optional Notifications (auto-hidden when empty)
    notif_card: GtkBox,
    notif_list_box: GtkBox,
    store: Arc<NotificationStore>,
    sys_tx: SystemActionSender,
    toast_overlay: NotificationOverlay,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingPowerAction {
    Suspend,
    LogOut,
    Restart,
    ShutDown,
}

impl PendingPowerAction {
    fn prompt(self) -> &'static str {
        match self {
            Self::Suspend => "Suspend computer now?",
            Self::LogOut => "Log out of NEON DE?",
            Self::Restart => "Restart computer now?",
            Self::ShutDown => "Shut down computer now?",
        }
    }

    fn confirm_label(self) -> &'static str {
        match self {
            Self::Suspend => "Suspend",
            Self::LogOut => "Log Out",
            Self::Restart => "Restart",
            Self::ShutDown => "Shut Down",
        }
    }

    fn is_destructive(self) -> bool {
        matches!(self, Self::Restart | Self::ShutDown)
    }
}

impl QuickSettingsPanel {
    pub fn build(
        app: &Application,
        ipc: IpcCommandSender,
        sys_tx: SystemActionSender,
        store: Arc<NotificationStore>,
        toast_overlay: NotificationOverlay,
    ) -> Self {
        // Fullscreen transparent overlay window so clicks anywhere outside immediately close Control Center.
        let window = ApplicationWindow::builder()
            .application(app)
            .title("NEON DE Control Center")
            .decorated(false)
            .build();
        window.add_css_class("neon-quicksettings-window");

        if gtk4_layer_shell::is_supported() {
            window.init_layer_shell();
            window.set_namespace(Some("neon-quick-settings"));
            window.set_layer(Layer::Overlay);
            window.set_anchor(Edge::Top, true);
            window.set_anchor(Edge::Bottom, true);
            window.set_anchor(Edge::Left, true);
            window.set_anchor(Edge::Right, true);
            window.set_exclusive_zone(-1);
            window.set_keyboard_mode(KeyboardMode::None);
        }

        let syncing = Rc::new(Cell::new(false));
        let wifi_enabled_state = Rc::new(Cell::new(false));
        let bt_powered_state = Rc::new(Cell::new(false));
        let is_light_theme = Rc::new(Cell::new(false));
        let is_open = Rc::new(Cell::new(false));
        let anim_gen = Rc::new(Cell::new(0u64));
        let on_state_change: StateChangeCallback = Rc::new(RefCell::new(None));
        let media_bus: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
        let media_art_path: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
        let active_power_profile = Rc::new(RefCell::new("balanced".to_string()));

        // Transparent fullscreen click-catcher backdrop
        let backdrop = GtkBox::new(Orientation::Vertical, 0);
        backdrop.add_css_class("neon-cc-backdrop");
        backdrop.set_hexpand(true);
        backdrop.set_vexpand(true);

        // Animated floating Control Center container anchored top-right below the menu bar
        let cc_surface = GtkBox::new(Orientation::Vertical, CC_GAP);
        cc_surface.add_css_class("neon-cc-surface");
        cc_surface.add_css_class("cc-closed");
        cc_surface.set_halign(Align::End);
        cc_surface.set_valign(Align::Start);
        cc_surface.set_hexpand(true);
        cc_surface.set_vexpand(false);
        cc_surface.set_margin_top(38);
        cc_surface.set_margin_end(12);
        cc_surface.set_size_request(CC_FULL, -1);

        // =========================================================================
        // ROWS 1–2: Top 2×2 Block (Left: Wi-Fi pill + BT/Network circles | Right: Now Playing 2×2)
        // =========================================================================
        let top_block = GtkBox::new(Orientation::Horizontal, CC_GAP);
        top_block.set_size_request(CC_FULL, CC_HALF);
        top_block.set_halign(Align::Center);

        // Left half (148 × 148): Wi-Fi 2×1 stadium pill on top + two 68×68 circles below
        let left_half = GtkBox::new(Orientation::Vertical, CC_GAP);
        left_half.set_size_request(CC_HALF, CC_HALF);

        let (wifi_btn, wifi_sub) = build_stadium_pill(
            "network-wireless-symbolic",
            "Wi-Fi",
            Some("Checking..."),
            true,
        );
        {
            let sys_tx = sys_tx.clone();
            let wifi_state = Rc::clone(&wifi_enabled_state);
            wifi_btn.connect_clicked(move |_| {
                sys_tx.send(SystemAction::SetWifiEnabled(!wifi_state.get()));
            });
        }

        let circle_pair = GtkBox::new(Orientation::Horizontal, CC_GAP);
        circle_pair.set_size_request(CC_HALF, CC_CELL);

        let (bt_btn, _) = build_circle_icon_button("bluetooth-active-symbolic", 24, "Bluetooth");
        {
            let sys_tx = sys_tx.clone();
            let bt_state = Rc::clone(&bt_powered_state);
            bt_btn.connect_clicked(move |_| {
                sys_tx.send(SystemAction::SetBluetoothPowered(!bt_state.get()));
            });
        }

        let (net_share_btn, _) = build_circle_icon_button(
            "network-wireless-hotspot-symbolic",
            24,
            "Network & Wi-Fi Settings",
        );

        circle_pair.append(&bt_btn);
        circle_pair.append(&net_share_btn);

        left_half.append(&wifi_btn);
        left_half.append(&circle_pair);

        // Right half (148 × 148): Real MPRIS Now Playing 2×2 Liquid Glass square
        let media_card = GtkBox::new(Orientation::Vertical, 6);
        media_card.add_css_class("neon-cc-glass");
        media_card.add_css_class("neon-cc-media-card");
        media_card.set_size_request(CC_HALF, CC_HALF);

        let media_art_frame = GtkBox::new(Orientation::Vertical, 0);
        media_art_frame.add_css_class("neon-cc-media-art");
        media_art_frame.set_size_request(46, 46);
        media_art_frame.set_halign(Align::Start);
        media_art_frame.set_valign(Align::Start);
        media_art_frame.set_overflow(Overflow::Hidden);

        let media_art_pic = Picture::new();
        media_art_pic.set_content_fit(ContentFit::Cover);
        media_art_pic.set_can_shrink(true);
        media_art_pic.set_size_request(46, 46);
        media_art_pic.set_visible(false);

        let media_art_fallback = GtkBox::new(Orientation::Horizontal, 0);
        media_art_fallback.add_css_class("neon-cc-media-art-placeholder");
        media_art_fallback.set_size_request(46, 46);
        media_art_fallback.set_halign(Align::Fill);
        media_art_fallback.set_vexpand(true);
        let note_icon = Image::from_icon_name("audio-x-generic-symbolic");
        note_icon.set_pixel_size(22);
        note_icon.set_hexpand(true);
        note_icon.set_vexpand(true);
        note_icon.set_halign(Align::Center);
        note_icon.set_valign(Align::Center);
        media_art_fallback.append(&note_icon);

        media_art_frame.append(&media_art_pic);
        media_art_frame.append(&media_art_fallback);

        let media_text = GtkBox::new(Orientation::Vertical, 1);
        media_text.set_vexpand(true);
        media_text.set_valign(Align::Center);

        let media_title = Label::new(Some("Not Playing"));
        media_title.add_css_class("neon-cc-title");
        media_title.set_halign(Align::Start);
        media_title.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        media_title.set_max_width_chars(12);

        let media_artist = Label::new(Some("No media active"));
        media_artist.add_css_class("neon-cc-sub");
        media_artist.set_halign(Align::Start);
        media_artist.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        media_artist.set_max_width_chars(14);

        media_text.append(&media_title);
        media_text.append(&media_artist);

        let media_controls = GtkBox::new(Orientation::Horizontal, 4);
        media_controls.set_homogeneous(true);
        media_controls.set_halign(Align::Fill);
        media_controls.set_valign(Align::End);

        let (media_prev_btn, _) = build_media_transport_btn("media-skip-backward-symbolic", 16, "Previous");
        let (media_play_btn, media_play_icon) =
            build_media_transport_btn("media-playback-start-symbolic", 20, "Play / Pause");
        let (media_next_btn, _) = build_media_transport_btn("media-skip-forward-symbolic", 16, "Next");

        for (btn, method) in [
            (&media_prev_btn, "Previous"),
            (&media_play_btn, "PlayPause"),
            (&media_next_btn, "Next"),
        ] {
            let sys_tx = sys_tx.clone();
            let bus_ref = Rc::clone(&media_bus);
            let m = method.to_string();
            btn.connect_clicked(move |_| {
                sys_tx.send(SystemAction::MediaCommand {
                    bus_name: bus_ref.borrow().clone(),
                    method: m.clone(),
                });
            });
        }

        media_controls.append(&media_prev_btn);
        media_controls.append(&media_play_btn);
        media_controls.append(&media_next_btn);

        media_card.append(&media_art_frame);
        media_card.append(&media_text);
        media_card.append(&media_controls);

        top_block.append(&left_half);
        top_block.append(&media_card);
        cc_surface.append(&top_block);

        // =========================================================================
        // ROW 3: Focus 2×1 Pill | Overview Circle | Displays Circle (exact 4-col grid)
        // =========================================================================
        let row3 = GtkBox::new(Orientation::Horizontal, CC_GAP);
        row3.set_size_request(CC_FULL, CC_CELL);
        row3.set_halign(Align::Center);

        let (dnd_btn, dnd_sub) =
            build_stadium_pill("weather-clear-night-symbolic", "Focus", None, false);
        {
            let store = Arc::clone(&store);
            let btn_c = dnd_btn.clone();
            let sub_c = dnd_sub.clone();
            dnd_btn.connect_clicked(move |_| {
                let next = !store.is_do_not_disturb();
                store.set_do_not_disturb(next);
                if next {
                    btn_c.add_css_class("active");
                    sub_c.set_text("On");
                    sub_c.set_visible(true);
                } else {
                    btn_c.remove_css_class("active");
                    sub_c.set_text("");
                    sub_c.set_visible(false);
                }
            });
        }

        let (overview_btn, _) = build_circle_icon_button(
            "sidebar-show-symbolic",
            24,
            "Overview & Workspaces",
        );
        let (displays_btn, _) = build_circle_icon_button(
            "video-joined-displays-symbolic",
            24,
            "Display Settings",
        );

        row3.append(&dnd_btn);
        row3.append(&overview_btn);
        row3.append(&displays_btn);
        cc_surface.append(&row3);

        // =========================================================================
        // ROW 4: Display Brightness Liquid Glass Card (sleek white track & round thumb)
        // =========================================================================
        let brightness_card = GtkBox::new(Orientation::Vertical, 6);
        brightness_card.add_css_class("neon-cc-glass");
        brightness_card.add_css_class("neon-cc-slider-card");
        brightness_card.set_size_request(CC_FULL, -1);

        let bright_head = GtkBox::new(Orientation::Horizontal, 8);
        let bright_title = Label::new(Some("Display"));
        bright_title.add_css_class("neon-cc-title");
        bright_title.set_hexpand(true);
        bright_title.set_halign(Align::Start);

        let initial_brightness = read_backlight_percent();
        let brightness_pct_label =
            Label::new(Some(&format!("{}%", initial_brightness.unwrap_or(100))));
        brightness_pct_label.add_css_class("neon-cc-sub");
        bright_head.append(&bright_title);
        bright_head.append(&brightness_pct_label);

        let bright_row = GtkBox::new(Orientation::Horizontal, 8);
        bright_row.set_valign(Align::Center);

        let dim_sun = centered_slot_icon("display-brightness-symbolic", 14, 22, true);
        let brightness_scale = Scale::with_range(Orientation::Horizontal, 5.0, 100.0, 1.0);
        brightness_scale.add_css_class("neon-cc-slider");
        brightness_scale.set_draw_value(false);
        brightness_scale.set_hexpand(true);
        brightness_scale.set_valign(Align::Center);
        brightness_scale.set_value(f64::from(initial_brightness.unwrap_or(100)));
        {
            let sys_tx = sys_tx.clone();
            let syncing = Rc::clone(&syncing);
            let pct_lbl = brightness_pct_label.clone();
            brightness_scale.connect_value_changed(move |s| {
                let pct = s.value().round() as u32;
                pct_lbl.set_text(&format!("{pct}%"));
                if syncing.get() {
                    return;
                }
                sys_tx.send(SystemAction::SetBrightness(pct));
            });
        }
        let max_sun = centered_slot_icon("display-brightness-symbolic", 18, 22, false);

        bright_row.append(&dim_sun);
        bright_row.append(&brightness_scale);
        bright_row.append(&max_sun);

        brightness_card.append(&bright_head);
        brightness_card.append(&bright_row);
        brightness_card.set_visible(initial_brightness.is_some());
        cc_surface.append(&brightness_card);

        // =========================================================================
        // ROW 5: Sound Liquid Glass Card (aligned slider + circular Audio Output button)
        // =========================================================================
        let audio_card = GtkBox::new(Orientation::Vertical, 6);
        audio_card.add_css_class("neon-cc-glass");
        audio_card.add_css_class("neon-cc-slider-card");
        audio_card.set_size_request(CC_FULL, -1);

        let audio_head = GtkBox::new(Orientation::Horizontal, 8);
        let sound_title = Label::new(Some("Sound"));
        sound_title.add_css_class("neon-cc-title");
        sound_title.set_hexpand(true);
        sound_title.set_halign(Align::Start);

        let vol_pct_label = Label::new(Some("70%"));
        vol_pct_label.add_css_class("neon-cc-sub");
        audio_head.append(&sound_title);
        audio_head.append(&vol_pct_label);

        let sound_row = GtkBox::new(Orientation::Horizontal, 8);
        sound_row.set_valign(Align::Center);

        let mute_icon = Image::from_icon_name("audio-volume-low-symbolic");
        mute_icon.set_pixel_size(15);
        mute_icon.set_halign(Align::Center);
        mute_icon.set_valign(Align::Center);
        let mute_btn = Button::new();
        mute_btn.add_css_class("neon-cc-inline-icon-btn");
        mute_btn.set_size_request(22, 22);
        mute_btn.set_halign(Align::Center);
        mute_btn.set_valign(Align::Center);
        mute_btn.set_child(Some(&mute_icon));
        mute_btn.set_tooltip_text(Some("Mute / Unmute"));
        {
            let sys_tx = sys_tx.clone();
            mute_btn.connect_clicked(move |_| {
                sys_tx.send(SystemAction::ToggleSinkMute);
            });
        }

        let volume_scale = Scale::with_range(Orientation::Horizontal, 0.0, 150.0, 2.0);
        volume_scale.add_css_class("neon-cc-slider");
        volume_scale.set_draw_value(false);
        volume_scale.set_hexpand(true);
        volume_scale.set_valign(Align::Center);
        volume_scale.set_value(70.0);
        {
            let sys_tx = sys_tx.clone();
            let syncing = Rc::clone(&syncing);
            let pct_lbl = vol_pct_label.clone();
            volume_scale.connect_value_changed(move |s| {
                pct_lbl.set_text(&format!("{:.0}%", s.value()));
                if syncing.get() {
                    return;
                }
                let vol = (s.value() as f32) / 100.0;
                sys_tx.send(SystemAction::SetSinkVolume(vol));
            });
        }

        let max_vol = centered_slot_icon("audio-volume-high-symbolic", 16, 20, false);

        let audio_out_btn = Button::new();
        audio_out_btn.add_css_class("neon-cc-output-circle");
        audio_out_btn.set_size_request(28, 28);
        audio_out_btn.set_halign(Align::Center);
        audio_out_btn.set_valign(Align::Center);
        audio_out_btn.set_tooltip_text(Some("Sound Output Devices"));
        let out_icon = Image::from_icon_name("audio-speakers-symbolic");
        out_icon.set_pixel_size(14);
        out_icon.set_halign(Align::Center);
        out_icon.set_valign(Align::Center);
        audio_out_btn.set_child(Some(&out_icon));

        sound_row.append(&mute_btn);
        sound_row.append(&volume_scale);
        sound_row.append(&max_vol);
        sound_row.append(&audio_out_btn);

        audio_card.append(&audio_head);
        audio_card.append(&sound_row);
        cc_surface.append(&audio_card);

        // =========================================================================
        // ROW 6: 4 Quick Utility Circles (Dark/Light, Lock, Power Profile, Screenshot)
        // =========================================================================
        let row6 = GtkBox::new(Orientation::Horizontal, CC_GAP);
        row6.set_size_request(CC_FULL, CC_CELL);
        row6.set_halign(Align::Center);

        // Circle 1: Crisp Cairo half-circle contrast icon (Light / Dark Mode)
        let theme_btn = Button::new();
        theme_btn.add_css_class("neon-cc-circle-btn");
        theme_btn.set_size_request(CC_CELL, CC_CELL);
        theme_btn.set_halign(Align::Center);
        theme_btn.set_valign(Align::Center);
        theme_btn.set_tooltip_text(Some("Appearance: Dark Mode (Click to switch)"));

        let theme_icon_area = DrawingArea::new();
        theme_icon_area.set_content_width(24);
        theme_icon_area.set_content_height(24);
        theme_icon_area.set_halign(Align::Center);
        theme_icon_area.set_valign(Align::Center);
        {
            let light_flag = Rc::clone(&is_light_theme);
            theme_icon_area.set_draw_func(move |_, cr, w, h| {
                let cx = f64::from(w) * 0.5;
                let cy = f64::from(h) * 0.5;
                let r = (f64::from(w.min(h)) * 0.5) - 2.0;
                let is_light = light_flag.get();
                if is_light {
                    cr.set_source_rgba(0.06, 0.10, 0.18, 0.94);
                } else {
                    cr.set_source_rgba(0.98, 0.99, 1.0, 0.95);
                }
                cr.set_line_width(1.9);
                cr.arc(cx, cy, r, 0.0, 2.0 * PI);
                let _ = cr.stroke();

                // Fill right half (or left half in light mode) for classic appearance contrast icon
                let (a0, a1) = if is_light {
                    (PI * 0.5, PI * 1.5)
                } else {
                    (-PI * 0.5, PI * 0.5)
                };
                cr.move_to(cx, cy);
                cr.arc(cx, cy, r - 0.5, a0, a1);
                cr.close_path();
                let _ = cr.fill();
            });
        }
        theme_btn.set_child(Some(&theme_icon_area));
        {
            let ipc = ipc.clone();
            let is_light = Rc::clone(&is_light_theme);
            theme_btn.connect_clicked(move |_| {
                let next_theme = if is_light.get() {
                    "neon_dark"
                } else {
                    "neon_light"
                };
                ipc.send(IpcRequest::UpdateAppearance {
                    theme: Some(next_theme.to_string()),
                    accent_color: None,
                    dock_transparency: None,
                    panel_transparency: None,
                    panel_visible: None,
                    show_clock: None,
                    show_workspaces: None,
                    high_contrast: None,
                    wallpaper: None,
                    reduce_blur: None,
                    blur_strength: None,
                    cursor_theme: None,
                    cursor_size: None,
                });
            });
        }

        // Circle 2: Lock Screen
        let (lock_btn, _) =
            build_circle_icon_button("system-lock-screen-symbolic", 22, "Lock Screen");

        // Circle 3: Power Profile cycle (Saver -> Balanced -> Performance)
        let (power_mode_btn, power_mode_icon) = build_circle_icon_button(
            "power-profile-balanced-symbolic",
            22,
            "Power Mode: Balanced (Click to cycle)",
        );
        {
            let sys_tx = sys_tx.clone();
            let prof_ref = Rc::clone(&active_power_profile);
            power_mode_btn.connect_clicked(move |_| {
                let next = match prof_ref.borrow().as_str() {
                    "power-saver" => "balanced",
                    "balanced" => "performance",
                    _ => "power-saver",
                };
                *prof_ref.borrow_mut() = next.to_string();
                sys_tx.send(SystemAction::SetPowerProfile(next.to_string()));
            });
        }

        // Circle 4: Screenshot
        let (shot_btn, _) =
            build_circle_icon_button("applets-screenshooter-symbolic", 23, "Screenshot");

        row6.append(&theme_btn);
        row6.append(&lock_btn);
        row6.append(&power_mode_btn);
        row6.append(&shot_btn);
        cc_surface.append(&row6);

        // =========================================================================
        // ROW 7: Compact System Power Pill (Sleep · Log Out · Restart · Shut Down)
        // =========================================================================
        let power_pill = GtkBox::new(Orientation::Horizontal, 4);
        power_pill.add_css_class("neon-cc-glass");
        power_pill.add_css_class("neon-cc-power-pill");
        power_pill.set_homogeneous(true);
        power_pill.set_size_request(CC_FULL, -1);

        let suspend_btn = build_power_action_btn("weather-clear-night-symbolic", "Sleep", false);
        let logout_btn = build_power_action_btn("system-log-out-symbolic", "Log Out", false);
        let restart_btn = build_power_action_btn("system-reboot-symbolic", "Restart", false);
        let shutdown_btn = build_power_action_btn("system-shutdown-symbolic", "Shut Down", true);

        power_pill.append(&suspend_btn);
        power_pill.append(&logout_btn);
        power_pill.append(&restart_btn);
        power_pill.append(&shutdown_btn);
        cc_surface.append(&power_pill);

        // Inline confirmation strip for power actions (prevents accidental shutdown/reboot)
        let pending_power_action: Rc<RefCell<Option<PendingPowerAction>>> =
            Rc::new(RefCell::new(None));
        let power_confirm_card = GtkBox::new(Orientation::Horizontal, 8);
        power_confirm_card.add_css_class("neon-cc-glass");
        power_confirm_card.add_css_class("neon-cc-confirm-card");
        power_confirm_card.set_size_request(CC_FULL, -1);
        power_confirm_card.set_valign(Align::Center);
        power_confirm_card.set_visible(false);

        let power_confirm_lbl = Label::new(Some("Shut down computer now?"));
        power_confirm_lbl.add_css_class("neon-cc-title");
        power_confirm_lbl.set_hexpand(true);
        power_confirm_lbl.set_halign(Align::Start);
        power_confirm_lbl.set_valign(Align::Center);

        let power_cancel_btn = Button::with_label("Cancel");
        power_cancel_btn.add_css_class("neon-cc-small-btn");
        power_cancel_btn.set_valign(Align::Center);

        let power_ok_btn = Button::with_label("Confirm");
        power_ok_btn.add_css_class("neon-power-confirm-btn");
        power_ok_btn.set_valign(Align::Center);

        power_confirm_card.append(&power_confirm_lbl);
        power_confirm_card.append(&power_cancel_btn);
        power_confirm_card.append(&power_ok_btn);
        cc_surface.append(&power_confirm_card);

        for (btn, action) in [
            (&suspend_btn, PendingPowerAction::Suspend),
            (&logout_btn, PendingPowerAction::LogOut),
            (&restart_btn, PendingPowerAction::Restart),
            (&shutdown_btn, PendingPowerAction::ShutDown),
        ] {
            let pending = Rc::clone(&pending_power_action);
            let card = power_confirm_card.clone();
            let lbl = power_confirm_lbl.clone();
            let ok_btn = power_ok_btn.clone();
            btn.connect_clicked(move |_| {
                *pending.borrow_mut() = Some(action);
                lbl.set_text(action.prompt());
                ok_btn.set_label(action.confirm_label());
                if action.is_destructive() {
                    ok_btn.add_css_class("danger");
                } else {
                    ok_btn.remove_css_class("danger");
                }
                card.set_visible(true);
            });
        }

        {
            let pending = Rc::clone(&pending_power_action);
            let card = power_confirm_card.clone();
            power_cancel_btn.connect_clicked(move |_| {
                *pending.borrow_mut() = None;
                card.set_visible(false);
            });
        }

        // =========================================================================
        // Dynamic Notifications Card (auto-hidden when empty)
        // =========================================================================
        let notif_card = GtkBox::new(Orientation::Vertical, 6);
        notif_card.add_css_class("neon-cc-glass");
        notif_card.add_css_class("neon-cc-slider-card");
        notif_card.set_size_request(CC_FULL, -1);
        notif_card.set_visible(false);

        let notif_head = GtkBox::new(Orientation::Horizontal, 6);
        let notif_title = Label::new(Some("Notifications"));
        notif_title.add_css_class("neon-cc-title");
        notif_title.set_hexpand(true);
        notif_title.set_halign(Align::Start);

        let clear_btn = Button::with_label("Clear");
        clear_btn.add_css_class("neon-cc-small-btn");
        {
            let store = Arc::clone(&store);
            clear_btn.connect_clicked(move |_| {
                store.clear_all();
            });
        }

        notif_head.append(&notif_title);
        notif_head.append(&clear_btn);
        notif_card.append(&notif_head);

        let notif_scroller = ScrolledWindow::builder()
            .max_content_height(116)
            .propagate_natural_height(true)
            .build();
        let notif_list_box = GtkBox::new(Orientation::Vertical, 6);
        notif_scroller.set_child(Some(&notif_list_box));
        notif_card.append(&notif_scroller);
        cc_surface.append(&notif_card);

        // =========================================================================
        // ROW 8: Centered Bottom Liquid Glass Pill ("Open Settings")
        // =========================================================================
        let settings_pill = Button::with_label("Open Settings");
        settings_pill.add_css_class("neon-cc-bottom-pill");
        settings_pill.set_halign(Align::Center);
        cc_surface.append(&settings_pill);

        backdrop.append(&cc_surface);
        window.set_child(Some(&backdrop));
        window.set_visible(false);

        let panel = Self {
            window,
            cc_surface,
            is_open,
            anim_gen,
            on_state_change,
            syncing,
            wifi_btn,
            wifi_sub,
            wifi_enabled_state,
            bt_btn,
            bt_powered_state,
            net_share_btn,
            media_bus,
            media_art_path,
            media_art_pic,
            media_art_fallback,
            media_title,
            media_artist,
            media_prev_btn,
            media_play_btn,
            media_play_icon,
            media_next_btn,
            dnd_btn,
            dnd_sub,
            brightness_card,
            brightness_pct_label,
            brightness_scale,
            audio_card,
            vol_pct_label,
            volume_scale,
            mute_btn,
            mute_icon,
            theme_btn,
            theme_icon_area,
            is_light_theme,
            power_mode_btn,
            power_mode_icon,
            active_power_profile,
            power_confirm_card,
            pending_power_action,
            notif_card,
            notif_list_box,
            store,
            sys_tx,
            toast_overlay,
        };

        // Wire click-anywhere-outside backdrop dismissal
        {
            let outside_click = GestureClick::new();
            outside_click.set_button(0);
            let p = panel.clone();
            let surface = panel.cc_surface.clone();
            outside_click.connect_pressed(move |gesture, _, x, y| {
                if !p.is_open.get() {
                    return;
                }
                let inside = gesture
                    .widget()
                    .and_then(|w| w.pick(x, y, gtk4::PickFlags::DEFAULT))
                    .is_some_and(|picked| {
                        picked == surface.clone().upcast::<gtk4::Widget>()
                            || picked.is_ancestor(&surface)
                    });
                if !inside {
                    p.close();
                }
            });
            backdrop.add_controller(outside_click);
        }

        // Wire Power Confirmation button -> real systemd-logind action
        {
            let p = panel.clone();
            let pending = Rc::clone(&panel.pending_power_action);
            let sys_tx_pwr = panel.sys_tx.clone();
            power_ok_btn.connect_clicked(move |_| {
                let Some(action) = pending.borrow_mut().take() else {
                    return;
                };
                p.power_confirm_card.set_visible(false);
                p.close();
                match action {
                    PendingPowerAction::Suspend => {
                        sys_tx_pwr.send(SystemAction::InvokePowerAction("Suspend".into()));
                    }
                    PendingPowerAction::LogOut => {
                        sys_tx_pwr.send(SystemAction::LogOut);
                    }
                    PendingPowerAction::Restart => {
                        sys_tx_pwr.send(SystemAction::InvokePowerAction("Reboot".into()));
                    }
                    PendingPowerAction::ShutDown => {
                        sys_tx_pwr.send(SystemAction::InvokePowerAction("PowerOff".into()));
                    }
                }
            });
        }

        // Wire Network / Displays / Audio Output / Settings / Overview / Lock / Screenshot
        {
            let p = panel.clone();
            panel.net_share_btn.connect_clicked(move |_| {
                p.close();
                launch_sibling_binary_with_args("neon-settings", &["--page", "network"]);
            });
        }
        {
            let p = panel.clone();
            let ipc = ipc.clone();
            overview_btn.connect_clicked(move |_| {
                p.close();
                ipc.send(IpcRequest::ToggleOverview);
            });
        }
        {
            let p = panel.clone();
            displays_btn.connect_clicked(move |_| {
                p.close();
                launch_sibling_binary_with_args("neon-settings", &["--page", "displays"]);
            });
        }
        {
            let p = panel.clone();
            audio_out_btn.connect_clicked(move |_| {
                p.close();
                launch_sibling_binary_with_args("neon-settings", &["--page", "sound"]);
            });
        }
        {
            let p = panel.clone();
            let ipc = ipc.clone();
            lock_btn.connect_clicked(move |_| {
                p.close();
                ipc.send(IpcRequest::LockSession);
            });
        }
        {
            let p = panel.clone();
            let ipc = ipc.clone();
            shot_btn.connect_clicked(move |_| {
                p.close();
                let ipc_c = ipc.clone();
                glib::timeout_add_local_once(Duration::from_millis(220), move || {
                    ipc_c.send(IpcRequest::OpenScreenshotUi);
                });
            });
        }
        {
            let p = panel.clone();
            settings_pill.connect_clicked(move |_| {
                p.close();
                launch_sibling_binary("neon-settings");
            });
        }

        // Escape key smoothly closes Control Center
        {
            let key_ctrl = EventControllerKey::new();
            let p = panel.clone();
            key_ctrl.connect_key_pressed(move |_, key, _, _| {
                if key == gdk::Key::Escape && p.is_open.get() {
                    p.close();
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            });
            panel.window.add_controller(key_ctrl);
        }

        panel.refresh_notifications();
        panel
    }

    /// Registers a listener notified whenever Control Center opens (`true`) or closes (`false`).
    pub fn set_on_state_change<F: Fn(bool) + 'static>(&self, cb: F) {
        *self.on_state_change.borrow_mut() = Some(Box::new(cb));
    }

    pub fn toggle(&self) {
        if self.is_open.get() {
            self.close();
        } else {
            self.open();
        }
    }

    pub fn open(&self) {
        let epoch = self.anim_gen.get().wrapping_add(1);
        self.anim_gen.set(epoch);
        self.is_open.set(true);
        self.notify_state_change(true);

        if gtk4_layer_shell::is_supported() {
            self.window.set_keyboard_mode(KeyboardMode::OnDemand);
        }

        self.toast_overlay.set_suppressed(true);
        self.sys_tx.send(SystemAction::RefreshNow);
        self.refresh_notifications();
        self.refresh_brightness();

        // Start cleanly in closed transform state, present the overlay, then transition into
        // `.cc-open` after GTK commits its first frame tick so the spring animation always plays.
        self.cc_surface.remove_css_class("cc-open");
        self.cc_surface.add_css_class("cc-closed");
        self.window.set_visible(true);
        self.window.present();

        let triggered = Rc::new(Cell::new(false));
        let ticks = Rc::new(Cell::new(0u8));
        {
            let gen_cell = Rc::clone(&self.anim_gen);
            let trig = Rc::clone(&triggered);
            let ticks_c = Rc::clone(&ticks);
            self.cc_surface.add_tick_callback(move |widget, _| {
                if gen_cell.get() != epoch || trig.get() {
                    return glib::ControlFlow::Break;
                }
                let next = ticks_c.get().saturating_add(1);
                ticks_c.set(next);
                if next >= 2 {
                    trig.set(true);
                    widget.remove_css_class("cc-closed");
                    widget.add_css_class("cc-open");
                    return glib::ControlFlow::Break;
                }
                glib::ControlFlow::Continue
            });
        }

        // Fallback timer guarantees reveal even if frame clock pauses briefly on initial map
        let surface_c = self.cc_surface.clone();
        let gen_cell = Rc::clone(&self.anim_gen);
        glib::timeout_add_local_once(Duration::from_millis(55), move || {
            if gen_cell.get() == epoch && !triggered.get() {
                triggered.set(true);
                surface_c.remove_css_class("cc-closed");
                surface_c.add_css_class("cc-open");
            }
        });
    }

    pub fn close(&self) {
        if !self.is_open.get() && !self.window.is_visible() {
            return;
        }
        let epoch = self.anim_gen.get().wrapping_add(1);
        self.anim_gen.set(epoch);
        self.is_open.set(false);
        *self.pending_power_action.borrow_mut() = None;
        self.power_confirm_card.set_visible(false);
        self.notify_state_change(false);

        self.cc_surface.remove_css_class("cc-open");
        self.cc_surface.add_css_class("cc-closed");

        let win = self.window.clone();
        let toast = self.toast_overlay.clone();
        let gen_cell = Rc::clone(&self.anim_gen);
        glib::timeout_add_local_once(Duration::from_millis(200), move || {
            if gen_cell.get() == epoch {
                if gtk4_layer_shell::is_supported() {
                    win.set_keyboard_mode(KeyboardMode::None);
                }
                win.set_visible(false);
                toast.set_suppressed(false);
            }
        });
    }

    fn notify_state_change(&self, open: bool) {
        if let Some(cb) = self.on_state_change.borrow().as_ref() {
            cb(open);
        }
    }

    fn refresh_brightness(&self) {
        if let Some(pct) = read_backlight_percent() {
            self.brightness_card.set_visible(true);
            self.syncing.set(true);
            self.brightness_scale.set_value(f64::from(pct));
            self.brightness_pct_label.set_text(&format!("{pct}%"));
            self.syncing.set(false);
        } else {
            self.brightness_card.set_visible(false);
        }
    }

    pub fn update_appearance(&self, appearance: &AppearanceInfo) {
        let light = appearance.theme == "neon_light" || appearance.theme == "light";
        self.is_light_theme.set(light);
        self.theme_btn.set_tooltip_text(Some(if light {
            "Appearance: Light Mode (Click for Dark)"
        } else {
            "Appearance: Dark Mode (Click for Light)"
        }));
        self.theme_icon_area.queue_draw();
    }

    pub fn update_system(&self, snap: &SystemSnapshot) {
        self.syncing.set(true);

        // Wi-Fi pill
        self.wifi_enabled_state.set(snap.network.wifi_enabled);
        self.wifi_btn.set_sensitive(snap.network.available);
        if snap.network.wifi_enabled {
            self.wifi_btn.add_css_class("active");
            let sub = snap.network.primary_name.as_deref().unwrap_or("On");
            self.wifi_sub.set_text(sub);
        } else {
            self.wifi_btn.remove_css_class("active");
            self.wifi_sub.set_text("Off");
        }

        // Bluetooth circle
        self.bt_powered_state.set(snap.bluetooth.powered);
        self.bt_btn.set_sensitive(snap.bluetooth.available);
        if snap.bluetooth.powered {
            self.bt_btn.add_css_class("active");
            let n = snap.bluetooth.connected_count();
            let tip = if n > 0 {
                format!("Bluetooth · {n} Connected")
            } else {
                "Bluetooth · On".to_string()
            };
            self.bt_btn.set_tooltip_text(Some(&tip));
        } else {
            self.bt_btn.remove_css_class("active");
            self.bt_btn.set_tooltip_text(Some("Bluetooth · Off"));
        }

        // Network share / quick settings circle lights up when network is online
        if snap.network.wifi_enabled || snap.network.primary_name.is_some() {
            self.net_share_btn.add_css_class("active");
        } else {
            self.net_share_btn.remove_css_class("active");
        }

        // MPRIS Now Playing 2×2 card
        *self.media_bus.borrow_mut() = snap.media.bus_name.clone();
        if snap.media.available {
            let title = snap
                .media
                .title
                .as_deref()
                .or(snap.media.player_name.as_deref())
                .unwrap_or("Media");
            let artist = snap
                .media
                .artist
                .as_deref()
                .or(snap.media.player_name.as_deref())
                .unwrap_or("Ready");
            self.media_title.set_text(title);
            self.media_artist.set_text(artist);
            self.media_play_icon.set_icon_name(Some(if snap.media.playing {
                "media-playback-pause-symbolic"
            } else {
                "media-playback-start-symbolic"
            }));
            self.media_prev_btn.set_sensitive(true);
            self.media_play_btn.set_sensitive(true);
            self.media_next_btn.set_sensitive(true);
        } else {
            self.media_title.set_text("Not Playing");
            self.media_artist.set_text("No media active");
            self.media_play_icon
                .set_icon_name(Some("media-playback-start-symbolic"));
            self.media_prev_btn.set_sensitive(false);
            self.media_play_btn.set_sensitive(false);
            self.media_next_btn.set_sensitive(false);
        }

        if *self.media_art_path.borrow() != snap.media.art_path {
            *self.media_art_path.borrow_mut() = snap.media.art_path.clone();
            if let Some(ref art) = snap.media.art_path
                && let Some(tex) = load_scaled_texture(Path::new(art), 96, 96)
            {
                self.media_art_pic.set_paintable(Some(&tex));
                self.media_art_pic.set_visible(true);
                self.media_art_fallback.set_visible(false);
            } else {
                self.media_art_pic.set_visible(false);
                self.media_art_fallback.set_visible(true);
            }
        }

        // Focus / DND pill
        if self.store.is_do_not_disturb() {
            self.dnd_btn.add_css_class("active");
            self.dnd_sub.set_text("On");
            self.dnd_sub.set_visible(true);
        } else {
            self.dnd_btn.remove_css_class("active");
            self.dnd_sub.set_text("");
            self.dnd_sub.set_visible(false);
        }

        // Audio
        if snap.audio.available {
            let sink = snap
                .audio
                .default_sink_name
                .as_deref()
                .unwrap_or("Sound Output");
            self.audio_card.set_tooltip_text(Some(sink));
            let pct = snap.audio.volume_percent();
            self.volume_scale.set_value(f64::from(pct));
            self.vol_pct_label.set_text(&format!("{pct}%"));
            let icon = if snap.audio.muted || pct == 0 {
                "audio-volume-muted-symbolic"
            } else {
                "audio-volume-low-symbolic"
            };
            self.mute_icon.set_icon_name(Some(icon));
            if snap.audio.muted {
                self.mute_btn.add_css_class("muted");
            } else {
                self.mute_btn.remove_css_class("muted");
            }
        } else {
            self.vol_pct_label.set_text("—");
        }

        // Power Mode utility circle + live Battery tooltip
        let active_prof = snap
            .power
            .active_profile
            .as_deref()
            .unwrap_or("balanced");
        *self.active_power_profile.borrow_mut() = active_prof.to_string();
        let (prof_icon, prof_label) = match active_prof {
            "power-saver" => ("power-profile-power-saver-symbolic", "Power Saver"),
            "performance" => ("power-profile-performance-symbolic", "Performance"),
            _ => ("power-profile-balanced-symbolic", "Balanced"),
        };
        self.power_mode_icon.set_icon_name(Some(prof_icon));
        if active_prof == "performance" {
            self.power_mode_btn.add_css_class("active");
        } else {
            self.power_mode_btn.remove_css_class("active");
        }
        let power_tip = if snap.power.has_battery {
            format!(
                "Power Mode: {prof_label} (Click to cycle)\nBattery: {:.0}% ({})",
                snap.power.battery_percentage,
                snap.power.battery_state.label()
            )
        } else {
            format!("Power Mode: {prof_label} (Click to cycle)")
        };
        self.power_mode_btn.set_tooltip_text(Some(&power_tip));

        self.syncing.set(false);
    }

    pub fn refresh_notifications(&self) {
        while let Some(child) = self.notif_list_box.first_child() {
            self.notif_list_box.remove(&child);
        }

        let history = self.store.history();
        if history.is_empty() {
            self.notif_card.set_visible(false);
            return;
        }

        self.notif_card.set_visible(true);
        for item in history.iter().take(4) {
            let row = GtkBox::new(Orientation::Vertical, 2);
            row.add_css_class("neon-cc-notif-item");
            let title = Label::new(Some(&format!("{} · {}", item.app_name, item.summary)));
            title.add_css_class("neon-cc-title");
            title.set_halign(Align::Start);
            title.set_ellipsize(gtk4::pango::EllipsizeMode::End);
            row.append(&title);

            if !item.body.is_empty() {
                let body = Label::new(Some(&item.body));
                body.add_css_class("neon-cc-sub");
                body.set_halign(Align::Start);
                body.set_wrap(true);
                body.set_max_width_chars(34);
                row.append(&body);
            }
            self.notif_list_box.append(&row);
        }
    }
}

/// Builds a 2×1 stadium Liquid Glass pill (`148 × 68 px`, `border-radius: 999px`) matching
/// the Wi-Fi and Focus pills in the 4-column grid.
fn build_stadium_pill(
    icon_name: &str,
    title: &str,
    subtitle: Option<&str>,
    accent_white_badge: bool,
) -> (Button, Label) {
    let btn = Button::new();
    btn.add_css_class("neon-cc-stadium-pill");
    if accent_white_badge {
        btn.add_css_class("wifi-style");
    }
    btn.set_size_request(CC_HALF, CC_CELL);
    btn.set_halign(Align::Center);
    btn.set_valign(Align::Center);

    let row = GtkBox::new(Orientation::Horizontal, 10);
    row.set_hexpand(true);
    row.set_vexpand(true);
    row.set_valign(Align::Center);

    let badge = GtkBox::new(Orientation::Horizontal, 0);
    badge.add_css_class("neon-cc-pill-badge");
    badge.set_size_request(42, 42);
    badge.set_hexpand(false);
    badge.set_vexpand(false);
    badge.set_halign(Align::Center);
    badge.set_valign(Align::Center);

    let icon = Image::from_icon_name(icon_name);
    icon.set_pixel_size(19);
    icon.set_hexpand(true);
    icon.set_vexpand(true);
    icon.set_halign(Align::Center);
    icon.set_valign(Align::Center);
    badge.append(&icon);

    let text = GtkBox::new(Orientation::Vertical, 1);
    text.set_hexpand(true);
    text.set_valign(Align::Center);

    let title_lbl = Label::new(Some(title));
    title_lbl.add_css_class("neon-cc-title");
    title_lbl.set_halign(Align::Start);

    let sub_lbl = Label::new(Some(subtitle.unwrap_or("")));
    sub_lbl.add_css_class("neon-cc-sub");
    sub_lbl.set_halign(Align::Start);
    sub_lbl.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    sub_lbl.set_max_width_chars(8);
    sub_lbl.set_visible(subtitle.is_some());

    text.append(&title_lbl);
    text.append(&sub_lbl);

    row.append(&badge);
    row.append(&text);
    btn.set_child(Some(&row));

    (btn, sub_lbl)
}

/// Builds a 1×1 circular Liquid Glass button (`68 × 68 px`, `border-radius: 999px`) with a
/// dead-center symbolic icon.
fn build_circle_icon_button(icon_name: &str, pixel_size: i32, tooltip: &str) -> (Button, Image) {
    let btn = Button::new();
    btn.add_css_class("neon-cc-circle-btn");
    btn.set_size_request(CC_CELL, CC_CELL);
    btn.set_halign(Align::Center);
    btn.set_valign(Align::Center);
    btn.set_tooltip_text(Some(tooltip));

    let icon = Image::from_icon_name(icon_name);
    icon.set_pixel_size(pixel_size);
    icon.set_hexpand(true);
    icon.set_vexpand(true);
    icon.set_halign(Align::Center);
    icon.set_valign(Align::Center);
    btn.set_child(Some(&icon));

    (btn, icon)
}

fn build_media_transport_btn(icon_name: &str, pixel_size: i32, tooltip: &str) -> (Button, Image) {
    let btn = Button::new();
    btn.add_css_class("neon-cc-media-btn");
    btn.set_size_request(34, 30);
    btn.set_halign(Align::Center);
    btn.set_valign(Align::Center);
    btn.set_tooltip_text(Some(tooltip));

    let icon = Image::from_icon_name(icon_name);
    icon.set_pixel_size(pixel_size);
    icon.set_halign(Align::Center);
    icon.set_valign(Align::Center);
    btn.set_child(Some(&icon));
    (btn, icon)
}

fn centered_slot_icon(icon_name: &str, pixel_size: i32, slot_px: i32, muted: bool) -> GtkBox {
    let slot = GtkBox::new(Orientation::Horizontal, 0);
    slot.set_size_request(slot_px, slot_px);
    slot.set_halign(Align::Center);
    slot.set_valign(Align::Center);

    let icon = Image::from_icon_name(icon_name);
    icon.set_pixel_size(pixel_size);
    icon.set_hexpand(true);
    icon.set_vexpand(true);
    icon.set_halign(Align::Center);
    icon.set_valign(Align::Center);
    icon.add_css_class(if muted {
        "neon-cc-slider-icon-muted"
    } else {
        "neon-cc-slider-icon"
    });
    slot.append(&icon);
    slot
}

fn build_power_action_btn(icon_name: &str, label: &str, danger: bool) -> Button {
    let btn = Button::new();
    btn.add_css_class("neon-cc-power-btn");
    if danger {
        btn.add_css_class("danger");
    }
    btn.set_tooltip_text(Some(label));

    let col = GtkBox::new(Orientation::Vertical, 3);
    col.set_hexpand(true);
    col.set_halign(Align::Center);
    col.set_valign(Align::Center);

    let icon = Image::from_icon_name(icon_name);
    icon.set_pixel_size(15);
    icon.set_halign(Align::Center);
    icon.set_valign(Align::Center);

    let lbl = Label::new(Some(label));
    lbl.add_css_class("neon-cc-power-lbl");
    lbl.set_halign(Align::Center);
    lbl.set_valign(Align::Center);

    col.append(&icon);
    col.append(&lbl);
    btn.set_child(Some(&col));
    btn
}
