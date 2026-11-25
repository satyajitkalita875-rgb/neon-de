//! Unified macOS-Style Right-Click Context Menu System for **NEON DE**.
//!
//! Architecture:
//! - Three `zwlr_layer_shell_v1` `Layer::Overlay` surfaces sharing one consistent frosted Liquid Glass
//!   visual design across the entire desktop (Desktop, Files & Folders, Dock, Windows, Panels,
//!   System Tray DBusMenu, and App Drawer):
//!   1. `neon-context-menu-backdrop` — fullscreen click-catcher dismissing menus on outside click
//!   2. `neon-context-menu` — compact floating rounded menu card with real compositor GPU backdrop blur
//!      (`13px` corners inside `14px` unclipped drop-shadow padding) and `135ms` pop-in animation
//!   3. `neon-context-submenu` — cascading submenu with identical GPU blur, smart left/right edge
//!      flipping, and hover-intent / arrow-key navigation
//! - Full keyboard navigation (`Up`, `Down`, `Home`, `End`, `Right`, `Left`, `Enter`, `Space`, `Esc`)
//!   synchronized seamlessly with pointer hover
//! - Multi-monitor & screen-edge aware clamping against the active output bounds

use std::{
    cell::{Cell, RefCell},
    path::{Path, PathBuf},
    rc::Rc,
    time::Duration,
};

use gtk4::{
    Align, Application, ApplicationWindow, Box as GtkBox, Button, EventControllerKey,
    EventControllerMotion, GestureClick, Image, Label, Orientation, Separator, gdk, glib,
    prelude::*,
};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use neon_ipc::{
    AppearanceUpdate, ContextMenuTarget, DesktopSnapshot, IpcRequest, OutputInfo, WindowInfo,
};
use neon_system::{
    SystemAction, SystemActionSender, SystemSnapshot,
    tray::{TrayAction, TrayActionSender, TrayMenuItem},
};
use tracing::warn;

use crate::{
    desktop_apps::{
        DesktopApp, find_apps_for_file, is_launch_at_login, resolve_window_desktop_app,
        select_favorites_with_pins, set_launch_at_login,
    },
    file_ops::{
        ArchiveFormat, FileClipboard, FileClipboardOp, compress_path, copy_text_to_clipboard,
        desktop_dir, duplicate_path, extract_archive, is_archive, move_to_trash, open_path,
        open_terminal_in, show_delete_confirm_dialog, show_new_folder_dialog,
        show_properties_dialog, show_rename_dialog,
    },
    ipc_bridge::IpcCommandSender,
};

type MenuCallback = Rc<dyn Fn()>;
type VoidHook = Rc<RefCell<Option<Box<dyn Fn()>>>>;

/// Specific Top Bar region right-clicked so panel menus stay deeply context-aware.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelMenuZone {
    General,
    Workspaces,
    Battery,
    QuickSettings,
    Clock,
}

/// Unified item model shared by every NEON DE context menu and cascading submenu.
pub enum ContextMenuItem {
    Header(String),
    Separator,
    Action {
        icon: Option<String>,
        label: String,
        shortcut: Option<String>,
        checked: Option<bool>,
        radio: bool,
        enabled: bool,
        destructive: bool,
        on_click: MenuCallback,
    },
    Submenu {
        icon: Option<String>,
        label: String,
        enabled: bool,
        items: Vec<ContextMenuItem>,
    },
}

impl ContextMenuItem {
    pub fn action<F: Fn() + 'static>(icon: &str, label: impl Into<String>, on_click: F) -> Self {
        Self::Action {
            icon: (!icon.is_empty()).then(|| icon.to_string()),
            label: label.into(),
            shortcut: None,
            checked: None,
            radio: false,
            enabled: true,
            destructive: false,
            on_click: Rc::new(on_click),
        }
    }

    pub fn action_sc<F: Fn() + 'static>(
        icon: &str,
        label: impl Into<String>,
        shortcut: &str,
        on_click: F,
    ) -> Self {
        Self::Action {
            icon: (!icon.is_empty()).then(|| icon.to_string()),
            label: label.into(),
            shortcut: Some(shortcut.to_string()),
            checked: None,
            radio: false,
            enabled: true,
            destructive: false,
            on_click: Rc::new(on_click),
        }
    }

    pub fn destructive<F: Fn() + 'static>(
        icon: &str,
        label: impl Into<String>,
        shortcut: Option<&str>,
        on_click: F,
    ) -> Self {
        Self::Action {
            icon: (!icon.is_empty()).then(|| icon.to_string()),
            label: label.into(),
            shortcut: shortcut.map(str::to_string),
            checked: None,
            radio: false,
            enabled: true,
            destructive: true,
            on_click: Rc::new(on_click),
        }
    }

    pub fn toggle<F: Fn() + 'static>(
        label: impl Into<String>,
        checked: bool,
        radio: bool,
        on_click: F,
    ) -> Self {
        Self::Action {
            icon: None,
            label: label.into(),
            shortcut: None,
            checked: Some(checked),
            radio,
            enabled: true,
            destructive: false,
            on_click: Rc::new(on_click),
        }
    }

    pub fn submenu(icon: &str, label: impl Into<String>, items: Vec<ContextMenuItem>) -> Self {
        let has_enabled = items.iter().any(|i| !matches!(i, Self::Separator | Self::Header(_)));
        Self::Submenu {
            icon: (!icon.is_empty()).then(|| icon.to_string()),
            label: label.into(),
            enabled: has_enabled,
            items,
        }
    }
}

/// Interactive row handle tracked for smooth keyboard + hover navigation.
struct RowHandle {
    btn: Button,
    enabled: bool,
    on_activate: Option<MenuCallback>,
    submenu_items: Option<Rc<Vec<ContextMenuItem>>>,
}

#[derive(Default)]
struct MenuNavState {
    is_open: bool,
    main_origin: (i32, i32),
    main_size: (i32, i32),
    active_output: Option<OutputInfo>,
    main_rows: Vec<RowHandle>,
    main_focus: Option<usize>,
    open_submenu_parent_idx: Option<usize>,
    sub_rows: Vec<RowHandle>,
    sub_focus: Option<usize>,
    hover_epoch: u64,
    pointer_in_submenu: bool,
}

const WRAPPER_PAD: i32 = 22;
const MENU_MIN_W: i32 = 248;
const SUBMENU_MIN_W: i32 = 228;

/// Unified macOS-style frosted glass context menu controller shared by the entire NEON DE shell.
#[derive(Clone)]
pub struct ContextMenuOverlay {
    app: Application,
    backdrop_window: ApplicationWindow,
    menu_window: ApplicationWindow,
    menu_card: GtkBox,
    submenu_window: ApplicationWindow,
    submenu_card: GtkBox,
    nav: Rc<RefCell<MenuNavState>>,
    anim_gen: Rc<Cell<u64>>,
    last_snapshot: Rc<RefCell<Option<DesktopSnapshot>>>,
    last_sys_snapshot: Rc<RefCell<SystemSnapshot>>,
    all_apps: Rc<RefCell<Vec<DesktopApp>>>,
    file_clipboard: Rc<RefCell<FileClipboard>>,
    on_desktop_refresh: VoidHook,
    on_open_workspaces: VoidHook,
    on_open_quick_settings: VoidHook,
    sys_tx: SystemActionSender,
    ipc: IpcCommandSender,
}

impl ContextMenuOverlay {
    pub fn build(
        app: &Application,
        all_apps: Rc<RefCell<Vec<DesktopApp>>>,
        ipc: IpcCommandSender,
        sys_tx: SystemActionSender,
    ) -> Self {
        // 1. Fullscreen transparent click-catcher backdrop
        let backdrop_window = ApplicationWindow::builder()
            .application(app)
            .title("NEON DE Context Menu Backdrop")
            .decorated(false)
            .build();
        backdrop_window.add_css_class("neon-ctx-backdrop-window");
        if gtk4_layer_shell::is_supported() {
            backdrop_window.init_layer_shell();
            backdrop_window.set_namespace(Some("neon-context-menu-backdrop"));
            backdrop_window.set_layer(Layer::Overlay);
            backdrop_window.set_anchor(Edge::Top, true);
            backdrop_window.set_anchor(Edge::Bottom, true);
            backdrop_window.set_anchor(Edge::Left, true);
            backdrop_window.set_anchor(Edge::Right, true);
            backdrop_window.set_exclusive_zone(-1);
            backdrop_window.set_keyboard_mode(KeyboardMode::None);
        }
        let backdrop_fill = GtkBox::new(Orientation::Vertical, 0);
        backdrop_fill.add_css_class("neon-ctx-backdrop-scrim");
        backdrop_fill.set_hexpand(true);
        backdrop_fill.set_vexpand(true);
        backdrop_window.set_child(Some(&backdrop_fill));
        backdrop_window.set_visible(false);

        // 2. Primary floating blurred menu window
        let menu_window = ApplicationWindow::builder()
            .application(app)
            .title("NEON DE Context Menu")
            .decorated(false)
            .resizable(false)
            .build();
        menu_window.add_css_class("neon-contextmenu-window");
        if gtk4_layer_shell::is_supported() {
            menu_window.init_layer_shell();
            menu_window.set_namespace(Some("neon-context-menu"));
            menu_window.set_layer(Layer::Overlay);
            menu_window.set_anchor(Edge::Top, true);
            menu_window.set_anchor(Edge::Left, true);
            menu_window.set_exclusive_zone(-1);
            menu_window.set_keyboard_mode(KeyboardMode::None);
        }

        let menu_wrapper = GtkBox::new(Orientation::Vertical, 0);
        menu_wrapper.add_css_class("neon-ctx-wrapper");
        let menu_card = GtkBox::new(Orientation::Vertical, 2);
        menu_card.add_css_class("neon-ctx-menu");
        menu_card.set_width_request(MENU_MIN_W);
        menu_wrapper.append(&menu_card);
        menu_window.set_child(Some(&menu_wrapper));
        menu_window.set_visible(false);

        // 3. Cascading floating blurred submenu window
        let submenu_window = ApplicationWindow::builder()
            .application(app)
            .title("NEON DE Context Submenu")
            .decorated(false)
            .resizable(false)
            .build();
        submenu_window.add_css_class("neon-contextmenu-window");
        if gtk4_layer_shell::is_supported() {
            submenu_window.init_layer_shell();
            submenu_window.set_namespace(Some("neon-context-submenu"));
            submenu_window.set_layer(Layer::Overlay);
            submenu_window.set_anchor(Edge::Top, true);
            submenu_window.set_anchor(Edge::Left, true);
            submenu_window.set_exclusive_zone(-1);
            submenu_window.set_keyboard_mode(KeyboardMode::None);
        }

        let sub_wrapper = GtkBox::new(Orientation::Vertical, 0);
        sub_wrapper.add_css_class("neon-ctx-wrapper");
        let submenu_card = GtkBox::new(Orientation::Vertical, 2);
        submenu_card.add_css_class("neon-ctx-menu");
        submenu_card.add_css_class("neon-ctx-submenu");
        submenu_card.set_width_request(SUBMENU_MIN_W);
        sub_wrapper.append(&submenu_card);
        submenu_window.set_child(Some(&sub_wrapper));
        submenu_window.set_visible(false);

        let overlay = Self {
            app: app.clone(),
            backdrop_window,
            menu_window,
            menu_card,
            submenu_window,
            submenu_card,
            nav: Rc::new(RefCell::new(MenuNavState::default())),
            anim_gen: Rc::new(Cell::new(0)),
            last_snapshot: Rc::new(RefCell::new(None)),
            last_sys_snapshot: Rc::new(RefCell::new(SystemSnapshot::default())),
            all_apps,
            file_clipboard: Rc::new(RefCell::new(FileClipboard::default())),
            on_desktop_refresh: Rc::new(RefCell::new(None)),
            on_open_workspaces: Rc::new(RefCell::new(None)),
            on_open_quick_settings: Rc::new(RefCell::new(None)),
            sys_tx,
            ipc,
        };

        // Outside click on backdrop dismisses menu + submenu immediately.
        {
            let click = GestureClick::new();
            click.set_button(0);
            let ov = overlay.clone();
            click.connect_pressed(move |_, _, _, _| {
                ov.close();
            });
            backdrop_fill.add_controller(click);
        }

        // Clicking inside the 14px transparent drop-shadow margin around the menu card also dismisses.
        {
            let click = GestureClick::new();
            click.set_button(0);
            let ov = overlay.clone();
            let card = overlay.menu_card.clone();
            click.connect_pressed(move |gesture, _, x, y| {
                if let Some(w) = gesture.widget()
                    && let Some(picked) = w.pick(x, y, gtk4::PickFlags::DEFAULT)
                {
                    let inside = picked == card.clone().upcast::<gtk4::Widget>()
                        || picked.is_ancestor(&card);
                    if !inside {
                        ov.close();
                    }
                }
            });
            menu_wrapper.add_controller(click);
        }

        // Track pointer entry/exit on the submenu window so hover-intent doesn't prematurely dismiss it.
        {
            let motion = EventControllerMotion::new();
            let nav_in = Rc::clone(&overlay.nav);
            motion.connect_enter(move |_, _, _| {
                let mut n = nav_in.borrow_mut();
                n.pointer_in_submenu = true;
                n.hover_epoch = n.hover_epoch.wrapping_add(1);
            });
            let nav_out = Rc::clone(&overlay.nav);
            motion.connect_leave(move |_| {
                nav_out.borrow_mut().pointer_in_submenu = false;
            });
            sub_wrapper.add_controller(motion);
        }

        // Full keyboard navigation (Up/Down/Home/End/Right/Left/Enter/Space/Escape)
        {
            let key_ctrl = EventControllerKey::new();
            let ov = overlay.clone();
            key_ctrl.connect_key_pressed(move |_, key, _, _| ov.handle_key(key));
            overlay.menu_window.add_controller(key_ctrl);
        }

        overlay
    }

