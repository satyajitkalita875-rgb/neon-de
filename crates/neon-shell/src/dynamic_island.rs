//! NEON DE Little Notch Dynamic Island (`zwlr_layer_shell_v1` `Layer::Overlay` Top+Center).
//!
//! Inspired by Little Notch (Dynamic Island for Mac) with Material 3 Expressive spring physics
//! and live album-art neon gradient glow:
//! - Top-flush concave-ear MacBook notch silhouette (`y = 0`) in 3 live appearance modes:
//!   **Black**, **Black Glass**, and **Liquid Glass**.
//! - 3-tier fluid interaction:
//!   1. Compact live notch (Idle clock, Now Playing art + animated neon waveform, Focus Timer
//!      ring + countdown, Screen Recording indicator, automatic Volume/Brightness slide-out HUD)
//!   2. Smooth hover-to-expand with velocity-preserving M3 Expressive springs & floating bottom
//!      5-icon module switcher dock + `⊞` All Modules drawer
//!   3. Click / `Super+N` / `Super+V` pinned-open mode with click-outside dismissal, keyboard
//!      shortcuts (`Alt+1..5` dock switch, `Alt+1..9` clipboard quick-copy, `Esc` close), and
//!      Wayland drag-and-drop onto the notch (auto-opens File Shelf)
//! - 12 real native Linux/Wayland modules: Home, Now Playing, Scrubbable Ruler Timer, Clipboard
//!   History (search, filters, hex color swatches, pin), File Shelf (drop, recent, zip/unzip,
//!   drag-out), Quick Notes, Calendar, Quick Tools (math/percentage calc, screenshot, recent
//!   colors), System Stats (CPU/RAM/Net/Disk), Battery & Bluetooth Devices, Keep Awake
//!   (`systemd-inhibit`), and Audio & Display HUD.

