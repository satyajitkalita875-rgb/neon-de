//! Real Linux filesystem operations, Freedesktop Trash/Clipboard integration, archive
//! compression/extraction, and macOS-styled modal dialogs (Rename, New Folder, Delete Confirm,
//! and Get Info / Properties) shared across NEON DE context menus and the Desktop icons layer.

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::UNIX_EPOCH,
};

use gtk4::{
    Align, Application, ApplicationWindow, Box as GtkBox, Button, Entry, EventControllerKey,
    Label, Orientation, Separator, Switch, gdk, glib, prelude::*,
};
use gtk4_layer_shell::{KeyboardMode, Layer, LayerShell};
use tracing::{info, warn};

use crate::desktop_apps::{guess_mime_type, load_app_icon, parse_desktop_file};

/// Clipboard transfer operation mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileClipboardOp {
    Copy,
    Cut,
}

/// Shared file clipboard state synchronized with Wayland (`x-special/gnome-copied-files`).
#[derive(Debug, Clone, Default)]
pub struct FileClipboard {
    pub op: Option<FileClipboardOp>,
    pub paths: Vec<PathBuf>,
}

impl FileClipboard {
    pub fn set(&mut self, op: FileClipboardOp, paths: Vec<PathBuf>) {
        self.op = if paths.is_empty() { None } else { Some(op) };
        self.paths = paths;
        self.sync_to_wayland_clipboard();
    }

    pub fn has_valid_items(&self) -> bool {
        self.op.is_some() && self.paths.iter().any(|p| p.exists())
    }

    pub fn is_cut(&self) -> bool {
        self.op == Some(FileClipboardOp::Cut)
    }

    pub fn item_count(&self) -> usize {
        self.paths.iter().filter(|p| p.exists()).count()
    }

    fn sync_to_wayland_clipboard(&self) {
        let Some(op) = self.op else {
            return;
        };
        let header = match op {
            FileClipboardOp::Copy => "copy",
            FileClipboardOp::Cut => "cut",
        };
        let mut gnome_payload = String::from(header);
        for p in &self.paths {
            gnome_payload.push('\n');
            gnome_payload.push_str(&path_to_file_uri(p));
        }
        // Best-effort sync to Wayland clipboard so Nautilus/Thunar can paste too.
        if let Ok(mut child) = Command::new("wl-copy")
            .args(["-t", "x-special/gnome-copied-files"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            && let Some(mut stdin) = child.stdin.take()
        {
            use std::io::Write;
            let _ = stdin.write_all(gnome_payload.as_bytes());
        }
    }

    /// Executes the pending Copy or Cut transfer into `dest_dir`.
    pub fn paste_into(&mut self, dest_dir: &Path) -> Result<Vec<PathBuf>, String> {
        let Some(op) = self.op else {
            return Err("Clipboard is empty".into());
        };
        if !dest_dir.is_dir() {
            return Err(format!("Destination is not a directory: {}", dest_dir.display()));
        }
        let mut created = Vec::new();
        for src in self.paths.clone() {
            if !src.exists() {
                continue;
            }
            let Some(file_name) = src.file_name() else {
                continue;
            };
            let mut target = dest_dir.join(file_name);
            if target == src {
                if op == FileClipboardOp::Cut {
                    continue;
                }
                target = unique_copy_path(&src);
            } else if target.exists() {
                target = unique_target_in_dir(dest_dir, &file_name.to_string_lossy());
            }

            match op {
                FileClipboardOp::Copy => {
                    copy_recursive(&src, &target)?;
                    created.push(target);
                }
                FileClipboardOp::Cut => {
                    if fs::rename(&src, &target).is_err() {
                        copy_recursive(&src, &target)?;
                        if src.is_dir() {
                            fs::remove_dir_all(&src).map_err(|e| e.to_string())?;
                        } else {
                            fs::remove_file(&src).map_err(|e| e.to_string())?;
                        }
                    }
                    created.push(target);
                }
            }
        }
        if op == FileClipboardOp::Cut {
            self.op = None;
            self.paths.clear();
        }
        Ok(created)
    }
}

/// Supported archive formats for right-click Compress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveFormat {
    Zip,
    TarGz,
}

/// Metadata summary rendered in the macOS-style Properties / Get Info modal.
#[derive(Debug, Clone)]
pub struct FileStats {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub mime: String,
    pub size_bytes: u64,
    pub item_count: Option<usize>,
    pub mode: u32,
    pub modified_secs: Option<u64>,
}

/// Resolves the user's XDG Desktop directory (`$XDG_DESKTOP_DIR` or `$HOME/Desktop`).
pub fn desktop_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("XDG_DESKTOP_DIR")
        && !d.is_empty()
    {
        return PathBuf::from(d);
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join("Desktop");
    }
    PathBuf::from("/tmp/Desktop")
}

