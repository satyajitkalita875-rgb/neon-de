//! Shared Settings state controller bridging XDG `neon-config`, non-blocking `neon-ipc`,
//! GTK toolkit synchronization, and auto-hiding transient feedback toasts.

use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    process::Command,
    rc::Rc,
    sync::mpsc,
    time::Duration,
};

use gtk4::{ApplicationWindow, Box as GtkBox, Label, glib, prelude::*};
use neon_config::{FocusMode, NeonConfig, ThemeMode, WindowButtonLayout, save_atomic};
use neon_ipc::{DiagnosticsInfo, IpcClient, IpcPayload, IpcRequest, IpcResult, OutputInfo};
use tracing::warn;

use crate::ui::style::SettingsTheme;

type NavCallback = Rc<dyn Fn(&str, Option<&str>)>;
type RebuildCallback = Rc<dyn Fn(&str)>;
type ButtonLayoutCallback = Rc<dyn Fn(WindowButtonLayout)>;

enum WorkerMsg {
    PersistAndSync {
        config_path: PathBuf,
        ipc_socket: PathBuf,
        cfg: NeonConfig,
        req: Option<IpcRequest>,
    },
    PersistSilent {
        config_path: PathBuf,
        cfg: NeonConfig,
    },
    PushFull {
        ipc_socket: PathBuf,
        reqs: Vec<IpcRequest>,
    },
}

/// Central state & IPC controller shared across all Settings pages.
pub struct SettingsController {
    pub config_path: PathBuf,
    pub ipc_socket: PathBuf,
    pub config: RefCell<NeonConfig>,
    pub theme: SettingsTheme,
    pub status_label: Label,
    pub status_bar: GtkBox,
    pub window: ApplicationWindow,
    pub navigate_cb: RefCell<Option<NavCallback>>,
    pub rebuild_page_cb: RefCell<Option<RebuildCallback>>,
    pub button_layout_cb: RefCell<Option<ButtonLayoutCallback>>,
    toast_generation: Rc<Cell<u64>>,
    worker_tx: mpsc::Sender<WorkerMsg>,
}

impl SettingsController {
    pub fn new(
        config_path: PathBuf,
        ipc_socket: PathBuf,
        config: NeonConfig,
        theme: SettingsTheme,
        status_label: Label,
        status_bar: GtkBox,
        window: ApplicationWindow,
    ) -> Self {
        let (tx, rx) = mpsc::channel::<WorkerMsg>();
        // Single dedicated background thread preserves write & IPC ordering without blocking GTK.
        std::thread::spawn(move || {
            while let Ok(msg) = rx.recv() {
                match msg {
                    WorkerMsg::PersistAndSync {
                        config_path,
                        ipc_socket,
                        cfg,
                        req,
                    } => {
                        if let Err(err) = save_atomic(&config_path, &cfg) {
                            warn!(?err, "Failed to persist NEON DE config");
                        }
                        if let Some(request) = req {
                            if let Ok(mut client) = IpcClient::connect(&ipc_socket) {
                                let _ = client.request(request);
                            }
                        }
                    }
                    WorkerMsg::PersistSilent { config_path, cfg } => {
                        if let Err(err) = save_atomic(&config_path, &cfg) {
                            warn!(?err, "Silent config save failed");
                        }
                    }
                    WorkerMsg::PushFull { ipc_socket, reqs } => {
                        if let Ok(mut client) = IpcClient::connect(&ipc_socket) {
                            for req in reqs {
                                let _ = client.request(req);
                            }
                        }
                    }
                }
            }
        });

        Self {
            config_path,
            ipc_socket,
            config: RefCell::new(config),
            theme,
            status_label,
            status_bar,
            window,
            navigate_cb: RefCell::new(None),
            rebuild_page_cb: RefCell::new(None),
            button_layout_cb: RefCell::new(None),
            toast_generation: Rc::new(Cell::new(0)),
            worker_tx: tx,
        }
    }

