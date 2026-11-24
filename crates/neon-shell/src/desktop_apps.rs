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
    pub categories: Vec<String>,
    pub terminal: bool,
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
    pub fn launch(&self) {
        let mut cmd = Command::new(&self.exec_program);
        cmd.args(&self.exec_args);
        if let Some(ref dir) = self.working_dir {
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
                    app = %self.name,
                    program = %self.exec_program,
                    "Launched XDG application from NEON Shell"
                );
            }
            Err(err) => {
                warn!(
                    app = %self.name,
                    program = %self.exec_program,
                    ?err,
                    "Failed to launch XDG application"
                );
            }
        }
    }
}

/// Scans standard XDG application directories and returns deduplicated, sorted [`DesktopApp`]s.
pub fn discover_applications() -> Vec<DesktopApp> {
    let dirs = xdg_application_dirs();
    discover_in_dirs(&dirs)
}

/// Loads an application icon cleanly for Dock or Launchpad with zero missing-icon symbols:
/// 1. Supports direct filesystem paths (`/snap/...`, `/opt/...`, `/usr/share/pixmaps/...`)
/// 2. Checks the active GTK4 IconTheme (`WhiteSur-dark` / `WhiteSur-light`) with smart Snap/Flatpak/name normalization
/// 3. Falls back cleanly to a valid WhiteSur generic app icon (`application-x-executable`).
pub fn load_app_icon(icon_spec: Option<&str>, app_name: &str, pixel_size: i32) -> Image {
    let icon_theme = Display::default().map(|d| IconTheme::for_display(&d));

    if let Some(raw) = icon_spec.map(str::trim).filter(|s| !s.is_empty()) {
        // 1. Absolute filesystem path (e.g. Snap / DaVinci Resolve / AppImage / pixmaps)
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

            // Try stripped stem (e.g. `foo.png` -> `foo`, `firefox_firefox` -> `firefox`, `org.foo.Bar` -> `bar`)
            let stem = Path::new(raw)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(raw);
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

        // 3. Check /usr/share/pixmaps fallback
        for ext in ["", ".png", ".svg", ".xpm"] {
            let pix = PathBuf::from(format!("/usr/share/pixmaps/{raw}{ext}"));
            if pix.exists() {
                let img = Image::from_file(pix);
                img.set_pixel_size(pixel_size);
                return img;
            }
        }
    }

    // 4. Smart lookup by normalized application name against WhiteSur icon theme
    if let Some(ref theme) = icon_theme {
        let lower = app_name.to_lowercase();
        let slug = lower
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect::<String>()
            .trim_matches('-')
            .to_string();
        let first_word = lower.split_whitespace().next().unwrap_or("").to_string();

        let mut name_candidates = vec![slug.clone(), first_word];
        if lower.contains("firefox") {
            name_candidates.push("firefox".into());
        } else if lower.contains("davinci") || lower.contains("resolve") {
            name_candidates.push("davinci-resolve".into());
            name_candidates.push("applications-multimedia".into());
        } else if lower.contains("app center") || lower.contains("software") {
            name_candidates.push("system-software-install".into());
        } else if lower.contains("firmware") {
            name_candidates.push("system-software-update".into());
        } else if lower.contains("instagram") {
            name_candidates.push("instagram".into());
            name_candidates.push("web-browser".into());
        } else if lower.contains("settings") {
            name_candidates.push("preferences-system".into());
        } else if lower.contains("terminal") {
            name_candidates.push("utilities-terminal".into());
        }

        for cand in name_candidates {
            if !cand.is_empty() && theme.has_icon(&cand) {
                let img = Image::from_icon_name(&cand);
                img.set_pixel_size(pixel_size);
                return img;
            }
        }
    }

    // 5. Guaranteed valid WhiteSur fallback icon (never shows red missing-icon glyph)
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
    if let Some(data_home) = std::env::var_os("XDG_DATA_HOME") {
        dirs.push(PathBuf::from(data_home).join("applications"));
    } else if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join(".local/share/applications"));
    }
    if let Some(data_dirs) = std::env::var_os("XDG_DATA_DIRS") {
        for part in std::env::split_paths(&data_dirs) {
            dirs.push(part.join("applications"));
        }
    } else {
        dirs.push(PathBuf::from("/usr/local/share/applications"));
        dirs.push(PathBuf::from("/usr/share/applications"));
    }
    dirs
}

/// Parses a single XDG `.desktop` file, returning `None` if hidden, `NoDisplay`, or non-Application.
pub fn parse_desktop_file(path: &Path, id: &str) -> Option<DesktopApp> {
    let content = fs::read_to_string(path).ok()?;
    parse_desktop_str(&content, id)
}

/// Parses XDG Desktop Entry INI text from `[Desktop Entry]`.
pub fn parse_desktop_str(content: &str, id: &str) -> Option<DesktopApp> {
    let mut in_desktop_entry = false;
    let mut entry_type = None;
    let mut name = None;
    let mut generic_name = None;
    let mut comment = None;
    let mut exec_raw = None;
    let mut working_dir = None;
    let mut icon = None;
    let mut categories = Vec::new();
    let mut no_display = false;
    let mut hidden = false;
    let mut terminal = false;

    for raw_line in content.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            in_desktop_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_desktop_entry {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        match key {
            "Type" => entry_type = Some(value.to_string()),
            "Name" => name = Some(value.to_string()),
            "GenericName" => generic_name = Some(value.to_string()),
            "Comment" => comment = Some(value.to_string()),
            "Exec" => exec_raw = Some(value.to_string()),
            "Path" => working_dir = Some(value.to_string()),
            "Icon" => icon = Some(value.to_string()),
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
            _ => {}
        }
    }

    if no_display || hidden || entry_type.as_deref() != Some("Application") {
        return None;
    }

    let name = name?;
    let (exec_program, exec_args) = parse_exec_command(exec_raw.as_deref()?)?;

    Some(DesktopApp {
        id: id.to_string(),
        name,
        generic_name,
        comment,
        exec_program,
        exec_args,
        working_dir,
        icon,
        categories,
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
                    categories: vec!["System".into(), "TerminalEmulator".into()],
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
}
