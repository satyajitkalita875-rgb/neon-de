//! XDG Desktop Entry (`.desktop`) discovery, parsing, search, and safe application launching.

use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{Mutex, OnceLock},
};

use gtk4::{IconTheme, Image, gdk::Display};
use tracing::{info, warn};

static LIVE_XWAYLAND_DISPLAY: OnceLock<Mutex<Option<String>>> = OnceLock::new();
static LIVE_CURSOR_CONFIG: OnceLock<Mutex<(String, u32)>> = OnceLock::new();

/// Updates the XWayland `DISPLAY` (e.g. `:0`) discovered from `neon-compositor` IPC.
pub fn set_live_xwayland_display(display: Option<&str>) {
    let lock = LIVE_XWAYLAND_DISPLAY.get_or_init(|| Mutex::new(None));
    if let Ok(mut guard) = lock.lock() {
        *guard = display.map(str::to_string);
    }
}

/// Updates the cursor theme & size propagated to newly launched applications.
pub fn set_live_cursor_config(theme: Option<&str>, size: Option<u32>) {
    let lock = LIVE_CURSOR_CONFIG.get_or_init(|| Mutex::new(("macOS".into(), 20)));
    if let Ok(mut guard) = lock.lock() {
        if let Some(t) = theme
            && !t.trim().is_empty()
        {
            guard.0 = t.to_string();
        }
        if let Some(s) = size {
            guard.1 = s.clamp(16, 48);
        }
    }
}

fn resolve_x11_display() -> Option<String> {
    if let Some(lock) = LIVE_XWAYLAND_DISPLAY.get()
        && let Ok(guard) = lock.lock()
        && let Some(ref d) = *guard
    {
        return Some(d.clone());
    }
    if let Ok(env_d) = std::env::var("DISPLAY")
        && !env_d.trim().is_empty()
    {
        return Some(env_d);
    }
    if let Ok(entries) = fs::read_dir("/tmp/.X11-unix") {
        let mut sockets: Vec<String> = entries
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                name.strip_prefix('X').map(|n| format!(":{n}"))
            })
            .collect();
        sockets.sort();
        if let Some(first) = sockets.into_iter().next() {
            return Some(first);
        }
    }
    None
}

/// An application-specific quick action parsed from `[Desktop Action <id>]` in a `.desktop` file
/// (e.g. "New Incognito Window", "New Private Window", "New Tab", etc.).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopAction {
    pub id: String,
    pub name: String,
    pub icon: Option<String>,
    pub exec_program: String,
    pub exec_args: Vec<String>,
}

impl DesktopAction {
    /// Launches this application-specific desktop action directly without a shell interpreter.
    pub fn launch(&self, working_dir: Option<&str>, terminal: bool) {
        spawn_desktop_command(
            &self.name,
            &self.exec_program,
            &self.exec_args,
            working_dir,
            terminal,
        );
    }
}

/// Normalized metadata for an installed desktop application parsed from an XDG `.desktop` file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopApp {
    pub id: String,
    pub name: String,
    pub generic_name: Option<String>,
    pub comment: Option<String>,
    pub exec_program: String,
    pub exec_args: Vec<String>,
    pub working_dir: Option<String>,
    pub icon: Option<String>,
    pub startup_wm_class: Option<String>,
    pub categories: Vec<String>,
    pub mime_types: Vec<String>,
    pub actions: Vec<DesktopAction>,
    pub desktop_file: Option<PathBuf>,
    pub terminal: bool,
}

fn spawn_desktop_command(
    label: &str,
    exec_program: &str,
    exec_args: &[String],
    working_dir: Option<&str>,
    terminal: bool,
) {
    let (program, args) = if terminal {
        match neon_system::apps::wrap_terminal_command(exec_program, exec_args) {
            Some(wrapped) => wrapped,
            None => {
                warn!(
                    app = %label,
                    program = %exec_program,
                    "Cannot launch Terminal=true application: no terminal emulator found in PATH"
                );
                return;
            }
        }
    } else {
        (exec_program.to_string(), exec_args.to_vec())
    };

    let mut cmd = Command::new(&program);
    cmd.args(&args);
    if let Some(dir) = working_dir {
        let p = Path::new(dir);
        if p.is_dir() {
            cmd.current_dir(p);
        }
    }
    if let Some(disp) = resolve_x11_display() {
        cmd.env("DISPLAY", disp);
    }
    if let Some(lock) = LIVE_CURSOR_CONFIG.get()
        && let Ok(guard) = lock.lock()
    {
        cmd.env("XCURSOR_THEME", &guard.0);
        cmd.env("XCURSOR_SIZE", guard.1.to_string());
    }
    match cmd.spawn() {
        Ok(child) => {
            info!(
                pid = child.id(),
                app = %label,
                program = %program,
                terminal,
                "Launched XDG application from NEON Shell"
            );
        }
        Err(err) => {
            warn!(
                app = %label,
                program = %program,
                terminal,
                ?err,
                "Failed to launch XDG application"
            );
        }
    }
}