/// Opens a file, folder, or `.desktop` shortcut using its native handler.
pub fn open_path(path: &Path) {
    if path.is_file()
        && path.extension().and_then(|e| e.to_str()) == Some("desktop")
        && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
        && let Some(app) = parse_desktop_file(path, stem)
    {
        app.launch();
        return;
    }
    if Command::new("gio")
        .arg("open")
        .arg(path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .is_ok()
    {
        return;
    }
    let _ = Command::new("xdg-open")
        .arg(path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

/// Launches the user's installed terminal emulator inside `dir`.
pub fn open_terminal_in(dir: &Path) {
    let cwd = if dir.is_dir() {
        dir.to_path_buf()
    } else {
        dir.parent()
            .filter(|p| p.is_dir())
            .map(Path::to_path_buf)
            .unwrap_or_else(desktop_dir)
    };
    for bin in [
        "kitty",
        "alacritty",
        "foot",
        "wezterm",
        "weston-terminal",
        "gnome-terminal",
        "xterm",
    ] {
        if Command::new("which")
            .arg(bin)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
        {
            let mut cmd = Command::new(bin);
            cmd.current_dir(&cwd);
            let _ = cmd.spawn();
            return;
        }
    }
}

/// Copies plain text (e.g. file path, timestamp) to both GTK4 Wayland clipboard and `wl-copy`.
pub fn copy_text_to_clipboard(text: &str) {
    if let Some(display) = gdk::Display::default() {
        display.clipboard().set_text(text);
    }
    if let Ok(mut child) = Command::new("wl-copy")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        && let Some(mut stdin) = child.stdin.take()
    {
        use std::io::Write;
        let _ = stdin.write_all(text.as_bytes());
    }
}

/// Creates a uniquely named folder inside `parent`.
pub fn create_new_folder(parent: &Path, base_name: &str) -> Result<PathBuf, String> {
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let clean = sanitize_filename(base_name);
    let target = unique_target_in_dir(parent, &clean);
    fs::create_dir_all(&target).map_err(|e| e.to_string())?;
    info!(path = %target.display(), "Created new folder");
    Ok(target)
}

/// Creates a uniquely named file inside `parent` with optional template contents and `+x` bit.
#[allow(dead_code)]
pub fn create_new_file(
    parent: &Path,
    stem: &str,
    ext: &str,
    content: &str,
    executable: bool,
) -> Result<PathBuf, String> {
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let base = if ext.is_empty() {
        sanitize_filename(stem)
    } else {
        format!("{}.{ext}", sanitize_filename(stem))
    };
    let target = unique_target_in_dir(parent, &base);
    fs::write(&target, content).map_err(|e| e.to_string())?;
    if executable {
        let _ = fs::set_permissions(&target, fs::Permissions::from_mode(0o755));
    }
    info!(path = %target.display(), "Created new file");
    Ok(target)
}

/// Recursively duplicates a file or folder beside `src` with macOS/GNOME `(copy)` naming.
pub fn duplicate_path(src: &Path) -> Result<PathBuf, String> {
    let dst = unique_copy_path(src);
    copy_recursive(src, &dst)?;
    info!(src = %src.display(), dst = %dst.display(), "Duplicated item");
    Ok(dst)
}

/// Renames `src` within its parent directory after validating the new filename.
pub fn rename_path(src: &Path, new_name: &str) -> Result<PathBuf, String> {
    let trimmed = new_name.trim();
    if trimmed.is_empty() || trimmed == "." || trimmed == ".." || trimmed.contains('/') {
        return Err("Invalid filename".into());
    }
    let parent = src.parent().ok_or_else(|| "Missing parent directory".to_string())?;
    let dst = parent.join(trimmed);
    if dst == src {
        return Ok(dst);
    }
    if dst.exists() {
        return Err(format!("'{trimmed}' already exists"));
    }
    fs::rename(src, &dst).map_err(|e| e.to_string())?;
    info!(src = %src.display(), dst = %dst.display(), "Renamed item");
    Ok(dst)
}

/// Moves `path` safely to the Freedesktop Trash (`gio trash`, with manual Trash spec fallback).
pub fn move_to_trash(path: &Path) -> Result<(), String> {
    if Command::new("gio")
        .arg("trash")
        .arg(path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
    {
        return Ok(());
    }

    // Fallback to Freedesktop Trash spec (`~/.local/share/Trash/{files,info}`)
    let trash_root = std::env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("Trash");
    let files_dir = trash_root.join("files");
    let info_dir = trash_root.join("info");
    fs::create_dir_all(&files_dir).map_err(|e| e.to_string())?;
    fs::create_dir_all(&info_dir).map_err(|e| e.to_string())?;

    let fname = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "item".into());
    let dst = unique_target_in_dir(&files_dir, &fname);
    let final_name = dst
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or(fname);
    if fs::rename(path, &dst).is_err() {
        copy_recursive(path, &dst)?;
        delete_permanently(path)?;
    }
    let info_path = info_dir.join(format!("{final_name}.trashinfo"));
    let info_body = format!(
        "[Trash Info]\nPath={}\nDeletionDate=2026-01-01T00:00:00\n",
        path.display()
    );
    let _ = fs::write(info_path, info_body);
    Ok(())
}

/// Permanently removes a file or directory from disk.
pub fn delete_permanently(path: &Path) -> Result<(), String> {
    if path.is_dir() && !path.is_symlink() {
        fs::remove_dir_all(path).map_err(|e| e.to_string())
    } else {
        fs::remove_file(path).map_err(|e| e.to_string())
    }
}

/// Returns `true` if `path` is a recognized archive file eligible for right-click Extract.
pub fn is_archive(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    [
        ".zip", ".tar", ".tar.gz", ".tgz", ".tar.xz", ".txz", ".tar.bz2", ".tbz2", ".tar.zst",
        ".7z", ".rar",
    ]
    .iter()
    .any(|ext| name.ends_with(ext))
}

/// Compresses `src` into `.zip` or `.tar.gz` inside its parent directory.
pub fn compress_path(src: &Path, format: ArchiveFormat) -> Result<PathBuf, String> {
    let parent = src.parent().ok_or_else(|| "Missing parent directory".to_string())?;
    let name = src
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .ok_or_else(|| "Invalid source name".to_string())?;
    let stem = src
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| name.clone());

    match format {
        ArchiveFormat::TarGz => {
            let out = unique_target_in_dir(parent, &format!("{stem}.tar.gz"));
            let status = Command::new("tar")
                .args(["-czf"])
                .arg(&out)
                .arg("-C")
                .arg(parent)
                .arg(&name)
                .status()
                .map_err(|e| format!("tar failed: {e}"))?;
            if status.success() {
                Ok(out)
            } else {
                Err("tar compression failed".into())
            }
        }
        ArchiveFormat::Zip => {
            let out = unique_target_in_dir(parent, &format!("{stem}.zip"));
            let zip_ok = Command::new("zip")
                .current_dir(parent)
                .args(["-rq"])
                .arg(&out)
                .arg(&name)
                .status()
                .is_ok_and(|s| s.success());
            if zip_ok {
                return Ok(out);
            }
            // Reliable Python3 stdlib zipfile fallback when `zip` CLI isn't installed
            let py_status = Command::new("python3")
                .args([
                    "-c",
                    "import os, sys, zipfile\nsrc, out = sys.argv[1], sys.argv[2]\nbase = os.path.dirname(src)\nwith zipfile.ZipFile(out, 'w', zipfile.ZIP_DEFLATED) as zf:\n  if os.path.isdir(src):\n    for r, _, fs in os.walk(src):\n      for f in fs:\n        p = os.path.join(r, f)\n        zf.write(p, os.path.relpath(p, base))\n  else:\n    zf.write(src, os.path.basename(src))\n",
                ])
                .arg(src)
                .arg(&out)
                .status()
                .map_err(|e| format!("zip failed: {e}"))?;
            if py_status.success() {
                Ok(out)
            } else {
                Err("zip compression failed".into())
            }
        }
    }
}

/// Extracts a supported archive into its parent directory (or a fresh subfolder).
pub fn extract_archive(archive: &Path, into_subfolder: bool) -> Result<PathBuf, String> {
    let parent = archive
        .parent()
        .ok_or_else(|| "Missing parent directory".to_string())?;
    let name_l = archive
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let dest = if into_subfolder {
        let base_stem = strip_archive_ext(
            archive
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("Extracted"),
        );
        let dir = unique_target_in_dir(parent, base_stem);
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        dir
    } else {
        parent.to_path_buf()
    };

    if name_l.ends_with(".zip") {
        let ok = Command::new("unzip")
            .args(["-o"])
            .arg(archive)
            .arg("-d")
            .arg(&dest)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        if ok {
            return Ok(dest);
        }
        let py_ok = Command::new("python3")
            .args([
                "-c",
                "import sys, zipfile\nwith zipfile.ZipFile(sys.argv[1]) as zf: zf.extractall(sys.argv[2])\n",
            ])
            .arg(archive)
            .arg(&dest)
            .status()
            .is_ok_and(|s| s.success());
        return if py_ok {
            Ok(dest)
        } else {
            Err("ZIP extraction failed".into())
        };
    }

    if name_l.ends_with(".7z") {
        let ok = Command::new("7z")
            .arg("x")
            .arg("-y")
            .arg(format!("-o{}", dest.display()))
            .arg(archive)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        return if ok {
            Ok(dest)
        } else {
            Err("7z extraction failed".into())
        };
    }

    let ok = Command::new("tar")
        .arg("-xf")
        .arg(archive)
        .arg("-C")
        .arg(&dest)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if ok {
        Ok(dest)
    } else {
        Err("Archive extraction failed".into())
    }
}

/// Computes metadata stats for a file or folder (for the Get Info / Properties modal).
pub fn compute_path_stats(path: &Path) -> Result<FileStats, String> {
    let meta = fs::metadata(path).map_err(|e| e.to_string())?;
    let is_dir = meta.is_dir();
    let (size_bytes, item_count) = if is_dir {
        let mut total = 0u64;
        let mut count = 0usize;
        if let Ok(rd) = fs::read_dir(path) {
            for e in rd.flatten() {
                count += 1;
                if let Ok(m) = e.metadata() {
                    total = total.saturating_add(m.len());
                }
            }
        }
        (total, Some(count))
    } else {
        (meta.len(), None)
    };
    let modified_secs = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs());
    Ok(FileStats {
        name: path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string()),
        path: path.to_path_buf(),
        is_dir,
        mime: guess_mime_type(path),
        size_bytes,
        item_count,
        mode: meta.permissions().mode(),
        modified_secs,
    })
}

pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut val = bytes as f64;
    let mut idx = 0usize;
    while val >= 1024.0 && idx < UNITS.len() - 1 {
        val /= 1024.0;
        idx += 1;
    }
    if idx == 0 {
        format!("{bytes} B")
    } else {
        format!("{val:.1} {}", UNITS[idx])
    }
}

pub fn format_unix_mode(mode: u32) -> String {
    let bits = mode & 0o777;
    let mut s = String::with_capacity(15);
    for shift in [6, 3, 0] {
        let oct = (bits >> shift) & 7;
        s.push(if oct & 4 != 0 { 'r' } else { '-' });
        s.push(if oct & 2 != 0 { 'w' } else { '-' });
        s.push(if oct & 1 != 0 { 'x' } else { '-' });
    }
    format!("{s} ({bits:03o})")
}

/// Resolves a clean display icon name for any file or folder on Desktop/Properties.
pub fn icon_for_path(path: &Path) -> String {
    if path.is_dir() {
        return "folder".into();
    }
    if path.extension().and_then(|e| e.to_str()) == Some("desktop")
        && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
        && let Some(app) = parse_desktop_file(path, stem)
        && let Some(ic) = app.icon
    {
        return ic;
    }
    let mime = guess_mime_type(path);
    if mime.starts_with("image/") {
        "image-x-generic".into()
    } else if mime.starts_with("video/") {
        "video-x-generic".into()
    } else if mime.starts_with("audio/") {
        "audio-x-generic".into()
    } else if mime == "application/pdf" {
        "application-pdf".into()
    } else if is_archive(path) {
        "package-x-generic".into()
    } else if mime == "application/x-shellscript" {
        "text-x-script".into()
    } else if mime.starts_with("text/") {
        "text-x-generic".into()
    } else {
        "application-octet-stream".into()
    }
}

