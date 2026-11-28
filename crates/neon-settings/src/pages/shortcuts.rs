//! Keyboard Shortcuts reference: clean, searchable listing of the exact active
//! shortcuts implemented in `neon-compositor`.

use std::rc::Rc;

use gtk4::{Align, Box as GtkBox, Label, Orientation, ScrolledWindow, SearchEntry, prelude::*};

use crate::{
    state::SettingsController,
    ui::{append_rows, boxed_card, info_row, page_shell, section_heading, setting_row},
};

struct ShortcutEntry {
    group: &'static str,
    action: &'static str,
    keys: &'static str,
}

const ACTIVE_SHORTCUTS: &[ShortcutEntry] = &[
    // Desktop & Launcher
    ShortcutEntry {
        group: "Desktop & Launcher",
        action: "Open / Close App Drawer",
        keys: "Super + Space  /  Super + A",
    },
    ShortcutEntry {
        group: "Desktop & Launcher",
        action: "Open Terminal",
        keys: "Super + Return  /  Ctrl + Alt + T",
    },
    ShortcutEntry {
        group: "Desktop & Launcher",
        action: "Lock Screen",
        keys: "Super + L",
    },
    ShortcutEntry {
        group: "Desktop & Launcher",
        action: "Open / Close Dynamic Island",
        keys: "Super + N",
    },
    ShortcutEntry {
        group: "Desktop & Launcher",
        action: "Open Clipboard History (Dynamic Island)",
        keys: "Super + V",
    },
    ShortcutEntry {
        group: "Desktop & Launcher",
        action: "Interactive Screenshot",
        keys: "Print  /  Super + Shift + S  /  Super + Shift + 4",
    },
    ShortcutEntry {
        group: "Desktop & Launcher",
        action: "Instant Fullscreen Screenshot",
        keys: "Super + Shift + 3",
    },
    // Window Management
    ShortcutEntry {
        group: "Windows",
        action: "Switch Windows",
        keys: "Alt + Tab",
    },
    ShortcutEntry {
        group: "Windows",
        action: "Close Focused Window",
        keys: "Super + Q",
    },
    ShortcutEntry {
        group: "Windows",
        action: "Minimize Focused Window",
        keys: "Super + H",
    },
    ShortcutEntry {
        group: "Windows",
        action: "Restore Last Minimized Window",
        keys: "Super + Shift + H",
    },
    ShortcutEntry {
        group: "Windows",
        action: "Maximize / Restore Window",
        keys: "Super + Up  /  Super + Down",
    },
    ShortcutEntry {
        group: "Windows",
        action: "Snap Window Left / Right",
        keys: "Super + Left  /  Super + Right",
    },
    ShortcutEntry {
        group: "Windows",
        action: "Toggle Floating Window",
        keys: "Super + Shift + F",
    },
    // Workspaces
    ShortcutEntry {
        group: "Workspaces",
        action: "Switch to Workspace 1–10",
        keys: "Super + 1…0  /  Super + Numpad 1…0",
    },
    ShortcutEntry {
        group: "Workspaces",
        action: "Next / Previous Workspace",
        keys: "Ctrl + Alt + Right/Left  /  Super + PgDn/PgUp",
    },
    ShortcutEntry {
        group: "Workspaces",
        action: "Move Window to Next / Previous Workspace",
        keys: "Ctrl + Alt + Shift + Right/Left",
    },
    // System
    ShortcutEntry {
        group: "System",
        action: "Switch Virtual Terminal (VT 1–12)",
        keys: "Ctrl + Alt + F1…F12",
    },
    ShortcutEntry {
        group: "System",
        action: "Exit NEON DE Session",
        keys: "Ctrl + Alt + Escape",
    },
];

pub fn build_page(_ctrl: Rc<SettingsController>) -> ScrolledWindow {
    let (scroller, page) = page_shell();

    let search = SearchEntry::new();
    search.add_css_class("settings-search");
    search.set_placeholder_text(Some("Filter shortcuts…"));
    page.append(&search);

    let list_root = GtkBox::new(Orientation::Vertical, 2);
    page.append(&list_root);

    let render = {
        let root_c = list_root.clone();
        Rc::new(move |query: &str| {
            while let Some(child) = root_c.first_child() {
                root_c.remove(&child);
            }
            let q = query.trim().to_ascii_lowercase();
            let mut groups: Vec<&'static str> = Vec::new();
            for s in ACTIVE_SHORTCUTS {
                if !groups.contains(&s.group) {
                    groups.push(s.group);
                }
            }

            let mut any = false;
            for grp in groups {
                let items: Vec<&ShortcutEntry> = ACTIVE_SHORTCUTS
                    .iter()
                    .filter(|s| {
                        s.group == grp
                            && (q.is_empty()
                                || s.action.to_ascii_lowercase().contains(&q)
                                || s.keys.to_ascii_lowercase().contains(&q)
                                || s.group.to_ascii_lowercase().contains(&q))
                    })
                    .collect();
                if items.is_empty() {
                    continue;
                }
                any = true;
                root_c.append(&section_heading(grp));
                let card = boxed_card();
                let mut rows = Vec::new();
                for item in items {
                    let pill = Label::new(Some(item.keys));
                    pill.add_css_class("kbd-pill");
                    pill.set_valign(Align::Center);
                    rows.push(setting_row(item.action, "", &pill));
                }
                append_rows(&card, rows);
                root_c.append(&card);
            }

            if !any {
                let card = boxed_card();
                card.append(&info_row("No Matching Shortcuts", "", "—"));
                root_c.append(&card);
            }
        })
    };

    render("");
    {
        let r = Rc::clone(&render);
        search.connect_search_changed(move |e| r(&e.text()));
    }

    scroller
}