impl DesktopApp {
    /// Returns true if this application matches a case-insensitive search query.
    pub fn matches_query(&self, query: &str) -> bool {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return true;
        }
        self.name.to_lowercase().contains(&q)
            || self.id.to_lowercase().contains(&q)
            || self
                .generic_name
                .as_ref()
                .map(|g| g.to_lowercase().contains(&q))
                .unwrap_or(false)
            || self
                .comment
                .as_ref()
                .map(|c| c.to_lowercase().contains(&q))
                .unwrap_or(false)
            || self
                .categories
                .iter()
                .any(|cat| cat.to_lowercase().contains(&q))
    }

    /// Returns true if the application belongs to the requested category (or `"All"`).
    #[allow(dead_code)]
    pub fn matches_category(&self, category: &str) -> bool {
        if category.eq_ignore_ascii_case("All") {
            return true;
        }
        let target = category.to_lowercase();
        self.categories.iter().any(|c| {
            let low = c.to_lowercase();
            low == target
                || (target == "internet" && low == "network")
                || (target == "utilities" && (low == "utility" || low == "accessories"))
        })
    }

    /// Launches this application directly without passing through a shell interpreter.
    ///
    /// Applications with `Terminal=true` (such as Vim, htop, or Ranger) are wrapped in an
    /// installed terminal emulator so they open interactively instead of exiting without a TTY.
    pub fn launch(&self) {
        spawn_desktop_command(
            &self.name,
            &self.exec_program,
            &self.exec_args,
            self.working_dir.as_deref(),
            self.terminal,
        );
    }

    /// Launches this application opening `file_path` as its target argument (for "Open With").
    pub fn launch_with_file(&self, file_path: &Path) {
        let mut args = self.exec_args.clone();
        args.push(file_path.to_string_lossy().into_owned());
        spawn_desktop_command(
            &self.name,
            &self.exec_program,
            &args,
            file_path
                .parent()
                .and_then(|p| p.to_str())
                .or(self.working_dir.as_deref()),
            self.terminal,
        );
    }
}

/// Returns the Freedesktop XDG user autostart directory (`$XDG_CONFIG_HOME/autostart`).
pub fn autostart_dir() -> PathBuf {
    if let Some(cfg_home) = std::env::var_os("XDG_CONFIG_HOME")
        && !cfg_home.is_empty()
    {
        return PathBuf::from(cfg_home).join("autostart");
    }
    std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join(".config/autostart"))
        .unwrap_or_else(|| PathBuf::from("/tmp/autostart"))
}

/// Returns `true` if `app_id` is enabled in `~/.config/autostart/` (and not marked `Hidden=true`).
pub fn is_launch_at_login(app_id: &str) -> bool {
    let path = autostart_dir().join(format!("{app_id}.desktop"));
    let Ok(content) = fs::read_to_string(path) else {
        return false;
    };
    !content
        .lines()
        .any(|l| l.trim().eq_ignore_ascii_case("Hidden=true") || l.trim().eq_ignore_ascii_case("X-GNOME-Autostart-enabled=false"))
}

/// Enables or disables XDG Launch at Login for `app` by managing `~/.config/autostart/<id>.desktop`.
pub fn set_launch_at_login(app: &DesktopApp, enabled: bool) -> Result<(), String> {
    let dir = autostart_dir();
    let target = dir.join(format!("{}.desktop", app.id));
    if !enabled {
        if target.exists() {
            fs::remove_file(&target).map_err(|e| e.to_string())?;
        }
        return Ok(());
    }
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    if let Some(ref src) = app.desktop_file
        && src.is_file()
    {
        fs::copy(src, &target).map_err(|e| e.to_string())?;
        return Ok(());
    }
    let mut exec_line = app.exec_program.clone();
    for a in &app.exec_args {
        exec_line.push(' ');
        if a.contains(' ') {
            exec_line.push('"');
            exec_line.push_str(a);
            exec_line.push('"');
        } else {
            exec_line.push_str(a);
        }
    }
    let body = format!(
        "[Desktop Entry]\nType=Application\nName={}\nExec={}\nIcon={}\nTerminal={}\nX-GNOME-Autostart-enabled=true\n",
        app.name,
        exec_line,
        app.icon.as_deref().unwrap_or("application-x-executable"),
        if app.terminal { "true" } else { "false" }
    );
    fs::write(&target, body).map_err(|e| e.to_string())
}

/// Resolves installed applications capable of opening `path` via MIME-type matching (for "Open With").
pub fn find_apps_for_file<'a>(path: &Path, all_apps: &'a [DesktopApp]) -> Vec<&'a DesktopApp> {
    let mime = guess_mime_type(path);
    let supertype = mime
        .split_once('/')
        .map(|(t, _)| format!("{t}/*"))
        .unwrap_or_default();

    let mut exact: Vec<&'a DesktopApp> = Vec::new();
    let mut fallback: Vec<&'a DesktopApp> = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();

    for app in all_apps {
        if app.mime_types.is_empty() {
            continue;
        }
        let has_exact = app.mime_types.iter().any(|m| m.eq_ignore_ascii_case(&mime));
        let has_super = !supertype.is_empty()
            && app.mime_types.iter().any(|m| {
                m.eq_ignore_ascii_case(&supertype)
                    || (mime.starts_with("text/") && m.eq_ignore_ascii_case("text/plain"))
            });
        if has_exact && seen.insert(&app.id) {
            exact.push(app);
        } else if has_super && seen.insert(&app.id) {
            fallback.push(app);
        }
    }
    exact.extend(fallback);
    exact.truncate(10);
    exact
}

