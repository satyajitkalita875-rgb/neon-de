//! Native Linux System Tray (`org.kde.StatusNotifierWatcher` + `StatusNotifierItem` +
//! `com.canonical.dbusmenu`) for **NEON DE**.
//!
//! Powers Top Bar background-app tray icons (such as Antigravity, OBS Studio, Discord, Steam,
//! Slack, Telegram, etc.) with left-click activation and right-click native DBusMenu context
//! menus (Show/Hide, Start Recording, Quit, etc.).
//!
//! Host-safety invariant:
//! - On a standalone NEON DE session where `org.kde.StatusNotifierWatcher` is unowned, NEON DE
//!   claims `org.kde.StatusNotifierWatcher` (and `org.freedesktop.StatusNotifierWatcher` if
//!   unowned) at `/StatusNotifierWatcher` and serves as both Watcher and Host.
//! - Inside a nested session where a host desktop already owns `org.kde.StatusNotifierWatcher`,
//!   NEON DE NEVER steals or replaces the host watcher; instead it connects as a read-only
//!   `StatusNotifierHost` client to display registered tray items cleanly.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use tracing::{debug, info, warn};
use zbus::{
    blocking::{Connection, Proxy, connection::Builder},
    interface,
    message::Header,
    names::WellKnownName,
    object_server::SignalEmitter,
    zvariant::{OwnedObjectPath, OwnedValue, Structure, Value},
};

pub const KDE_WATCHER_NAME: &str = "org.kde.StatusNotifierWatcher";
pub const FDO_WATCHER_NAME: &str = "org.freedesktop.StatusNotifierWatcher";
pub const WATCHER_PATH: &str = "/StatusNotifierWatcher";
pub const KDE_ITEM_IFACE: &str = "org.kde.StatusNotifierItem";
pub const FDO_ITEM_IFACE: &str = "org.freedesktop.StatusNotifierItem";
pub const DBUSMENU_IFACE: &str = "com.canonical.dbusmenu";

/// Decoded RGBA8888 icon image converted from an SNI `IconPixmap` (big-endian ARGB32).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayPixmap {
    pub width: i32,
    pub height: i32,
    pub rgba: Vec<u8>,
}

/// Snapshot of a single active System Tray (`StatusNotifierItem`) entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayItem {
    /// Canonical SNI identifier (`"{bus_name}{object_path}"`).
    pub key: String,
    pub bus_name: String,
    pub object_path: String,
    pub id: String,
    pub title: String,
    pub tooltip: String,
    pub status: String,
    pub icon_name: Option<String>,
    pub icon_file: Option<String>,
    pub icon_rgba: Option<TrayPixmap>,
    pub menu_path: Option<String>,
    pub item_is_menu: bool,
}

impl TrayItem {
    /// Human-friendly display label for tooltips or fallback captions.
    pub fn display_title(&self) -> &str {
        if !self.tooltip.is_empty() {
            &self.tooltip
        } else if !self.title.is_empty() {
            &self.title
        } else if !self.id.is_empty() {
            &self.id
        } else {
            &self.bus_name
        }
    }
}

/// Single entry inside a `com.canonical.dbusmenu` context menu tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayMenuItem {
    pub id: i32,
    pub label: String,
    pub enabled: bool,
    pub visible: bool,
    pub is_separator: bool,
    pub toggle_type: Option<String>,
    pub toggle_state: i32,
    pub icon_name: Option<String>,
    pub children: Vec<TrayMenuItem>,
}

/// Asynchronous actions sent from the GTK4 UI thread to the background D-Bus tray worker.
#[derive(Debug, Clone)]
pub enum TrayAction {
    Refresh,
    Activate {
        bus_name: String,
        object_path: String,
        x: i32,
        y: i32,
    },
    SecondaryActivate {
        bus_name: String,
        object_path: String,
        x: i32,
        y: i32,
    },
    FetchMenu {
        key: String,
        bus_name: String,
        menu_path: String,
    },
    TriggerMenuItem {
        bus_name: String,
        menu_path: String,
        menu_item_id: i32,
    },
}