use std::{
    cell::{Cell, RefCell},
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use gtk4::{
    Align, Application, ApplicationWindow, Box as GtkBox, Button, ContentFit, DragSource,
    DrawingArea, DropTarget, Entry, EventControllerKey, EventControllerMotion,
    EventControllerScroll, EventControllerScrollFlags, FlowBox, GestureClick, GestureDrag, Label,
    Orientation, Overlay as GtkOverlay, Picture, PolicyType, Scale, ScrolledWindow, SearchEntry, SelectionMode, Stack, StackTransitionType, TextView, WrapMode, cairo, gdk, gio,
    glib, prelude::*,
};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use neon_ipc::{DesktopSnapshot, IpcRequest, WindowInfo};
use neon_system::{
    SystemAction, SystemActionSender, SystemSnapshot,
    clipboard::{
        ClipboardEntry, ClipboardStore, format_relative_age, now_unix_secs,
        write_wayland_clipboard_text,
    },
    media::MediaStatus,
    notifications::{DesktopNotification, NotificationStore, NotificationUrgency},
    power::{BatteryState, read_backlight_percent},
    telemetry::{KeepAwakeInhibitor, TelemetrySampler, TelemetrySnapshot, format_net_rate},
};

use crate::{
    ipc_bridge::IpcCommandSender,
    island_data::{
        CalendarEventStore, ClipboardItemKind, FileShelfItem, NotchModule, NotchPrefs,
        QuickNotesStore, classify_clipboard_text, compress_shelf_item, eval_quick_math,
        extract_shelf_archive, format_file_size, list_recent_user_files,
        load_persisted_shelf_items, next_dropped_image_path, parse_uri_list_paths,
        save_persisted_shelf_items,
    },
    island_visuals::{
        IslandAuraKind, NeonPalette, NotchAppearance, Rgb, Spring1D, UiVectorIcon,
        draw_apple_battery_glyph, draw_apple_fallback_art, draw_apple_silent_pill,
        draw_audio_waveform, draw_battery_ring, draw_cpu_bars, draw_dock_pill_backdrop,
        draw_hud_bar, draw_media_progress_bar, draw_module_badge_tile, draw_module_vector_icon,
        draw_network_sparkline, draw_notch_backdrop, draw_round_circle_button,
        draw_round_pin_button, draw_sf_airplay_icon, draw_sf_play_pause_icon, draw_sf_skip_icon,
        draw_timer_ring, draw_timer_ruler, draw_ui_vector_icon, extract_neon_palette_from_texture,
    },
    notifications::{
        activate_notification, build_notification_icon, visible_notification_actions,
    },
};

const CANVAS_W: i32 = 684;
const CANVAS_H: i32 = 456;
const SHELL_TOP_OFFSET: i32 = 10;
const COMPACT_EAR_R: f64 = 12.0;
const EXPANDED_EAR_R: f64 = 16.0;
const COMPACT_BOTTOM_R: f64 = 18.0;
const EXPANDED_BOTTOM_R: f64 = 36.0;
const DOCK_GAP: i32 = 4;
const DOCK_H: i32 = 54;

type VoidCallback = Rc<RefCell<Option<Box<dyn Fn()>>>>;
type BoolCallback = Rc<RefCell<Option<Box<dyn Fn(bool)>>>>;

#[derive(Debug, Clone)]
struct IslandMorph {
    w: Spring1D,
    h: Spring1D,
    br: Spring1D,
    er: Spring1D,
    alpha: Spring1D,
    target_expanded: bool,
    zeta_spatial: f64,
    omega_spatial: f64,
    last_tick: Instant,
    tick_running: bool,
    hide_on_collapse: bool,
}

impl Default for IslandMorph {
    fn default() -> Self {
        Self {
            w: Spring1D::new(152.0),
            h: Spring1D::new(36.0),
            br: Spring1D::new(COMPACT_BOTTOM_R),
            er: Spring1D::new(COMPACT_EAR_R),
            alpha: Spring1D::new(0.0),
            target_expanded: false,
            zeta_spatial: 0.74,
            omega_spatial: 21.0,
            last_tick: Instant::now(),
            tick_running: false,
            hide_on_collapse: false,
        }
    }
}

#[derive(Debug, Clone)]
enum LivePillKind {
    Hud {
        is_brightness: bool,
        label: String,
        fraction: f64,
        muted: bool,
    },
    Charging {
        pct: f64,
        charging: bool,
    },
    Silent {
        silent: bool,
        label: String,
    },
}

#[derive(Debug, Clone)]
struct LivePillPopup {
    kind: LivePillKind,
    until: Instant,
}

struct IslandState {
    combined_mode: bool,
    island_enabled: bool,
    locked: bool,
    obscured: bool,
    obscured_fullscreen: bool,
    overview_open: bool,
    workspaces_open: bool,
    hovered: bool,
    pinned_open: bool,
    drag_hover: bool,
    desktop_drag_active: bool,
    outgoing_drag: bool,
    peek_over_obscured: bool,
    clock_24h: bool,
    accent: Rgb,
    prefs: NotchPrefs,
    active_module: NotchModule,
    // System & Media
    sys: SystemSnapshot,
    media: MediaStatus,
    prev_volume: Option<(f32, bool)>,
    prev_brightness: Option<u32>,
    prev_on_battery: Option<bool>,
    prev_dnd: Option<bool>,
    hud_popup: Option<LivePillPopup>,
    // Notifications & Recording
    recording_active: bool,
    recording_secs: u64,
    unread_count: usize,
    dnd: bool,
    suppressed: bool,
    active_notif: Option<DesktopNotification>,
    last_art_path: Option<String>,
    last_rendered_notif_key: Option<(u32, u64)>,
    // Generation counters
    hover_gen: u64,
    notif_gen: u64,
    copy_flash_gen: u64,
    test_force_expand: bool,
    // Visual / neon animation
    aura_kind: IslandAuraKind,
    cur_palette: NeonPalette,
    target_palette: NeonPalette,
    neon_intensity: f64,
    target_neon_intensity: f64,
    neon_phase: f64,
    // Interactive Media scrubber
    media_progress_hovered: bool,
    media_dragging: bool,
    media_drag_frac: f64,
    media_seek_override: Option<(i64, Instant)>,
    // Scrubbable Ruler Timer (Focus / Break)
    timer_focus_mode: bool,
    timer_set_mins: u32,
    timer_total_secs: u64,
    timer_remaining_secs: u64,
    timer_running: bool,
    timer_sound: bool,
    timer_ruler_hovered: bool,
    timer_dragging: bool,
    // Clipboard UI
    clip_filter: String,
    clip_query: String,
    copied_entry_id: Option<u64>,
    // File Shelf
    shelf_items: Vec<FileShelfItem>,
    shelf_show_recent: bool,
    shelf_selected_path: Option<PathBuf>,
    shelf_status_msg: Option<String>,
    // Quick Notes
    notes_store: QuickNotesStore,
    active_note_id: u64,
    suppress_note_change: bool,
    // Calendar
    cal_store: CalendarEventStore,
    cal_selected_day_offset: i32,
    // Quick Tools
    calc_last_result: Option<String>,
    // Telemetry (CPU/RAM/Net/Disk)
    telemetry: TelemetrySnapshot,
    cpu_history: Vec<f32>,
    net_rx_history: Vec<u64>,
    net_tx_history: Vec<u64>,
    // Keep Awake
    awake_active: bool,
    awake_remaining_secs: Option<u64>, // None = infinity
    awake_duration_choice: Option<u64>,
    // Morph springs
    morph: IslandMorph,
}

impl IslandState {
    fn new() -> Self {
        let notes_store = QuickNotesStore::load();
        let active_note_id = notes_store.notes.first().map(|n| n.id).unwrap_or(1);
        let prefs = NotchPrefs::load();
        let cal_store = CalendarEventStore::load();
        let shelf_items = load_persisted_shelf_items();
        let shelf_selected_path = shelf_items.first().map(|i| i.path.clone());
        Self {
            combined_mode: false,
            island_enabled: true,
            locked: false,
            obscured: false,
            obscured_fullscreen: false,
            overview_open: false,
            workspaces_open: false,
            hovered: false,
            pinned_open: false,
            drag_hover: false,
            desktop_drag_active: false,
            outgoing_drag: false,
            peek_over_obscured: false,
            clock_24h: false,
            accent: Rgb::new(0.22, 0.74, 0.97),
            prefs,
            active_module: NotchModule::Home,
            sys: SystemSnapshot::default(),
            media: MediaStatus::default(),
            prev_volume: None,
            prev_brightness: None,
            prev_on_battery: None,
            prev_dnd: None,
            hud_popup: None,
            recording_active: false,
            recording_secs: 0,
            unread_count: 0,
            dnd: false,
            suppressed: false,
            active_notif: None,
            last_art_path: None,
            last_rendered_notif_key: None,
            hover_gen: 0,
            notif_gen: 0,
            copy_flash_gen: 0,
            test_force_expand: false,
            aura_kind: IslandAuraKind::Idle,
            cur_palette: NeonPalette::default(),
            target_palette: NeonPalette::default(),
            neon_intensity: 0.0,
            target_neon_intensity: 0.0,
            neon_phase: 0.0,
            media_progress_hovered: false,
            media_dragging: false,
            media_drag_frac: 0.0,
            media_seek_override: None,
            timer_focus_mode: true,
            timer_set_mins: 25,
            timer_total_secs: 25 * 60,
            timer_remaining_secs: 25 * 60,
            timer_running: false,
            timer_sound: true,
            timer_ruler_hovered: false,
            timer_dragging: false,
            clip_filter: "all".into(),
            clip_query: String::new(),
            copied_entry_id: None,
            shelf_items,
            shelf_show_recent: false,
            shelf_selected_path,
            shelf_status_msg: None,
            notes_store,
            active_note_id,
            suppress_note_change: false,
            cal_store,
            cal_selected_day_offset: 0,
            calc_last_result: None,
            telemetry: TelemetrySnapshot::default(),
            cpu_history: vec![0.0; 22],
            net_rx_history: vec![0; 24],
            net_tx_history: vec![0; 24],
            awake_active: false,
            awake_remaining_secs: Some(3600),
            awake_duration_choice: Some(3600),
            morph: IslandMorph::default(),
        }
    }

    /// True when the island's target state is the invisible `260×5px` top-edge sensor over an app.
    fn wants_stealth_sensor(&self) -> bool {
        self.combined_mode
            && self.island_enabled
            && !self.locked
            && !self.overview_open
            && !self.workspaces_open
            && self.obscured
            && !self.obscured_fullscreen
            && !self.pinned_open
            && self.hud_popup.is_none()
            && self.active_notif.is_none()
            && !self.recording_active
            && !self.hovered
            && !self.peek_over_obscured
            && !self.desktop_drag_active
            && !self.drag_hover
            && !self.outgoing_drag
            && !self.test_force_expand
    }

    /// True only once the island has finished its closing spring and settled into stealth mode.
    fn is_stealth_sensor(&self) -> bool {
        self.wants_stealth_sensor()
            && !self.morph.target_expanded
            && self.morph.alpha.value <= 0.02
            && (self.morph.w.value - self.morph.w.target).abs() <= 2.0
            && (self.morph.h.value - self.morph.h.target).abs() <= 2.0
    }
}

/// Interactive Little Notch Dynamic Island overlay for NEON DE.
#[derive(Clone)]
pub struct DynamicIsland {
    window: ApplicationWindow,
    backdrop_window: ApplicationWindow,
    bg_canvas: DrawingArea,
    hover_column: GtkBox,
    shell_box: GtkOverlay,
    dock_pill: GtkOverlay,
    dock_bg_canvas: DrawingArea,
    dock_items_box: GtkBox,
    dock_store_btn: Button,
    dock_store_canvas: DrawingArea,
    compact_layer: GtkBox,
    // Compact capsule views
    pill_charge_group: GtkBox,
    pill_charge_pct: Label,
    pill_charge_bat: DrawingArea,
    pill_silent_group: GtkBox,
    pill_silent_badge: DrawingArea,
    pill_silent_lbl: Label,
    pill_hud_group: GtkBox,
    pill_hud_icon: DrawingArea,
    pill_hud_label: Label,
    pill_hud_bar: DrawingArea,
    pill_hud_pct: Label,
    pill_rec_group: GtkBox,
    pill_rec_label: Label,
    pill_timer_group: GtkBox,
    pill_timer_ring: DrawingArea,
    pill_timer_label: Label,
    pill_media_group: GtkBox,
    pill_media_pic: Picture,
    pill_media_fallback: DrawingArea,
    pill_media_label: Label,
    pill_wave_area: DrawingArea,
    pill_idle_group: GtkBox,
    pill_idle_dot: GtkBox,
    pill_idle_clock: Label,
    pill_idle_badge: Label,
    // Clipped expanded area
    expanded_clip: ScrolledWindow,
    drawer_stack: Stack,
    notif_page: GtkBox,
    rec_page: GtkBox,
    // Home page widgets
    home_weekday: Label,
    home_clock: Label,
    home_date: Label,
    home_live_box: GtkBox,
    home_dnd_btn: Button,
    home_dnd_lbl: Label,
    // Media page widgets
    exp_art_pic: Picture,
    exp_art_fallback: DrawingArea,
    exp_media_title: Label,
    exp_media_sub: Label,
    exp_wave_area: DrawingArea,
    exp_media_elapsed: Label,
    exp_media_remaining: Label,
    media_progress_area: DrawingArea,
    exp_play_canvas: DrawingArea,
    // Timer page widgets
    timer_focus_btn: Button,
    timer_break_btn: Button,
    timer_ruler_area: DrawingArea,
    timer_start_btn: Button,
    timer_sound_btn: Button,
    timer_sound_canvas: DrawingArea,
    timer_readout_lbl: Label,
    // Clipboard page widgets
    clip_filter_row: GtkBox,
    clip_cards_box: GtkBox,
    clip_empty_lbl: Label,
    clip_clear_btn: Button,
    // File Shelf page widgets
    shelf_tab_btn: Button,
    shelf_recent_btn: Button,
    shelf_strip_box: GtkBox,
    shelf_empty_lbl: Label,
    shelf_action_bar: GtkBox,
    shelf_status_lbl: Label,
    // Quick Notes page widgets
    notes_list_box: GtkBox,
    notes_editor: TextView,
    notes_count_lbl: Label,
    // Calendar page widgets
    cal_month_lbl: Label,
    cal_days_strip: GtkBox,
    cal_agenda_box: GtkBox,
    // Quick Tools (Keyboard Quick Calculator) widgets
    tools_calc_entry: Entry,
    tools_calc_result_btn: Button,
    tools_calc_result_lbl: Label,
    // System Stats page widgets
    stats_cpu_lbl: Label,
    stats_cpu_sub: Label,
    stats_cpu_canvas: DrawingArea,
    stats_mem_lbl: Label,
    stats_mem_sub: Label,
    stats_mem_bar: GtkBox,
    stats_mem_fill: GtkBox,
    stats_net_lbl: Label,
    stats_net_sub: Label,
    stats_net_canvas: DrawingArea,
    stats_disk_lbl: Label,
    stats_disk_bar: GtkBox,
    stats_disk_fill: GtkBox,
    // Battery & Devices page widgets
    bat_ring_canvas: DrawingArea,
    bat_status_lbl: Label,
    bat_sub_lbl: Label,
    bat_profile_row: GtkBox,
    bat_devices_box: GtkBox,
    // Keep Awake page widgets
    awake_readout_lbl: Label,
    awake_sub_lbl: Label,
    awake_toggle_btn: Button,
    awake_presets_row: GtkBox,
    // Audio & Display HUD page widgets
    hud_vol_scale: Scale,
    hud_vol_pct: Label,
    hud_vol_sink: Label,
    hud_mute_btn: Button,
    hud_mute_canvas: DrawingArea,
    hud_bright_scale: Scale,
    hud_bright_pct: Label,
    // Store / All Modules page widgets
    store_grid: FlowBox,
    store_count_lbl: Label,
    store_appearance_row: GtkBox,
    // Notification & Recording page widgets
    notif_icon_slot: GtkBox,
    notif_app_lbl: Label,
    notif_sum_lbl: Label,
    notif_body_lbl: Label,
    notif_actions_box: GtkBox,
    exp_rec_time_lbl: Label,
    exp_rec_media_strip: GtkBox,
    exp_rec_media_lbl: Label,
    exp_rec_play_canvas: DrawingArea,
    // Dependencies & shared state
    state: Rc<RefCell<IslandState>>,
    telemetry_sampler: Rc<RefCell<TelemetrySampler>>,
    keep_awake: Arc<KeepAwakeInhibitor>,
    windows: Rc<RefCell<Vec<WindowInfo>>>,
    store: Arc<NotificationStore>,
    clipboard_store: Arc<ClipboardStore>,
    sys_tx: SystemActionSender,
    ipc: IpcCommandSender,
    on_stop_recording: VoidCallback,
    on_open_notifications: VoidCallback,
    on_open_screenshot: BoolCallback,
    motion_enabled: Rc<Cell<bool>>,
    speed_scale: Rc<Cell<f64>>,
    suppress_hud_sliders: Rc<Cell<bool>>,
}

impl DynamicIsland {
    pub fn build(
        app: &Application,
        ipc: IpcCommandSender,
        sys_tx: SystemActionSender,
        store: Arc<NotificationStore>,
        clipboard_store: Arc<ClipboardStore>,
        windows: Rc<RefCell<Vec<WindowInfo>>>,
    ) -> Self {
        // Transparent fullscreen click-outside catcher (Layer::Top, KeyboardMode::None).
        let backdrop_window = ApplicationWindow::builder()
            .application(app)
            .title("NEON DE Dynamic Island Backdrop")
            .decorated(false)
            .resizable(false)
            .build();
        backdrop_window.add_css_class("neon-dynamic-island-window");
        if gtk4_layer_shell::is_supported() {
            backdrop_window.init_layer_shell();
            backdrop_window.set_namespace(Some("neon-dynamic-island-backdrop"));
            backdrop_window.set_layer(Layer::Top);
            backdrop_window.set_anchor(Edge::Top, true);
            backdrop_window.set_anchor(Edge::Bottom, true);
            backdrop_window.set_anchor(Edge::Left, true);
            backdrop_window.set_anchor(Edge::Right, true);
            backdrop_window.set_exclusive_zone(-1);
            backdrop_window.set_keyboard_mode(KeyboardMode::None);
        }
        let backdrop_box = GtkBox::new(Orientation::Vertical, 0);
        backdrop_box.set_hexpand(true);
        backdrop_box.set_vexpand(true);
        backdrop_window.set_child(Some(&backdrop_box));
        backdrop_window.set_visible(false);

        // Main top-flush Little Notch Overlay window (`y = 0`).
        let window = ApplicationWindow::builder()
            .application(app)
            .title("NEON DE Dynamic Island")
            .decorated(false)
            .resizable(false)
            .default_width(CANVAS_W)
            .default_height(CANVAS_H)
            .build();
        window.add_css_class("neon-dynamic-island-window");
        if gtk4_layer_shell::is_supported() {
            window.init_layer_shell();
            window.set_namespace(Some("neon-dynamic-island"));
            window.set_layer(Layer::Overlay);
            window.set_anchor(Edge::Top, true);
            window.set_anchor(Edge::Bottom, false);
            window.set_anchor(Edge::Left, false);
            window.set_anchor(Edge::Right, false);
            window.set_margin(Edge::Top, 0);
            window.set_exclusive_zone(0);
            window.set_keyboard_mode(KeyboardMode::None);
        }

        let stage = GtkOverlay::new();
        stage.add_css_class("neon-island-canvas");
        stage.set_size_request(CANVAS_W, CANVAS_H);
        stage.set_halign(Align::Center);
        stage.set_valign(Align::Start);

        // Layer 0: Concave-ear notch backdrop drawn with Cairo Antialias::Best.
        let bg_canvas = DrawingArea::new();
        bg_canvas.set_size_request(CANVAS_W, CANVAS_H);
        bg_canvas.set_can_target(false);
        stage.set_child(Some(&bg_canvas));

        // Layer 1: Unified vertical hover column holding Island Body + gap + Floating Dock Pill.
        // Starts at `y = 0` (`margin_top = 0`) so top-edge hover & File Dock drag-drop work at `y = 0..5`!
        let hover_column = GtkBox::new(Orientation::Vertical, DOCK_GAP);
        hover_column.set_halign(Align::Center);
        hover_column.set_valign(Align::Start);
        hover_column.set_margin_top(0);

        let shell_box = GtkOverlay::new();
        shell_box.add_css_class("neon-island-shell");
        shell_box.set_halign(Align::Center);
        shell_box.set_valign(Align::Start);
        shell_box.set_margin_top(SHELL_TOP_OFFSET);
        shell_box.set_overflow(gtk4::Overflow::Hidden);
        shell_box.set_size_request(158, 36);

        // =====================================================================
        // 1. Compact Apple Dynamic Island Pill Views
        // =====================================================================
        let compact_layer = GtkBox::new(Orientation::Horizontal, 0);
        compact_layer.set_halign(Align::Fill);
        compact_layer.set_valign(Align::Center);
        compact_layer.set_hexpand(true);
        compact_layer.set_vexpand(true);

        let pill_btn = Button::new();
        pill_btn.add_css_class("neon-island-pill-btn");
        pill_btn.set_hexpand(true);
        pill_btn.set_valign(Align::Center);

        let pill_row = GtkBox::new(Orientation::Horizontal, 8);
        pill_row.set_hexpand(true);
        pill_row.set_halign(Align::Fill);
        pill_row.set_valign(Align::Center);

        // 1a. Apple iOS Charging Pill ("Charging" left · "75%" + green battery glyph right)
        let pill_charge_group = GtkBox::new(Orientation::Horizontal, 8);
        pill_charge_group.set_hexpand(true);
        pill_charge_group.set_valign(Align::Center);
        let pill_charge_title = Label::new(Some("Charging"));
        pill_charge_title.add_css_class("neon-ios-pill-title");
        pill_charge_title.set_halign(Align::Start);
        let charge_spacer = GtkBox::new(Orientation::Horizontal, 0);
        charge_spacer.set_hexpand(true);
        let pill_charge_pct = Label::new(Some("100%"));
        pill_charge_pct.add_css_class("neon-ios-charge-pct");
        let pill_charge_bat = DrawingArea::new();
        pill_charge_bat.set_size_request(34, 17);
        pill_charge_bat.set_valign(Align::Center);
        pill_charge_bat.set_can_target(false);
        pill_charge_group.append(&pill_charge_title);
        pill_charge_group.append(&charge_spacer);
        pill_charge_group.append(&pill_charge_pct);
        pill_charge_group.append(&pill_charge_bat);
        pill_charge_group.set_visible(false);

        // 1b. Apple iOS Silent / Mute / DND Pill (red bell-slash capsule left · "Silent" right)
        let pill_silent_group = GtkBox::new(Orientation::Horizontal, 8);
        pill_silent_group.set_hexpand(true);
        pill_silent_group.set_valign(Align::Center);
        let pill_silent_badge = DrawingArea::new();
        pill_silent_badge.set_size_request(56, 30);
        pill_silent_badge.set_valign(Align::Center);
        pill_silent_badge.set_can_target(false);
        let silent_spacer = GtkBox::new(Orientation::Horizontal, 0);
        silent_spacer.set_hexpand(true);
        let pill_silent_lbl = Label::new(Some("Silent"));
        pill_silent_lbl.add_css_class("neon-ios-silent-lbl");
        pill_silent_lbl.set_halign(Align::End);
        pill_silent_group.append(&pill_silent_badge);
        pill_silent_group.append(&silent_spacer);
        pill_silent_group.append(&pill_silent_lbl);
        pill_silent_group.set_visible(false);

        // 1c. Live Volume / Brightness Slide-Out HUD strip
        let pill_hud_group = GtkBox::new(Orientation::Horizontal, 8);
        pill_hud_group.set_hexpand(true);
        pill_hud_group.set_valign(Align::Center);
        let pill_hud_icon = DrawingArea::new();
        pill_hud_icon.set_size_request(16, 16);
        pill_hud_icon.set_valign(Align::Center);
        pill_hud_icon.set_can_target(false);
        let pill_hud_label = Label::new(Some("Volume"));
        pill_hud_label.add_css_class("neon-island-hud-name");
        pill_hud_label.set_max_width_chars(12);
        pill_hud_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        let pill_hud_bar = DrawingArea::new();
        pill_hud_bar.set_hexpand(true);
        pill_hud_bar.set_size_request(100, 14);
        pill_hud_bar.set_valign(Align::Center);
        let pill_hud_pct = Label::new(Some("56%"));
        pill_hud_pct.add_css_class("neon-island-hud-pct");
        pill_hud_group.append(&pill_hud_icon);
        pill_hud_group.append(&pill_hud_label);
        pill_hud_group.append(&pill_hud_bar);
        pill_hud_group.append(&pill_hud_pct);
        pill_hud_group.set_visible(false);

        // 1d. Screen Recording compact strip
        let pill_rec_group = GtkBox::new(Orientation::Horizontal, 7);
        pill_rec_group.set_halign(Align::Center);
        pill_rec_group.set_hexpand(true);
        pill_rec_group.set_valign(Align::Center);
        let rec_dot = GtkBox::new(Orientation::Horizontal, 0);
        rec_dot.add_css_class("neon-island-rec-dot");
        rec_dot.set_valign(Align::Center);
        let pill_rec_label = Label::new(Some("00:00"));
        pill_rec_label.add_css_class("neon-island-rec-time");
        let rec_tag = Label::new(Some("REC"));
        rec_tag.add_css_class("neon-island-rec-tag");
        pill_rec_group.append(&rec_dot);
        pill_rec_group.append(&pill_rec_label);
        pill_rec_group.append(&rec_tag);
        pill_rec_group.set_visible(false);

        // 1e. Live Focus/Break Timer compact strip (left orange ring + right amber countdown)
        let pill_timer_group = GtkBox::new(Orientation::Horizontal, 8);
        pill_timer_group.set_hexpand(true);
        pill_timer_group.set_valign(Align::Center);
        let pill_timer_ring = DrawingArea::new();
        pill_timer_ring.set_size_request(20, 20);
        pill_timer_ring.set_valign(Align::Center);
        let timer_spacer = GtkBox::new(Orientation::Horizontal, 0);
        timer_spacer.set_hexpand(true);
        let pill_timer_label = Label::new(Some("25:00"));
        pill_timer_label.add_css_class("neon-island-timer-pill");
        pill_timer_group.append(&pill_timer_ring);
        pill_timer_group.append(&timer_spacer);
        pill_timer_group.append(&pill_timer_label);
        pill_timer_group.set_visible(false);

        // 1f. Live Now Playing compact strip (left Apple album art + title + right neon waveform)
        let pill_media_group = GtkBox::new(Orientation::Horizontal, 9);
        pill_media_group.set_hexpand(true);
        pill_media_group.set_valign(Align::Center);
        let pill_media_art_box = GtkOverlay::new();
        pill_media_art_box.add_css_class("neon-island-mini-art");
        pill_media_art_box.set_size_request(22, 22);
        pill_media_art_box.set_overflow(gtk4::Overflow::Hidden);
        let pill_media_pic = Picture::new();
        pill_media_pic.set_content_fit(ContentFit::Cover);
        pill_media_pic.set_size_request(22, 22);
        pill_media_pic.set_visible(false);
        let pill_media_fallback = DrawingArea::new();
        pill_media_fallback.set_size_request(22, 22);
        pill_media_fallback.set_can_target(false);
        pill_media_art_box.set_child(Some(&pill_media_fallback));
        pill_media_art_box.add_overlay(&pill_media_pic);

        let pill_media_label = Label::new(Some("Now Playing"));
        pill_media_label.add_css_class("neon-island-media-pill-title");
        pill_media_label.set_hexpand(true);
        pill_media_label.set_halign(Align::Center);
        pill_media_label.set_max_width_chars(18);
        pill_media_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);

        let pill_wave_area = DrawingArea::new();
        pill_wave_area.set_size_request(26, 16);
        pill_wave_area.set_valign(Align::Center);
        pill_wave_area.set_can_target(false);

        pill_media_group.append(&pill_media_art_box);
        pill_media_group.append(&pill_media_label);
        pill_media_group.append(&pill_wave_area);
        pill_media_group.set_visible(false);

        // 1g. Idle compact Dynamic Island pill strip
        let pill_idle_group = GtkBox::new(Orientation::Horizontal, 7);
        pill_idle_group.set_halign(Align::Center);
        pill_idle_group.set_hexpand(true);
        pill_idle_group.set_valign(Align::Center);
        let pill_idle_dot = GtkBox::new(Orientation::Horizontal, 0);
        pill_idle_dot.add_css_class("neon-island-idle-dot");
        pill_idle_dot.set_valign(Align::Center);
        let pill_idle_clock = Label::new(Some(&format_compact_time(false)));
        pill_idle_clock.add_css_class("neon-island-idle-clock");
        let pill_idle_badge = Label::new(Some("0"));
        pill_idle_badge.add_css_class("neon-island-unread-badge");
        pill_idle_badge.set_visible(false);
        pill_idle_group.append(&pill_idle_dot);
        pill_idle_group.append(&pill_idle_clock);
        pill_idle_group.append(&pill_idle_badge);

        pill_row.append(&pill_charge_group);
        pill_row.append(&pill_silent_group);
        pill_row.append(&pill_hud_group);
        pill_row.append(&pill_rec_group);
        pill_row.append(&pill_timer_group);
        pill_row.append(&pill_media_group);
        pill_row.append(&pill_idle_group);
        pill_btn.set_child(Some(&pill_row));
        compact_layer.append(&pill_btn);
        shell_box.set_child(Some(&compact_layer));

        // =====================================================================
        // 2. Clipped Expandable Module Stage (0x0 min-size for smooth M3 spring)
        // =====================================================================
        let expanded_clip = ScrolledWindow::builder()
            .hscrollbar_policy(PolicyType::External)
            .vscrollbar_policy(PolicyType::External)
            .propagate_natural_width(false)
            .propagate_natural_height(false)
            .kinetic_scrolling(false)
            .min_content_width(0)
            .min_content_height(0)
            .hexpand(true)
            .vexpand(true)
            .build();
        expanded_clip.add_css_class("neon-island-clip");
        expanded_clip.set_overflow(gtk4::Overflow::Hidden);
        expanded_clip.set_visible(false);
        expanded_clip.set_opacity(0.0);
        expanded_clip.set_can_target(false);

        let drawer_stack = Stack::new();
        drawer_stack.add_css_class("neon-island-stack");
        drawer_stack.set_transition_type(StackTransitionType::Crossfade);
        drawer_stack.set_transition_duration(135);
        drawer_stack.set_vhomogeneous(false);
        drawer_stack.set_hhomogeneous(false);
        drawer_stack.set_valign(Align::Start);
        drawer_stack.set_halign(Align::Fill);

        // ---------------------------------------------------------------------
        // MODULE 1: HOME (Exact Little Notch layout: Left Clock/Date + Right Live Now)
        // ---------------------------------------------------------------------
        let home_page = GtkBox::new(Orientation::Horizontal, 18);
        home_page.add_css_class("neon-island-page-pad");
        home_page.set_size_request(496, 162);

        let home_left = GtkBox::new(Orientation::Vertical, 2);
        home_left.set_valign(Align::Center);
        home_left.set_width_request(186);
        let home_weekday = Label::new(Some(&format_weekday()));
        home_weekday.add_css_class("neon-island-muted-xs");
        home_weekday.set_halign(Align::Start);
        let home_clock = Label::new(Some(&format_compact_time(false)));
        home_clock.add_css_class("neon-island-hero-clock");
        home_clock.set_halign(Align::Start);
        let home_date = Label::new(Some(&format_month_day()));
        home_date.add_css_class("neon-island-sub-lbl");
        home_date.set_halign(Align::Start);

        let home_quick_row = GtkBox::new(Orientation::Horizontal, 6);
        home_quick_row.set_margin_top(10);
        let home_nc_btn = Button::with_label("Alerts");
        home_nc_btn.add_css_class("neon-island-action-pill");
        let home_dnd_btn = Button::new();
        home_dnd_btn.add_css_class("neon-island-action-pill");
        let home_dnd_lbl = Label::new(Some("DND"));
        home_dnd_btn.set_child(Some(&home_dnd_lbl));
        home_quick_row.append(&home_nc_btn);
        home_quick_row.append(&home_dnd_btn);

        home_left.append(&home_weekday);
        home_left.append(&home_clock);
        home_left.append(&home_date);
        home_left.append(&home_quick_row);

        let home_sep = GtkBox::new(Orientation::Vertical, 0);
        home_sep.add_css_class("neon-island-vsep");

        let home_right = GtkBox::new(Orientation::Vertical, 8);
        home_right.set_hexpand(true);
        home_right.set_valign(Align::Center);
        let live_eyebrow = Label::new(Some("Live now"));
        live_eyebrow.add_css_class("neon-island-eyebrow");
        live_eyebrow.set_halign(Align::Start);
        let home_live_box = GtkBox::new(Orientation::Vertical, 7);
        home_live_box.set_hexpand(true);
        home_right.append(&live_eyebrow);
        home_right.append(&home_live_box);

        home_page.append(&home_left);
        home_page.append(&home_sep);
        home_page.append(&home_right);
        drawer_stack.add_named(&home_page, Some("home"));

        // ---------------------------------------------------------------------
        // MODULE 2: NOW PLAYING (Authentic Apple iOS Dynamic Island Layout!)
        // ---------------------------------------------------------------------
        let media_page = GtkBox::new(Orientation::Vertical, 12);
        media_page.add_css_class("neon-island-media-card");
        media_page.set_size_request(416, 198);

        let media_top = GtkBox::new(Orientation::Horizontal, 14);
        media_top.set_valign(Align::Center);
        media_top.set_margin_bottom(2);

        let art_frame = GtkOverlay::new();
        art_frame.add_css_class("neon-island-art");
        art_frame.set_size_request(58, 58);
        art_frame.set_overflow(gtk4::Overflow::Hidden);
        let exp_art_fallback = DrawingArea::new();
        exp_art_fallback.set_size_request(58, 58);
        exp_art_fallback.set_can_target(false);
        let exp_art_pic = Picture::new();
        exp_art_pic.set_content_fit(ContentFit::Cover);
        exp_art_pic.set_size_request(58, 58);
        exp_art_pic.set_visible(false);
        art_frame.set_child(Some(&exp_art_fallback));
        art_frame.add_overlay(&exp_art_pic);

        let media_focus_btn = Button::new();
        media_focus_btn.add_css_class("neon-ios-frameless-btn");
        media_focus_btn.set_hexpand(true);
        let media_text = GtkBox::new(Orientation::Vertical, 2);
        media_text.set_valign(Align::Center);
        let exp_media_title = Label::new(Some("Nothing Playing"));
        exp_media_title.add_css_class("neon-island-media-title");
        exp_media_title.set_halign(Align::Start);
        exp_media_title.set_xalign(0.0);
        exp_media_title.set_max_width_chars(24);
        exp_media_title.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        let exp_media_sub = Label::new(Some("Play music or video to control it here"));
        exp_media_sub.add_css_class("neon-island-media-artist");
        exp_media_sub.set_halign(Align::Start);
        exp_media_sub.set_xalign(0.0);
        exp_media_sub.set_max_width_chars(26);
        exp_media_sub.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        media_text.append(&exp_media_title);
        media_text.append(&exp_media_sub);
        media_focus_btn.set_child(Some(&media_text));

        let exp_wave_area = DrawingArea::new();
        exp_wave_area.set_size_request(32, 22);
        exp_wave_area.set_valign(Align::Center);
        exp_wave_area.set_can_target(false);

        media_top.append(&art_frame);
        media_top.append(&media_focus_btn);
        media_top.append(&exp_wave_area);
        media_page.append(&media_top);

        let progress_row = GtkBox::new(Orientation::Horizontal, 10);
        progress_row.set_valign(Align::Center);
        let exp_media_elapsed = Label::new(Some("0:00"));
        exp_media_elapsed.add_css_class("neon-island-media-time");
        exp_media_elapsed.set_width_chars(5);
        exp_media_elapsed.set_xalign(0.0);

        let media_progress_area = DrawingArea::new();
        media_progress_area.set_hexpand(true);
        media_progress_area.set_size_request(180, 18);
        media_progress_area.set_valign(Align::Center);
        media_progress_area.set_cursor_from_name(Some("pointer"));

        let exp_media_remaining = Label::new(Some("-0:00"));
        exp_media_remaining.add_css_class("neon-island-media-time");
        exp_media_remaining.set_width_chars(6);
        exp_media_remaining.set_xalign(1.0);

        progress_row.append(&exp_media_elapsed);
        progress_row.append(&media_progress_area);
        progress_row.append(&exp_media_remaining);
        media_page.append(&progress_row);

        // Bottom control row: Frameless bold white SF glyphs dead-center + AirPlay icon far right!
        let media_ctrl_overlay = GtkOverlay::new();
        media_ctrl_overlay.set_hexpand(true);
        media_ctrl_overlay.set_margin_top(2);

        let media_ctrl_row = GtkBox::new(Orientation::Horizontal, 24);
        media_ctrl_row.set_halign(Align::Center);
        media_ctrl_row.set_valign(Align::Center);

        let prev_btn = Button::new();
        prev_btn.add_css_class("neon-ios-transport-btn");
        prev_btn.set_size_request(44, 38);
        prev_btn.set_tooltip_text(Some("Previous"));
        let prev_canvas = DrawingArea::new();
        prev_canvas.set_size_request(30, 22);
        prev_canvas.set_halign(Align::Center);
        prev_canvas.set_valign(Align::Center);
        prev_canvas.set_can_target(false);
        prev_btn.set_child(Some(&prev_canvas));

        let play_btn = Button::new();
        play_btn.add_css_class("neon-ios-transport-btn");
        play_btn.set_size_request(48, 40);
        play_btn.set_tooltip_text(Some("Play / Pause"));
        let exp_play_canvas = DrawingArea::new();
        exp_play_canvas.set_size_request(28, 28);
        exp_play_canvas.set_halign(Align::Center);
        exp_play_canvas.set_valign(Align::Center);
        exp_play_canvas.set_can_target(false);
        play_btn.set_child(Some(&exp_play_canvas));

        let next_btn = Button::new();
        next_btn.add_css_class("neon-ios-transport-btn");
        next_btn.set_size_request(44, 38);
        next_btn.set_tooltip_text(Some("Next"));
        let next_canvas = DrawingArea::new();
        next_canvas.set_size_request(30, 22);
        next_canvas.set_halign(Align::Center);
        next_canvas.set_valign(Align::Center);
        next_canvas.set_can_target(false);
        next_btn.set_child(Some(&next_canvas));

        media_ctrl_row.append(&prev_btn);
        media_ctrl_row.append(&play_btn);
        media_ctrl_row.append(&next_btn);
        media_ctrl_overlay.set_child(Some(&media_ctrl_row));

        let media_audio_btn = Button::new();
        media_audio_btn.add_css_class("neon-ios-transport-btn");
        media_audio_btn.set_size_request(36, 36);
        media_audio_btn.set_halign(Align::End);
        media_audio_btn.set_valign(Align::Center);
        media_audio_btn.set_tooltip_text(Some("Audio & Display"));
        let airplay_canvas = DrawingArea::new();
        airplay_canvas.set_size_request(24, 24);
        airplay_canvas.set_halign(Align::Center);
        airplay_canvas.set_valign(Align::Center);
        airplay_canvas.set_can_target(false);
        media_audio_btn.set_child(Some(&airplay_canvas));
        media_ctrl_overlay.add_overlay(&media_audio_btn);

        media_page.append(&media_ctrl_overlay);
        drawer_stack.add_named(&media_page, Some("media"));

        // ---------------------------------------------------------------------
        // MODULE 3: TIMER (Exact Little Notch Scrubbable Tick-Mark Ruler!)
        // ---------------------------------------------------------------------
        let timer_page = GtkBox::new(Orientation::Vertical, 10);
        timer_page.add_css_class("neon-island-page-pad");
        timer_page.set_size_request(520, 174);

        let timer_top = GtkBox::new(Orientation::Horizontal, 8);
        let timer_focus_btn = Button::with_label("Focus");
        timer_focus_btn.add_css_class("neon-island-mode-chip");
        timer_focus_btn.add_css_class("focus-active");
        let timer_top_spacer = GtkBox::new(Orientation::Horizontal, 0);
        timer_top_spacer.set_hexpand(true);
        let timer_hint = Label::new(Some("Drag or scroll ruler to set minutes"));
        timer_hint.add_css_class("neon-island-muted-xs");
        let timer_top_spacer2 = GtkBox::new(Orientation::Horizontal, 0);
        timer_top_spacer2.set_hexpand(true);
        let timer_break_btn = Button::with_label("Break");
        timer_break_btn.add_css_class("neon-island-mode-chip");
        timer_top.append(&timer_focus_btn);
        timer_top.append(&timer_top_spacer);
        timer_top.append(&timer_hint);
        timer_top.append(&timer_top_spacer2);
        timer_top.append(&timer_break_btn);
        timer_page.append(&timer_top);

        let timer_ruler_area = DrawingArea::new();
        timer_ruler_area.set_hexpand(true);
        timer_ruler_area.set_size_request(480, 44);
        timer_ruler_area.set_cursor_from_name(Some("ew-resize"));
        timer_page.append(&timer_ruler_area);

        let timer_bottom = GtkBox::new(Orientation::Horizontal, 10);
        timer_bottom.set_valign(Align::Center);
        let timer_start_btn = Button::with_label("Start Timer");
        timer_start_btn.add_css_class("neon-island-timer-start");
        let timer_sound_btn = Button::new();
        timer_sound_btn.add_css_class("neon-island-icon-pill");
        timer_sound_btn.add_css_class("active");
        timer_sound_btn.set_tooltip_text(Some("Toggle completion alert"));
        let timer_sound_canvas = DrawingArea::new();
        timer_sound_canvas.set_size_request(16, 16);
        timer_sound_canvas.set_halign(Align::Center);
        timer_sound_canvas.set_valign(Align::Center);
        timer_sound_canvas.set_can_target(false);
        timer_sound_btn.set_child(Some(&timer_sound_canvas));
        let timer_reset_btn = Button::new();
        timer_reset_btn.add_css_class("neon-island-icon-pill");
        timer_reset_btn.set_tooltip_text(Some("Reset timer"));
        let tr_ic = build_vector_ui_canvas(UiVectorIcon::Refresh, 16, (0.92, 0.92, 0.95, 0.96));
        timer_reset_btn.set_child(Some(&tr_ic));
        let tb_spacer = GtkBox::new(Orientation::Horizontal, 0);
        tb_spacer.set_hexpand(true);
        let timer_readout_lbl = Label::new(Some("25:00"));
        timer_readout_lbl.add_css_class("neon-island-timer-hero");

        timer_bottom.append(&timer_start_btn);
        timer_bottom.append(&timer_sound_btn);
        timer_bottom.append(&timer_reset_btn);
        timer_bottom.append(&tb_spacer);
        timer_bottom.append(&timer_readout_lbl);
        timer_page.append(&timer_bottom);
        drawer_stack.add_named(&timer_page, Some("timer"));

        // ---------------------------------------------------------------------
        // MODULE 4: CLIPBOARD HISTORY (Exact Little Notch Search + Filter Tabs + Card Strip!)
        // ---------------------------------------------------------------------
        let clipboard_page = GtkBox::new(Orientation::Vertical, 10);
        clipboard_page.add_css_class("neon-island-page-pad");
        clipboard_page.set_size_request(604, 246);

        let clip_bar = GtkBox::new(Orientation::Horizontal, 8);
        clip_bar.set_valign(Align::Center);
        let clip_search = SearchEntry::new();
        clip_search.add_css_class("neon-island-search");
        clip_search.set_placeholder_text(Some("Search…"));
        clip_search.set_width_request(146);

        let clip_filter_row = GtkBox::new(Orientation::Horizontal, 4);
        clip_filter_row.set_valign(Align::Center);
        let clip_spacer = GtkBox::new(Orientation::Horizontal, 0);
        clip_spacer.set_hexpand(true);
        let clip_kbd = Label::new(Some("Super+V"));
        clip_kbd.add_css_class("neon-island-kbd-chip");
        let clip_clear_btn = Button::with_label("Clear");
        clip_clear_btn.add_css_class("neon-island-clear-btn");

        clip_bar.append(&clip_search);
        clip_bar.append(&clip_filter_row);
        clip_bar.append(&clip_spacer);
        clip_bar.append(&clip_kbd);
        clip_bar.append(&clip_clear_btn);
        clipboard_page.append(&clip_bar);

        let clip_scroller = ScrolledWindow::builder()
            .hscrollbar_policy(PolicyType::Automatic)
            .vscrollbar_policy(PolicyType::Never)
            .overlay_scrolling(false)
            .propagate_natural_height(false)
            .kinetic_scrolling(true)
            .min_content_height(172)
            .max_content_height(172)
            .hexpand(true)
            .build();
        clip_scroller.add_css_class("neon-island-hscroller");
        attach_vertical_wheel_to_horizontal_scroll(&clip_scroller);

        let clip_cards_wrap = GtkBox::new(Orientation::Horizontal, 0);
        let clip_cards_box = GtkBox::new(Orientation::Horizontal, 9);
        clip_cards_box.add_css_class("neon-island-clip-cards");
        let clip_empty_lbl = Label::new(Some(
            "Clipboard is empty — copy text, code, links, or #hex colors anywhere.",
        ));
        clip_empty_lbl.add_css_class("neon-island-sub-lbl");
        clip_empty_lbl.set_halign(Align::Center);
        clip_empty_lbl.set_valign(Align::Center);
        clip_empty_lbl.set_hexpand(true);
        clip_cards_wrap.append(&clip_cards_box);
        clip_cards_wrap.append(&clip_empty_lbl);
        clip_scroller.set_child(Some(&clip_cards_wrap));
        clipboard_page.append(&clip_scroller);
        drawer_stack.add_named(&clipboard_page, Some("clipboard"));

        // ---------------------------------------------------------------------
        // MODULE 5: FILE DOCK / SHELF (OnePlus-style Seamless Drag-In & Drag-Out!)
        // ---------------------------------------------------------------------
        let files_page = GtkBox::new(Orientation::Vertical, 9);
        files_page.add_css_class("neon-island-page-pad");
        files_page.set_size_request(604, 224);

        let shelf_bar = GtkBox::new(Orientation::Horizontal, 8);
        shelf_bar.set_valign(Align::Center);
        let shelf_tab_btn = Button::with_label("File Dock 0");
        shelf_tab_btn.add_css_class("neon-island-filter-pill");
        shelf_tab_btn.add_css_class("active");
        let shelf_recent_btn = Button::with_label("Recent");
        shelf_recent_btn.add_css_class("neon-island-filter-pill");
        let shelf_sp = GtkBox::new(Orientation::Horizontal, 0);
        shelf_sp.set_hexpand(true);
        let shelf_add_btn = Button::with_label("+ Add File");
        shelf_add_btn.add_css_class("neon-island-action-pill");
        let shelf_clear_btn = Button::with_label("Clear");
        shelf_clear_btn.add_css_class("neon-island-clear-btn");
        shelf_bar.append(&shelf_tab_btn);
        shelf_bar.append(&shelf_recent_btn);
        shelf_bar.append(&shelf_sp);
        shelf_bar.append(&shelf_add_btn);
        shelf_bar.append(&shelf_clear_btn);
        files_page.append(&shelf_bar);

        let shelf_scroller = ScrolledWindow::builder()
            .hscrollbar_policy(PolicyType::Automatic)
            .vscrollbar_policy(PolicyType::Never)
            .overlay_scrolling(false)
            .propagate_natural_height(false)
            .kinetic_scrolling(true)
            .min_content_height(114)
            .max_content_height(114)
            .hexpand(true)
            .build();
        shelf_scroller.add_css_class("neon-island-hscroller");
        attach_vertical_wheel_to_horizontal_scroll(&shelf_scroller);
        let shelf_wrap = GtkBox::new(Orientation::Horizontal, 0);
        let shelf_strip_box = GtkBox::new(Orientation::Horizontal, 10);
        shelf_strip_box.add_css_class("neon-island-shelf-strip");
        let shelf_empty_lbl = Label::new(Some(
            "Drag any file or PNG onto the top notch to dock it here, then drag out to any app.",
        ));
        shelf_empty_lbl.add_css_class("neon-island-sub-lbl");
        shelf_empty_lbl.set_hexpand(true);
        shelf_empty_lbl.set_halign(Align::Center);
        shelf_empty_lbl.set_valign(Align::Center);
        shelf_wrap.append(&shelf_strip_box);
        shelf_wrap.append(&shelf_empty_lbl);
        shelf_scroller.set_child(Some(&shelf_wrap));
        files_page.append(&shelf_scroller);

        let shelf_action_bar = GtkBox::new(Orientation::Horizontal, 6);
        shelf_action_bar.add_css_class("neon-island-mini-strip");
        shelf_action_bar.set_valign(Align::Center);
        let shelf_status_lbl = Label::new(Some("Select a file to Zip, Unzip, Copy Path, or Drag Out"));
        shelf_status_lbl.add_css_class("neon-island-muted-xs");
        shelf_status_lbl.set_hexpand(true);
        shelf_status_lbl.set_halign(Align::Start);
        shelf_status_lbl.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        let shelf_zip_btn = Button::with_label("Zip / Unzip");
        shelf_zip_btn.add_css_class("neon-island-mini-btn");
        let shelf_copy_btn = Button::with_label("Copy Path");
        shelf_copy_btn.add_css_class("neon-island-mini-btn");
        let shelf_open_btn = Button::with_label("Show in Files");
        shelf_open_btn.add_css_class("neon-island-mini-btn");
        shelf_action_bar.append(&shelf_status_lbl);
        shelf_action_bar.append(&shelf_zip_btn);
        shelf_action_bar.append(&shelf_copy_btn);
        shelf_action_bar.append(&shelf_open_btn);
        files_page.append(&shelf_action_bar);
        drawer_stack.add_named(&files_page, Some("files"));

        // ---------------------------------------------------------------------
        // MODULE 6: QUICK NOTES (Exact Little Notch Split-Pane Scratchpad!)
        // ---------------------------------------------------------------------
        let notes_page = GtkBox::new(Orientation::Horizontal, 12);
        notes_page.add_css_class("neon-island-page-pad");
        notes_page.set_size_request(580, 236);

        let notes_sidebar = GtkBox::new(Orientation::Vertical, 6);
        notes_sidebar.set_width_request(164);
        let notes_side_head = GtkBox::new(Orientation::Horizontal, 6);
        let notes_side_title = Label::new(Some("Quick Notes"));
        notes_side_title.add_css_class("neon-island-title-lbl");
        notes_side_title.set_hexpand(true);
        notes_side_title.set_halign(Align::Start);
        let notes_new_btn = Button::new();
        notes_new_btn.add_css_class("neon-island-icon-pill");
        notes_new_btn.set_tooltip_text(Some("New note"));
        let nn_ic = build_vector_ui_canvas(UiVectorIcon::Plus, 15, (1.0, 1.0, 1.0, 0.96));
        notes_new_btn.set_child(Some(&nn_ic));
        notes_side_head.append(&notes_side_title);
        notes_side_head.append(&notes_new_btn);
        notes_sidebar.append(&notes_side_head);

        let notes_scroller = ScrolledWindow::builder()
            .hscrollbar_policy(PolicyType::Never)
            .vscrollbar_policy(PolicyType::Automatic)
            .propagate_natural_height(false)
            .min_content_height(172)
            .max_content_height(172)
            .build();
        notes_scroller.add_css_class("neon-island-clip-scroller");
        let notes_list_box = GtkBox::new(Orientation::Vertical, 5);
        notes_scroller.set_child(Some(&notes_list_box));
        notes_sidebar.append(&notes_scroller);

        let notes_vsep = GtkBox::new(Orientation::Vertical, 0);
        notes_vsep.add_css_class("neon-island-vsep");

        let notes_right = GtkBox::new(Orientation::Vertical, 6);
        notes_right.set_hexpand(true);
        let notes_ed_scroll = ScrolledWindow::builder()
            .hscrollbar_policy(PolicyType::Never)
            .vscrollbar_policy(PolicyType::Automatic)
            .propagate_natural_height(false)
            .min_content_height(166)
            .max_content_height(166)
            .hexpand(true)
            .build();
        notes_ed_scroll.add_css_class("neon-island-clip-scroller");
        let notes_editor = TextView::new();
        notes_editor.add_css_class("neon-island-note-editor");
        notes_editor.set_wrap_mode(WrapMode::WordChar);
        notes_ed_scroll.set_child(Some(&notes_editor));

        let notes_foot = GtkBox::new(Orientation::Horizontal, 6);
        notes_foot.set_valign(Align::Center);
        let note_check_btn = Button::with_label("☐");
        note_check_btn.add_css_class("neon-island-mini-btn");
        note_check_btn.set_tooltip_text(Some("Insert checklist item"));
        let note_bullet_btn = Button::with_label("•");
        note_bullet_btn.add_css_class("neon-island-mini-btn");
        let note_h_btn = Button::with_label("#");
        note_h_btn.add_css_class("neon-island-mini-btn");
        let note_del_btn = Button::with_label("Delete");
        note_del_btn.add_css_class("neon-island-clear-btn");
        let nf_sp = GtkBox::new(Orientation::Horizontal, 0);
        nf_sp.set_hexpand(true);
        let notes_count_lbl = Label::new(Some("0 · 0 words"));
        notes_count_lbl.add_css_class("neon-island-muted-xs");

        notes_foot.append(&note_check_btn);
        notes_foot.append(&note_bullet_btn);
        notes_foot.append(&note_h_btn);
        notes_foot.append(&note_del_btn);
        notes_foot.append(&nf_sp);
        notes_foot.append(&notes_count_lbl);

        notes_right.append(&notes_ed_scroll);
        notes_right.append(&notes_foot);
        notes_page.append(&notes_sidebar);
        notes_page.append(&notes_vsep);
        notes_page.append(&notes_right);
        drawer_stack.add_named(&notes_page, Some("notes"));

        // ---------------------------------------------------------------------
        // MODULE 7: CALENDAR (Exact Little Notch 7-Day Strip + Quick Add + Agenda!)
        // ---------------------------------------------------------------------
        let cal_page = GtkBox::new(Orientation::Vertical, 10);
        cal_page.add_css_class("neon-island-page-pad");
        cal_page.set_size_request(548, 252);

        let cal_head = GtkBox::new(Orientation::Horizontal, 10);
        cal_head.set_valign(Align::Center);
        let cal_today_lbl = Label::new(Some("Today"));
        cal_today_lbl.add_css_class("neon-island-title-lbl");
        let cal_month_lbl = Label::new(Some(&format_month_day()));
        cal_month_lbl.add_css_class("neon-island-sub-lbl");
        let cal_sp = GtkBox::new(Orientation::Horizontal, 0);
        cal_sp.set_hexpand(true);
        let cal_add_entry = Entry::new();
        cal_add_entry.add_css_class("neon-island-search");
        cal_add_entry.set_placeholder_text(Some("+ Add Design review 3:30pm"));
        cal_add_entry.set_width_request(240);
        cal_head.append(&cal_today_lbl);
        cal_head.append(&cal_month_lbl);
        cal_head.append(&cal_sp);
        cal_head.append(&cal_add_entry);
        cal_page.append(&cal_head);

        let cal_days_strip = GtkBox::new(Orientation::Horizontal, 6);
        cal_days_strip.set_homogeneous(true);
        cal_page.append(&cal_days_strip);

        let cal_scroller = ScrolledWindow::builder()
            .hscrollbar_policy(PolicyType::Never)
            .vscrollbar_policy(PolicyType::Automatic)
            .propagate_natural_height(false)
            .min_content_height(114)
            .max_content_height(114)
            .hexpand(true)
            .build();
        cal_scroller.add_css_class("neon-island-clip-scroller");
        let cal_agenda_box = GtkBox::new(Orientation::Vertical, 6);
        cal_scroller.set_child(Some(&cal_agenda_box));
        cal_page.append(&cal_scroller);
        drawer_stack.add_named(&cal_page, Some("calendar"));

        // ---------------------------------------------------------------------
        // MODULE 8: QUICK TOOLS (2×2 Quick Tiles + Keyboard-First Quick Calculator)
        // ---------------------------------------------------------------------
        let tools_page = GtkBox::new(Orientation::Horizontal, 14);
        tools_page.add_css_class("neon-island-page-pad");
        tools_page.set_size_request(534, 184);

        let tools_grid = GtkBox::new(Orientation::Vertical, 8);
        tools_grid.set_width_request(208);
        tools_grid.set_valign(Align::Center);
        let tg_r1 = GtkBox::new(Orientation::Horizontal, 8);
        tg_r1.set_homogeneous(true);
        let tg_r2 = GtkBox::new(Orientation::Horizontal, 8);
        tg_r2.set_homogeneous(true);

        let tool_shot_btn = build_tool_tile_ui(UiVectorIcon::Screenshot, "Screenshot");
        let tool_rec_btn = build_tool_tile_ui(UiVectorIcon::Record, "Record");
        let tool_clip_btn = build_tool_tile_mod(NotchModule::Clipboard, "Clipboard");
        let tool_files_btn = build_tool_tile_mod(NotchModule::Files, "File Dock");
        tg_r1.append(&tool_shot_btn);
        tg_r1.append(&tool_rec_btn);
        tg_r2.append(&tool_clip_btn);
        tg_r2.append(&tool_files_btn);
        tools_grid.append(&tg_r1);
        tools_grid.append(&tg_r2);

        let tools_right = GtkBox::new(Orientation::Vertical, 8);
        tools_right.set_hexpand(true);
        tools_right.set_vexpand(true);
        tools_right.set_homogeneous(true);

        // Upper box: expression input with subtle low-opacity +/- example placeholder
        let calc_in_box = GtkBox::new(Orientation::Horizontal, 0);
        calc_in_box.add_css_class("neon-island-calc-input-box");
        calc_in_box.set_hexpand(true);
        calc_in_box.set_vexpand(true);
        let tools_calc_entry = Entry::new();
        tools_calc_entry.set_placeholder_text(Some("24 + 16 - 5"));
        tools_calc_entry.add_css_class("neon-island-calc-entry");
        tools_calc_entry.set_hexpand(true);
        tools_calc_entry.set_valign(Align::Center);
        calc_in_box.append(&tools_calc_entry);

        // Lower box: `=` mark on left + live answer on right (click or Enter copies)
        let tools_calc_result_btn = Button::new();
        tools_calc_result_btn.add_css_class("neon-island-calc-ans-box");
        tools_calc_result_btn.set_hexpand(true);
        tools_calc_result_btn.set_vexpand(true);
        tools_calc_result_btn.set_tooltip_text(Some("Press Enter or click to copy"));
        let ans_row = GtkBox::new(Orientation::Horizontal, 10);
        ans_row.set_valign(Align::Center);
        ans_row.set_hexpand(true);
        let eq_lbl = Label::new(Some("="));
        eq_lbl.add_css_class("neon-island-calc-eq");
        eq_lbl.set_halign(Align::Start);
        let tools_calc_result_lbl = Label::new(Some("0"));
        tools_calc_result_lbl.add_css_class("neon-island-calc-hero");
        tools_calc_result_lbl.add_css_class("empty");
        tools_calc_result_lbl.set_hexpand(true);
        tools_calc_result_lbl.set_halign(Align::End);
        tools_calc_result_lbl.set_xalign(1.0);
        tools_calc_result_lbl.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        ans_row.append(&eq_lbl);
        ans_row.append(&tools_calc_result_lbl);
        tools_calc_result_btn.set_child(Some(&ans_row));

        tools_right.append(&calc_in_box);
        tools_right.append(&tools_calc_result_btn);
        tools_page.append(&tools_grid);
        tools_page.append(&tools_right);
        drawer_stack.add_named(&tools_page, Some("tools"));

        // ---------------------------------------------------------------------
        // MODULE 9: SYSTEM STATS (Exact Little Notch 4-Column Live Telemetry!)
        // ---------------------------------------------------------------------
        let stats_page = GtkBox::new(Orientation::Horizontal, 14);
        stats_page.add_css_class("neon-island-page-pad");
        stats_page.set_size_request(588, 174);
        stats_page.set_homogeneous(true);

        // Col 1: CPU
        let st_cpu = GtkBox::new(Orientation::Vertical, 3);
        let st_cpu_k = Label::new(Some("CPU"));
        st_cpu_k.add_css_class("neon-island-eyebrow");
        st_cpu_k.set_halign(Align::Start);
        let stats_cpu_lbl = Label::new(Some("0%"));
        stats_cpu_lbl.add_css_class("neon-island-stat-val");
        stats_cpu_lbl.set_halign(Align::Start);
        let stats_cpu_sub = Label::new(Some("Cool"));
        stats_cpu_sub.add_css_class("neon-island-muted-xs");
        stats_cpu_sub.set_halign(Align::Start);
        let stats_cpu_canvas = DrawingArea::new();
        stats_cpu_canvas.set_size_request(110, 44);
        stats_cpu_canvas.set_vexpand(true);
        st_cpu.append(&st_cpu_k);
        st_cpu.append(&stats_cpu_lbl);
        st_cpu.append(&stats_cpu_sub);
        st_cpu.append(&stats_cpu_canvas);

        // Col 2: Memory
        let st_mem = GtkBox::new(Orientation::Vertical, 4);
        let st_mem_k = Label::new(Some("Memory"));
        st_mem_k.add_css_class("neon-island-eyebrow");
        st_mem_k.set_halign(Align::Start);
        let stats_mem_lbl = Label::new(Some("0.0 GB"));
        stats_mem_lbl.add_css_class("neon-island-stat-val");
        stats_mem_lbl.set_halign(Align::Start);
        let stats_mem_sub = Label::new(Some("of 0 GB"));
        stats_mem_sub.add_css_class("neon-island-muted-xs");
        stats_mem_sub.set_halign(Align::Start);
        let mem_sp = GtkBox::new(Orientation::Vertical, 0);
        mem_sp.set_vexpand(true);
        let stats_mem_bar = GtkBox::new(Orientation::Horizontal, 0);
        stats_mem_bar.add_css_class("neon-island-stat-track");
        let stats_mem_fill = GtkBox::new(Orientation::Horizontal, 0);
        stats_mem_fill.add_css_class("neon-island-stat-fill");
        stats_mem_fill.set_size_request(16, 6);
        stats_mem_bar.append(&stats_mem_fill);
        st_mem.append(&st_mem_k);
        st_mem.append(&stats_mem_lbl);
        st_mem.append(&stats_mem_sub);
        st_mem.append(&mem_sp);
        st_mem.append(&stats_mem_bar);

        // Col 3: Network
        let st_net = GtkBox::new(Orientation::Vertical, 3);
        let st_net_k = Label::new(Some("Network"));
        st_net_k.add_css_class("neon-island-eyebrow");
        st_net_k.set_halign(Align::Start);
        let stats_net_lbl = Label::new(Some("0 KB/s"));
        stats_net_lbl.add_css_class("neon-island-stat-val");
        stats_net_lbl.set_halign(Align::Start);
        let stats_net_sub = Label::new(Some("0 B/s up"));
        stats_net_sub.add_css_class("neon-island-muted-xs");
        stats_net_sub.set_halign(Align::Start);
        let stats_net_canvas = DrawingArea::new();
        stats_net_canvas.set_size_request(110, 44);
        stats_net_canvas.set_vexpand(true);
        st_net.append(&st_net_k);
        st_net.append(&stats_net_lbl);
        st_net.append(&stats_net_sub);
        st_net.append(&stats_net_canvas);

        // Col 4: Disk
        let st_disk = GtkBox::new(Orientation::Vertical, 4);
        let st_disk_k = Label::new(Some("Disk"));
        st_disk_k.add_css_class("neon-island-eyebrow");
        st_disk_k.set_halign(Align::Start);
        let stats_disk_lbl = Label::new(Some("0.0 GB"));
        stats_disk_lbl.add_css_class("neon-island-stat-val");
        stats_disk_lbl.set_halign(Align::Start);
        let st_disk_sub = Label::new(Some("free on /"));
        st_disk_sub.add_css_class("neon-island-muted-xs");
        st_disk_sub.set_halign(Align::Start);
        let disk_sp = GtkBox::new(Orientation::Vertical, 0);
        disk_sp.set_vexpand(true);
        let stats_disk_bar = GtkBox::new(Orientation::Horizontal, 0);
        stats_disk_bar.add_css_class("neon-island-stat-track");
        let stats_disk_fill = GtkBox::new(Orientation::Horizontal, 0);
        stats_disk_fill.add_css_class("neon-island-stat-fill");
        stats_disk_fill.set_size_request(16, 6);
        stats_disk_bar.append(&stats_disk_fill);
        st_disk.append(&st_disk_k);
        st_disk.append(&stats_disk_lbl);
        st_disk.append(&st_disk_sub);
        st_disk.append(&disk_sp);
        st_disk.append(&stats_disk_bar);

        stats_page.append(&st_cpu);
        stats_page.append(&st_mem);
        stats_page.append(&st_net);
        stats_page.append(&st_disk);
        drawer_stack.add_named(&stats_page, Some("stats"));

        // ---------------------------------------------------------------------
        // MODULE 10: BATTERY & BLUETOOTH DEVICES (Exact Little Notch Rings!)
        // ---------------------------------------------------------------------
        let bat_page = GtkBox::new(Orientation::Horizontal, 16);
        bat_page.add_css_class("neon-island-page-pad");
        bat_page.set_size_request(512, 174);

        let bat_left = GtkBox::new(Orientation::Vertical, 8);
        bat_left.set_valign(Align::Center);
        let bat_top_row = GtkBox::new(Orientation::Horizontal, 12);
        bat_top_row.set_valign(Align::Center);
        let bat_ring_canvas = DrawingArea::new();
        bat_ring_canvas.set_size_request(54, 54);
        let bat_txt = GtkBox::new(Orientation::Vertical, 2);
        bat_txt.set_valign(Align::Center);
        let bat_status_lbl = Label::new(Some("Battery"));
        bat_status_lbl.add_css_class("neon-island-title-lbl");
        bat_status_lbl.set_halign(Align::Start);
        let bat_sub_lbl = Label::new(Some("Checking power…"));
        bat_sub_lbl.add_css_class("neon-island-sub-lbl");
        bat_sub_lbl.set_halign(Align::Start);
        bat_txt.append(&bat_status_lbl);
        bat_txt.append(&bat_sub_lbl);
        bat_top_row.append(&bat_ring_canvas);
        bat_top_row.append(&bat_txt);

        let bat_profile_row = GtkBox::new(Orientation::Horizontal, 5);
        bat_left.append(&bat_top_row);
        bat_left.append(&bat_profile_row);

        let bat_sep = GtkBox::new(Orientation::Vertical, 0);
        bat_sep.add_css_class("neon-island-vsep");

        let bat_right = GtkBox::new(Orientation::Vertical, 6);
        bat_right.set_hexpand(true);
        bat_right.set_valign(Align::Center);
        let bt_head = Label::new(Some("Connected Devices"));
        bt_head.add_css_class("neon-island-eyebrow");
        bt_head.set_halign(Align::Start);
        let bat_devices_box = GtkBox::new(Orientation::Vertical, 5);
        bat_right.append(&bt_head);
        bat_right.append(&bat_devices_box);

        bat_page.append(&bat_left);
        bat_page.append(&bat_sep);
        bat_page.append(&bat_right);
        drawer_stack.add_named(&bat_page, Some("battery"));

        // ---------------------------------------------------------------------
        // MODULE 11: KEEP AWAKE (Exact Little Notch Huge Tabular Countdown!)
        // ---------------------------------------------------------------------
        let awake_page = GtkBox::new(Orientation::Vertical, 12);
        awake_page.add_css_class("neon-island-page-pad");
        awake_page.set_size_request(472, 166);

        let awake_top = GtkBox::new(Orientation::Horizontal, 14);
        awake_top.set_valign(Align::Center);
        let awake_left = GtkBox::new(Orientation::Vertical, 2);
        awake_left.set_hexpand(true);
        let awake_readout_lbl = Label::new(Some("1:00:00"));
        awake_readout_lbl.add_css_class("neon-island-awake-hero");
        awake_readout_lbl.set_halign(Align::Start);
        let awake_sub_lbl = Label::new(Some("Keep display & system awake"));
        awake_sub_lbl.add_css_class("neon-island-sub-lbl");
        awake_sub_lbl.set_halign(Align::Start);
        awake_left.append(&awake_readout_lbl);
        awake_left.append(&awake_sub_lbl);

        let awake_toggle_btn = Button::with_label("Start");
        awake_toggle_btn.add_css_class("neon-island-awake-btn");
        awake_top.append(&awake_left);
        awake_top.append(&awake_toggle_btn);

        let awake_presets_row = GtkBox::new(Orientation::Horizontal, 8);
        awake_presets_row.set_homogeneous(true);
        awake_page.append(&awake_top);
        awake_page.append(&awake_presets_row);
        drawer_stack.add_named(&awake_page, Some("awake"));

        // ---------------------------------------------------------------------
        // MODULE 12: AUDIO & DISPLAY HUD (Interactive Volume + Brightness)
        // ---------------------------------------------------------------------
        let hud_page = GtkBox::new(Orientation::Vertical, 12);
        hud_page.add_css_class("neon-island-page-pad");
        hud_page.set_size_request(468, 162);

        let vol_head = GtkBox::new(Orientation::Horizontal, 8);
        let hud_vol_sink = Label::new(Some("Output Audio"));
        hud_vol_sink.add_css_class("neon-island-title-lbl");
        hud_vol_sink.set_hexpand(true);
        hud_vol_sink.set_halign(Align::Start);
        hud_vol_sink.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        let hud_vol_pct = Label::new(Some("50%"));
        hud_vol_pct.add_css_class("neon-island-media-time");
        vol_head.append(&hud_vol_sink);
        vol_head.append(&hud_vol_pct);

        let vol_row = GtkBox::new(Orientation::Horizontal, 10);
        vol_row.set_valign(Align::Center);
        let hud_mute_btn = Button::new();
        hud_mute_btn.add_css_class("neon-island-icon-pill");
        hud_mute_btn.set_size_request(30, 30);
        hud_mute_btn.set_halign(Align::Center);
        hud_mute_btn.set_valign(Align::Center);
        let hud_mute_canvas = DrawingArea::new();
        hud_mute_canvas.set_size_request(18, 18);
        hud_mute_canvas.set_halign(Align::Center);
        hud_mute_canvas.set_valign(Align::Center);
        hud_mute_canvas.set_can_target(false);
        hud_mute_btn.set_child(Some(&hud_mute_canvas));
        let hud_vol_scale = Scale::with_range(Orientation::Horizontal, 0.0, 100.0, 1.0);
        hud_vol_scale.add_css_class("neon-island-slider");
        hud_vol_scale.set_draw_value(false);
        hud_vol_scale.set_hexpand(true);
        vol_row.append(&hud_mute_btn);
        vol_row.append(&hud_vol_scale);

        let br_head = GtkBox::new(Orientation::Horizontal, 8);
        let br_title = Label::new(Some("Display Brightness"));
        br_title.add_css_class("neon-island-title-lbl");
        br_title.set_hexpand(true);
        br_title.set_halign(Align::Start);
        let hud_bright_pct = Label::new(Some("75%"));
        hud_bright_pct.add_css_class("neon-island-media-time");
        br_head.append(&br_title);
        br_head.append(&hud_bright_pct);

        let br_row = GtkBox::new(Orientation::Horizontal, 10);
        br_row.set_valign(Align::Center);
        let br_badge = GtkBox::new(Orientation::Horizontal, 0);
        br_badge.set_size_request(30, 30);
        br_badge.set_hexpand(false);
        br_badge.set_halign(Align::Center);
        br_badge.set_valign(Align::Center);
        let br_ic = build_vector_ui_canvas(UiVectorIcon::Brightness, 20, (1.0, 0.84, 0.24, 0.98));
        br_ic.set_margin_start(5);
        br_ic.set_margin_end(5);
        br_badge.append(&br_ic);
        let hud_bright_scale = Scale::with_range(Orientation::Horizontal, 5.0, 100.0, 1.0);
        hud_bright_scale.add_css_class("neon-island-slider");
        hud_bright_scale.set_draw_value(false);
        hud_bright_scale.set_hexpand(true);
        br_row.append(&br_badge);
        br_row.append(&hud_bright_scale);

        hud_page.append(&vol_head);
        hud_page.append(&vol_row);
        hud_page.append(&br_head);
        hud_page.append(&br_row);
        drawer_stack.add_named(&hud_page, Some("hud"));

        // ---------------------------------------------------------------------
        // MODULE STORE & APPEARANCE SWITCHER (Clean 3×4 App Gallery, Zero Scroll!)
        // ---------------------------------------------------------------------
        let store_page = GtkBox::new(Orientation::Vertical, 10);
        store_page.add_css_class("neon-island-store-pad");
        store_page.set_size_request(628, 308);

        let store_head = GtkBox::new(Orientation::Horizontal, 8);
        store_head.set_valign(Align::Center);
        let store_title = Label::new(Some("Modules"));
        store_title.add_css_class("neon-island-title-lbl");
        let store_count_lbl = Label::new(Some("5/5 Docked"));
        store_count_lbl.add_css_class("neon-island-count-chip");
        let store_sp = GtkBox::new(Orientation::Horizontal, 0);
        store_sp.set_hexpand(true);
        let app_lbl = Label::new(Some("Style"));
        app_lbl.add_css_class("neon-island-muted-xs");
        let store_appearance_row = GtkBox::new(Orientation::Horizontal, 2);
        store_appearance_row.add_css_class("neon-notch-style-seg");
        store_head.append(&store_title);
        store_head.append(&store_count_lbl);
        store_head.append(&store_sp);
        store_head.append(&app_lbl);
        store_head.append(&store_appearance_row);
        store_page.append(&store_head);

        let store_grid = FlowBox::new();
        store_grid.add_css_class("neon-notch-store-grid");
        store_grid.set_max_children_per_line(3);
        store_grid.set_min_children_per_line(3);
        store_grid.set_row_spacing(8);
        store_grid.set_column_spacing(8);
        store_grid.set_homogeneous(true);
        store_grid.set_vexpand(true);
        store_grid.set_selection_mode(SelectionMode::None);
        store_page.append(&store_grid);
        drawer_stack.add_named(&store_page, Some("store"));

        // ---------------------------------------------------------------------
        // LIVE OVERLAY: INCOMING NOTIFICATION PILL (Apple iOS Call/Alert Style!)
        // ---------------------------------------------------------------------
        let notif_page = GtkBox::new(Orientation::Vertical, 8);
        notif_page.add_css_class("neon-ios-notif-page");
        notif_page.set_width_request(396);

        let notif_row = GtkBox::new(Orientation::Horizontal, 12);
        notif_row.set_valign(Align::Center);

        let notif_icon_slot = GtkBox::new(Orientation::Horizontal, 0);
        notif_icon_slot.add_css_class("neon-ios-notif-avatar");
        notif_icon_slot.set_size_request(44, 44);
        notif_icon_slot.set_halign(Align::Center);
        notif_icon_slot.set_valign(Align::Center);
        notif_icon_slot.set_overflow(gtk4::Overflow::Hidden);

        let notif_body_btn = Button::new();
        notif_body_btn.add_css_class("neon-ios-frameless-btn");
        notif_body_btn.set_hexpand(true);
        notif_body_btn.set_valign(Align::Center);
        let notif_col = GtkBox::new(Orientation::Vertical, 1);
        notif_col.set_valign(Align::Center);
        let notif_app_lbl = Label::new(Some("Notification"));
        notif_app_lbl.add_css_class("neon-ios-notif-eyebrow");
        notif_app_lbl.set_halign(Align::Start);
        notif_app_lbl.set_xalign(0.0);
        notif_app_lbl.set_max_width_chars(24);
        notif_app_lbl.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        let notif_sum_lbl = Label::new(Some("Notification Title"));
        notif_sum_lbl.add_css_class("neon-ios-notif-title");
        notif_sum_lbl.set_halign(Align::Start);
        notif_sum_lbl.set_xalign(0.0);
        notif_sum_lbl.set_max_width_chars(24);
        notif_sum_lbl.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        let notif_body_lbl = Label::new(None);
        notif_body_lbl.add_css_class("neon-ios-notif-body");
        notif_body_lbl.set_halign(Align::Start);
        notif_body_lbl.set_xalign(0.0);
        notif_body_lbl.set_max_width_chars(28);
        notif_body_lbl.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        notif_col.append(&notif_app_lbl);
        notif_col.append(&notif_sum_lbl);
        notif_col.append(&notif_body_lbl);
        notif_body_btn.set_child(Some(&notif_col));

        let notif_btns = GtkBox::new(Orientation::Horizontal, 10);
        notif_btns.set_valign(Align::Center);

        let notif_close_btn = build_notif_circle_btn(false, "Dismiss");
        let notif_open_btn = build_notif_circle_btn(true, "Open");

        notif_btns.append(&notif_close_btn);
        notif_btns.append(&notif_open_btn);

        notif_row.append(&notif_icon_slot);
        notif_row.append(&notif_body_btn);
        notif_row.append(&notif_btns);
        notif_page.append(&notif_row);

        let notif_actions_box = GtkBox::new(Orientation::Horizontal, 6);
        notif_actions_box.set_visible(false);
        notif_page.append(&notif_actions_box);
        drawer_stack.add_named(&notif_page, Some("notif"));

        // ---------------------------------------------------------------------
        // LIVE OVERLAY: ACTIVE SCREEN RECORDING
        // ---------------------------------------------------------------------
        let rec_page = GtkBox::new(Orientation::Vertical, 10);
        rec_page.add_css_class("neon-island-page-pad");
        rec_page.set_width_request(380);

        let rec_top = GtkBox::new(Orientation::Horizontal, 10);
        rec_top.set_valign(Align::Center);
        let rec_emblem = GtkBox::new(Orientation::Horizontal, 0);
        rec_emblem.add_css_class("neon-island-rec-emblem");
        rec_emblem.set_size_request(36, 36);
        rec_emblem.set_halign(Align::Center);
        rec_emblem.set_valign(Align::Center);
        let rec_emblem_dot = GtkBox::new(Orientation::Horizontal, 0);
        rec_emblem_dot.add_css_class("neon-island-rec-dot");
        rec_emblem_dot.set_halign(Align::Center);
        rec_emblem_dot.set_valign(Align::Center);
        rec_emblem.append(&rec_emblem_dot);

        let rec_meta = GtkBox::new(Orientation::Vertical, 1);
        rec_meta.set_hexpand(true);
        let exp_rec_title = Label::new(Some("Screen Recording"));
        exp_rec_title.add_css_class("neon-island-title-lbl");
        exp_rec_title.set_halign(Align::Start);
        let exp_rec_time_lbl = Label::new(Some("00:00 elapsed"));
        exp_rec_time_lbl.add_css_class("neon-island-sub-lbl");
        exp_rec_time_lbl.set_halign(Align::Start);
        rec_meta.append(&exp_rec_title);
        rec_meta.append(&exp_rec_time_lbl);

        let stop_rec_btn = Button::with_label("Stop");
        stop_rec_btn.add_css_class("neon-island-stop-btn");
        rec_top.append(&rec_emblem);
        rec_top.append(&rec_meta);
        rec_top.append(&stop_rec_btn);
        rec_page.append(&rec_top);

        let exp_rec_media_strip = GtkBox::new(Orientation::Horizontal, 8);
        exp_rec_media_strip.add_css_class("neon-island-mini-strip");
        exp_rec_media_strip.set_visible(false);
        let rec_m_icon = build_module_icon_canvas(NotchModule::Media, 16, (0.92, 0.92, 0.95, 0.95));
        let exp_rec_media_lbl = Label::new(Some(""));
        exp_rec_media_lbl.add_css_class("neon-island-sub-lbl");
        exp_rec_media_lbl.set_hexpand(true);
        exp_rec_media_lbl.set_halign(Align::Start);
        exp_rec_media_lbl.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        let exp_rec_play_btn = Button::new();
        exp_rec_play_btn.add_css_class("neon-island-icon-pill");
        let exp_rec_play_canvas = DrawingArea::new();
        exp_rec_play_canvas.set_size_request(16, 16);
        exp_rec_play_canvas.set_halign(Align::Center);
        exp_rec_play_canvas.set_valign(Align::Center);
        exp_rec_play_canvas.set_can_target(false);
        exp_rec_play_btn.set_child(Some(&exp_rec_play_canvas));
        exp_rec_media_strip.append(&rec_m_icon);
        exp_rec_media_strip.append(&exp_rec_media_lbl);
        exp_rec_media_strip.append(&exp_rec_play_btn);
        rec_page.append(&exp_rec_media_strip);
        drawer_stack.add_named(&rec_page, Some("recording"));

        expanded_clip.set_child(Some(&drawer_stack));
        shell_box.add_overlay(&expanded_clip);

        // =====================================================================
        // 3. Signature Floating Bottom Dock Switcher Pill (Gaussian Cairo Shadow!)
        // =====================================================================
        let dock_pill = GtkOverlay::new();
        dock_pill.add_css_class("neon-notch-dock-pill");
        dock_pill.set_halign(Align::Center);
        dock_pill.set_valign(Align::Start);
        dock_pill.set_size_request(-1, DOCK_H);
        dock_pill.set_visible(false);
        dock_pill.set_opacity(0.0);

        let dock_bg_canvas = DrawingArea::new();
        dock_bg_canvas.set_hexpand(true);
        dock_bg_canvas.set_vexpand(true);
        dock_bg_canvas.set_can_target(false);
        dock_pill.set_child(Some(&dock_bg_canvas));

        let dock_inner = GtkBox::new(Orientation::Horizontal, 5);
        dock_inner.set_valign(Align::Center);
        dock_inner.set_margin_start(19);
        dock_inner.set_margin_end(19);
        dock_inner.set_margin_top(4);
        dock_inner.set_margin_bottom(14);

        let dock_items_box = GtkBox::new(Orientation::Horizontal, 3);
        dock_items_box.set_valign(Align::Center);
        let dock_div = GtkBox::new(Orientation::Vertical, 0);
        dock_div.add_css_class("neon-notch-dock-sep");
        dock_div.set_valign(Align::Center);
        let dock_store_btn = Button::new();
        dock_store_btn.add_css_class("neon-notch-dock-btn");
        dock_store_btn.set_tooltip_text(Some("All Modules & Style"));
        let dock_store_canvas = DrawingArea::new();
        dock_store_canvas.set_size_request(20, 20);
        dock_store_canvas.set_halign(Align::Center);
        dock_store_canvas.set_valign(Align::Center);
        dock_store_canvas.set_can_target(false);
        dock_store_btn.set_child(Some(&dock_store_canvas));

        dock_inner.append(&dock_items_box);
        dock_inner.append(&dock_div);
        dock_inner.append(&dock_store_btn);
        dock_pill.add_overlay(&dock_inner);
        dock_pill.set_measure_overlay(&dock_inner, true);

        hover_column.append(&shell_box);
        hover_column.append(&dock_pill);
        stage.add_overlay(&hover_column);
        window.set_child(Some(&stage));
        window.set_visible(false);

        let state = Rc::new(RefCell::new(IslandState::new()));
        let telemetry_sampler = Rc::new(RefCell::new(TelemetrySampler::new()));
        let keep_awake = Arc::new(KeepAwakeInhibitor::new());

        let island = Self {
            window,
            backdrop_window,
            bg_canvas,
            hover_column,
            shell_box,
            dock_pill,
            dock_bg_canvas,
            dock_items_box,
            dock_store_btn,
            dock_store_canvas,
            compact_layer,
            pill_charge_group,
            pill_charge_pct,
            pill_charge_bat,
            pill_silent_group,
            pill_silent_badge,
            pill_silent_lbl,
            pill_hud_group,
            pill_hud_icon,
            pill_hud_label,
            pill_hud_bar,
            pill_hud_pct,
            pill_rec_group,
            pill_rec_label,
            pill_timer_group,
            pill_timer_ring,
            pill_timer_label,
            pill_media_group,
            pill_media_pic,
            pill_media_fallback,
            pill_media_label,
            pill_wave_area,
            pill_idle_group,
            pill_idle_dot,
            pill_idle_clock,
            pill_idle_badge,
            expanded_clip,
            drawer_stack,
            notif_page,
            rec_page,
            home_weekday,
            home_clock,
            home_date,
            home_live_box,
            home_dnd_btn,
            home_dnd_lbl,
            exp_art_pic,
            exp_art_fallback,
            exp_media_title,
            exp_media_sub,
            exp_wave_area,
            exp_media_elapsed,
            exp_media_remaining,
            media_progress_area,
            exp_play_canvas,
            timer_focus_btn,
            timer_break_btn,
            timer_ruler_area,
            timer_start_btn,
            timer_sound_btn,
            timer_sound_canvas,
            timer_readout_lbl,
            clip_filter_row,
            clip_cards_box,
            clip_empty_lbl,
            clip_clear_btn,
            shelf_tab_btn,
            shelf_recent_btn,
            shelf_strip_box,
            shelf_empty_lbl,
            shelf_action_bar,
            shelf_status_lbl,
            notes_list_box,
            notes_editor,
            notes_count_lbl,
            cal_month_lbl,
            cal_days_strip,
            cal_agenda_box,
            tools_calc_entry,
            tools_calc_result_btn,
            tools_calc_result_lbl,
            stats_cpu_lbl,
            stats_cpu_sub,
            stats_cpu_canvas,
            stats_mem_lbl,
            stats_mem_sub,
            stats_mem_bar,
            stats_mem_fill,
            stats_net_lbl,
            stats_net_sub,
            stats_net_canvas,
            stats_disk_lbl,
            stats_disk_bar,
            stats_disk_fill,
            bat_ring_canvas,
            bat_status_lbl,
            bat_sub_lbl,
            bat_profile_row,
            bat_devices_box,
            awake_readout_lbl,
            awake_sub_lbl,
            awake_toggle_btn,
            awake_presets_row,
            hud_vol_scale,
            hud_vol_pct,
            hud_vol_sink,
            hud_mute_btn,
            hud_mute_canvas,
            hud_bright_scale,
            hud_bright_pct,
            store_grid,
            store_count_lbl,
            store_appearance_row,
            notif_icon_slot,
            notif_app_lbl,
            notif_sum_lbl,
            notif_body_lbl,
            notif_actions_box,
            exp_rec_time_lbl,
            exp_rec_media_strip,
            exp_rec_media_lbl,
            exp_rec_play_canvas,
            state,
            telemetry_sampler,
            keep_awake,
            windows,
            store,
            clipboard_store,
            sys_tx,
            ipc,
            on_stop_recording: Rc::new(RefCell::new(None)),
            on_open_notifications: Rc::new(RefCell::new(None)),
            on_open_screenshot: Rc::new(RefCell::new(None)),
            motion_enabled: Rc::new(Cell::new(true)),
            speed_scale: Rc::new(Cell::new(1.0)),
            suppress_hud_sliders: Rc::new(Cell::new(false)),
        };

        // =====================================================================
        // Cairo Draw Callbacks (All Antialias::Best)
        // =====================================================================
        {
            let st_rc = Rc::clone(&island.state);
            island.bg_canvas.set_draw_func(move |_, cr, w, _h| {
                let st = st_rc.borrow();
                if st.is_stealth_sensor() {
                    if !st.shelf_items.is_empty() {
                        cr.set_antialias(cairo::Antialias::Best);
                        let bar_w = 72.0;
                        let bx = ((f64::from(w) - bar_w) * 0.5).round();
                        crate::island_visuals::rounded_rect_path(cr, bx, 0.0, bar_w, 3.0, 1.5);
                        cr.set_source_rgba(st.accent.r, st.accent.g, st.accent.b, 0.78);
                        let _ = cr.fill();
                    }
                    return;
                }
                let close_fade = if st.wants_stealth_sensor() {
                    smoothstep((st.morph.alpha.value / 0.55).clamp(0.0, 1.0))
                } else {
                    1.0
                };
                if close_fade <= 0.004 {
                    return;
                }
                if close_fade < 0.996 {
                    cr.push_group();
                }
                draw_notch_backdrop(
                    cr,
                    f64::from(w),
                    f64::from(SHELL_TOP_OFFSET),
                    st.morph.w.value,
                    st.morph.h.value,
                    st.morph.br.value,
                    st.morph.er.value,
                    st.prefs.appearance,
                    st.aura_kind,
                    st.morph.alpha.value.clamp(0.0, 1.0),
                    st.cur_palette,
                    st.neon_intensity,
                    st.neon_phase,
                    st.accent,
                    st.drag_hover || st.desktop_drag_active,
                );
                if close_fade < 0.996 {
                    let _ = cr.pop_group_to_source();
                    let _ = cr.paint_with_alpha(close_fade);
                }
            });
        }
        {
            let st_rc = Rc::clone(&island.state);
            island.dock_bg_canvas.set_draw_func(move |_, cr, w, h| {
                let appearance = st_rc.borrow().prefs.appearance;
                draw_dock_pill_backdrop(cr, f64::from(w), f64::from(h), appearance);
            });
        }
        {
            let store_hover = Rc::new(Cell::new(false));
            let sh_in = Rc::clone(&store_hover);
            let sc_in = island.dock_store_canvas.clone();
            let sh_out = Rc::clone(&store_hover);
            let sc_out = island.dock_store_canvas.clone();
            let mot = EventControllerMotion::new();
            mot.connect_enter(move |_, _, _| {
                sh_in.set(true);
                sc_in.queue_draw();
            });
            mot.connect_leave(move |_| {
                sh_out.set(false);
                sc_out.queue_draw();
            });
            island.dock_store_btn.add_controller(mot);

            let st_rc = Rc::clone(&island.state);
            island.dock_store_canvas.set_draw_func(move |_, cr, w, h| {
                let active = st_rc.borrow().active_module == NotchModule::Store;
                let rgba = if active || store_hover.get() {
                    (1.0, 1.0, 1.0, 0.98)
                } else {
                    (0.62, 0.62, 0.66, 0.95)
                };
                draw_module_vector_icon(cr, f64::from(w), f64::from(h), NotchModule::Store, rgba);
            });
        }
        {
            let st_rc = Rc::clone(&island.state);
            island.pill_hud_icon.set_draw_func(move |_, cr, w, h| {
                let st = st_rc.borrow();
                let (is_br, muted) = match st.hud_popup.as_ref().map(|p| &p.kind) {
                    Some(LivePillKind::Hud {
                        is_brightness,
                        muted,
                        fraction,
                        ..
                    }) => (*is_brightness, *muted || *fraction <= 0.01),
                    _ => (false, st.sys.audio.muted),
                };
                if is_br {
                    draw_ui_vector_icon(
                        cr,
                        f64::from(w),
                        f64::from(h),
                        UiVectorIcon::Brightness,
                        (1.0, 0.84, 0.24, 0.98),
                    );
                } else {
                    draw_ui_vector_icon(
                        cr,
                        f64::from(w),
                        f64::from(h),
                        UiVectorIcon::Volume { muted },
                        (1.0, 1.0, 1.0, 0.98),
                    );
                }
            });
        }
        {
            let st_rc = Rc::clone(&island.state);
            island.timer_sound_canvas.set_draw_func(move |_, cr, w, h| {
                let sound_on = st_rc.borrow().timer_sound;
                let rgba = if sound_on {
                    (1.0, 1.0, 1.0, 0.96)
                } else {
                    (0.62, 0.62, 0.66, 0.90)
                };
                draw_ui_vector_icon(
                    cr,
                    f64::from(w),
                    f64::from(h),
                    UiVectorIcon::Volume { muted: !sound_on },
                    rgba,
                );
            });
        }
        {
            let st_rc = Rc::clone(&island.state);
            island.hud_mute_canvas.set_draw_func(move |_, cr, w, h| {
                let muted = st_rc.borrow().sys.audio.muted;
                let rgba = if muted {
                    (1.0, 0.27, 0.23, 0.98)
                } else {
                    (1.0, 1.0, 1.0, 0.96)
                };
                draw_ui_vector_icon(
                    cr,
                    f64::from(w),
                    f64::from(h),
                    UiVectorIcon::Volume { muted },
                    rgba,
                );
            });
        }
        {
            let st_rc = Rc::clone(&island.state);
            island.exp_rec_play_canvas.set_draw_func(move |_, cr, w, h| {
                let playing = st_rc.borrow().media.playing;
                let _ = cr.save();
                cr.scale(f64::from(w) / 28.0, f64::from(h) / 28.0);
                draw_sf_play_pause_icon(cr, 28.0, 28.0, playing);
                let _ = cr.restore();
            });
        }
        {
            let st_rc = Rc::clone(&island.state);
            island.pill_charge_bat.set_draw_func(move |_, cr, w, h| {
                let st = st_rc.borrow();
                let (pct, charging) = match st.hud_popup.as_ref().map(|p| &p.kind) {
                    Some(LivePillKind::Charging { pct, charging }) => (*pct, *charging),
                    _ => (
                        st.sys.power.battery_percentage,
                        !st.sys.power.on_battery
                            || st.sys.power.battery_state == BatteryState::Charging,
                    ),
                };
                draw_apple_battery_glyph(cr, f64::from(w), f64::from(h), pct, charging);
            });
        }
        {
            let st_rc = Rc::clone(&island.state);
            island.pill_silent_badge.set_draw_func(move |_, cr, w, h| {
                let st = st_rc.borrow();
                let silent = match st.hud_popup.as_ref().map(|p| &p.kind) {
                    Some(LivePillKind::Silent { silent, .. }) => *silent,
                    _ => st.dnd || st.sys.audio.muted,
                };
                draw_apple_silent_pill(cr, f64::from(w), f64::from(h), silent);
            });
        }
        {
            let st_rc = Rc::clone(&island.state);
            island.pill_media_fallback.set_draw_func(move |_, cr, w, h| {
                let st = st_rc.borrow();
                draw_apple_fallback_art(cr, f64::from(w), f64::from(h), st.cur_palette);
            });
        }
        {
            let st_rc = Rc::clone(&island.state);
            island.exp_art_fallback.set_draw_func(move |_, cr, w, h| {
                let st = st_rc.borrow();
                draw_apple_fallback_art(cr, f64::from(w), f64::from(h), st.cur_palette);
            });
        }
        prev_canvas.set_draw_func(move |_, cr, w, h| {
            draw_sf_skip_icon(cr, f64::from(w), f64::from(h), false);
        });
        {
            let st_rc = Rc::clone(&island.state);
            island.exp_play_canvas.set_draw_func(move |_, cr, w, h| {
                let playing = st_rc.borrow().media.playing;
                draw_sf_play_pause_icon(cr, f64::from(w), f64::from(h), playing);
            });
        }
        next_canvas.set_draw_func(move |_, cr, w, h| {
            draw_sf_skip_icon(cr, f64::from(w), f64::from(h), true);
        });
        airplay_canvas.set_draw_func(move |_, cr, w, h| {
            draw_sf_airplay_icon(cr, f64::from(w), f64::from(h));
        });
        {
            let st_rc = Rc::clone(&island.state);
            island.pill_wave_area.set_draw_func(move |_, cr, w, h| {
                let st = st_rc.borrow();
                draw_audio_waveform(
                    cr,
                    f64::from(w),
                    f64::from(h),
                    5,
                    st.media.playing,
                    st.neon_phase,
                    st.cur_palette,
                );
            });
        }
        {
            let st_rc = Rc::clone(&island.state);
            island.exp_wave_area.set_draw_func(move |_, cr, w, h| {
                let st = st_rc.borrow();
                draw_audio_waveform(
                    cr,
                    f64::from(w),
                    f64::from(h),
                    6,
                    st.media.playing,
                    st.neon_phase,
                    st.cur_palette,
                );
            });
        }
        {
            let st_rc = Rc::clone(&island.state);
            island.pill_timer_ring.set_draw_func(move |_, cr, w, h| {
                let st = st_rc.borrow();
                let frac = if st.timer_total_secs > 0 {
                    st.timer_remaining_secs as f64 / st.timer_total_secs as f64
                } else {
                    1.0
                };
                draw_timer_ring(cr, f64::from(w), f64::from(h), frac, st.timer_focus_mode);
            });
        }
        {
            let st_rc = Rc::clone(&island.state);
            island.pill_hud_bar.set_draw_func(move |_, cr, w, h| {
                let st = st_rc.borrow();
                if let Some(LivePillPopup {
                    kind:
                        LivePillKind::Hud {
                            is_brightness,
                            fraction,
                            muted,
                            ..
                        },
                    ..
                }) = st.hud_popup
                {
                    draw_hud_bar(
                        cr,
                        f64::from(w),
                        f64::from(h),
                        fraction,
                        is_brightness,
                        muted,
                    );
                }
            });
        }
        {
            let st_rc = Rc::clone(&island.state);
            island.media_progress_area.set_draw_func(move |_, cr, w, h| {
                let st = st_rc.borrow();
                let frac = if st.media_dragging {
                    st.media_drag_frac
                } else {
                    compute_media_progress_fraction(&st.media, st.media_seek_override)
                };
                draw_media_progress_bar(
                    cr,
                    f64::from(w),
                    f64::from(h),
                    frac,
                    st.media_progress_hovered || st.media_dragging,
                    st.cur_palette,
                );
            });
        }
        {
            let st_rc = Rc::clone(&island.state);
            island.timer_ruler_area.set_draw_func(move |_, cr, w, h| {
                let st = st_rc.borrow();
                let mins = if st.timer_running {
                    ((st.timer_remaining_secs + 59) / 60).max(1) as u32
                } else {
                    st.timer_set_mins
                };
                draw_timer_ruler(
                    cr,
                    f64::from(w),
                    f64::from(h),
                    mins,
                    st.timer_focus_mode,
                    st.timer_ruler_hovered || st.timer_dragging,
                );
            });
        }
        {
            let st_rc = Rc::clone(&island.state);
            island.stats_cpu_canvas.set_draw_func(move |_, cr, w, h| {
                let st = st_rc.borrow();
                draw_cpu_bars(cr, f64::from(w), f64::from(h), &st.cpu_history);
            });
        }
        {
            let st_rc = Rc::clone(&island.state);
            island.stats_net_canvas.set_draw_func(move |_, cr, w, h| {
                let st = st_rc.borrow();
                draw_network_sparkline(
                    cr,
                    f64::from(w),
                    f64::from(h),
                    &st.net_rx_history,
                    &st.net_tx_history,
                );
            });
        }
        {
            let st_rc = Rc::clone(&island.state);
            island.bat_ring_canvas.set_draw_func(move |_, cr, w, h| {
                let st = st_rc.borrow();
                let pct = if st.sys.power.has_battery {
                    st.sys.power.battery_percentage
                } else {
                    100.0
                };
                let charging = !st.sys.power.on_battery
                    || st.sys.power.battery_state == BatteryState::Charging;
                draw_battery_ring(cr, f64::from(w), f64::from(h), pct, charging);
            });
        }

        // =====================================================================
        // Controllers: Input Region, Hover, Click-Outside, Keyboard, Drag-Drop
        // =====================================================================
        {
            let di = island.clone();
            island.window.connect_map(move |_| {
                let (stealth, outgoing, top_off, w, h, alpha) = {
                    let st = di.state.borrow();
                    let top_off = if st.prefs.appearance.is_top_flush_notch() {
                        0
                    } else {
                        SHELL_TOP_OFFSET
                    };
                    (
                        st.is_stealth_sensor(),
                        st.outgoing_drag,
                        top_off,
                        st.morph.w.value,
                        st.morph.h.value,
                        st.morph.alpha.value,
                    )
                };
                if outgoing {
                    apply_passthrough_input_region(&di.window);
                } else if stealth {
                    apply_sensor_input_region(&di.window);
                } else {
                    apply_input_region(&di.window, top_off, w, h, alpha > 0.25);
                }
            });
        }

        // Click outside backdrop closes pinned notch cleanly.
        {
            let click = GestureClick::new();
            let di = island.clone();
            click.connect_pressed(move |_, _, _, _| {
                di.close_pinned();
            });
            backdrop_box.add_controller(click);
        }

        // Keyboard shortcuts on main notch window (`Esc`, `Alt+1..5`, `Alt+1..9` clipboard).
        {
            let key_ctrl = EventControllerKey::new();
            let di = island.clone();
            key_ctrl.connect_key_pressed(move |_, key, _, mods| di.handle_key_pressed(key, mods));
            island.window.add_controller(key_ctrl);
        }

        // Unified Hover Controller covering Island Body + 8px gap + Floating Bottom Dock Pill!
        {
            let motion = EventControllerMotion::new();
            let di_in = island.clone();
            motion.connect_enter(move |_, _, _| di_in.on_hover_enter());
            let di_out = island.clone();
            motion.connect_leave(move |_| di_out.on_hover_leave());
            island.hover_column.add_controller(motion);
        }

        // OnePlus File Dock Seamless Drag-In (`DropTarget`): accepts files, browser PNGs & URI lists!
        {
            let drop_target = DropTarget::new(glib::Type::INVALID, gdk::DragAction::COPY);
            drop_target.set_types(&[
                gdk::FileList::static_type(),
                gdk::Texture::static_type(),
                glib::Type::STRING,
            ]);
            let di_e = island.clone();
            drop_target.connect_enter(move |_, _, _| {
                if di_e.state.borrow().outgoing_drag {
                    return gdk::DragAction::empty();
                }
                {
                    let mut st = di_e.state.borrow_mut();
                    st.drag_hover = true;
                    st.peek_over_obscured = true;
                    st.active_module = NotchModule::Files;
                    st.shelf_show_recent = false;
                    st.hovered = true;
                    st.hover_gen = st.hover_gen.wrapping_add(1);
                }
                di_e.refresh_file_shelf_ui();
                di_e.rebuild_dock_bar();
                di_e.sync_ui();
                gdk::DragAction::COPY
            });
            let di_m = island.clone();
            drop_target.connect_motion(move |_, _, _| {
                if di_m.state.borrow().outgoing_drag {
                    gdk::DragAction::empty()
                } else {
                    gdk::DragAction::COPY
                }
            });
            let di_l = island.clone();
            drop_target.connect_leave(move |_| {
                let epoch = {
                    let mut st = di_l.state.borrow_mut();
                    st.drag_hover = false;
                    st.hover_gen = st.hover_gen.wrapping_add(1);
                    st.hover_gen
                };
                di_l.bg_canvas.queue_draw();
                let di_collapse = di_l.clone();
                glib::timeout_add_local_once(Duration::from_millis(420), move || {
                    let should_collapse = {
                        let mut st = di_collapse.state.borrow_mut();
                        if st.hover_gen == epoch
                            && !st.pinned_open
                            && !st.drag_hover
                            && !st.test_force_expand
                        {
                            st.hovered = false;
                            if !st.desktop_drag_active {
                                st.peek_over_obscured = false;
                            }
                            true
                        } else {
                            false
                        }
                    };
                    if should_collapse {
                        di_collapse.sync_ui();
                    }
                });
            });
            let di_d = island.clone();
            drop_target.connect_drop(move |_, value, _, _| {
                if di_d.state.borrow().outgoing_drag {
                    return false;
                }
                {
                    let mut st = di_d.state.borrow_mut();
                    st.drag_hover = false;
                    st.desktop_drag_active = false;
                }
                let mut added = 0usize;
                if let Ok(file_list) = value.get::<gdk::FileList>() {
                    for f in file_list.files() {
                        if let Some(p) = f.path()
                            && di_d.add_file_to_shelf(p)
                        {
                            added += 1;
                        }
                    }
                } else if let Ok(texture) = value.get::<gdk::Texture>() {
                    if let Some(dest) = next_dropped_image_path()
                        && texture.save_to_png(&dest).is_ok()
                        && di_d.add_file_to_shelf(dest)
                    {
                        added += 1;
                    }
                } else if let Ok(raw_str) = value.get::<String>() {
                    for p in parse_uri_list_paths(&raw_str) {
                        if di_d.add_file_to_shelf(p) {
                            added += 1;
                        }
                    }
                }
                if added > 0 {
                    {
                        let mut st = di_d.state.borrow_mut();
                        st.shelf_show_recent = false;
                        st.active_module = NotchModule::Files;
                        st.hovered = true;
                        st.peek_over_obscured = true;
                        st.shelf_status_msg = Some(format!(
                            "Docked {} file{} — drag out to any app",
                            added,
                            if added == 1 { "" } else { "s" }
                        ));
                    }
                    di_d.rebuild_dock_bar();
                    di_d.refresh_file_shelf_ui();
                    di_d.sync_ui();
                    return true;
                }
                di_d.sync_ui();
                false
            });
            island.hover_column.add_controller(drop_target);
        }

        // Clicking compact notch pins it open (matching Little Notch "hover to peek, click to open").
        {
            let di = island.clone();
            pill_btn.connect_clicked(move |_| {
                let target_mod = {
                    let st = di.state.borrow();
                    if st.timer_running {
                        NotchModule::Timer
                    } else if st.media.available
                        && (st.media.playing || st.media.title.as_deref().is_some_and(|t| !t.is_empty()))
                    {
                        NotchModule::Media
                    } else {
                        NotchModule::Home
                    }
                };
                di.open_pinned_module(target_mod);
            });
        }

        // Floating Dock `⊞` Store toggle button
        {
            let di = island.clone();
            island.dock_store_btn.clone().connect_clicked(move |_| {
                let next = if di.state.borrow().active_module == NotchModule::Store {
                    NotchModule::Home
                } else {
                    NotchModule::Store
                };
                di.switch_module(next, true);
            });
        }

        // Home quick buttons
        {
            let di = island.clone();
            home_nc_btn.connect_clicked(move |_| {
                di.close_pinned();
                if let Some(cb) = di.on_open_notifications.borrow().as_ref() {
                    cb();
                }
            });
        }
        {
            let di = island.clone();
            island.home_dnd_btn.clone().connect_clicked(move |_| {
                let next = !di.store.is_do_not_disturb();
                di.store.set_do_not_disturb(next);
                di.state.borrow_mut().dnd = next;
                di.sync_ui();
            });
        }

        // Media transport & scrubber
        {
            let motion = EventControllerMotion::new();
            let di_in = island.clone();
            motion.connect_enter(move |_, _, _| {
                di_in.state.borrow_mut().media_progress_hovered = true;
                di_in.media_progress_area.queue_draw();
            });
            let di_out = island.clone();
            motion.connect_leave(move |_| {
                di_out.state.borrow_mut().media_progress_hovered = false;
                di_out.media_progress_area.queue_draw();
            });
            island.media_progress_area.add_controller(motion);

            let click = GestureClick::new();
            let di_click = island.clone();
            click.connect_released(move |_, _, x, _| {
                let w = f64::from(di_click.media_progress_area.width().max(1));
                let frac = ((x - 5.0) / (w - 10.0).max(1.0)).clamp(0.0, 1.0);
                di_click.seek_media_to_fraction(frac);
            });
            island.media_progress_area.add_controller(click);

            let drag = GestureDrag::new();
            let start_x = Rc::new(Cell::new(0.0_f64));
            let sx_b = Rc::clone(&start_x);
            let di_db = island.clone();
            drag.connect_drag_begin(move |_, x, _| {
                sx_b.set(x);
                let w = f64::from(di_db.media_progress_area.width().max(1));
                let frac = ((x - 5.0) / (w - 10.0).max(1.0)).clamp(0.0, 1.0);
                {
                    let mut st = di_db.state.borrow_mut();
                    st.media_dragging = true;
                    st.media_drag_frac = frac;
                }
                di_db.refresh_media_time_labels();
                di_db.media_progress_area.queue_draw();
            });
            let sx_u = Rc::clone(&start_x);
            let di_du = island.clone();
            drag.connect_drag_update(move |_, off_x, _| {
                let x = sx_u.get() + off_x;
                let w = f64::from(di_du.media_progress_area.width().max(1));
                let frac = ((x - 5.0) / (w - 10.0).max(1.0)).clamp(0.0, 1.0);
                di_du.state.borrow_mut().media_drag_frac = frac;
                di_du.refresh_media_time_labels();
                di_du.media_progress_area.queue_draw();
            });
            let di_de = island.clone();
            drag.connect_drag_end(move |_, _, _| {
                let frac = {
                    let mut st = di_de.state.borrow_mut();
                    st.media_dragging = false;
                    st.media_drag_frac
                };
                di_de.seek_media_to_fraction(frac);
            });
            island.media_progress_area.add_controller(drag);
        }
        {
            let di = island.clone();
            prev_btn.connect_clicked(move |_| di.dispatch_media_cmd("Previous"));
        }
        {
            let di = island.clone();
            play_btn.connect_clicked(move |_| di.dispatch_media_cmd("PlayPause"));
        }
        {
            let di = island.clone();
            next_btn.connect_clicked(move |_| di.dispatch_media_cmd("Next"));
        }
        {
            let di = island.clone();
            media_audio_btn.connect_clicked(move |_| di.switch_module(NotchModule::Hud, true));
        }
        {
            let di = island.clone();
            media_focus_btn.connect_clicked(move |_| {
                let player = di.state.borrow().media.player_name.clone();
                if let Some(pname) = player {
                    let fake = DesktopNotification {
                        id: 0,
                        app_name: pname.clone(),
                        app_icon: pname.to_ascii_lowercase(),
                        summary: String::new(),
                        body: String::new(),
                        urgency: NotificationUrgency::Normal,
                        expire_timeout_ms: 0,
                        received_unix_secs: 0,
                        dismissed: false,
                        actions: Vec::new(),
                        desktop_entry: Some(pname.to_ascii_lowercase()),
                        resident: false,
                    };
                    if let Some(wid) =
                        crate::notifications::match_notification_window(&fake, &di.windows.borrow())
                    {
                        di.ipc.send(IpcRequest::SetOverview { open: false });
                        di.ipc.send(IpcRequest::FocusWindow { window_id: wid });
                    }
                }
            });
        }

        // Scrubbable Ruler Timer controls (click, drag, scroll wheel, Focus/Break, Start/Pause/Reset)
        {
            let di = island.clone();
            island.timer_focus_btn.clone().connect_clicked(move |_| {
                di.set_timer_mode(true);
            });
            let di = island.clone();
            island.timer_break_btn.clone().connect_clicked(move |_| {
                di.set_timer_mode(false);
            });
            let di = island.clone();
            island.timer_start_btn.clone().connect_clicked(move |_| {
                di.toggle_timer_running();
            });
            let di = island.clone();
            island.timer_sound_btn.clone().connect_clicked(move |_| {
                let on = {
                    let mut st = di.state.borrow_mut();
                    st.timer_sound = !st.timer_sound;
                    st.timer_sound
                };
                if on {
                    di.timer_sound_btn.add_css_class("active");
                } else {
                    di.timer_sound_btn.remove_css_class("active");
                }
                di.timer_sound_canvas.queue_draw();
            });
            let di = island.clone();
            timer_reset_btn.connect_clicked(move |_| {
                di.reset_timer();
            });

            let r_motion = EventControllerMotion::new();
            let di_in = island.clone();
            r_motion.connect_enter(move |_, _, _| {
                di_in.state.borrow_mut().timer_ruler_hovered = true;
                di_in.timer_ruler_area.queue_draw();
            });
            let di_out = island.clone();
            r_motion.connect_leave(move |_| {
                di_out.state.borrow_mut().timer_ruler_hovered = false;
                di_out.timer_ruler_area.queue_draw();
            });
            island.timer_ruler_area.add_controller(r_motion);

            let r_click = GestureClick::new();
            let di_rc = island.clone();
            r_click.connect_pressed(move |_, _, x, _| {
                di_rc.scrub_timer_ruler_at(x);
            });
            island.timer_ruler_area.add_controller(r_click);

            let r_drag = GestureDrag::new();
            let sx = Rc::new(Cell::new(0.0_f64));
            let sx_b = Rc::clone(&sx);
            let di_b = island.clone();
            r_drag.connect_drag_begin(move |_, x, _| {
                sx_b.set(x);
                di_b.state.borrow_mut().timer_dragging = true;
                di_b.scrub_timer_ruler_at(x);
            });
            let sx_u = Rc::clone(&sx);
            let di_u = island.clone();
            r_drag.connect_drag_update(move |_, ox, _| {
                di_u.scrub_timer_ruler_at(sx_u.get() + ox);
            });
            let di_e = island.clone();
            r_drag.connect_drag_end(move |_, _, _| {
                di_e.state.borrow_mut().timer_dragging = false;
                di_e.timer_ruler_area.queue_draw();
            });
            island.timer_ruler_area.add_controller(r_drag);

            let r_scroll = EventControllerScroll::new(
                EventControllerScrollFlags::BOTH_AXES | EventControllerScrollFlags::DISCRETE,
            );
            let di_sc = island.clone();
            r_scroll.connect_scroll(move |_, dx, dy| {
                let delta = if dx.abs() > dy.abs() { dx } else { -dy };
                let step = if delta > 0.0 { 1i32 } else { -1i32 };
                di_sc.nudge_timer_minutes(step);
                glib::Propagation::Stop
            });
            island.timer_ruler_area.add_controller(r_scroll);
        }

        // Clipboard search & clear controls
        {
            let di = island.clone();
            clip_search.connect_search_changed(move |entry| {
                di.state.borrow_mut().clip_query = entry.text().to_string();
                di.refresh_clipboard_ui();
            });
            let click_pin = GestureClick::new();
            let di_p = island.clone();
            click_pin.connect_pressed(move |_, _, _, _| {
                di_p.promote_to_pinned();
            });
            clip_search.add_controller(click_pin);

            let di = island.clone();
            island.clip_clear_btn.clone().connect_clicked(move |_| {
                if di.clipboard_store.clear_unpinned_or_all() {
                    di.refresh_clipboard_ui();
                }
            });
        }

        // File Shelf buttons
        {
            let di = island.clone();
            island.shelf_tab_btn.clone().connect_clicked(move |_| {
                di.state.borrow_mut().shelf_show_recent = false;
                di.refresh_file_shelf_ui();
            });
            let di = island.clone();
            island.shelf_recent_btn.clone().connect_clicked(move |_| {
                di.state.borrow_mut().shelf_show_recent = true;
                di.refresh_file_shelf_ui();
            });
            let di = island.clone();
            shelf_clear_btn.connect_clicked(move |_| {
                {
                    let mut st = di.state.borrow_mut();
                    st.shelf_items.clear();
                    st.shelf_selected_path = None;
                    st.shelf_status_msg = Some("File Dock cleared".into());
                    save_persisted_shelf_items(&st.shelf_items);
                }
                di.refresh_file_shelf_ui();
                di.bg_canvas.queue_draw();
            });
            let di = island.clone();
            shelf_add_btn.connect_clicked(move |_| {
                di.open_shelf_file_picker();
            });
            let di = island.clone();
            shelf_zip_btn.connect_clicked(move |_| {
                di.toggle_zip_selected_shelf_file();
            });
            let di = island.clone();
            shelf_copy_btn.connect_clicked(move |_| {
                if let Some(p) = di.state.borrow().shelf_selected_path.clone() {
                    let s = p.to_string_lossy().into_owned();
                    di.copy_plain_text_to_clipboard(&s);
                    di.state.borrow_mut().shelf_status_msg = Some("Copied file path".into());
                    di.refresh_file_shelf_ui();
                }
            });
            let di = island.clone();
            shelf_open_btn.connect_clicked(move |_| {
                if let Some(p) = di.state.borrow().shelf_selected_path.clone() {
                    let target = p.parent().unwrap_or(&p).to_path_buf();
                    let _ = std::process::Command::new("xdg-open").arg(target).spawn();
                    di.close_pinned();
                }
            });
        }

        // Quick Notes controls
        {
            let di = island.clone();
            notes_new_btn.connect_clicked(move |_| {
                let id = di.state.borrow_mut().notes_store.create();
                di.state.borrow_mut().active_note_id = id;
                di.refresh_notes_ui();
                di.promote_to_pinned();
                di.notes_editor.grab_focus();
            });
            let di = island.clone();
            note_del_btn.connect_clicked(move |_| {
                let next_id = {
                    let mut st = di.state.borrow_mut();
                    let cur = st.active_note_id;
                    st.notes_store.delete(cur).unwrap_or(1)
                };
                di.state.borrow_mut().active_note_id = next_id;
                di.refresh_notes_ui();
            });
            let di = island.clone();
            note_check_btn.connect_clicked(move |_| di.insert_note_snippet("\n- [ ] "));
            let di = island.clone();
            note_bullet_btn.connect_clicked(move |_| di.insert_note_snippet("\n• "));
            let di = island.clone();
            note_h_btn.connect_clicked(move |_| di.insert_note_snippet("\n## "));

            let click_pin = GestureClick::new();
            let di_p = island.clone();
            click_pin.connect_pressed(move |_, _, _, _| {
                di_p.promote_to_pinned();
            });
            island.notes_editor.add_controller(click_pin);

            let di = island.clone();
            island.notes_editor.buffer().connect_changed(move |buf| {
                if di.state.borrow().suppress_note_change {
                    return;
                }
                let text = buf
                    .text(&buf.start_iter(), &buf.end_iter(), false)
                    .to_string();
                let words = text.split_whitespace().count();
                let chars = text.chars().count();
                di.notes_count_lbl.set_text(&format!("{chars} · {words} words"));
                {
                    let mut st = di.state.borrow_mut();
                    let id = st.active_note_id;
                    st.notes_store.update_body(id, text);
                }
                di.refresh_notes_sidebar_only();
            });
        }

        // Calendar quick-add entry
        {
            let click_pin = GestureClick::new();
            let di_p = island.clone();
            click_pin.connect_pressed(move |_, _, _, _| di_p.promote_to_pinned());
            cal_add_entry.add_controller(click_pin);

            let di = island.clone();
            cal_add_entry.connect_activate(move |entry| {
                let raw = entry.text().to_string();
                let date_iso = iso_date_with_offset(di.state.borrow().cal_selected_day_offset);
                if di.state.borrow_mut().cal_store.add_quick(&date_iso, &raw).is_some() {
                    entry.set_text("");
                    di.refresh_calendar_ui();
                }
            });
        }

        // Quick Tools controls
        {
            let di = island.clone();
            tool_shot_btn.connect_clicked(move |_| {
                di.close_pinned();
                if let Some(cb) = di.on_open_screenshot.borrow().as_ref() {
                    cb(false);
                } else {
                    di.ipc.send(IpcRequest::OpenScreenshotUi);
                }
            });
            let di = island.clone();
            tool_rec_btn.connect_clicked(move |_| {
                di.close_pinned();
                if let Some(cb) = di.on_open_screenshot.borrow().as_ref() {
                    cb(true);
                } else {
                    di.ipc.send(IpcRequest::OpenScreenshotUi);
                }
            });
            let di = island.clone();
            tool_clip_btn.connect_clicked(move |_| di.switch_module(NotchModule::Clipboard, true));
            let di = island.clone();
            tool_files_btn.connect_clicked(move |_| di.switch_module(NotchModule::Files, true));

            let click_focus = GestureClick::new();
            let di_f = island.clone();
            click_focus.connect_pressed(move |_, _, _, _| {
                di_f.focus_tools_calculator();
            });
            island.tools_calc_entry.add_controller(click_focus);

            let di = island.clone();
            island.tools_calc_entry.connect_changed(move |entry| {
                let txt = entry.text().to_string();
                if let Some(val) = eval_quick_math(&txt) {
                    let formatted = if (val - val.round()).abs() < 1e-6 && val.abs() < 1e12 {
                        format!("{}", val.round() as i64)
                    } else {
                        let s = format!("{val:.6}");
                        s.trim_end_matches('0').trim_end_matches('.').to_string()
                    };
                    di.tools_calc_result_lbl.set_text(&formatted);
                    di.tools_calc_result_lbl.remove_css_class("empty");
                    di.state.borrow_mut().calc_last_result = Some(formatted);
                } else {
                    di.tools_calc_result_lbl.set_text("0");
                    di.tools_calc_result_lbl.add_css_class("empty");
                    di.state.borrow_mut().calc_last_result = None;
                }
            });
            let di = island.clone();
            island.tools_calc_entry.connect_activate(move |entry| {
                if let Some(res) = di.state.borrow().calc_last_result.clone() {
                    di.copy_plain_text_to_clipboard(&res);
                    entry.select_region(0, -1);
                    di.tools_calc_result_lbl.set_text("Copied");
                    let lbl = di.tools_calc_result_lbl.clone();
                    let res_c = res;
                    glib::timeout_add_local_once(Duration::from_millis(850), move || {
                        lbl.set_text(&res_c);
                    });
                }
            });
            let di = island.clone();
            island.tools_calc_result_btn.clone().connect_clicked(move |_| {
                if let Some(res) = di.state.borrow().calc_last_result.clone() {
                    di.copy_plain_text_to_clipboard(&res);
                    di.tools_calc_result_lbl.set_text("Copied");
                    let lbl = di.tools_calc_result_lbl.clone();
                    let res_c = res;
                    glib::timeout_add_local_once(Duration::from_millis(850), move || {
                        lbl.set_text(&res_c);
                    });
                }
            });
        }

        // Keep Awake toggle
        {
            let di = island.clone();
            island.awake_toggle_btn.clone().connect_clicked(move |_| {
                di.toggle_keep_awake();
            });
        }

        // Audio & Display HUD sliders
        {
            let di = island.clone();
            island.hud_mute_btn.clone().connect_clicked(move |_| {
                di.sys_tx.send(SystemAction::ToggleSinkMute);
            });
            let di = island.clone();
            island.hud_vol_scale.clone().connect_value_changed(move |sc| {
                if di.suppress_hud_sliders.get() {
                    return;
                }
                let pct = sc.value().round().clamp(0.0, 100.0);
                di.hud_vol_pct.set_text(&format!("{}%", pct as i32));
                di.sys_tx.send(SystemAction::SetSinkVolume((pct / 100.0) as f32));
            });
            let di = island.clone();
            island.hud_bright_scale.clone().connect_value_changed(move |sc| {
                if di.suppress_hud_sliders.get() {
                    return;
                }
                let pct = sc.value().round().clamp(5.0, 100.0) as u32;
                di.hud_bright_pct.set_text(&format!("{pct}%"));
                di.sys_tx.send(SystemAction::SetBrightness(pct));
            });
        }

        // Notification & Screen Recording buttons
        {
            let di = island.clone();
            notif_close_btn.connect_clicked(move |_| {
                if let Some(notif) = di.state.borrow_mut().active_notif.take() {
                    di.store.dismiss(notif.id, 2);
                }
                di.sync_ui();
            });
            let di = island.clone();
            notif_open_btn.connect_clicked(move |_| {
                if let Some(notif) = di.state.borrow_mut().active_notif.take() {
                    activate_notification(&notif, None, &di.store, &di.windows.borrow(), &di.ipc);
                }
                di.close_pinned();
                di.sync_ui();
            });
            let di = island.clone();
            notif_body_btn.connect_clicked(move |_| {
                if let Some(notif) = di.state.borrow_mut().active_notif.take() {
                    activate_notification(&notif, None, &di.store, &di.windows.borrow(), &di.ipc);
                }
                di.close_pinned();
                di.sync_ui();
            });
            let di = island.clone();
            stop_rec_btn.connect_clicked(move |_| {
                if let Some(cb) = di.on_stop_recording.borrow().as_ref() {
                    cb();
                }
            });
            let di = island.clone();
            exp_rec_play_btn.connect_clicked(move |_| di.dispatch_media_cmd("PlayPause"));
        }

        // Live 250ms ticker: updates clock, media timestamps, Focus Timer, Keep Awake, HUD expiry, and 1s System Stats.
        {
            let di = island.clone();
            let mut ticks = 0u32;
            glib::timeout_add_local(Duration::from_millis(250), move || {
                ticks = ticks.wrapping_add(1);
                let is_1s = ticks.is_multiple_of(4);

                if is_1s {
                    di.tick_one_second();
                }

                if di.window.is_visible() {
                    let (playing, hud_expired) = {
                        let st = di.state.borrow();
                        let exp = st.hud_popup.as_ref().is_some_and(|h| Instant::now() >= h.until);
                        (st.media.playing, exp)
                    };
                    if playing {
                        di.refresh_media_time_labels();
                        di.media_progress_area.queue_draw();
                    }
                    if hud_expired {
                        di.state.borrow_mut().hud_popup = None;
                        di.sync_ui();
                    }
                }
                glib::ControlFlow::Continue
            });
        }

        // Initial render of all dynamic sub-views
        island.sync_appearance_layout();
        island.rebuild_dock_bar();
        island.rebuild_store_ui();
        island.rebuild_clipboard_filters();
        island.refresh_clipboard_ui();
        island.refresh_file_shelf_ui();
        island.refresh_notes_ui();
        island.refresh_calendar_ui();
        island.rebuild_awake_presets();
        island.refresh_awake_ui();
        island.refresh_timer_ui();
        island.refresh_home_live_ui();
        island
    }

    pub fn set_on_stop_recording<F: Fn() + 'static>(&self, cb: F) {
        *self.on_stop_recording.borrow_mut() = Some(Box::new(cb));
    }

    pub fn set_on_open_notifications<F: Fn() + 'static>(&self, cb: F) {
        *self.on_open_notifications.borrow_mut() = Some(Box::new(cb));
    }

    pub fn set_on_open_screenshot<F: Fn(bool) + 'static>(&self, cb: F) {
        *self.on_open_screenshot.borrow_mut() = Some(Box::new(cb));
    }

    /// Notifies the Dynamic Island when any Wayland drag-and-drop operation starts or ends across the desktop,
    /// allowing the File Dock to peek over maximized windows automatically.
    pub fn set_desktop_drag_active(&self, active: bool) {
        let changed = {
            let mut st = self.state.borrow_mut();
            if st.desktop_drag_active == active {
                false
            } else {
                st.desktop_drag_active = active;
                if active && !st.outgoing_drag {
                    st.peek_over_obscured = true;
                } else if !active && !st.hovered && !st.pinned_open && !st.drag_hover {
                    st.peek_over_obscured = false;
                }
                true
            }
        };
        if changed {
            self.sync_ui();
        }
    }

    /// Opens or closes the Little Notch Dynamic Island hub (`Super+N`).
    pub fn toggle_island(&self) {
        let is_pinned = self.state.borrow().pinned_open;
        if is_pinned {
            self.close_pinned();
        } else {
            let preferred = {
                let st = self.state.borrow();
                if st.active_module == NotchModule::Store {
                    NotchModule::Home
                } else {
                    st.active_module
                }
            };
            self.open_pinned_module(preferred);
        }
    }

    /// Toggles Clipboard History directly inside the Dynamic Island (`Super+V`).
    pub fn toggle_clipboard(&self) {
        let already = {
            let st = self.state.borrow();
            st.pinned_open && st.active_module == NotchModule::Clipboard
        };
        if already {
            self.close_pinned();
        } else {
            self.open_pinned_module(NotchModule::Clipboard);
        }
    }

    /// Opens a specific module pinned with keyboard focus & click-outside dismissal.
    pub fn open_pinned_module(&self, module: NotchModule) {
        {
            let mut st = self.state.borrow_mut();
            if st.locked {
                return;
            }
            st.pinned_open = true;
            st.hovered = true;
            st.active_module = module;
            st.active_notif = None;
            st.copied_entry_id = None;
            st.hover_gen = st.hover_gen.wrapping_add(1);
        }

        self.prepare_module_data(module);
        self.rebuild_dock_bar();

        self.backdrop_window.set_visible(true);
        self.backdrop_window.present();
        if gtk4_layer_shell::is_supported() {
            self.window.set_keyboard_mode(KeyboardMode::Exclusive);
        }
        self.sync_ui();
    }

    /// Promotes a hover-expanded island into pinned interactive mode (e.g. when typing in Search/Notes).
    fn promote_to_pinned(&self) {
        let already = self.state.borrow().pinned_open;
        if already {
            return;
        }
        let module = self.state.borrow().active_module;
        self.open_pinned_module(module);
    }

    /// Grants keyboard focus to the Quick Calculator without pinning the island, so hovering out
    /// still collapses cleanly and returns keyboard focus to the active app.
    fn focus_tools_calculator(&self) {
        if gtk4_layer_shell::is_supported() {
            self.window.set_keyboard_mode(KeyboardMode::Exclusive);
        }
        let entry = self.tools_calc_entry.clone();
        glib::idle_add_local_once(move || {
            entry.grab_focus();
            entry.set_position(-1);
        });
    }

    /// Closes pinned or hover-expanded mode and cleanly returns keyboard focus to the active app.
    pub fn close_pinned(&self) {
        let was_active = {
            let mut st = self.state.borrow_mut();
            let prev = st.pinned_open || st.hovered || st.peek_over_obscured;
            st.pinned_open = false;
            st.hovered = false;
            st.peek_over_obscured = false;
            st.copied_entry_id = None;
            st.hover_gen = st.hover_gen.wrapping_add(1);
            prev
        };
        if was_active {
            if gtk4_layer_shell::is_supported() {
                self.window.set_keyboard_mode(KeyboardMode::None);
            }
            self.backdrop_window.set_visible(false);
            self.sync_ui();
        }
    }

    /// Alias kept for mutual-exclusion hooks in `main.rs`.
    pub fn close_clipboard(&self) {
        self.close_pinned();
    }

    pub fn on_clipboard_updated(&self) {
        self.refresh_clipboard_ui();
    }

    pub fn set_workspaces_open(&self, open: bool) {
        {
            let mut st = self.state.borrow_mut();
            if st.workspaces_open == open {
                return;
            }
            st.workspaces_open = open;
        }
        if open {
            self.close_pinned();
        }
        self.sync_ui();
    }

    pub fn set_overview_open(&self, open: bool) {
        {
            let mut st = self.state.borrow_mut();
            if st.overview_open == open {
                return;
            }
            st.overview_open = open;
        }
        if open {
            self.close_pinned();
        }
        self.sync_ui();
    }

    pub fn update_desktop(&self, snapshot: &DesktopSnapshot, combined_mode: bool) {
        self.motion_enabled
            .set(snapshot.effects.enabled && !snapshot.effects.reduced_motion);
        self.speed_scale
            .set(snapshot.effects.speed_scale.clamp(0.5, 2.5));

        let obscured = is_island_obscured_by_windows(snapshot);
        let obscured_fullscreen = is_island_obscured_by_fullscreen(snapshot);
        let accent = Rgb::from_hex(&snapshot.appearance.accent_color);

        {
            let mut st = self.state.borrow_mut();
            st.combined_mode = combined_mode;
            st.island_enabled = snapshot.appearance.dynamic_island_enabled;
            st.locked = snapshot.locked;
            st.obscured = obscured;
            st.obscured_fullscreen = obscured_fullscreen;
            if !obscured && !st.desktop_drag_active && !st.drag_hover {
                st.peek_over_obscured = false;
            }
            st.overview_open = snapshot.overview_open;
            st.clock_24h = snapshot.appearance.clock_24h;
            st.accent = accent;
        }
        if snapshot.locked {
            self.close_pinned();
        }
        self.sync_ui();
    }

    pub fn update_system(&self, sys: &SystemSnapshot, unread_notifs: usize, dnd: bool) {
        let mut live_pill_triggered = false;
        {
            let mut st = self.state.borrow_mut();

            // 1. Detect AC charger plug-in -> Apple iOS green Charging pill!
            if sys.power.has_battery {
                if st.prev_on_battery == Some(true)
                    && (!sys.power.on_battery || sys.power.battery_state == BatteryState::Charging)
                    && !st.pinned_open
                    && !st.hovered
                {
                    st.hud_popup = Some(LivePillPopup {
                        kind: LivePillKind::Charging {
                            pct: sys.power.battery_percentage,
                            charging: true,
                        },
                        until: Instant::now() + Duration::from_millis(2600),
                    });
                    live_pill_triggered = true;
                }
                st.prev_on_battery = Some(sys.power.on_battery);
            }

            // 2. Detect DND toggle -> Apple iOS Silent pill!
            if let Some(prev_dnd) = st.prev_dnd
                && prev_dnd != dnd
                && !st.pinned_open
                && !st.hovered
            {
                st.hud_popup = Some(LivePillPopup {
                    kind: LivePillKind::Silent {
                        silent: dnd,
                        label: if dnd { "Silent".into() } else { "Ring".into() },
                    },
                    until: Instant::now() + Duration::from_millis(2200),
                });
                live_pill_triggered = true;
            }
            st.prev_dnd = Some(dnd);

            // 3. Detect Audio Mute toggle -> Apple iOS Silent/Muted pill, or Volume slider -> HUD pill!
            let cur_vol = (sys.audio.volume, sys.audio.muted);
            if let Some(prev) = st.prev_volume
                && !st.pinned_open
                && !st.hovered
            {
                if cur_vol.1 != prev.1 {
                    st.hud_popup = Some(LivePillPopup {
                        kind: LivePillKind::Silent {
                            silent: cur_vol.1,
                            label: if cur_vol.1 {
                                "Silent".into()
                            } else {
                                "Sound On".into()
                            },
                        },
                        until: Instant::now() + Duration::from_millis(2000),
                    });
                    live_pill_triggered = true;
                } else if (cur_vol.0 - prev.0).abs() >= 0.015 {
                    let sink = sys
                        .audio
                        .default_sink_name
                        .clone()
                        .unwrap_or_else(|| "Volume".into());
                    st.hud_popup = Some(LivePillPopup {
                        kind: LivePillKind::Hud {
                            is_brightness: false,
                            label: sink,
                            fraction: f64::from(cur_vol.0).clamp(0.0, 1.0),
                            muted: cur_vol.1,
                        },
                        until: Instant::now() + Duration::from_millis(1700),
                    });
                    live_pill_triggered = true;
                }
            }
            st.prev_volume = Some(cur_vol);

            // 4. Detect Backlight brightness changes -> Live HUD pill
            if let Some(cur_br) = read_backlight_percent() {
                if let Some(prev_br) = st.prev_brightness
                    && cur_br != prev_br
                    && !st.pinned_open
                    && !st.hovered
                {
                    st.hud_popup = Some(LivePillPopup {
                        kind: LivePillKind::Hud {
                            is_brightness: true,
                            label: "Brightness".into(),
                            fraction: f64::from(cur_br) / 100.0,
                            muted: false,
                        },
                        until: Instant::now() + Duration::from_millis(1700),
                    });
                    live_pill_triggered = true;
                }
                st.prev_brightness = Some(cur_br);
            }

            if st.media.position_us != sys.media.position_us {
                st.media_seek_override = None;
            }
            st.media = sys.media.clone();
            st.sys = sys.clone();
            st.unread_count = unread_notifs;
            st.dnd = dnd;
        }
        self.refresh_battery_ui();
        self.refresh_hud_controls_ui();
        self.refresh_home_live_ui();
        if live_pill_triggered {
            self.pill_hud_icon.queue_draw();
            self.pill_hud_bar.queue_draw();
            self.pill_charge_bat.queue_draw();
            self.pill_silent_badge.queue_draw();
        }
        self.sync_ui();
    }

    pub fn update_notification_summary(&self, unread_notifs: usize, dnd: bool) {
        let mut trigger_silent = false;
        {
            let mut st = self.state.borrow_mut();
            if let Some(prev_dnd) = st.prev_dnd
                && prev_dnd != dnd
                && !st.pinned_open
                && !st.hovered
            {
                st.hud_popup = Some(LivePillPopup {
                    kind: LivePillKind::Silent {
                        silent: dnd,
                        label: if dnd { "Silent".into() } else { "Ring".into() },
                    },
                    until: Instant::now() + Duration::from_millis(2200),
                });
                trigger_silent = true;
            }
            st.prev_dnd = Some(dnd);
            st.unread_count = unread_notifs;
            st.dnd = dnd;
        }
        if trigger_silent {
            self.pill_silent_badge.queue_draw();
        }
        self.refresh_home_live_ui();
        self.sync_ui();
    }

    pub fn set_recording_status(&self, active: bool, elapsed_secs: u64) {
        {
            let mut st = self.state.borrow_mut();
            st.recording_active = active;
            st.recording_secs = elapsed_secs;
        }
        self.refresh_home_live_ui();
        self.sync_ui();
    }

    pub fn set_suppressed(&self, suppressed: bool) {
        let changed = {
            let mut st = self.state.borrow_mut();
            let had = st.active_notif.is_some();
            st.suppressed = suppressed;
            if suppressed && had {
                st.active_notif = None;
                st.notif_gen = st.notif_gen.wrapping_add(1);
                true
            } else {
                false
            }
        };
        if changed {
            self.sync_ui();
        }
    }

    pub fn push_notification(&self, item: DesktopNotification) {
        let should_show = {
            let st = self.state.borrow();
            st.combined_mode
                && st.island_enabled
                && !st.locked
                && !st.suppressed
                && !st.obscured
                && !st.pinned_open
                && (!st.dnd || item.urgency == NotificationUrgency::Critical)
        };
        if !should_show {
            return;
        }
        let id = item.id;
        let timeout_ms = (item.expire_timeout_ms.max(4200)).min(8000) as u64;
        let epoch = {
            let mut st = self.state.borrow_mut();
            st.notif_gen = st.notif_gen.wrapping_add(1);
            st.active_notif = Some(item);
            st.notif_gen
        };
        self.sync_ui();

        let di = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(timeout_ms), move || {
            di.expire_notif_or_defer(id, epoch);
        });
    }

    pub fn dismiss_notification(&self, id: u32) {
        let changed = {
            let mut st = self.state.borrow_mut();
            if st.active_notif.as_ref().is_some_and(|n| n.id == id) {
                st.active_notif = None;
                st.notif_gen = st.notif_gen.wrapping_add(1);
                true
            } else {
                false
            }
        };
        if changed {
            self.sync_ui();
        }
    }

    pub fn clear_notifications(&self) {
        let changed = {
            let mut st = self.state.borrow_mut();
            if st.active_notif.is_some() {
                st.active_notif = None;
                st.notif_gen = st.notif_gen.wrapping_add(1);
                true
            } else {
                false
            }
        };
        if changed {
            self.sync_ui();
        }
    }

    /// Preview helper for automated visual verification (`compact`, `charging`, `silent`, `notif`,
    /// `home`, `media`, `timer`, `clipboard`, `files`, `notes`, `calendar`, `tools`, `stats`,
    /// `battery`, `awake`, `store`).
    pub fn open_for_test(&self, mode: &str) {
        {
            let mut st = self.state.borrow_mut();
            st.combined_mode = true;
            st.island_enabled = true;
            st.locked = false;
            st.obscured = false;
            st.overview_open = false;
            st.workspaces_open = false;
            st.hud_popup = None;
            st.active_notif = None;
            let is_compact_preview = matches!(mode, "compact" | "charging" | "silent");
            st.test_force_expand = !is_compact_preview;
            st.hovered = !is_compact_preview;
            st.active_module = match mode {
                "charging" => {
                    st.hud_popup = Some(LivePillPopup {
                        kind: LivePillKind::Charging {
                            pct: 75.0,
                            charging: true,
                        },
                        until: Instant::now() + Duration::from_secs(30),
                    });
                    NotchModule::Home
                }
                "silent" => {
                    st.hud_popup = Some(LivePillPopup {
                        kind: LivePillKind::Silent {
                            silent: true,
                            label: "Silent".into(),
                        },
                        until: Instant::now() + Duration::from_secs(30),
                    });
                    NotchModule::Home
                }
                "notif" => {
                    st.notif_gen = st.notif_gen.wrapping_add(1);
                    st.active_notif = Some(DesktopNotification {
                        id: 999,
                        app_name: "Messages".into(),
                        app_icon: "mail-unread-symbolic".into(),
                        summary: "Olivia Rodrigo".into(),
                        body: "mobile".into(),
                        urgency: NotificationUrgency::Normal,
                        expire_timeout_ms: 30_000,
                        received_unix_secs: now_unix_secs(),
                        dismissed: false,
                        actions: Vec::new(),
                        desktop_entry: None,
                        resident: false,
                    });
                    NotchModule::Home
                }
                "media" => {
                    st.media = MediaStatus {
                        available: true,
                        bus_name: Some("org.mpris.MediaPlayer2.spotify".into()),
                        player_name: Some("Spotify".into()),
                        title: Some("Levitating".into()),
                        artist: Some("Dua Lipa".into()),
                        album: Some("Future Nostalgia".into()),
                        art_url: None,
                        art_path: None,
                        track_id: Some("/org/mpris/MediaPlayer2/Track/1".into()),
                        position_us: Some(170_000_000),
                        length_us: Some(221_000_000),
                        can_seek: true,
                        observed_unix_ms: now_unix_ms(),
                        playing: true,
                    };
                    st.target_palette = NeonPalette {
                        primary: Rgb::new(0.98, 0.25, 0.62),
                        secondary: Rgb::new(0.42, 0.36, 0.99),
                        tertiary: Rgb::new(0.14, 0.86, 0.98),
                    };
                    st.cur_palette = st.target_palette;
                    NotchModule::Media
                }
                "timer" => NotchModule::Timer,
                "clipboard" => NotchModule::Clipboard,
                "files" => NotchModule::Files,
                "notes" => NotchModule::Notes,
                "calendar" => NotchModule::Calendar,
                "tools" => NotchModule::Tools,
                "stats" => NotchModule::Stats,
                "battery" => NotchModule::Battery,
                "awake" => NotchModule::Awake,
                "hud" => NotchModule::Hud,
                "store" => NotchModule::Store,
                _ => NotchModule::Home,
            };
        }
        let active = self.state.borrow().active_module;
        self.prepare_module_data(active);
        self.rebuild_dock_bar();
        self.sync_ui();
    }

    // =========================================================================
    // Module Switching, Dock Bar, Store & Per-Module Logic
    // =========================================================================

    fn switch_module(&self, module: NotchModule, keep_open: bool) {
        {
            let mut st = self.state.borrow_mut();
            st.active_module = module;
            st.active_notif = None;
            st.hover_gen = st.hover_gen.wrapping_add(1);
            if keep_open {
                st.hovered = true;
            }
        }
        self.prepare_module_data(module);
        self.rebuild_dock_bar();
        if module == NotchModule::Tools {
            self.sync_ui();
            self.focus_tools_calculator();
        } else {
            if !self.state.borrow().pinned_open && gtk4_layer_shell::is_supported() {
                self.window.set_keyboard_mode(KeyboardMode::None);
            }
            self.sync_ui();
        }
    }

    fn prepare_module_data(&self, module: NotchModule) {
        match module {
            NotchModule::Home => self.refresh_home_live_ui(),
            NotchModule::Media => self.refresh_media_time_labels(),
            NotchModule::Timer => self.refresh_timer_ui(),
            NotchModule::Clipboard => self.refresh_clipboard_ui(),
            NotchModule::Files => self.refresh_file_shelf_ui(),
            NotchModule::Notes => self.refresh_notes_ui(),
            NotchModule::Calendar => self.refresh_calendar_ui(),
            NotchModule::Tools => {
                let entry = self.tools_calc_entry.clone();
                glib::idle_add_local_once(move || {
                    entry.grab_focus();
                    entry.set_position(-1);
                });
            }
            NotchModule::Stats => self.sample_and_refresh_stats(),
            NotchModule::Battery => self.refresh_battery_ui(),
            NotchModule::Awake => self.refresh_awake_ui(),
            NotchModule::Hud => self.refresh_hud_controls_ui(),
            NotchModule::Store => self.rebuild_store_ui(),
        }
    }

    fn rebuild_dock_bar(&self) {
        while let Some(c) = self.dock_items_box.first_child() {
            self.dock_items_box.remove(&c);
        }
        let (pinned, active) = {
            let st = self.state.borrow();
            (st.prefs.pinned.clone(), st.active_module)
        };
        for module in pinned {
            let is_active = module == active;
            let btn = Button::new();
            btn.add_css_class("neon-notch-dock-btn");
            if is_active {
                btn.add_css_class("active");
            }
            btn.set_tooltip_text(Some(module.title()));

            let hovered = Rc::new(Cell::new(false));
            let canvas = DrawingArea::new();
            canvas.set_size_request(20, 20);
            canvas.set_halign(Align::Center);
            canvas.set_valign(Align::Center);
            canvas.set_can_target(false);

            let hov_draw = Rc::clone(&hovered);
            canvas.set_draw_func(move |_, cr, w, h| {
                let rgba = if is_active || hov_draw.get() {
                    (1.0, 1.0, 1.0, 0.98)
                } else {
                    (0.62, 0.62, 0.66, 0.95)
                };
                draw_module_vector_icon(cr, f64::from(w), f64::from(h), module, rgba);
            });

            let h_in = Rc::clone(&hovered);
            let c_in = canvas.clone();
            let h_out = Rc::clone(&hovered);
            let c_out = canvas.clone();
            let mot = EventControllerMotion::new();
            mot.connect_enter(move |_, _, _| {
                h_in.set(true);
                c_in.queue_draw();
            });
            mot.connect_leave(move |_| {
                h_out.set(false);
                c_out.queue_draw();
            });
            btn.add_controller(mot);

            btn.set_child(Some(&canvas));
            let di = self.clone();
            btn.connect_clicked(move |_| {
                di.switch_module(module, true);
            });
            self.dock_items_box.append(&btn);
        }

        if active == NotchModule::Store {
            self.dock_store_btn.add_css_class("active");
        } else {
            self.dock_store_btn.remove_css_class("active");
        }
        self.dock_store_canvas.queue_draw();
        self.dock_bg_canvas.queue_draw();
    }

    fn rebuild_store_ui(&self) {
        while let Some(c) = self.store_grid.first_child() {
            self.store_grid.remove(&c);
        }
        let (pinned, appearance, accent) = {
            let st = self.state.borrow();
            (st.prefs.pinned.clone(), st.prefs.appearance, st.accent)
        };
        self.store_count_lbl
            .set_text(&format!("{}/5 Docked", pinned.len()));

        for &module in NotchModule::ALL_MODULES {
            let is_pinned = pinned.contains(&module);
            let card = GtkBox::new(Orientation::Horizontal, 6);
            card.add_css_class("neon-notch-store-card");
            card.set_valign(Align::Center);
            if is_pinned {
                card.add_css_class("pinned");
            }

            let open_btn = Button::new();
            open_btn.add_css_class("neon-island-clip-body-btn");
            open_btn.set_hexpand(true);
            open_btn.set_valign(Align::Center);

            let open_row = GtkBox::new(Orientation::Horizontal, 8);
            open_row.set_valign(Align::Center);
            let badge = build_module_badge_canvas(module, 32);

            let col = GtkBox::new(Orientation::Vertical, 1);
            col.set_valign(Align::Center);
            col.set_hexpand(true);
            let title = Label::new(Some(module.title()));
            title.add_css_class("neon-notch-store-title");
            title.set_halign(Align::Start);
            title.set_xalign(0.0);
            title.set_max_width_chars(13);
            title.set_ellipsize(gtk4::pango::EllipsizeMode::End);
            let sub = Label::new(Some(module.subtitle()));
            sub.add_css_class("neon-notch-store-sub");
            sub.set_halign(Align::Start);
            sub.set_xalign(0.0);
            sub.set_max_width_chars(16);
            sub.set_ellipsize(gtk4::pango::EllipsizeMode::End);
            col.append(&title);
            col.append(&sub);

            open_row.append(&badge);
            open_row.append(&col);
            open_btn.set_child(Some(&open_row));
            {
                let di = self.clone();
                open_btn.connect_clicked(move |_| {
                    di.switch_module(module, true);
                });
            }

            let pin_btn = build_round_pin_btn(
                is_pinned,
                26,
                accent,
                if is_pinned {
                    "Unpin from dock"
                } else {
                    "Pin to dock"
                },
            );
            {
                let di = self.clone();
                pin_btn.connect_clicked(move |_| {
                    {
                        let mut st = di.state.borrow_mut();
                        st.prefs.toggle_pin(module);
                        st.hovered = true;
                        st.hover_gen = st.hover_gen.wrapping_add(1);
                    }
                    di.rebuild_dock_bar();
                    di.rebuild_store_ui();
                });
            }

            card.append(&open_btn);
            card.append(&pin_btn);
            self.store_grid.insert(&card, -1);
        }

        // Rebuild 3-way appearance toggle (iOS Black / Mac Notch / Liquid Glass)
        while let Some(c) = self.store_appearance_row.first_child() {
            self.store_appearance_row.remove(&c);
        }
        for mode in [
            NotchAppearance::Black,
            NotchAppearance::BlackGlass,
            NotchAppearance::LiquidGlass,
        ] {
            let btn = Button::with_label(mode.label());
            btn.add_css_class("neon-island-filter-pill");
            if mode == appearance {
                btn.add_css_class("active");
            }
            let di = self.clone();
            btn.connect_clicked(move |_| {
                {
                    let mut st = di.state.borrow_mut();
                    st.prefs.appearance = mode;
                    st.prefs.save();
                    st.hovered = true;
                    st.hover_gen = st.hover_gen.wrapping_add(1);
                }
                di.sync_appearance_layout();
                di.rebuild_store_ui();
                di.sync_ui();
            });
            self.store_appearance_row.append(&btn);
        }
    }

    fn sync_appearance_layout(&self) {
        let (is_notch, stealth, outgoing, w, h, alpha) = {
            let st = self.state.borrow();
            (
                st.prefs.appearance.is_top_flush_notch(),
                st.is_stealth_sensor(),
                st.outgoing_drag,
                st.morph.w.value,
                st.morph.h.value,
                st.morph.alpha.value,
            )
        };
        let top_off = if is_notch { 0 } else { SHELL_TOP_OFFSET };
        self.shell_box.set_margin_top(top_off);
        if is_notch {
            self.shell_box.add_css_class("notch-flush");
        } else {
            self.shell_box.remove_css_class("notch-flush");
        }
        self.bg_canvas.queue_draw();
        self.dock_bg_canvas.queue_draw();
        if outgoing {
            apply_passthrough_input_region(&self.window);
        } else if stealth {
            apply_sensor_input_region(&self.window);
        } else {
            apply_input_region(&self.window, top_off, w, h, alpha > 0.25);
        }
    }

    fn refresh_home_live_ui(&self) {
        while let Some(c) = self.home_live_box.first_child() {
            self.home_live_box.remove(&c);
        }
        let (
            media,
            timer_run,
            timer_rem,
            awake_on,
            awake_rem,
            rec_on,
            rec_secs,
            shelf_len,
            unread,
            dnd,
        ) = {
            let st = self.state.borrow();
            (
                st.media.clone(),
                st.timer_running,
                st.timer_remaining_secs,
                st.awake_active,
                st.awake_remaining_secs,
                st.recording_active,
                st.recording_secs,
                st.shelf_items.len(),
                st.unread_count,
                st.dnd,
            )
        };

        if dnd {
            self.home_dnd_btn.add_css_class("active");
            self.home_dnd_lbl.set_text("DND On");
        } else {
            self.home_dnd_btn.remove_css_class("active");
            self.home_dnd_lbl.set_text("DND");
        }

        let mut any_live = false;
        let media_active = media.available
            && (media.playing || media.title.as_deref().is_some_and(|t| !t.is_empty()));
        if media_active {
            any_live = true;
            let title = media
                .title
                .as_deref()
                .or(media.player_name.as_deref())
                .unwrap_or("Now Playing");
            let tag = if media.playing { "Playing" } else { "Paused" };
            let btn = self.build_live_home_row(NotchModule::Media, title, tag, NotchModule::Media);
            self.home_live_box.append(&btn);
        }
        if timer_run {
            any_live = true;
            let rem = format_mm_ss(timer_rem);
            let btn =
                self.build_live_home_row(NotchModule::Timer, "Focus Timer", &rem, NotchModule::Timer);
            self.home_live_box.append(&btn);
        }
        if shelf_len > 0 {
            any_live = true;
            let lbl = format!(
                "File Dock ({shelf_len} {})",
                if shelf_len == 1 { "item" } else { "items" }
            );
            let btn =
                self.build_live_home_row(NotchModule::Files, &lbl, "Drag out", NotchModule::Files);
            self.home_live_box.append(&btn);
        }
        if awake_on {
            any_live = true;
            let rem = awake_rem
                .map(format_hh_mm_ss)
                .unwrap_or_else(|| "∞".into());
            let btn =
                self.build_live_home_row(NotchModule::Awake, "Keep Awake", &rem, NotchModule::Awake);
            self.home_live_box.append(&btn);
        }
        if rec_on {
            any_live = true;
            let rem = format_mm_ss(rec_secs);
            let btn = self.build_live_home_row(
                NotchModule::Tools,
                "Screen Recording",
                &rem,
                NotchModule::Home,
            );
            self.home_live_box.append(&btn);
        }

        if !any_live {
            let status_txt = match unread {
                0 => "All caught up · Press Super+N anytime".to_string(),
                1 => "1 unread notification".to_string(),
                n => format!("{n} unread notifications"),
            };
            let btn = self.build_live_home_row(
                NotchModule::Clipboard,
                &format!("Clipboard ({} items)", self.clipboard_store.len()),
                "Super+V",
                NotchModule::Clipboard,
            );
            self.home_live_box.append(&btn);

            let sub = Label::new(Some(&status_txt));
            sub.add_css_class("neon-island-muted-xs");
            sub.set_halign(Align::Start);
            self.home_live_box.append(&sub);
        }
    }

    fn build_live_home_row(
        &self,
        badge_module: NotchModule,
        title: &str,
        right_tag: &str,
        target: NotchModule,
    ) -> Button {
        let btn = Button::new();
        btn.add_css_class("neon-island-mini-strip-btn");
        let row = GtkBox::new(Orientation::Horizontal, 8);
        row.set_valign(Align::Center);

        let badge = build_module_badge_canvas(badge_module, 24);

        let lbl = Label::new(Some(title));
        lbl.add_css_class("neon-island-clip-preview");
        lbl.set_hexpand(true);
        lbl.set_halign(Align::Start);
        lbl.set_ellipsize(gtk4::pango::EllipsizeMode::End);

        let tag = Label::new(Some(right_tag));
        tag.add_css_class("neon-island-media-time");

        row.append(&badge);
        row.append(&lbl);
        row.append(&tag);
        btn.set_child(Some(&row));

        let di = self.clone();
        btn.connect_clicked(move |_| {
            di.switch_module(target, true);
        });
        btn
    }

    // ---- Scrubbable Ruler Timer ----

    fn set_timer_mode(&self, focus: bool) {
        {
            let mut st = self.state.borrow_mut();
            st.timer_focus_mode = focus;
            if !st.timer_running {
                st.timer_set_mins = if focus { 25 } else { 5 };
                st.timer_total_secs = u64::from(st.timer_set_mins) * 60;
                st.timer_remaining_secs = st.timer_total_secs;
            }
        }
        self.refresh_timer_ui();
        self.sync_ui();
    }

    fn scrub_timer_ruler_at(&self, x: f64) {
        let w = f64::from(self.timer_ruler_area.width().max(60));
        let pad_x = 14.0;
        let span = (w - pad_x * 2.0).max(20.0);
        let frac = ((x - pad_x) / span).clamp(0.0, 1.0);
        let mins = (5.0 + frac * 55.0).round() as u32;
        {
            let mut st = self.state.borrow_mut();
            st.timer_set_mins = mins.clamp(1, 60);
            st.timer_total_secs = u64::from(st.timer_set_mins) * 60;
            st.timer_remaining_secs = st.timer_total_secs;
            st.timer_running = false;
        }
        self.refresh_timer_ui();
        self.sync_ui();
    }

    fn nudge_timer_minutes(&self, delta: i32) {
        {
            let mut st = self.state.borrow_mut();
            let next = (st.timer_set_mins as i32 + delta).clamp(1, 60) as u32;
            st.timer_set_mins = next;
            st.timer_total_secs = u64::from(next) * 60;
            st.timer_remaining_secs = st.timer_total_secs;
            st.timer_running = false;
        }
        self.refresh_timer_ui();
        self.sync_ui();
    }

    fn toggle_timer_running(&self) {
        {
            let mut st = self.state.borrow_mut();
            if st.timer_remaining_secs == 0 {
                st.timer_total_secs = u64::from(st.timer_set_mins) * 60;
                st.timer_remaining_secs = st.timer_total_secs;
            }
            st.timer_running = !st.timer_running;
        }
        self.refresh_timer_ui();
        self.refresh_home_live_ui();
        self.sync_ui();
    }

    fn reset_timer(&self) {
        {
            let mut st = self.state.borrow_mut();
            st.timer_running = false;
            st.timer_total_secs = u64::from(st.timer_set_mins) * 60;
            st.timer_remaining_secs = st.timer_total_secs;
        }
        self.refresh_timer_ui();
        self.refresh_home_live_ui();
        self.sync_ui();
    }

    fn refresh_timer_ui(&self) {
        let (focus, running, rem) = {
            let st = self.state.borrow();
            (st.timer_focus_mode, st.timer_running, st.timer_remaining_secs)
        };
        if focus {
            self.timer_focus_btn.add_css_class("focus-active");
            self.timer_break_btn.remove_css_class("break-active");
            self.timer_readout_lbl.remove_css_class("break");
        } else {
            self.timer_focus_btn.remove_css_class("focus-active");
            self.timer_break_btn.add_css_class("break-active");
            self.timer_readout_lbl.add_css_class("break");
        }
        self.timer_start_btn
            .set_label(if running { "Pause" } else { "Start Timer" });
        let formatted = format_mm_ss(rem);
        self.timer_readout_lbl.set_text(&formatted);
        self.pill_timer_label.set_text(&formatted);
        self.timer_ruler_area.queue_draw();
        self.pill_timer_ring.queue_draw();
    }

    // ---- Clipboard History (Horizontal Card Strip + Filters + Color Swatches!) ----

    fn rebuild_clipboard_filters(&self) {
        while let Some(c) = self.clip_filter_row.first_child() {
            self.clip_filter_row.remove(&c);
        }
        let active = self.state.borrow().clip_filter.clone();
        for (id, label) in [
            ("all", "All"),
            ("text", "Text"),
            ("links", "Links"),
            ("colors", "Colors"),
            ("code", "Code"),
        ] {
            let btn = Button::with_label(label);
            btn.add_css_class("neon-island-filter-pill");
            if active == id {
                btn.add_css_class("active");
            }
            let di = self.clone();
            let id_s = id.to_string();
            btn.connect_clicked(move |_| {
                di.state.borrow_mut().clip_filter = id_s.clone();
                di.rebuild_clipboard_filters();
                di.refresh_clipboard_ui();
            });
            self.clip_filter_row.append(&btn);
        }
    }

    fn filtered_clipboard_entries(&self) -> Vec<ClipboardEntry> {
        let (filter, query) = {
            let st = self.state.borrow();
            (st.clip_filter.clone(), st.clip_query.trim().to_ascii_lowercase())
        };
        self.clipboard_store
            .entries()
            .into_iter()
            .filter(|e| {
                let kind = classify_clipboard_text(&e.text);
                kind.matches_filter(&filter)
                    && (query.is_empty()
                        || e.text.to_ascii_lowercase().contains(&query)
                        || e.preview.to_ascii_lowercase().contains(&query))
            })
            .collect()
    }

    fn refresh_clipboard_ui(&self) {
        let entries = self.filtered_clipboard_entries();
        let total = self.clipboard_store.len();
        self.clip_clear_btn.set_visible(total > 0);
        let (copied_id, accent) = {
            let st = self.state.borrow();
            (st.copied_entry_id, st.accent)
        };

        while let Some(c) = self.clip_cards_box.first_child() {
            self.clip_cards_box.remove(&c);
        }

        if entries.is_empty() {
            self.clip_cards_box.set_visible(false);
            self.clip_empty_lbl.set_visible(true);
            return;
        }
        self.clip_cards_box.set_visible(true);
        self.clip_empty_lbl.set_visible(false);

        let now = now_unix_secs();
        for entry in entries {
            let kind = classify_clipboard_text(&entry.text);
            let is_copied = copied_id == Some(entry.id);

            let card = GtkBox::new(Orientation::Vertical, 6);
            card.add_css_class("neon-notch-clip-card");
            if entry.pinned {
                card.add_css_class("pinned");
            }
            if is_copied {
                card.add_css_class("copied");
            }
            card.set_size_request(158, 146);

            let body_btn = Button::new();
            body_btn.add_css_class("neon-island-clip-body-btn");
            body_btn.set_vexpand(true);

            let body_col = GtkBox::new(Orientation::Vertical, 4);
            body_col.set_vexpand(true);

            match &kind {
                ClipboardItemKind::Color(hex) => {
                    // Exact Little Notch orange/neon vibrant color swatch card!
                    let swatch = DrawingArea::new();
                    swatch.set_size_request(134, 82);
                    swatch.set_vexpand(true);
                    let rgb = Rgb::from_hex(hex);
                    swatch.set_draw_func(move |_, cr, w, h| {
                        cr.set_antialias(cairo::Antialias::Best);
                        crate::island_visuals::rounded_rect_path(cr, 0.0, 0.0, f64::from(w), f64::from(h), 10.0);
                        cr.set_source_rgba(rgb.r, rgb.g, rgb.b, 1.0);
                        let _ = cr.fill();
                    });
                    let hex_lbl = Label::new(Some(hex));
                    hex_lbl.add_css_class("neon-notch-clip-code");
                    hex_lbl.set_halign(Align::Start);
                    body_col.append(&swatch);
                    body_col.append(&hex_lbl);
                }
                ClipboardItemKind::Code => {
                    let lbl = Label::new(Some(&entry.preview));
                    lbl.add_css_class("neon-notch-clip-code");
                    lbl.set_halign(Align::Start);
                    lbl.set_valign(Align::Start);
                    lbl.set_xalign(0.0);
                    lbl.set_yalign(0.0);
                    lbl.set_wrap(true);
                    lbl.set_wrap_mode(gtk4::pango::WrapMode::WordChar);
                    lbl.set_lines(5);
                    lbl.set_max_width_chars(20);
                    lbl.set_ellipsize(gtk4::pango::EllipsizeMode::End);
                    lbl.set_vexpand(true);
                    body_col.append(&lbl);
                }
                _ => {
                    let lbl = Label::new(Some(&entry.preview));
                    lbl.add_css_class("neon-island-clip-preview");
                    lbl.set_halign(Align::Start);
                    lbl.set_valign(Align::Start);
                    lbl.set_xalign(0.0);
                    lbl.set_yalign(0.0);
                    lbl.set_wrap(true);
                    lbl.set_wrap_mode(gtk4::pango::WrapMode::WordChar);
                    lbl.set_lines(5);
                    lbl.set_max_width_chars(20);
                    lbl.set_ellipsize(gtk4::pango::EllipsizeMode::End);
                    lbl.set_vexpand(true);
                    body_col.append(&lbl);
                }
            }
            body_btn.set_child(Some(&body_col));
            {
                let di = self.clone();
                let e_c = entry.clone();
                body_btn.connect_clicked(move |_| di.copy_clipboard_entry(&e_c));
            }

            let foot = GtkBox::new(Orientation::Horizontal, 3);
            foot.set_valign(Align::Center);
            let age = format_relative_age(entry.copied_at_unix, now);
            let meta = Label::new(Some(&if is_copied {
                "Copied!".into()
            } else {
                age
            }));
            meta.add_css_class(if is_copied {
                "neon-island-clip-copied-tag"
            } else {
                "neon-island-clip-meta"
            });
            meta.set_hexpand(true);
            meta.set_halign(Align::Start);
            meta.set_ellipsize(gtk4::pango::EllipsizeMode::End);

            let badge = Label::new(Some(kind.badge()));
            badge.add_css_class("neon-notch-type-tag");

            let pin_btn = build_round_pin_btn(
                entry.pinned,
                20,
                accent,
                if entry.pinned { "Unpin" } else { "Pin snippet" },
            );
            {
                let di = self.clone();
                let id = entry.id;
                pin_btn.connect_clicked(move |_| {
                    if di.clipboard_store.toggle_pin(id) {
                        di.refresh_clipboard_ui();
                    }
                });
            }

            let del_btn = build_round_close_btn(20, "Remove snippet");
            {
                let di = self.clone();
                let id = entry.id;
                del_btn.connect_clicked(move |_| {
                    if di.clipboard_store.remove(id) {
                        di.refresh_clipboard_ui();
                    }
                });
            }

            foot.append(&meta);
            foot.append(&badge);
            foot.append(&pin_btn);
            foot.append(&del_btn);

            card.append(&body_btn);
            card.append(&foot);
            self.clip_cards_box.append(&card);
        }
    }

    fn copy_plain_text_to_clipboard(&self, text: &str) {
        if let Some(display) = gdk::Display::default() {
            display.clipboard().set_text(text);
        }
        let owned = text.to_string();
        std::thread::spawn(move || {
            let _ = write_wayland_clipboard_text(&owned);
        });
    }

    fn copy_clipboard_entry(&self, entry: &ClipboardEntry) {
        self.copy_plain_text_to_clipboard(&entry.text);
        let flash_gen = {
            let mut st = self.state.borrow_mut();
            st.copied_entry_id = Some(entry.id);
            st.copy_flash_gen = st.copy_flash_gen.wrapping_add(1);
            st.copy_flash_gen
        };
        self.refresh_clipboard_ui();

        let di = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(190), move || {
            if di.state.borrow().copy_flash_gen == flash_gen {
                di.close_pinned();
            }
        });
    }

    // ---- OnePlus-Style Persistent File Dock ----

    fn add_file_to_shelf(&self, path: PathBuf) -> bool {
        let Some(item) = FileShelfItem::from_path(path) else {
            return false;
        };
        {
            let mut st = self.state.borrow_mut();
            st.shelf_items.retain(|i| i.path != item.path);
            st.shelf_selected_path = Some(item.path.clone());
            st.shelf_items.insert(0, item);
            st.shelf_items.truncate(24);
            save_persisted_shelf_items(&st.shelf_items);
        }
        self.refresh_home_live_ui();
        true
    }

    fn open_shelf_file_picker(&self) {
        let dialog = gtk4::FileDialog::builder()
            .title("Add File to Notch Shelf")
            .build();
        let di = self.clone();
        dialog.open_multiple(Some(&self.window), None::<&gio::Cancellable>, move |res| {
            if let Ok(list) = res {
                let mut added = 0usize;
                for i in 0..list.n_items() {
                    if let Some(obj) = list.item(i)
                        && let Ok(f) = obj.downcast::<gio::File>()
                        && let Some(p) = f.path()
                        && di.add_file_to_shelf(p)
                    {
                        added += 1;
                    }
                }
                if added > 0 {
                    di.state.borrow_mut().shelf_show_recent = false;
                    di.state.borrow_mut().shelf_status_msg = Some(format!("Added {added} file(s)"));
                    di.refresh_file_shelf_ui();
                }
            }
        });
    }

    fn toggle_zip_selected_shelf_file(&self) {
        let Some(path) = self.state.borrow().shelf_selected_path.clone() else {
            return;
        };
        let is_arch = FileShelfItem::from_path(path.clone())
            .map(|i| i.is_archive)
            .unwrap_or(false);
        let res = if is_arch {
            extract_shelf_archive(&path).map(|out| ("Extracted", out))
        } else {
            compress_shelf_item(&path).map(|out| ("Compressed", out))
        };
        match res {
            Ok((verb, out)) => {
                let size_str = FileShelfItem::from_path(out.clone())
                    .map(|i| format_file_size(i.size_bytes))
                    .unwrap_or_default();
                let name = out
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                self.add_file_to_shelf(out);
                self.state.borrow_mut().shelf_show_recent = false;
                self.state.borrow_mut().shelf_status_msg =
                    Some(format!("{verb} {name} ({size_str})"));
            }
            Err(err) => {
                self.state.borrow_mut().shelf_status_msg = Some(err);
            }
        }
        self.refresh_file_shelf_ui();
    }

    fn refresh_file_shelf_ui(&self) {
        let (show_recent, shelf_count, items, selected_path, status_msg) = {
            let st = self.state.borrow();
            let list = if st.shelf_show_recent {
                list_recent_user_files(16)
            } else {
                st.shelf_items.clone()
            };
            (
                st.shelf_show_recent,
                st.shelf_items.len(),
                list,
                st.shelf_selected_path.clone(),
                st.shelf_status_msg.clone(),
            )
        };

        self.shelf_tab_btn
            .set_label(&format!("File Dock {shelf_count}"));
        if show_recent {
            self.shelf_tab_btn.remove_css_class("active");
            self.shelf_recent_btn.add_css_class("active");
        } else {
            self.shelf_tab_btn.add_css_class("active");
            self.shelf_recent_btn.remove_css_class("active");
        }

        if let Some(msg) = status_msg {
            self.shelf_status_lbl.set_text(&msg);
        } else if let Some(ref p) = selected_path {
            let fname = p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            self.shelf_status_lbl
                .set_text(&format!("{fname} · Drag tile directly into any app"));
        } else {
            self.shelf_status_lbl
                .set_text("Drag files onto notch · Drag any tile out to drop into another app");
        }
        self.shelf_action_bar.set_visible(!items.is_empty());

        while let Some(c) = self.shelf_strip_box.first_child() {
            self.shelf_strip_box.remove(&c);
        }

        if items.is_empty() {
            self.shelf_strip_box.set_visible(false);
            self.shelf_empty_lbl.set_visible(true);
            return;
        }
        self.shelf_strip_box.set_visible(true);
        self.shelf_empty_lbl.set_visible(false);

        for item in items {
            let is_sel = selected_path.as_ref() == Some(&item.path);
            let card_overlay = GtkOverlay::new();
            card_overlay.set_size_request(88, 96);

            let tile = Button::new();
            tile.add_css_class("neon-notch-file-tile");
            if is_sel {
                tile.add_css_class("selected");
            }
            tile.set_size_request(88, 96);
            tile.set_tooltip_text(Some(&format!(
                "{}\nDrag to drop into any application",
                item.name
            )));

            let col = GtkBox::new(Orientation::Vertical, 4);
            col.set_halign(Align::Center);
            col.set_valign(Align::Center);

            if item.is_image
                && let Ok(tex) = gdk::Texture::from_filename(&item.path)
            {
                let frame = GtkOverlay::new();
                frame.add_css_class("neon-notch-file-thumb");
                frame.set_size_request(50, 44);
                frame.set_overflow(gtk4::Overflow::Hidden);
                let pic = Picture::new();
                pic.set_paintable(Some(&tex));
                pic.set_content_fit(ContentFit::Cover);
                pic.set_size_request(50, 44);
                frame.set_child(Some(&pic));
                col.append(&frame);
            } else {
                let kind = if item.is_dir {
                    UiVectorIcon::Folder
                } else if item.is_image {
                    UiVectorIcon::ImageFile
                } else if item.is_archive {
                    UiVectorIcon::ArchiveFile
                } else {
                    UiVectorIcon::DocFile
                };
                let ic_canvas = DrawingArea::new();
                ic_canvas.set_size_request(44, 44);
                ic_canvas.set_halign(Align::Center);
                ic_canvas.set_valign(Align::Center);
                ic_canvas.set_can_target(false);
                ic_canvas.set_draw_func(move |_, cr, w, h| {
                    cr.set_antialias(cairo::Antialias::Best);
                    crate::island_visuals::rounded_rect_path(
                        cr,
                        2.0,
                        2.0,
                        f64::from(w) - 4.0,
                        f64::from(h) - 4.0,
                        10.0,
                    );
                    cr.set_source_rgba(0.20, 0.68, 0.90, 0.16);
                    let _ = cr.fill();
                    draw_ui_vector_icon(
                        cr,
                        f64::from(w),
                        f64::from(h),
                        kind,
                        (0.39, 0.82, 1.0, 0.98),
                    );
                });
                col.append(&ic_canvas);
            }

            let name = Label::new(Some(&item.name));
            name.add_css_class("neon-notch-file-name");
            name.set_max_width_chars(10);
            name.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);

            let sz = Label::new(Some(&if item.is_dir {
                "Folder".into()
            } else {
                format_file_size(item.size_bytes)
            }));
            sz.add_css_class("neon-island-muted-xs");

            col.append(&name);
            col.append(&sz);
            tile.set_child(Some(&col));

            // Seamless OnePlus File Dock Outgoing Drag (`DragSource`):
            // Exports both `gdk::FileList` (`text/uri-list`) AND `gdk::Texture` (`image/png` for images),
            // attaches a crisp 56×56 preview badge under the cursor, and immediately tucks the notch &
            // click-catcher out of the way while keeping the widget rooted so any window underneath receives the drop!
            let drag_src = DragSource::new();
            drag_src.set_actions(gdk::DragAction::COPY);
            if let Some(drag_tex) = build_file_drag_icon_texture(&item) {
                drag_src.set_icon(Some(&drag_tex), 28, 28);
            }
            let p_drag = item.path.clone();
            let is_img_drag = item.is_image;
            drag_src.connect_prepare(move |_, _, _| {
                let gfile = gio::File::for_path(&p_drag);
                let flist = gdk::FileList::from_array(&[gfile]);
                let file_prov = gdk::ContentProvider::for_value(&flist.to_value());
                if is_img_drag
                    && let Ok(tex) = gdk::Texture::from_filename(&p_drag)
                {
                    let tex_prov = gdk::ContentProvider::for_value(&tex.to_value());
                    return Some(gdk::ContentProvider::new_union(&[file_prov, tex_prov]));
                }
                Some(file_prov)
            });
            {
                let di_b = self.clone();
                drag_src.connect_drag_begin(move |_, _| {
                    {
                        let mut st = di_b.state.borrow_mut();
                        st.outgoing_drag = true;
                        st.pinned_open = false;
                        st.hovered = false;
                        st.peek_over_obscured = false;
                        st.drag_hover = false;
                    }
                    if gtk4_layer_shell::is_supported() {
                        di_b.window.set_keyboard_mode(KeyboardMode::None);
                    }
                    di_b.backdrop_window.set_visible(false);
                    di_b.sync_ui();
                });
            }
            {
                let di_e = self.clone();
                drag_src.connect_drag_end(move |_, _, _| {
                    {
                        let mut st = di_e.state.borrow_mut();
                        st.outgoing_drag = false;
                        st.desktop_drag_active = false;
                    }
                    di_e.sync_ui();
                });
            }
            {
                let di_c = self.clone();
                drag_src.connect_drag_cancel(move |_, _, _| {
                    {
                        let mut st = di_c.state.borrow_mut();
                        st.outgoing_drag = false;
                        st.desktop_drag_active = false;
                    }
                    di_c.sync_ui();
                    false
                });
            }
            tile.add_controller(drag_src);

            let di = self.clone();
            let p_click = item.path.clone();
            tile.connect_clicked(move |_| {
                {
                    let mut st = di.state.borrow_mut();
                    st.shelf_selected_path = Some(p_click.clone());
                    st.shelf_status_msg = None;
                }
                di.refresh_file_shelf_ui();
            });
            card_overlay.set_child(Some(&tile));

            if !show_recent {
                let rm_btn = build_round_close_btn(18, "Remove from File Dock");
                rm_btn.set_halign(Align::End);
                rm_btn.set_valign(Align::Start);
                rm_btn.set_margin_top(3);
                rm_btn.set_margin_end(3);
                let di_rm = self.clone();
                let p_rm = item.path.clone();
                rm_btn.connect_clicked(move |_| {
                    {
                        let mut st = di_rm.state.borrow_mut();
                        st.shelf_items.retain(|i| i.path != p_rm);
                        if st.shelf_selected_path.as_ref() == Some(&p_rm) {
                            st.shelf_selected_path =
                                st.shelf_items.first().map(|i| i.path.clone());
                        }
                        save_persisted_shelf_items(&st.shelf_items);
                    }
                    di_rm.refresh_file_shelf_ui();
                    di_rm.refresh_home_live_ui();
                    di_rm.bg_canvas.queue_draw();
                });
                card_overlay.add_overlay(&rm_btn);
            }

            self.shelf_strip_box.append(&card_overlay);
        }
    }

    // ---- Quick Notes ----

    fn insert_note_snippet(&self, snippet: &str) {
        self.promote_to_pinned();
        let buf = self.notes_editor.buffer();
        buf.insert_at_cursor(snippet);
        self.notes_editor.grab_focus();
    }

    fn refresh_notes_sidebar_only(&self) {
        while let Some(c) = self.notes_list_box.first_child() {
            self.notes_list_box.remove(&c);
        }
        let (notes, active_id) = {
            let st = self.state.borrow();
            (st.notes_store.notes.clone(), st.active_note_id)
        };
        for note in notes {
            let btn = Button::new();
            btn.add_css_class("neon-notch-note-item");
            if note.id == active_id {
                btn.add_css_class("active");
            }
            let col = GtkBox::new(Orientation::Vertical, 1);
            let t = Label::new(Some(&note.title_line()));
            t.add_css_class("neon-notch-store-title");
            t.set_halign(Align::Start);
            t.set_max_width_chars(18);
            t.set_ellipsize(gtk4::pango::EllipsizeMode::End);
            let p = Label::new(Some(&note.preview_line()));
            p.add_css_class("neon-island-muted-xs");
            p.set_halign(Align::Start);
            p.set_max_width_chars(20);
            p.set_ellipsize(gtk4::pango::EllipsizeMode::End);
            col.append(&t);
            col.append(&p);
            btn.set_child(Some(&col));
            let di = self.clone();
            let nid = note.id;
            btn.connect_clicked(move |_| {
                di.state.borrow_mut().active_note_id = nid;
                di.refresh_notes_ui();
            });
            self.notes_list_box.append(&btn);
        }
    }

    fn refresh_notes_ui(&self) {
        self.refresh_notes_sidebar_only();
        let body = {
            let st = self.state.borrow();
            st.notes_store
                .notes
                .iter()
                .find(|n| n.id == st.active_note_id)
                .map(|n| n.body.clone())
                .unwrap_or_default()
        };
        let words = body.split_whitespace().count();
        let chars = body.chars().count();
        self.notes_count_lbl.set_text(&format!("{chars} · {words} words"));
        self.state.borrow_mut().suppress_note_change = true;
        self.notes_editor.buffer().set_text(&body);
        self.state.borrow_mut().suppress_note_change = false;
    }

    // ---- Calendar ----

    fn refresh_calendar_ui(&self) {
        self.cal_month_lbl.set_text(&format_month_day());
        let selected_off = self.state.borrow().cal_selected_day_offset;

        while let Some(c) = self.cal_days_strip.first_child() {
            self.cal_days_strip.remove(&c);
        }
        for off in 0..7 {
            let (dow, day_num, iso) = calendar_day_info(off);
            let has_ev = self.state.borrow().cal_store.has_events_on(&iso);
            let btn = Button::new();
            btn.add_css_class("neon-notch-cal-day");
            if off == 0 {
                btn.add_css_class("today");
            }
            if off == selected_off {
                btn.add_css_class("selected");
            }
            let col = GtkBox::new(Orientation::Vertical, 2);
            col.set_halign(Align::Center);
            let d1 = Label::new(Some(&dow));
            d1.add_css_class("neon-island-muted-xs");
            let d2 = Label::new(Some(&day_num));
            d2.add_css_class("neon-notch-cal-num");
            let dot = GtkBox::new(Orientation::Horizontal, 0);
            dot.add_css_class("neon-notch-cal-dot");
            dot.set_halign(Align::Center);
            dot.set_opacity(if has_ev { 1.0 } else { 0.0 });
            col.append(&d1);
            col.append(&d2);
            col.append(&dot);
            btn.set_child(Some(&col));

            let di = self.clone();
            btn.connect_clicked(move |_| {
                di.state.borrow_mut().cal_selected_day_offset = off;
                di.refresh_calendar_ui();
            });
            self.cal_days_strip.append(&btn);
        }

        while let Some(c) = self.cal_agenda_box.first_child() {
            self.cal_agenda_box.remove(&c);
        }
        let sel_iso = iso_date_with_offset(selected_off);
        let events = self.state.borrow().cal_store.for_date(&sel_iso);
        if events.is_empty() {
            let empty = Label::new(Some("Free — add an event above (e.g. \"Standup 10:30am\")"));
            empty.add_css_class("neon-island-sub-lbl");
            empty.set_halign(Align::Start);
            empty.set_margin_top(12);
            self.cal_agenda_box.append(&empty);
        } else {
            for ev in events {
                let row = GtkBox::new(Orientation::Horizontal, 10);
                row.add_css_class("neon-notch-cal-event");
                row.set_valign(Align::Center);
                let accent_bar = GtkBox::new(Orientation::Vertical, 0);
                accent_bar.add_css_class("neon-notch-cal-bar");
                let time_lbl = Label::new(Some(&ev.time_label));
                time_lbl.add_css_class("neon-island-media-time");
                time_lbl.set_width_chars(8);
                time_lbl.set_xalign(0.0);
                let title_lbl = Label::new(Some(&ev.title));
                title_lbl.add_css_class("neon-island-clip-preview");
                title_lbl.set_hexpand(true);
                title_lbl.set_halign(Align::Start);
                title_lbl.set_ellipsize(gtk4::pango::EllipsizeMode::End);
                let del_btn = build_round_close_btn(22, "Remove event");
                {
                    let di = self.clone();
                    let id = ev.id;
                    del_btn.connect_clicked(move |_| {
                        di.state.borrow_mut().cal_store.remove(id);
                        di.refresh_calendar_ui();
                    });
                }
                row.append(&accent_bar);
                row.append(&time_lbl);
                row.append(&title_lbl);
                row.append(&del_btn);
                self.cal_agenda_box.append(&row);
            }
        }
    }

    // ---- System Stats ----

    fn sample_and_refresh_stats(&self) {
        let snap = self.telemetry_sampler.borrow_mut().sample();
        {
            let mut st = self.state.borrow_mut();
            st.telemetry = snap.clone();
            st.cpu_history.push(snap.cpu_percent);
            if st.cpu_history.len() > 24 {
                st.cpu_history.remove(0);
            }
            st.net_rx_history.push(snap.net_rx_bps);
            if st.net_rx_history.len() > 24 {
                st.net_rx_history.remove(0);
            }
            st.net_tx_history.push(snap.net_tx_bps);
            if st.net_tx_history.len() > 24 {
                st.net_tx_history.remove(0);
            }
        }
        self.stats_cpu_lbl.set_text(&format!("{}%", snap.cpu_percent.round() as i32));
        let temp_txt = match snap.cpu_temp_c {
            Some(c) if c >= 80.0 => format!("Hot · {:.0}°C", c),
            Some(c) if c >= 62.0 => format!("Warm · {:.0}°C", c),
            Some(c) => format!("Cool · {:.0}°C", c),
            None => "Nominal".into(),
        };
        self.stats_cpu_sub.set_text(&temp_txt);

        self.stats_mem_lbl.set_text(&format!("{:.2} GB", snap.mem_used_gb));
        self.stats_mem_sub.set_text(&format!("of {:.0} GB", snap.mem_total_gb));
        let mem_frac = if snap.mem_total_gb > 0.1 {
            (snap.mem_used_gb / snap.mem_total_gb).clamp(0.05, 1.0)
        } else {
            0.1
        };
        let mem_track_w = self.stats_mem_bar.width().max(108) as f32;
        self.stats_mem_fill.set_size_request((mem_track_w * mem_frac).round() as i32, 6);

        self.stats_net_lbl.set_text(&format_net_rate(snap.net_rx_bps));
        self.stats_net_sub.set_text(&format!("{} up", format_net_rate(snap.net_tx_bps)));

        self.stats_disk_lbl.set_text(&format!("{:.1} GB", snap.disk_free_gb));
        let disk_used_frac = if snap.disk_total_gb > 0.1 {
            ((snap.disk_total_gb - snap.disk_free_gb) / snap.disk_total_gb).clamp(0.05, 1.0)
        } else {
            0.1
        };
        let disk_track_w = self.stats_disk_bar.width().max(108) as f32;
        self.stats_disk_fill.set_size_request((disk_track_w * disk_used_frac).round() as i32, 6);

        self.stats_cpu_canvas.queue_draw();
        self.stats_net_canvas.queue_draw();
    }

    // ---- Battery & Bluetooth Devices ----

    fn refresh_battery_ui(&self) {
        let (power, bt) = {
            let st = self.state.borrow();
            (st.sys.power.clone(), st.sys.bluetooth.clone())
        };
        let status = if !power.has_battery {
            "AC Power".to_string()
        } else {
            power.battery_state.label().to_string()
        };
        self.bat_status_lbl.set_text(&status);
        let pct_str = if power.has_battery {
            format!("{}% remaining", power.battery_percentage.round() as i32)
        } else {
            "Desktop power".into()
        };
        self.bat_sub_lbl.set_text(&pct_str);
        self.bat_ring_canvas.queue_draw();

        while let Some(c) = self.bat_profile_row.first_child() {
            self.bat_profile_row.remove(&c);
        }
        let cur_prof = power.active_profile.as_deref().unwrap_or("balanced");
        for (id, label) in [
            ("power-saver", "Saver"),
            ("balanced", "Balanced"),
            ("performance", "Perf"),
        ] {
            let btn = Button::with_label(label);
            btn.add_css_class("neon-island-filter-pill");
            if cur_prof.eq_ignore_ascii_case(id) {
                btn.add_css_class("active");
            }
            let tx = self.sys_tx.clone();
            let p_id = id.to_string();
            btn.connect_clicked(move |_| {
                tx.send(SystemAction::SetPowerProfile(p_id.clone()));
            });
            self.bat_profile_row.append(&btn);
        }

        while let Some(c) = self.bat_devices_box.first_child() {
            self.bat_devices_box.remove(&c);
        }
        let connected: Vec<_> = bt.devices.iter().filter(|d| d.connected).collect();
        if connected.is_empty() {
            let lbl = Label::new(Some(if bt.powered {
                "No Bluetooth devices connected"
            } else {
                "Bluetooth is turned off"
            }));
            lbl.add_css_class("neon-island-sub-lbl");
            lbl.set_halign(Align::Start);
            self.bat_devices_box.append(&lbl);
        } else {
            for dev in connected.into_iter().take(3) {
                let row = GtkBox::new(Orientation::Horizontal, 7);
                row.add_css_class("neon-island-mini-strip");
                let dot = GtkBox::new(Orientation::Horizontal, 0);
                dot.add_css_class("neon-notch-bt-dot");
                dot.set_valign(Align::Center);
                let name = Label::new(Some(&dev.name));
                name.add_css_class("neon-island-clip-preview");
                name.set_hexpand(true);
                name.set_halign(Align::Start);
                name.set_ellipsize(gtk4::pango::EllipsizeMode::End);
                let tag = Label::new(Some("Connected"));
                tag.add_css_class("neon-island-clip-copied-tag");
                row.append(&dot);
                row.append(&name);
                row.append(&tag);
                self.bat_devices_box.append(&row);
            }
        }
    }

    // ---- Keep Awake (`systemd-inhibit`) ----

    fn rebuild_awake_presets(&self) {
        while let Some(c) = self.awake_presets_row.first_child() {
            self.awake_presets_row.remove(&c);
        }
        let choice = self.state.borrow().awake_duration_choice;
        for (secs, label) in [
            (Some(1800u64), "30 min"),
            (Some(3600u64), "1 hour"),
            (Some(7200u64), "2 hours"),
            (None, "Indefinite"),
        ] {
            let btn = Button::with_label(label);
            btn.add_css_class("neon-island-action-pill");
            if choice == secs {
                btn.add_css_class("active");
            }
            let di = self.clone();
            btn.connect_clicked(move |_| {
                {
                    let mut st = di.state.borrow_mut();
                    st.awake_duration_choice = secs;
                    st.awake_remaining_secs = secs;
                    if st.awake_active {
                        di.keep_awake.start(secs);
                    }
                }
                di.rebuild_awake_presets();
                di.refresh_awake_ui();
            });
            self.awake_presets_row.append(&btn);
        }
    }

    fn toggle_keep_awake(&self) {
        let now_active = {
            let mut st = self.state.borrow_mut();
            st.awake_active = !st.awake_active;
            if st.awake_active {
                let dur = st.awake_duration_choice;
                st.awake_remaining_secs = dur;
                self.keep_awake.start(dur);
            } else {
                self.keep_awake.stop();
            }
            st.awake_active
        };
        let _ = now_active;
        self.refresh_awake_ui();
        self.refresh_home_live_ui();
    }

    fn refresh_awake_ui(&self) {
        let (active, rem) = {
            let st = self.state.borrow();
            (st.awake_active, st.awake_remaining_secs)
        };
        self.awake_readout_lbl.set_text(&match rem {
            Some(s) => format_hh_mm_ss(s),
            None => "∞".into(),
        });
        self.awake_sub_lbl.set_text(if active {
            "Preventing idle & sleep (systemd-inhibit active)"
        } else {
            "Stop your system from sleeping for as long as needed"
        });
        self.awake_toggle_btn
            .set_label(if active { "Stop" } else { "Start" });
        if active {
            self.awake_toggle_btn.add_css_class("active");
        } else {
            self.awake_toggle_btn.remove_css_class("active");
        }
    }

    // ---- Audio & Display HUD Sliders ----

    fn refresh_hud_controls_ui(&self) {
        let audio = self.state.borrow().sys.audio.clone();
        self.suppress_hud_sliders.set(true);
        let vol_pct = (audio.volume * 100.0).round().clamp(0.0, 100.0);
        self.hud_vol_scale.set_value(f64::from(vol_pct));
        self.hud_vol_pct.set_text(&if audio.muted {
            "Muted".into()
        } else {
            format!("{}%", vol_pct as i32)
        });
        self.hud_vol_sink.set_text(
            audio
                .default_sink_name
                .as_deref()
                .unwrap_or("Output Audio"),
        );
        if audio.muted {
            self.hud_mute_btn.add_css_class("active");
        } else {
            self.hud_mute_btn.remove_css_class("active");
        }
        self.hud_mute_canvas.queue_draw();
        let br = read_backlight_percent().unwrap_or(80);
        self.hud_bright_scale.set_value(f64::from(br));
        self.hud_bright_pct.set_text(&format!("{br}%"));
        self.suppress_hud_sliders.set(false);
    }

    // ---- 1-Second Periodic Tick ----

    fn tick_one_second(&self) {
        let mut timer_finished = false;
        let mut timer_focus_was = true;
        let mut timer_sound_on = false;
        let mut awake_finished = false;

        let (visible, active_mod, c24, timer_running, awake_active) = {
            let mut st = self.state.borrow_mut();
            if st.timer_running {
                if st.timer_remaining_secs > 1 {
                    st.timer_remaining_secs -= 1;
                } else {
                    st.timer_remaining_secs = 0;
                    st.timer_running = false;
                    timer_finished = true;
                    timer_focus_was = st.timer_focus_mode;
                    timer_sound_on = st.timer_sound;
                }
            }
            if st.awake_active
                && let Some(ref mut rem) = st.awake_remaining_secs
            {
                if *rem > 1 {
                    *rem -= 1;
                } else {
                    *rem = 0;
                    st.awake_active = false;
                    awake_finished = true;
                }
            }
            (
                self.window.is_visible(),
                st.active_module,
                st.clock_24h,
                st.timer_running,
                st.awake_active,
            )
        };

        if awake_finished {
            self.keep_awake.stop();
            self.refresh_awake_ui();
            self.refresh_home_live_ui();
        }
        if timer_finished {
            self.refresh_timer_ui();
            self.refresh_home_live_ui();
            let (title, body) = if timer_focus_was {
                ("Focus Session Complete", "Great work — take a short break.")
            } else {
                ("Break Complete", "Ready to jump back into Focus.")
            };
            self.store.push(
                0,
                "NEON Timer".into(),
                "alarm-symbolic".into(),
                title.into(),
                body.into(),
                NotificationUrgency::Normal,
                6000,
            );
            if timer_sound_on {
                let _ = std::process::Command::new("paplay")
                    .arg("/usr/share/sounds/freedesktop/stereo/complete.oga")
                    .spawn();
            }
            self.sync_ui();
        }

        if visible {
            let t = format_compact_time(c24);
            self.pill_idle_clock.set_text(&t);
            self.home_clock.set_text(&t);
            self.home_weekday.set_text(&format_weekday());
            self.home_date.set_text(&format_month_day());
            if timer_running {
                self.refresh_timer_ui();
            }
            if awake_active {
                self.refresh_awake_ui();
            }
            if active_mod == NotchModule::Stats {
                self.sample_and_refresh_stats();
            }
        }
    }

    // ---- Keyboard Shortcuts inside Pinned Notch ----

    fn handle_key_pressed(&self, key: gdk::Key, mods: gdk::ModifierType) -> glib::Propagation {
        let can_handle = {
            let st = self.state.borrow();
            st.pinned_open || (st.hovered && st.active_module == NotchModule::Tools)
        };
        if !can_handle {
            return glib::Propagation::Proceed;
        }
        if key == gdk::Key::Escape {
            if self.state.borrow().active_module == NotchModule::Tools
                && !self.tools_calc_entry.text().is_empty()
            {
                self.tools_calc_entry.set_text("");
                self.tools_calc_entry.grab_focus();
                return glib::Propagation::Stop;
            }
            self.close_pinned();
            return glib::Propagation::Stop;
        }
        let alt_or_ctrl = mods.contains(gdk::ModifierType::ALT_MASK)
            || mods.contains(gdk::ModifierType::CONTROL_MASK);
        if alt_or_ctrl && let Some(digit) = key_to_digit_1_9(key) {
            let active = self.state.borrow().active_module;
            // Alt+1..9 on Clipboard copies the N-th visible card directly (exact Little Notch shortcut!).
            if active == NotchModule::Clipboard && mods.contains(gdk::ModifierType::ALT_MASK) {
                let entries = self.filtered_clipboard_entries();
                if let Some(entry) = entries.get(digit - 1) {
                    self.copy_clipboard_entry(entry);
                    return glib::Propagation::Stop;
                }
            }
            // Otherwise Alt+1..5 / Ctrl+1..5 switches between pinned dock modules!
            if digit <= 5 {
                let target = self.state.borrow().prefs.pinned.get(digit - 1).copied();
                if let Some(m) = target {
                    self.switch_module(m, true);
                    return glib::Propagation::Stop;
                }
            }
        }
        glib::Propagation::Proceed
    }

    // ---- Hover / Notification / Media Helpers ----

    fn expire_notif_or_defer(&self, id: u32, epoch: u64) {
        {
            let st = self.state.borrow();
            if st.notif_gen != epoch || st.active_notif.as_ref().is_none_or(|n| n.id != id) {
                return;
            }
            if st.hovered {
                drop(st);
                let di = self.clone();
                glib::timeout_add_local_once(Duration::from_millis(900), move || {
                    di.expire_notif_or_defer(id, epoch);
                });
                return;
            }
        }
        self.state.borrow_mut().active_notif = None;
        self.sync_ui();
    }

    fn on_hover_enter(&self) {
        enum HoverAction {
            Ignore,
            Immediate(NotchModule),
            Dwell(u64),
        }
        let action = {
            let mut st = self.state.borrow_mut();
            if st.outgoing_drag {
                return;
            }
            st.hover_gen = st.hover_gen.wrapping_add(1);
            if st.hovered {
                HoverAction::Ignore
            } else if st.is_stealth_sensor() {
                HoverAction::Dwell(st.hover_gen)
            } else {
                st.hovered = true;
                if st.obscured {
                    st.peek_over_obscured = true;
                }
                if st.active_module == NotchModule::Home {
                    if st.timer_running {
                        st.active_module = NotchModule::Timer;
                    } else if st.media.available && st.media.playing {
                        st.active_module = NotchModule::Media;
                    } else if !st.shelf_items.is_empty() && st.desktop_drag_active {
                        st.active_module = NotchModule::Files;
                    }
                }
                HoverAction::Immediate(st.active_module)
            }
        };
        match action {
            HoverAction::Ignore => {}
            HoverAction::Immediate(m) => {
                self.prepare_module_data(m);
                self.rebuild_dock_bar();
                self.sync_ui();
                if m == NotchModule::Tools {
                    self.focus_tools_calculator();
                }
            }
            HoverAction::Dwell(epoch) => {
                let di = self.clone();
                glib::timeout_add_local_once(Duration::from_millis(130), move || {
                    let reveal_mod = {
                        let mut st = di.state.borrow_mut();
                        if st.hover_gen == epoch && !st.outgoing_drag {
                            st.peek_over_obscured = true;
                            st.hovered = true;
                            if !st.shelf_items.is_empty()
                                && !st.timer_running
                                && !(st.media.available && st.media.playing)
                            {
                                st.active_module = NotchModule::Files;
                            }
                            Some(st.active_module)
                        } else {
                            None
                        }
                    };
                    if let Some(m) = reveal_mod {
                        di.prepare_module_data(m);
                        di.rebuild_dock_bar();
                        di.sync_ui();
                        if m == NotchModule::Tools {
                            di.focus_tools_calculator();
                        }
                    }
                });
            }
        }
    }

    fn on_hover_leave(&self) {
        let epoch = {
            let mut st = self.state.borrow_mut();
            st.hover_gen = st.hover_gen.wrapping_add(1);
            if st.test_force_expand
                || st.pinned_open
                || st.media_dragging
                || st.timer_dragging
                || st.drag_hover
                || st.outgoing_drag
            {
                return;
            }
            st.hover_gen
        };
        let di = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(200), move || {
            let should_collapse = {
                let mut st = di.state.borrow_mut();
                if st.hover_gen == epoch
                    && !st.pinned_open
                    && !st.test_force_expand
                    && !st.media_dragging
                    && !st.timer_dragging
                    && !st.drag_hover
                    && !st.outgoing_drag
                {
                    st.hovered = false;
                    if !st.desktop_drag_active {
                        st.peek_over_obscured = false;
                    }
                    true
                } else {
                    false
                }
            };
            if should_collapse {
                if gtk4_layer_shell::is_supported() {
                    di.window.set_keyboard_mode(KeyboardMode::None);
                }
                di.sync_ui();
            }
        });
    }

    fn dispatch_media_cmd(&self, method: &str) {
        let bus = self.state.borrow().media.bus_name.clone();
        self.sys_tx.send(SystemAction::MediaCommand {
            bus_name: bus,
            method: method.into(),
        });
        let sys_tx = self.sys_tx.clone();
        glib::timeout_add_local_once(Duration::from_millis(200), move || {
            sys_tx.send(SystemAction::RefreshNow);
        });
    }

    fn seek_media_to_fraction(&self, frac: f64) {
        let (bus, track_id, target_us, cur_us) = {
            let mut st = self.state.borrow_mut();
            let Some(len) = st.media.length_us.filter(|&l| l > 0) else {
                return;
            };
            let target = ((len as f64) * frac.clamp(0.0, 1.0)).round() as i64;
            let cur = current_media_position_us(&st.media, st.media_seek_override);
            st.media_seek_override = Some((target, Instant::now()));
            (
                st.media.bus_name.clone(),
                st.media.track_id.clone(),
                target,
                cur,
            )
        };
        self.refresh_media_time_labels();
        self.media_progress_area.queue_draw();
        self.sys_tx.send(SystemAction::MediaSeek {
            bus_name: bus,
            track_id,
            position_us: target_us,
            current_position_us: cur_us,
        });
        let sys_tx = self.sys_tx.clone();
        glib::timeout_add_local_once(Duration::from_millis(260), move || {
            sys_tx.send(SystemAction::RefreshNow);
        });
    }

    fn refresh_media_time_labels(&self) {
        let (elapsed_str, rem_str) = {
            let st = self.state.borrow();
            if let Some(len) = st.media.length_us.filter(|&l| l > 0) {
                let pos = if st.media_dragging {
                    ((len as f64) * st.media_drag_frac).round() as i64
                } else {
                    current_media_position_us(&st.media, st.media_seek_override).unwrap_or(0)
                };
                let pos_clamped = pos.clamp(0, len);
                let rem = (len - pos_clamped).max(0);
                (
                    format_duration_us(pos_clamped),
                    format!("-{}", format_duration_us(rem)),
                )
            } else {
                let pos = current_media_position_us(&st.media, st.media_seek_override).unwrap_or(0);
                (format_duration_us(pos), "LIVE".into())
            }
        };
        self.exp_media_elapsed.set_text(&elapsed_str);
        self.exp_media_remaining.set_text(&rem_str);
    }

    // =========================================================================
    // Spring Morph & Layout Engine
    // =========================================================================

    fn compute_target_geometry(
        &self,
        expanded: bool,
        page_name: &str,
        live_pill: Option<&LivePillKind>,
        rec_active: bool,
        timer_running: bool,
        media_active: bool,
        unread: usize,
        clock_24h: bool,
        is_top_flush_notch: bool,
    ) -> (f64, f64, f64, f64) {
        if !expanded {
            if let Some(kind) = live_pill {
                return match kind {
                    LivePillKind::Charging { .. } | LivePillKind::Silent { .. } => {
                        (336.0, 44.0, 22.0, COMPACT_EAR_R)
                    }
                    LivePillKind::Hud { .. } => (324.0, 40.0, 20.0, COMPACT_EAR_R),
                };
            }
            if rec_active {
                return (196.0, 36.0, 18.0, COMPACT_EAR_R);
            }
            if timer_running {
                return (256.0, 36.0, 18.0, COMPACT_EAR_R);
            }
            if media_active {
                return (264.0, 38.0, 19.0, COMPACT_EAR_R);
            }
            let base_w = if clock_24h { 144.0 } else { 158.0 };
            let extra = if unread > 0 { 26.0 } else { 0.0 };
            return (base_w + extra, 36.0, COMPACT_BOTTOM_R, COMPACT_EAR_R);
        }

        let (w, h, r) = match page_name {
            "home" => (496.0, 166.0, EXPANDED_BOTTOM_R),
            "media" => (416.0, 198.0, EXPANDED_BOTTOM_R),
            "timer" => (520.0, 178.0, EXPANDED_BOTTOM_R),
            "clipboard" => (604.0, 248.0, 32.0),
            "files" => (604.0, 232.0, 32.0),
            "notes" => (580.0, 238.0, 32.0),
            "calendar" => (548.0, 254.0, 32.0),
            "tools" => (534.0, 184.0, 32.0),
            "stats" => (588.0, 176.0, 32.0),
            "battery" => (512.0, 176.0, 32.0),
            "awake" => (472.0, 168.0, 32.0),
            "hud" => (468.0, 164.0, 32.0),
            "store" => (628.0, 308.0, 32.0),
            "notif" => {
                let (_, nat_h, _, _) = self.notif_page.measure(Orientation::Vertical, 368);
                let nh = f64::from((((nat_h + 11) / 2) * 2).clamp(74, 152));
                (396.0, nh, (nh * 0.5).min(36.0))
            }
            "recording" => {
                let (_, nat_h, _, _) = self.rec_page.measure(Orientation::Vertical, 348);
                let rh = f64::from((((nat_h + 21) / 2) * 2).clamp(112, 186));
                (380.0, rh, 32.0)
            }
            _ => (496.0, 166.0, EXPANDED_BOTTOM_R),
        };
        let final_r = if is_top_flush_notch { r.min(28.0) } else { r };
        (w, h, final_r, EXPANDED_EAR_R)
    }

    fn retarget_morph(
        &self,
        target_w: f64,
        target_h: f64,
        target_br: f64,
        target_er: f64,
        expanded: bool,
        hide_on_collapse: bool,
    ) {
        let motion_on = self.motion_enabled.get();
        let target_alpha = if expanded { 1.0 } else { 0.0 };

        {
            let mut st = self.state.borrow_mut();
            let was_expanded = st.morph.target_expanded;
            st.morph.hide_on_collapse = hide_on_collapse;

            if !motion_on {
                st.morph.w.snap(target_w);
                st.morph.h.snap(target_h);
                st.morph.br.snap(target_br);
                st.morph.er.snap(target_er);
                st.morph.alpha.snap(target_alpha);
                st.morph.target_expanded = expanded;
                st.cur_palette = st.target_palette;
                st.neon_intensity = st.target_neon_intensity;
                drop(st);
                self.apply_morph_frame(target_w, target_h, target_alpha, expanded);
                if hide_on_collapse && !expanded {
                    self.window.set_visible(false);
                }
                return;
            }

            st.morph.w.set_target(target_w);
            st.morph.h.set_target(target_h);
            st.morph.br.set_target(target_br);
            st.morph.er.set_target(target_er);
            st.morph.alpha.set_target(target_alpha);
            st.morph.target_expanded = expanded;

            if expanded && !was_expanded {
                st.morph.zeta_spatial = 0.72;
                st.morph.omega_spatial = 20.5;
            } else if !expanded && was_expanded {
                st.morph.zeta_spatial = 0.88;
                st.morph.omega_spatial = 23.0;
            } else {
                st.morph.zeta_spatial = 0.78;
                st.morph.omega_spatial = 21.5;
            }
        }

        if expanded {
            self.shell_box.add_css_class("expanded");
            self.expanded_clip.set_visible(true);
        }

        self.ensure_tick_running();
    }

    fn ensure_tick_running(&self) {
        let should_start = {
            let mut st = self.state.borrow_mut();
            if st.morph.tick_running {
                false
            } else {
                st.morph.tick_running = true;
                st.morph.last_tick = Instant::now();
                true
            }
        };
        if !should_start {
            return;
        }

        let di = self.clone();
        self.window.add_tick_callback(move |_, _| {
            let motion_on = di.motion_enabled.get();
            let speed = di.speed_scale.get().clamp(0.5, 2.5);

            let (w, h, alpha, target_expanded, redraw_waves, should_hide, keep_going) = {
                let mut st = di.state.borrow_mut();
                let now = Instant::now();
                let raw_dt = now.duration_since(st.morph.last_tick).as_secs_f64();
                st.morph.last_tick = now;
                let dt = raw_dt.clamp(0.001, 0.05);
                let sp_dt = dt * speed;

                let z = st.morph.zeta_spatial;
                let om = st.morph.omega_spatial;
                let w_act = st.morph.w.step(sp_dt, z, om, 0.14);
                let h_act = st.morph.h.step(sp_dt, z, om, 0.14);
                let br_act = st.morph.br.step(sp_dt, 0.86, 22.0, 0.08);
                let er_act = st.morph.er.step(sp_dt, 0.86, 22.0, 0.08);
                let a_act = st.morph.alpha.step(sp_dt, 0.98, 25.0, 0.004);

                st.morph.w.value = st.morph.w.value.clamp(112.0, f64::from(CANVAS_W - 44));
                st.morph.h.value = st.morph.h.value.clamp(30.0, f64::from(CANVAS_H - DOCK_H - DOCK_GAP - 22));

                let pal_step = (dt * 6.5).clamp(0.0, 1.0);
                let pal_moving = !st.cur_palette.is_close_to(st.target_palette);
                if pal_moving {
                    st.cur_palette = st.cur_palette.lerp(st.target_palette, pal_step);
                    if st.cur_palette.is_close_to(st.target_palette) {
                        st.cur_palette = st.target_palette;
                    }
                }

                let int_diff = st.target_neon_intensity - st.neon_intensity;
                let int_moving = int_diff.abs() > 0.004;
                if int_moving {
                    st.neon_intensity += int_diff * (dt * 7.0).clamp(0.0, 1.0);
                } else {
                    st.neon_intensity = st.target_neon_intensity;
                }

                let media_anim = motion_on && st.media.playing && st.neon_intensity > 0.01;
                let rec_anim = motion_on && st.recording_active;
                if media_anim {
                    st.neon_phase = (st.neon_phase + dt * 1.45) % (std::f64::consts::TAU * 100.0);
                } else if rec_anim {
                    st.neon_phase = (st.neon_phase + dt * 1.8) % (std::f64::consts::TAU * 100.0);
                }

                let spring_active = w_act || h_act || br_act || er_act || a_act;
                let should_hide =
                    !spring_active && st.morph.hide_on_collapse && !st.morph.target_expanded;
                let keep_going = !should_hide
                    && (spring_active || pal_moving || int_moving || media_anim || rec_anim);
                if !keep_going {
                    st.morph.tick_running = false;
                }

                (
                    st.morph.w.value,
                    st.morph.h.value,
                    st.morph.alpha.value,
                    st.morph.target_expanded,
                    media_anim || pal_moving,
                    should_hide,
                    keep_going,
                )
            };

            di.apply_morph_frame(w, h, alpha, target_expanded);
            if redraw_waves {
                di.pill_wave_area.queue_draw();
                di.exp_wave_area.queue_draw();
                di.media_progress_area.queue_draw();
                di.pill_media_fallback.queue_draw();
                di.exp_art_fallback.queue_draw();
            }
            if should_hide {
                di.window.set_visible(false);
                return glib::ControlFlow::Break;
            }
            if keep_going {
                glib::ControlFlow::Continue
            } else {
                glib::ControlFlow::Break
            }
        });
    }

    fn apply_morph_frame(&self, w: f64, h: f64, alpha: f64, target_expanded: bool) {
        // Snap width & height to EVEN integers so `(CANVAS_W - wi) / 2` is ALWAYS an exact integer pixel!
        let wi = (((w * 0.5).round() as i32) * 2).max(112);
        let hi = (((h * 0.5).round() as i32) * 2).max(30);
        if self.shell_box.width_request() != wi || self.shell_box.height_request() != hi {
            self.shell_box.set_size_request(wi, hi);
        }

        let (wants_stealth, stealth_settled, outgoing, show_dock_allowed, top_off) = {
            let st = self.state.borrow();
            let top_off = if st.prefs.appearance.is_top_flush_notch() {
                0
            } else {
                SHELL_TOP_OFFSET
            };
            (
                st.wants_stealth_sensor(),
                st.is_stealth_sensor(),
                st.outgoing_drag,
                st.active_notif.is_none() && !st.recording_active && !st.outgoing_drag,
                top_off,
            )
        };

        if stealth_settled {
            if !self.compact_layer.is_visible() {
                self.compact_layer.set_visible(true);
            }
            self.compact_layer.set_opacity(0.01);
            self.compact_layer.set_can_target(true);
            if self.expanded_clip.is_visible() {
                self.expanded_clip.set_visible(false);
            }
            if self.dock_pill.is_visible() {
                self.dock_pill.set_visible(false);
            }
            self.shell_box.remove_css_class("expanded");
            self.bg_canvas.queue_draw();
            apply_sensor_input_region(&self.window);
            return;
        }

        let raw_compact_op = (1.0 - smoothstep((alpha / 0.44).clamp(0.0, 1.0))).clamp(0.0, 1.0);
        let compact_op = if outgoing {
            0.55
        } else if wants_stealth {
            0.0
        } else if raw_compact_op > 0.99 {
            1.0
        } else if raw_compact_op < 0.01 {
            0.0
        } else {
            raw_compact_op
        };

        let raw_exp_op = smoothstep(((alpha - 0.14) / 0.72).clamp(0.0, 1.0)).clamp(0.0, 1.0);
        let expanded_op = if outgoing {
            0.0
        } else if raw_exp_op > 0.99 {
            1.0
        } else if raw_exp_op < 0.01 {
            0.0
        } else {
            raw_exp_op
        };

        let show_compact = compact_op > 0.005;
        if self.compact_layer.is_visible() != show_compact {
            self.compact_layer.set_visible(show_compact);
        }
        self.compact_layer.set_opacity(compact_op);
        self.compact_layer.set_can_target(!outgoing && compact_op > 0.45);

        // Keep `expanded_clip` mapped (`opacity = 0.0`) during `outgoing_drag` so the active GTK4
        // `DragSource` tile inside `shelf_strip_box` never unroots mid-drag!
        let show_expanded = outgoing || expanded_op > 0.005 || (target_expanded && alpha > 0.02);
        if self.expanded_clip.is_visible() != show_expanded {
            self.expanded_clip.set_visible(show_expanded);
        }
        self.expanded_clip.set_opacity(expanded_op);
        self.expanded_clip.set_can_target(!outgoing && !wants_stealth && expanded_op > 0.35);
        if self.expanded_clip.vadjustment().value() != 0.0 {
            self.expanded_clip.vadjustment().set_value(0.0);
        }

        let dock_op = if show_dock_allowed { expanded_op } else { 0.0 };
        let show_dock = dock_op > 0.02;
        if self.dock_pill.is_visible() != show_dock {
            self.dock_pill.set_visible(show_dock);
        }
        self.dock_pill.set_opacity(dock_op);
        self.dock_pill.set_can_target(!wants_stealth && dock_op > 0.45);

        if target_expanded {
            self.shell_box.add_css_class("expanded");
        } else if alpha <= 0.03 {
            self.shell_box.remove_css_class("expanded");
        }

        self.bg_canvas.queue_draw();
        if show_dock {
            self.dock_bg_canvas.queue_draw();
        }
        if outgoing {
            apply_passthrough_input_region(&self.window);
        } else if wants_stealth {
            apply_sensor_input_region(&self.window);
        } else {
            apply_input_region(&self.window, top_off, f64::from(wi), f64::from(hi), show_dock);
        }
    }

    fn sync_ui(&self) {
        let (
            base_visible,
            wants_stealth,
            stealth_settled,
            outgoing_drag,
            desktop_drag_active,
            pinned_open,
            locked,
            hovered,
            active_module,
            hud_popup,
            rec_active,
            rec_secs,
            timer_running,
            media,
            active_notif,
            notif_gen,
            prev_rendered_notif,
            unread,
            clock_24h,
            art_changed,
            force_exp,
            is_top_flush_notch,
        ) = {
            let mut st = self.state.borrow_mut();
            let allowed = st.combined_mode
                && st.island_enabled
                && !st.locked
                && !st.overview_open
                && !st.workspaces_open;
            let wants_stealth = st.wants_stealth_sensor();
            let stealth_settled = st.is_stealth_sensor();
            let base_vis = allowed
                && (!st.obscured
                    || st.hovered
                    || st.peek_over_obscured
                    || st.desktop_drag_active
                    || st.drag_hover
                    || st.outgoing_drag);
            let art_changed = st.last_art_path != st.media.art_path;
            if art_changed {
                st.last_art_path = st.media.art_path.clone();
            }
            (
                base_vis,
                wants_stealth,
                stealth_settled,
                st.outgoing_drag,
                st.desktop_drag_active,
                st.pinned_open,
                st.locked,
                st.hovered,
                st.active_module,
                st.hud_popup.clone(),
                st.recording_active,
                st.recording_secs,
                st.timer_running,
                st.media.clone(),
                st.active_notif.clone(),
                st.notif_gen,
                st.last_rendered_notif_key,
                st.unread_count,
                st.clock_24h,
                art_changed,
                st.test_force_expand,
                st.prefs.appearance.is_top_flush_notch(),
            )
        };

        let hud_active = hud_popup.is_some();
        let visible =
            !locked && (base_visible || wants_stealth || pinned_open || hud_active || outgoing_drag);

        if !visible {
            let currently_shown = self.window.is_visible();
            let was_expanded = self.state.borrow().morph.target_expanded;
            if currently_shown && was_expanded && self.motion_enabled.get() && !outgoing_drag {
                self.retarget_morph(158.0, 36.0, COMPACT_BOTTOM_R, COMPACT_EAR_R, false, true);
                return;
            }
            {
                let mut st = self.state.borrow_mut();
                st.hovered = false;
                st.peek_over_obscured = false;
                st.morph.hide_on_collapse = false;
                st.morph.target_expanded = false;
                st.morph.alpha.snap(0.0);
                st.neon_intensity = 0.0;
                st.target_neon_intensity = 0.0;
            }
            self.expanded_clip.set_visible(false);
            self.dock_pill.set_visible(false);
            self.shell_box.remove_css_class("expanded");
            self.window.set_visible(false);
            return;
        }

        // Update cached album art texture + extract live neon palette ONLY when art changes.
        if art_changed {
            let texture = media
                .art_path
                .as_deref()
                .filter(|p| Path::new(p).is_file())
                .and_then(|p| gdk::Texture::from_filename(p).ok());
            match texture {
                Some(tex) => {
                    let extracted = extract_neon_palette_from_texture(&tex);
                    self.state.borrow_mut().target_palette = extracted;
                    self.pill_media_pic.set_paintable(Some(&tex));
                    self.pill_media_pic.set_visible(true);
                    self.pill_media_fallback.set_visible(false);
                    self.exp_art_pic.set_paintable(Some(&tex));
                    self.exp_art_pic.set_visible(true);
                    self.exp_art_fallback.set_visible(false);
                }
                None => {
                    self.state.borrow_mut().target_palette = NeonPalette::default();
                    self.pill_media_pic.set_paintable(None::<&gdk::Paintable>);
                    self.pill_media_pic.set_visible(false);
                    self.pill_media_fallback.set_visible(true);
                    self.pill_media_fallback.queue_draw();
                    self.exp_art_pic.set_paintable(None::<&gdk::Paintable>);
                    self.exp_art_pic.set_visible(false);
                    self.exp_art_fallback.set_visible(true);
                    self.exp_art_fallback.queue_draw();
                }
            }
        }

        let media_active = media.available
            && (media.playing || media.title.as_deref().is_some_and(|t| !t.is_empty()));

        // Compact strip priority: Charging/Silent/HUD Pill > Desktop Drag Hint > Recording > Timer > Media > Idle
        self.pill_charge_group.set_visible(false);
        self.pill_silent_group.set_visible(false);
        self.pill_hud_group.set_visible(false);
        self.pill_rec_group.set_visible(false);
        self.pill_timer_group.set_visible(false);
        self.pill_media_group.set_visible(false);
        self.pill_idle_group.set_visible(false);

        if let Some(ref popup) = hud_popup {
            match &popup.kind {
                LivePillKind::Charging { pct, .. } => {
                    self.pill_charge_pct
                        .set_text(&format!("{}%", pct.round().clamp(0.0, 100.0) as i32));
                    self.pill_charge_bat.queue_draw();
                    self.pill_charge_group.set_visible(true);
                }
                LivePillKind::Silent { silent, label } => {
                    self.pill_silent_lbl.set_text(label);
                    if *silent {
                        self.pill_silent_lbl.remove_css_class("muted-off");
                    } else {
                        self.pill_silent_lbl.add_css_class("muted-off");
                    }
                    self.pill_silent_badge.queue_draw();
                    self.pill_silent_group.set_visible(true);
                }
                LivePillKind::Hud {
                    label,
                    fraction,
                    muted,
                    ..
                } => {
                    self.pill_hud_icon.queue_draw();
                    self.pill_hud_label.set_text(label);
                    self.pill_hud_pct.set_text(&if *muted {
                        "Mute".into()
                    } else {
                        format!("{}%", (*fraction * 100.0).round() as i32)
                    });
                    self.pill_hud_bar.queue_draw();
                    self.pill_hud_group.set_visible(true);
                }
            }
        } else if desktop_drag_active && !outgoing_drag {
            self.pill_idle_dot.add_css_class("unread");
            self.pill_idle_clock.set_text("Drop to File Dock");
            self.pill_idle_badge.set_visible(false);
            self.pill_idle_group.set_visible(true);
        } else if rec_active {
            self.pill_rec_label.set_text(&format_mm_ss(rec_secs));
            self.pill_rec_group.set_visible(true);
        } else if timer_running {
            self.pill_timer_group.set_visible(true);
            self.pill_timer_ring.queue_draw();
        } else if media_active {
            let title = media
                .title
                .as_deref()
                .or(media.player_name.as_deref())
                .unwrap_or("Now Playing");
            self.pill_media_label.set_text(title);
            self.pill_media_group.set_visible(true);
        } else {
            self.pill_idle_clock.set_text(&format_compact_time(clock_24h));
            if unread > 0 {
                self.pill_idle_dot.add_css_class("unread");
                self.pill_idle_badge.set_text(&unread.to_string());
                self.pill_idle_badge.set_visible(true);
            } else {
                self.pill_idle_dot.remove_css_class("unread");
                self.pill_idle_badge.set_visible(false);
            }
            self.pill_idle_group.set_visible(true);
        }

        // Update Media card text & vector Play/Pause glyph so switching to Now Playing is always fresh.
        if media_active {
            let title = media
                .title
                .as_deref()
                .filter(|s| !s.is_empty())
                .unwrap_or("Now Playing");
            let sub = media
                .artist
                .as_deref()
                .filter(|s| !s.is_empty())
                .or_else(|| media.album.as_deref().filter(|s| !s.is_empty()))
                .or_else(|| media.player_name.as_deref().filter(|s| !s.is_empty()))
                .unwrap_or("Media");
            self.exp_media_title.set_text(title);
            self.exp_media_sub.set_text(sub);
            self.exp_play_canvas.queue_draw();
            self.refresh_media_time_labels();
        } else {
            self.exp_media_title.set_text("Nothing Playing");
            self.exp_media_sub
                .set_text("Play music or video to control it here");
            self.exp_play_canvas.queue_draw();
        }

        // Expanded page selection (`outgoing_drag` forces compact tuck-away!)
        let should_expand =
            !outgoing_drag && !wants_stealth && (pinned_open || active_notif.is_some() || hovered || force_exp);
        let (page_name, aura_kind) = if let Some(ref item) = active_notif
            && !pinned_open
        {
            let key = (item.id, notif_gen);
            if prev_rendered_notif != Some(key) {
                self.state.borrow_mut().last_rendered_notif_key = Some(key);
                while let Some(c) = self.notif_icon_slot.first_child() {
                    self.notif_icon_slot.remove(&c);
                }
                let ic = build_notification_icon(item, 28);
                ic.set_halign(Align::Center);
                ic.set_valign(Align::Center);
                ic.set_hexpand(true);
                self.notif_icon_slot.append(&ic);
                self.notif_app_lbl.set_text(if item.app_name.is_empty() {
                    "Notification"
                } else {
                    &item.app_name
                });
                self.notif_sum_lbl.set_text(&item.summary);
                if item.body.is_empty() {
                    self.notif_body_lbl.set_visible(false);
                } else {
                    self.notif_body_lbl.set_text(&item.body);
                    self.notif_body_lbl.set_visible(true);
                }
                while let Some(c) = self.notif_actions_box.first_child() {
                    self.notif_actions_box.remove(&c);
                }
                let actions = visible_notification_actions(&item.actions);
                self.notif_actions_box.set_visible(!actions.is_empty());
                for act in actions {
                    let abtn = Button::with_label(&act.label);
                    abtn.add_css_class("neon-island-action-pill");
                    let item_c = item.clone();
                    let key_c = act.key.clone();
                    let di = self.clone();
                    abtn.connect_clicked(move |_| {
                        di.state.borrow_mut().active_notif = None;
                        di.close_pinned();
                        activate_notification(
                            &item_c,
                            Some(&key_c),
                            &di.store,
                            &di.windows.borrow(),
                            &di.ipc,
                        );
                        di.sync_ui();
                    });
                    self.notif_actions_box.append(&abtn);
                }
            }
            self.drawer_stack.set_visible_child_name("notif");
            ("notif", IslandAuraKind::Notification)
        } else if rec_active && !pinned_open {
            self.exp_rec_time_lbl
                .set_text(&format!("{} elapsed", format_mm_ss(rec_secs)));
            if media_active {
                let label = media
                    .title
                    .as_deref()
                    .or(media.player_name.as_deref())
                    .unwrap_or("Media");
                self.exp_rec_media_lbl.set_text(label);
                self.exp_rec_play_canvas.queue_draw();
                self.exp_rec_media_strip.set_visible(true);
            } else {
                self.exp_rec_media_strip.set_visible(false);
            }
            self.drawer_stack.set_visible_child_name("recording");
            ("recording", IslandAuraKind::Recording)
        } else {
            if prev_rendered_notif.is_some() {
                self.state.borrow_mut().last_rendered_notif_key = None;
            }
            let sname = active_module.stack_name();
            self.drawer_stack.set_visible_child_name(sname);
            let aura = if let Some(ref popup) = hud_popup
                && !should_expand
            {
                match popup.kind {
                    LivePillKind::Charging { .. } => IslandAuraKind::Charging,
                    LivePillKind::Silent { .. } => IslandAuraKind::Silent,
                    LivePillKind::Hud { .. } => IslandAuraKind::Hud,
                }
            } else if !should_expand && timer_running {
                IslandAuraKind::Timer
            } else if !should_expand && media_active {
                IslandAuraKind::Media
            } else {
                match active_module {
                    NotchModule::Media => IslandAuraKind::Media,
                    NotchModule::Timer => IslandAuraKind::Timer,
                    NotchModule::Clipboard => IslandAuraKind::Clipboard,
                    NotchModule::Files => IslandAuraKind::FileShelf,
                    _ => IslandAuraKind::Idle,
                }
            };
            (sname, aura)
        };

        // Wandering dark-AMOLED song-color gradient glows across all panels whenever media is playing.
        {
            let mut st = self.state.borrow_mut();
            st.aura_kind = aura_kind;
            let show_neon = !wants_stealth && media_active;
            st.target_neon_intensity = if show_neon {
                if media.playing {
                    1.0
                } else if !should_expand || page_name == "media" {
                    0.35
                } else {
                    0.0
                }
            } else {
                0.0
            };
        }

        let (mut tw, mut th, tbr, ter) = self.compute_target_geometry(
            should_expand,
            page_name,
            hud_popup.as_ref().map(|p| &p.kind),
            rec_active,
            timer_running,
            media_active,
            unread,
            clock_24h,
            is_top_flush_notch,
        );
        if !should_expand && desktop_drag_active && !outgoing_drag && hud_popup.is_none() {
            tw = tw.max(216.0);
            th = th.max(38.0);
        }

        let newly_shown = !self.window.is_visible();
        if newly_shown {
            if !should_expand {
                {
                    let mut st = self.state.borrow_mut();
                    st.morph.w.snap(tw);
                    st.morph.h.snap(th);
                    st.morph.br.snap(tbr);
                    st.morph.er.snap(ter);
                    st.morph.alpha.snap(0.0);
                    st.morph.target_expanded = false;
                }
                self.apply_morph_frame(tw, th, 0.0, false);
            } else {
                {
                    let mut st = self.state.borrow_mut();
                    st.morph.w.snap(158.0);
                    st.morph.h.snap(36.0);
                    st.morph.br.snap(COMPACT_BOTTOM_R);
                    st.morph.er.snap(COMPACT_EAR_R);
                    st.morph.alpha.snap(0.0);
                    st.morph.target_expanded = false;
                }
                self.apply_morph_frame(158.0, 36.0, 0.0, false);
            }
            self.window.set_visible(true);
            self.window.present();
        }

        self.retarget_morph(tw, th, tbr, ter, should_expand, false);
        if stealth_settled || outgoing_drag {
            let (cw, ch, ca, te) = {
                let st = self.state.borrow();
                (
                    st.morph.w.value,
                    st.morph.h.value,
                    st.morph.alpha.value,
                    st.morph.target_expanded,
                )
            };
            self.apply_morph_frame(cw, ch, ca, te);
        }
        self.bg_canvas.queue_draw();
        self.pill_wave_area.queue_draw();
        self.exp_wave_area.queue_draw();
        self.media_progress_area.queue_draw();

        if newly_shown {
            let (cw, ch, a) = {
                let st = self.state.borrow();
                (st.morph.w.value, st.morph.h.value, st.morph.alpha.value)
            };
            let top_off = if is_top_flush_notch {
                0
            } else {
                SHELL_TOP_OFFSET
            };
            if outgoing_drag {
                apply_passthrough_input_region(&self.window);
            } else if wants_stealth {
                apply_sensor_input_region(&self.window);
            } else {
                apply_input_region(&self.window, top_off, cw, ch, a > 0.25);
            }
        }
    }
}