/// Fast MIME type guesser combining directory detection and common Linux file extensions.
pub fn guess_mime_type(path: &Path) -> String {
    if path.is_dir() {
        return "inode/directory".into();
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    match ext.as_str() {
        "txt" | "log" | "cfg" | "conf" | "ini" | "toml" | "yaml" | "yml" | "env" => "text/plain",
        "md" | "markdown" => "text/markdown",
        "rs" => "text/rust",
        "py" => "text/x-python",
        "js" | "mjs" | "cjs" => "application/javascript",
        "ts" | "tsx" => "text/typescript",
        "json" => "application/json",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "sh" | "bash" | "zsh" => "application/x-shellscript",
        "c" | "h" | "cpp" | "cc" | "hpp" => "text/x-csrc",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "bmp" => "image/bmp",
        "pdf" => "application/pdf",
        "mp4" | "m4v" => "video/mp4",
        "mkv" => "video/x-matroska",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "flac" => "audio/flac",
        "ogg" => "audio/ogg",
        "zip" => "application/zip",
        "tar" => "application/x-tar",
        "gz" | "tgz" => "application/gzip",
        "xz" => "application/x-xz",
        "bz2" => "application/x-bzip2",
        "7z" => "application/x-7z-compressed",
        "rar" => "application/vnd.rar",
        "desktop" => "application/x-desktop",
        "csv" => "text/csv",
        "xml" => "application/xml",
        _ => "application/octet-stream",
    }
    .to_string()
}

/// Resolves the ordered list of pinned Dock apps: uses `pinned_ids` if non-empty,
/// falling back to smart defaults (`select_default_favorites`) on first run.
pub fn select_favorites_with_pins(apps: &[DesktopApp], pinned_ids: &[String]) -> Vec<DesktopApp> {
    if pinned_ids.is_empty() {
        return select_default_favorites(apps);
    }
    let mut out = Vec::new();
    for id in pinned_ids {
        if let Some(app) = apps.iter().find(|a| a.id.eq_ignore_ascii_case(id))
            && !out.iter().any(|existing: &DesktopApp| existing.id == app.id)
        {
            out.push(app.clone());
        }
    }
    if out.is_empty() {
        select_default_favorites(apps)
    } else {
        out
    }
}

/// Scans standard XDG, Flatpak, Snap, and user application directories and returns
/// deduplicated, sorted [`DesktopApp`]s.
pub fn discover_applications() -> Vec<DesktopApp> {
    let dirs = xdg_application_dirs();
    discover_in_dirs(&dirs)
}

/// Computes a fast fingerprint of all XDG/Flatpak/Snap `.desktop` directories (filenames + mtimes)
/// so the App Drawer and Dock automatically detect newly installed, edited, or removed apps
/// without needing to restart `neon-shell`.
pub fn compute_apps_dir_signature() -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    let mix = |h: &mut u64, bytes: &[u8]| {
        for &b in bytes {
            *h ^= u64::from(b);
            *h = h.wrapping_mul(0x100000001b3);
        }
    };

    for dir in xdg_application_dirs() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        let mut names: Vec<(String, u64)> = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("desktop") {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let mtime = fs::metadata(&path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            names.push((name, mtime));
        }
        names.sort();
        mix(&mut hash, dir.to_string_lossy().as_bytes());
        for (name, mtime) in names {
            mix(&mut hash, name.as_bytes());
            mix(&mut hash, &mtime.to_le_bytes());
        }
    }
    hash
}

/// Registers Flatpak, Snap, local hicolor, and pixmap icon search paths on the active GTK4
/// `IconTheme` so newly installed and sandboxed apps resolve crisp native icons.
pub fn register_extra_icon_search_paths() {
    let Some(display) = Display::default() else {
        return;
    };
    let theme = IconTheme::for_display(&display);
    let existing: HashSet<PathBuf> = theme.search_path().into_iter().collect();
    for dir in extra_icon_roots() {
        if dir.is_dir() && !existing.contains(&dir) {
            theme.add_search_path(&dir);
        }
    }
}

fn extra_icon_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        let h = PathBuf::from(home);
        roots.push(h.join(".local/share/icons"));
        roots.push(h.join(".local/share/flatpak/exports/share/icons"));
        roots.push(h.join(".icons"));
    }
    roots.push(PathBuf::from("/var/lib/flatpak/exports/share/icons"));
    roots.push(PathBuf::from("/var/lib/snapd/desktop/icons"));
    roots.push(PathBuf::from("/usr/local/share/icons"));
    roots.push(PathBuf::from("/usr/share/icons"));
    roots.push(PathBuf::from("/usr/share/pixmaps"));
    roots
}

fn strip_image_ext(name: &str) -> &str {
    for ext in [".svg", ".png", ".xpm", ".ico", ".jpg", ".jpeg"] {
        if let Some(prefix) = name
            .get(..name.len().saturating_sub(ext.len()))
            .filter(|_| name.len() > ext.len() && name[name.len() - ext.len()..].eq_ignore_ascii_case(ext))
        {
            return prefix;
        }
    }
    name
}

/// Searches hicolor/scalable/pixmaps directories directly on disk for an icon name (handles
/// newly installed Flatpak/local apps whose icon files were added after GTK cached its theme).
fn find_icon_file_on_disk(icon_name: &str) -> Option<PathBuf> {
    let trimmed = icon_name.trim();
    if trimmed.is_empty() {
        return None;
    }
    let direct = Path::new(trimmed);
    if direct.is_absolute() && direct.is_file() {
        return Some(direct.to_path_buf());
    }

    let stem = strip_image_ext(trimmed);
    let exts = ["svg", "png", "xpm"];
    let sizes = [
        "scalable", "512x512", "256x256", "192x192", "128x128", "96x96", "64x64", "48x48", "32x32",
    ];

    for root in extra_icon_roots() {
        if root.ends_with("pixmaps") {
            for ext in ["", ".svg", ".png", ".xpm"] {
                let p = root.join(format!("{trimmed}{ext}"));
                if p.is_file() {
                    return Some(p);
                }
            }
            continue;
        }
        for size in sizes {
            let apps_dir = root.join("hicolor").join(size).join("apps");
            if !apps_dir.is_dir() {
                continue;
            }
            for ext in exts {
                let p = apps_dir.join(format!("{stem}.{ext}"));
                if p.is_file() {
                    return Some(p);
                }
            }
        }
    }
    None
}