/// Events pushed from the background D-Bus tray worker to the GTK4 UI thread.
#[derive(Debug, Clone)]
pub enum TrayEvent {
    ItemsUpdated(Vec<TrayItem>),
    MenuLoaded {
        key: String,
        bus_name: String,
        menu_path: String,
        items: Vec<TrayMenuItem>,
    },
}

/// Cloneable sender handle for dispatching [`TrayAction`]s off the UI thread.
#[derive(Clone)]
pub struct TrayActionSender {
    tx: mpsc::Sender<TrayAction>,
}

impl TrayActionSender {
    pub fn send(&self, action: TrayAction) {
        let _ = self.tx.send(action);
    }
}

/// Shared registry state owned by NEON DE's `StatusNotifierWatcher` D-Bus object.
#[derive(Debug)]
struct WatcherRegistry {
    items: Mutex<Vec<String>>,
    hosts: Mutex<HashSet<String>>,
    notify_tx: mpsc::Sender<TrayAction>,
}

impl WatcherRegistry {
    fn new(notify_tx: mpsc::Sender<TrayAction>) -> Self {
        Self {
            items: Mutex::new(Vec::new()),
            hosts: Mutex::new(HashSet::new()),
            notify_tx,
        }
    }

    fn insert_item(&self, canonical: String) -> bool {
        let inserted = if let Ok(mut items) = self.items.lock() {
            if items.iter().any(|existing| existing == &canonical) {
                false
            } else {
                items.push(canonical);
                true
            }
        } else {
            false
        };
        let _ = self.notify_tx.send(TrayAction::Refresh);
        inserted
    }

    fn remove_items_matching<F>(&self, mut pred: F) -> Vec<String>
    where
        F: FnMut(&str) -> bool,
    {
        let mut removed = Vec::new();
        if let Ok(mut items) = self.items.lock() {
            items.retain(|entry| {
                if pred(entry) {
                    removed.push(entry.clone());
                    false
                } else {
                    true
                }
            });
        }
        removed
    }

    fn snapshot_items(&self) -> Vec<String> {
        self.items
            .lock()
            .map(|items| items.clone())
            .unwrap_or_default()
    }

    fn register_host(&self, host: String) {
        if let Ok(mut hosts) = self.hosts.lock() {
            hosts.insert(host);
        }
    }
}

/// D-Bus `org.kde.StatusNotifierWatcher` object served at `/StatusNotifierWatcher`.
struct StatusNotifierWatcherIface {
    registry: Arc<WatcherRegistry>,
}

#[interface(name = "org.kde.StatusNotifierWatcher")]
impl StatusNotifierWatcherIface {
    async fn register_status_notifier_item(
        &self,
        service: &str,
        #[zbus(header)] header: Header<'_>,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) {
        let sender = header.sender().map(|s| s.as_str());
        let Some((bus_name, object_path)) = parse_service_target(service, sender) else {
            warn!(service, ?sender, "Ignoring malformed StatusNotifierItem registration");
            return;
        };
        let canonical = format!("{bus_name}{object_path}");
        let inserted = self.registry.insert_item(canonical.clone());
        info!(item = %canonical, inserted, "Registered StatusNotifierItem");
        if inserted {
            let _ = Self::status_notifier_item_registered(&emitter, &canonical).await;
            let _ = self.registered_status_notifier_items_changed(&emitter).await;
        }
    }

    async fn register_status_notifier_host(
        &self,
        service: &str,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) {
        if !service.is_empty() {
            self.registry.register_host(service.to_string());
            let _ = Self::status_notifier_host_registered(&emitter).await;
            let _ = self.is_status_notifier_host_registered_changed(&emitter).await;
        }
    }

    #[zbus(property)]
    fn registered_status_notifier_items(&self) -> Vec<String> {
        self.registry.snapshot_items()
    }