fn attach_vertical_wheel_to_horizontal_scroll(scroller: &ScrolledWindow) {
    let scroll = EventControllerScroll::new(EventControllerScrollFlags::VERTICAL);
    scroll.set_propagation_phase(gtk4::PropagationPhase::Capture);
    let sc = scroller.clone();
    scroll.connect_scroll(move |ctrl, _dx, dy| {
        if dy.abs() <= 0.001 {
            return glib::Propagation::Proceed;
        }
        let hadj = sc.hadjustment();
        let max_val = (hadj.upper() - hadj.page_size()).max(hadj.lower());
        if max_val <= hadj.lower() {
            return glib::Propagation::Proceed;
        }
        let delta = match ctrl.unit() {
            gdk::ScrollUnit::Surface => dy,
            _ => dy * 112.0,
        };
        let next = (hadj.value() + delta).clamp(hadj.lower(), max_val);
        hadj.set_value(next);
        glib::Propagation::Stop
    });
    scroller.add_controller(scroll);
}

fn build_vector_ui_canvas(
    kind: UiVectorIcon,
    size: i32,
    rgba: (f64, f64, f64, f64),
) -> DrawingArea {
    let area = DrawingArea::new();
    area.set_size_request(size, size);
    area.set_halign(Align::Center);
    area.set_valign(Align::Center);
    area.set_can_target(false);
    area.set_draw_func(move |_, cr, w, h| {
        draw_ui_vector_icon(cr, f64::from(w), f64::from(h), kind, rgba);
    });
    area
}