/// Authoritative, strict window-to-[`DesktopApp`] matcher shared by Dock and Context Menu.
///
/// Uses tiered exact matching (`StartupWMClass` > `.desktop` ID > reverse-DNS tail >
/// executable basename > Flatpak app-id arg > slugified name) and explicitly prevents Chrome
/// `--app-id=` PWAs or random alphabetical window-title substrings from hijacking other apps.
pub fn resolve_window_desktop_app<'a>(
    app_id: Option<&str>,
    title: Option<&str>,
    all_apps: &'a [DesktopApp],
) -> Option<&'a DesktopApp> {
    let raw_id = app_id.unwrap_or("").trim();
    let id_l = raw_id.to_lowercase();
    let id_tail = id_l.rsplit('.').next().unwrap_or(&id_l);
    let id_slug = normalize_slug(&id_l);
    let title_l = title.unwrap_or("").trim().to_lowercase();

    let mut best: Option<(u32, &'a DesktopApp)> = None;
    let mut consider = |score: u32, app: &'a DesktopApp| {
        if best.is_none_or(|(best_s, _)| score > best_s) {
            best = Some((score, app));
        }
    };

    for app in all_apps {
        let is_chrome_pwa = app
            .exec_args
            .iter()
            .any(|a| a.starts_with("--app-id=") || a.starts_with("--app="));

        if !id_l.is_empty() {
            // Tier 1: Exact StartupWMClass match (handles Chrome PWAs, OBS, Antigravity, BitChord, etc.)
            if let Some(ref wmc) = app.startup_wm_class
                && wmc.trim().eq_ignore_ascii_case(raw_id)
            {
                consider(100, app);
                continue;
            }

            // Tier 2: Exact .desktop file ID match (handles Flatpak reverse-DNS & standard IDs)
            let app_id_l = app.id.to_lowercase();
            if app_id_l == id_l {
                consider(95, app);
                continue;
            }

            // Chrome PWAs must ONLY match via exact StartupWMClass, exact .desktop ID, or exact
            // Chrome PWA `--app-id=<id>` token (`crx_<id>` / `chrome-<id>-*`), so they can never
            // hijack Google Chrome's Dock icon.
            if is_chrome_pwa {
                let pwa_id_match = app.exec_args.iter().find_map(|a| a.strip_prefix("--app-id=")).is_some_and(|pwa_id| {
                    let pid = pwa_id.to_lowercase();
                    !pid.is_empty()
                        && (id_l == format!("crx_{pid}")
                            || id_l.starts_with(&format!("chrome-{pid}-"))
                            || id_l == format!("chrome-{pid}"))
                });
                if pwa_id_match {
                    consider(98, app);
                }
                continue;
            }

            // Tier 3: Reverse-DNS tail match (`com.blackmagicdesign.resolve` <-> `resolve`,
            // `org.gnome.Nautilus` <-> `nautilus`, `firefox_firefox` <-> `firefox`)
            let app_tail = app_id_l.rsplit('.').next().unwrap_or(&app_id_l);
            let snap_prefix = app_id_l.split('_').next().unwrap_or(&app_id_l);
            if app_tail == id_l || app_id_l == id_tail || (id_tail.len() >= 3 && app_tail == id_tail) || snap_prefix == id_l {
                consider(90, app);
                continue;
            }

            // Tier 4: Exact executable basename match (ignoring generic launchers like `flatpak`)
            let exec_base = Path::new(&app.exec_program)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or(&app.exec_program)
                .to_lowercase();
            let exec_clean = exec_base
                .strip_suffix("-stable")
                .or_else(|| exec_base.strip_suffix("-bin"))
                .unwrap_or(&exec_base);
            let is_generic_wrapper = matches!(
                exec_base.as_str(),
                "flatpak" | "env" | "sh" | "bash" | "python" | "python3" | "java" | "electron" | "snap"
            );
            if !is_generic_wrapper && (exec_base == id_l || exec_clean == id_l || exec_clean == id_tail) {
                consider(85, app);
                continue;
            }

            // Tier 5: Flatpak `exec_args` exact app ID or `--command=<bin>` match
            if exec_base == "flatpak" {
                let fp_match = app.exec_args.iter().any(|arg| {
                    let al = arg.to_lowercase();
                    al == id_l
                        || al.strip_prefix("--command=").is_some_and(|cmd| cmd == id_l || cmd == id_tail)
                });
                if fp_match {
                    consider(82, app);
                    continue;
                }
            }

            // Tier 6: Exact normalized name slug match (`davinci-resolve` <-> `DaVinci Resolve`)
            let name_slug = normalize_slug(&app.name);
            if !id_slug.is_empty() && id_slug.len() >= 3 && name_slug == id_slug {
                consider(70, app);
                continue;
            }
        } else if !title_l.is_empty() && !is_chrome_pwa {
            // Tier 7 (ONLY when `app_id` is completely missing): exact match against the trailing
            // app-name segment of the window title (e.g. `"Document - LibreOffice Writer"`).
            let name_l = app.name.to_lowercase();
            if name_l.len() >= 3 {
                let trailing = title_l
                    .rsplit(['-', '—', '–', '|'])
                    .next()
                    .map(str::trim)
                    .unwrap_or("");
                if trailing == name_l || title_l == name_l {
                    consider(50, app);
                }
            }
        }
    }

    best.map(|(_, app)| app)
}

