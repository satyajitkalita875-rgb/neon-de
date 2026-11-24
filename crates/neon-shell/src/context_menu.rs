//! NEON DE System-Wide macOS-Style Right-Click Context Menu (`zwlr_layer_shell_v1` `Layer::Overlay`).
//!
//! Provides unified native context menus across the entire desktop environment:
//! - Desktop / Wallpaper right-click (Terminal, Files, New Folder, Change Wallpaper, Displays, Screenshot, Lock)
//! - Dock app icons & Dock bar right-click (Focus, Minimize, Maximize, Snap, Move Workspace, New Window, Close)
//! - Top Menu Bar right-click (Focused window actions, Terminal, Screenshot, Settings, Lock)
//! - Window right-click (SSD titlebars or Super+Right-Click on any app window)

use std::{
    cell::RefCell,
    fs,
    path::PathBuf,
    process::Command,
    rc::Rc,
    time::Duration,
};

use gtk4::{
    Align, Application, ApplicationWindow, Box as GtkBox, Button, EventControllerKey, Fixed,
    GestureClick, Image, Label, Orientation, Separator, gdk, glib, prelude::*,
};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use neon_ipc::{ContextMenuTarget, DesktopSnapshot, IpcRequest, WindowInfo};

use crate::{
    desktop_apps::DesktopApp,
    ipc_bridge::IpcCommandSender,
};

type MenuCallback = Box<dyn Fn()>;

/// A single entry inside a NEON DE right-click context menu.
pub enum ContextMenuItem {
    Action {
        icon: &'static str,
        label: String,
        shortcut: Option<&'static str>,
        destructive: bool,
        on_click: MenuCallback,
    },
    Separator,
}

/// Reusable fullscreen click-catcher + positioned macOS frosted-glass context menu overlay.
#[derive(Clone)]
pub struct ContextMenuOverlay {
    window: ApplicationWindow,
    fixed: Fixed,
    menu_card: GtkBox,
    last_snapshot: Rc<RefCell<Option<DesktopSnapshot>>>,
    all_apps: Rc<Vec<DesktopApp>>,
    ipc: IpcCommandSender,
}