    /// Registers a hook called after Desktop / File mutations or explicit `Refresh`.
    pub fn set_on_desktop_refresh<F: Fn() + 'static>(&self, cb: F) {
        *self.on_desktop_refresh.borrow_mut() = Some(Box::new(cb));
    }

    /// Registers a hook to open the Workspaces Sidebar from Desktop/Panel menus.
    pub fn set_on_open_workspaces<F: Fn() + 'static>(&self, cb: F) {
        *self.on_open_workspaces.borrow_mut() = Some(Box::new(cb));
    }

    /// Registers a hook to open Quick Settings from Panel menus.
    pub fn set_on_open_quick_settings<F: Fn() + 'static>(&self, cb: F) {
        *self.on_open_quick_settings.borrow_mut() = Some(Box::new(cb));
    }

    pub fn update_snapshot(&self, snapshot: &DesktopSnapshot) {
        *self.last_snapshot.borrow_mut() = Some(snapshot.clone());
    }

    pub fn update_system_snapshot(&self, sys: &SystemSnapshot) {
        *self.last_sys_snapshot.borrow_mut() = sys.clone();
    }

    pub fn output_size(&self) -> (i32, i32) {
        self.last_snapshot
            .borrow()
            .as_ref()
            .and_then(|s| s.outputs.iter().find(|o| o.primary).or(s.outputs.first()))
            .map(|o| (o.width.max(800), o.height.max(600)))
            .unwrap_or((1920, 1080))
    }

    fn output_containing(&self, x: i32, y: i32) -> OutputInfo {
        let snap = self.last_snapshot.borrow();
        if let Some(s) = snap.as_ref() {
            if let Some(found) = s.outputs.iter().find(|o| {
                x >= o.x && x < o.x + o.width && y >= o.y && y < o.y + o.height
            }) {
                return found.clone();
            }
            if let Some(prim) = s.outputs.iter().find(|o| o.primary).or(s.outputs.first()) {
                return prim.clone();
            }
        }
        OutputInfo {
            name: "primary".into(),
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
            scale: 1.0,
            refresh_mhz: 60_000,
            primary: true,
            transform: "normal".into(),
        }
    }

    fn bind_windows_to_monitor(&self, output: &OutputInfo) {
        if !gtk4_layer_shell::is_supported() {
            return;
        }
        let Some(display) = gdk::Display::default() else {
            return;
        };
        let monitors = display.monitors();
        let n = monitors.n_items();
        if n <= 1 {
            return;
        }
        for i in 0..n {
            if let Some(obj) = monitors.item(i)
                && let Ok(mon) = obj.downcast::<gdk::Monitor>()
            {
                let geo = mon.geometry();
                let connector_match = mon
                    .connector()
                    .is_some_and(|c| c.as_str() == output.name.as_str());
                let rect_match = geo.x() == output.x && geo.y() == output.y;
                if connector_match || rect_match {
                    self.backdrop_window.set_monitor(Some(&mon));
                    self.menu_window.set_monitor(Some(&mon));
                    self.submenu_window.set_monitor(Some(&mon));
                    return;
                }
            }
        }
    }

    fn trigger_desktop_refresh(&self) {
        if let Some(cb) = self.on_desktop_refresh.borrow().as_ref() {
            cb();
        }
    }

    pub fn close_submenu(&self) {
        {
            let mut nav = self.nav.borrow_mut();
            if let Some(parent_idx) = nav.open_submenu_parent_idx.take()
                && let Some(row) = nav.main_rows.get(parent_idx)
            {
                row.btn.remove_css_class("submenu-open");
            }
            nav.sub_rows.clear();
            nav.sub_focus = None;
            nav.pointer_in_submenu = false;
        }
        self.submenu_window.set_visible(false);
    }

    pub fn close(&self) {
        let was_open = {
            let mut nav = self.nav.borrow_mut();
            let prev = nav.is_open;
            nav.is_open = false;
            nav.hover_epoch = nav.hover_epoch.wrapping_add(1);
            nav.main_rows.clear();
            nav.main_focus = None;
            nav.open_submenu_parent_idx = None;
            nav.sub_rows.clear();
            nav.sub_focus = None;
            nav.pointer_in_submenu = false;
            prev
        };
        let epoch = self.anim_gen.get().wrapping_add(1);
        self.anim_gen.set(epoch);

        if gtk4_layer_shell::is_supported() {
            self.menu_window.set_keyboard_mode(KeyboardMode::None);
        }
        self.submenu_window.set_visible(false);
        self.backdrop_window.set_visible(false);

        if was_open {
            self.ipc.send(IpcRequest::AnimateOverlay {
                namespace: "neon-context-menu".into(),
                opening: false,
            });
            let win = self.menu_window.clone();
            let gen_c = Rc::clone(&self.anim_gen);
            glib::timeout_add_local_once(Duration::from_millis(95), move || {
                if gen_c.get() == epoch {
                    win.set_visible(false);
                }
            });
        } else {
            self.menu_window.set_visible(false);
        }
    }

    /// Displays the unified macOS context menu anchored smartly at screen coordinates `(x, y)`.
    pub fn show_at(&self, x: i32, y: i32, items: Vec<ContextMenuItem>) {
        // Cancel any pending close animation
        let epoch = self.anim_gen.get().wrapping_add(1);
        self.anim_gen.set(epoch);
        self.submenu_window.set_visible(false);

        let output = self.output_containing(x, y);
        self.bind_windows_to_monitor(&output);

        let (rows, est_w, est_h) = self.populate_card(&self.menu_card, items, false);
        let card_w = est_w.max(MENU_MIN_W);
        let card_h = est_h.max(40);
        self.menu_card.set_width_request(card_w);

        // Monitor-local positioning with smart edge flipping
        let local_x = x - output.x;
        let local_y = y - output.y;
        let margin_pad = 8;
        let card_x = if local_x + card_w + margin_pad <= output.width {
            local_x.max(margin_pad)
        } else {
            (local_x - card_w).clamp(margin_pad, (output.width - card_w - margin_pad).max(margin_pad))
        };
        let card_y = if local_y + card_h + margin_pad <= output.height {
            local_y.max(margin_pad)
        } else {
            (local_y - card_h).clamp(margin_pad, (output.height - card_h - margin_pad).max(margin_pad))
        };

        {
            let mut nav = self.nav.borrow_mut();
            nav.is_open = true;
            nav.main_origin = (card_x, card_y);
            nav.main_size = (card_w, card_h);
            nav.active_output = Some(output);
            nav.main_rows = rows;
            nav.main_focus = None;
            nav.open_submenu_parent_idx = None;
            nav.sub_rows.clear();
            nav.sub_focus = None;
            nav.hover_epoch = nav.hover_epoch.wrapping_add(1);
            nav.pointer_in_submenu = false;
        }

        if gtk4_layer_shell::is_supported() {
            self.menu_window.set_margin(Edge::Left, (card_x - WRAPPER_PAD).max(0));
            self.menu_window.set_margin(Edge::Top, (card_y - WRAPPER_PAD).max(0));
            self.menu_window.set_keyboard_mode(KeyboardMode::Exclusive);
        }

        self.backdrop_window.set_visible(true);
        self.backdrop_window.present();

        // Force shrink-to-fit measurement when menu height shrinks between invocations
        self.menu_window.set_default_size(1, 1);
        self.ipc.send(IpcRequest::AnimateOverlay {
            namespace: "neon-context-menu".into(),
            opening: true,
        });
        self.menu_window.set_visible(true);
        self.menu_window.present();
    }

    fn populate_card(
        &self,
        card: &GtkBox,
        items: Vec<ContextMenuItem>,
        is_submenu: bool,
    ) -> (Vec<RowHandle>, i32, i32) {
        while let Some(child) = card.first_child() {
            card.remove(&child);
        }

        // Collapse leading, trailing, or consecutive separators cleanly.
        let mut cleaned: Vec<ContextMenuItem> = Vec::with_capacity(items.len());
        for item in items {
            if matches!(item, ContextMenuItem::Separator) {
                if cleaned.is_empty() || matches!(cleaned.last(), Some(ContextMenuItem::Separator)) {
                    continue;
                }
            }
            cleaned.push(item);
        }
        while matches!(cleaned.last(), Some(ContextMenuItem::Separator)) {
            cleaned.pop();
        }

        let mut rows: Vec<RowHandle> = Vec::new();
        let mut est_h: i32 = 12;
        let mut max_chars: usize = 20;

        for item in cleaned {
            match item {
                ContextMenuItem::Separator => {
                    let sep = Separator::new(Orientation::Horizontal);
                    sep.add_css_class("neon-ctx-sep");
                    card.append(&sep);
                    est_h += 9;
                }
                ContextMenuItem::Header(text) => {
                    let hdr = Label::new(Some(&text));
                    hdr.add_css_class("neon-ctx-header");
                    hdr.set_halign(Align::Start);
                    hdr.set_ellipsize(gtk4::pango::EllipsizeMode::End);
                    hdr.set_max_width_chars(30);
                    card.append(&hdr);
                    est_h += 24;
                }
                ContextMenuItem::Action {
                    icon,
                    label,
                    shortcut,
                    checked,
                    radio,
                    enabled,
                    destructive,
                    on_click,
                } => {
                    let row_idx = rows.len();
                    let label_len = label.chars().count()
                        + shortcut.as_ref().map(|s| s.chars().count() + 4).unwrap_or(0);
                    max_chars = max_chars.max(label_len.min(34));

                    let btn = Button::new();
                    btn.add_css_class("neon-ctx-item");
                    if destructive {
                        btn.add_css_class("destructive");
                    }
                    if !enabled {
                        btn.add_css_class("disabled");
                        btn.set_sensitive(false);
                    }

                    let row = GtkBox::new(Orientation::Horizontal, 9);
                    row.set_valign(Align::Center);

                    // Left 16px leading gutter: checkmark / radio dot / symbolic icon
                    let lead = GtkBox::new(Orientation::Horizontal, 0);
                    lead.set_size_request(16, 16);
                    lead.set_halign(Align::Center);
                    lead.set_valign(Align::Center);
                    if let Some(is_checked) = checked {
                        if is_checked {
                            let ic = if radio {
                                "radio-checked-symbolic"
                            } else {
                                "object-select-symbolic"
                            };
                            let img = Image::from_icon_name(ic);
                            img.set_pixel_size(13);
                            img.add_css_class("neon-ctx-check");
                            lead.append(&img);
                        }
                    } else if let Some(ref ic_name) = icon {
                        let img = Image::from_icon_name(ic_name);
                        img.set_pixel_size(14);
                        img.add_css_class("neon-ctx-icon");
                        lead.append(&img);
                    }
                    row.append(&lead);

                    let lbl = Label::new(Some(&label));
                    lbl.set_halign(Align::Start);
                    lbl.set_hexpand(true);
                    lbl.set_ellipsize(gtk4::pango::EllipsizeMode::End);
                    lbl.set_max_width_chars(34);
                    row.append(&lbl);

                    if let Some(sc) = shortcut {
                        let sc_lbl = Label::new(Some(&sc));
                        sc_lbl.add_css_class("neon-ctx-shortcut");
                        row.append(&sc_lbl);
                    }

                    btn.set_child(Some(&row));

                    if enabled {
                        let ov = self.clone();
                        let cb = Rc::clone(&on_click);
                        btn.connect_clicked(move |_| {
                            ov.close();
                            cb();
                        });

                        let motion = EventControllerMotion::new();
                        let ov_h = self.clone();
                        motion.connect_enter(move |_, _, _| {
                            ov_h.on_row_hovered(is_submenu, row_idx, false);
                        });
                        btn.add_controller(motion);
                    }

                    card.append(&btn);
                    rows.push(RowHandle {
                        btn,
                        enabled,
                        on_activate: Some(on_click),
                        submenu_items: None,
                    });
                    est_h += 31;
                }
                ContextMenuItem::Submenu {
                    icon,
                    label,
                    enabled,
                    items: sub_items,
                } => {
                    let row_idx = rows.len();
                    max_chars = max_chars.max(label.chars().count() + 4);

                    let btn = Button::new();
                    btn.add_css_class("neon-ctx-item");
                    btn.add_css_class("neon-ctx-has-submenu");
                    if !enabled {
                        btn.add_css_class("disabled");
                        btn.set_sensitive(false);
                    }

                    let row = GtkBox::new(Orientation::Horizontal, 9);
                    row.set_valign(Align::Center);

                    let lead = GtkBox::new(Orientation::Horizontal, 0);
                    lead.set_size_request(16, 16);
                    lead.set_halign(Align::Center);
                    lead.set_valign(Align::Center);
                    if let Some(ref ic_name) = icon {
                        let img = Image::from_icon_name(ic_name);
                        img.set_pixel_size(14);
                        img.add_css_class("neon-ctx-icon");
                        lead.append(&img);
                    }
                    row.append(&lead);

                    let lbl = Label::new(Some(&label));
                    lbl.set_halign(Align::Start);
                    lbl.set_hexpand(true);
                    lbl.set_ellipsize(gtk4::pango::EllipsizeMode::End);
                    lbl.set_max_width_chars(26);
                    row.append(&lbl);

                    let chev = Image::from_icon_name("pan-end-symbolic");
                    chev.set_pixel_size(13);
                    chev.add_css_class("neon-ctx-chevron");
                    row.append(&chev);

                    btn.set_child(Some(&row));

                    let sub_rc = Rc::new(sub_items);
                    if enabled && !is_submenu {
                        let ov_c = self.clone();
                        btn.connect_clicked(move |_| {
                            ov_c.open_submenu_for_row(row_idx, false);
                        });

                        let motion = EventControllerMotion::new();
                        let ov_h = self.clone();
                        motion.connect_enter(move |_, _, _| {
                            ov_h.on_row_hovered(false, row_idx, true);
                        });
                        btn.add_controller(motion);
                    }

                    card.append(&btn);
                    rows.push(RowHandle {
                        btn,
                        enabled,
                        on_activate: None,
                        submenu_items: Some(sub_rc),
                    });
                    est_h += 31;
                }
            }
        }

        let est_w = (max_chars as i32 * 8 + 86)
            .clamp(if is_submenu { SUBMENU_MIN_W } else { MENU_MIN_W }, 360);
        (rows, est_w, est_h)
    }

    fn on_row_hovered(&self, is_submenu: bool, row_idx: usize, is_submenu_trigger: bool) {
        let epoch = {
            let mut nav = self.nav.borrow_mut();
            nav.hover_epoch = nav.hover_epoch.wrapping_add(1);
            if is_submenu {
                set_focus_class(&nav.sub_rows, nav.sub_focus, Some(row_idx));
                nav.sub_focus = Some(row_idx);
                return;
            } else {
                set_focus_class(&nav.main_rows, nav.main_focus, Some(row_idx));
                nav.main_focus = Some(row_idx);
            }
            nav.hover_epoch
        };

        let ov = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(95), move || {
            let still_valid = {
                let nav = ov.nav.borrow();
                nav.is_open && nav.hover_epoch == epoch
            };
            if !still_valid {
                return;
            }
            if is_submenu_trigger {
                ov.open_submenu_for_row(row_idx, false);
            } else if !ov.nav.borrow().pointer_in_submenu {
                ov.close_submenu();
            }
        });
    }

    /// Opens the cascading submenu belonging to `parent_row_idx` positioned cleanly beside it.
    pub fn open_submenu_for_row(&self, parent_row_idx: usize, focus_first: bool) {
        let (items_cloned, row_y_offset, main_origin, main_size, output) = {
            let mut nav = self.nav.borrow_mut();
            if !nav.is_open {
                return;
            }
            if nav.open_submenu_parent_idx == Some(parent_row_idx)
                && self.submenu_window.is_visible()
            {
                if focus_first && !nav.sub_rows.is_empty() {
                    let first = nav.sub_rows.iter().position(|r| r.enabled);
                    set_focus_class(&nav.sub_rows, nav.sub_focus, first);
                    nav.sub_focus = first;
                }
                return;
            }
            if let Some(prev) = nav.open_submenu_parent_idx.take()
                && let Some(prev_row) = nav.main_rows.get(prev)
            {
                prev_row.btn.remove_css_class("submenu-open");
            }
            let Some(row) = nav.main_rows.get(parent_row_idx) else {
                return;
            };
            let Some(ref sub_rc) = row.submenu_items else {
                return;
            };
            row.btn.add_css_class("submenu-open");
            let rel_y = row
                .btn
                .compute_bounds(&self.menu_card)
                .map(|b| b.y().round() as i32)
                .unwrap_or((parent_row_idx as i32) * 31 + 6);

            let items = clone_menu_items(sub_rc);
            let actual_w = self.menu_card.width().max(nav.main_size.0);
            let actual_h = self.menu_card.height().max(nav.main_size.1);
            nav.main_size = (actual_w, actual_h);
            nav.open_submenu_parent_idx = Some(parent_row_idx);
            (
                items,
                rel_y,
                nav.main_origin,
                nav.main_size,
                nav.active_output.clone(),
            )
        };

        let (sub_rows, est_w, est_h) = self.populate_card(&self.submenu_card, items_cloned, true);
        let sub_w = est_w.max(SUBMENU_MIN_W);
        let sub_h = est_h.max(40);
        self.submenu_card.set_width_request(sub_w);

        let out = output.unwrap_or_else(|| self.output_containing(main_origin.0, main_origin.1));
        let margin_pad = 8;
        let right_x = main_origin.0 + main_size.0 - 4;
        let sub_x = if right_x + sub_w + margin_pad <= out.width {
            right_x
        } else {
            (main_origin.0 - sub_w + 4).clamp(margin_pad, (out.width - sub_w - margin_pad).max(margin_pad))
        };
        let sub_y = (main_origin.1 + row_y_offset - 5)
            .clamp(margin_pad, (out.height - sub_h - margin_pad).max(margin_pad));

        {
            let mut nav = self.nav.borrow_mut();
            let first = if focus_first {
                sub_rows.iter().position(|r| r.enabled)
            } else {
                None
            };
            set_focus_class(&sub_rows, None, first);
            nav.sub_rows = sub_rows;
            nav.sub_focus = first;
        }

        if gtk4_layer_shell::is_supported() {
            self.submenu_window
                .set_margin(Edge::Left, (sub_x - WRAPPER_PAD).max(0));
            self.submenu_window
                .set_margin(Edge::Top, (sub_y - WRAPPER_PAD).max(0));
        }
        self.submenu_window.set_default_size(1, 1);
        self.ipc.send(IpcRequest::AnimateOverlay {
            namespace: "neon-context-submenu".into(),
            opening: true,
        });
        self.submenu_window.set_visible(true);
        self.submenu_window.present();
    }

    fn handle_key(&self, key: gdk::Key) -> glib::Propagation {
        match key {
            gdk::Key::Escape => {
                if self.nav.borrow().open_submenu_parent_idx.is_some()
                    && self.submenu_window.is_visible()
                    && self.nav.borrow().sub_focus.is_some()
                {
                    self.close_submenu();
                } else {
                    self.close();
                }
                glib::Propagation::Stop
            }
            gdk::Key::Down => {
                self.step_focus(1);
                glib::Propagation::Stop
            }
            gdk::Key::Up => {
                self.step_focus(-1);
                glib::Propagation::Stop
            }
            gdk::Key::Home => {
                self.jump_focus(true);
                glib::Propagation::Stop
            }
            gdk::Key::End => {
                self.jump_focus(false);
                glib::Propagation::Stop
            }
            gdk::Key::Right => {
                let target = {
                    let nav = self.nav.borrow();
                    if nav.sub_focus.is_none() {
                        nav.main_focus.filter(|&idx| {
                            nav.main_rows
                                .get(idx)
                                .is_some_and(|r| r.enabled && r.submenu_items.is_some())
                        })
                    } else {
                        None
                    }
                };
                if let Some(idx) = target {
                    self.open_submenu_for_row(idx, true);
                }
                glib::Propagation::Stop
            }
            gdk::Key::Left => {
                if self.submenu_window.is_visible() {
                    self.close_submenu();
                }
                glib::Propagation::Stop
            }
            gdk::Key::Return | gdk::Key::KP_Enter | gdk::Key::space => {
                enum Activate {
                    None,
                    Callback(MenuCallback),
                    OpenSubmenu(usize),
                }
                let action = {
                    let nav = self.nav.borrow();
                    if let Some(sidx) = nav.sub_focus {
                        nav.sub_rows
                            .get(sidx)
                            .filter(|r| r.enabled)
                            .and_then(|r| r.on_activate.clone())
                            .map(Activate::Callback)
                            .unwrap_or(Activate::None)
                    } else if let Some(midx) = nav.main_focus {
                        if let Some(row) = nav.main_rows.get(midx).filter(|r| r.enabled) {
                            if row.submenu_items.is_some() {
                                Activate::OpenSubmenu(midx)
                            } else if let Some(ref cb) = row.on_activate {
                                Activate::Callback(Rc::clone(cb))
                            } else {
                                Activate::None
                            }
                        } else {
                            Activate::None
                        }
                    } else {
                        Activate::None
                    }
                };
                match action {
                    Activate::Callback(cb) => {
                        self.close();
                        cb();
                    }
                    Activate::OpenSubmenu(idx) => {
                        self.open_submenu_for_row(idx, true);
                    }
                    Activate::None => {}
                }
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        }
    }

    fn step_focus(&self, delta: isize) {
        let mut nav = self.nav.borrow_mut();
        if nav.sub_focus.is_some() && !nav.sub_rows.is_empty() {
            let next = next_enabled_index(&nav.sub_rows, nav.sub_focus, delta);
            set_focus_class(&nav.sub_rows, nav.sub_focus, next);
            nav.sub_focus = next;
        } else if !nav.main_rows.is_empty() {
            let next = next_enabled_index(&nav.main_rows, nav.main_focus, delta);
            set_focus_class(&nav.main_rows, nav.main_focus, next);
            nav.main_focus = next;
        }
    }

    fn jump_focus(&self, first: bool) {
        let mut nav_guard = self.nav.borrow_mut();
        let nav = &mut *nav_guard;
        let (rows, focus) = if nav.sub_focus.is_some() && !nav.sub_rows.is_empty() {
            (&nav.sub_rows, &mut nav.sub_focus)
        } else {
            (&nav.main_rows, &mut nav.main_focus)
        };
        let target = if first {
            rows.iter().position(|r| r.enabled)
        } else {
            rows.iter().rposition(|r| r.enabled)
        };
        set_focus_class(rows, *focus, target);
        *focus = target;
    }

    /// Handles a compositor-initiated context menu request (`Desktop` or `Window`).
    pub fn handle_compositor_request(&self, x: i32, y: i32, target: ContextMenuTarget) {
        match target {
            ContextMenuTarget::Desktop => self.open_desktop_menu(x, y),
            ContextMenuTarget::Window { window_id } => self.open_window_menu(x, y, window_id),
        }
    }

    // =========================================================================
    // 1. DESKTOP CONTEXT MENU
    // =========================================================================

    pub fn open_desktop_menu(&self, x: i32, y: i32) {
        let snap = self.last_snapshot.borrow().clone();
        let app_info = snap.as_ref().map(|s| s.appearance.clone()).unwrap_or_default();
        let desk = desktop_dir();

        let mut items: Vec<ContextMenuItem> = Vec::new();

        // New Folder…
        {
            let gtk_app = self.app.clone();
            let d = desk.clone();
            let ov = self.clone();
            let ipc = self.ipc.clone();
            items.push(ContextMenuItem::action(
                "folder-new-symbolic",
                "New Folder…",
                move || {
                    let ov2 = ov.clone();
                    let ipc2 = ipc.clone();
                    show_new_folder_dialog(
                        &gtk_app,
                        d.clone(),
                        Box::new(move || {
                            ipc2.send(IpcRequest::from(AppearanceUpdate {
                                show_desktop_icons: Some(true),
                                ..Default::default()
                            }));
                            ov2.trigger_desktop_refresh();
                        }),
                    );
                },
            ));
        }

        // Context-aware Paste (ONLY shown when clipboard has valid file(s))
        if self.file_clipboard.borrow().has_valid_items() {
            let count = self.file_clipboard.borrow().item_count();
            let is_cut = self.file_clipboard.borrow().is_cut();
            let label = match (is_cut, count) {
                (true, 1) => "Move Item Here".to_string(),
                (true, n) => format!("Move {n} Items Here"),
                (false, 1) => "Paste Item".to_string(),
                (false, n) => format!("Paste {n} Items"),
            };
            let clip = Rc::clone(&self.file_clipboard);
            let d = desk.clone();
            let ov = self.clone();
            items.push(ContextMenuItem::action_sc(
                "edit-paste-symbolic",
                label,
                "Ctrl+V",
                move || {
                    if let Err(err) = clip.borrow_mut().paste_into(&d) {
                        warn!(%err, "Desktop paste failed");
                    } else {
                        ov.trigger_desktop_refresh();
                    }
                },
            ));
        }

        // Open Terminal Here
        {
            let d = desk.clone();
            items.push(ContextMenuItem::action_sc(
                "utilities-terminal-symbolic",
                "Open Terminal Here",
                "Ctrl+Alt+T",
                move || open_terminal_in(&d),
            ));
        }

        items.push(ContextMenuItem::Separator);

        // Sort By ▶ submenu
        {
            let cur_sort = app_info.desktop_sort_by.as_str();
            let sorts = [
                ("name", "Name"),
                ("modified", "Date Modified"),
                ("size", "Size"),
                ("type", "Type"),
            ];
            let sub: Vec<ContextMenuItem> = sorts
                .into_iter()
                .map(|(key, label)| {
                    let ipc = self.ipc.clone();
                    let ov = self.clone();
                    ContextMenuItem::toggle(label, cur_sort == key, true, move || {
                        ipc.send(IpcRequest::from(AppearanceUpdate {
                            desktop_sort_by: Some(key.to_string()),
                            show_desktop_icons: Some(true),
                            ..Default::default()
                        }));
                        ov.trigger_desktop_refresh();
                    })
                })
                .collect();
            items.push(ContextMenuItem::submenu("view-sort-ascending-symbolic", "Sort By", sub));
        }

        // Arrange Icons ▶ submenu
        {
            let show_icons = app_info.show_desktop_icons;
            let auto_arr = app_info.desktop_auto_arrange;
            let ipc_show = self.ipc.clone();
            let ipc_arr = self.ipc.clone();
            let ov_ref = self.clone();
            let sub = vec![
                ContextMenuItem::toggle("Show Desktop Icons", show_icons, false, move || {
                    ipc_show.send(IpcRequest::from(AppearanceUpdate {
                        show_desktop_icons: Some(!show_icons),
                        ..Default::default()
                    }));
                }),
                ContextMenuItem::toggle("Auto-Arrange Grid", auto_arr, false, move || {
                    ipc_arr.send(IpcRequest::from(AppearanceUpdate {
                        desktop_auto_arrange: Some(!auto_arr),
                        show_desktop_icons: Some(true),
                        ..Default::default()
                    }));
                }),
                ContextMenuItem::Separator,
                ContextMenuItem::action("view-grid-symbolic", "Align to Grid", move || {
                    ov_ref.trigger_desktop_refresh();
                }),
            ];
            items.push(ContextMenuItem::submenu("view-grid-symbolic", "Arrange Icons", sub));
        }

        // Workspaces ▶ submenu
        items.push(self.build_workspaces_submenu(snap.as_ref()));

        items.push(ContextMenuItem::Separator);

        // Change Wallpaper…
        items.push(ContextMenuItem::action(
            "preferences-desktop-wallpaper-symbolic",
            "Change Wallpaper…",
            || launch_settings_page(Some("appearance")),
        ));

        // Personalize ▶ submenu (Live Light/Dark + Accent swatches + Appearance Settings)
        {
            let is_light = app_info.theme == "neon_light" || app_info.theme == "light";
            let ipc_d = self.ipc.clone();
            let ipc_l = self.ipc.clone();
            let mut sub = vec![
                ContextMenuItem::Header("Theme".into()),
                ContextMenuItem::toggle("Dark Mode", !is_light, true, move || {
                    ipc_d.send(IpcRequest::from(AppearanceUpdate {
                        theme: Some("neon_dark".into()),
                        ..Default::default()
                    }));
                }),
                ContextMenuItem::toggle("Light Mode", is_light, true, move || {
                    ipc_l.send(IpcRequest::from(AppearanceUpdate {
                        theme: Some("neon_light".into()),
                        ..Default::default()
                    }));
                }),
                ContextMenuItem::Separator,
                ContextMenuItem::Header("Accent Color".into()),
            ];
            for (name, hex) in [
                ("Sky Blue", "#38BDF8"),
                ("Ocean Blue", "#0A84FF"),
                ("Purple", "#A855F7"),
                ("Emerald", "#22C55E"),
                ("Amber", "#F59E0B"),
                ("Coral Red", "#EF4444"),
                ("Rose Pink", "#EC4899"),
            ] {
                let ipc_c = self.ipc.clone();
                let active = app_info.accent_color.eq_ignore_ascii_case(hex);
                sub.push(ContextMenuItem::toggle(name, active, true, move || {
                    ipc_c.send(IpcRequest::from(AppearanceUpdate {
                        accent_color: Some(hex.to_string()),
                        ..Default::default()
                    }));
                }));
            }
            sub.push(ContextMenuItem::Separator);
            sub.push(ContextMenuItem::action(
                "preferences-desktop-appearance-symbolic",
                "Appearance Settings…",
                || launch_settings_page(Some("appearance")),
            ));
            items.push(ContextMenuItem::submenu(
                "preferences-desktop-appearance-symbolic",
                "Personalize",
                sub,
            ));
        }

        // Display Settings…
        items.push(ContextMenuItem::action(
            "video-display-symbolic",
            "Display Settings…",
            || launch_settings_page(Some("displays")),
        ));

        self.show_at(x, y, items);
    }

    // =========================================================================
    // 2. FILES & FOLDERS CONTEXT MENU
    // =========================================================================

    pub fn open_file_menu(&self, x: i32, y: i32, path: PathBuf) {
        let is_dir = path.is_dir();
        let archive = is_archive(&path);
        let fname = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());

        let mut items: Vec<ContextMenuItem> = vec![ContextMenuItem::Header(fname)];

        // Open
        {
            let p = path.clone();
            items.push(ContextMenuItem::action_sc(
                if is_dir {
                    "folder-open-symbolic"
                } else {
                    "document-open-symbolic"
                },
                "Open",
                "Enter",
                move || open_path(&p),
            ));
        }

        // Open With ▶ (real XDG MIME-matched apps!)
        {
            let apps_guard = self.all_apps.borrow();
            let matching = find_apps_for_file(&path, &apps_guard);
            let mut sub: Vec<ContextMenuItem> = matching
                .into_iter()
                .map(|app| {
                    let app_clone = app.clone();
                    let p = path.clone();
                    let ic = app
                        .icon
                        .as_deref()
                        .filter(|s| !s.contains('/'))
                        .unwrap_or("application-x-executable");
                    ContextMenuItem::action(ic, app.name.clone(), move || {
                        app_clone.launch_with_file(&p);
                    })
                })
                .collect();
            if is_dir {
                let p = path.clone();
                sub.push(ContextMenuItem::action(
                    "utilities-terminal-symbolic",
                    "Terminal",
                    move || open_terminal_in(&p),
                ));
            }
            if sub.is_empty() {
                let p = path.clone();
                sub.push(ContextMenuItem::action(
                    "document-open-symbolic",
                    "System Default",
                    move || open_path(&p),
                ));
            }
            items.push(ContextMenuItem::submenu("open-menu-symbolic", "Open With", sub));
        }

        if is_dir {
            let p = path.clone();
            items.push(ContextMenuItem::action(
                "utilities-terminal-symbolic",
                "Open Terminal Here",
                move || open_terminal_in(&p),
            ));
        }

        items.push(ContextMenuItem::Separator);

        // Cut / Copy / (Paste on folders)
        {
            let clip = Rc::clone(&self.file_clipboard);
            let p = path.clone();
            items.push(ContextMenuItem::action_sc(
                "edit-cut-symbolic",
                "Cut",
                "Ctrl+X",
                move || clip.borrow_mut().set(FileClipboardOp::Cut, vec![p.clone()]),
            ));
        }
        {
            let clip = Rc::clone(&self.file_clipboard);
            let p = path.clone();
            items.push(ContextMenuItem::action_sc(
                "edit-copy-symbolic",
                "Copy",
                "Ctrl+C",
                move || clip.borrow_mut().set(FileClipboardOp::Copy, vec![p.clone()]),
            ));
        }
        if is_dir && self.file_clipboard.borrow().has_valid_items() {
            let clip = Rc::clone(&self.file_clipboard);
            let p = path.clone();
            let ov = self.clone();
            items.push(ContextMenuItem::action_sc(
                "edit-paste-symbolic",
                "Paste Into Folder",
                "Ctrl+V",
                move || {
                    if let Err(err) = clip.borrow_mut().paste_into(&p) {
                        warn!(%err, "Paste into folder failed");
                    } else {
                        ov.trigger_desktop_refresh();
                    }
                },
            ));
        }

        // Rename… / Duplicate
        {
            let gtk_app = self.app.clone();
            let p = path.clone();
            let ov = self.clone();
            items.push(ContextMenuItem::action_sc(
                "document-edit-symbolic",
                "Rename…",
                "F2",
                move || {
                    let ov2 = ov.clone();
                    show_rename_dialog(
                        &gtk_app,
                        p.clone(),
                        Box::new(move || ov2.trigger_desktop_refresh()),
                    );
                },
            ));
        }
        {
            let p = path.clone();
            let ov = self.clone();
            items.push(ContextMenuItem::action(
                "edit-copy-symbolic",
                "Duplicate",
                move || {
                    if let Err(err) = duplicate_path(&p) {
                        warn!(%err, "Duplicate failed");
                    } else {
                        ov.trigger_desktop_refresh();
                    }
                },
            ));
        }

        items.push(ContextMenuItem::Separator);

        // Compress ▶
        {
            let p_zip = path.clone();
            let ov_zip = self.clone();
            let p_tar = path.clone();
            let ov_tar = self.clone();
            let sub = vec![
                ContextMenuItem::action("package-x-generic-symbolic", "Compress to ZIP (.zip)", move || {
                    if let Err(err) = compress_path(&p_zip, ArchiveFormat::Zip) {
                        warn!(%err, "ZIP compression failed");
                    } else {
                        ov_zip.trigger_desktop_refresh();
                    }
                }),
                ContextMenuItem::action(
                    "package-x-generic-symbolic",
                    "Compress to TAR.GZ (.tar.gz)",
                    move || {
                        if let Err(err) = compress_path(&p_tar, ArchiveFormat::TarGz) {
                            warn!(%err, "TAR.GZ compression failed");
                        } else {
                            ov_tar.trigger_desktop_refresh();
                        }
                    },
                ),
            ];
            items.push(ContextMenuItem::submenu("package-x-generic-symbolic", "Compress", sub));
        }

        // Context-aware Extract ▶ (ONLY shown for recognized archive files!)
        if archive {
            let p_here = path.clone();
            let ov_here = self.clone();
            let p_dir = path.clone();
            let ov_dir = self.clone();
            let sub = vec![
                ContextMenuItem::action("extract-archive-symbolic", "Extract Here", move || {
                    if let Err(err) = extract_archive(&p_here, false) {
                        warn!(%err, "Extract here failed");
                    } else {
                        ov_here.trigger_desktop_refresh();
                    }
                }),
                ContextMenuItem::action("folder-new-symbolic", "Extract to Folder", move || {
                    if let Err(err) = extract_archive(&p_dir, true) {
                        warn!(%err, "Extract to folder failed");
                    } else {
                        ov_dir.trigger_desktop_refresh();
                    }
                }),
            ];
            items.push(ContextMenuItem::submenu("extract-archive-symbolic", "Extract", sub));
        }

        // Copy Path
        {
            let p_str = path.to_string_lossy().into_owned();
            items.push(ContextMenuItem::action(
                "edit-find-replace-symbolic",
                "Copy Path",
                move || copy_text_to_clipboard(&p_str),
            ));
        }

        // Properties… (macOS Get Info)
        {
            let gtk_app = self.app.clone();
            let p = path.clone();
            let ov = self.clone();
            items.push(ContextMenuItem::action_sc(
                "document-properties-symbolic",
                "Properties…",
                "Alt+Enter",
                move || {
                    let ov2 = ov.clone();
                    show_properties_dialog(
                        &gtk_app,
                        p.clone(),
                        Box::new(move || ov2.trigger_desktop_refresh()),
                    );
                },
            ));
        }

        items.push(ContextMenuItem::Separator);

        // Move to Trash & Delete Permanently…
        {
            let p = path.clone();
            let ov = self.clone();
            items.push(ContextMenuItem::destructive(
                "user-trash-symbolic",
                "Move to Trash",
                Some("Del"),
                move || {
                    if let Err(err) = move_to_trash(&p) {
                        warn!(%err, "Move to Trash failed");
                    } else {
                        ov.trigger_desktop_refresh();
                    }
                },
            ));
        }
        {
            let gtk_app = self.app.clone();
            let p = path;
            let ov = self.clone();
            items.push(ContextMenuItem::destructive(
                "edit-delete-symbolic",
                "Delete Permanently…",
                Some("Shift+Del"),
                move || {
                    let ov2 = ov.clone();
                    show_delete_confirm_dialog(
                        &gtk_app,
                        p.clone(),
                        Box::new(move || ov2.trigger_desktop_refresh()),
                    );
                },
            ));
        }

        self.show_at(x, y, items);
    }

    // =========================================================================
    // 3. DOCK APP & EMPTY DOCK CONTEXT MENUS
    // =========================================================================

    /// Unified, intelligent Dock app context menu (for both pinned apps and running windows).
    pub fn open_dock_app_menu(
        &self,
        x: i32,
        y: i32,
        explicit_app: Option<DesktopApp>,
        explicit_window_id: Option<u64>,
    ) {
        let snap = self.last_snapshot.borrow().clone();
        let apps_guard = self.all_apps.borrow();

        let target_win: Option<WindowInfo> = explicit_window_id.and_then(|wid| {
            snap.as_ref()
                .and_then(|s| s.windows.iter().find(|w| w.id == wid).cloned())
        });

        let matched_app: Option<DesktopApp> = explicit_app.or_else(|| {
            target_win.as_ref().and_then(|w| {
                resolve_window_desktop_app(w.app_id.as_deref(), w.title.as_deref(), &apps_guard)
                    .cloned()
            })
        });

        // Collect ALL running windows belonging to this app.
        let app_windows: Vec<WindowInfo> = if let Some(ref s) = snap {
            s.windows
                .iter()
                .filter(|w| {
                    if Some(w.id) == explicit_window_id {
                        return true;
                    }
                    if let Some(ref app) = matched_app {
                        resolve_window_desktop_app(
                            w.app_id.as_deref(),
                            w.title.as_deref(),
                            &apps_guard,
                        )
                        .is_some_and(|m| m.id == app.id)
                    } else if let Some(ref tw) = target_win {
                        tw.app_id.is_some() && w.app_id == tw.app_id
                    } else {
                        false
                    }
                })
                .cloned()
                .collect()
        } else {
            Vec::new()
        };
        drop(apps_guard);

        let is_running = !app_windows.is_empty();
        let all_minimized = is_running && app_windows.iter().all(|w| w.minimized);

        let mut items: Vec<ContextMenuItem> = Vec::new();
        if let Some(ref app) = matched_app {
            items.push(ContextMenuItem::Header(app.name.clone()));
        } else if let Some(ref tw) = target_win {
            let hdr = tw
                .title
                .as_deref()
                .or(tw.app_id.as_deref())
                .unwrap_or("Application");
            items.push(ContextMenuItem::Header(hdr.to_string()));
        }

        // Open / Focus
        if is_running {
            let focus_id = explicit_window_id.unwrap_or(app_windows[0].id);
            let ipc = self.ipc.clone();
            items.push(ContextMenuItem::action(
                "media-playback-start-symbolic",
                if all_minimized { "Show" } else { "Open" },
                move || {
                    ipc.send(IpcRequest::SetOverview { open: false });
                    ipc.send(IpcRequest::FocusWindow {
                        window_id: focus_id,
                    });
                },
            ));
        } else if let Some(ref app) = matched_app {
            let app_c = app.clone();
            let ipc = self.ipc.clone();
            items.push(ContextMenuItem::action(
                "media-playback-start-symbolic",
                "Open",
                move || {
                    app_c.launch();
                    ipc.send(IpcRequest::SetOverview { open: false });
                },
            ));
        }

        // New Window
        if let Some(ref app) = matched_app {
            if is_running {
                let app_c = app.clone();
                let ipc = self.ipc.clone();
                items.push(ContextMenuItem::action(
                    "window-new-symbolic",
                    "New Window",
                    move || {
                        app_c.launch();
                        ipc.send(IpcRequest::SetOverview { open: false });
                    },
                ));
            }

            // Application-specific XDG `[Desktop Action ...]` entries!
            self.append_desktop_actions(&mut items, app, is_running);
        }

        // Pin / Unpin & Launch at Login
        if let Some(ref app) = matched_app {
            items.push(ContextMenuItem::Separator);
            let pinned_ids = self.effective_pinned_ids(snap.as_ref());
            let is_pinned = pinned_ids
                .iter()
                .any(|id| id.eq_ignore_ascii_case(&app.id));
            {
                let mut next_pins = pinned_ids;
                let app_id = app.id.clone();
                let ipc = self.ipc.clone();
                if is_pinned {
                    next_pins.retain(|id| !id.eq_ignore_ascii_case(&app_id));
                } else {
                    next_pins.push(app_id);
                }
                items.push(ContextMenuItem::action(
                    if is_pinned {
                        "window-unpin-symbolic"
                    } else {
                        "view-pin-symbolic"
                    },
                    if is_pinned {
                        "Unpin from Dock"
                    } else {
                        "Pin to Dock"
                    },
                    move || {
                        ipc.send(IpcRequest::from(AppearanceUpdate {
                            dock_pinned_apps: Some(next_pins.clone()),
                            ..Default::default()
                        }));
                    },
                ));
            }

            let autostart = is_launch_at_login(&app.id);
            {
                let app_c = app.clone();
                items.push(ContextMenuItem::toggle(
                    "Launch at Login",
                    autostart,
                    false,
                    move || {
                        if let Err(err) = set_launch_at_login(&app_c, !autostart) {
                            warn!(%err, app = %app_c.id, "Failed to update Launch at Login");
                        }
                    },
                ));
            }
        }

        // Running-app window controls: Show All Windows, Move to Workspace, Window Actions, Hide, Quit
        if is_running {
            items.push(ContextMenuItem::Separator);

            // Show All Windows (if multiple, submenu lets you jump to any OR raise all!)
            if app_windows.len() > 1 {
                let all_ids: Vec<u64> = app_windows.iter().map(|w| w.id).collect();
                let ipc_all = self.ipc.clone();
                let mut sub = vec![
                    ContextMenuItem::action("view-grid-symbolic", "Bring All to Front", move || {
                        ipc_all.send(IpcRequest::SetOverview { open: false });
                        for &wid in &all_ids {
                            ipc_all.send(IpcRequest::FocusWindow { window_id: wid });
                        }
                    }),
                    ContextMenuItem::Separator,
                ];
                for w in &app_windows {
                    let wid = w.id;
                    let ipc_w = self.ipc.clone();
                    let t = w
                        .title
                        .as_deref()
                        .or(w.app_id.as_deref())
                        .unwrap_or("Window")
                        .to_string();
                    sub.push(ContextMenuItem::toggle(t, w.focused, true, move || {
                        ipc_w.send(IpcRequest::SetOverview { open: false });
                        ipc_w.send(IpcRequest::FocusWindow { window_id: wid });
                    }));
                }
                items.push(ContextMenuItem::submenu(
                    "view-restore-symbolic",
                    format!("Show All Windows ({})", app_windows.len()),
                    sub,
                ));
            } else {
                let wid = app_windows[0].id;
                let ipc = self.ipc.clone();
                items.push(ContextMenuItem::action(
                    "view-restore-symbolic",
                    "Show All Windows",
                    move || {
                        ipc.send(IpcRequest::SetOverview { open: false });
                        ipc.send(IpcRequest::FocusWindow { window_id: wid });
                    },
                ));
            }

            // Move to Workspace ▶
            if let Some(ref s) = snap {
                let primary_wid = explicit_window_id.unwrap_or(app_windows[0].id);
                let current_ws_id = app_windows
                    .iter()
                    .find(|w| w.id == primary_wid)
                    .map(|w| w.workspace_id);
                items.push(self.build_move_to_workspace_submenu(s, primary_wid, current_ws_id));
            }

            // Full window controls submenu when right-clicking a running Dock icon
            if let Some(ref win) = target_win.or_else(|| app_windows.first().cloned()) {
                let win_sub = self.build_window_actions_list(win, snap.as_ref(), false);
                items.push(ContextMenuItem::submenu(
                    "preferences-system-windows-symbolic",
                    "Window",
                    win_sub,
                ));
            }

            items.push(ContextMenuItem::Separator);

            // Hide / Show
            {
                let ids: Vec<u64> = app_windows.iter().map(|w| w.id).collect();
                let ipc = self.ipc.clone();
                let from_pinned = explicit_window_id.is_none();
                items.push(ContextMenuItem::action(
                    if all_minimized {
                        "window-restore-symbolic"
                    } else {
                        "window-minimize-symbolic"
                    },
                    if all_minimized { "Show" } else { "Hide" },
                    move || {
                        for &wid in &ids {
                            if from_pinned {
                                ipc.send(IpcRequest::SetWindowDockAnchor {
                                    window_id: wid,
                                    x,
                                    y,
                                });
                            }
                            if all_minimized {
                                ipc.send(IpcRequest::FocusWindow { window_id: wid });
                            } else {
                                ipc.send(IpcRequest::MinimizeWindow { window_id: wid });
                            }
                        }
                    },
                ));
            }

            // Quit (closes all windows of this application)
            {
                let ids: Vec<u64> = app_windows.iter().map(|w| w.id).collect();
                let ipc = self.ipc.clone();
                let from_pinned = explicit_window_id.is_none();
                items.push(ContextMenuItem::destructive(
                    "application-exit-symbolic",
                    "Quit",
                    Some("Super+Q"),
                    move || {
                        for &wid in &ids {
                            if from_pinned {
                                ipc.send(IpcRequest::SetWindowDockAnchor {
                                    window_id: wid,
                                    x,
                                    y,
                                });
                            }
                            ipc.send(IpcRequest::CloseWindow { window_id: wid });
                        }
                    },
                ));
            }
        }

        self.show_at(x, y, items);
    }

    /// Right-click menu for App Drawer / Launchpad tiles (Open, App Actions, Pin/Unpin, Autostart).
    pub fn open_app_menu(&self, x: i32, y: i32, app: DesktopApp) {
        self.open_dock_app_menu(x, y, Some(app), None);
    }

    /// Right-click menu for the empty Dock background area.
    pub fn open_dock_menu(&self, x: i32, y: i32) {
        let app_info = self
            .last_snapshot
            .borrow()
            .as_ref()
            .map(|s| s.appearance.clone())
            .unwrap_or_default();

        // Position ▶ (Bottom / Left / Right)
        let pos_sub: Vec<ContextMenuItem> = [("bottom", "Bottom"), ("left", "Left"), ("right", "Right")]
            .into_iter()
            .map(|(val, label)| {
                let ipc = self.ipc.clone();
                ContextMenuItem::toggle(
                    label,
                    app_info.dock_position.eq_ignore_ascii_case(val),
                    true,
                    move || {
                        ipc.send(IpcRequest::from(AppearanceUpdate {
                            dock_position: Some(val.to_string()),
                            ..Default::default()
                        }));
                    },
                )
            })
            .collect();

        // Size ▶ (Tiny / Compact / Small / Medium / Large)
        let cur_sz = app_info.dock_icon_size;
        let presets: [(u32, &str); 5] = [
            (24, "Tiny (24 px)"),
            (30, "Compact (30 px)"),
            (36, "Small (36 px)"),
            (42, "Medium (42 px)"),
            (52, "Large (52 px)"),
        ];
        let closest_sz = presets
            .iter()
            .min_by_key(|(sz, _)| (cur_sz as i32 - *sz as i32).abs())
            .map(|(sz, _)| *sz)
            .unwrap_or(36);
        let size_sub: Vec<ContextMenuItem> = presets
            .into_iter()
            .map(|(sz, label)| {
                let ipc = self.ipc.clone();
                let active = sz == closest_sz;
                ContextMenuItem::toggle(label, active, true, move || {
                    ipc.send(IpcRequest::from(AppearanceUpdate {
                        dock_icon_size: Some(sz),
                        ..Default::default()
                    }));
                })
            })
            .collect();

        // Auto Hide ▶ (Always Visible / Auto-Hide / Smart Hide)
        let hide_mode = if app_info.dock_auto_hide {
            1
        } else if app_info.dock_smart_hide {
            2
        } else {
            0
        };
        let ipc_h0 = self.ipc.clone();
        let ipc_h1 = self.ipc.clone();
        let ipc_h2 = self.ipc.clone();
        let hide_sub = vec![
            ContextMenuItem::toggle("Always Visible", hide_mode == 0, true, move || {
                ipc_h0.send(IpcRequest::from(AppearanceUpdate {
                    dock_auto_hide: Some(false),
                    dock_smart_hide: Some(false),
                    ..Default::default()
                }));
            }),
            ContextMenuItem::toggle("Auto-Hide", hide_mode == 1, true, move || {
                ipc_h1.send(IpcRequest::from(AppearanceUpdate {
                    dock_auto_hide: Some(true),
                    dock_smart_hide: Some(false),
                    ..Default::default()
                }));
            }),
            ContextMenuItem::toggle("Smart Hide (Intelligent)", hide_mode == 2, true, move || {
                ipc_h2.send(IpcRequest::from(AppearanceUpdate {
                    dock_auto_hide: Some(false),
                    dock_smart_hide: Some(true),
                    ..Default::default()
                }));
            }),
        ];

        let ipc_mag = self.ipc.clone();
        let mag_on = app_info.dock_magnification;

        let items = vec![
            ContextMenuItem::Header("Dock".into()),
            ContextMenuItem::submenu("view-paged-symbolic", "Position", pos_sub),
            ContextMenuItem::submenu("zoom-in-symbolic", "Size", size_sub),
            ContextMenuItem::toggle("Magnification", mag_on, false, move || {
                ipc_mag.send(IpcRequest::from(AppearanceUpdate {
                    dock_magnification: Some(!mag_on),
                    ..Default::default()
                }));
            }),
            ContextMenuItem::submenu("view-conceal-symbolic", "Auto Hide", hide_sub),
            ContextMenuItem::Separator,
            ContextMenuItem::action("preferences-system-symbolic", "Dock Settings…", || {
                launch_settings_page(Some("dock"))
            }),
        ];

        self.show_at(x, y, items);
    }

    // =========================================================================
    // 4. WINDOW CONTEXT MENU
    // =========================================================================

    pub fn open_window_menu(&self, x: i32, y: i32, window_id: u64) {
        let snap = self.last_snapshot.borrow().clone();
        let Some(win) = snap
            .as_ref()
            .and_then(|s| s.windows.iter().find(|w| w.id == window_id).cloned())
        else {
            return;
        };
        let items = self.build_window_actions_list(&win, snap.as_ref(), true);
        self.show_at(x, y, items);
    }

    fn build_window_actions_list(
        &self,
        win: &WindowInfo,
        snap: Option<&DesktopSnapshot>,
        include_app_actions_and_close: bool,
    ) -> Vec<ContextMenuItem> {
        let wid = win.id;
        let mut items: Vec<ContextMenuItem> = Vec::new();

        // Minimize / Restore
        {
            let ipc = self.ipc.clone();
            if win.minimized {
                items.push(ContextMenuItem::action(
                    "window-restore-symbolic",
                    "Restore",
                    move || ipc.send(IpcRequest::FocusWindow { window_id: wid }),
                ));
            } else {
                items.push(ContextMenuItem::action_sc(
                    "window-minimize-symbolic",
                    "Minimize",
                    "Super+H",
                    move || ipc.send(IpcRequest::MinimizeWindow { window_id: wid }),
                ));
            }
        }

        // Maximize / Unmaximize
        {
            let ipc = self.ipc.clone();
            items.push(ContextMenuItem::action_sc(
                if win.maximized {
                    "window-restore-symbolic"
                } else {
                    "window-maximize-symbolic"
                },
                if win.maximized {
                    "Unmaximize"
                } else {
                    "Maximize"
                },
                "Super+↑",
                move || ipc.send(IpcRequest::ToggleMaximizeWindow { window_id: wid }),
            ));
        }

        // Fullscreen / Exit Fullscreen
        {
            let ipc = self.ipc.clone();
            items.push(ContextMenuItem::action_sc(
                if win.fullscreen {
                    "view-restore-symbolic"
                } else {
                    "view-fullscreen-symbolic"
                },
                if win.fullscreen {
                    "Exit Fullscreen"
                } else {
                    "Fullscreen"
                },
                "Super+F",
                move || ipc.send(IpcRequest::ToggleFullscreenWindow { window_id: wid }),
            ));
        }

        // Always on Top (live checkmark toggle)
        {
            let ipc = self.ipc.clone();
            items.push(ContextMenuItem::toggle(
                "Always on Top",
                win.always_on_top,
                false,
                move || ipc.send(IpcRequest::ToggleAlwaysOnTop { window_id: wid }),
            ));
        }

        items.push(ContextMenuItem::Separator);

        // Center
        {
            let ipc = self.ipc.clone();
            items.push(ContextMenuItem::action(
                "format-justify-center-symbolic",
                "Center",
                move || ipc.send(IpcRequest::CenterWindow { window_id: wid }),
            ));
        }

        // Tile ▶ submenu
        {
            let tile_regions = [
                ("go-previous-symbolic", "Left Half", Some("Super+←"), "left"),
                ("go-next-symbolic", "Right Half", Some("Super+→"), "right"),
                ("view-grid-symbolic", "Top Left Quarter", None, "top_left"),
                ("view-grid-symbolic", "Top Right Quarter", None, "top_right"),
                ("view-grid-symbolic", "Bottom Left Quarter", None, "bottom_left"),
                ("view-grid-symbolic", "Bottom Right Quarter", None, "bottom_right"),
            ];
            let mut sub: Vec<ContextMenuItem> = tile_regions
                .into_iter()
                .map(|(ic, label, sc, reg)| {
                    let ipc = self.ipc.clone();
                    let reg_s = reg.to_string();
                    if let Some(shortcut) = sc {
                        ContextMenuItem::action_sc(ic, label, shortcut, move || {
                            ipc.send(IpcRequest::SnapWindow {
                                window_id: wid,
                                region: reg_s.clone(),
                            });
                        })
                    } else {
                        ContextMenuItem::action(ic, label, move || {
                            ipc.send(IpcRequest::SnapWindow {
                                window_id: wid,
                                region: reg_s.clone(),
                            });
                        })
                    }
                })
                .collect();
            let ipc_flt = self.ipc.clone();
            sub.push(ContextMenuItem::Separator);
            sub.push(ContextMenuItem::action(
                "window-restore-symbolic",
                "Restore Floating",
                move || ipc_flt.send(IpcRequest::SetFloatingWindow { window_id: wid }),
            ));
            items.push(ContextMenuItem::submenu("view-dual-symbolic", "Tile", sub));
        }

        // Move to Workspace ▶
        if let Some(s) = snap {
            items.push(self.build_move_to_workspace_submenu(s, wid, Some(win.workspace_id)));

            // Context-aware Move to Monitor ▶ (only shown when 2+ monitors are connected!)
            if s.outputs.len() > 1 {
                let mon_sub: Vec<ContextMenuItem> = s
                    .outputs
                    .iter()
                    .map(|out| {
                        let out_name = out.name.clone();
                        let label = if out.primary {
                            format!("{} ({}×{} · Primary)", out.name, out.width, out.height)
                        } else {
                            format!("{} ({}×{})", out.name, out.width, out.height)
                        };
                        let on_this = win.x >= out.x
                            && win.x < out.x + out.width
                            && win.y >= out.y
                            && win.y < out.y + out.height;
                        let ipc = self.ipc.clone();
                        ContextMenuItem::toggle(label, on_this, true, move || {
                            ipc.send(IpcRequest::MoveWindowToOutput {
                                window_id: wid,
                                output_name: out_name.clone(),
                            });
                        })
                    })
                    .collect();
                items.push(ContextMenuItem::submenu(
                    "video-display-symbolic",
                    "Move to Monitor",
                    mon_sub,
                ));
            }
        }

        if include_app_actions_and_close {
            let matched = {
                let guard = self.all_apps.borrow();
                resolve_window_desktop_app(win.app_id.as_deref(), win.title.as_deref(), &guard)
                    .cloned()
            };
            if let Some(app) = matched {
                let has_actions = !app.actions.is_empty();
                items.push(ContextMenuItem::Separator);
                let app_new = app.clone();
                items.push(ContextMenuItem::action(
                    "window-new-symbolic",
                    format!("New {} Window", app.name),
                    move || app_new.launch(),
                ));
                if has_actions {
                    self.append_desktop_actions(&mut items, &app, true);
                }
            }

            items.push(ContextMenuItem::Separator);
            let ipc = self.ipc.clone();
            items.push(ContextMenuItem::destructive(
                "window-close-symbolic",
                "Close",
                Some("Super+Q"),
                move || ipc.send(IpcRequest::CloseWindow { window_id: wid }),
            ));
        }

        items
    }

    // =========================================================================
    // 5. PANELS & SYSTEM TRAY CONTEXT MENUS
    // =========================================================================

    pub fn open_panel_menu(&self, x: i32, y: i32, zone: PanelMenuZone) {
        let snap = self.last_snapshot.borrow().clone();
        let app_info = snap.as_ref().map(|s| s.appearance.clone()).unwrap_or_default();
        let sys = self.last_sys_snapshot.borrow().clone();

        let items = match zone {
            PanelMenuZone::Workspaces => {
                let mut list = self.build_workspaces_items(snap.as_ref());
                list.push(ContextMenuItem::Separator);
                let ipc_w = self.ipc.clone();
                let show_ws = app_info.show_workspaces;
                list.push(ContextMenuItem::toggle(
                    "Show Workspaces in Panel",
                    show_ws,
                    false,
                    move || {
                        ipc_w.send(IpcRequest::from(AppearanceUpdate {
                            show_workspaces: Some(!show_ws),
                            ..Default::default()
                        }));
                    },
                ));
                list
            }
            PanelMenuZone::Battery => {
                let mut list = self.build_battery_items(&sys);
                list.push(ContextMenuItem::Separator);
                let ipc_b = self.ipc.clone();
                let show_bat = app_info.show_battery;
                list.push(ContextMenuItem::toggle(
                    "Show Battery in Panel",
                    show_bat,
                    false,
                    move || {
                        ipc_b.send(IpcRequest::from(AppearanceUpdate {
                            show_battery: Some(!show_bat),
                            ..Default::default()
                        }));
                    },
                ));
                list
            }
            PanelMenuZone::Clock => {
                let fmt = if app_info.clock_24h {
                    "%A, %B %-e, %Y %H:%M"
                } else {
                    "%A, %B %-e, %Y %-I:%M %p"
                };
                let now_str = glib::DateTime::now_local()
                    .ok()
                    .and_then(|dt| dt.format(fmt).ok())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "Current Time".into());
                let copy_str = now_str.clone();
                let ipc_c = self.ipc.clone();
                let ipc_12h = self.ipc.clone();
                let show_clk = app_info.show_clock;
                let use_12h = !app_info.clock_24h;
                vec![
                    ContextMenuItem::Header(now_str),
                    ContextMenuItem::action("edit-copy-symbolic", "Copy Date & Time", move || {
                        copy_text_to_clipboard(&copy_str);
                    }),
                    ContextMenuItem::toggle("12-Hour Clock (AM/PM)", use_12h, false, move || {
                        ipc_12h.send(IpcRequest::from(AppearanceUpdate {
                            clock_24h: Some(use_12h),
                            ..Default::default()
                        }));
                    }),
                    ContextMenuItem::toggle("Show Clock in Panel", show_clk, false, move || {
                        ipc_c.send(IpcRequest::from(AppearanceUpdate {
                            show_clock: Some(!show_clk),
                            ..Default::default()
                        }));
                    }),
                    ContextMenuItem::Separator,
                    ContextMenuItem::action(
                        "preferences-system-time-symbolic",
                        "Date & Time Settings…",
                        || launch_settings_page(Some("datetime")),
                    ),
                ]
            }
            PanelMenuZone::QuickSettings => {
                let qs_hook = Rc::clone(&self.on_open_quick_settings);
                vec![
                    ContextMenuItem::Header("Control Center".into()),
                    ContextMenuItem::action(
                        "preferences-system-symbolic",
                        "Open Quick Settings",
                        move || {
                            if let Some(cb) = qs_hook.borrow().as_ref() {
                                cb();
                            }
                        },
                    ),
                    ContextMenuItem::Separator,
                    ContextMenuItem::submenu(
                        "network-wireless-symbolic",
                        "Network",
                        self.build_network_items(&sys),
                    ),
                    ContextMenuItem::submenu(
                        "bluetooth-active-symbolic",
                        "Bluetooth",
                        self.build_bluetooth_items(&sys),
                    ),
                    ContextMenuItem::submenu(
                        "audio-volume-high-symbolic",
                        "Audio",
                        self.build_audio_items(&sys),
                    ),
                    ContextMenuItem::submenu(
                        "battery-full-symbolic",
                        "Battery & Power",
                        self.build_battery_items(&sys),
                    ),
                    ContextMenuItem::Separator,
                    ContextMenuItem::action(
                        "preferences-system-symbolic",
                        "System Settings…",
                        || launch_settings_page(None),
                    ),
                ]
            }
            PanelMenuZone::General => {
                // Add Widgets ▶ (live panel widget toggles)
                let widgets_sub = {
                    let ipc_ws = self.ipc.clone();
                    let ipc_at = self.ipc.clone();
                    let ipc_tr = self.ipc.clone();
                    let ipc_ba = self.ipc.clone();
                    let ipc_qs = self.ipc.clone();
                    let ipc_cl = self.ipc.clone();
                    vec![
                        ContextMenuItem::toggle(
                            "Workspaces Indicator",
                            app_info.show_workspaces,
                            false,
                            move || {
                                ipc_ws.send(IpcRequest::from(AppearanceUpdate {
                                    show_workspaces: Some(!app_info.show_workspaces),
                                    ..Default::default()
                                }));
                            },
                        ),
                        ContextMenuItem::toggle(
                            "Active App Title",
                            app_info.show_app_title,
                            false,
                            move || {
                                ipc_at.send(IpcRequest::from(AppearanceUpdate {
                                    show_app_title: Some(!app_info.show_app_title),
                                    ..Default::default()
                                }));
                            },
                        ),
                        ContextMenuItem::toggle(
                            "System Tray",
                            app_info.show_system_tray,
                            false,
                            move || {
                                ipc_tr.send(IpcRequest::from(AppearanceUpdate {
                                    show_system_tray: Some(!app_info.show_system_tray),
                                    ..Default::default()
                                }));
                            },
                        ),
                        ContextMenuItem::toggle(
                            "Battery & Power",
                            app_info.show_battery,
                            false,
                            move || {
                                ipc_ba.send(IpcRequest::from(AppearanceUpdate {
                                    show_battery: Some(!app_info.show_battery),
                                    ..Default::default()
                                }));
                            },
                        ),
                        ContextMenuItem::toggle(
                            "Quick Settings",
                            app_info.show_quick_settings,
                            false,
                            move || {
                                ipc_qs.send(IpcRequest::from(AppearanceUpdate {
                                    show_quick_settings: Some(!app_info.show_quick_settings),
                                    ..Default::default()
                                }));
                            },
                        ),
                        ContextMenuItem::toggle(
                            "Date & Clock",
                            app_info.show_clock,
                            false,
                            move || {
                                ipc_cl.send(IpcRequest::from(AppearanceUpdate {
                                    show_clock: Some(!app_info.show_clock),
                                    ..Default::default()
                                }));
                            },
                        ),
                    ]
                };

                // Edit Panel ▶
                let edit_sub = {
                    let ipc_sh = self.ipc.clone();
                    let sh = app_info.panel_smart_hide;
                    let mut list = vec![
                        ContextMenuItem::toggle("Smart Hide Top Bar", sh, false, move || {
                            ipc_sh.send(IpcRequest::from(AppearanceUpdate {
                                panel_smart_hide: Some(!sh),
                                ..Default::default()
                            }));
                        }),
                        ContextMenuItem::Separator,
                        ContextMenuItem::Header("Panel Opacity".into()),
                    ];
                    for (lbl, val) in [
                        ("Translucent (60%)", 0.60f32),
                        ("Balanced (76%)", 0.76f32),
                        ("Frosted (88%)", 0.88f32),
                        ("Solid (100%)", 1.00f32),
                    ] {
                        let ipc_o = self.ipc.clone();
                        let active = (app_info.panel_transparency - val).abs() < 0.06;
                        list.push(ContextMenuItem::toggle(lbl, active, true, move || {
                            ipc_o.send(IpcRequest::from(AppearanceUpdate {
                                panel_transparency: Some(val),
                                ..Default::default()
                            }));
                        }));
                    }
                    list
                };

                let ipc_shot = self.ipc.clone();
                let ipc_lock = self.ipc.clone();
                let mut list = vec![
                    ContextMenuItem::submenu("list-add-symbolic", "Add Widgets", widgets_sub),
                    ContextMenuItem::submenu("document-edit-symbolic", "Edit Panel", edit_sub),
                    ContextMenuItem::action(
                        "preferences-system-symbolic",
                        "Panel Settings…",
                        || launch_settings_page(Some("dock")),
                    ),
                    ContextMenuItem::Separator,
                    ContextMenuItem::submenu(
                        "network-wireless-symbolic",
                        "Network",
                        self.build_network_items(&sys),
                    ),
                    ContextMenuItem::submenu(
                        "bluetooth-active-symbolic",
                        "Bluetooth",
                        self.build_bluetooth_items(&sys),
                    ),
                    ContextMenuItem::submenu(
                        "audio-volume-high-symbolic",
                        "Audio",
                        self.build_audio_items(&sys),
                    ),
                    ContextMenuItem::submenu(
                        "battery-full-symbolic",
                        "Battery & Power",
                        self.build_battery_items(&sys),
                    ),
                    ContextMenuItem::Separator,
                    ContextMenuItem::action_sc(
                        "camera-photo-symbolic",
                        "Screenshot & Record…",
                        "Super+Shift+4",
                        move || {
                            let ipc_c = ipc_shot.clone();
                            glib::timeout_add_local_once(Duration::from_millis(80), move || {
                                ipc_c.send(IpcRequest::OpenScreenshotUi);
                            });
                        },
                    ),
                    ContextMenuItem::action_sc(
                        "system-lock-screen-symbolic",
                        "Lock Screen",
                        "Super+L",
                        move || ipc_lock.send(IpcRequest::LockSession),
                    ),
                    ContextMenuItem::action(
                        "preferences-system-symbolic",
                        "System Settings…",
                        || launch_settings_page(None),
                    ),
                ];

                // If a window is currently focused, include a concise focused-window submenu at top
                if let Some(focused) = snap
                    .as_ref()
                    .and_then(|s| s.windows.iter().find(|w| w.focused))
                {
                    let title = focused
                        .title
                        .as_deref()
                        .or(focused.app_id.as_deref())
                        .unwrap_or("Active Window");
                    let short: String = title.chars().take(20).collect();
                    let win_items = self.build_window_actions_list(focused, snap.as_ref(), true);
                    list.insert(0, ContextMenuItem::Separator);
                    list.insert(
                        0,
                        ContextMenuItem::submenu(
                            "preferences-system-windows-symbolic",
                            short,
                            win_items,
                        ),
                    );
                }
                list
            }
        };

        self.show_at(x, y, items);
    }

    /// Displays a System Tray (`StatusNotifierItem`) DBusMenu using the EXACT same unified macOS
    /// context menu engine, including true cascading submenus!
    #[allow(clippy::too_many_arguments)]
    pub fn open_tray_menu(
        &self,
        x: i32,
        y: i32,
        app_title: &str,
        bus_name: &str,
        object_path: &str,
        menu_path: Option<&str>,
        menu_items: &[TrayMenuItem],
        tray_tx: TrayActionSender,
    ) {
        let mut items: Vec<ContextMenuItem> = Vec::new();
        if !app_title.is_empty() {
            items.push(ContextMenuItem::Header(app_title.to_string()));
        }

        if let Some(mpath) = menu_path {
            let converted = convert_dbus_menu(menu_items, bus_name, mpath, &tray_tx);
            items.extend(converted);
        }

        let has_actions = items
            .iter()
            .any(|i| matches!(i, ContextMenuItem::Action { .. } | ContextMenuItem::Submenu { .. }));
        if !has_actions {
            let b = bus_name.to_string();
            let op = object_path.to_string();
            let lbl = if app_title.is_empty() {
                "Open Application".to_string()
            } else {
                format!("Open {app_title}")
            };
            items.push(ContextMenuItem::action("window-new-symbolic", lbl, move || {
                tray_tx.send(TrayAction::Activate {
                    bus_name: b.clone(),
                    object_path: op.clone(),
                    x,
                    y,
                });
            }));
        }

        self.show_at(x, y, items);
    }

    // =========================================================================
    // SHARED LIVE SUBMENU BUILDERS
    // =========================================================================

    fn effective_pinned_ids(&self, snap: Option<&DesktopSnapshot>) -> Vec<String> {
        let configured = snap
            .map(|s| s.appearance.dock_pinned_apps.clone())
            .unwrap_or_default();
        if !configured.is_empty() {
            return configured;
        }
        let guard = self.all_apps.borrow();
        select_favorites_with_pins(&guard, &[])
            .into_iter()
            .map(|a| a.id)
            .collect()
    }

    fn append_desktop_actions(
        &self,
        items: &mut Vec<ContextMenuItem>,
        app: &DesktopApp,
        skip_plain_new_window: bool,
    ) {
        for act in app.actions.iter().take(6) {
            if skip_plain_new_window && act.name.eq_ignore_ascii_case("new window") {
                continue;
            }
            let act_c = act.clone();
            let wd = app.working_dir.clone();
            let term = app.terminal;
            let ipc = self.ipc.clone();
            let ic = act
                .icon
                .as_deref()
                .filter(|s| !s.contains('/'))
                .unwrap_or("system-run-symbolic");
            items.push(ContextMenuItem::action(ic, act.name.clone(), move || {
                act_c.launch(wd.as_deref(), term);
                ipc.send(IpcRequest::SetOverview { open: false });
            }));
        }
    }

    fn build_workspaces_submenu(&self, snap: Option<&DesktopSnapshot>) -> ContextMenuItem {
        ContextMenuItem::submenu(
            "view-paged-symbolic",
            "Workspaces",
            self.build_workspaces_items(snap),
        )
    }

    fn build_workspaces_items(&self, snap: Option<&DesktopSnapshot>) -> Vec<ContextMenuItem> {
        let mut sub: Vec<ContextMenuItem> = Vec::new();
        let active_idx = snap.map(|s| s.active_workspace_index).unwrap_or(0);
        if let Some(s) = snap {
            for ws in &s.workspaces {
                let idx = ws.index;
                let count = ws.window_ids.len();
                let label = if count == 0 {
                    format!("Desktop {}", idx + 1)
                } else if count == 1 {
                    format!("Desktop {} (1 window)", idx + 1)
                } else {
                    format!("Desktop {} ({count} windows)", idx + 1)
                };
                let ipc = self.ipc.clone();
                sub.push(ContextMenuItem::toggle(label, ws.active, true, move || {
                    ipc.send(IpcRequest::SwitchWorkspace { index: idx });
                }));
            }
            sub.push(ContextMenuItem::Separator);
            let ipc_add = self.ipc.clone();
            sub.push(ContextMenuItem::action("list-add-symbolic", "Add Workspace", move || {
                ipc_add.send(IpcRequest::AddWorkspace);
            }));
            if let Some(cur) = s.workspaces.get(active_idx) {
                let pinned = cur.pinned;
                let ipc_pin = self.ipc.clone();
                sub.push(ContextMenuItem::toggle(
                    "Pin Current Workspace",
                    pinned,
                    false,
                    move || {
                        ipc_pin.send(IpcRequest::TogglePinWorkspace { index: active_idx });
                    },
                ));
            }
            if s.workspaces.len() > 1 {
                let ipc_rem = self.ipc.clone();
                sub.push(ContextMenuItem::destructive(
                    "list-remove-symbolic",
                    format!("Remove Desktop {}", active_idx + 1),
                    None,
                    move || {
                        ipc_rem.send(IpcRequest::RemoveWorkspace { index: active_idx });
                    },
                ));
            }
        }
        let ws_hook = Rc::clone(&self.on_open_workspaces);
        sub.push(ContextMenuItem::Separator);
        sub.push(ContextMenuItem::action(
            "view-dual-symbolic",
            "Open Workspaces Sidebar",
            move || {
                if let Some(cb) = ws_hook.borrow().as_ref() {
                    cb();
                }
            },
        ));
        sub
    }

    fn build_move_to_workspace_submenu(
        &self,
        snap: &DesktopSnapshot,
        window_id: u64,
        current_ws_id: Option<u64>,
    ) -> ContextMenuItem {
        let mut sub: Vec<ContextMenuItem> = snap
            .workspaces
            .iter()
            .map(|ws| {
                let idx = ws.index;
                let is_cur = current_ws_id == Some(ws.id);
                let ipc = self.ipc.clone();
                ContextMenuItem::toggle(format!("Desktop {}", idx + 1), is_cur, true, move || {
                    ipc.send(IpcRequest::MoveWindowToWorkspace {
                        window_id,
                        workspace_index: idx,
                    });
                })
            })
            .collect();

        if snap.workspaces.len() < 10 {
            let next_idx = snap.workspaces.len();
            let ipc = self.ipc.clone();
            sub.push(ContextMenuItem::Separator);
            sub.push(ContextMenuItem::action(
                "list-add-symbolic",
                "Move to New Workspace",
                move || {
                    ipc.send(IpcRequest::AddWorkspace);
                    let ipc2 = ipc.clone();
                    glib::timeout_add_local_once(Duration::from_millis(60), move || {
                        ipc2.send(IpcRequest::MoveWindowToWorkspace {
                            window_id,
                            workspace_index: next_idx,
                        });
                    });
                },
            ));
        }

        ContextMenuItem::submenu("view-paged-symbolic", "Move to Workspace", sub)
    }

    fn build_network_items(&self, sys: &SystemSnapshot) -> Vec<ContextMenuItem> {
        let mut sub = Vec::new();
        let status_hdr = if let Some(ref name) = sys.network.primary_name {
            match sys.network.primary_kind {
                neon_system::network::ConnectionKind::Wifi => format!("Wi-Fi · {name}"),
                neon_system::network::ConnectionKind::Ethernet => format!("Ethernet · {name}"),
                neon_system::network::ConnectionKind::Vpn => format!("VPN · {name}"),
                _ => name.clone(),
            }
        } else if sys.network.wifi_enabled {
            "Wi-Fi · Disconnected".to_string()
        } else {
            "Wi-Fi Off".to_string()
        };
        sub.push(ContextMenuItem::Header(status_hdr));

        let wifi_on = sys.network.wifi_enabled;
        let tx = self.sys_tx.clone();
        sub.push(ContextMenuItem::toggle("Wi-Fi Enabled", wifi_on, false, move || {
            tx.send(SystemAction::SetWifiEnabled(!wifi_on));
        }));
        sub.push(ContextMenuItem::Separator);
        sub.push(ContextMenuItem::action(
            "network-wireless-symbolic",
            "Network Settings…",
            || launch_settings_page(Some("network")),
        ));
        sub
    }

    fn build_bluetooth_items(&self, sys: &SystemSnapshot) -> Vec<ContextMenuItem> {
        let mut sub = Vec::new();
        let bt_on = sys.bluetooth.powered;
        let tx = self.sys_tx.clone();
        sub.push(ContextMenuItem::toggle("Bluetooth Enabled", bt_on, false, move || {
            tx.send(SystemAction::SetBluetoothPowered(!bt_on));
        }));
        if !sys.bluetooth.devices.is_empty() {
            sub.push(ContextMenuItem::Separator);
            for dev in sys.bluetooth.devices.iter().take(6) {
                let path = dev.object_path.clone();
                let next_conn = !dev.connected;
                let tx_d = self.sys_tx.clone();
                sub.push(ContextMenuItem::toggle(
                    dev.name.clone(),
                    dev.connected,
                    false,
                    move || {
                        tx_d.send(SystemAction::SetBluetoothDeviceConnected {
                            object_path: path.clone(),
                            connect: next_conn,
                        });
                    },
                ));
            }
        }
        sub.push(ContextMenuItem::Separator);
        sub.push(ContextMenuItem::action(
            "bluetooth-active-symbolic",
            "Bluetooth Settings…",
            || launch_settings_page(Some("bluetooth")),
        ));
        sub
    }

    fn build_audio_items(&self, sys: &SystemSnapshot) -> Vec<ContextMenuItem> {
        let mut sub = Vec::new();
        let vol_pct = (sys.audio.volume * 100.0).round() as i32;
        sub.push(ContextMenuItem::Header(format!(
            "Volume · {}%{}",
            vol_pct,
            if sys.audio.muted { " (Muted)" } else { "" }
        )));

        let tx_m = self.sys_tx.clone();
        sub.push(ContextMenuItem::toggle(
            "Mute Output",
            sys.audio.muted,
            false,
            move || tx_m.send(SystemAction::ToggleSinkMute),
        ));
        let tx_mic = self.sys_tx.clone();
        sub.push(ContextMenuItem::toggle(
            "Mute Microphone",
            sys.audio.mic_muted,
            false,
            move || tx_mic.send(SystemAction::ToggleSourceMute),
        ));

        sub.push(ContextMenuItem::Separator);
        for (lbl, v) in [
            ("Volume 25%", 0.25f32),
            ("Volume 50%", 0.50f32),
            ("Volume 75%", 0.75f32),
            ("Volume 100%", 1.00f32),
        ] {
            let tx_v = self.sys_tx.clone();
            let close = !sys.audio.muted && (sys.audio.volume - v).abs() < 0.08;
            sub.push(ContextMenuItem::toggle(lbl, close, true, move || {
                tx_v.send(SystemAction::SetSinkMute(false));
                tx_v.send(SystemAction::SetSinkVolume(v));
            }));
        }

        if sys.audio.sinks.len() > 1 {
            sub.push(ContextMenuItem::Separator);
            sub.push(ContextMenuItem::Header("Output Device".into()));
            for sink in sys.audio.sinks.iter().take(5) {
                let id = sink.id;
                let tx_s = self.sys_tx.clone();
                sub.push(ContextMenuItem::toggle(
                    sink.name.clone(),
                    sink.is_default,
                    true,
                    move || tx_s.send(SystemAction::SetDefaultSink(id)),
                ));
            }
        }

        sub.push(ContextMenuItem::Separator);
        sub.push(ContextMenuItem::action(
            "audio-volume-high-symbolic",
            "Sound Settings…",
            || launch_settings_page(Some("sound")),
        ));
        sub
    }

    fn build_battery_items(&self, sys: &SystemSnapshot) -> Vec<ContextMenuItem> {
        let mut sub = Vec::new();
        let hdr = if sys.power.has_battery {
            format!(
                "Battery · {:.0}% ({})",
                sys.power.battery_percentage.clamp(0.0, 100.0),
                sys.power.battery_state.label()
            )
        } else {
            "Power Source · AC".to_string()
        };
        sub.push(ContextMenuItem::Header(hdr));

        let active_prof = sys.power.active_profile.as_deref().unwrap_or("balanced");
        for (id, label) in [
            ("power-saver", "Power Saver"),
            ("balanced", "Balanced"),
            ("performance", "Performance"),
        ] {
            let tx = self.sys_tx.clone();
            sub.push(ContextMenuItem::toggle(
                label,
                active_prof.eq_ignore_ascii_case(id),
                true,
                move || tx.send(SystemAction::SetPowerProfile(id.to_string())),
            ));
        }

        sub.push(ContextMenuItem::Separator);
        sub.push(ContextMenuItem::action(
            "battery-full-symbolic",
            "Power & Battery Settings…",
            || launch_settings_page(Some("power")),
        ));
        sub
    }
}