fn normalize_slug(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

/// Loads an application icon cleanly for Dock or Launchpad with zero missing-icon symbols:
/// 1. Supports direct filesystem paths (`/snap/...`, `/opt/...`, `/usr/share/pixmaps/...`)
/// 2. Checks the active GTK4 IconTheme (`WhiteSur-dark` / `WhiteSur-light`) + Flatpak/local roots
/// 3. Falls back to direct hicolor/pixmap disk lookup (catches newly installed apps immediately!)
/// 4. Falls back cleanly to `application-x-executable`.
pub fn load_app_icon(icon_spec: Option<&str>, app_hint: &str, pixel_size: i32) -> Image {
    let icon_theme = Display::default().map(|d| IconTheme::for_display(&d));

    if let Some(raw) = icon_spec.map(str::trim).filter(|s| !s.is_empty()) {
        // 1. Absolute filesystem path (e.g. Chrome PWA / DaVinci Resolve / AppImage / pixmaps)
        let as_path = Path::new(raw);
        if as_path.is_absolute() && as_path.exists() {
            let img = Image::from_file(as_path);
            img.set_pixel_size(pixel_size);
            return img;
        }

        // 2. Direct lookup in active icon theme
        if let Some(ref theme) = icon_theme {
            if theme.has_icon(raw) {
                let img = Image::from_icon_name(raw);
                img.set_pixel_size(pixel_size);
                return img;
            }

            // Try stripped image extension & reverse-DNS / snap stem variants
            let stem = strip_image_ext(raw);
            let mut candidates: Vec<String> = vec![stem.to_string(), stem.to_lowercase()];
            if let Some((prefix, _)) = stem.split_once('_') {
                candidates.push(prefix.to_lowercase());
            }
            if let Some((_, suffix)) = stem.rsplit_once('.') {
                candidates.push(suffix.to_lowercase());
            }
            for cand in candidates {
                if !cand.is_empty() && theme.has_icon(&cand) {
                    let img = Image::from_icon_name(&cand);
                    img.set_pixel_size(pixel_size);
                    return img;
                }
            }
        }

        // 3. Direct disk scan in hicolor / Flatpak / local icons / pixmaps (catches newly installed apps!)
        if let Some(disk_path) = find_icon_file_on_disk(raw) {
            let img = Image::from_file(disk_path);
            img.set_pixel_size(pixel_size);
            return img;
        }
    }

    // 4. Smart lookup by normalized app_hint (`app_id` or `DesktopApp::name`)
    let raw_hint = app_hint.trim();
    let lower = raw_hint.to_lowercase();
    if !lower.is_empty() {
        let slug = normalize_slug(&lower);
        let tail = lower.rsplit('.').next().unwrap_or(&lower).to_string();
        let mut name_candidates = vec![raw_hint.to_string(), lower.clone(), slug, tail];
        if lower.contains("firefox") {
            name_candidates.push("firefox".into());
        } else if lower.contains("davinci") || lower == "resolve" {
            name_candidates.push("davinci-resolve".into());
            name_candidates.push("applications-multimedia".into());
        } else if lower.contains("obs") {
            name_candidates.push("com.obsproject.Studio".into());
            name_candidates.push("obs".into());
        } else if lower.contains("chrome") {
            name_candidates.push("google-chrome".into());
        } else if lower.contains("app center") || lower.contains("software") {
            name_candidates.push("system-software-install".into());
        } else if lower.contains("firmware") {
            name_candidates.push("system-software-update".into());
        } else if lower.contains("neonde.settings") || lower == "settings" {
            name_candidates.push("preferences-system".into());
        } else if lower == "terminal" || lower == "kitty" || lower == "alacritty" || lower == "foot" {
            name_candidates.push("utilities-terminal".into());
        }

        if let Some(ref theme) = icon_theme {
            for cand in &name_candidates {
                if !cand.is_empty() && theme.has_icon(cand) {
                    let img = Image::from_icon_name(cand);
                    img.set_pixel_size(pixel_size);
                    return img;
                }
            }
        }
        for cand in &name_candidates {
            if let Some(disk_path) = find_icon_file_on_disk(cand) {
                let img = Image::from_file(disk_path);
                img.set_pixel_size(pixel_size);
                return img;
            }
        }
    }

    // 5. Guaranteed valid fallback icon (never shows broken missing-icon glyph)
    let fallback = Image::from_icon_name("application-x-executable");
    fallback.set_pixel_size(pixel_size);
    fallback
}

/// Scans the given directories in precedence order (earlier directories override later ones).
pub fn discover_in_dirs(dirs: &[PathBuf]) -> Vec<DesktopApp> {
    let mut by_id: HashMap<String, DesktopApp> = HashMap::new();
    let mut hidden_ids: HashSet<String> = HashSet::new();

    for dir in dirs {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("desktop") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let id = stem.to_string();
            if by_id.contains_key(&id) || hidden_ids.contains(&id) {
                continue;
            }
            match parse_desktop_file(&path, &id) {
                Some(app) => {
                    by_id.insert(id, app);
                }
                None => {
                    hidden_ids.insert(id);
                }
            }
        }
    }

    let mut apps: Vec<DesktopApp> = by_id.into_values().collect();
    apps.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    apps
}

