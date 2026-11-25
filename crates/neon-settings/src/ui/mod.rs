//! Reusable macOS-style GTK4 UI building blocks, async task helper,
//! debounced sliders, confirmation modals, and display revert countdown dialog.

pub mod style;

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::mpsc,
    time::Duration,
};

use gtk4::{
    Align, ApplicationWindow, Box as GtkBox, Button, DropDown, Label, Orientation, PolicyType,
    Scale, ScrolledWindow, Separator, StringList, Switch, Window, glib, prelude::*,
};

use crate::state::SettingsController;

/// Creates a clean macOS-style scrollable settings column WITHOUT duplicating the header bar title.
pub fn page_shell() -> (ScrolledWindow, GtkBox) {
    let outer = GtkBox::new(Orientation::Vertical, 0);
    outer.set_halign(Align::Fill);
    outer.set_hexpand(true);

    let column = GtkBox::new(Orientation::Vertical, 2);
    column.add_css_class("settings-content-clamp");
    column.set_hexpand(true);
    outer.append(&column);

    let scroller = ScrolledWindow::builder()
        .hscrollbar_policy(PolicyType::Never)
        .vscrollbar_policy(PolicyType::Automatic)
        .hexpand(true)
        .vexpand(true)
        .child(&outer)
        .build();
    (scroller, column)
}

/// Optional macOS-style bottom-right "Reset Defaults…" footer button for configurable pages.
pub fn page_reset_footer(
    category_id: &'static str,
    category_title: &str,
    ctrl: &Rc<SettingsController>,
) -> GtkBox {
    let row = GtkBox::new(Orientation::Horizontal, 8);
    row.set_halign(Align::End);
    row.set_margin_top(6);

    let reset_btn = Button::with_label("Reset Defaults…");
    reset_btn.add_css_class("settings-secondary-btn");
    let ctrl_c = Rc::clone(ctrl);
    let title_owned = category_title.to_string();
    reset_btn.connect_clicked(move |_| {
        let ctrl_inner = Rc::clone(&ctrl_c);
        confirm_action_dialog(
            &ctrl_c.window,
            &format!("Reset {title_owned}?"),
            "This restores all settings on this page to NEON DE defaults.",
            "Reset",
            true,
            move || {
                ctrl_inner.reset_category(category_id);
            },
        );
    });
    row.append(&reset_btn);
    row
}

pub fn section_heading(title: &str) -> Label {
    let lbl = Label::new(Some(title));
    lbl.add_css_class("settings-section-title");
    lbl.set_halign(Align::Start);
    lbl
}

pub fn boxed_card() -> GtkBox {
    let card = GtkBox::new(Orientation::Vertical, 0);
    card.add_css_class("settings-card");
    card
}

pub fn row_separator() -> Separator {
    let sep = Separator::new(Orientation::Horizontal);
    sep.add_css_class("settings-row-sep");
    sep
}

pub fn append_rows(card: &GtkBox, rows: Vec<GtkBox>) {
    for (idx, row) in rows.into_iter().enumerate() {
        if idx > 0 {
            card.append(&row_separator());
        }
        card.append(&row);
    }
}

pub fn setting_row(title: &str, subtitle: &str, control: &impl IsA<gtk4::Widget>) -> GtkBox {
    let row = GtkBox::new(Orientation::Horizontal, 14);
    row.add_css_class("settings-row");
    row.set_valign(Align::Center);

    let text = GtkBox::new(Orientation::Vertical, 1);
    text.set_hexpand(true);
    text.set_valign(Align::Center);

    let t = Label::new(Some(title));
    t.add_css_class("settings-row-title");
    t.set_halign(Align::Start);
    t.set_wrap(true);
    text.append(&t);

    if !subtitle.is_empty() {
        let s = Label::new(Some(subtitle));
        s.add_css_class("settings-row-desc");
        s.set_halign(Align::Start);
        s.set_wrap(true);
        text.append(&s);
    }

    row.append(&text);
    row.append(control);
    row
}

pub fn status_badge(text: &str, variant: &str) -> Label {
    let badge = Label::new(Some(text));
    badge.add_css_class("settings-badge");
    if !variant.is_empty() {
        badge.add_css_class(variant);
    }
    badge.set_valign(Align::Center);
    badge
}

pub fn info_row(title: &str, subtitle: &str, value: &str) -> GtkBox {
    let val = Label::new(Some(value));
    val.add_css_class("settings-row-desc");
    val.set_valign(Align::Center);
    val.set_selectable(true);
    setting_row(title, subtitle, &val)
}