    #[zbus(property)]
    fn is_status_notifier_host_registered(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn protocol_version(&self) -> i32 {
        0
    }

    #[zbus(signal)]
    async fn status_notifier_item_registered(
        emitter: &SignalEmitter<'_>,
        service: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn status_notifier_item_unregistered(
        emitter: &SignalEmitter<'_>,
        service: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn status_notifier_host_registered(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
}

/// Parses a `RegisterStatusNotifierItem(service)` argument + D-Bus sender into `(bus_name, object_path)`.
///
/// Real-world Linux apps use three conventions:
/// 1. `service = "/StatusNotifierItem"` or `"/org/ayatana/NotificationItem/app"` -> bus is `sender`
/// 2. `service = ":1.42/StatusNotifierItem"` -> split at first `'/'`
/// 3. `service = "org.kde.StatusNotifierItem-1234-1"` or `":1.42"` -> path is `"/StatusNotifierItem"`
pub fn parse_service_target(service: &str, sender: Option<&str>) -> Option<(String, String)> {
    let trimmed = service.trim();
    if trimmed.is_empty() {
        return sender.map(|s| (s.to_string(), "/StatusNotifierItem".to_string()));
    }
    if trimmed.starts_with('/') {
        let bus = sender?.trim();
        if bus.is_empty() {
            return None;
        }
        return Some((bus.to_string(), trimmed.to_string()));
    }
    if let Some((bus, rest)) = trimmed.split_once('/') {
        if bus.is_empty() {
            return None;
        }
        return Some((bus.to_string(), format!("/{rest}")));
    }
    Some((trimmed.to_string(), "/StatusNotifierItem".to_string()))
}

/// Splits a canonical `"{bus_name}{object_path}"` entry into `(bus_name, object_path)`.
pub fn split_canonical_item(canonical: &str) -> Option<(String, String)> {
    parse_service_target(canonical, None)
}

/// Converts big-endian ARGB32 bytes (`[A, R, G, B]`, per SNI spec) into RGBA8888 (`[R, G, B, A]`).
pub fn argb32_to_rgba(width: i32, height: i32, argb: &[u8]) -> Option<TrayPixmap> {
    if width <= 0 || height <= 0 || width > 512 || height > 512 {
        return None;
    }
    let expected = (width as usize).checked_mul(height as usize)?.checked_mul(4)?;
    if argb.len() < expected {
        return None;
    }
    let mut rgba = Vec::with_capacity(expected);
    for px in argb[..expected].chunks_exact(4) {
        let a = px[0];
        let r = px[1];
        let g = px[2];
        let b = px[3];
        rgba.push(r);
        rgba.push(g);
        rgba.push(b);
        rgba.push(a);
    }
    Some(TrayPixmap {
        width,
        height,
        rgba,
    })
}

/// Strips GTK/Qt keyboard mnemonic underscores from a DBusMenu label (`"_Quit"` -> `"Quit"`,
/// `"Open __File"` -> `"Open _File"`).
pub fn strip_dbusmenu_mnemonic(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '_' {
            if chars.peek() == Some(&'_') {
                out.push('_');
                chars.next();
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Resolves an icon file path when `icon_name` is already a file path or resides inside
/// `icon_theme_path` (common for Electron/Chromium/AppIndicator apps like Antigravity & Discord).
pub fn resolve_icon_file(icon_name: &str, icon_theme_path: Option<&str>) -> Option<String> {
    let trimmed = icon_name.trim();
    if trimmed.is_empty() {
        return None;
    }
    let direct = Path::new(trimmed);
    if direct.is_absolute() && direct.is_file() {
        return Some(trimmed.to_string());
    }

    let base_dir = icon_theme_path.map(str::trim).filter(|s| !s.is_empty())?;
    let base = Path::new(base_dir);
    if !base.is_dir() {
        return None;
    }

    // 1. Direct `<dir>/<name>.{png,svg,xpm}` or `<dir>/<name>`
    let candidates = [
        base.join(trimmed),
        base.join(format!("{trimmed}.png")),
        base.join(format!("{trimmed}.svg")),
        base.join(format!("{trimmed}.xpm")),
    ];
    for c in candidates {
        if c.is_file() {
            return Some(c.to_string_lossy().to_string());
        }
    }

    // 2. Standard hicolor subdirectories inside `IconThemePath`
    for sub in [
        "hicolor/24x24/apps",
        "hicolor/22x22/apps",
        "hicolor/32x32/apps",
        "hicolor/48x48/apps",
        "hicolor/scalable/apps",
        "hicolor/24x24/status",
        "hicolor/22x22/status",
        "hicolor/32x32/status",
        "hicolor/scalable/status",
        "24x24/apps",
        "22x22/apps",
        "scalable/apps",
    ] {
        for ext in ["png", "svg"] {
            let p: PathBuf = base.join(sub).join(format!("{trimmed}.{ext}"));
            if p.is_file() {
                return Some(p.to_string_lossy().to_string());
            }
        }
    }

    None
}

/// Starts the background NEON DE System Tray D-Bus service (`StatusNotifierWatcher` +
/// `StatusNotifierItem` client + `com.canonical.dbusmenu` bridge).
pub fn start_tray_service() -> (TrayActionSender, mpsc::Receiver<TrayEvent>) {
    let (action_tx, action_rx) = mpsc::channel::<TrayAction>();
    let (event_tx, event_rx) = mpsc::channel::<TrayEvent>();

    let worker_notify_tx = action_tx.clone();
    thread::Builder::new()
        .name("neon-tray-service".to_string())
        .spawn(move || {
            run_tray_worker(worker_notify_tx, action_rx, event_tx);
        })
        .ok();

    (TrayActionSender { tx: action_tx }, event_rx)
}

fn bus_name_has_owner(conn: &Connection, name: &str) -> bool {
    let Ok(proxy) = Proxy::new(
        conn,
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
    ) else {
        return false;
    };
    proxy.call::<_, _, bool>("NameHasOwner", &(name,)).unwrap_or(false)
}

fn list_bus_names(conn: &Connection) -> Vec<String> {
    let Ok(proxy) = Proxy::new(
        conn,
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
    ) else {
        return Vec::new();
    };
    proxy.call::<_, _, Vec<String>>("ListNames", &()).unwrap_or_default()
}

fn run_tray_worker(
    notify_tx: mpsc::Sender<TrayAction>,
    action_rx: mpsc::Receiver<TrayAction>,
    event_tx: mpsc::Sender<TrayEvent>,
) {
    let probe_conn = match Connection::session() {
        Ok(c) => c,
        Err(err) => {
            warn!(%err, "Session D-Bus unavailable; System Tray service disabled");
            return;
        }
    };

    let registry = Arc::new(WatcherRegistry::new(notify_tx));
    let mut owned_watcher_conn: Option<Connection> = None;

    if !bus_name_has_owner(&probe_conn, KDE_WATCHER_NAME) {
        let iface = StatusNotifierWatcherIface {
            registry: Arc::clone(&registry),
        };
        let host_name = format!("org.kde.StatusNotifierHost-{}", std::process::id());
        registry.register_host(host_name.clone());

        let mut builder = match Builder::session() {
            Ok(b) => b,
            Err(err) => {
                warn!(%err, "Failed to build session D-Bus connection for StatusNotifierWatcher");
                return;
            }
        };
        if let Ok(wkn) = WellKnownName::try_from(KDE_WATCHER_NAME) {
            builder = match builder.name(wkn) {
                Ok(b) => b,
                Err(err) => {
                    warn!(%err, "Failed to request org.kde.StatusNotifierWatcher");
                    return;
                }
            };
        }
        if let Ok(host_wkn) = WellKnownName::try_from(host_name.as_str()) {
            match builder.name(host_wkn) {
                Ok(b) => builder = b,
                Err(err) => {
                    warn!(%err, "Failed to request StatusNotifierHost bus name");
                    return;
                }
            }
        }
        // Claim legacy Freedesktop watcher name too if free (for older Ayatana clients).
        if !bus_name_has_owner(&probe_conn, FDO_WATCHER_NAME)
            && let Ok(fdo_wkn) = WellKnownName::try_from(FDO_WATCHER_NAME)
        {
            match builder.name(fdo_wkn) {
                Ok(b) => builder = b,
                Err(_) => return,
            }
        }
        match builder
            .serve_at(WATCHER_PATH, iface)
            .and_then(Builder::build)
        {
            Ok(conn) => {
                info!(
                    watcher = KDE_WATCHER_NAME,
                    host = %host_name,
                    "Started NEON DE System Tray StatusNotifierWatcher"
                );
                // Discover any StatusNotifierItems already running before neon-shell started.
                for name in list_bus_names(&probe_conn) {
                    if name.starts_with("org.kde.StatusNotifierItem-")
                        || name.starts_with("org.ayatana.NotificationItem-")
                    {
                        let canonical = format!("{name}/StatusNotifierItem");
                        registry.insert_item(canonical);
                    }
                }
                owned_watcher_conn = Some(conn);
            }
            Err(err) => {
                warn!(%err, "Failed to start StatusNotifierWatcher; falling back to host-only mode");
            }
        }
    } else {
        info!(
            "External {} already running; attaching NEON DE Top Bar as StatusNotifierHost client",
            KDE_WATCHER_NAME
        );
        // Register our host identity with the external watcher.
        let host_name = format!("org.kde.StatusNotifierHost-{}", std::process::id());
        if let Ok(proxy) = Proxy::new(&probe_conn, KDE_WATCHER_NAME, WATCHER_PATH, KDE_WATCHER_NAME)
        {
            let _ = proxy.call::<_, _, ()>("RegisterStatusNotifierHost", &(host_name.as_str(),));
        }
    }

    let mut last_snapshot: Vec<TrayItem> = Vec::new();
    let mut needs_sync = true;

    loop {
        if needs_sync {
            needs_sync = false;
            let raw_keys = if owned_watcher_conn.is_some() {
                // Prune any registered item whose bus owner has exited.
                let removed = registry.remove_items_matching(|canonical| {
                    split_canonical_item(canonical)
                        .map(|(bus, _)| !bus_name_has_owner(&probe_conn, &bus))
                        .unwrap_or(true)
                });
                if !removed.is_empty() {
                    debug!(count = removed.len(), "Pruned exited StatusNotifierItems");
                }
                registry.snapshot_items()
            } else {
                query_external_watcher_items(&probe_conn)
            };

            let mut resolved_items = Vec::new();
            for key in raw_keys {
                if let Some(item) = query_sni_item(&probe_conn, &key)
                    && item.status != "Passive"
                {
                    resolved_items.push(item);
                }
            }

            if resolved_items != last_snapshot {
                last_snapshot = resolved_items.clone();
                if event_tx.send(TrayEvent::ItemsUpdated(resolved_items)).is_err() {
                    break;
                }
            }
        }

        match action_rx.recv_timeout(Duration::from_millis(1500)) {
            Ok(action) => {
                handle_tray_action(&probe_conn, action, &event_tx, &mut needs_sync);
                while let Ok(more) = action_rx.try_recv() {
                    handle_tray_action(&probe_conn, more, &event_tx, &mut needs_sync);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                needs_sync = true;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    drop(owned_watcher_conn);
}

fn handle_tray_action(
    conn: &Connection,
    action: TrayAction,
    event_tx: &mpsc::Sender<TrayEvent>,
    needs_sync: &mut bool,
) {
    match action {
        TrayAction::Refresh => {
            *needs_sync = true;
        }
        TrayAction::Activate {
            bus_name,
            object_path,
            x,
            y,
        } => {
            if !invoke_sni_method(conn, &bus_name, &object_path, "Activate", x, y) {
                debug!(bus = %bus_name, path = %object_path, "SNI Activate failed or unsupported");
            }
            *needs_sync = true;
        }
        TrayAction::SecondaryActivate {
            bus_name,
            object_path,
            x,
            y,
        } => {
            let _ = invoke_sni_method(conn, &bus_name, &object_path, "SecondaryActivate", x, y);
            *needs_sync = true;
        }
        TrayAction::FetchMenu {
            key,
            bus_name,
            menu_path,
        } => {
            let items = fetch_dbusmenu_items(conn, &bus_name, &menu_path);
            let _ = event_tx.send(TrayEvent::MenuLoaded {
                key,
                bus_name,
                menu_path,
                items,
            });
        }
        TrayAction::TriggerMenuItem {
            bus_name,
            menu_path,
            menu_item_id,
        } => {
            trigger_dbusmenu_item(conn, &bus_name, &menu_path, menu_item_id);
            *needs_sync = true;
        }
    }
}

fn query_external_watcher_items(conn: &Connection) -> Vec<String> {
    let Ok(proxy) = Proxy::new(conn, KDE_WATCHER_NAME, WATCHER_PATH, KDE_WATCHER_NAME) else {
        return Vec::new();
    };
    proxy
        .get_property::<Vec<String>>("RegisteredStatusNotifierItems")
        .unwrap_or_default()
}

fn query_sni_item(conn: &Connection, canonical_key: &str) -> Option<TrayItem> {
    let (bus_name, object_path) = split_canonical_item(canonical_key)?;
    if !bus_name_has_owner(conn, &bus_name) {
        return None;
    }

    // Try KDE interface first, falling back to Freedesktop interface if needed.
    let props = fetch_all_properties(conn, &bus_name, &object_path, KDE_ITEM_IFACE)
        .or_else(|| fetch_all_properties(conn, &bus_name, &object_path, FDO_ITEM_IFACE))?;

    let id = prop_string(&props, "Id").unwrap_or_default();
    let title = prop_string(&props, "Title").unwrap_or_default();
    let status = prop_string(&props, "Status").unwrap_or_else(|| "Active".to_string());
    let icon_name = prop_string(&props, "IconName").filter(|s| !s.is_empty());
    let icon_theme_path = prop_string(&props, "IconThemePath").filter(|s| !s.is_empty());
    let icon_file = icon_name
        .as_deref()
        .and_then(|name| resolve_icon_file(name, icon_theme_path.as_deref()));
    let icon_rgba = extract_best_pixmap(&props, "IconPixmap");
    let tooltip = extract_tooltip_text(&props);
    let menu_path = props.get("Menu").and_then(owned_value_to_path_string);
    let item_is_menu = props
        .get("ItemIsMenu")
        .and_then(|v| bool::try_from(v).ok())
        .unwrap_or(false);

    Some(TrayItem {
        key: canonical_key.to_string(),
        bus_name,
        object_path,
        id,
        title,
        tooltip,
        status,
        icon_name,
        icon_file,
        icon_rgba,
        menu_path,
        item_is_menu,
    })
}

fn fetch_all_properties(
    conn: &Connection,
    bus_name: &str,
    object_path: &str,
    iface: &str,
) -> Option<HashMap<String, OwnedValue>> {
    let proxy = Proxy::new(
        conn,
        bus_name,
        object_path,
        "org.freedesktop.DBus.Properties",
    )
    .ok()?;
    let map: HashMap<String, OwnedValue> = proxy.call("GetAll", &(iface,)).ok()?;
    if map.is_empty() { None } else { Some(map) }
}

fn prop_string(props: &HashMap<String, OwnedValue>, key: &str) -> Option<String> {
    let val = props.get(key)?;
    <&str>::try_from(val).ok().map(ToOwned::to_owned)
}

fn owned_value_to_path_string(val: &OwnedValue) -> Option<String> {
    if let Ok(path) = OwnedObjectPath::try_from(val.try_clone().ok()?) {
        let s = path.as_str().to_string();
        if s.len() > 1 && s != "/NO_DBUSMENU" {
            return Some(s);
        }
    }
    if let Ok(s) = <&str>::try_from(val)
        && s.starts_with('/')
        && s.len() > 1
        && s != "/NO_DBUSMENU"
    {
        return Some(s.to_string());
    }
    None
}

fn extract_tooltip_text(props: &HashMap<String, OwnedValue>) -> String {
    let Some(val) = props.get("ToolTip") else {
        return String::new();
    };
    let Ok(s) = <&Structure<'_>>::try_from(val) else {
        return String::new();
    };
    let fields = s.fields();
    let title = fields
        .get(2)
        .and_then(|v| <&str>::try_from(v).ok())
        .unwrap_or("")
        .trim();
    let desc = fields
        .get(3)
        .and_then(|v| <&str>::try_from(v).ok())
        .unwrap_or("")
        .trim();
    match (title.is_empty(), desc.is_empty()) {
        (false, false) => format!("{title} — {desc}"),
        (false, true) => title.to_string(),
        (true, false) => desc.to_string(),
        (true, true) => String::new(),
    }
}

fn extract_best_pixmap(props: &HashMap<String, OwnedValue>, key: &str) -> Option<TrayPixmap> {
    let val = props.get(key)?;
    let raw_list: Vec<(i32, i32, Vec<u8>)> = val.try_clone().ok()?.try_into().ok()?;
    // Choose the pixmap closest to 22px (preferring >= 18px so it downscales crisply).
    let best = raw_list
        .into_iter()
        .filter(|(w, h, bytes)| *w > 0 && *h > 0 && bytes.len() >= (*w as usize * *h as usize * 4))
        .min_by_key(|(w, h, _)| {
            let dim = (*w).max(*h);
            let penalty = if dim < 16 { 1000 } else { 0 };
            penalty + (dim - 24).abs()
        })?;
    argb32_to_rgba(best.0, best.1, &best.2)
}

fn invoke_sni_method(
    conn: &Connection,
    bus_name: &str,
    object_path: &str,
    method: &str,
    x: i32,
    y: i32,
) -> bool {
    for iface in [KDE_ITEM_IFACE, FDO_ITEM_IFACE] {
        if let Ok(proxy) = Proxy::new(conn, bus_name, object_path, iface)
            && proxy.call::<_, _, ()>(method, &(x, y)).is_ok()
        {
            return true;
        }
    }
    false
}

fn fetch_dbusmenu_items(conn: &Connection, bus_name: &str, menu_path: &str) -> Vec<TrayMenuItem> {
    let Ok(proxy) = Proxy::new(conn, bus_name, menu_path, DBUSMENU_IFACE) else {
        return Vec::new();
    };
    let _ = proxy.call::<_, _, bool>("AboutToShow", &(0i32,));

    let prop_names: Vec<&str> = Vec::new();
    let Ok((_rev, root)) =
        proxy.call::<_, _, (u32, OwnedValue)>("GetLayout", &(0i32, -1i32, prop_names))
    else {
        return Vec::new();
    };

    parse_dbusmenu_node(&root)
        .map(|node| node.children)
        .unwrap_or_default()
}

fn parse_dbusmenu_node(val: &OwnedValue) -> Option<TrayMenuItem> {
    let s = <&Structure<'_>>::try_from(val).ok()?;
    let fields = s.fields();
    if fields.len() < 3 {
        return None;
    }

    let id = i32::try_from(&fields[0]).ok()?;
    let props: HashMap<String, OwnedValue> = fields[1].try_clone().ok()?.try_into().ok().unwrap_or_default();
    let raw_children: Vec<OwnedValue> = fields[2].try_clone().ok()?.try_into().ok().unwrap_or_default();

    let visible = props
        .get("visible")
        .and_then(|v| bool::try_from(v).ok())
        .unwrap_or(true);
    let enabled = props
        .get("enabled")
        .and_then(|v| bool::try_from(v).ok())
        .unwrap_or(true);
    let item_type = prop_string(&props, "type").unwrap_or_default();
    let is_separator = item_type == "separator";
    let label = prop_string(&props, "label")
        .map(|raw| strip_dbusmenu_mnemonic(&raw))
        .unwrap_or_default();
    let toggle_type = prop_string(&props, "toggle-type").filter(|s| !s.is_empty());
    let toggle_state = props
        .get("toggle-state")
        .and_then(|v| i32::try_from(v).ok())
        .unwrap_or(-1);
    let icon_name = prop_string(&props, "icon-name").filter(|s| !s.is_empty());

    let children: Vec<TrayMenuItem> = raw_children
        .iter()
        .filter_map(parse_dbusmenu_node)
        .filter(|child| child.visible)
        .collect();

    Some(TrayMenuItem {
        id,
        label,
        enabled,
        visible,
        is_separator,
        toggle_type,
        toggle_state,
        icon_name,
        children,
    })
}

fn trigger_dbusmenu_item(conn: &Connection, bus_name: &str, menu_path: &str, menu_item_id: i32) {
    let Ok(proxy) = Proxy::new(conn, bus_name, menu_path, DBUSMENU_IFACE) else {
        return;
    };
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as u32)
        .unwrap_or(0);
    let empty_data = Value::from("");
    if let Err(err) = proxy.call::<_, _, ()>("Event", &(menu_item_id, "clicked", empty_data, ts)) {
        warn!(%err, bus = %bus_name, path = %menu_path, id = menu_item_id, "DBusMenu Event failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_service_target_handles_all_linux_sni_conventions() {
        assert_eq!(
            parse_service_target("/StatusNotifierItem", Some(":1.42")),
            Some((":1.42".to_string(), "/StatusNotifierItem".to_string()))
        );
        assert_eq!(
            parse_service_target("/org/ayatana/NotificationItem/obs", Some(":1.88")),
            Some((
                ":1.88".to_string(),
                "/org/ayatana/NotificationItem/obs".to_string()
            ))
        );
        assert_eq!(
            parse_service_target("org.kde.StatusNotifierItem-1234-1", Some(":1.99")),
            Some((
                "org.kde.StatusNotifierItem-1234-1".to_string(),
                "/StatusNotifierItem".to_string()
            ))
        );
        assert_eq!(
            parse_service_target(":1.55/org/kde/item", None),
            Some((":1.55".to_string(), "/org/kde/item".to_string()))
        );
    }

    #[test]
    fn argb32_to_rgba_converts_network_byte_order() {
        let argb = [0x80, 0xFF, 0x20, 0x10, 0xFF, 0x00, 0xAA, 0xBB];
        let pix = argb32_to_rgba(2, 1, &argb).expect("valid 2x1 pixmap");
        assert_eq!(pix.width, 2);
        assert_eq!(pix.height, 1);
        assert_eq!(pix.rgba, vec![0xFF, 0x20, 0x10, 0x80, 0x00, 0xAA, 0xBB, 0xFF]);
    }

    #[test]
    fn strip_dbusmenu_mnemonic_preserves_escaped_underscores() {
        assert_eq!(strip_dbusmenu_mnemonic("_Quit"), "Quit");
        assert_eq!(strip_dbusmenu_mnemonic("Show _Antigravity"), "Show Antigravity");
        assert_eq!(strip_dbusmenu_mnemonic("Open __Config"), "Open _Config");
    }

    #[test]
    fn resolve_icon_file_finds_icon_inside_theme_path() {
        let dir = std::env::temp_dir().join(format!("neon-tray-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let png = dir.join("antigravity_tray.png");
        let _ = std::fs::write(&png, b"png");

        let resolved = resolve_icon_file("antigravity_tray", dir.to_str());
        assert_eq!(resolved.as_deref(), png.to_str());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