fn xdg_application_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let mut push_unique = |p: PathBuf| {
        if !dirs.contains(&p) {
            dirs.push(p);
        }
    };

    if let Some(data_home) = std::env::var_os("XDG_DATA_HOME")
        && !data_home.is_empty()
    {
        push_unique(PathBuf::from(data_home).join("applications"));
    } else if let Some(home) = std::env::var_os("HOME") {
        push_unique(PathBuf::from(home).join(".local/share/applications"));
    }
    if let Some(home) = std::env::var_os("HOME") {
        push_unique(PathBuf::from(home).join(".local/share/flatpak/exports/share/applications"));
    }
    if let Some(data_dirs) = std::env::var_os("XDG_DATA_DIRS") {
        for part in std::env::split_paths(&data_dirs) {
            push_unique(part.join("applications"));
        }
    }
    // Always include standard system, Flatpak, and Snap desktop entry directories even if
    // `XDG_DATA_DIRS` was customized or stripped in a bare-metal login session.
    push_unique(PathBuf::from("/usr/local/share/applications"));
    push_unique(PathBuf::from("/usr/share/applications"));
    push_unique(PathBuf::from("/var/lib/flatpak/exports/share/applications"));
    push_unique(PathBuf::from("/var/lib/snapd/desktop/applications"));
    dirs
}

/// Parses a single XDG `.desktop` file, returning `None` if hidden, `NoDisplay`, or non-Application.
pub fn parse_desktop_file(path: &Path, id: &str) -> Option<DesktopApp> {
    let content = fs::read_to_string(path).ok()?;
    let mut app = parse_desktop_str(&content, id)?;
    app.desktop_file = Some(path.to_path_buf());
    Some(app)
}

/// Parses XDG Desktop Entry INI text including `[Desktop Entry]` and `[Desktop Action <id>]`.
pub fn parse_desktop_str(content: &str, id: &str) -> Option<DesktopApp> {
    #[derive(Default)]
    struct RawAction {
        name: Option<String>,
        icon: Option<String>,
        exec: Option<String>,
    }

    enum Section {
        None,
        DesktopEntry,
        DesktopAction(String),
    }

    let mut section = Section::None;
    let mut entry_type = None;
    let mut name = None;
    let mut generic_name = None;
    let mut comment = None;
    let mut exec_raw = None;
    let mut working_dir = None;
    let mut icon = None;
    let mut startup_wm_class = None;
    let mut categories = Vec::new();
    let mut mime_types = Vec::new();
    let mut declared_actions: Vec<String> = Vec::new();
    let mut raw_actions: HashMap<String, RawAction> = HashMap::new();
    let mut no_display = false;
    let mut hidden = false;
    let mut terminal = false;

    for raw_line in content.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            let header = &line[1..line.len() - 1];
            section = if header == "Desktop Entry" {
                Section::DesktopEntry
            } else if let Some(act_id) = header.strip_prefix("Desktop Action ") {
                Section::DesktopAction(act_id.trim().to_string())
            } else {
                Section::None
            };
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        match &section {
            Section::DesktopEntry => match key {
                "Type" => entry_type = Some(value.to_string()),
                "Name" => name = Some(value.to_string()),
                "GenericName" => generic_name = Some(value.to_string()),
                "Comment" => comment = Some(value.to_string()),
                "Exec" => exec_raw = Some(value.to_string()),
                "Path" => working_dir = Some(value.to_string()),
                "Icon" => icon = Some(value.to_string()),
                "StartupWMClass" => startup_wm_class = Some(value.to_string()),
                "NoDisplay" => no_display = value.eq_ignore_ascii_case("true"),
                "Hidden" => hidden = value.eq_ignore_ascii_case("true"),
                "Terminal" => terminal = value.eq_ignore_ascii_case("true"),
                "Categories" => {
                    categories = value
                        .split(';')
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                        .collect();
                }
                "MimeType" => {
                    mime_types = value
                        .split(';')
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                        .collect();
                }
                "Actions" => {
                    declared_actions = value
                        .split(';')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                        .collect();
                }
                _ => {}
            },
            Section::DesktopAction(act_id) => {
                let entry = raw_actions.entry(act_id.clone()).or_default();
                match key {
                    "Name" => entry.name = Some(value.to_string()),
                    "Icon" => entry.icon = Some(value.to_string()),
                    "Exec" => entry.exec = Some(value.to_string()),
                    _ => {}
                }
            }
            Section::None => {}
        }
    }

    if no_display || hidden || entry_type.as_deref() != Some("Application") {
        return None;
    }

    let name = name?;
    let (exec_program, exec_args) = parse_exec_command(exec_raw.as_deref()?)?;

    // Preserve declared action ordering from `Actions=...;` (fallback to parsed section order).
    let mut actions = Vec::new();
    let mut added_ids: HashSet<String> = HashSet::new();
    for act_id in declared_actions {
        if let Some(raw) = raw_actions.get(&act_id)
            && let (Some(aname), Some(aexec)) = (raw.name.as_ref(), raw.exec.as_ref())
            && let Some((prog, args)) = parse_exec_command(aexec)
            && added_ids.insert(act_id.clone())
        {
            actions.push(DesktopAction {
                id: act_id,
                name: aname.clone(),
                icon: raw.icon.clone(),
                exec_program: prog,
                exec_args: args,
            });
        }
    }

    Some(DesktopApp {
        id: id.to_string(),
        name,
        generic_name,
        comment,
        exec_program,
        exec_args,
        working_dir,
        icon,
        startup_wm_class,
        categories,
        mime_types,
        actions,
        desktop_file: None,
        terminal,
    })
}