fn build_module_icon_canvas(
    module: NotchModule,
    size: i32,
    rgba: (f64, f64, f64, f64),
) -> DrawingArea {
    let area = DrawingArea::new();
    area.set_size_request(size, size);
    area.set_halign(Align::Center);
    area.set_valign(Align::Center);
    area.set_can_target(false);
    area.set_draw_func(move |_, cr, w, h| {
        draw_module_vector_icon(cr, f64::from(w), f64::from(h), module, rgba);
    });
    area
}

fn build_module_badge_canvas(module: NotchModule, size: i32) -> DrawingArea {
    let area = DrawingArea::new();
    area.set_size_request(size, size);
    area.set_halign(Align::Center);
    area.set_valign(Align::Center);
    area.set_can_target(false);
    area.set_draw_func(move |_, cr, w, h| {
        draw_module_badge_tile(cr, f64::from(w), f64::from(h), module);
    });
    area
}

fn build_round_pin_btn(pinned: bool, size: i32, accent: Rgb, tooltip: &str) -> Button {
    let btn = Button::new();
    btn.add_css_class("neon-ios-frameless-btn");
    btn.set_size_request(size, size);
    btn.set_halign(Align::Center);
    btn.set_valign(Align::Center);
    btn.set_tooltip_text(Some(tooltip));

    let hovered = Rc::new(Cell::new(false));
    let canvas = DrawingArea::new();
    canvas.set_size_request(size, size);
    canvas.set_halign(Align::Center);
    canvas.set_valign(Align::Center);
    canvas.set_can_target(false);

    let hov_draw = Rc::clone(&hovered);
    canvas.set_draw_func(move |_, cr, w, h| {
        draw_round_pin_button(cr, f64::from(w), f64::from(h), pinned, hov_draw.get(), accent);
    });

    let h_in = Rc::clone(&hovered);
    let c_in = canvas.clone();
    let h_out = Rc::clone(&hovered);
    let c_out = canvas.clone();
    let mot = EventControllerMotion::new();
    mot.connect_enter(move |_, _, _| {
        h_in.set(true);
        c_in.queue_draw();
    });
    mot.connect_leave(move |_| {
        h_out.set(false);
        c_out.queue_draw();
    });
    btn.add_controller(mot);
    btn.set_child(Some(&canvas));
    btn
}