// ── macOS-Styled Frosted Modal Dialogs ───────────────────────────────────────

fn make_modal_window(app: &Application, title: &str, width: i32) -> ApplicationWindow {
    let win = ApplicationWindow::builder()
        .application(app)
        .title(title)
        .decorated(false)
        .resizable(false)
        .default_width(width)
        .build();
    win.add_css_class("neon-dialog-window");
    if gtk4_layer_shell::is_supported() {
        win.init_layer_shell();
        win.set_namespace(Some("neon-dialog"));
        win.set_layer(Layer::Overlay);
        win.set_exclusive_zone(-1);
        win.set_keyboard_mode(KeyboardMode::Exclusive);
    }
    let esc = EventControllerKey::new();
    {
        let w = win.clone();
        esc.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Escape {
                w.close();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
    }
    win.add_controller(esc);
    win
}

/// Opens a clean macOS-styled inline Rename dialog for `path`.
pub fn show_rename_dialog(app: &Application, path: PathBuf, on_done: Box<dyn Fn()>) {
    let win = make_modal_window(app, "Rename", 360);
    let card = GtkBox::new(Orientation::Vertical, 12);
    card.add_css_class("neon-dialog-card");

    let title = Label::new(Some("Rename Item"));
    title.add_css_class("neon-dialog-title");
    title.set_halign(Align::Start);
    card.append(&title);

    let orig_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let entry = Entry::new();
    entry.add_css_class("neon-dialog-entry");
    entry.set_text(&orig_name);
    card.append(&entry);

    let err_lbl = Label::new(None);
    err_lbl.add_css_class("neon-dialog-error");
    err_lbl.set_halign(Align::Start);
    err_lbl.set_visible(false);
    card.append(&err_lbl);

    let actions = GtkBox::new(Orientation::Horizontal, 8);
    actions.set_halign(Align::End);
    let cancel = Button::with_label("Cancel");
    cancel.add_css_class("neon-dialog-btn");
    let ok = Button::with_label("Rename");
    ok.add_css_class("neon-dialog-btn");
    ok.add_css_class("primary");
    actions.append(&cancel);
    actions.append(&ok);
    card.append(&actions);

    let on_done = std::rc::Rc::new(on_done);
    let submit = {
        let w = win.clone();
        let e = entry.clone();
        let err = err_lbl.clone();
        let p = path.clone();
        let cb = std::rc::Rc::clone(&on_done);
        move || {
            let next = e.text().to_string();
            match rename_path(&p, &next) {
                Ok(_) => {
                    cb();
                    w.close();
                }
                Err(msg) => {
                    err.set_text(&msg);
                    err.set_visible(true);
                }
            }
        }
    };
    {
        let w = win.clone();
        cancel.connect_clicked(move |_| w.close());
    }
    {
        let s = submit.clone();
        ok.connect_clicked(move |_| s());
    }
    {
        let s = submit;
        entry.connect_activate(move |_| s());
    }

    win.set_child(Some(&card));
    win.present();
    entry.grab_focus();
    // Select filename stem before extension just like macOS Finder
    if let Some(dot) = orig_name.rfind('.')
        && dot > 0
        && !path.is_dir()
    {
        entry.select_region(0, dot as i32);
    } else {
        entry.select_region(0, -1);
    }
}

/// Opens a clean macOS-styled New Folder prompt inside `parent`.
pub fn show_new_folder_dialog(app: &Application, parent: PathBuf, on_done: Box<dyn Fn()>) {
    let win = make_modal_window(app, "New Folder", 360);
    let card = GtkBox::new(Orientation::Vertical, 12);
    card.add_css_class("neon-dialog-card");

    let title = Label::new(Some("New Folder"));
    title.add_css_class("neon-dialog-title");
    title.set_halign(Align::Start);
    card.append(&title);

    let entry = Entry::new();
    entry.add_css_class("neon-dialog-entry");
    entry.set_text("Untitled Folder");
    card.append(&entry);

    let err_lbl = Label::new(None);
    err_lbl.add_css_class("neon-dialog-error");
    err_lbl.set_halign(Align::Start);
    err_lbl.set_visible(false);
    card.append(&err_lbl);

    let actions = GtkBox::new(Orientation::Horizontal, 8);
    actions.set_halign(Align::End);
    let cancel = Button::with_label("Cancel");
    cancel.add_css_class("neon-dialog-btn");
    let ok = Button::with_label("Create");
    ok.add_css_class("neon-dialog-btn");
    ok.add_css_class("primary");
    actions.append(&cancel);
    actions.append(&ok);
    card.append(&actions);

    let on_done = std::rc::Rc::new(on_done);
    let submit = {
        let w = win.clone();
        let e = entry.clone();
        let err = err_lbl.clone();
        let p = parent.clone();
        let cb = std::rc::Rc::clone(&on_done);
        move || {
            let name = e.text().trim().to_string();
            let chosen = if name.is_empty() {
                "Untitled Folder".to_string()
            } else {
                name
            };
            match create_new_folder(&p, &chosen) {
                Ok(_) => {
                    cb();
                    w.close();
                }
                Err(msg) => {
                    err.set_text(&msg);
                    err.set_visible(true);
                }
            }
        }
    };
    {
        let w = win.clone();
        cancel.connect_clicked(move |_| w.close());
    }
    {
        let s = submit.clone();
        ok.connect_clicked(move |_| s());
    }
    {
        let s = submit;
        entry.connect_activate(move |_| s());
    }

    win.set_child(Some(&card));
    win.present();
    entry.grab_focus();
    entry.select_region(0, -1);
}

/// Opens a confirmation dialog before permanently deleting `path`.
pub fn show_delete_confirm_dialog(app: &Application, path: PathBuf, on_done: Box<dyn Fn()>) {
    let win = make_modal_window(app, "Delete Permanently", 360);
    let card = GtkBox::new(Orientation::Vertical, 12);
    card.add_css_class("neon-dialog-card");

    let fname = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    let title = Label::new(Some(&format!("Delete \"{fname}\" permanently?")));
    title.add_css_class("neon-dialog-title");
    title.set_halign(Align::Start);
    title.set_wrap(true);
    card.append(&title);

    let sub = Label::new(Some("This item will be erased immediately and cannot be undone."));
    sub.add_css_class("neon-dialog-sub");
    sub.set_halign(Align::Start);
    sub.set_wrap(true);
    card.append(&sub);

    let actions = GtkBox::new(Orientation::Horizontal, 8);
    actions.set_halign(Align::End);
    let cancel = Button::with_label("Cancel");
    cancel.add_css_class("neon-dialog-btn");
    let del = Button::with_label("Delete");
    del.add_css_class("neon-dialog-btn");
    del.add_css_class("destructive");
    actions.append(&cancel);
    actions.append(&del);
    card.append(&actions);

    {
        let w = win.clone();
        cancel.connect_clicked(move |_| w.close());
    }
    {
        let w = win.clone();
        del.connect_clicked(move |_| {
            if let Err(err) = delete_permanently(&path) {
                warn!(%err, "Permanent delete failed");
            }
            on_done();
            w.close();
        });
    }

    win.set_child(Some(&card));
    win.present();
}

/// Opens a sleek macOS Finder "Get Info" / Properties modal for `path`.
pub fn show_properties_dialog(app: &Application, path: PathBuf, on_done: Box<dyn Fn()>) {
    let Ok(stats) = compute_path_stats(&path) else {
        return;
    };
    let win = make_modal_window(app, &format!("{} — Info", stats.name), 390);
    let card = GtkBox::new(Orientation::Vertical, 12);
    card.add_css_class("neon-dialog-card");

    // Header row: large crisp icon + filename + kind
    let hdr = GtkBox::new(Orientation::Horizontal, 14);
    hdr.set_valign(Align::Center);
    let ic_name = icon_for_path(&path);
    let icon = load_app_icon(Some(&ic_name), &stats.name, 48);
    hdr.append(&icon);

    let hdr_text = GtkBox::new(Orientation::Vertical, 2);
    hdr_text.set_hexpand(true);
    let name_lbl = Label::new(Some(&stats.name));
    name_lbl.add_css_class("neon-dialog-title");
    name_lbl.set_halign(Align::Start);
    name_lbl.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    let kind_str = if stats.is_dir {
        "Folder".to_string()
    } else {
        stats.mime.clone()
    };
    let kind_lbl = Label::new(Some(&kind_str));
    kind_lbl.add_css_class("neon-dialog-sub");
    kind_lbl.set_halign(Align::Start);
    hdr_text.append(&name_lbl);
    hdr_text.append(&kind_lbl);
    hdr.append(&hdr_text);
    card.append(&hdr);

    let sep1 = Separator::new(Orientation::Horizontal);
    sep1.add_css_class("neon-ctx-sep");
    card.append(&sep1);

    let add_row = |k: &str, v: &str| {
        let r = GtkBox::new(Orientation::Horizontal, 10);
        let kl = Label::new(Some(k));
        kl.add_css_class("neon-dialog-key");
        kl.set_halign(Align::Start);
        kl.set_width_chars(12);
        let vl = Label::new(Some(v));
        vl.add_css_class("neon-dialog-val");
        vl.set_halign(Align::Start);
        vl.set_hexpand(true);
        vl.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);
        vl.set_selectable(true);
        r.append(&kl);
        r.append(&vl);
        card.append(&r);
    };

    let size_text = if let Some(n) = stats.item_count {
        let noun = if n == 1 { "item" } else { "items" };
        format!("{} ({n} {noun})", format_size(stats.size_bytes))
    } else {
        format!("{} ({} bytes)", format_size(stats.size_bytes), stats.size_bytes)
    };
    add_row("Size", &size_text);
    add_row(
        "Location",
        &stats
            .path
            .parent()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "/".into()),
    );
    add_row("Permissions", &format_unix_mode(stats.mode));
    if let Some(secs) = stats.modified_secs {
        let ts = format_epoch_local(secs);
        add_row("Modified", &ts);
    }

    if !stats.is_dir {
        let sep2 = Separator::new(Orientation::Horizontal);
        sep2.add_css_class("neon-ctx-sep");
        card.append(&sep2);

        let exec_row = GtkBox::new(Orientation::Horizontal, 10);
        let exec_lbl = Label::new(Some("Allow executing as program"));
        exec_lbl.add_css_class("neon-dialog-val");
        exec_lbl.set_halign(Align::Start);
        exec_lbl.set_hexpand(true);
        let sw = Switch::new();
        sw.set_active((stats.mode & 0o111) != 0);
        sw.set_valign(Align::Center);
        {
            let p = path.clone();
            let orig_mode = stats.mode;
            sw.connect_state_set(move |_, active| {
                let next_mode = if active {
                    orig_mode | 0o111
                } else {
                    orig_mode & !0o111
                };
                let _ = fs::set_permissions(&p, fs::Permissions::from_mode(next_mode));
                glib::Propagation::Proceed
            });
        }
        exec_row.append(&exec_lbl);
        exec_row.append(&sw);
        card.append(&exec_row);
    }

    let footer = GtkBox::new(Orientation::Horizontal, 8);
    footer.set_halign(Align::End);
    footer.set_margin_top(6);
    let copy_btn = Button::with_label("Copy Path");
    copy_btn.add_css_class("neon-dialog-btn");
    {
        let p_str = stats.path.to_string_lossy().into_owned();
        copy_btn.connect_clicked(move |_| copy_text_to_clipboard(&p_str));
    }
    let close_btn = Button::with_label("Done");
    close_btn.add_css_class("neon-dialog-btn");
    close_btn.add_css_class("primary");
    {
        let w = win.clone();
        close_btn.connect_clicked(move |_| {
            on_done();
            w.close();
        });
    }
    footer.append(&copy_btn);
    footer.append(&close_btn);
    card.append(&footer);

    win.set_child(Some(&card));
    win.present();
}