pub fn toggle_row(
    title: &str,
    subtitle: &str,
    initial: bool,
    on_change: impl Fn(bool) + 'static,
) -> GtkBox {
    let sw = Switch::new();
    sw.set_active(initial);
    sw.set_valign(Align::Center);
    sw.connect_state_set(move |_, active| {
        on_change(active);
        glib::Propagation::Proceed
    });
    setting_row(title, subtitle, &sw)
}

/// Smooth macOS slider row with 45ms debounce so dragging never floods disk/IPC on every pixel.
pub fn slider_row(
    title: &str,
    subtitle: &str,
    min: f64,
    max: f64,
    step: f64,
    initial: f64,
    digits: i32,
    on_change: impl Fn(f64) + 'static,
) -> GtkBox {
    let scale = Scale::with_range(Orientation::Horizontal, min, max, step);
    scale.add_css_class("settings-scale");
    scale.set_width_request(185);
    scale.set_draw_value(true);
    scale.set_digits(digits);
    scale.set_value(initial.clamp(min, max));

    let cb = Rc::new(on_change);
    let generation = Rc::new(Cell::new(0u64));
    scale.connect_value_changed(move |s| {
        let val = s.value();
        let my_gen = generation.get().wrapping_add(1);
        generation.set(my_gen);
        let cb_c = Rc::clone(&cb);
        let gen_c = Rc::clone(&generation);
        glib::timeout_add_local_once(Duration::from_millis(45), move || {
            if gen_c.get() == my_gen {
                cb_c(val);
            }
        });
    });
    setting_row(title, subtitle, &scale)
}

pub fn dropdown_row(
    title: &str,
    subtitle: &str,
    items: &[&str],
    initial_idx: u32,
    on_change: impl Fn(u32) + 'static,
) -> GtkBox {
    let dd = DropDown::new(Some(StringList::new(items)), gtk4::Expression::NONE);
    dd.set_valign(Align::Center);
    dd.set_selected(initial_idx.min(items.len().saturating_sub(1) as u32));
    dd.connect_selected_notify(move |d| on_change(d.selected()));
    setting_row(title, subtitle, &dd)
}

/// Runs a blocking system/D-Bus/disk operation on a background thread and delivers
/// the result safely back to the GTK main thread without freezing the UI.
pub fn run_async<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
    on_done: impl FnOnce(T) + 'static,
) {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(work());
    });
    let mut callback = Some(on_done);
    glib::timeout_add_local(Duration::from_millis(20), move || match rx.try_recv() {
        Ok(result) => {
            if let Some(cb) = callback.take() {
                cb(result);
            }
            glib::ControlFlow::Break
        }
        Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
        Err(mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
    });
}

/// Shows a clean modal confirmation dialog for destructive or critical actions.
pub fn confirm_action_dialog(
    parent: &ApplicationWindow,
    title: &str,
    body: &str,
    confirm_label: &str,
    destructive: bool,
    on_confirm: impl FnOnce() + 'static,
) {
    let dialog = Window::builder()
        .transient_for(parent)
        .modal(true)
        .resizable(false)
        .title(title)
        .default_width(370)
        .build();
    dialog.add_css_class("neon-settings-window");

    let vbox = GtkBox::new(Orientation::Vertical, 12);
    vbox.set_margin_top(18);
    vbox.set_margin_bottom(18);
    vbox.set_margin_start(20);
    vbox.set_margin_end(20);

    let heading = Label::new(Some(title));
    heading.add_css_class("settings-section-title");
    heading.set_halign(Align::Start);

    let msg = Label::new(Some(body));
    msg.add_css_class("settings-row-desc");
    msg.set_halign(Align::Start);
    msg.set_wrap(true);

    let actions = GtkBox::new(Orientation::Horizontal, 8);
    actions.set_halign(Align::End);
    actions.set_margin_top(6);

    let cancel_btn = Button::with_label("Cancel");
    cancel_btn.add_css_class("settings-secondary-btn");
    {
        let dlg = dialog.clone();
        cancel_btn.connect_clicked(move |_| dlg.close());
    }

    let confirm_btn = Button::with_label(confirm_label);
    confirm_btn.add_css_class(if destructive {
        "settings-destructive-btn"
    } else {
        "settings-action-btn"
    });
    let cb = Rc::new(RefCell::new(Some(on_confirm)));
    {
        let dlg = dialog.clone();
        confirm_btn.connect_clicked(move |_| {
            dlg.close();
            if let Some(f) = cb.borrow_mut().take() {
                f();
            }
        });
    }

    actions.append(&cancel_btn);
    actions.append(&confirm_btn);
    vbox.append(&heading);
    vbox.append(&msg);
    vbox.append(&actions);

    dialog.set_child(Some(&vbox));
    dialog.present();
}