// =========================================================================
// HELPERS
// =========================================================================

fn set_focus_class(rows: &[RowHandle], prev: Option<usize>, next: Option<usize>) {
    if let Some(p) = prev
        && let Some(r) = rows.get(p)
    {
        r.btn.remove_css_class("focused");
    }
    if let Some(n) = next
        && let Some(r) = rows.get(n)
    {
        r.btn.add_css_class("focused");
    }
}

fn next_enabled_index(rows: &[RowHandle], current: Option<usize>, delta: isize) -> Option<usize> {
    let len = rows.len();
    if len == 0 {
        return None;
    }
    let start = match current {
        Some(c) => (c as isize + delta).rem_euclid(len as isize) as usize,
        None => {
            if delta >= 0 {
                0
            } else {
                len - 1
            }
        }
    };
    for step in 0..len {
        let idx = (start as isize + step as isize * delta.signum()).rem_euclid(len as isize) as usize;
        if rows[idx].enabled {
            return Some(idx);
        }
    }
    None
}

fn clone_menu_items(items: &[ContextMenuItem]) -> Vec<ContextMenuItem> {
    items
        .iter()
        .map(|it| match it {
            ContextMenuItem::Header(h) => ContextMenuItem::Header(h.clone()),
            ContextMenuItem::Separator => ContextMenuItem::Separator,
            ContextMenuItem::Action {
                icon,
                label,
                shortcut,
                checked,
                radio,
                enabled,
                destructive,
                on_click,
            } => ContextMenuItem::Action {
                icon: icon.clone(),
                label: label.clone(),
                shortcut: shortcut.clone(),
                checked: *checked,
                radio: *radio,
                enabled: *enabled,
                destructive: *destructive,
                on_click: Rc::clone(on_click),
            },
            ContextMenuItem::Submenu {
                icon,
                label,
                enabled,
                items,
            } => ContextMenuItem::Submenu {
                icon: icon.clone(),
                label: label.clone(),
                enabled: *enabled,
                items: clone_menu_items(items),
            },
        })
        .collect()
}