    /// Saves the updated configuration atomically on the background worker, refreshes CSS only
    /// if theme/accent changed, and sends live IPC updates without stalling the UI thread.
    pub fn persist_and_sync(&self, req: Option<IpcRequest>, note: &str) {
        let cfg = self.config.borrow().clone();
        self.theme
            .update(cfg.appearance.theme, &cfg.appearance.accent_color);

        let _ = self.worker_tx.send(WorkerMsg::PersistAndSync {
            config_path: self.config_path.clone(),
            ipc_socket: self.ipc_socket.clone(),
            cfg,
            req,
        });
        self.set_status_ok(note);
    }

    /// Saves configuration quietly on the background worker (e.g. last opened page).
    pub fn persist_silent(&self) {
        let cfg = self.config.borrow().clone();
        let _ = self.worker_tx.send(WorkerMsg::PersistSilent {
            config_path: self.config_path.clone(),
            cfg,
        });
    }

    fn show_toast(&self, msg: &str, variant: Option<&str>, auto_hide_ms: u64) {
        self.status_bar.remove_css_class("status-ok");
        self.status_bar.remove_css_class("status-error");
        if let Some(cls) = variant {
            self.status_bar.add_css_class(cls);
        }
        self.status_label.set_text(msg);
        self.status_bar.set_visible(true);

        // Use a monotonic generation counter instead of SourceId::remove() so fired timers
        // can never panic on subsequent clicks (e.g. changing wallpapers multiple times).
        let my_gen = self.toast_generation.get().wrapping_add(1);
        self.toast_generation.set(my_gen);
        let gen_c = Rc::clone(&self.toast_generation);
        let bar = self.status_bar.clone();
        glib::timeout_add_local_once(Duration::from_millis(auto_hide_ms), move || {
            if gen_c.get() == my_gen {
                bar.set_visible(false);
            }
        });
    }

    pub fn set_status_ok(&self, msg: &str) {
        self.show_toast(msg, Some("status-ok"), 2200);
    }

    pub fn set_status_err(&self, msg: &str) {
        self.show_toast(msg, Some("status-error"), 4000);
    }

    pub fn set_status_info(&self, msg: &str) {
        self.show_toast(msg, None, 2200);
    }

    /// Sends a typed IPC request to `neon-compositor` synchronously (prefer calling inside `run_async`).
    pub fn send_ipc(&self, req: IpcRequest) -> Result<IpcResult, String> {
        send_ipc_socket(&self.ipc_socket, req)
    }

    /// Pushes effects, appearance, and window-behavior settings live to `neon-compositor`.
    pub fn push_full_config_live(&self) {
        let cfg = self.config.borrow().clone();
        self.theme
            .update(cfg.appearance.theme, &cfg.appearance.accent_color);
        let reqs = vec![
            build_effects_request(&cfg),
            build_appearance_request(&cfg),
            build_window_behavior_request(&cfg),
        ];
        let _ = self.worker_tx.send(WorkerMsg::PushFull {
            ipc_socket: self.ipc_socket.clone(),
            reqs,
        });
    }

    /// Resets a single settings category back to defaults, syncs live, and refreshes the page UI.
    pub fn reset_category(&self, category_id: &str) {
        let changed = match category_id {
            "dock" => {
                let mut cfg = self.config.borrow_mut();
                let a = cfg.reset_category("dock");
                let b = cfg.reset_category("effects");
                let c = cfg.reset_category("windows");
                a || b || c
            }
            other => self.config.borrow_mut().reset_category(other),
        };
        if !changed {
            self.set_status_info("Already at default settings");
            return;
        }
        self.persist_silent();
        self.push_full_config_live();
        let layout = self.config.borrow().windows.button_layout;
        apply_window_button_layout(layout);
        self.notify_button_layout_changed(layout);
        self.rebuild_page(category_id);
        self.set_status_ok("Restored default settings");
    }

    /// Updates `neon-settings`'s own header window buttons live when button side changes.
    pub fn notify_button_layout_changed(&self, layout: WindowButtonLayout) {
        let cb = self.button_layout_cb.borrow().clone();
        if let Some(cb) = cb {
            cb(layout);
        }
    }