fn build_round_close_btn(size: i32, tooltip: &str) -> Button {
    let btn = Button::new();
    btn.add_css_class("neon-ios-frameless-btn");
    btn.set_size_request(size, size);
    btn.set_halign(Align::Center);
    btn.set_valign(Align::Center);
    btn.set_tooltip_text(Some(tooltip));

    let hovered = Rc::new(Cell::new(false));
    let canvas = DrawingArea::new();
    canvas.set_size_request(size, size);
    canvas.set_halign(Align::Center);
    canvas.set_valign(Align::Center);
    canvas.set_can_target(false);

    let hov_draw = Rc::clone(&hovered);
    canvas.set_draw_func(move |_, cr, w, h| {
        let (bg, fg) = if hov_draw.get() {
            ((1.0, 0.23, 0.19, 0.92), (1.0, 1.0, 1.0, 0.98))
        } else {
            ((0.20, 0.20, 0.22, 0.92), (0.72, 0.72, 0.75, 0.95))
        };
        draw_round_circle_button(cr, f64::from(w), f64::from(h), bg, fg, UiVectorIcon::Close);
    });

    let h_in = Rc::clone(&hovered);
    let c_in = canvas.clone();
    let h_out = Rc::clone(&hovered);
    let c_out = canvas.clone();
    let mot = EventControllerMotion::new();
    mot.connect_enter(move |_, _, _| {
        h_in.set(true);
        c_in.queue_draw();
    });
    mot.connect_leave(move |_| {
        h_out.set(false);
        c_out.queue_draw();
    });
    btn.add_controller(mot);
    btn.set_child(Some(&canvas));
    btn
}