// ── Internal filesystem helpers ──────────────────────────────────────────────

fn path_to_file_uri(path: &Path) -> String {
    let raw = path.to_string_lossy();
    let mut encoded = String::from("file://");
    for b in raw.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(b as char);
            }
            other => {
                encoded.push_str(&format!("%{other:02X}"));
            }
        }
    }
    encoded
}

fn sanitize_filename(name: &str) -> String {
    let cleaned: String = name
        .trim()
        .chars()
        .filter(|&c| c != '/' && c != '\0')
        .collect();
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        "Untitled".into()
    } else {
        cleaned
    }
}

fn unique_target_in_dir(dir: &Path, filename: &str) -> PathBuf {
    let candidate = dir.join(filename);
    if !candidate.exists() {
        return candidate;
    }
    let (stem, ext) = split_stem_ext(filename);
    let mut idx = 2u32;
    loop {
        let next = if ext.is_empty() {
            format!("{stem} {idx}")
        } else {
            format!("{stem} {idx}.{ext}")
        };
        let p = dir.join(next);
        if !p.exists() {
            return p;
        }
        idx += 1;
    }
}

fn unique_copy_path(src: &Path) -> PathBuf {
    let parent = src.parent().unwrap_or_else(|| Path::new("."));
    let name = src
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Item".into());
    let (stem, ext) = if src.is_dir() {
        (name.as_str(), "")
    } else {
        split_stem_ext(&name)
    };
    let first = if ext.is_empty() {
        format!("{stem} (copy)")
    } else {
        format!("{stem} (copy).{ext}")
    };
    let p1 = parent.join(first);
    if !p1.exists() {
        return p1;
    }
    let mut idx = 2u32;
    loop {
        let next = if ext.is_empty() {
            format!("{stem} (copy {idx})")
        } else {
            format!("{stem} (copy {idx}).{ext}")
        };
        let p = parent.join(next);
        if !p.exists() {
            return p;
        }
        idx += 1;
    }
}