    /// Navigates programmatically to any category (optionally focusing a setting id).
    #[allow(dead_code)]
    pub fn navigate_to(&self, category_id: &str, highlight_setting: Option<&str>) {
        let cb = self.navigate_cb.borrow().clone();
        if let Some(cb) = cb {
            cb(category_id, highlight_setting);
        }
    }

    /// Forces a lazy-cached page to rebuild with updated state.
    pub fn rebuild_page(&self, category_id: &str) {
        let cb = self.rebuild_page_cb.borrow().clone();
        if let Some(cb) = cb {
            cb(category_id);
        }
    }
}

pub fn send_ipc_socket(socket: &std::path::Path, req: IpcRequest) -> Result<IpcResult, String> {
    let mut client = IpcClient::connect(socket).map_err(|e| e.to_string())?;
    client.request(req).map_err(|e| e.to_string())
}

pub fn fetch_outputs_sync(socket: &std::path::Path) -> Vec<OutputInfo> {
    match send_ipc_socket(socket, IpcRequest::GetOutputs) {
        Ok(IpcResult::Ok {
            payload: IpcPayload::Outputs { outputs },
        }) => outputs,
        _ => Vec::new(),
    }
}

pub fn fetch_diagnostics_sync(socket: &std::path::Path) -> Option<DiagnosticsInfo> {
    match send_ipc_socket(socket, IpcRequest::GetDiagnostics) {
        Ok(IpcResult::Ok {
            payload: IpcPayload::Diagnostics { diagnostics },
        }) => Some(diagnostics),
        _ => None,
    }
}

pub fn build_effects_request(cfg: &NeonConfig) -> IpcRequest {
    IpcRequest::UpdateEffects {
        enabled: Some(cfg.effects.enabled),
        reduced_motion: Some(cfg.effects.reduced_motion),
        shadows_enabled: Some(cfg.effects.shadows_enabled),
        borders_enabled: Some(cfg.effects.borders_enabled),
        blur_enabled: Some(cfg.effects.blur_enabled),
        blur_strength: Some(cfg.effects.blur_strength),
        inactive_opacity: Some(cfg.effects.inactive_opacity),
        corner_radius: Some(cfg.effects.corner_radius),
        speed_scale: Some(cfg.effects.speed_scale),
    }
}

pub fn build_appearance_request(cfg: &NeonConfig) -> IpcRequest {
    let theme = match cfg.appearance.theme {
        ThemeMode::NeonDark => "neon_dark",
        ThemeMode::NeonLight => "neon_light",
        ThemeMode::Auto => "auto",
    };
    let dock_pos = match cfg.dock.position {
        neon_config::DockPosition::Bottom => "bottom",
        neon_config::DockPosition::Left => "left",
        neon_config::DockPosition::Right => "right",
    };
    IpcRequest::from(neon_ipc::AppearanceUpdate {
        theme: Some(theme.into()),
        accent_color: Some(cfg.appearance.accent_color.clone()),
        dock_transparency: Some(cfg.dock.transparency),
        dock_auto_hide: Some(cfg.dock.auto_hide),
        dock_smart_hide: Some(cfg.dock.intelligent_hide),
        dock_position: Some(dock_pos.into()),
        dock_icon_size: Some(cfg.dock.icon_size),
        dock_magnification: Some(cfg.dock.magnification),
        dock_pinned_apps: Some(cfg.dock.pinned_apps.clone()),
        panel_transparency: Some(cfg.panel.transparency),
        panel_visible: Some(cfg.panel.visible),
        panel_smart_hide: Some(cfg.panel.smart_hide),
        show_clock: Some(cfg.panel.show_clock),
        clock_24h: Some(cfg.panel.clock_24h),
        show_workspaces: Some(cfg.panel.show_workspaces),
        show_app_title: Some(cfg.panel.show_app_title),
        show_system_tray: Some(cfg.panel.show_system_tray),
        show_battery: Some(cfg.panel.show_battery),
        show_quick_settings: Some(cfg.panel.show_quick_settings),
        show_desktop_icons: Some(cfg.desktop.show_desktop_icons),
        desktop_sort_by: Some(cfg.desktop.desktop_sort_by.clone()),
        desktop_auto_arrange: Some(cfg.desktop.desktop_auto_arrange),
        high_contrast: Some(cfg.privacy_accessibility.high_contrast),
        wallpaper: Some(cfg.appearance.wallpaper_path.clone().unwrap_or_default()),
        reduce_blur: Some(cfg.appearance.reduce_blur),
        blur_strength: Some(cfg.effects.blur_strength),
        cursor_theme: Some(cfg.appearance.cursor_theme.clone()),
        cursor_size: Some(cfg.appearance.cursor_size),
    })
}

