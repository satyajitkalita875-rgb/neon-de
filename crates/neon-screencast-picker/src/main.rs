//! Native GNOME/KDE-style GTK4 Screen, Window, and Workspace selector for NEON DE.
//!
//! Invoked by `xdg-desktop-portal-wlr` (`chooser_type=simple`, `chooser_cmd=neon-screencast-picker`)
//! whenever OBS Studio, Chrome, Firefox, or Discord opens the screen sharing selector.
//!
//! Communicates with `neon-compositor` over `neon-ipc` to:
//! 1. Discover all connected Displays, open Windows (across all workspaces), and Workspaces.
//! 2. Stream fast live GPU preview thumbnails (`320×200`) into every card.
//! 3. Set the active zero-copy DMA-BUF `ScreencastTarget` (`Output`, `Window`, or `Workspace`)
//!    and output `Monitor: <output_name>` to stdout for `xdg-desktop-portal-wlr`.

mod style;

use std::{
    cell::RefCell,
    collections::HashMap,
    fs,
    io::{self, Write},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::ExitCode,
    rc::Rc,
    time::{Duration, Instant},
};

use clap::Parser;
use gtk4::{
    Align, Application, ApplicationWindow, Box as GtkBox, Button, ContentFit, Entry,
    EventControllerKey, FlowBox, GestureClick, IconTheme, Image, Label, Orientation, Overlay,
    Picture, ScrolledWindow, SelectionMode, Stack, StackTransitionType, gdk, gio, glib, prelude::*,
};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use neon_ipc::{
    DesktopSnapshot, IpcClient, IpcPayload, IpcRequest, IpcResult, OutputInfo, ScreencastTarget,
    WindowInfo, WorkspaceInfo, default_socket_path,
};

#[derive(Parser, Debug, Clone)]
#[command(
    name = "neon-screencast-picker",
    about = "NEON DE native Display, Window, and Workspace selector for OBS & PipeWire screen capture"
)]
struct Cli {
    /// Explicit NEON DE IPC socket path (auto-discovered from WAYLAND_DISPLAY if omitted).
    #[arg(long)]
    socket: Option<PathBuf>,

    /// Output format expected by xdg-desktop-portal-wlr (`simple` prints `Monitor: <name>`,
    /// `dmenu` prints `<name>`).
    #[arg(long, default_value = "simple")]
    format: String,

    /// Optional initial tab (`displays`, `windows`, or `workspaces`).
    #[arg(long)]
    tab: Option<String>,

    /// Preview mode: shows the UI without mutating the compositor's active screencast target.
    #[arg(long)]
    preview_only: bool,
}

#[derive(Debug, Clone)]
struct SelectionChoice {
    target: ScreencastTarget,
    output_name: String,
    summary: String,
}

struct PreviewSlot {
    path: PathBuf,
    frame: Overlay,
    placeholder: Image,
    cover: bool,
    loaded: bool,
}

struct TempDirGuard(PathBuf);

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn resolve_ipc_socket(explicit: Option<&Path>) -> PathBuf {
    if let Some(p) = explicit {
        return p.to_path_buf();
    }
    if let Some(env_p) = std::env::var_os("NEON_IPC_SOCKET")
        && !env_p.is_empty()
    {
        return PathBuf::from(env_p);
    }
    let wl = std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "wayland-0".to_string());
    default_socket_path(&wl)
}

fn fetch_desktop_state(
    socket: &Path,
) -> (Option<DesktopSnapshot>, ScreencastTarget) {
    let Ok(mut client) = IpcClient::connect(socket) else {
        return (None, ScreencastTarget::default());
    };
    let snapshot = match client.request(IpcRequest::GetState) {
        Ok(IpcResult::Ok {
            payload: IpcPayload::State { snapshot },
        }) => Some(snapshot),
        _ => None,
    };
    let active_target = match client.request(IpcRequest::GetScreencastTarget) {
        Ok(IpcResult::Ok {
            payload: IpcPayload::ScreencastTarget { target },
        }) => target,
        _ => ScreencastTarget::default(),
    };
    (snapshot, active_target)
}

fn format_portal_output(format: &str, output_name: &str) -> String {
    if format.eq_ignore_ascii_case("dmenu") {
        output_name.to_string()
    } else {
        format!("Monitor: {output_name}")
    }
}

fn primary_output_name(outputs: &[OutputInfo]) -> String {
    outputs
        .iter()
        .find(|o| o.primary)
        .or_else(|| outputs.first())
        .map(|o| o.name.clone())
        .unwrap_or_else(|| "eDP-1".to_string())
}

fn is_self_window(win: &WindowInfo) -> bool {
    let app_is_self = win
        .app_id
        .as_deref()
        .map(|a| a.to_lowercase().contains("screencast-picker"))
        .unwrap_or(false);
    let title_is_self = win
        .title
        .as_deref()
        .map(|t| t == "Share Screen" || t == "NEON Share Screen")
        .unwrap_or(false);
    app_is_self || title_is_self
}