/// Shows a display configuration confirmation dialog with a live countdown timer.
pub fn display_confirm_countdown_dialog(
    parent: &ApplicationWindow,
    seconds: u32,
    on_keep: impl FnOnce() + 'static,
    on_revert: impl FnOnce() + 'static,
) {
    let dialog = Window::builder()
        .transient_for(parent)
        .modal(true)
        .resizable(false)
        .title("Keep Display Changes?")
        .default_width(390)
        .build();
    dialog.add_css_class("neon-settings-window");

    let vbox = GtkBox::new(Orientation::Vertical, 12);
    vbox.set_margin_top(18);
    vbox.set_margin_bottom(18);
    vbox.set_margin_start(20);
    vbox.set_margin_end(20);

    let heading = Label::new(Some("Keep these display settings?"));
    heading.add_css_class("settings-section-title");
    heading.set_halign(Align::Start);

    let countdown_lbl = Label::new(Some(&format!(
        "Previous display configuration will be restored automatically in {seconds} seconds."
    )));
    countdown_lbl.add_css_class("settings-row-desc");
    countdown_lbl.set_halign(Align::Start);
    countdown_lbl.set_wrap(true);

    let actions = GtkBox::new(Orientation::Horizontal, 8);
    actions.set_halign(Align::End);
    actions.set_margin_top(6);

    let settled = Rc::new(Cell::new(false));
    let keep_cb = Rc::new(RefCell::new(Some(on_keep)));
    let revert_cb = Rc::new(RefCell::new(Some(on_revert)));

    let revert_btn = Button::with_label("Revert");
    revert_btn.add_css_class("settings-secondary-btn");
    {
        let dlg = dialog.clone();
        let settled_c = Rc::clone(&settled);
        let revert_c = Rc::clone(&revert_cb);
        revert_btn.connect_clicked(move |_| {
            if !settled_c.replace(true) {
                if let Some(f) = revert_c.borrow_mut().take() {
                    f();
                }
            }
            dlg.close();
        });
    }

    let keep_btn = Button::with_label("Keep Changes");
    keep_btn.add_css_class("settings-action-btn");
    {
        let dlg = dialog.clone();
        let settled_c = Rc::clone(&settled);
        let keep_c = Rc::clone(&keep_cb);
        keep_btn.connect_clicked(move |_| {
            if !settled_c.replace(true) {
                if let Some(f) = keep_c.borrow_mut().take() {
                    f();
                }
            }
            dlg.close();
        });
    }

    {
        let settled_c = Rc::clone(&settled);
        let revert_c = Rc::clone(&revert_cb);
        dialog.connect_close_request(move |_| {
            if !settled_c.replace(true) {
                if let Some(f) = revert_c.borrow_mut().take() {
                    f();
                }
            }
            glib::Propagation::Proceed
        });
    }

    let remaining = Rc::new(Cell::new(seconds));
    {
        let dlg = dialog.clone();
        let lbl = countdown_lbl.clone();
        let settled_c = Rc::clone(&settled);
        let revert_c = Rc::clone(&revert_cb);
        let rem_c = Rc::clone(&remaining);
        glib::timeout_add_local(Duration::from_secs(1), move || {
            if settled_c.get() {
                return glib::ControlFlow::Break;
            }
            let next = rem_c.get().saturating_sub(1);
            rem_c.set(next);
            if next == 0 {
                if !settled_c.replace(true) {
                    if let Some(f) = revert_c.borrow_mut().take() {
                        f();
                    }
                }
                dlg.close();
                return glib::ControlFlow::Break;
            }
            lbl.set_text(&format!(
                "Previous display configuration will be restored automatically in {next} seconds."
            ));
            glib::ControlFlow::Continue
        });
    }

    actions.append(&revert_btn);
    actions.append(&keep_btn);
    vbox.append(&heading);
    vbox.append(&countdown_lbl);
    vbox.append(&actions);

    dialog.set_child(Some(&vbox));
    dialog.present();
}