fn build_notif_circle_btn(is_accept: bool, tooltip: &str) -> Button {
    let btn = Button::new();
    btn.add_css_class("neon-ios-frameless-btn");
    btn.set_size_request(38, 38);
    btn.set_halign(Align::Center);
    btn.set_valign(Align::Center);
    btn.set_tooltip_text(Some(tooltip));

    let hovered = Rc::new(Cell::new(false));
    let canvas = DrawingArea::new();
    canvas.set_size_request(38, 38);
    canvas.set_halign(Align::Center);
    canvas.set_valign(Align::Center);
    canvas.set_can_target(false);

    let hov_draw = Rc::clone(&hovered);
    canvas.set_draw_func(move |_, cr, w, h| {
        let bg = if is_accept {
            if hov_draw.get() {
                (0.24, 0.86, 0.42, 1.0)
            } else {
                (0.20, 0.78, 0.35, 1.0)
            }
        } else if hov_draw.get() {
            (1.0, 0.30, 0.25, 1.0)
        } else {
            (1.0, 0.23, 0.19, 1.0)
        };
        let icon = if is_accept {
            UiVectorIcon::Check
        } else {
            UiVectorIcon::Close
        };
        draw_round_circle_button(
            cr,
            f64::from(w),
            f64::from(h),
            bg,
            (1.0, 1.0, 1.0, 0.98),
            icon,
        );
    });

    let h_in = Rc::clone(&hovered);
    let c_in = canvas.clone();
    let h_out = Rc::clone(&hovered);
    let c_out = canvas.clone();
    let mot = EventControllerMotion::new();
    mot.connect_enter(move |_, _, _| {
        h_in.set(true);
        c_in.queue_draw();
    });
    mot.connect_leave(move |_| {
        h_out.set(false);
        c_out.queue_draw();
    });
    btn.add_controller(mot);
    btn.set_child(Some(&canvas));
    btn
}

