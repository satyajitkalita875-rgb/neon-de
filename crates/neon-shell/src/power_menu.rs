//! NEON DE Fullscreen Centered Power Menu (`zwlr_layer_shell_v1` `Layer::Overlay`).
//!
//! Opens across the full display over a real GPU-blurred backdrop (rendered by `neon-renderer`
//! for the `"neon-power-menu"` namespace) with a simple, clean centered row matching Quick Settings:
//! **Lock · Sleep · Log Out · Restart · Shut Down**.

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};

use gtk4::{
    Align, Application, ApplicationWindow, Box as GtkBox, Button, EventControllerKey, GestureClick,
    Image, Label, Orientation, gdk, glib, prelude::*,
};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use neon_ipc::IpcRequest;
use neon_system::{SystemAction, SystemActionSender};

use crate::ipc_bridge::IpcCommandSender;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfirmAction {
    LogOut,
    Restart,
    ShutDown,
}

impl ConfirmAction {
    fn prompt(self) -> &'static str {
        match self {
            Self::LogOut => "Log out of NEON DE?",
            Self::Restart => "Restart computer now?",
            Self::ShutDown => "Shut down computer now?",
        }
    }

    fn button_label(self) -> &'static str {
        match self {
            Self::LogOut => "Log Out",
            Self::Restart => "Restart",
            Self::ShutDown => "Shut Down",
        }
    }

    fn is_danger(self) -> bool {
        matches!(self, Self::Restart | Self::ShutDown)
    }
}

/// Simple fullscreen centered Power Menu overlay.
#[derive(Clone)]
pub struct PowerMenuOverlay {
    window: ApplicationWindow,
    ipc: IpcCommandSender,
    card: GtkBox,
    confirm_box: GtkBox,
    hint_label: Label,
    pending_action: Rc<RefCell<Option<ConfirmAction>>>,
    is_open: Rc<Cell<bool>>,
    anim_gen: Rc<Cell<u64>>,
}