fn split_stem_ext(filename: &str) -> (&str, &str) {
    if let Some(dot) = filename.rfind('.')
        && dot > 0
        && dot + 1 < filename.len()
    {
        (&filename[..dot], &filename[dot + 1..])
    } else {
        (filename, "")
    }
}

fn strip_archive_ext(filename: &str) -> &str {
    let lower = filename.to_lowercase();
    for suffix in [".tar.gz", ".tar.xz", ".tar.bz2", ".tar.zst"] {
        if lower.ends_with(suffix) && filename.len() > suffix.len() {
            return &filename[..filename.len() - suffix.len()];
        }
    }
    split_stem_ext(filename).0
}

fn copy_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    let meta = fs::symlink_metadata(src).map_err(|e| e.to_string())?;
    if meta.file_type().is_symlink() {
        let target = fs::read_link(src).map_err(|e| e.to_string())?;
        std::os::unix::fs::symlink(target, dst).map_err(|e| e.to_string())?;
        return Ok(());
    }
    if meta.is_dir() {
        fs::create_dir_all(dst).map_err(|e| e.to_string())?;
        for entry in fs::read_dir(src).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            copy_recursive(&entry.path(), &dst.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        fs::copy(src, dst).map_err(|e| e.to_string())?;
        let _ = fs::set_permissions(dst, meta.permissions());
        Ok(())
    }
}