pub fn build_window_behavior_request(cfg: &NeonConfig) -> IpcRequest {
    let layout = match cfg.windows.button_layout {
        WindowButtonLayout::RightStandard => "right_standard",
        WindowButtonLayout::LeftTrafficLights => "left_traffic_lights",
    };
    IpcRequest::UpdateWindowBehavior {
        focus_follows_mouse: Some(cfg.windows.focus_mode == FocusMode::FocusFollowsMouse),
        snap_gap: Some(cfg.windows.snap_gap),
        button_layout: Some(layout.into()),
    }
}

/// Applies cursor theme and pixel size immediately to GTK4 and GNOME/GTK gsettings.
pub fn apply_system_cursor(theme: &str, size: u32) {
    if let Some(s) = gtk4::Settings::default() {
        s.set_gtk_cursor_theme_name(Some(theme));
        s.set_gtk_cursor_theme_size(size as i32);
    }
    let theme_owned = theme.to_string();
    std::thread::spawn(move || {
        let _ = Command::new("gsettings")
            .args([
                "set",
                "org.gnome.desktop.interface",
                "cursor-theme",
                &theme_owned,
            ])
            .status();
        let _ = Command::new("gsettings")
            .args([
                "set",
                "org.gnome.desktop.interface",
                "cursor-size",
                &size.to_string(),
            ])
            .status();
    });
}

/// Applies GTK/icon theme and font scaling via gsettings without blocking the UI thread.
pub fn apply_toolkit_appearance(
    gtk_theme: Option<&str>,
    icon_theme: Option<&str>,
    font_name: Option<&str>,
    text_scale: Option<f64>,
) {
    if let Some(s) = gtk4::Settings::default() {
        if let Some(icon) = icon_theme {
            s.set_gtk_icon_theme_name(Some(icon));
        }
        if let Some(font) = font_name {
            s.set_gtk_font_name(Some(font));
        }
    }
    let gtk_t = gtk_theme.map(str::to_string);
    let icon_t = icon_theme.map(str::to_string);
    let font_t = font_name.map(str::to_string);
    std::thread::spawn(move || {
        if let Some(v) = gtk_t {
            let _ = Command::new("gsettings")
                .args(["set", "org.gnome.desktop.interface", "gtk-theme", &v])
                .status();
        }
        if let Some(v) = icon_t {
            let _ = Command::new("gsettings")
                .args(["set", "org.gnome.desktop.interface", "icon-theme", &v])
                .status();
        }
        if let Some(v) = font_t {
            let _ = Command::new("gsettings")
                .args(["set", "org.gnome.desktop.interface", "font-name", &v])
                .status();
        }
        if let Some(scale) = text_scale {
            let _ = Command::new("gsettings")
                .args([
                    "set",
                    "org.gnome.desktop.interface",
                    "text-scaling-factor",
                    &format!("{scale:.2}"),
                ])
                .status();
        }
    });
}

/// Synchronizes window titlebar button layout (macOS left traffic lights vs right controls)
/// across GTK4, gsettings (`org.gnome.desktop.wm.preferences button-layout`), and GTK3/4 `settings.ini`.
pub fn apply_window_button_layout(layout: WindowButtonLayout) {
    let buttons_right = layout == WindowButtonLayout::RightStandard;
    let val = if buttons_right {
        ":minimize,maximize,close"
    } else {
        "close,minimize,maximize:"
    };
    if let Some(s) = gtk4::Settings::default() {
        s.set_gtk_decoration_layout(Some(val));
    }
    std::thread::spawn(move || {
        neon_system::portals::sync_window_button_layout(buttons_right);
    });
}