fn friendly_app_name(app_id: Option<&str>) -> String {
    let raw = app_id.unwrap_or("Application").trim();
    if raw.is_empty() {
        return "Application".to_string();
    }
    let low = raw.to_lowercase();
    if low.contains("obsproject") || low == "obs" {
        return "OBS Studio".to_string();
    }
    if low.contains("resolve") {
        return "DaVinci Resolve".to_string();
    }
    if low.contains("google-chrome") || low == "chrome" {
        return "Google Chrome".to_string();
    }
    if low.contains("firefox") {
        return "Firefox".to_string();
    }
    if low.contains("neon-settings") || low.contains("neonde.settings") {
        return "Settings".to_string();
    }
    let tail = raw.rsplit('.').next().unwrap_or(raw);
    tail.split(['-', '_'])
        .filter(|s| !s.is_empty())
        .map(|word| {
            let mut ch = word.chars();
            match ch.next() {
                Some(first) => format!("{}{}", first.to_uppercase(), ch.as_str()),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn resolve_window_icon(app_id: Option<&str>, pixel_size: i32) -> Image {
    let theme = gdk::Display::default().map(|d| IconTheme::for_display(&d));
    if let (Some(theme), Some(raw)) = (theme.as_ref(), app_id.map(str::trim).filter(|s| !s.is_empty())) {
        let low = raw.to_lowercase();
        let mut candidates = vec![raw.to_string(), low.clone()];
        if let Some((_, tail)) = raw.rsplit_once('.') {
            candidates.push(tail.to_string());
            candidates.push(tail.to_lowercase());
        }
        if low.contains("obs") {
            candidates.push("com.obsproject.Studio".into());
            candidates.push("obs".into());
        } else if low.contains("resolve") {
            candidates.push("davinci-resolve".into());
            candidates.push("applications-multimedia".into());
        } else if low.contains("chrome") {
            candidates.push("google-chrome".into());
        } else if low.contains("kitty") || low.contains("alacritty") || low.contains("terminal") {
            candidates.push("utilities-terminal".into());
        } else if low.contains("settings") {
            candidates.push("preferences-system".into());
        }

        for cand in candidates {
            if !cand.is_empty() && theme.has_icon(&cand) {
                let img = Image::from_icon_name(&cand);
                img.set_pixel_size(pixel_size);
                return img;
            }
        }
    }
    let img = Image::from_icon_name("application-x-executable");
    img.set_pixel_size(pixel_size);
    img
}

fn create_preview_dir() -> Option<TempDirGuard> {
    let dir = std::env::temp_dir().join(format!("neon-sc-preview-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).ok()?;
    let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o700));
    Some(TempDirGuard(dir))
}

fn main() -> ExitCode {
    // Prevent GTK4 from calling back into xdg-desktop-portal while xdg-desktop-portal-wlr is
    // synchronously waiting on this picker process.
    unsafe {
        std::env::set_var("GTK_USE_PORTAL", "0");
    }

    tracing_subscriber::fmt()
        .with_writer(io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();

    let cli = Cli::parse();
    let ipc_socket = resolve_ipc_socket(cli.socket.as_deref());
    let (snapshot_opt, current_target) = fetch_desktop_state(&ipc_socket);
    let preview_guard = create_preview_dir();
    let preview_dir = preview_guard.as_ref().map(|g| g.0.clone());

    let confirmed: Rc<RefCell<Option<SelectionChoice>>> = Rc::new(RefCell::new(None));
    let confirmed_for_app = Rc::clone(&confirmed);
    let cli_for_app = cli.clone();
    let ipc_for_app = ipc_socket.clone();
    let current_target_for_app = current_target.clone();

    let app = Application::builder()
        .application_id("org.neonde.screencast-picker")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();

    app.connect_activate(move |app| {
        build_ui(
            app,
            snapshot_opt.clone(),
            current_target_for_app.clone(),
            cli_for_app.clone(),
            ipc_for_app.clone(),
            preview_dir.clone(),
            Rc::clone(&confirmed_for_app),
        );
    });

    // Pass empty argv so GTK4 does not choke on our custom CLI flags (--format, --tab, etc.).
    let empty_args: [&str; 0] = [];
    let _ = app.run_with_args(&empty_args);

    let choice = confirmed.borrow().clone();
    drop(preview_guard);

    if let Some(sel) = choice {
        if !cli.preview_only {
            if let Ok(mut client) = IpcClient::connect(&ipc_socket) {
                let _ = client.request(IpcRequest::SetScreencastTarget {
                    target: sel.target.clone(),
                });
            }
        }
        let line = format_portal_output(&cli.format, &sel.output_name);
        let mut stdout = io::stdout().lock();
        let _ = writeln!(stdout, "{line}");
        let _ = stdout.flush();
        ExitCode::SUCCESS
    } else {
        // Empty stdout + non-zero exit signals user cancellation to xdg-desktop-portal-wlr.
        ExitCode::from(1)
    }
}

fn build_ui(
    app: &Application,
    snapshot: Option<DesktopSnapshot>,
    current_target: ScreencastTarget,
    cli: Cli,
    ipc_socket: PathBuf,
    preview_dir: Option<PathBuf>,
    confirmed_out: Rc<RefCell<Option<SelectionChoice>>>,
) {
    let (light_theme, accent_hex, blur_active, high_contrast) = snapshot
        .as_ref()
        .map(|s| {
            let light = s.appearance.theme == "neon_light" || s.appearance.theme == "light";
            let blur_on = s.effects.blur_enabled
                && !s.appearance.reduce_blur
                && s.effects.blur_strength > 0.01
                && !s.appearance.high_contrast;
            (
                light,
                s.appearance.accent_color.clone(),
                blur_on,
                s.appearance.high_contrast,
            )
        })
        .unwrap_or((false, "#0A84FF".to_string(), true, false));

    if let Some(settings) = gtk4::Settings::default() {
        settings.set_gtk_application_prefer_dark_theme(!light_theme);
        settings.set_gtk_icon_theme_name(Some(if light_theme {
            "WhiteSur-light"
        } else {
            "WhiteSur-dark"
        }));
    }
    if let Some(display) = gdk::Display::default() {
        let icon_theme = IconTheme::for_display(&display);
        for extra in [
            "/var/lib/flatpak/exports/share/icons",
            "/usr/share/pixmaps",
        ] {
            if Path::new(extra).is_dir() {
                icon_theme.add_search_path(extra);
            }
        }
        if let Some(home) = std::env::var_os("HOME") {
            let home_p = PathBuf::from(home);
            for rel in [".local/share/icons", ".local/share/flatpak/exports/share/icons"] {
                let p = home_p.join(rel);
                if p.is_dir() {
                    icon_theme.add_search_path(p);
                }
            }
        }
    }

    style::apply_picker_css(light_theme, &accent_hex, blur_active, high_contrast);

    let outputs: Vec<OutputInfo> = snapshot
        .as_ref()
        .map(|s| s.outputs.clone())
        .filter(|o| !o.is_empty())
        .unwrap_or_else(|| {
            vec![OutputInfo {
                name: "eDP-1".into(),
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
                scale: 1.0,
                refresh_mhz: 60_000,
                primary: true,
                transform: "normal".into(),
            }]
        });
    let primary_out = primary_output_name(&outputs);

    let workspaces: Vec<WorkspaceInfo> = snapshot
        .as_ref()
        .map(|s| s.workspaces.clone())
        .unwrap_or_else(|| {
            vec![WorkspaceInfo {
                id: 1,
                index: 0,
                active: true,
                pinned: false,
                window_ids: Vec::new(),
            }]
        });

    let ws_index_by_id: HashMap<u64, usize> =
        workspaces.iter().map(|w| (w.id, w.index)).collect();

    let mut windows: Vec<WindowInfo> = snapshot
        .as_ref()
        .map(|s| {
            s.windows
                .iter()
                .filter(|w| !is_self_window(w))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    // Sort focused windows first, then active workspace, then by title.
    let active_ws_id = workspaces.iter().find(|w| w.active).map(|w| w.id);
    windows.sort_by(|a, b| {
        b.focused
            .cmp(&a.focused)
            .then_with(|| {
                (Some(b.workspace_id) == active_ws_id).cmp(&(Some(a.workspace_id) == active_ws_id))
            })
            .then_with(|| a.minimized.cmp(&b.minimized))
            .then_with(|| {
                a.title
                    .as_deref()
                    .unwrap_or("")
                    .to_lowercase()
                    .cmp(&b.title.as_deref().unwrap_or("").to_lowercase())
            })
    });

    // 1. Fullscreen dimmed click-catcher backdrop (`Layer::Overlay` below the frosted picker).
    let backdrop_window = ApplicationWindow::builder()
        .application(app)
        .title("Share Screen Backdrop")
        .decorated(false)
        .build();
    backdrop_window.add_css_class("picker-backdrop-window");
    if gtk4_layer_shell::is_supported() {
        backdrop_window.init_layer_shell();
        backdrop_window.set_namespace(Some("neon-screencast-picker-backdrop"));
        backdrop_window.set_layer(Layer::Overlay);
        backdrop_window.set_anchor(Edge::Top, true);
        backdrop_window.set_anchor(Edge::Bottom, true);
        backdrop_window.set_anchor(Edge::Left, true);
        backdrop_window.set_anchor(Edge::Right, true);
        backdrop_window.set_exclusive_zone(-1);
        backdrop_window.set_keyboard_mode(KeyboardMode::None);
    }
    let backdrop_box = GtkBox::new(Orientation::Vertical, 0);
    backdrop_box.add_css_class("picker-backdrop-scrim");
    backdrop_box.set_hexpand(true);
    backdrop_box.set_vexpand(true);
    backdrop_window.set_child(Some(&backdrop_box));

    // 2. Centered frosted-glass picker overlay (`neon-renderer` blurs its exact 22px rounded
    //    frame using the unified System Blur slider and floats cleanly above fullscreen OBS).
    let window = ApplicationWindow::builder()
        .application(app)
        .title("Share Screen")
        .default_width(820)
        .default_height(590)
        .resizable(false)
        .decorated(false)
        .build();
    window.add_css_class("picker-window");
    if gtk4_layer_shell::is_supported() {
        window.init_layer_shell();
        window.set_namespace(Some("neon-screencast-picker"));
        window.set_layer(Layer::Overlay);
        // Zero anchors -> compositor centers the modal dead-center on the primary output.
        window.set_exclusive_zone(-1);
        window.set_keyboard_mode(KeyboardMode::Exclusive);
    }

    let app_for_close = app.clone();
    let dismiss_picker: Rc<dyn Fn()> = Rc::new(move || {
        app_for_close.quit();
    });

    {
        let click = GestureClick::new();
        click.set_button(0);
        click.set_propagation_phase(gtk4::PropagationPhase::Capture);
        let dismiss = Rc::clone(&dismiss_picker);
        click.connect_pressed(move |_, _, _, _| dismiss());
        backdrop_window.add_controller(click);
    }

    let root = GtkBox::new(Orientation::Vertical, 0);
    root.add_css_class("picker-shell");
    root.set_size_request(820, 590);

    // ── Header ───────────────────────────────────────────────────────────────
    let header = GtkBox::new(Orientation::Vertical, 12);
    header.add_css_class("picker-header");

    let top_bar = GtkBox::new(Orientation::Horizontal, 12);
    let title_col = GtkBox::new(Orientation::Vertical, 2);
    title_col.set_hexpand(true);
    let title_lbl = Label::new(Some("Share Screen"));
    title_lbl.set_halign(Align::Start);
    title_lbl.add_css_class("picker-title");
    let sub_lbl = Label::new(Some("Choose a display, window, or workspace to stream"));
    sub_lbl.set_halign(Align::Start);
    sub_lbl.add_css_class("picker-subtitle");
    title_col.append(&title_lbl);
    title_col.append(&sub_lbl);

    let close_btn = Button::from_icon_name("window-close-symbolic");
    close_btn.add_css_class("picker-close-btn");
    close_btn.set_valign(Align::Start);
    close_btn.set_tooltip_text(Some("Cancel (Esc)"));
    {
        let dismiss = Rc::clone(&dismiss_picker);
        close_btn.connect_clicked(move |_| dismiss());
    }

    top_bar.append(&title_col);
    top_bar.append(&close_btn);
    header.append(&top_bar);

    // ── Segmented Tab Switcher (Displays | Windows | Workspaces) ─────────────
    let tabs_bar = GtkBox::new(Orientation::Horizontal, 4);
    tabs_bar.add_css_class("picker-tabs");
    tabs_bar.set_halign(Align::Center);

    let tab_displays = make_tab_button("video-display-symbolic", "Displays", outputs.len());
    let tab_windows = make_tab_button("window-new-symbolic", "Windows", windows.len());
    let tab_workspaces = make_tab_button("view-grid-symbolic", "Workspaces", workspaces.len());
    tabs_bar.append(&tab_displays);
    tabs_bar.append(&tab_windows);
    tabs_bar.append(&tab_workspaces);
    header.append(&tabs_bar);
    root.append(&header);

    // ── Shared selection & preview state ─────────────────────────────────────
    let selected: Rc<RefCell<Option<SelectionChoice>>> = Rc::new(RefCell::new(None));
    let all_cards: Rc<RefCell<Vec<(Button, ScreencastTarget)>>> = Rc::new(RefCell::new(Vec::new()));
    let preview_slots: Rc<RefCell<Vec<PreviewSlot>>> = Rc::new(RefCell::new(Vec::new()));
    let preview_requests: Rc<RefCell<Vec<(ScreencastTarget, PathBuf)>>> =
        Rc::new(RefCell::new(Vec::new()));

    // Footer controls created early so cards can update selection note + Share sensitivity.
    let selection_note = Label::new(Some("Select a source to share"));
    selection_note.set_halign(Align::Start);
    selection_note.set_hexpand(true);
    selection_note.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    selection_note.add_css_class("picker-selection-note");

    let share_btn = Button::with_label("Share");
    share_btn.add_css_class("picker-share-btn");
    share_btn.set_sensitive(false);

    let cancel_btn = Button::with_label("Cancel");
    cancel_btn.add_css_class("picker-cancel-btn");
    {
        let dismiss = Rc::clone(&dismiss_picker);
        cancel_btn.connect_clicked(move |_| dismiss());
    }

    let confirm_action: Rc<dyn Fn()> = {
        let sel_c = Rc::clone(&selected);
        let conf_c = Rc::clone(&confirmed_out);
        let dismiss = Rc::clone(&dismiss_picker);
        Rc::new(move || {
            if let Some(choice) = sel_c.borrow().clone() {
                *conf_c.borrow_mut() = Some(choice);
                dismiss();
            }
        })
    };

    {
        let act = Rc::clone(&confirm_action);
        share_btn.connect_clicked(move |_| act());
    }

    let select_choice: Rc<dyn Fn(SelectionChoice)> = {
        let sel_c = Rc::clone(&selected);
        let cards_c = Rc::clone(&all_cards);
        let note_c = selection_note.clone();
        let share_c = share_btn.clone();
        Rc::new(move |choice: SelectionChoice| {
            for (btn, tgt) in cards_c.borrow().iter() {
                if *tgt == choice.target {
                    btn.add_css_class("selected");
                } else {
                    btn.remove_css_class("selected");
                }
            }
            note_c.set_text(&format!("Ready to share: {}", choice.summary));
            share_c.set_sensitive(true);
            *sel_c.borrow_mut() = Some(choice);
        })
    };

    // ── Content Stack ────────────────────────────────────────────────────────
    let stack = Stack::new();
    stack.set_transition_type(StackTransitionType::Crossfade);
    stack.set_transition_duration(120);
    stack.set_vexpand(true);
    stack.set_hexpand(true);

    let mut default_display_choice: Option<SelectionChoice> = None;
    let mut default_workspace_choice: Option<SelectionChoice> = None;
    let mut initial_choice: Option<SelectionChoice> = None;

    // 1. Displays Page
    {
        let flow = make_card_flowbox();
        for (idx, out) in outputs.iter().enumerate() {
            let target = ScreencastTarget::Output {
                name: Some(out.name.clone()),
            };
            let hz = (out.refresh_mhz as f64 / 1000.0).round() as i32;
            let subtitle = format!("{} × {} · {} Hz", out.width, out.height, hz.max(60));
            let badge = if out.primary {
                Some(("Primary", true))
            } else {
                None
            };
            let thumb_path = preview_dir.as_ref().map(|d| d.join(format!("disp-{idx}.png")));
            let (card, slot) = build_preview_card(
                &out.name,
                &subtitle,
                Image::from_icon_name("video-display-symbolic"),
                "video-display-symbolic",
                badge,
                thumb_path.clone(),
                316,
                178,
            );
            if let (Some(mut slot), Some(path)) = (slot, thumb_path) {
                slot.cover = true;
                preview_slots.borrow_mut().push(slot);
                preview_requests.borrow_mut().push((target.clone(), path));
            }

            let choice = SelectionChoice {
                target: target.clone(),
                output_name: out.name.clone(),
                summary: format!("Display {}", out.name),
            };
            if default_display_choice.is_none() || out.primary {
                default_display_choice = Some(choice.clone());
            }
            if matches!(&current_target, ScreencastTarget::Output { name: Some(n) } if n == &out.name)
                || (matches!(&current_target, ScreencastTarget::Output { name: None }) && out.primary)
            {
                initial_choice = Some(choice.clone());
            }

            wire_card_interaction(
                &card,
                choice,
                Rc::clone(&select_choice),
                Rc::clone(&confirm_action),
            );
            all_cards.borrow_mut().push((card.clone(), target));
            flow.insert(&card, -1);
        }
        let scroller = make_scroller(&flow);
        stack.add_named(&scroller, Some("displays"));
    }

    // 2. Windows Page (with live search filter!)
    {
        let win_page = GtkBox::new(Orientation::Vertical, 10);
        win_page.set_margin_top(14);
        win_page.set_margin_start(18);
        win_page.set_margin_end(18);
        win_page.set_margin_bottom(8);

        let search_entry = Entry::builder()
            .placeholder_text("Search open windows…")
            .primary_icon_name("system-search-symbolic")
            .build();
        search_entry.add_css_class("picker-search");
        win_page.append(&search_entry);

        if windows.is_empty() {
            win_page.append(&make_empty_state(
                "window-new-symbolic",
                "No open windows",
                "Open an application window to share it individually.",
            ));
        } else {
            let flow = make_card_flowbox();
            flow.set_margin_top(4);
            flow.set_margin_start(0);
            flow.set_margin_end(0);

            let filterable: Rc<RefCell<Vec<(gtk4::FlowBoxChild, String)>>> =
                Rc::new(RefCell::new(Vec::new()));

            for win in &windows {
                let target = ScreencastTarget::Window { window_id: win.id };
                let app_name = friendly_app_name(win.app_id.as_deref());
                let title = win
                    .title
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .unwrap_or(&app_name);
                let ws_num = ws_index_by_id
                    .get(&win.workspace_id)
                    .copied()
                    .unwrap_or(0)
                    + 1;
                let mut sub = format!("{app_name} · Desktop {ws_num}");
                if win.minimized {
                    sub.push_str(" · Minimized");
                }
                let badge = if win.focused {
                    Some(("Focused", true))
                } else if win.fullscreen {
                    Some(("Fullscreen", false))
                } else {
                    None
                };

                let thumb_path = preview_dir
                    .as_ref()
                    .map(|d| d.join(format!("win-{}.png", win.id)));
                let icon = resolve_window_icon(win.app_id.as_deref(), 18);
                let (card, slot) = build_preview_card(
                    title,
                    &sub,
                    icon,
                    "window-new-symbolic",
                    badge,
                    thumb_path.clone(),
                    316,
                    178,
                );
                if let (Some(slot), Some(path)) = (slot, thumb_path) {
                    preview_slots.borrow_mut().push(slot);
                    preview_requests.borrow_mut().push((target.clone(), path));
                }

                let choice = SelectionChoice {
                    target: target.clone(),
                    output_name: primary_out.clone(),
                    summary: format!("Window — {title}"),
                };
                if current_target == target {
                    initial_choice = Some(choice.clone());
                }

                wire_card_interaction(
                    &card,
                    choice,
                    Rc::clone(&select_choice),
                    Rc::clone(&confirm_action),
                );
                all_cards.borrow_mut().push((card.clone(), target));
                flow.insert(&card, -1);
                if let Some(child) = card
                    .parent()
                    .and_then(|p| p.downcast::<gtk4::FlowBoxChild>().ok())
                {
                    let haystack = format!(
                        "{} {} {}",
                        title.to_lowercase(),
                        app_name.to_lowercase(),
                        win.app_id.as_deref().unwrap_or("").to_lowercase()
                    );
                    filterable.borrow_mut().push((child, haystack));
                }
            }

            {
                let items = Rc::clone(&filterable);
                search_entry.connect_changed(move |entry| {
                    let q = entry.text().trim().to_lowercase();
                    for (child, hay) in items.borrow().iter() {
                        child.set_visible(q.is_empty() || hay.contains(&q));
                    }
                });
            }

            let scroller = ScrolledWindow::builder()
                .hscrollbar_policy(gtk4::PolicyType::Never)
                .vscrollbar_policy(gtk4::PolicyType::Automatic)
                .vexpand(true)
                .child(&flow)
                .build();
            win_page.append(&scroller);
        }

        stack.add_named(&win_page, Some("windows"));
    }

    // 3. Workspaces Page
    {
        let flow = make_card_flowbox();
        let win_by_id: HashMap<u64, &WindowInfo> = windows.iter().map(|w| (w.id, w)).collect();

        for ws in &workspaces {
            let target = ScreencastTarget::Workspace { index: ws.index };
            let title = format!("Desktop {}", ws.index + 1);
            let apps_on_ws: Vec<String> = ws
                .window_ids
                .iter()
                .filter_map(|wid| win_by_id.get(wid))
                .map(|w| friendly_app_name(w.app_id.as_deref()))
                .collect();
            let subtitle = if apps_on_ws.is_empty() {
                "Empty desktop".to_string()
            } else {
                let preview_names = apps_on_ws.iter().take(3).cloned().collect::<Vec<_>>().join(", ");
                let count = apps_on_ws.len();
                let noun = if count == 1 { "window" } else { "windows" };
                format!("{count} {noun} · {preview_names}")
            };
            let badge = if ws.active {
                Some(("Active", true))
            } else {
                None
            };

            let thumb_path = preview_dir
                .as_ref()
                .map(|d| d.join(format!("ws-{}.png", ws.index)));
            let (card, slot) = build_preview_card(
                &title,
                &subtitle,
                Image::from_icon_name("view-grid-symbolic"),
                "view-grid-symbolic",
                badge,
                thumb_path.clone(),
                316,
                178,
            );
            if let (Some(mut slot), Some(path)) = (slot, thumb_path) {
                slot.cover = true;
                preview_slots.borrow_mut().push(slot);
                preview_requests.borrow_mut().push((target.clone(), path));
            }

            let choice = SelectionChoice {
                target: target.clone(),
                output_name: primary_out.clone(),
                summary: format!("Workspace — {title}"),
            };
            if ws.active || default_workspace_choice.is_none() {
                default_workspace_choice = Some(choice.clone());
            }
            if current_target == target {
                initial_choice = Some(choice.clone());
            }

            wire_card_interaction(
                &card,
                choice,
                Rc::clone(&select_choice),
                Rc::clone(&confirm_action),
            );
            all_cards.borrow_mut().push((card.clone(), target));
            flow.insert(&card, -1);
        }
        let scroller = make_scroller(&flow);
        stack.add_named(&scroller, Some("workspaces"));
    }

    root.append(&stack);

    // ── Footer ───────────────────────────────────────────────────────────────
    let footer = GtkBox::new(Orientation::Horizontal, 10);
    footer.add_css_class("picker-footer");
    footer.append(&selection_note);
    footer.append(&cancel_btn);
    footer.append(&share_btn);
    root.append(&footer);

    // ── Tab switching wiring ─────────────────────────────────────────────────
    let tab_buttons = vec![
        ("displays", tab_displays.clone()),
        ("windows", tab_windows.clone()),
        ("workspaces", tab_workspaces.clone()),
    ];
    let switch_tab: Rc<dyn Fn(&str)> = {
        let stack_c = stack.clone();
        let btns = tab_buttons.clone();
        Rc::new(move |name: &str| {
            stack_c.set_visible_child_name(name);
            for (k, b) in &btns {
                if *k == name {
                    b.add_css_class("active");
                } else {
                    b.remove_css_class("active");
                }
            }
        })
    };
    for (name, btn) in &tab_buttons {
        let sw = Rc::clone(&switch_tab);
        let key = *name;
        btn.connect_clicked(move |_| sw(key));
    }

    // Choose initial tab based on CLI flag or currently active screencast target.
    let initial_tab = cli
        .tab
        .as_deref()
        .and_then(|t| match t.to_lowercase().as_str() {
            "display" | "displays" | "monitor" => Some("displays"),
            "window" | "windows" => Some("windows"),
            "workspace" | "workspaces" | "desktop" => Some("workspaces"),
            _ => None,
        })
        .unwrap_or(match current_target {
            ScreencastTarget::Output { .. } => "displays",
            ScreencastTarget::Window { .. } => "windows",
            ScreencastTarget::Workspace { .. } => "workspaces",
        });
    switch_tab(initial_tab);

    // Pre-select the active target (or active workspace/display for the open tab) so Enter works immediately.
    let tab_fallback = if initial_tab == "workspaces" {
        default_workspace_choice.or(default_display_choice)
    } else {
        default_display_choice.or(default_workspace_choice)
    };
    let effective_initial = if cli.tab.is_some() && initial_tab == "workspaces" {
        tab_fallback
    } else {
        initial_choice.or(tab_fallback)
    };
    if let Some(choice) = effective_initial {
        select_choice(choice);
    }

    // ── Keyboard shortcuts (Esc = Cancel, Enter = Share) ─────────────────────
    for w in [&window, &backdrop_window] {
        let key_ctrl = EventControllerKey::new();
        let dismiss = Rc::clone(&dismiss_picker);
        let act = Rc::clone(&confirm_action);
        key_ctrl.connect_key_pressed(move |_, key, _, _| match key {
            gdk::Key::Escape => {
                dismiss();
                glib::Propagation::Stop
            }
            gdk::Key::Return | gdk::Key::KP_Enter => {
                act();
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        });
        w.add_controller(key_ctrl);
    }

    // Trigger compositor-driven 60 FPS entrance animation (promoted on first painted buffer).
    if let Ok(mut client) = IpcClient::connect(&ipc_socket) {
        let _ = client.request(IpcRequest::AnimateOverlay {
            namespace: "neon-screencast-picker".into(),
            opening: true,
        });
    }

    // ── Async Live GPU Preview Requests & Polling (slightly deferred for 60 FPS open) ──
    let queued = preview_requests.borrow().clone();
    if !queued.is_empty() {
        let sock_bg = ipc_socket.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(180));
            if let Ok(mut client) = IpcClient::connect(&sock_bg) {
                for (target, path) in queued {
                    let _ = client.request(IpcRequest::CaptureScreencastPreview {
                        target,
                        path: path.to_string_lossy().into_owned(),
                        max_width: Some(320),
                        max_height: Some(200),
                    });
                }
            }
        });

        let slots_c = Rc::clone(&preview_slots);
        let started = Instant::now();
        glib::timeout_add_local(Duration::from_millis(45), move || {
            let mut slots = slots_c.borrow_mut();
            let mut remaining = 0usize;
            for slot in slots.iter_mut() {
                if slot.loaded {
                    continue;
                }
                if slot.path.exists() {
                    if let Ok(tex) = gdk::Texture::from_filename(&slot.path) {
                        let pic = Picture::for_paintable(&tex);
                        pic.set_content_fit(if slot.cover {
                            ContentFit::Cover
                        } else {
                            ContentFit::Contain
                        });
                        pic.set_hexpand(true);
                        pic.set_vexpand(true);
                        pic.add_css_class("picker-thumb-pic");
                        slot.frame.set_child(Some(&pic));
                        slot.placeholder.set_visible(false);
                        slot.loaded = true;
                        continue;
                    }
                }
                remaining += 1;
            }
            if remaining == 0 || started.elapsed() > Duration::from_secs(4) {
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
    }

    window.set_child(Some(&root));
    if gtk4_layer_shell::is_supported() {
        backdrop_window.present();
        backdrop_window.queue_draw();
    }
    window.present();
}

fn make_tab_button(icon_name: &str, label: &str, count: usize) -> Button {
    let btn = Button::new();
    btn.add_css_class("picker-tab-btn");
    let row = GtkBox::new(Orientation::Horizontal, 7);
    let icon = Image::from_icon_name(icon_name);
    icon.set_pixel_size(15);
    let text = Label::new(Some(label));
    let pill = Label::new(Some(&count.to_string()));
    pill.add_css_class("picker-tab-count");
    row.append(&icon);
    row.append(&text);
    row.append(&pill);
    btn.set_child(Some(&row));
    btn
}

fn make_card_flowbox() -> FlowBox {
    let flow = FlowBox::new();
    flow.set_selection_mode(SelectionMode::None);
    flow.set_homogeneous(true);
    flow.set_min_children_per_line(2);
    flow.set_max_children_per_line(2);
    flow.set_row_spacing(12);
    flow.set_column_spacing(12);
    flow.set_margin_top(16);
    flow.set_margin_bottom(16);
    flow.set_margin_start(18);
    flow.set_margin_end(18);
    flow.set_valign(Align::Start);
    flow
}

fn make_scroller(flow: &FlowBox) -> ScrolledWindow {
    ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vscrollbar_policy(gtk4::PolicyType::Automatic)
        .vexpand(true)
        .child(flow)
        .build()
}

fn make_empty_state(icon_name: &str, title: &str, subtitle: &str) -> GtkBox {
    let bx = GtkBox::new(Orientation::Vertical, 8);
    bx.set_halign(Align::Center);
    bx.set_valign(Align::Center);
    bx.set_vexpand(true);
    let img = Image::from_icon_name(icon_name);
    img.set_pixel_size(40);
    img.add_css_class("picker-thumb-placeholder");
    let t = Label::new(Some(title));
    t.add_css_class("picker-empty-title");
    let s = Label::new(Some(subtitle));
    s.add_css_class("picker-empty-sub");
    bx.append(&img);
    bx.append(&t);
    bx.append(&s);
    bx
}

#[allow(clippy::too_many_arguments)]
fn build_preview_card(
    title: &str,
    subtitle: &str,
    small_icon: Image,
    placeholder_icon_name: &str,
    badge: Option<(&str, bool)>,
    thumb_path: Option<PathBuf>,
    thumb_w: i32,
    thumb_h: i32,
) -> (Button, Option<PreviewSlot>) {
    let btn = Button::new();
    btn.add_css_class("picker-card");

    let vbox = GtkBox::new(Orientation::Vertical, 8);

    // Thumbnail preview area (16:9 rounded surface + optional top-right status pill)
    let thumb_overlay = Overlay::new();
    thumb_overlay.add_css_class("picker-thumb-frame");
    thumb_overlay.set_size_request(thumb_w, thumb_h);
    thumb_overlay.set_overflow(gtk4::Overflow::Hidden);

    let placeholder = Image::from_icon_name(placeholder_icon_name);
    placeholder.set_pixel_size(42);
    placeholder.set_halign(Align::Center);
    placeholder.set_valign(Align::Center);
    placeholder.add_css_class("picker-thumb-placeholder");
    thumb_overlay.set_child(Some(&placeholder));

    if let Some((badge_text, accent)) = badge {
        let pill = Label::new(Some(badge_text));
        pill.add_css_class("picker-badge");
        if accent {
            pill.add_css_class("picker-badge-accent");
        }
        pill.set_halign(Align::End);
        pill.set_valign(Align::Start);
        thumb_overlay.add_overlay(&pill);
    }

    vbox.append(&thumb_overlay);

    // Metadata footer row: app/type icon + bold title + muted subtitle
    let meta = GtkBox::new(Orientation::Horizontal, 9);
    small_icon.set_pixel_size(18);
    small_icon.set_valign(Align::Center);
    meta.append(&small_icon);

    let text_col = GtkBox::new(Orientation::Vertical, 1);
    text_col.set_hexpand(true);

    let t_lbl = Label::new(Some(title));
    t_lbl.set_halign(Align::Start);
    t_lbl.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    t_lbl.set_max_width_chars(30);
    t_lbl.add_css_class("picker-card-title");

    let s_lbl = Label::new(Some(subtitle));
    s_lbl.set_halign(Align::Start);
    s_lbl.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    s_lbl.set_max_width_chars(34);
    s_lbl.add_css_class("picker-card-sub");

    text_col.append(&t_lbl);
    text_col.append(&s_lbl);
    meta.append(&text_col);
    vbox.append(&meta);

    btn.set_child(Some(&vbox));

    let slot = thumb_path.map(|path| PreviewSlot {
        path,
        frame: thumb_overlay,
        placeholder,
        cover: false,
        loaded: false,
    });

    (btn, slot)
}

fn wire_card_interaction(
    card: &Button,
    choice: SelectionChoice,
    on_select: Rc<dyn Fn(SelectionChoice)>,
    on_confirm: Rc<dyn Fn()>,
) {
    {
        let ch = choice.clone();
        let sel = Rc::clone(&on_select);
        card.connect_clicked(move |_| sel(ch.clone()));
    }
    let dbl = GestureClick::new();
    dbl.set_button(1);
    {
        let ch = choice;
        let sel = on_select;
        let conf = on_confirm;
        dbl.connect_pressed(move |_, n_press, _, _| {
            if n_press == 2 {
                sel(ch.clone());
                conf();
            }
        });
    }
    card.add_controller(dbl);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_portal_output_for_simple_and_dmenu_modes() {
        assert_eq!(format_portal_output("simple", "eDP-1"), "Monitor: eDP-1");
        assert_eq!(format_portal_output("dmenu", "HDMI-A-1"), "HDMI-A-1");
    }

    #[test]
    fn normalizes_friendly_app_names_and_filters_self() {
        assert_eq!(friendly_app_name(Some("com.obsproject.Studio")), "OBS Studio");
        assert_eq!(friendly_app_name(Some("google-chrome")), "Google Chrome");
        assert_eq!(friendly_app_name(Some("org.gnome.Nautilus")), "Nautilus");

        let picker_win = WindowInfo {
            id: 99,
            workspace_id: 1,
            app_id: Some("org.neonde.screencast-picker".into()),
            title: Some("Share Screen".into()),
            x: 0,
            y: 0,
            width: 780,
            height: 560,
            focused: true,
            minimized: false,
            maximized: false,
            fullscreen: false,
            always_on_top: false,
        };
        assert!(is_self_window(&picker_win));
    }
}