fn build_tool_tile_ui(kind: UiVectorIcon, label: &str) -> Button {
    let btn = Button::new();
    btn.add_css_class("neon-notch-tool-tile");
    let col = GtkBox::new(Orientation::Vertical, 5);
    col.set_halign(Align::Center);
    col.set_valign(Align::Center);
    let ic = build_vector_ui_canvas(kind, 22, (1.0, 1.0, 1.0, 0.96));
    let lbl = Label::new(Some(label));
    lbl.add_css_class("neon-notch-store-title");
    col.append(&ic);
    col.append(&lbl);
    btn.set_child(Some(&col));
    btn
}

fn build_tool_tile_mod(module: NotchModule, label: &str) -> Button {
    let btn = Button::new();
    btn.add_css_class("neon-notch-tool-tile");
    let col = GtkBox::new(Orientation::Vertical, 5);
    col.set_halign(Align::Center);
    col.set_valign(Align::Center);
    let ic = build_module_icon_canvas(module, 22, (1.0, 1.0, 1.0, 0.96));
    let lbl = Label::new(Some(label));
    lbl.add_css_class("neon-notch-store-title");
    col.append(&ic);
    col.append(&lbl);
    btn.set_child(Some(&col));
    btn
}

/// Renders a crisp `56×56` rounded drag-preview badge (`gdk::MemoryTexture`) for outgoing File Dock drags.
fn build_file_drag_icon_texture(item: &FileShelfItem) -> Option<gdk::MemoryTexture> {
    const SZ: i32 = 56;
    let surf = cairo::ImageSurface::create(cairo::Format::ARgb32, SZ, SZ).ok()?;
    {
        let cr = cairo::Context::new(&surf).ok()?;
        cr.set_antialias(cairo::Antialias::Best);
        crate::island_visuals::rounded_rect_path(&cr, 2.0, 2.0, 52.0, 52.0, 13.0);
        cr.set_source_rgba(0.11, 0.11, 0.12, 0.96);
        let _ = cr.fill();

        let mut drew_photo = false;
        if item.is_image
            && let Ok(tex) = gdk::Texture::from_filename(&item.path)
        {
            let tw = tex.width();
            let th = tex.height();
            if tw > 0 && th > 0 {
                let mut dl = gdk::TextureDownloader::new(&tex);
                dl.set_format(gdk::MemoryFormat::B8g8r8a8Premultiplied);
                let (bytes, stride) = dl.download_bytes();
                if let Ok(img_surf) = cairo::ImageSurface::create_for_data(
                    bytes.as_ref().to_vec(),
                    cairo::Format::ARgb32,
                    tw,
                    th,
                    stride as i32,
                ) {
                    let _ = cr.save();
                    crate::island_visuals::rounded_rect_path(&cr, 4.0, 4.0, 48.0, 48.0, 10.0);
                    cr.clip();
                    let scale = (48.0 / f64::from(tw)).max(48.0 / f64::from(th));
                    let ox = 4.0 + (48.0 - f64::from(tw) * scale) * 0.5;
                    let oy = 4.0 + (48.0 - f64::from(th) * scale) * 0.5;
                    cr.translate(ox, oy);
                    cr.scale(scale, scale);
                    let _ = cr.set_source_surface(&img_surf, 0.0, 0.0);
                    let _ = cr.paint();
                    let _ = cr.restore();
                    drew_photo = true;
                }
            }
        }

        if !drew_photo {
            let kind = if item.is_dir {
                UiVectorIcon::Folder
            } else if item.is_image {
                UiVectorIcon::ImageFile
            } else if item.is_archive {
                UiVectorIcon::ArchiveFile
            } else {
                UiVectorIcon::DocFile
            };
            draw_ui_vector_icon(&cr, 56.0, 56.0, kind, (0.39, 0.82, 1.0, 0.98));
        }

        crate::island_visuals::rounded_rect_path(&cr, 2.5, 2.5, 51.0, 51.0, 12.5);
        cr.set_source_rgba(1.0, 1.0, 1.0, 0.28);
        cr.set_line_width(1.2);
        let _ = cr.stroke();
    }
    surf.flush();
    let mut data = Vec::with_capacity((SZ * SZ * 4) as usize);
    surf.with_data(|slice| data.extend_from_slice(slice)).ok()?;
    let gbytes = glib::Bytes::from_owned(data);
    Some(gdk::MemoryTexture::new(
        SZ,
        SZ,
        gdk::MemoryFormat::B8g8r8a8Premultiplied,
        &gbytes,
        (SZ * 4) as usize,
    ))
}