fn convert_dbus_menu(
    entries: &[TrayMenuItem],
    bus_name: &str,
    menu_path: &str,
    tray_tx: &TrayActionSender,
) -> Vec<ContextMenuItem> {
    let mut out = Vec::new();
    for entry in entries {
        if !entry.visible {
            continue;
        }
        if entry.is_separator {
            out.push(ContextMenuItem::Separator);
            continue;
        }
        let label = entry.label.trim().to_string();
        if label.is_empty() && entry.children.is_empty() {
            continue;
        }
        if !entry.children.is_empty() {
            let sub = convert_dbus_menu(&entry.children, bus_name, menu_path, tray_tx);
            if !sub.is_empty() {
                out.push(ContextMenuItem::Submenu {
                    icon: entry.icon_name.clone(),
                    label: if label.is_empty() { "More".into() } else { label },
                    enabled: entry.enabled,
                    items: sub,
                });
            }
            continue;
        }

        let lower = label.to_lowercase();
        let destructive = lower == "quit"
            || lower == "exit"
            || lower.starts_with("quit ")
            || lower.starts_with("exit ");
        let checked = entry.toggle_type.as_ref().map(|_| entry.toggle_state == 1);
        let radio = entry
            .toggle_type
            .as_deref()
            .is_some_and(|t| t.eq_ignore_ascii_case("radio"));
        let tx = tray_tx.clone();
        let b = bus_name.to_string();
        let mp = menu_path.to_string();
        let id = entry.id;
        out.push(ContextMenuItem::Action {
            icon: entry.icon_name.clone(),
            label,
            shortcut: None,
            checked,
            radio,
            enabled: entry.enabled,
            destructive,
            on_click: Rc::new(move || {
                tx.send(TrayAction::TriggerMenuItem {
                    bus_name: b.clone(),
                    menu_path: mp.clone(),
                    menu_item_id: id,
                });
            }),
        });
    }
    out
}

pub fn launch_settings_page(page: Option<&str>) {
    let mut cmd = if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
        && dir.join("neon-settings").exists()
    {
        std::process::Command::new(dir.join("neon-settings"))
    } else {
        std::process::Command::new("neon-settings")
    };
    if let Some(p) = page {
        cmd.arg("--page").arg(p);
    }
    let _ = cmd.spawn();
}

#[allow(dead_code)]
fn _path_unused(_: &Path) {}