/// Splits an XDG `Exec=` command into `(program, args)` and strips `%u/%U/%f/%F/%i/%c/%k` field codes.
pub fn parse_exec_command(exec: &str) -> Option<(String, Vec<String>)> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_quotes = None;
    let mut chars = exec.chars().peekable();

    while let Some(ch) = chars.next() {
        match in_quotes {
            Some(q) if ch == q => in_quotes = None,
            Some(_) => {
                if ch == '\\'
                    && let Some(next) = chars.next()
                {
                    current.push(next);
                } else {
                    current.push(ch);
                }
            }
            None => match ch {
                '"' | '\'' => in_quotes = Some(ch),
                c if c.is_whitespace() => {
                    if !current.is_empty() {
                        tokens.push(std::mem::take(&mut current));
                    }
                }
                '\\' => {
                    if let Some(next) = chars.next() {
                        current.push(next);
                    }
                }
                _ => current.push(ch),
            },
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }

    let filtered: Vec<String> = tokens
        .into_iter()
        .filter(|tok| {
            !matches!(
                tok.as_str(),
                "%f" | "%F"
                    | "%u"
                    | "%U"
                    | "%d"
                    | "%D"
                    | "%n"
                    | "%N"
                    | "%i"
                    | "%c"
                    | "%k"
                    | "%v"
                    | "%m"
            )
        })
        .collect();

    let mut iter = filtered.into_iter();
    let program = iter.next()?;
    Some((program, iter.collect()))
}