/// Evaluates whether a true fullscreen window is active on the current workspace.
pub(crate) fn is_island_obscured_by_fullscreen(snapshot: &DesktopSnapshot) -> bool {
    let active_ws_id = snapshot
        .workspaces
        .iter()
        .find(|ws| ws.active)
        .map(|ws| ws.id);
    snapshot.windows.iter().any(|w| {
        !w.minimized
            && active_ws_id.is_none_or(|ws_id| w.workspace_id == ws_id)
            && w.fullscreen
    })
}

/// Evaluates whether any visible window on the active workspace obscures the top-center notch.
pub(crate) fn is_island_obscured_by_windows(snapshot: &DesktopSnapshot) -> bool {
    let active_ws_id = snapshot
        .workspaces
        .iter()
        .find(|ws| ws.active)
        .map(|ws| ws.id);
    let out_w = snapshot
        .outputs
        .iter()
        .find(|o| o.primary)
        .or_else(|| snapshot.outputs.first())
        .map(|o| o.width.max(800))
        .unwrap_or(1920);
    let island_left = (out_w - 380) / 2;
    let island_right = island_left + 380;
    const ISLAND_TOP_BAND: i32 = 54;

    snapshot.windows.iter().any(|w| {
        if w.minimized {
            return false;
        }
        if active_ws_id.is_some_and(|ws_id| w.workspace_id != ws_id) {
            return false;
        }
        if w.fullscreen || w.maximized {
            return true;
        }
        let win_right = w.x + w.width;
        let win_bottom = w.y + w.height;
        w.y < ISLAND_TOP_BAND && win_bottom > 0 && w.x < island_right && win_right > island_left
    })
}

fn current_media_position_us(
    media: &MediaStatus,
    seek_override: Option<(i64, Instant)>,
) -> Option<i64> {
    if let Some((override_pos, set_at)) = seek_override
        && set_at.elapsed() < Duration::from_secs(3)
    {
        let extra = if media.playing {
            set_at.elapsed().as_micros() as i64
        } else {
            0
        };
        let pos = override_pos.saturating_add(extra);
        return Some(match media.length_us {
            Some(len) if len > 0 => pos.clamp(0, len),
            _ => pos.max(0),
        });
    }

    let base = media.position_us?;
    let extra = if media.playing && media.observed_unix_ms > 0 {
        let now_ms = now_unix_ms();
        now_ms.saturating_sub(media.observed_unix_ms).min(10_000) as i64 * 1_000
    } else {
        0
    };
    let pos = base.saturating_add(extra);
    Some(match media.length_us {
        Some(len) if len > 0 => pos.clamp(0, len),
        _ => pos.max(0),
    })
}

fn compute_media_progress_fraction(
    media: &MediaStatus,
    seek_override: Option<(i64, Instant)>,
) -> f64 {
    let Some(len) = media.length_us.filter(|&l| l > 0) else {
        return if media.playing { 1.0 } else { 0.0 };
    };
    let pos = current_media_position_us(media, seek_override).unwrap_or(0);
    (pos as f64 / len as f64).clamp(0.0, 1.0)
}

fn format_duration_us(us: i64) -> String {
    let total_secs = (us.max(0) / 1_000_000) as u64;
    let hrs = total_secs / 3600;
    let mins = (total_secs % 3600) / 60;
    let secs = total_secs % 60;
    if hrs > 0 {
        format!("{hrs}:{mins:02}:{secs:02}")
    } else {
        format!("{mins}:{secs:02}")
    }
}

fn format_mm_ss(secs: u64) -> String {
    format!("{:02}:{:02}", secs / 60, secs % 60)
}

fn format_hh_mm_ss(secs: u64) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    format!("{h}:{m:02}:{s:02}")
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn smoothstep(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn key_to_digit_1_9(key: gdk::Key) -> Option<usize> {
    match key {
        gdk::Key::_1 | gdk::Key::KP_1 => Some(1),
        gdk::Key::_2 | gdk::Key::KP_2 => Some(2),
        gdk::Key::_3 | gdk::Key::KP_3 => Some(3),
        gdk::Key::_4 | gdk::Key::KP_4 => Some(4),
        gdk::Key::_5 | gdk::Key::KP_5 => Some(5),
        gdk::Key::_6 | gdk::Key::KP_6 => Some(6),
        gdk::Key::_7 | gdk::Key::KP_7 => Some(7),
        gdk::Key::_8 | gdk::Key::KP_8 => Some(8),
        gdk::Key::_9 | gdk::Key::KP_9 => Some(9),
        _ => None,
    }
}

fn apply_sensor_input_region(window: &ApplicationWindow) {
    if let Some(surf) = window.surface() {
        // Slim 260×5px top-center sensor strip so users can drag files onto the File Dock
        // or dwell-peek the notch even over maximized windows without blocking app headerbars!
        let sw = 260;
        let sx = (CANVAS_W - sw) / 2;
        let rect = cairo::RectangleInt::new(sx, 0, sw, 5);
        let region = cairo::Region::create_rectangle(&rect);
        surf.set_input_region(Some(&region));
    }
}

fn apply_passthrough_input_region(window: &ApplicationWindow) {
    if let Some(surf) = window.surface() {
        // Empty input region during outgoing File Dock drags so 100% of the screen underneath
        // (including maximized browser tab bars!) can receive the drop cleanly.
        let region = cairo::Region::create();
        surf.set_input_region(Some(&region));
    }
}

fn apply_input_region(
    window: &ApplicationWindow,
    top_offset: i32,
    w: f64,
    h: f64,
    include_dock: bool,
) {
    if let Some(surf) = window.surface() {
        let ear_pad = if top_offset == 0 { 22 } else { 8 };
        let iw = (w.ceil() as i32 + ear_pad * 2).clamp(112, CANVAS_W);
        let iy = 0;
        let ih = (top_offset + h.ceil() as i32 + 8).clamp(36, CANVAS_H);
        let ix = ((CANVAS_W - iw) / 2).max(0);
        let top_rect = cairo::RectangleInt::new(ix, iy, iw, ih);
        let region = cairo::Region::create_rectangle(&top_rect);
        if include_dock {
            // Bridge the gap + floating bottom dock pill so cursor never triggers leave!
            let dock_w = 320.min(CANVAS_W);
            let dock_x = (CANVAS_W - dock_w) / 2;
            let dock_y = (top_offset + h.floor() as i32 - 2).max(0);
            let dock_h = (DOCK_GAP + DOCK_H + 8).min(CANVAS_H - dock_y).max(1);
            let dock_rect = cairo::RectangleInt::new(dock_x, dock_y, dock_w, dock_h);
            let _ = region.union_rectangle(&dock_rect);
        }
        surf.set_input_region(Some(&region));
    }
}

fn format_compact_time(clock_24h: bool) -> String {
    glib::DateTime::now_local()
        .ok()
        .and_then(|dt| {
            let fmt = if clock_24h { "%H:%M" } else { "%I:%M %p" };
            dt.format(fmt).ok()
        })
        .map(|s| s.to_string())
        .unwrap_or_else(|| "12:00".to_string())
}

fn format_weekday() -> String {
    glib::DateTime::now_local()
        .ok()
        .and_then(|dt| dt.format("%A").ok())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "Today".to_string())
}

fn format_month_day() -> String {
    glib::DateTime::now_local()
        .ok()
        .and_then(|dt| dt.format("%B %-d").ok())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "Today".to_string())
}

fn iso_date_with_offset(day_offset: i32) -> String {
    glib::DateTime::now_local()
        .ok()
        .and_then(|dt| dt.add_days(day_offset).ok())
        .and_then(|dt| dt.format("%Y-%m-%d").ok())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "2026-01-01".to_string())
}

fn calendar_day_info(day_offset: i32) -> (String, String, String) {
    if let Ok(now) = glib::DateTime::now_local()
        && let Ok(dt) = now.add_days(day_offset)
    {
        let dow = dt
            .format("%a")
            .map(|s| s.to_string().to_ascii_uppercase())
            .unwrap_or_else(|_| "DAY".into());
        let num = dt
            .format("%-d")
            .map(|s| s.to_string())
            .unwrap_or_else(|_| "1".into());
        let iso = dt
            .format("%Y-%m-%d")
            .map(|s| s.to_string())
            .unwrap_or_else(|_| "2026-01-01".into());
        return (dow, num, iso);
    }
    ("DAY".into(), "1".into(), "2026-01-01".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use neon_ipc::{AppearanceInfo, EffectsInfo, WindowBehaviorInfo, WorkspaceInfo};

    fn make_snap(windows: Vec<WindowInfo>) -> DesktopSnapshot {
        DesktopSnapshot {
            active_workspace_index: 0,
            overview_open: false,
            locked: false,
            windows,
            workspaces: vec![
                WorkspaceInfo {
                    id: 1,
                    index: 0,
                    active: true,
                    pinned: false,
                    window_ids: vec![1],
                },
                WorkspaceInfo {
                    id: 2,
                    index: 1,
                    active: false,
                    pinned: false,
                    window_ids: vec![2],
                },
            ],
            outputs: vec![],
            effects: EffectsInfo::default(),
            appearance: AppearanceInfo::default(),
            window_behavior: WindowBehaviorInfo {
                focus_follows_mouse: false,
                snap_gap: 8,
                button_layout: "left_traffic_lights".into(),
                ..Default::default()
            },
            xwayland_display: None,
        }
    }

    fn base_win(id: u64, ws_id: u64, x: i32, y: i32, w: i32, h: i32) -> WindowInfo {
        WindowInfo {
            id,
            title: Some("App".into()),
            app_id: Some("app".into()),
            workspace_id: ws_id,
            focused: true,
            minimized: false,
            maximized: false,
            fullscreen: false,
            tiled: false,
            always_on_top: false,
            x,
            y,
            width: w,
            height: h,
        }
    }

    #[test]
    fn island_hides_on_fullscreen_maximized_or_covering_window_on_active_workspace() {
        let centered = make_snap(vec![base_win(1, 1, 240, 120, 800, 560)]);
        assert!(!is_island_obscured_by_windows(&centered));

        let mut max_win = base_win(1, 1, 0, 0, 1920, 1028);
        max_win.maximized = true;
        assert!(is_island_obscured_by_windows(&make_snap(vec![max_win])));

        let mut fs_win = base_win(1, 1, 0, 0, 1920, 1080);
        fs_win.fullscreen = true;
        assert!(is_island_obscured_by_windows(&make_snap(vec![fs_win])));

        let covering_top = base_win(1, 1, 780, 12, 420, 320);
        assert!(is_island_obscured_by_windows(&make_snap(vec![covering_top])));

        let mut other_ws_fs = base_win(2, 2, 0, 0, 1920, 1080);
        other_ws_fs.fullscreen = true;
        assert!(!is_island_obscured_by_windows(&make_snap(vec![other_ws_fs])));
    }

    #[test]
    fn duration_and_timer_formatters_work_cleanly() {
        assert_eq!(format_duration_us(0), "0:00");
        assert_eq!(format_duration_us(84_000_000), "1:24");
        assert_eq!(format_duration_us(3_665_000_000), "1:01:05");
        assert_eq!(format_mm_ss(1505), "25:05");
        assert_eq!(format_hh_mm_ss(7199), "1:59:59");
    }
}