impl ContextMenuOverlay {
    pub fn build(app: &Application, all_apps: Vec<DesktopApp>, ipc: IpcCommandSender) -> Self {
        let window = ApplicationWindow::builder()
            .application(app)
            .title("NEON DE Context Menu")
            .decorated(false)
            .build();
        window.add_css_class("neon-contextmenu-window");

        if gtk4_layer_shell::is_supported() {
            window.init_layer_shell();
            window.set_namespace(Some("neon-context-menu"));
            window.set_layer(Layer::Overlay);
            window.set_anchor(Edge::Top, true);
            window.set_anchor(Edge::Bottom, true);
            window.set_anchor(Edge::Left, true);
            window.set_anchor(Edge::Right, true);
            window.set_exclusive_zone(-1);
            window.set_keyboard_mode(KeyboardMode::None);
        }

        let fixed = Fixed::new();
        fixed.set_hexpand(true);
        fixed.set_vexpand(true);

        let menu_card = GtkBox::new(Orientation::Vertical, 2);
        menu_card.add_css_class("neon-ctx-menu");
        menu_card.set_width_request(236);
        fixed.put(&menu_card, 0.0, 0.0);

        window.set_child(Some(&fixed));
        window.set_visible(false);

        let overlay = Self {
            window,
            fixed,
            menu_card,
            last_snapshot: Rc::new(RefCell::new(None)),
            all_apps: Rc::new(all_apps),
            ipc,
        };

        // Clicking anywhere outside the menu card (left or right click) dismisses it immediately.
        let outside_click = GestureClick::new();
        outside_click.set_button(0);
        {
            let ov = overlay.clone();
            let card = overlay.menu_card.clone();
            outside_click.connect_pressed(move |gesture, _, x, y| {
                if let Some(widget) = gesture.widget()
                    && let Some(picked) = widget.pick(x, y, gtk4::PickFlags::DEFAULT)
                {
                    let inside_card = picked == card.clone().upcast::<gtk4::Widget>()
                        || picked.is_ancestor(&card);
                    if !inside_card {
                        ov.close();
                    }
                } else {
                    ov.close();
                }
            });
        }
        overlay.fixed.add_controller(outside_click);

        // Escape dismisses the context menu immediately.
        let key_ctrl = EventControllerKey::new();
        {
            let ov = overlay.clone();
            key_ctrl.connect_key_pressed(move |_, key, _, _| {
                if key == gdk::Key::Escape {
                    ov.close();
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            });
        }
        overlay.window.add_controller(key_ctrl);

        overlay
    }

    pub fn update_snapshot(&self, snapshot: &DesktopSnapshot) {
        *self.last_snapshot.borrow_mut() = Some(snapshot.clone());
    }

    pub fn output_size(&self) -> (i32, i32) {
        self.last_snapshot
            .borrow()
            .as_ref()
            .and_then(|s| s.outputs.iter().find(|o| o.primary).or(s.outputs.first()))
            .map(|o| (o.width.max(800), o.height.max(600)))
            .unwrap_or((1920, 1080))
    }

    pub fn close(&self) {
        if gtk4_layer_shell::is_supported() {
            self.window.set_keyboard_mode(KeyboardMode::None);
        }
        self.window.set_visible(false);
    }

    pub fn show_at(&self, x: i32, y: i32, items: Vec<ContextMenuItem>) {
        while let Some(child) = self.menu_card.first_child() {
            self.menu_card.remove(&child);
        }

        let mut estimated_h: i32 = 16;
        for item in items {
            match item {
                ContextMenuItem::Separator => {
                    let sep = Separator::new(Orientation::Horizontal);
                    sep.add_css_class("neon-ctx-sep");
                    self.menu_card.append(&sep);
                    estimated_h += 9;
                }
                ContextMenuItem::Action {
                    icon,
                    label,
                    shortcut,
                    destructive,
                    on_click,
                } => {
                    let btn = Button::new();
                    btn.add_css_class("neon-ctx-item");
                    if destructive {
                        btn.add_css_class("destructive");
                    }

                    let row = GtkBox::new(Orientation::Horizontal, 10);
                    row.set_valign(Align::Center);

                    let img = Image::from_icon_name(icon);
                    img.set_pixel_size(14);
                    img.add_css_class("neon-ctx-icon");
                    row.append(&img);

                    let lbl = Label::new(Some(&label));
                    lbl.set_halign(Align::Start);
                    lbl.set_hexpand(true);
                    lbl.set_ellipsize(gtk4::pango::EllipsizeMode::End);
                    lbl.set_max_width_chars(26);
                    row.append(&lbl);

                    if let Some(sc) = shortcut {
                        let sc_lbl = Label::new(Some(sc));
                        sc_lbl.add_css_class("neon-ctx-shortcut");
                        row.append(&sc_lbl);
                    }

                    btn.set_child(Some(&row));

                    let ov = self.clone();
                    btn.connect_clicked(move |_| {
                        ov.close();
                        on_click();
                    });

                    self.menu_card.append(&btn);
                    estimated_h += 32;
                }
            }
        }

        let (out_w, out_h) = self.output_size();
        let menu_w = 244;
        let pad = 10;
        let clamped_x = x.clamp(pad, (out_w - menu_w - pad).max(pad));
        let clamped_y = y.clamp(34, (out_h - estimated_h - pad).max(34));

        self.fixed
            .move_(&self.menu_card, f64::from(clamped_x), f64::from(clamped_y));

        if gtk4_layer_shell::is_supported() {
            self.window.set_keyboard_mode(KeyboardMode::OnDemand);
        }
        self.window.set_visible(true);
        self.window.present();
    }

    /// Handles a compositor-initiated context menu request (`Desktop` or `Window`).
    pub fn handle_compositor_request(&self, x: i32, y: i32, target: ContextMenuTarget) {
        match target {
            ContextMenuTarget::Desktop => self.open_desktop_menu(x, y),
            ContextMenuTarget::Window { window_id } => self.open_window_menu(x, y, window_id),
        }
    }

    /// Right-click menu for the Desktop / Wallpaper canvas.
    pub fn open_desktop_menu(&self, x: i32, y: i32) {
        let ipc_shot = self.ipc.clone();
        let ipc_ov = self.ipc.clone();
        let ipc_lock = self.ipc.clone();

        let items = vec![
            ContextMenuItem::Action {
                icon: "utilities-terminal-symbolic",
                label: "Open Terminal".into(),
                shortcut: Some("Ctrl+Alt+T"),
                destructive: false,
                on_click: Box::new(launch_default_terminal),
            },
            ContextMenuItem::Action {
                icon: "system-file-manager-symbolic",
                label: "Open Files".into(),
                shortcut: None,
                destructive: false,
                on_click: Box::new(launch_default_files),
            },
            ContextMenuItem::Action {
                icon: "folder-new-symbolic",
                label: "New Folder on Desktop".into(),
                shortcut: None,
                destructive: false,
                on_click: Box::new(create_desktop_folder_and_open),
            },
            ContextMenuItem::Separator,
            ContextMenuItem::Action {
                icon: "preferences-desktop-wallpaper-symbolic",
                label: "Change Wallpaper…".into(),
                shortcut: None,
                destructive: false,
                on_click: Box::new(|| launch_settings_page(Some("appearance"))),
            },
            ContextMenuItem::Action {
                icon: "video-display-symbolic",
                label: "Display Settings…".into(),
                shortcut: None,
                destructive: false,
                on_click: Box::new(|| launch_settings_page(Some("displays"))),
            },
            ContextMenuItem::Action {
                icon: "preferences-system-symbolic",
                label: "System Settings…".into(),
                shortcut: None,
                destructive: false,
                on_click: Box::new(|| launch_settings_page(None)),
            },
            ContextMenuItem::Separator,
            ContextMenuItem::Action {
                icon: "camera-photo-symbolic",
                label: "Take Screenshot…".into(),
                shortcut: Some("Super+Shift+4"),
                destructive: false,
                on_click: Box::new(move || {
                    let ipc_c = ipc_shot.clone();
                    glib::timeout_add_local_once(Duration::from_millis(80), move || {
                        ipc_c.send(IpcRequest::OpenScreenshotUi);
                    });
                }),
            },
            ContextMenuItem::Action {
                icon: "view-app-grid-symbolic",
                label: "Applications & Mission Control".into(),
                shortcut: Some("Super+Space"),
                destructive: false,
                on_click: Box::new(move || {
                    ipc_ov.send(IpcRequest::ToggleOverview);
                }),
            },
            ContextMenuItem::Separator,
            ContextMenuItem::Action {
                icon: "system-lock-screen-symbolic",
                label: "Lock Screen".into(),
                shortcut: Some("Super+L"),
                destructive: false,
                on_click: Box::new(move || {
                    ipc_lock.send(IpcRequest::LockSession);
                }),
            },
        ];

        self.show_at(x, y, items);
    }

    /// Right-click menu for any window (from SSD titlebar, Super+Right-Click, or Dock running icon).
    pub fn open_window_menu(&self, x: i32, y: i32, window_id: u64) {
        let win_info: Option<WindowInfo> = self
            .last_snapshot
            .borrow()
            .as_ref()
            .and_then(|s| s.windows.iter().find(|w| w.id == window_id).cloned());
        let workspace_count = self
            .last_snapshot
            .borrow()
            .as_ref()
            .map(|s| s.workspaces.len())
            .unwrap_or(1);
        let active_ws = self
            .last_snapshot
            .borrow()
            .as_ref()
            .map(|s| s.active_workspace_index)
            .unwrap_or(0);

        let is_min = win_info.as_ref().map(|w| w.minimized).unwrap_or(false);
        let is_max = win_info.as_ref().map(|w| w.maximized).unwrap_or(false);
        let matched_app = win_info
            .as_ref()
            .and_then(|w| find_matching_desktop_app(w, &self.all_apps));

        let mut items: Vec<ContextMenuItem> = Vec::new();

        if is_min {
            let ipc = self.ipc.clone();
            items.push(ContextMenuItem::Action {
                icon: "window-restore-symbolic",
                label: "Restore Window".into(),
                shortcut: None,
                destructive: false,
                on_click: Box::new(move || {
                    ipc.send(IpcRequest::FocusWindow { window_id });
                }),
            });
        } else {
            let ipc = self.ipc.clone();
            items.push(ContextMenuItem::Action {
                icon: "window-minimize-symbolic",
                label: "Minimize".into(),
                shortcut: Some("Super+H"),
                destructive: false,
                on_click: Box::new(move || {
                    ipc.send(IpcRequest::MinimizeWindow { window_id });
                }),
            });
        }

        {
            let ipc = self.ipc.clone();
            items.push(ContextMenuItem::Action {
                icon: if is_max {
                    "window-restore-symbolic"
                } else {
                    "window-maximize-symbolic"
                },
                label: if is_max {
                    "Unmaximize".into()
                } else {
                    "Maximize".into()
                },
                shortcut: Some("Super+↑"),
                destructive: false,
                on_click: Box::new(move || {
                    ipc.send(IpcRequest::ToggleMaximizeWindow { window_id });
                }),
            });
        }

        {
            let ipc_l = self.ipc.clone();
            let ipc_r = self.ipc.clone();
            items.push(ContextMenuItem::Separator);
            items.push(ContextMenuItem::Action {
                icon: "go-previous-symbolic",
                label: "Snap Left Half".into(),
                shortcut: Some("Super+←"),
                destructive: false,
                on_click: Box::new(move || {
                    ipc_l.send(IpcRequest::SnapWindow {
                        window_id,
                        region: "left".into(),
                    });
                }),
            });
            items.push(ContextMenuItem::Action {
                icon: "go-next-symbolic",
                label: "Snap Right Half".into(),
                shortcut: Some("Super+→"),
                destructive: false,
                on_click: Box::new(move || {
                    ipc_r.send(IpcRequest::SnapWindow {
                        window_id,
                        region: "right".into(),
                    });
                }),
            });
        }

        if workspace_count > 1 {
            let target_ws = (active_ws + 1) % workspace_count;
            let ipc = self.ipc.clone();
            items.push(ContextMenuItem::Action {
                icon: "view-paged-symbolic",
                label: format!("Move to Desktop {}", target_ws + 1),
                shortcut: None,
                destructive: false,
                on_click: Box::new(move || {
                    ipc.send(IpcRequest::MoveWindowToWorkspace {
                        window_id,
                        workspace_index: target_ws,
                    });
                }),
            });
        }

        if let Some(app) = matched_app {
            items.push(ContextMenuItem::Separator);
            let label = format!("New {} Window", app.name);
            items.push(ContextMenuItem::Action {
                icon: "window-new-symbolic",
                label,
                shortcut: None,
                destructive: false,
                on_click: Box::new(move || {
                    app.launch();
                }),
            });
        }

        items.push(ContextMenuItem::Separator);
        {
            let ipc = self.ipc.clone();
            items.push(ContextMenuItem::Action {
                icon: "window-close-symbolic",
                label: "Close Window".into(),
                shortcut: Some("Super+Q"),
                destructive: true,
                on_click: Box::new(move || {
                    ipc.send(IpcRequest::CloseWindow { window_id });
                }),
            });
        }

        self.show_at(x, y, items);
    }

    /// Right-click menu for a pinned favorite app icon on the Dock.
    pub fn open_pinned_app_menu(&self, x: i32, y: i32, app: DesktopApp) {
        let open_label = format!("Open {}", app.name);
        let app_clone = app.clone();
        let items = vec![
            ContextMenuItem::Action {
                icon: "media-playback-start-symbolic",
                label: open_label,
                shortcut: None,
                destructive: false,
                on_click: Box::new(move || app_clone.launch()),
            },
            ContextMenuItem::Separator,
            ContextMenuItem::Action {
                icon: "view-grid-symbolic",
                label: "Dock & Panel Settings…".into(),
                shortcut: None,
                destructive: false,
                on_click: Box::new(|| launch_settings_page(Some("dock"))),
            },
            ContextMenuItem::Action {
                icon: "preferences-system-symbolic",
                label: "System Settings…".into(),
                shortcut: None,
                destructive: false,
                on_click: Box::new(|| launch_settings_page(None)),
            },
        ];
        self.show_at(x, y, items);
    }

    /// Right-click menu for the Dock container background.
    pub fn open_dock_menu(&self, x: i32, y: i32) {
        let ipc_ov = self.ipc.clone();
        let items = vec![
            ContextMenuItem::Action {
                icon: "view-grid-symbolic",
                label: "Dock & Panel Settings…".into(),
                shortcut: None,
                destructive: false,
                on_click: Box::new(|| launch_settings_page(Some("dock"))),
            },
            ContextMenuItem::Action {
                icon: "preferences-desktop-appearance-symbolic",
                label: "Appearance & Blur…".into(),
                shortcut: None,
                destructive: false,
                on_click: Box::new(|| launch_settings_page(Some("appearance"))),
            },
            ContextMenuItem::Action {
                icon: "preferences-system-symbolic",
                label: "System Settings…".into(),
                shortcut: None,
                destructive: false,
                on_click: Box::new(|| launch_settings_page(None)),
            },
            ContextMenuItem::Separator,
            ContextMenuItem::Action {
                icon: "view-app-grid-symbolic",
                label: "Applications & Mission Control".into(),
                shortcut: Some("Super+Space"),
                destructive: false,
                on_click: Box::new(move || {
                    ipc_ov.send(IpcRequest::ToggleOverview);
                }),
            },
        ];
        self.show_at(x, y, items);
    }

    /// Right-click menu for the Top Menu Bar.
    pub fn open_panel_menu(&self, x: i32, y: i32) {
        let focused = self
            .last_snapshot
            .borrow()
            .as_ref()
            .and_then(|s| s.windows.iter().find(|w| w.focused).cloned());

        let mut items: Vec<ContextMenuItem> = Vec::new();
        if let Some(win) = focused {
            let wid = win.id;
            let ipc_min = self.ipc.clone();
            let ipc_max = self.ipc.clone();
            let ipc_cls = self.ipc.clone();
            let title = win
                .title
                .as_deref()
                .or(win.app_id.as_deref())
                .unwrap_or("Active Window");
            let short_title: String = title.chars().take(18).collect();
            items.push(ContextMenuItem::Action {
                icon: "window-minimize-symbolic",
                label: format!("Minimize {short_title}"),
                shortcut: Some("Super+H"),
                destructive: false,
                on_click: Box::new(move || {
                    ipc_min.send(IpcRequest::MinimizeWindow { window_id: wid });
                }),
            });
            items.push(ContextMenuItem::Action {
                icon: "window-maximize-symbolic",
                label: if win.maximized {
                    "Unmaximize Window".into()
                } else {
                    "Maximize Window".into()
                },
                shortcut: Some("Super+↑"),
                destructive: false,
                on_click: Box::new(move || {
                    ipc_max.send(IpcRequest::ToggleMaximizeWindow { window_id: wid });
                }),
            });
            items.push(ContextMenuItem::Action {
                icon: "window-close-symbolic",
                label: format!("Close {short_title}"),
                shortcut: Some("Super+Q"),
                destructive: true,
                on_click: Box::new(move || {
                    ipc_cls.send(IpcRequest::CloseWindow { window_id: wid });
                }),
            });
            items.push(ContextMenuItem::Separator);
        }

        let ipc_shot = self.ipc.clone();
        let ipc_ov = self.ipc.clone();
        let ipc_lock = self.ipc.clone();

        items.extend([
            ContextMenuItem::Action {
                icon: "utilities-terminal-symbolic",
                label: "Open Terminal".into(),
                shortcut: Some("Ctrl+Alt+T"),
                destructive: false,
                on_click: Box::new(launch_default_terminal),
            },
            ContextMenuItem::Action {
                icon: "camera-photo-symbolic",
                label: "Take Screenshot…".into(),
                shortcut: Some("Super+Shift+4"),
                destructive: false,
                on_click: Box::new(move || {
                    let ipc_c = ipc_shot.clone();
                    glib::timeout_add_local_once(Duration::from_millis(80), move || {
                        ipc_c.send(IpcRequest::OpenScreenshotUi);
                    });
                }),
            },
            ContextMenuItem::Action {
                icon: "view-app-grid-symbolic",
                label: "Applications & Mission Control".into(),
                shortcut: Some("Super+Space"),
                destructive: false,
                on_click: Box::new(move || {
                    ipc_ov.send(IpcRequest::ToggleOverview);
                }),
            },
            ContextMenuItem::Separator,
            ContextMenuItem::Action {
                icon: "view-grid-symbolic",
                label: "Dock & Panel Settings…".into(),
                shortcut: None,
                destructive: false,
                on_click: Box::new(|| launch_settings_page(Some("dock"))),
            },
            ContextMenuItem::Action {
                icon: "preferences-system-symbolic",
                label: "System Settings…".into(),
                shortcut: None,
                destructive: false,
                on_click: Box::new(|| launch_settings_page(None)),
            },
            ContextMenuItem::Separator,
            ContextMenuItem::Action {
                icon: "system-lock-screen-symbolic",
                label: "Lock Screen".into(),
                shortcut: Some("Super+L"),
                destructive: false,
                on_click: Box::new(move || {
                    ipc_lock.send(IpcRequest::LockSession);
                }),
            },
        ]);

        self.show_at(x, y, items);
    }
}

pub fn launch_settings_page(page: Option<&str>) {
    let mut cmd = if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
        && dir.join("neon-settings").exists()
    {
        Command::new(dir.join("neon-settings"))
    } else {
        Command::new("neon-settings")
    };
    if let Some(p) = page {
        cmd.arg("--page").arg(p);
    }
    let _ = cmd.spawn();
}

fn launch_default_terminal() {
    for bin in ["kitty", "alacritty", "weston-terminal", "foot", "gnome-terminal", "xterm"] {
        if Command::new("which")
            .arg(bin)
            .output()
            .is_ok_and(|o| o.status.success())
        {
            let _ = Command::new(bin).spawn();
            return;
        }
    }
}

fn launch_default_files() {
    for bin in ["nautilus", "thunar", "dolphin", "nemo", "pcmanfm"] {
        if Command::new("which")
            .arg(bin)
            .output()
            .is_ok_and(|o| o.status.success())
        {
            let _ = Command::new(bin).spawn();
            return;
        }
    }
}

fn create_desktop_folder_and_open() {
    let desktop = std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join("Desktop"))
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    let _ = fs::create_dir_all(&desktop);
    let mut target = desktop.join("New Folder");
    let mut idx = 2u32;
    while target.exists() {
        target = desktop.join(format!("New Folder {idx}"));
        idx += 1;
    }
    let _ = fs::create_dir_all(&target);
    let _ = Command::new("xdg-open").arg(&target).spawn();
}

fn find_matching_desktop_app(win: &WindowInfo, apps: &[DesktopApp]) -> Option<DesktopApp> {
    let id_l = win.app_id.as_deref().unwrap_or("").to_lowercase();
    let title_l = win.title.as_deref().unwrap_or("").to_lowercase();
    apps.iter()
        .find(|app| {
            let name_l = app.name.to_lowercase();
            let exec_l = app.exec_program.to_lowercase();
            (!id_l.is_empty() && (app.id.to_lowercase() == id_l || exec_l.contains(&id_l) || name_l.contains(&id_l)))
                || (!title_l.is_empty() && title_l.contains(&name_l))
        })
        .cloned()
}