/// Picks default pinned dock favorites from the discovered XDG applications list.
pub fn select_default_favorites(apps: &[DesktopApp]) -> Vec<DesktopApp> {
    const PREFERRED_IDS: &[&str] = &[
        "Alacritty",
        "kitty",
        "weston-terminal",
        "org.gnome.Nautilus",
        "thunar",
        "firefox",
        "brave-browser",
        "google-chrome",
        "code",
        "org.gnome.TextEditor",
    ];

    let mut selected = Vec::new();
    for preferred in PREFERRED_IDS {
        if let Some(app) = apps
            .iter()
            .find(|a| a.id.eq_ignore_ascii_case(preferred) || a.exec_program == *preferred)
            && !selected.iter().any(|s: &DesktopApp| s.id == app.id)
        {
            selected.push(app.clone());
            if selected.len() >= 5 {
                break;
            }
        }
    }

    // Ensure at least a terminal favorite is always available if installed.
    if selected.is_empty() {
        for fallback in ["weston-terminal", "alacritty", "kitty"] {
            if Command::new("which")
                .arg(fallback)
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
            {
                selected.push(DesktopApp {
                    id: fallback.into(),
                    name: "Terminal".into(),
                    generic_name: Some("Terminal Emulator".into()),
                    comment: Some("NEON DE Terminal".into()),
                    exec_program: fallback.into(),
                    exec_args: Vec::new(),
                    working_dir: None,
                    icon: Some("utilities-terminal".into()),
                    startup_wm_class: None,
                    categories: vec!["System".into(), "TerminalEmulator".into()],
                    mime_types: Vec::new(),
                    actions: Vec::new(),
                    desktop_file: None,
                    terminal: false,
                });
                break;
            }
        }
    }

    selected
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_desktop_entry_and_strips_field_codes() {
        let sample = r#"
[Desktop Entry]
Type=Application
Name=Alacritty
GenericName=Terminal
Comment=A fast, cross-platform, OpenGL terminal emulator
Exec=alacritty --working-directory /tmp %U
Icon=Alacritty
Categories=System;TerminalEmulator;
"#;
        let app = parse_desktop_str(sample, "Alacritty").expect("valid desktop entry");
        assert_eq!(app.name, "Alacritty");
        assert_eq!(app.exec_program, "alacritty");
        assert_eq!(app.exec_args, vec!["--working-directory", "/tmp"]);
        assert!(app.matches_query("opengl"));
        assert!(app.matches_category("System"));
        assert!(!app.matches_category("Graphics"));
    }

    #[test]
    fn skips_nodisplay_and_hidden_entries() {
        let hidden =
            "[Desktop Entry]\nType=Application\nName=Secret\nExec=secret\nNoDisplay=true\n";
        assert!(parse_desktop_str(hidden, "secret").is_none());
    }

    #[test]
    fn parses_terminal_entry_and_builds_terminal_wrapper_argv() {
        let vim = "[Desktop Entry]\nType=Application\nName=Vim\nExec=vim %F\nTerminal=true\n";
        let app = parse_desktop_str(vim, "vim").expect("valid vim entry");
        assert!(app.terminal);
        assert_eq!(app.exec_program, "vim");
        assert!(app.exec_args.is_empty());

        let (p1, a1) = neon_system::apps::build_terminal_argv("kitty", "vim", &[]);
        assert_eq!(p1, "kitty");
        assert_eq!(a1, vec!["--", "vim"]);

        let (p2, a2) =
            neon_system::apps::build_terminal_argv("alacritty", "vim", &["-u".into(), "NONE".into()]);
        assert_eq!(p2, "alacritty");
        assert_eq!(a2, vec!["-e", "vim", "-u", "NONE"]);

        let (p3, a3) = neon_system::apps::build_terminal_argv("wezterm", "htop", &[]);
        assert_eq!(p3, "wezterm");
        assert_eq!(a3, vec!["start", "--", "htop"]);
    }

    #[test]
    fn dock_window_matcher_handles_flatpaks_pwas_and_never_swaps_by_title() {
        let apps = vec![
            parse_desktop_str(
                "[Desktop Entry]\nType=Application\nName=Antigravity\nExec=/home/u/.local/bin/antigravity %U\nIcon=antigravity\nStartupWMClass=Antigravity\n",
                "antigravity",
            )
            .unwrap(),
            parse_desktop_str(
                "[Desktop Entry]\nType=Application\nName=Files\nExec=nautilus\nIcon=org.gnome.Nautilus\n",
                "org.gnome.Nautilus",
            )
            .unwrap(),
            parse_desktop_str(
                "[Desktop Entry]\nType=Application\nName=Google Chrome\nExec=/usr/bin/google-chrome-stable %U\nIcon=google-chrome\n",
                "google-chrome",
            )
            .unwrap(),
            parse_desktop_str(
                "[Desktop Entry]\nType=Application\nName=Instagram\nExec=/opt/google/chrome/google-chrome --profile-directory=Default --app-id=akpamiohjf\nIcon=/tmp/insta.png\nStartupWMClass=crx_akpamiohjf\n",
                "chrome-akpamiohjf-Default",
            )
            .unwrap(),
            parse_desktop_str(
                "[Desktop Entry]\nType=Application\nName=LocalSend\nExec=/usr/bin/flatpak run --command=localsend org.localsend.localsend_app\nIcon=org.localsend.localsend_app\n",
                "org.localsend.localsend_app",
            )
            .unwrap(),
            parse_desktop_str(
                "[Desktop Entry]\nType=Application\nName=OBS Studio\nExec=/usr/bin/flatpak run --command=obs com.obsproject.Studio\nIcon=com.obsproject.Studio\nStartupWMClass=obs\n",
                "com.obsproject.Studio",
            )
            .unwrap(),
            parse_desktop_str(
                "[Desktop Entry]\nType=Application\nName=Blackmagic Remote Monitor\nExec=\"/opt/resolve/bin/Blackmagic Remote Monitor\"\nIcon=/opt/resolve/graphics/Remote_Monitoring.png\n",
                "com.blackmagicdesign.resolve-DaVinciRemoteMonitoring",
            )
            .unwrap(),
            parse_desktop_str(
                "[Desktop Entry]\nType=Application\nName=DaVinci Resolve\nExec=/opt/resolve/bin/resolve\nIcon=/opt/resolve/graphics/DV_Resolve.png\n",
                "com.blackmagicdesign.resolve",
            )
            .unwrap(),
        ];

        // Chrome showing an Instagram/Files/Terminal tab must NEVER be hijacked by Instagram or Files!
        let chrome = resolve_window_desktop_app(
            Some("google-chrome"),
            Some("Instagram - Files - Terminal - Google Chrome"),
            &apps,
        )
        .expect("match chrome");
        assert_eq!(chrome.id, "google-chrome");

        // Instagram PWA matches via StartupWMClass or chrome-<app-id>-<profile>
        let insta = resolve_window_desktop_app(
            Some("crx_akpamiohjf"),
            Some("Instagram"),
            &apps,
        )
        .expect("match pwa");
        assert_eq!(insta.name, "Instagram");
        assert_eq!(
            resolve_window_desktop_app(Some("chrome-akpamiohjf-Profile_1"), Some("Instagram"), &apps)
                .unwrap()
                .name,
            "Instagram"
        );

        // Flatpak OBS matches by BOTH reverse-DNS app_id AND StartupWMClass `obs`
        assert_eq!(
            resolve_window_desktop_app(Some("com.obsproject.Studio"), Some("OBS"), &apps)
                .unwrap()
                .icon
                .as_deref(),
            Some("com.obsproject.Studio")
        );
        assert_eq!(
            resolve_window_desktop_app(Some("obs"), Some("OBS 30"), &apps)
                .unwrap()
                .icon
                .as_deref(),
            Some("com.obsproject.Studio")
        );

        // Flatpak LocalSend & DaVinci Resolve (never hijacked by Blackmagic Remote Monitor!) & Antigravity
        assert_eq!(
            resolve_window_desktop_app(Some("org.localsend.localsend_app"), None, &apps)
                .unwrap()
                .name,
            "LocalSend"
        );
        assert_eq!(
            resolve_window_desktop_app(Some("resolve"), Some("Project Manager"), &apps)
                .unwrap()
                .icon
                .as_deref(),
            Some("/opt/resolve/graphics/DV_Resolve.png")
        );
        assert_eq!(
            resolve_window_desktop_app(Some("antigravity"), Some("dock.rs - Files"), &apps)
                .unwrap()
                .name,
            "Antigravity"
        );

        // Reverse-DNS icon names must NOT have their last segment stripped as a file extension
        assert_eq!(strip_image_ext("com.obsproject.Studio"), "com.obsproject.Studio");
        assert_eq!(strip_image_ext("com.obsproject.Studio.svg"), "com.obsproject.Studio");
        assert_eq!(strip_image_ext("org.localsend.localsend_app.png"), "org.localsend.localsend_app");
    }

    #[test]
    fn parses_desktop_actions_and_mime_open_with() {
        let chrome = parse_desktop_str(
            "[Desktop Entry]\nType=Application\nName=Google Chrome\nExec=/usr/bin/google-chrome-stable %U\nIcon=google-chrome\nMimeType=text/html;image/png;\nActions=new-window;new-private-window;\n\n[Desktop Action new-window]\nName=New Window\nExec=/usr/bin/google-chrome-stable\n\n[Desktop Action new-private-window]\nName=New Incognito Window\nExec=/usr/bin/google-chrome-stable --incognito\n",
            "google-chrome",
        )
        .expect("parse chrome");
        assert_eq!(chrome.actions.len(), 2);
        assert_eq!(chrome.actions[0].name, "New Window");
        assert_eq!(chrome.actions[1].name, "New Incognito Window");
        assert_eq!(chrome.actions[1].exec_args, vec!["--incognito"]);

        let apps_slice = [chrome];
        let matches = find_apps_for_file(Path::new("/tmp/photo.png"), &apps_slice);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].id, "google-chrome");
    }
}