fn format_epoch_local(secs: u64) -> String {
    if let Ok(dt) = glib::DateTime::from_unix_local(secs as i64)
        && let Ok(formatted) = dt.format("%b %d, %Y %H:%M")
    {
        return formatted.to_string();
    }
    format!("{secs}s")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_ops_create_duplicate_rename_compress_extract_and_paste() {
        let base = std::env::temp_dir().join(format!("neon-fileops-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();

        let f1 = create_new_file(&base, "Notes", "txt", "hello neon", false).unwrap();
        assert_eq!(f1.file_name().unwrap(), "Notes.txt");

        let dup = duplicate_path(&f1).unwrap();
        assert_eq!(dup.file_name().unwrap(), "Notes (copy).txt");
        assert_eq!(fs::read_to_string(&dup).unwrap(), "hello neon");

        let renamed = rename_path(&dup, "Renamed.txt").unwrap();
        assert_eq!(renamed.file_name().unwrap(), "Renamed.txt");

        let folder = create_new_folder(&base, "Projects").unwrap();
        let mut clip = FileClipboard::default();
        clip.set(FileClipboardOp::Copy, vec![renamed.clone()]);
        assert!(clip.has_valid_items());
        let pasted = clip.paste_into(&folder).unwrap();
        assert_eq!(pasted.len(), 1);
        assert!(folder.join("Renamed.txt").is_file());

        let tgz = compress_path(&folder, ArchiveFormat::TarGz).unwrap();
        assert!(is_archive(&tgz));
        let extracted = extract_archive(&tgz, true).unwrap();
        assert!(extracted.join("Projects/Renamed.txt").is_file());

        let stats = compute_path_stats(&folder).unwrap();
        assert!(stats.is_dir);
        assert_eq!(stats.item_count, Some(1));

        let _ = fs::remove_dir_all(&base);
    }
}