impl PowerMenuOverlay {
    pub fn build(app: &Application, ipc: IpcCommandSender, sys_tx: SystemActionSender) -> Self {
        let window = ApplicationWindow::builder()
            .application(app)
            .title("NEON DE Power Menu")
            .decorated(false)
            .build();
        window.add_css_class("neon-powermenu-window");

        if gtk4_layer_shell::is_supported() {
            window.init_layer_shell();
            window.set_namespace(Some("neon-power-menu"));
            window.set_layer(Layer::Overlay);
            window.set_anchor(Edge::Top, true);
            window.set_anchor(Edge::Bottom, true);
            window.set_anchor(Edge::Left, true);
            window.set_anchor(Edge::Right, true);
            window.set_exclusive_zone(-1);
            window.set_keyboard_mode(KeyboardMode::None);
        }

        let is_open = Rc::new(Cell::new(false));
        let anim_gen = Rc::new(Cell::new(0u64));
        let pending_action: Rc<RefCell<Option<ConfirmAction>>> = Rc::new(RefCell::new(None));

        // Fullscreen dimmed backdrop (real GPU blur is rendered behind this layer by neon-renderer)
        let backdrop = GtkBox::new(Orientation::Vertical, 0);
        backdrop.add_css_class("neon-power-menu-backdrop");
        backdrop.set_hexpand(true);
        backdrop.set_vexpand(true);
        backdrop.set_halign(Align::Fill);
        backdrop.set_valign(Align::Fill);

        // Simple centered boxless container right in the middle of the full blurred screen
        let card = GtkBox::new(Orientation::Vertical, 18);
        card.add_css_class("neon-power-menu-card");
        card.set_halign(Align::Center);
        card.set_valign(Align::Center);
        card.set_hexpand(true);
        card.set_vexpand(true);

        let btn_row = GtkBox::new(Orientation::Horizontal, 28);
        btn_row.set_homogeneous(true);
        btn_row.set_halign(Align::Center);
        btn_row.set_valign(Align::Center);

        let lock_btn = build_pm_button("system-lock-screen-symbolic", "Lock", false);
        let sleep_btn = build_pm_button("weather-clear-night-symbolic", "Sleep", false);
        let logout_btn = build_pm_button("system-log-out-symbolic", "Log Out", false);
        let restart_btn = build_pm_button("system-reboot-symbolic", "Restart", false);
        let shutdown_btn = build_pm_button("system-shutdown-symbolic", "Shut Down", true);

        btn_row.append(&lock_btn);
        btn_row.append(&sleep_btn);
        btn_row.append(&logout_btn);
        btn_row.append(&restart_btn);
        btn_row.append(&shutdown_btn);
        card.append(&btn_row);

        // Inline confirmation strip shown when Log Out / Restart / Shut Down is selected
        let confirm_box = GtkBox::new(Orientation::Horizontal, 10);
        confirm_box.add_css_class("neon-power-menu-confirm");
        confirm_box.set_halign(Align::Fill);
        confirm_box.set_valign(Align::Center);
        confirm_box.set_visible(false);

        let confirm_lbl = Label::new(Some("Shut down computer now?"));
        confirm_lbl.add_css_class("neon-cc-title");
        confirm_lbl.set_hexpand(true);
        confirm_lbl.set_halign(Align::Start);

        let cancel_btn = Button::with_label("Cancel");
        cancel_btn.add_css_class("neon-cc-small-btn");

        let ok_btn = Button::with_label("Confirm");
        ok_btn.add_css_class("neon-power-confirm-btn");

        confirm_box.append(&confirm_lbl);
        confirm_box.append(&cancel_btn);
        confirm_box.append(&ok_btn);
        card.append(&confirm_box);

        let hint_label = Label::new(Some("Press Esc or click outside to cancel"));
        hint_label.add_css_class("neon-cc-sub");
        hint_label.set_halign(Align::Center);
        card.append(&hint_label);

        backdrop.append(&card);
        window.set_child(Some(&backdrop));
        window.set_visible(false);

        let menu = Self {
            window,
            ipc: ipc.clone(),
            card,
            confirm_box,
            hint_label,
            pending_action,
            is_open,
            anim_gen,
        };

        // Direct actions: Lock & Sleep
        {
            let m = menu.clone();
            let ipc_c = ipc.clone();
            lock_btn.connect_clicked(move |_| {
                m.close();
                ipc_c.send(IpcRequest::LockSession);
            });
        }
        {
            let m = menu.clone();
            let tx = sys_tx.clone();
            sleep_btn.connect_clicked(move |_| {
                m.close();
                tx.send(SystemAction::InvokePowerAction("Suspend".into()));
            });
        }

        // Confirmed actions: Log Out, Restart, Shut Down
        for (btn, act) in [
            (&logout_btn, ConfirmAction::LogOut),
            (&restart_btn, ConfirmAction::Restart),
            (&shutdown_btn, ConfirmAction::ShutDown),
        ] {
            let pending = Rc::clone(&menu.pending_action);
            let cbox = menu.confirm_box.clone();
            let hint = menu.hint_label.clone();
            let lbl = confirm_lbl.clone();
            let ok = ok_btn.clone();
            btn.connect_clicked(move |_| {
                *pending.borrow_mut() = Some(act);
                lbl.set_text(act.prompt());
                ok.set_label(act.button_label());
                if act.is_danger() {
                    ok.add_css_class("danger");
                } else {
                    ok.remove_css_class("danger");
                }
                hint.set_visible(false);
                cbox.set_visible(true);
            });
        }

        {
            let pending = Rc::clone(&menu.pending_action);
            let cbox = menu.confirm_box.clone();
            let hint = menu.hint_label.clone();
            cancel_btn.connect_clicked(move |_| {
                *pending.borrow_mut() = None;
                cbox.set_visible(false);
                hint.set_visible(true);
            });
        }

        {
            let m = menu.clone();
            let pending = Rc::clone(&menu.pending_action);
            let tx = sys_tx;
            ok_btn.connect_clicked(move |_| {
                let Some(act) = pending.borrow_mut().take() else {
                    return;
                };
                m.close();
                match act {
                    ConfirmAction::LogOut => tx.send(SystemAction::LogOut),
                    ConfirmAction::Restart => {
                        tx.send(SystemAction::InvokePowerAction("Reboot".into()))
                    }
                    ConfirmAction::ShutDown => {
                        tx.send(SystemAction::InvokePowerAction("PowerOff".into()))
                    }
                }
            });
        }

        // Click anywhere outside the centered card dismisses the Power Menu
        {
            let click = GestureClick::new();
            click.set_button(0);
            let m = menu.clone();
            let card_c = menu.card.clone();
            click.connect_pressed(move |gesture, _, x, y| {
                if !m.is_open.get() {
                    return;
                }
                let inside = gesture
                    .widget()
                    .and_then(|w| w.pick(x, y, gtk4::PickFlags::DEFAULT))
                    .is_some_and(|picked| {
                        picked == card_c.clone().upcast::<gtk4::Widget>()
                            || picked.is_ancestor(&card_c)
                    });
                if !inside {
                    m.close();
                }
            });
            backdrop.add_controller(click);
        }

        // Escape dismisses cleanly
        {
            let key_ctrl = EventControllerKey::new();
            let m = menu.clone();
            key_ctrl.connect_key_pressed(move |_, key, _, _| {
                if key == gdk::Key::Escape && m.is_open.get() {
                    m.close();
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            });
            menu.window.add_controller(key_ctrl);
        }

        menu
    }

    pub fn open(&self) {
        let epoch = self.anim_gen.get().wrapping_add(1);
        self.anim_gen.set(epoch);
        self.is_open.set(true);
        *self.pending_action.borrow_mut() = None;
        self.confirm_box.set_visible(false);
        self.hint_label.set_visible(true);

        if gtk4_layer_shell::is_supported() {
            self.window.set_keyboard_mode(KeyboardMode::Exclusive);
        }

        self.ipc.send(IpcRequest::AnimateOverlay {
            namespace: "neon-power-menu".into(),
            opening: true,
        });
        self.window.set_visible(true);
        self.window.present();
    }

    pub fn close(&self) {
        if !self.is_open.get() && !self.window.is_visible() {
            return;
        }
        let epoch = self.anim_gen.get().wrapping_add(1);
        self.anim_gen.set(epoch);
        self.is_open.set(false);
        *self.pending_action.borrow_mut() = None;
        self.confirm_box.set_visible(false);

        self.ipc.send(IpcRequest::AnimateOverlay {
            namespace: "neon-power-menu".into(),
            opening: false,
        });

        let win = self.window.clone();
        let gen_c = Rc::clone(&self.anim_gen);
        glib::timeout_add_local_once(Duration::from_millis(150), move || {
            if gen_c.get() == epoch {
                if gtk4_layer_shell::is_supported() {
                    win.set_keyboard_mode(KeyboardMode::None);
                }
                win.set_visible(false);
            }
        });
    }
}

fn build_pm_button(icon_name: &str, label: &str, danger: bool) -> Button {
    let btn = Button::new();
    btn.add_css_class("neon-pm-action-btn");
    if danger {
        btn.add_css_class("danger");
    }
    btn.set_size_request(96, 92);

    let col = GtkBox::new(Orientation::Vertical, 10);
    col.set_halign(Align::Center);
    col.set_valign(Align::Center);
    col.set_hexpand(true);
    col.set_vexpand(true);

    let icon = Image::from_icon_name(icon_name);
    icon.set_pixel_size(34);
    icon.set_halign(Align::Center);

    let lbl = Label::new(Some(label));
    lbl.add_css_class("neon-pm-action-lbl");
    lbl.set_halign(Align::Center);

    col.append(&icon);
    col.append(&lbl);
    btn.set_child(Some(&col));
    btn
}
